//! The Rojo projects in the open folder: its `*.project.json` files, except
//! those of installed packages.

use flareite_extension_api::files;
use serde::Deserialize;

/// A project file.
#[derive(Debug, Clone, PartialEq)]
pub struct Project {
  /// Absolute, with `/` between names.
  pub path: String,
  /// Relative to the open folder, with `/`.
  pub relative: String,
  /// Its `name`, or the file's name without `.project.json`.
  pub name: String,
  /// Its tree is a DataModel: it builds a place, not a model.
  pub place: bool,
}

impl Project {
  /// The file a build makes by default: `<name>.rbxl` for a place, and
  /// `<name>.rbxm` for a model.
  pub fn default_output(&self) -> String {
    let name: String = self
      .name
      .chars()
      .map(|c| {
        if c.is_alphanumeric() || "-_ .".contains(c) {
          c
        } else {
          '_'
        }
      })
      .collect();
    format!(
      "{}.{}",
      name.trim(),
      if self.place { "rbxl" } else { "rbxm" }
    )
  }
}

#[derive(Deserialize)]
struct ProjectFile {
  name: Option<String>,
  tree: Option<Tree>,
}

#[derive(Deserialize)]
struct Tree {
  #[serde(rename = "$className")]
  class_name: Option<String>,
}

/// Folders that hold installed packages, whose project files aren't the
/// folder's.
const PACKAGE_FOLDERS: [&str; 5] = [
  "Packages",
  "ServerPackages",
  "DevPackages",
  "_Index",
  "node_modules",
];

/// Whether a path, relative to the open folder, is a project file of the
/// folder's own.
pub fn is_project_file(relative: &str) -> bool {
  let relative = relative.replace('\\', "/");
  relative.ends_with(".project.json")
    && !relative
      .split('/')
      .any(|part| PACKAGE_FOLDERS.contains(&part))
}

/// Joins the open folder and a relative path, with `/`.
pub fn join(folder: &str, relative: &str) -> String {
  format!(
    "{}/{}",
    folder.trim_end_matches(['/', '\\']).replace('\\', "/"),
    relative.trim_start_matches(['/', '\\']).replace('\\', "/")
  )
}

/// `path` relative to `folder`, with `/`, if it's inside it.
pub fn relative_to(folder: &str, path: &str) -> Option<String> {
  let folder = folder.trim_end_matches(['/', '\\']).replace('\\', "/");
  let path = path.replace('\\', "/");
  let rest = if cfg!(windows) || folder.contains(':') {
    // Windows paths compare without case.
    let lower = path.to_lowercase();
    lower
      .starts_with(&folder.to_lowercase())
      .then(|| path[folder.len()..].to_owned())?
  } else {
    path.strip_prefix(&folder)?.to_owned()
  };
  rest.strip_prefix('/').map(str::to_owned)
}

/// Reads a project file's name and kind; `None` if it isn't readable JSON.
pub fn read(folder: &str, relative: &str) -> Option<Project> {
  let path = join(folder, relative);
  let text = files::read_file(&path).ok()?;
  let file: Option<ProjectFile> = serde_json::from_str(strip_comments(&text).as_str()).ok();
  let file_name = relative.rsplit('/').next().unwrap_or(relative);
  let fallback = file_name.trim_end_matches(".project.json").to_owned();
  Some(Project {
    name: file
      .as_ref()
      .and_then(|file| file.name.clone())
      .filter(|name| !name.trim().is_empty())
      .unwrap_or(fallback),
    place: file
      .and_then(|file| file.tree)
      .and_then(|tree| tree.class_name)
      .is_some_and(|class| class == "DataModel"),
    relative: relative.to_owned(),
    path,
  })
}

/// The open folder's projects, `default.project.json` first, then by path.
pub fn find(folder: &str) -> Vec<Project> {
  let Ok(list) = files::list_files(folder) else {
    return Vec::new();
  };
  let mut projects: Vec<Project> = list
    .paths
    .iter()
    .filter_map(|path| relative_to(folder, path))
    .filter(|relative| is_project_file(relative))
    .filter_map(|relative| read(folder, &relative))
    .collect();
  projects.sort_by_key(|project| {
    (
      project.relative != "default.project.json",
      project.relative.matches('/').count(),
      project.relative.clone(),
    )
  });
  projects
}

/// JSON with comments (`//` and `/* */`, which Rojo allows) without them.
pub fn strip_comments(text: &str) -> String {
  let mut out = String::with_capacity(text.len());
  let mut chars = text.chars().peekable();
  let mut in_string = false;
  while let Some(c) = chars.next() {
    if in_string {
      out.push(c);
      match c {
        '\\' => {
          if let Some(next) = chars.next() {
            out.push(next);
          }
        }
        '"' => in_string = false,
        _ => {}
      }
      continue;
    }
    match (c, chars.peek()) {
      ('"', _) => {
        in_string = true;
        out.push(c);
      }
      ('/', Some('/')) => {
        for c in chars.by_ref() {
          if c == '\n' {
            out.push('\n');
            break;
          }
        }
      }
      ('/', Some('*')) => {
        chars.next();
        let mut last = ' ';
        for c in chars.by_ref() {
          if last == '*' && c == '/' {
            break;
          }
          last = c;
        }
      }
      _ => out.push(c),
    }
  }
  out
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn project_files_of_packages_are_left_out() {
    assert!(is_project_file("default.project.json"));
    assert!(is_project_file("places/lobby.project.json"));
    assert!(!is_project_file("Packages/_Index/a/b/default.project.json"));
    assert!(!is_project_file("node_modules\\x\\default.project.json"));
    assert!(!is_project_file("default.project.json.bak"));
  }

  #[test]
  fn paths_join_and_split_with_slashes() {
    assert_eq!(join("C:\\game\\", "src/a.luau"), "C:/game/src/a.luau");
    assert_eq!(
      relative_to("C:\\Game", "c:/game/src/a.luau").as_deref(),
      Some("src/a.luau")
    );
    assert_eq!(relative_to("/game", "/other/a.luau"), None);
  }

  #[test]
  fn comments_are_stripped_but_not_from_strings() {
    let text = "{ // a comment\n \"url\": \"http://x\", /* more */ \"a\": 1 }";
    let value: serde_json::Value = serde_json::from_str(&strip_comments(text)).expect("JSON");
    assert_eq!(value["url"], "http://x");
    assert_eq!(value["a"], 1);
  }

  #[test]
  fn builds_go_to_the_name_and_kind() {
    let project = Project {
      path: "/g/default.project.json".into(),
      relative: "default.project.json".into(),
      name: "My Game: Two".into(),
      place: true,
    };
    assert_eq!(project.default_output(), "My Game_ Two.rbxl");
    let model = Project {
      place: false,
      ..project
    };
    assert_eq!(model.default_output(), "My Game_ Two.rbxm");
  }
}
