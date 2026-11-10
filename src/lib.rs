//! Rojo for Flareite: Roblox projects made from files.
//!
//! - **Serving.** `rojo serve` runs for one project at a time, from the
//!   status bar or the Projects view; its output goes to the extension's
//!   log as it's written (`spawn-streaming`), the port it listens on shows
//!   in the status bar, and its errors are shown. Before the first serve
//!   with a version of Rojo, it offers to install Rojo's Studio plugin.
//! - **Sourcemaps.** `rojo sourcemap --watch` keeps `sourcemap.json` up to
//!   date for luau-lsp (the Luau extension). It's read for the Instances
//!   view, and for explorer marks saying what each file becomes.
//! - **Commands.** Build (and open the place in Studio), upload (with an
//!   Open Cloud API key kept in the system's credential store), syncback,
//!   format, and new projects; installing Rojo with Rokit; Rokit's tools,
//!   and Wally's packages (with `wally-package-types` after).
//! - **Schemas** for project, meta, and model files, made from Roblox's API
//!   (`schema.rs`).

mod projects;
mod schema;
mod sourcemap;
mod tools;

use std::cell::RefCell;
use std::collections::HashMap;

use flareite_extension_api::downloads;
use flareite_extension_api::explorer::{self, FileMark};
use flareite_extension_api::processes::{self, ProcessError, ProcessOptions, ProcessOutput};
use flareite_extension_api::prompts::{self, AnswerValue, ConfirmOptions, InputOptions, PickItem};
use flareite_extension_api::schemas::{self, Schema, SchemaFormat};
use flareite_extension_api::storage::{self, StorageScope};
use flareite_extension_api::types::{
  Activation, CommandContext, ConfigValue, DocumentChange, DocumentId, DocumentInfo, LogLevel,
  MessageLevel, StatusItem,
};
use flareite_extension_api::views::{self, Welcome, WelcomeButton};
use flareite_extension_api::{
  Guest, export_extension, host, links, secrets, timers, workbench, workspace,
};

use crate::projects::{Project, join, relative_to};
use crate::tools::Tool;

const OUTPUT: &str = "flareite.rojo.output";
const FINISHED: &str = "flareite.rojo.finished";
const ANSWERED: &str = "flareite.rojo.answered";
const DOWNLOADED: &str = "flareite.rojo.downloaded";
const FILES_CHANGED: &str = "flareite.rojo.filesChanged";
const TIMER: &str = "flareite.rojo.timer";
const PROJECTS_VIEW: &str = "projects";
const INSTANCES_VIEW: &str = "instances";
const STATUS: &str = "serve";
const API_KEY: &str = "open-cloud-api-key";
const INSTALL_GUIDE: &str = "https://rojo.space/docs/v7/getting-started/installation/";
/// How long after Wally installs packages `wally-package-types` waits for
/// the sourcemap to have them, at most.
const PACKAGE_TYPES_WAIT_MS: u32 = 5_000;

/// What the settings ask for.
struct Config {
  project: String,
  sourcemap: bool,
  include_non_scripts: bool,
  serve_port: i64,
  explorer_marks: bool,
  wally_prompt: bool,
}

impl Config {
  fn read() -> Self {
    let text = |key: &str, default: &str| match host::configuration(key) {
      Some(ConfigValue::Text(text)) if !text.trim().is_empty() => text.trim().to_owned(),
      _ => default.to_owned(),
    };
    let flag = |key: &str| !matches!(host::configuration(key), Some(ConfigValue::Boolean(false)));
    Self {
      project: text("project", "default.project.json")
        .replace('\\', "/")
        .trim_start_matches("./")
        .to_owned(),
      sourcemap: flag("sourcemap"),
      include_non_scripts: flag("sourcemap_include_non_scripts"),
      serve_port: match host::configuration("serve_port") {
        Some(ConfigValue::Integer(port)) => port,
        _ => 0,
      },
      explorer_marks: flag("explorer_marks"),
      wally_prompt: flag("wally_prompt"),
    }
  }
}

/// The `rojo serve` running.
struct Serve {
  process: u64,
  project: Project,
  address: Option<String>,
  port: Option<String>,
  /// The last error it logged, to say why it stopped.
  last_error: Option<String>,
}

/// The `rojo sourcemap --watch` running, and what for.
struct SourcemapRun {
  process: u64,
  folder: String,
  args: Vec<String>,
}

/// A program run once, and what to do when it ends.
#[derive(Debug, Clone)]
enum Task {
  Build { output: String, open: bool },
  Upload,
  Syncback,
  Format,
  Init { kind: String },
  PluginInstall { version: String },
  RokitInit,
  RokitAdd { global: bool },
  RokitTrust,
  RokitInstall,
  WallyInstall,
  PackageTypes,
}

impl Task {
  /// What it's doing, for messages: "Building", ...
  fn doing(&self) -> &'static str {
    match self {
      Self::Build { .. } => "Building",
      Self::Upload => "Uploading",
      Self::Syncback => "Syncback",
      Self::Format => "Formatting the project",
      Self::Init { .. } => "Making the project",
      Self::PluginInstall { .. } => "Installing the Studio plugin",
      Self::RokitInit | Self::RokitAdd { .. } => "Installing Rojo",
      Self::RokitTrust | Self::RokitInstall => "Installing Rokit's tools",
      Self::WallyInstall => "Installing Wally's packages",
      Self::PackageTypes => "Typing Wally's packages",
    }
  }

  fn program(&self) -> &'static str {
    match self {
      Self::RokitInit | Self::RokitAdd { .. } | Self::RokitTrust | Self::RokitInstall => "rokit",
      Self::WallyInstall => "wally",
      Self::PackageTypes => "wally-package-types",
      _ => "rojo",
    }
  }
}

struct Running {
  task: Task,
  /// Its last lines, for saying why it failed.
  lines: Vec<String>,
}

/// What a project is chosen for.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Action {
  Serve,
  Build { open: bool },
  Upload,
  Syncback,
  Format,
}

/// A question asked, and what the answer is for.
enum Pending {
  Project(Action),
  BuildOutput {
    project: Project,
    open: bool,
  },
  UploadUniverse {
    project: Project,
  },
  UploadPlace {
    project: Project,
    universe: String,
  },
  UploadKey {
    project: Project,
    universe: String,
    place: String,
  },
  SyncbackFile {
    project: Project,
  },
  SyncbackPath {
    project: Project,
  },
  SyncbackConfirm {
    project: Project,
    input: String,
  },
  NewProject,
  Plugin {
    version: String,
  },
  InstallRojo,
  InstallGuide,
  InstallTools {
    tools: Vec<Tool>,
  },
  InstallPackages,
}

#[derive(Default)]
struct State {
  /// The open folder, with `/`.
  folder: Option<String>,
  trusted: bool,
  projects: Vec<Project>,
  watch: Option<u64>,
  serve: Option<Serve>,
  sourcemap: Option<SourcemapRun>,
  /// Sourcemap failures are shown once, until it runs again.
  sourcemap_failure_shown: bool,
  tasks: HashMap<u64, Running>,
  pending: HashMap<u64, Pending>,
  /// Roblox's API and its documentation, as they're downloaded.
  dump_request: Option<u64>,
  docs_request: Option<u64>,
  dump: Option<Vec<u8>>,
  /// `Some(None)` once its download failed.
  docs: Option<Option<Vec<u8>>>,
  /// `wally-package-types` runs at the next sourcemap (or this timer).
  package_types_timer: Option<u64>,
  /// Installing Rojo was offered, this session.
  install_offered: bool,
}

thread_local! {
  static STATE: RefCell<State> = RefCell::default();
}

fn with_state<R>(f: impl FnOnce(&mut State) -> R) -> R {
  STATE.with_borrow_mut(f)
}

fn log(level: LogLevel, message: &str) {
  host::log(level, message);
}

fn warn(message: &str) {
  log(LogLevel::Warn, message);
  host::show_message(MessageLevel::Warning, message);
}

fn info(message: &str) {
  log(LogLevel::Info, message);
  host::show_message(MessageLevel::Info, message);
}

fn options(cwd: &str) -> ProcessOptions {
  ProcessOptions {
    cwd: Some(cwd.to_owned()),
    env: Vec::new(),
    stdin: None,
    timeout_ms: None,
  }
}

/// The open folder, if it's trusted (asking the user to trust it if not).
fn trusted_folder() -> Result<String, String> {
  let folder = with_state(|state| state.folder.clone().map(|f| (f, state.trusted)));
  match folder {
    None => Err("Open a folder with a Rojo project first.".into()),
    Some((_, false)) => {
      workspace::request_trust();
      Err("Rojo runs only in folders you trust.".into())
    }
    Some((folder, true)) => Ok(folder),
  }
}

/// Starts a program, its output going to the log.
fn spawn(program: &str, args: &[String], folder: &str) -> Result<u64, ProcessError> {
  processes::spawn_streaming(program, args, &options(folder), FINISHED, OUTPUT)
}

/// Why a program couldn't run, in words.
fn describe(program: &str, error: &ProcessError) -> String {
  match error {
    ProcessError::NotFound(_) => format!("{program} isn't installed."),
    ProcessError::PermissionDenied => format!(
      "Running {program} isn't allowed: allow the Rojo extension to run programs in the Extensions view."
    ),
    ProcessError::Untrusted => format!("{program} runs only in folders you trust."),
    other => format!("{program} couldn't run: {other:?}"),
  }
}

/// Whether a program's outcome says it isn't installed (for this project,
/// with Rokit's shims).
fn not_installed(outcome: &Result<ProcessOutput, ProcessError>, lines: &[String]) -> bool {
  match outcome {
    Err(ProcessError::NotFound(_)) => true,
    Ok(output) if output.exit_code != Some(0) => lines.iter().any(|line| tools::says_missing(line)),
    _ => false,
  }
}

// --- Settings, the folder, and views ---------------------------------------

/// Looks at the open folder again: its projects, the watch, the sourcemap,
/// and what's shown.
fn refresh_folder() {
  let folder = workspace::folder();
  let path = folder.as_ref().map(|f| f.path.replace('\\', "/"));
  let changed = with_state(|state| {
    let changed = state.folder != path;
    state.folder = path.clone();
    state.trusted = folder.as_ref().is_some_and(|f| f.trusted);
    changed
  });
  if changed {
    if let Some(watch) = with_state(|state| state.watch.take()) {
      workspace::unwatch(watch);
    }
    stop_serving();
    if let Some(path) = &path {
      match workspace::watch(path, FILES_CHANGED) {
        Ok(watch) => with_state(|state| state.watch = Some(watch)),
        Err(error) => log(
          LogLevel::Warn,
          &format!("couldn't watch the folder: {error:?}"),
        ),
      }
    }
  }
  refresh_projects();
  configure_sourcemap(false);
  show_instances();
}

fn refresh_projects() {
  let folder = with_state(|state| state.folder.clone());
  let projects = folder.as_deref().map(projects::find).unwrap_or_default();
  with_state(|state| state.projects = projects);
  show_projects();
  show_status();
}

/// The project sourcemaps are made from and that's served by default: the
/// Project setting's, or the first.
fn main_project() -> Option<Project> {
  let wanted = Config::read().project;
  with_state(|state| {
    state
      .projects
      .iter()
      .find(|project| project.relative == wanted)
      .or_else(|| state.projects.first())
      .cloned()
  })
}

fn show_projects() {
  let (folder, projects, serving) = with_state(|state| {
    (
      state.folder.clone(),
      state.projects.clone(),
      state
        .serve
        .as_ref()
        .map(|serve| (serve.project.path.clone(), serve.port.clone())),
    )
  });
  let items: Vec<views::TreeItem> = projects
    .iter()
    .map(|project| {
      let served = serving
        .as_ref()
        .filter(|(path, _)| *path == project.path)
        .map(|(_, port)| port.clone());
      let description = match &served {
        Some(Some(port)) => format!("{} · serving on {port}", project.relative),
        Some(None) => format!("{} · starting", project.relative),
        None => project.relative.clone(),
      };
      views::TreeItem {
        id: project.relative.clone(),
        parent: None,
        label: project.name.clone(),
        description: Some(description),
        tooltip: Some(format!(
          "{}\n{}",
          project.relative,
          if project.place { "A place" } else { "A model" }
        )),
        icon: views::ItemIcon::Named(if served.is_some() { "play" } else { "file" }.into()),
        mark: None,
        context: Some(
          if served.is_some() {
            "serving"
          } else {
            "project"
          }
          .into(),
        ),
        expansion: views::Expansion::None,
        command: Some("flareite.rojo.openProject".into()),
        argument: Some(project.path.clone()),
      }
    })
    .collect();
  let _ = views::set_items(PROJECTS_VIEW, &items);
  let welcome = match (&folder, projects.is_empty()) {
    (None, _) => Some(Welcome {
      text: "Open a folder with a Rojo project to serve and build it.".into(),
      buttons: Vec::new(),
    }),
    (Some(_), true) => Some(Welcome {
      text: "This folder has no Rojo project (a *.project.json file).".into(),
      buttons: vec![WelcomeButton {
        label: "New Project".into(),
        command: "flareite.rojo.newProject".into(),
        argument: None,
      }],
    }),
    _ => None,
  };
  let _ = views::set_welcome(PROJECTS_VIEW, welcome.as_ref());
}

fn show_status() {
  let (has_projects, serve) = with_state(|state| {
    (
      !state.projects.is_empty(),
      state.serve.as_ref().map(|serve| {
        (
          serve.project.relative.clone(),
          serve.address.clone(),
          serve.port.clone(),
        )
      }),
    )
  });
  let item = match serve {
    Some((project, address, Some(port))) => Some(StatusItem {
      text: format!("Rojo: serving on {port}"),
      tooltip: Some(format!(
        "Serving {project} at {}:{port}: connect from Rojo's plugin in Roblox Studio. Click to stop.",
        address.as_deref().unwrap_or("localhost")
      )),
      command: Some("flareite.rojo.toggleServe".into()),
    }),
    Some((project, _, None)) => Some(StatusItem {
      text: "Rojo: starting…".into(),
      tooltip: Some(format!("Starting to serve {project}. Click to stop.")),
      command: Some("flareite.rojo.toggleServe".into()),
    }),
    None if has_projects => Some(StatusItem {
      text: "Rojo".into(),
      tooltip: Some("Serve the project to Roblox Studio".into()),
      command: Some("flareite.rojo.toggleServe".into()),
    }),
    None => None,
  };
  let _ = workbench::set_status_item(STATUS, item.as_ref());
}

/// Reads `sourcemap.json` again, for the Instances view and the explorer.
fn show_instances() {
  let config = Config::read();
  let folder = with_state(|state| state.folder.clone());
  let root = folder.as_deref().and_then(|folder| {
    let text = flareite_extension_api::files::read_file(&join(folder, "sourcemap.json")).ok()?;
    match sourcemap::parse(&text) {
      Ok(root) => Some(root),
      Err(error) => {
        // Rojo may be writing it; the next change reads it again.
        log(LogLevel::Debug, &error);
        None
      }
    }
  });
  match (&root, &folder) {
    (Some(root), Some(folder)) => {
      let (items, truncated) = sourcemap::items(root, folder);
      let _ = views::set_items(INSTANCES_VIEW, &items);
      let _ = views::set_welcome(INSTANCES_VIEW, None);
      if truncated {
        log(
          LogLevel::Info,
          &format!(
            "the Instances view shows the first {} instances",
            sourcemap::MAX_ITEMS
          ),
        );
      }
      let marks: Vec<FileMark> = if config.explorer_marks {
        sourcemap::marks(root, folder)
      } else {
        Vec::new()
      };
      if let Err(error) = explorer::set_file_marks(&marks) {
        log(LogLevel::Warn, &format!("couldn't mark files: {error:?}"));
      }
    }
    _ => {
      let _ = views::set_items(INSTANCES_VIEW, &[]);
      let _ = explorer::set_file_marks(&[]);
      let text = if folder.is_none() {
        "Open a folder with a Rojo project to see its instances."
      } else if !config.sourcemap {
        "Turn on the Sourcemap setting to see the project's instances here."
      } else {
        "The project's instances show here once Rojo has made sourcemap.json."
      };
      let buttons = if folder.is_some() && config.sourcemap {
        vec![WelcomeButton {
          label: "Regenerate Sourcemap".into(),
          command: "flareite.rojo.regenerateSourcemap".into(),
          argument: None,
        }]
      } else {
        Vec::new()
      };
      let _ = views::set_welcome(
        INSTANCES_VIEW,
        Some(&Welcome {
          text: text.into(),
          buttons,
        }),
      );
    }
  }
}

// --- Sourcemaps ------------------------------------------------------------

fn sourcemap_args(project: &Project, config: &Config) -> Vec<String> {
  let mut args: Vec<String> = [
    "sourcemap",
    &project.relative,
    "--watch",
    "--output",
    "sourcemap.json",
    "--color",
    "never",
  ]
  .map(str::to_owned)
  .to_vec();
  if config.include_non_scripts {
    args.push("--include-non-scripts".into());
  }
  args
}

/// Starts, stops, or restarts `rojo sourcemap --watch` as the settings and
/// the folder call for; `again` restarts it even if it's running as asked.
fn configure_sourcemap(again: bool) {
  let config = Config::read();
  let (folder, trusted) = with_state(|state| (state.folder.clone(), state.trusted));
  let wanted = match (folder, main_project()) {
    (Some(folder), Some(project)) if config.sourcemap && trusted => {
      Some((folder, sourcemap_args(&project, &config)))
    }
    _ => None,
  };
  let running = with_state(|state| {
    let same = match (&state.sourcemap, &wanted) {
      (Some(run), Some((folder, args))) => &run.folder == folder && &run.args == args,
      _ => false,
    };
    if same && !again {
      return true;
    }
    if let Some(run) = state.sourcemap.take() {
      // Its end is no longer the running one's, and says nothing.
      processes::kill(run.process);
    }
    false
  });
  let Some((folder, args)) = wanted.filter(|_| !running) else {
    return;
  };
  match spawn("rojo", &args, &folder) {
    Ok(process) => {
      log(
        LogLevel::Info,
        &format!("making sourcemaps: rojo {}", args.join(" ")),
      );
      with_state(|state| {
        state.sourcemap = Some(SourcemapRun {
          process,
          folder,
          args,
        });
      });
    }
    Err(error) => sourcemap_failed(
      &describe("Rojo", &error),
      matches!(error, ProcessError::NotFound(_)),
    ),
  }
}

/// Says why sourcemaps aren't made, once; offers to install Rojo if it's
/// missing.
fn sourcemap_failed(message: &str, missing: bool) {
  log(LogLevel::Warn, message);
  let first = with_state(|state| !std::mem::replace(&mut state.sourcemap_failure_shown, true));
  if !first {
    return;
  }
  if missing {
    offer_install_rojo("Rojo isn't installed, so sourcemap.json isn't kept up to date.");
  } else {
    host::show_message(
      MessageLevel::Warning,
      &format!("{message} (Turn off the Rojo extension's Sourcemap setting to make it yourself.)"),
    );
  }
}

// --- Serving ---------------------------------------------------------------

fn stop_serving() {
  if let Some(serve) = with_state(|state| state.serve.take()) {
    processes::kill(serve.process);
    log(
      LogLevel::Info,
      &format!("stopped serving {}", serve.project.relative),
    );
  }
  show_status();
  show_projects();
}

fn serve(project: Project) -> Result<(), String> {
  let folder = trusted_folder()?;
  stop_serving();
  let mut args: Vec<String> = ["serve", &project.relative, "--color", "never"]
    .map(str::to_owned)
    .to_vec();
  let port = Config::read().serve_port;
  if port > 0 {
    args.push("--port".into());
    args.push(port.to_string());
  }
  match spawn("rojo", &args, &folder) {
    Ok(process) => {
      log(LogLevel::Info, &format!("serving: rojo {}", args.join(" ")));
      with_state(|state| {
        state.serve = Some(Serve {
          process,
          project,
          address: None,
          port: None,
          last_error: None,
        });
      });
      show_status();
      show_projects();
      Ok(())
    }
    Err(error @ ProcessError::NotFound(_)) => {
      offer_install_rojo(&describe("Rojo", &error));
      Ok(())
    }
    Err(error) => Err(describe("Rojo", &error)),
  }
}

/// A version's major and minor numbers: "7.4.1" is (7, 4).
fn major_minor(version: &str) -> Option<(u32, u32)> {
  let mut parts = version.split(['.', '-']);
  Some((parts.next()?.parse().ok()?, parts.next()?.parse().ok()?))
}

/// `rojo --version`'s version, such as "7.4.1".
fn rojo_version(folder: &str) -> Option<String> {
  let output = processes::run("rojo", &["--version".to_owned()], &options(folder)).ok()?;
  if output.exit_code != Some(0) {
    return None;
  }
  String::from_utf8_lossy(&output.stdout)
    .split_whitespace()
    .nth(1)
    .map(str::to_owned)
}

/// Offers to install Rojo's Studio plugin, once for each version of Rojo.
fn offer_plugin(folder: &str) {
  let Some(version) = rojo_version(folder) else {
    return;
  };
  let offered = storage::get(StorageScope::Global, "plugin-offered")
    .and_then(|value| serde_json::from_str::<String>(&value).ok());
  if offered.as_deref() == Some(version.as_str()) {
    return;
  }
  ask_confirm(
    "Install Rojo's plugin for Roblox Studio?",
    Some(format!(
      "Studio syncs with `rojo serve` through it, and it should be Rojo's version ({version}). Installing replaces the one Studio has."
    )),
    &["Install Plugin", "It's Installed"],
    false,
    Pending::Plugin { version },
  );
}

/// What a line of `rojo serve` says: where it listens, or an error. Once
/// it listens, the Studio plugin is offered.
fn serve_line(text: &str) {
  let trimmed = text.trim();
  let (key, value) = trimmed.split_once(':').unwrap_or_default();
  let value = value.trim();
  let error = error_message(text).map(str::to_owned);
  let (changed, listening) = with_state(|state| {
    let Some(serve) = &mut state.serve else {
      return (false, None);
    };
    if error.is_some() {
      serve.last_error.clone_from(&error);
    }
    match key {
      "Address" if !value.is_empty() => serve.address = Some(value.to_owned()),
      "Port" if !value.is_empty() && serve.port.is_none() => {
        serve.port = Some(value.to_owned());
        return (true, state.folder.clone());
      }
      _ => return (false, None),
    }
    (true, None)
  });
  if changed {
    show_status();
    show_projects();
  }
  if let Some(error) = error {
    host::show_message(MessageLevel::Warning, &format!("Rojo: {error}"));
  }
  if let Some(folder) = listening {
    offer_plugin(&folder);
  }
}

/// An error line Rojo logged (`[ERROR librojo::...] message`), as its message.
fn error_message(line: &str) -> Option<&str> {
  let rest = line.strip_prefix("[ERROR")?;
  let (_, message) = rest.split_once(']')?;
  Some(message.trim()).filter(|message| !message.is_empty())
}

// --- Programs' output and ends ---------------------------------------------

fn output() {
  let Some(lines) = processes::lines() else {
    return;
  };
  let process = lines.process;
  enum Whose {
    Serve,
    Sourcemap,
    Task(&'static str),
    Gone,
  }
  let whose = with_state(|state| {
    if state
      .serve
      .as_ref()
      .is_some_and(|serve| serve.process == process)
    {
      Whose::Serve
    } else if state
      .sourcemap
      .as_ref()
      .is_some_and(|run| run.process == process)
    {
      Whose::Sourcemap
    } else if let Some(running) = state.tasks.get(&process) {
      Whose::Task(running.task.program())
    } else {
      Whose::Gone
    }
  });
  if lines.dropped > 0 {
    log(
      LogLevel::Warn,
      &format!("{} lines of output were left out", lines.dropped),
    );
  }
  for line in &lines.lines {
    let plain = strip_ansi(&line.text);
    let text = plain.trim_end();
    if text.trim().is_empty() {
      continue;
    }
    // Rojo and Rokit write progress to standard error too.
    let lower = text.trim_start().to_lowercase();
    let level = if line.stderr
      && (lower.starts_with("[error") || lower.starts_with("[warn") || lower.starts_with("error"))
    {
      LogLevel::Warn
    } else {
      LogLevel::Info
    };
    match whose {
      Whose::Serve => {
        log(level, &format!("serve: {text}"));
        serve_line(text);
      }
      Whose::Sourcemap => {
        log(level, &format!("sourcemap: {text}"));
        if let Some(message) = error_message(text) {
          let first =
            with_state(|state| !std::mem::replace(&mut state.sourcemap_failure_shown, true));
          if first {
            host::show_message(
              MessageLevel::Warning,
              &format!("Rojo couldn't make the sourcemap: {message}"),
            );
          }
        }
      }
      Whose::Task(program) => {
        log(level, &format!("{program}: {text}"));
        with_state(|state| {
          if let Some(running) = state.tasks.get_mut(&process) {
            running.lines.push(text.to_owned());
            if running.lines.len() > 40 {
              running.lines.remove(0);
            }
          }
        });
      }
      Whose::Gone => log(LogLevel::Debug, text),
    }
  }
}

/// Text without terminal color codes (`ESC [ 31 m`), which Rokit writes even
/// when it isn't writing to a terminal.
fn strip_ansi(text: &str) -> String {
  let mut out = String::with_capacity(text.len());
  let mut chars = text.chars();
  while let Some(c) = chars.next() {
    if c == '\u{1b}' {
      // ESC, '[', parameters, then a final letter.
      if chars.next() == Some('[') {
        for c in chars.by_ref() {
          if c.is_ascii_alphabetic() {
            break;
          }
        }
      }
    } else {
      out.push(c);
    }
  }
  out
}

/// The line that best says why a program failed: Rojo's last logged error,
/// a command line error (`error: ...`), or its last line.
fn why(lines: &[String], outcome: &Result<ProcessOutput, ProcessError>) -> String {
  lines
    .iter()
    .rev()
    .find_map(|line| error_message(line))
    .or_else(|| {
      lines.iter().rev().find_map(|line| {
        let line = line.trim();
        line
          .get(..6)
          .filter(|start| start.eq_ignore_ascii_case("error:"))
          .map(|_| line[6..].trim())
      })
    })
    .map(str::to_owned)
    .or_else(|| {
      lines
        .iter()
        .rev()
        .map(|line| line.trim())
        .find(|line| !line.is_empty())
        .map(str::to_owned)
    })
    .unwrap_or_else(|| match outcome {
      Ok(output) => output.exit_code.map_or_else(
        || "it was stopped".into(),
        |code| format!("exit code {code}"),
      ),
      Err(error) => format!("{error:?}"),
    })
}

fn finished() {
  let Some(finished) = processes::finished() else {
    return;
  };
  let process = finished.process;
  let outcome = finished.outcome;
  let serve = with_state(|state| {
    if state
      .serve
      .as_ref()
      .is_some_and(|serve| serve.process == process)
    {
      state.serve.take()
    } else {
      None
    }
  });
  if let Some(serve) = serve {
    show_status();
    show_projects();
    if !matches!(outcome, Err(ProcessError::Killed)) {
      if not_installed(&outcome, &[]) {
        offer_install_rojo("Rojo isn't installed.");
      } else {
        let why = serve
          .last_error
          .clone()
          .unwrap_or_else(|| why(&[], &outcome));
        // Its last error was just shown.
        let message = format!("Rojo stopped serving {}: {why}", serve.project.relative);
        if serve.last_error.is_some() {
          log(LogLevel::Warn, &message);
          host::show_message(
            MessageLevel::Warning,
            &format!("Rojo stopped serving {}.", serve.project.relative),
          );
        } else {
          warn(&message);
        }
      }
    }
    return;
  }
  let sourcemap = with_state(|state| {
    let current = state
      .sourcemap
      .as_ref()
      .is_some_and(|run| run.process == process);
    if current {
      state.sourcemap = None;
    }
    current
  });
  if sourcemap {
    match &outcome {
      Err(ProcessError::Killed) => {}
      Err(error) => sourcemap_failed(
        &describe("Rojo", error),
        matches!(error, ProcessError::NotFound(_)),
      ),
      Ok(_) => sourcemap_failed(
        &format!("Rojo stopped making sourcemaps: {}", why(&[], &outcome)),
        false,
      ),
    }
    return;
  }
  let Some(running) = with_state(|state| state.tasks.remove(&process)) else {
    return;
  };
  task_finished(running, &outcome);
}

/// Starts a program for a task.
fn run_task(task: Task, args: Vec<String>) -> Result<(), String> {
  let folder = trusted_folder()?;
  let program = task.program();
  match spawn(program, &args, &folder) {
    Ok(process) => {
      // Uploads' arguments have the API key.
      if !matches!(task, Task::Upload) {
        log(
          LogLevel::Info,
          &format!("{}: {program} {}", task.doing(), args.join(" ")),
        );
      }
      with_state(|state| {
        state.tasks.insert(
          process,
          Running {
            task,
            lines: Vec::new(),
          },
        );
      });
      Ok(())
    }
    Err(error @ ProcessError::NotFound(_)) if program == "rojo" => {
      offer_install_rojo(&describe("Rojo", &error));
      Ok(())
    }
    Err(error) => Err(describe(program, &error)),
  }
}

fn task_finished(running: Running, outcome: &Result<ProcessOutput, ProcessError>) {
  let Running { task, lines } = running;
  let ok = matches!(outcome, Ok(output) if output.exit_code == Some(0));
  if !ok {
    if matches!(outcome, Err(ProcessError::Killed)) {
      return;
    }
    if not_installed(outcome, &lines) {
      match task.program() {
        "rojo" => offer_install_rojo("Rojo isn't installed."),
        "wally-package-types" => log(
          LogLevel::Info,
          "wally-package-types isn't installed, so Wally's packages aren't typed: add it with Rokit (JohnnyMorganz/wally-package-types).",
        ),
        program => warn(&format!(
          "{program} isn't installed: add it with Rokit, or install it from its releases."
        )),
      }
      return;
    }
    let mut message = format!("{} failed: {}", task.doing(), why(&lines, outcome));
    if matches!(task, Task::Upload)
      && lines
        .iter()
        .any(|line| line.contains("401") || line.contains("403") || line.contains("Unauthorized"))
    {
      message.push_str(" (Forget Open Cloud API Key, then upload again to enter another.)");
    }
    warn(&message);
    return;
  }
  match task {
    Task::Build { output, open } => {
      if open {
        match workspace::open_externally(&output) {
          Ok(()) => info(&format!("Built {output}, and opened it.")),
          Err(error) => warn(&format!("Built {output}, but couldn't open it: {error:?}")),
        }
      } else {
        info(&format!("Built {output}."));
      }
    }
    Task::Upload => info("Uploaded the project to Roblox."),
    Task::Syncback => info("Syncback updated the project's files from the place."),
    Task::Format => info("Formatted the project file."),
    Task::Init { kind } => {
      info(&format!("Made a new {kind} project."));
      refresh_projects();
      configure_sourcemap(false);
    }
    Task::PluginInstall { version } => {
      let _ = storage::set(
        StorageScope::Global,
        "plugin-offered",
        Some(&serde_json::to_string(&version).unwrap_or_default()),
      );
      info("Installed Rojo's Studio plugin: restart Roblox Studio to load it.");
    }
    Task::RokitInit => {
      if let Err(error) = run_task(
        Task::RokitAdd { global: false },
        args(&["add", "rojo-rbx/rojo"]),
      ) {
        warn(&error);
      }
    }
    Task::RokitAdd { global } => {
      info(if global {
        "Installed Rojo for every project."
      } else {
        "Added Rojo to this project's rokit.toml, and installed it."
      });
      with_state(|state| state.sourcemap_failure_shown = false);
      configure_sourcemap(true);
    }
    Task::RokitTrust => {
      if let Err(error) = run_task(Task::RokitInstall, args(&["install"])) {
        warn(&error);
      }
    }
    Task::RokitInstall => {
      info("Installed the tools rokit.toml lists.");
      with_state(|state| state.sourcemap_failure_shown = false);
      configure_sourcemap(true);
    }
    Task::WallyInstall => {
      info("Installed Wally's packages.");
      // Types once the sourcemap has the packages.
      let timer = timers::set_timer(PACKAGE_TYPES_WAIT_MS, TIMER).ok();
      with_state(|state| state.package_types_timer = timer);
    }
    Task::PackageTypes => log(LogLevel::Info, "typed Wally's packages"),
  }
}

fn args(list: &[&str]) -> Vec<String> {
  list.iter().map(|arg| (*arg).to_owned()).collect()
}

// --- Prompts ---------------------------------------------------------------

fn remember(result: Result<u64, impl std::fmt::Debug>, pending: Pending) {
  match result {
    Ok(prompt) => with_state(|state| {
      state.pending.insert(prompt, pending);
    }),
    Err(error) => log(LogLevel::Warn, &format!("couldn't ask: {error:?}")),
  }
}

fn ask_confirm(
  message: &str,
  detail: Option<String>,
  buttons: &[&str],
  destructive: bool,
  pending: Pending,
) {
  let result = prompts::confirm(
    &ConfirmOptions {
      message: message.into(),
      detail,
      buttons: buttons.iter().map(|b| (*b).to_owned()).collect(),
      destructive,
    },
    ANSWERED,
  );
  remember(result, pending);
}

fn ask_input(
  title: &str,
  value: String,
  placeholder: Option<&str>,
  password: bool,
  pending: Pending,
) {
  let options = InputOptions {
    title: title.into(),
    value,
    placeholder: placeholder.map(str::to_owned),
  };
  let result = if password {
    prompts::input_password(&options, ANSWERED)
  } else {
    prompts::input(&options, ANSWERED)
  };
  remember(result, pending);
}

fn answered() -> Result<(), String> {
  let Some(answer) = prompts::answer() else {
    return Ok(());
  };
  let Some(pending) = with_state(|state| state.pending.remove(&answer.prompt)) else {
    return Ok(());
  };
  let text = match &answer.value {
    AnswerValue::Text(text) => Some(text.trim().to_owned()),
    _ => None,
  };
  let picked = match &answer.value {
    AnswerValue::Picked(value) => Some(value.clone()),
    _ => None,
  };
  let button = match answer.value {
    AnswerValue::Button(index) => Some(index),
    _ => None,
  };
  match pending {
    Pending::Project(action) => {
      if let Some(path) = picked {
        let project = with_state(|state| state.projects.iter().find(|p| p.path == path).cloned());
        if let Some(project) = project {
          return act(action, project);
        }
      }
    }
    Pending::BuildOutput { project, open } => {
      if let Some(output) = text.filter(|t| !t.is_empty()) {
        build_to(project, output, open)?;
      }
    }
    Pending::UploadUniverse { project } => {
      if let Some(universe) = text.filter(|t| !t.is_empty()) {
        if !universe.chars().all(|c| c.is_ascii_digit()) {
          return Err("A universe ID is a number, as in the experience's settings.".into());
        }
        let place = project_value(&project, "upload-place").unwrap_or_default();
        ask_input(
          "The place ID to upload to",
          place,
          Some("The number in the place's address"),
          false,
          Pending::UploadPlace { project, universe },
        );
      }
    }
    Pending::UploadPlace { project, universe } => {
      if let Some(place) = text.filter(|t| !t.is_empty()) {
        if !place.chars().all(|c| c.is_ascii_digit()) {
          return Err("A place ID is a number, as in the place's address.".into());
        }
        set_project_value(&project, "upload-universe", &universe);
        set_project_value(&project, "upload-place", &place);
        match secrets::get(API_KEY) {
          Ok(Some(key)) => upload(&project, &universe, &place, &key)?,
          Ok(None) => ask_input(
            "Your Open Cloud API key, with place publishing access (from create.roblox.com/credentials)",
            String::new(),
            None,
            true,
            Pending::UploadKey {
              project,
              universe,
              place,
            },
          ),
          Err(error) => return Err(format!("The API key couldn't be read: {error:?}")),
        }
      }
    }
    Pending::UploadKey {
      project,
      universe,
      place,
    } => {
      if let Some(key) = text.filter(|t| !t.is_empty()) {
        if let Err(error) = secrets::set(API_KEY, Some(&key)) {
          log(
            LogLevel::Warn,
            &format!("couldn't keep the API key: {error:?}"),
          );
        }
        upload(&project, &universe, &place, &key)?;
      }
    }
    Pending::SyncbackFile { project } => match picked.as_deref() {
      Some("") => ask_input(
        "The place file to sync back from, relative to the folder",
        String::new(),
        Some("place.rbxl"),
        false,
        Pending::SyncbackPath { project },
      ),
      Some(input) => confirm_syncback(project, input.to_owned()),
      None => {}
    },
    Pending::SyncbackPath { project } => {
      if let Some(input) = text.filter(|t| !t.is_empty()) {
        confirm_syncback(project, input);
      }
    }
    Pending::SyncbackConfirm { project, input } => {
      if button == Some(0) {
        run_task(
          Task::Syncback,
          args(&[
            "syncback",
            &project.relative,
            "--input",
            &input,
            "--non-interactive",
            "--color",
            "never",
          ]),
        )?;
      }
    }
    Pending::NewProject => {
      if let Some(kind) = picked {
        run_task(
          Task::Init { kind: kind.clone() },
          args(&["init", "--kind", &kind, "--color", "never"]),
        )?;
      }
    }
    Pending::Plugin { version } => match button {
      Some(0) => run_task(
        Task::PluginInstall { version },
        args(&["plugin", "install", "--color", "never"]),
      )?,
      Some(1) => {
        let _ = storage::set(
          StorageScope::Global,
          "plugin-offered",
          Some(&serde_json::to_string(&version).unwrap_or_default()),
        );
      }
      _ => {}
    },
    Pending::InstallRojo => match button {
      Some(0) => {
        let folder = trusted_folder()?;
        let has_manifest =
          flareite_extension_api::files::read_file(&join(&folder, "rokit.toml")).is_ok();
        if has_manifest {
          run_task(
            Task::RokitAdd { global: false },
            args(&["add", "rojo-rbx/rojo"]),
          )?;
        } else {
          run_task(Task::RokitInit, args(&["init"]))?;
        }
      }
      Some(1) => run_task(
        Task::RokitAdd { global: true },
        args(&["add", "--global", "rojo-rbx/rojo"]),
      )?,
      _ => {}
    },
    Pending::InstallGuide => {
      if button == Some(0) {
        links::open_in_browser(INSTALL_GUIDE).map_err(|error| format!("{error:?}"))?;
      }
    }
    Pending::InstallTools { tools } => {
      if button == Some(0) {
        let mut trust = vec!["trust".to_owned()];
        trust.extend(tools.iter().map(|tool| tool.repository.clone()));
        run_task(Task::RokitTrust, trust)?;
      }
    }
    Pending::InstallPackages => match button {
      Some(0) => install_packages()?,
      Some(1) => {
        let _ = host::set_configuration("wally_prompt", Some(&ConfigValue::Boolean(false)));
      }
      _ => {}
    },
  }
  Ok(())
}

// --- Commands --------------------------------------------------------------

/// A value kept for a project in the open folder.
fn project_value(project: &Project, key: &str) -> Option<String> {
  storage::get(
    StorageScope::Workspace,
    &format!("{key}:{}", project.relative),
  )
  .and_then(|value| serde_json::from_str(&value).ok())
}

fn set_project_value(project: &Project, key: &str, value: &str) {
  let _ = storage::set(
    StorageScope::Workspace,
    &format!("{key}:{}", project.relative),
    Some(&serde_json::to_string(value).unwrap_or_default()),
  );
}

/// The project a command is for: the argument (from the Projects view), the
/// only one, or the one the user picks.
fn with_project(action: Action) -> Result<(), String> {
  if let Some(path) = workbench::command_argument() {
    let project = with_state(|state| state.projects.iter().find(|p| p.path == path).cloned());
    return match project {
      Some(project) => act(action, project),
      None => Err("That project file is gone.".into()),
    };
  }
  let projects = with_state(|state| state.projects.clone());
  match projects.len() {
    0 => Err("This folder has no Rojo project: New Project makes one.".into()),
    1 => act(action, projects[0].clone()),
    _ if action == Action::Serve => match main_project() {
      Some(project) if project.relative == Config::read().project => act(action, project),
      _ => pick_project(action, &projects),
    },
    _ => pick_project(action, &projects),
  }
}

fn pick_project(action: Action, projects: &[Project]) -> Result<(), String> {
  let title = match action {
    Action::Serve => "Serve which project?",
    Action::Build { .. } => "Build which project?",
    Action::Upload => "Upload which project?",
    Action::Syncback => "Sync back into which project?",
    Action::Format => "Format which project file?",
  };
  let items: Vec<PickItem> = projects
    .iter()
    .map(|project| PickItem {
      label: project.name.clone(),
      description: Some(project.relative.clone()),
      value: project.path.clone(),
    })
    .collect();
  remember(
    prompts::pick(title, &items, ANSWERED),
    Pending::Project(action),
  );
  Ok(())
}

fn act(action: Action, project: Project) -> Result<(), String> {
  match action {
    Action::Serve => serve(project),
    Action::Build { open } => {
      let output = project_value(&project, "build").unwrap_or_else(|| project.default_output());
      let title = if open {
        format!("Build {} to (then open it in Roblox Studio)", project.name)
      } else {
        format!("Build {} to", project.name)
      };
      ask_input(
        &title,
        output,
        Some(".rbxl or .rbxlx for a place; .rbxm or .rbxmx for a model"),
        false,
        Pending::BuildOutput { project, open },
      );
      Ok(())
    }
    Action::Upload => {
      if !project.place {
        return Err("Only places can be uploaded: this project builds a model.".into());
      }
      let universe = project_value(&project, "upload-universe").unwrap_or_default();
      ask_input(
        "The universe ID of the experience to upload to",
        universe,
        Some("The number in the experience's settings on create.roblox.com"),
        false,
        Pending::UploadUniverse { project },
      );
      Ok(())
    }
    Action::Syncback => {
      let folder = trusted_folder()?;
      // Rojo 7.5 added syncback.
      if let Some(version) = rojo_version(&folder)
        && major_minor(&version).is_some_and(|version| version < (7, 5))
      {
        return Err(format!(
          "Syncback needs Rojo 7.5 or later, and this folder's is {version}: update it (with Rokit, `rokit add rojo-rbx/rojo` in the project, or `rokit update rojo`)."
        ));
      }
      let mut items: Vec<PickItem> = flareite_extension_api::files::list_files(&folder)
        .map(|list| list.paths)
        .unwrap_or_default()
        .iter()
        .filter(|path| path.ends_with(".rbxl") || path.ends_with(".rbxlx"))
        .filter_map(|path| relative_to(&folder, path))
        .map(|relative| PickItem {
          label: relative.clone(),
          description: None,
          value: relative,
        })
        .collect();
      items.push(PickItem {
        label: "Another file…".into(),
        description: Some("Type its path".into()),
        value: String::new(),
      });
      remember(
        prompts::pick("Sync back from which place file?", &items, ANSWERED),
        Pending::SyncbackFile { project },
      );
      Ok(())
    }
    Action::Format => run_task(
      Task::Format,
      args(&["fmt-project", &project.relative, "--color", "never"]),
    ),
  }
}

fn build_to(project: Project, output: String, open: bool) -> Result<(), String> {
  let output = output.replace('\\', "/");
  let extension = output.rsplit('.').next().unwrap_or_default().to_lowercase();
  if !["rbxl", "rbxlx", "rbxm", "rbxmx"].contains(&extension.as_str()) {
    return Err("The file must end in .rbxl, .rbxlx, .rbxm, or .rbxmx.".into());
  }
  if open && !extension.starts_with("rbxl") {
    return Err("Roblox Studio opens places: build to a .rbxl or .rbxlx file.".into());
  }
  let folder = trusted_folder()?;
  set_project_value(&project, "build", &output);
  let absolute = if output.starts_with('/') || output.contains(':') {
    output.clone()
  } else {
    join(&folder, &output)
  };
  run_task(
    Task::Build {
      output: absolute,
      open,
    },
    args(&[
      "build",
      &project.relative,
      "--output",
      &output,
      "--color",
      "never",
    ]),
  )
}

fn upload(project: &Project, universe: &str, place: &str, key: &str) -> Result<(), String> {
  run_task(
    Task::Upload,
    args(&[
      "upload",
      &project.relative,
      "--asset_id",
      place,
      "--universe_id",
      universe,
      "--api_key",
      key,
      "--color",
      "never",
    ]),
  )
}

fn confirm_syncback(project: Project, input: String) {
  ask_confirm(
    &format!("Sync {} back from {input}?", project.name),
    Some(
      "Rojo rewrites the project's files to match the place: files are changed, added, and removed. Commit or back up your work first."
        .into(),
    ),
    &["Syncback"],
    true,
    Pending::SyncbackConfirm { project, input },
  );
}

fn new_project() -> Result<(), String> {
  let folder = trusted_folder()?;
  if flareite_extension_api::files::read_file(&join(&folder, "default.project.json")).is_ok() {
    return Err("This folder already has a default.project.json.".into());
  }
  let items = [
    ("place", "Place", "A game: a DataModel with its services"),
    ("model", "Model", "A model or library, such as a package"),
    ("plugin", "Plugin", "A Roblox Studio plugin"),
  ]
  .map(|(value, label, description)| PickItem {
    label: label.into(),
    description: Some(description.into()),
    value: value.into(),
  });
  remember(
    prompts::pick("What kind of project?", &items, ANSWERED),
    Pending::NewProject,
  );
  Ok(())
}

/// Offers to install Rojo with Rokit, or explains how without it.
fn offer_install_rojo(why: &str) {
  let offered = with_state(|state| std::mem::replace(&mut state.install_offered, true));
  if offered {
    log(LogLevel::Warn, why);
    return;
  }
  install_rojo(why);
}

fn install_rojo(why: &str) {
  let folder = with_state(|state| state.folder.clone());
  let rokit = folder
    .as_deref()
    .and_then(|folder| processes::run("rokit", &["--version".to_owned()], &options(folder)).ok())
    .is_some_and(|output| output.exit_code == Some(0));
  if rokit {
    ask_confirm(
      &format!("{why} Install it with Rokit?"),
      Some(
        "Rokit downloads Rojo from GitHub (rojo-rbx/rojo). Added to this project's rokit.toml, everyone working on it gets the same version."
          .into(),
      ),
      &["Add to This Project", "Install for All Projects"],
      false,
      Pending::InstallRojo,
    );
  } else {
    ask_confirm(
      why,
      Some(
        "Install Rokit, the toolchain manager for Roblox, then Rojo with it (`rokit add rojo-rbx/rojo`); or download Rojo from its releases. Restart Flareite after, if it's not found."
          .into(),
      ),
      &["Open Installation Guide"],
      false,
      Pending::InstallGuide,
    );
  }
}

fn install_tools() -> Result<(), String> {
  let folder = trusted_folder()?;
  let text = flareite_extension_api::files::read_file(&join(&folder, "rokit.toml"))
    .map_err(|_| "This folder has no rokit.toml.".to_owned())?;
  let tools = tools::rokit_tools(&text);
  if tools.is_empty() {
    return Err("rokit.toml lists no tools.".into());
  }
  ask_confirm(
    "Install the tools rokit.toml lists?",
    Some(format!(
      "Rokit downloads them from GitHub, and trusts them from now on: {}.",
      tools
        .iter()
        .map(|tool| format!("{} ({})", tool.name, tool.spec))
        .collect::<Vec<_>>()
        .join(", ")
    )),
    &["Install"],
    false,
    Pending::InstallTools { tools },
  );
  Ok(())
}

fn install_packages() -> Result<(), String> {
  let folder = trusted_folder()?;
  if flareite_extension_api::files::read_file(&join(&folder, "wally.toml")).is_err() {
    return Err("This folder has no wally.toml.".into());
  }
  run_task(Task::WallyInstall, args(&["install"]))
}

/// Types the installed packages, with `wally-package-types` (if it's
/// installed), from the sourcemap.
fn type_packages() {
  let Some(folder) = with_state(|state| {
    state.package_types_timer.take();
    state.folder.clone()
  }) else {
    return;
  };
  if flareite_extension_api::files::read_file(&join(&folder, "sourcemap.json")).is_err() {
    log(
      LogLevel::Info,
      "no sourcemap.json, so Wally's packages aren't typed",
    );
    return;
  }
  let listed = flareite_extension_api::files::list_files(&folder)
    .map(|list| list.paths)
    .unwrap_or_default();
  for packages in ["Packages", "ServerPackages", "DevPackages"] {
    let prefix = format!("{packages}/");
    let has = listed
      .iter()
      .filter_map(|path| relative_to(&folder, path))
      .any(|relative| relative.starts_with(&prefix));
    if has
      && let Err(error) = run_task(
        Task::PackageTypes,
        args(&["--sourcemap", "sourcemap.json", &prefix]),
      )
    {
      log(LogLevel::Info, &error);
    }
  }
}

fn files_changed() {
  let Some(changes) = workspace::changes() else {
    return;
  };
  let folder = with_state(|state| state.folder.clone());
  let Some(folder) = folder else {
    return;
  };
  if changes.overflow {
    refresh_projects();
    show_instances();
    return;
  }
  let relative: Vec<String> = changes
    .paths
    .iter()
    .filter_map(|path| relative_to(&folder, path))
    .collect();
  if relative.iter().any(|path| projects::is_project_file(path)) {
    refresh_projects();
    configure_sourcemap(false);
  }
  if relative.iter().any(|path| path == "sourcemap.json") {
    with_state(|state| state.sourcemap_failure_shown = false);
    show_instances();
    let waiting = with_state(|state| state.package_types_timer.take());
    if let Some(timer) = waiting {
      timers::clear_timer(timer);
      type_packages();
    }
  }
  if relative.iter().any(|path| path == "wally.toml") && Config::read().wally_prompt {
    let asking = with_state(|state| {
      state
        .pending
        .values()
        .any(|pending| matches!(pending, Pending::InstallPackages))
    });
    let installing = with_state(|state| {
      state
        .tasks
        .values()
        .any(|running| matches!(running.task, Task::WallyInstall))
    });
    if !asking && !installing {
      ask_confirm(
        "wally.toml changed. Install its packages?",
        Some("Runs `wally install`, then types the packages for luau-lsp.".into()),
        &["Install", "Don't Ask Again"],
        false,
        Pending::InstallPackages,
      );
    }
  }
}

// --- Schemas ---------------------------------------------------------------

/// Sets the project and instance file schemas from `api`.
fn set_schemas(api: &schema::Api) {
  let sets = [
    (
      "project",
      vec!["*.project.json".to_owned()],
      schema::project_schema(api),
    ),
    (
      "instance",
      vec!["*.meta.json".to_owned(), "*.model.json".to_owned()],
      schema::instance_file_schema(api),
    ),
  ];
  for (id, files, content) in sets {
    if let Err(error) = schemas::set_schema(&Schema {
      id: id.into(),
      files,
      format: SchemaFormat::JsonSchema,
      content,
    }) {
      log(
        LogLevel::Warn,
        &format!("couldn't set the {id} schema: {error:?}"),
      );
    }
  }
}

/// Sets schemas that check the structure only, and asks for Roblox's API,
/// which the full ones are made from.
fn start_schemas() {
  set_schemas(&schema::Api::default());
  let dump = downloads::download(schema::API_DUMP, DOWNLOADED);
  let docs = downloads::download(schema::API_DOCS, DOWNLOADED);
  match (dump, docs) {
    (Ok(dump), Ok(docs)) => with_state(|state| {
      state.dump_request = Some(dump);
      state.docs_request = Some(docs);
    }),
    (Err(error), _) | (_, Err(error)) => log(
      LogLevel::Info,
      &format!(
        "project files are checked without Roblox's API, which can't be downloaded: {error:?}"
      ),
    ),
  }
}

fn downloaded() {
  let Some(file) = downloads::downloaded() else {
    return;
  };
  let ready = with_state(|state| {
    if state.dump_request == Some(file.request) {
      match file.outcome {
        Ok(bytes) => state.dump = Some(bytes),
        Err(error) => {
          state.dump_request = None;
          return Err(error);
        }
      }
    } else if state.docs_request == Some(file.request) {
      state.docs = Some(file.outcome.ok());
    }
    Ok(state.dump.is_some() && state.docs.is_some())
  });
  match ready {
    Err(error) => log(
      LogLevel::Warn,
      &format!(
        "Roblox's API couldn't be downloaded, so project files are checked without it: {error}"
      ),
    ),
    Ok(false) => {}
    Ok(true) => {
      let (dump, docs) = with_state(|state| (state.dump.take(), state.docs.take()));
      let (Some(dump), Some(docs)) = (dump, docs) else {
        return;
      };
      match schema::Api::parse(&dump, docs.as_deref()) {
        Ok(api) => {
          set_schemas(&api);
          log(
            LogLevel::Info,
            &format!(
              "project files are checked against Roblox's API ({} classes)",
              api.class_count()
            ),
          );
        }
        Err(error) => log(LogLevel::Warn, &error),
      }
    }
  }
}

// --- The extension ---------------------------------------------------------

struct Rojo;

impl Guest for Rojo {
  fn activate(_context: Activation) -> Result<(), String> {
    refresh_folder();
    start_schemas();
    Ok(())
  }

  fn deactivate() {
    with_state(|state| {
      if let Some(serve) = state.serve.take() {
        processes::kill(serve.process);
      }
      if let Some(run) = state.sourcemap.take() {
        processes::kill(run.process);
      }
    });
  }

  fn run_command(command: String, _context: CommandContext) -> Result<(), String> {
    match command.trim_start_matches("flareite.rojo.") {
      "serve" => with_project(Action::Serve)?,
      "stop" => stop_serving(),
      "toggleServe" => {
        if with_state(|state| state.serve.is_some()) {
          stop_serving();
        } else {
          with_project(Action::Serve)?;
        }
      }
      "build" => with_project(Action::Build { open: false })?,
      "buildAndOpen" => with_project(Action::Build { open: true })?,
      "upload" => with_project(Action::Upload)?,
      "syncback" => with_project(Action::Syncback)?,
      "formatProject" => with_project(Action::Format)?,
      "openProject" | "openInstance" => {
        let path = workbench::command_argument().ok_or("No file was given.")?;
        workspace::open_file(&path)
          .map_err(|error| format!("{path} couldn't be opened: {error:?}"))?;
      }
      "newProject" => new_project()?,
      "refresh" => {
        refresh_projects();
        show_instances();
      }
      "installPlugin" => {
        let folder = trusted_folder()?;
        let version = rojo_version(&folder).unwrap_or_default();
        run_task(
          Task::PluginInstall { version },
          args(&["plugin", "install", "--color", "never"]),
        )?;
      }
      "installRojo" => install_rojo("Install Rojo?"),
      "regenerateSourcemap" => {
        with_state(|state| state.sourcemap_failure_shown = false);
        let config = Config::read();
        if !config.sourcemap {
          return Err("Turn on the Rojo extension's Sourcemap setting first.".into());
        }
        trusted_folder()?;
        if main_project().is_none() {
          return Err("This folder has no Rojo project to make a sourcemap from.".into());
        }
        configure_sourcemap(true);
      }
      "installTools" => install_tools()?,
      "installPackages" => install_packages()?,
      "forgetApiKey" => {
        secrets::set(API_KEY, None).map_err(|error| format!("{error:?}"))?;
        info("Forgot the Open Cloud API key: the next upload asks for one.");
      }
      "configure" => {
        configure_sourcemap(false);
        show_instances();
        show_status();
      }
      "folderChanged" => refresh_folder(),
      "filesChanged" => files_changed(),
      "output" => output(),
      "finished" => finished(),
      "answered" => answered()?,
      "downloaded" => downloaded(),
      "timer" => {
        let fired = timers::fired();
        let ours = with_state(|state| state.package_types_timer == fired);
        if ours {
          type_packages();
        }
      }
      _ => return Err(format!("unknown command {command}")),
    }
    Ok(())
  }

  fn document_opened(_document: DocumentInfo) {}

  fn document_changed(_change: DocumentChange) {}

  fn document_closed(_document: DocumentId) {}
}

export_extension!(Rojo);

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn rojo_error_lines_give_their_message() {
    assert_eq!(
      error_message(
        "[ERROR librojo::change_processor] File contained malformed init.meta.json data"
      ),
      Some("File contained malformed init.meta.json data")
    );
    assert_eq!(error_message("        Caused by:"), None);
    assert_eq!(error_message("[INFO rojo] fine"), None);
  }

  #[test]
  fn failures_say_the_last_error() {
    let lines = vec![
      "[ERROR rojo] Rojo project referred to a file using $path that could not be turned into a Roblox Instance by Rojo.".to_owned(),
      "        Check that the file exists and is a file type known by Rojo.".to_owned(),
      "        File $path: nope".to_owned(),
    ];
    let outcome = Ok(ProcessOutput {
      exit_code: Some(1),
      stdout: Vec::new(),
      stderr: Vec::new(),
      truncated: false,
    });
    assert!(why(&lines, &outcome).starts_with("Rojo project referred to a file using $path"));
    assert_eq!(why(&[], &outcome), "exit code 1");
    let usage = vec![
      "error: Found argument 'syncback' which wasn't expected, or isn't valid in this context"
        .to_owned(),
      "USAGE:".to_owned(),
      "For more information try --help".to_owned(),
    ];
    assert_eq!(
      why(&usage, &outcome),
      "Found argument 'syncback' which wasn't expected, or isn't valid in this context"
    );
  }

  #[test]
  fn color_codes_are_stripped() {
    assert_eq!(
      strip_ansi("\u{1b}[31mERROR\u{1b}[0m Failed to find tool 'wally'"),
      "ERROR Failed to find tool 'wally'"
    );
    assert_eq!(strip_ansi("plain"), "plain");
  }

  #[test]
  fn versions_compare_by_major_and_minor() {
    assert!(major_minor("7.4.1").is_some_and(|v| v < (7, 5)));
    assert!(major_minor("7.10.0").is_some_and(|v| v >= (7, 5)));
    assert_eq!(major_minor("7.5.0-rc.1"), Some((7, 5)));
    assert_eq!(major_minor("nope"), None);
  }

  #[test]
  fn rokit_shims_say_when_a_tool_is_missing() {
    let outcome = Ok(ProcessOutput {
      exit_code: Some(1),
      stdout: Vec::new(),
      stderr: Vec::new(),
      truncated: false,
    });
    let lines = vec!["ERROR Failed to find tool 'wally' in any project manifest file.".to_owned()];
    assert!(not_installed(&outcome, &lines));
    assert!(!not_installed(&outcome, &["[ERROR rojo] other".to_owned()]));
    assert!(not_installed(
      &Err(ProcessError::NotFound("gone".into())),
      &[]
    ));
  }

  #[test]
  fn sourcemaps_watch_the_project() {
    let project = Project {
      path: "/g/default.project.json".into(),
      relative: "default.project.json".into(),
      name: "g".into(),
      place: true,
    };
    let config = Config {
      project: "default.project.json".into(),
      sourcemap: true,
      include_non_scripts: true,
      serve_port: 0,
      explorer_marks: true,
      wally_prompt: true,
    };
    assert_eq!(
      sourcemap_args(&project, &config),
      [
        "sourcemap",
        "default.project.json",
        "--watch",
        "--output",
        "sourcemap.json",
        "--color",
        "never",
        "--include-non-scripts"
      ]
    );
  }
}
