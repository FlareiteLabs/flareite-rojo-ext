//! `sourcemap.json`, which `rojo sourcemap` writes: the instances a project
//! makes, and the files each is made from. luau-lsp reads it to resolve
//! requires; here it's the Instances view, and the explorer's marks.

use flareite_extension_api::explorer::FileMark;
use flareite_extension_api::views::{Expansion, ItemIcon, TreeItem};
use serde::Deserialize;

use crate::projects::join;

/// Most instances the view lists (the view takes 10,000).
pub const MAX_ITEMS: usize = 9_999;

/// An instance.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Node {
  pub name: String,
  pub class_name: String,
  /// Relative to the folder Rojo ran in.
  #[serde(default)]
  pub file_paths: Vec<String>,
  #[serde(default)]
  pub children: Vec<Node>,
}

pub fn parse(text: &str) -> Result<Node, String> {
  serde_json::from_str(text).map_err(|error| format!("sourcemap.json isn't readable: {error}"))
}

/// Whether a name can follow a `.` in Luau.
fn is_identifier(name: &str) -> bool {
  let mut chars = name.chars();
  chars
    .next()
    .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
    && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// A child's path from its parent's, as Luau writes it.
fn child_path(parent: &str, name: &str) -> String {
  if is_identifier(name) {
    format!("{parent}.{name}")
  } else {
    format!("{parent}[{name:?}]")
  }
}

/// The file to open for an instance: its first file that isn't a
/// `.meta.json`, or that one.
fn main_file(node: &Node) -> Option<&str> {
  node
    .file_paths
    .iter()
    .find(|path| !path.ends_with(".meta.json"))
    .or_else(|| node.file_paths.first())
    .map(String::as_str)
}

fn file_name(path: &str) -> &str {
  path.rsplit(['/', '\\']).next().unwrap_or(path)
}

/// The badge for a script: SS for a Script, LS for a LocalScript (or a
/// client Script with a RunContext), MS for a ModuleScript.
pub fn badge(class: &str, file: &str) -> Option<&'static str> {
  match class {
    "LocalScript" => Some("LS"),
    "Script" if file_name(file).contains(".client.") => Some("LS"),
    "Script" => Some("SS"),
    "ModuleScript" => Some("MS"),
    _ => None,
  }
}

/// The view's items: the root's children (a place's services), or the root
/// itself for a model; and whether some were left out.
pub fn items(root: &Node, folder: &str) -> (Vec<TreeItem>, bool) {
  let mut items = Vec::new();
  let mut truncated = false;
  if root.class_name == "DataModel" {
    for (index, child) in root.children.iter().enumerate() {
      add_items(
        child,
        index.to_string(),
        None,
        &child_path("game", &child.name),
        folder,
        &mut items,
        &mut truncated,
      );
    }
  } else {
    add_items(
      root,
      "0".into(),
      None,
      &root.name,
      folder,
      &mut items,
      &mut truncated,
    );
    if let Some(item) = items.first_mut() {
      item.expansion = Expansion::Expanded;
    }
  }
  (items, truncated)
}

fn add_items(
  node: &Node,
  id: String,
  parent: Option<String>,
  path: &str,
  folder: &str,
  items: &mut Vec<TreeItem>,
  truncated: &mut bool,
) {
  if items.len() >= MAX_ITEMS {
    *truncated = true;
    return;
  }
  let file = main_file(node);
  let icon = match file {
    Some(file) if !file.ends_with(".project.json") && !file.ends_with(".meta.json") => {
      ItemIcon::File(file_name(file).to_owned())
    }
    _ if node.class_name == "Folder" || !node.children.is_empty() => {
      ItemIcon::Folder(node.name.clone())
    }
    _ => ItemIcon::Named("layers".into()),
  };
  let mut tooltip = format!("{path}\n{}", node.class_name);
  if let Some(file) = file {
    tooltip.push('\n');
    tooltip.push_str(file);
  }
  items.push(TreeItem {
    id: id.clone(),
    parent,
    label: node.name.clone(),
    description: Some(node.class_name.clone()),
    tooltip: Some(tooltip),
    icon,
    mark: None,
    context: Some(if file.is_some() { "file" } else { "instance" }.into()),
    expansion: if node.children.is_empty() {
      Expansion::None
    } else {
      Expansion::Collapsed
    },
    command: file.map(|_| "flareite.rojo.openInstance".into()),
    argument: file.map(|file| join(folder, file)),
  });
  for (index, child) in node.children.iter().enumerate() {
    add_items(
      child,
      format!("{id}/{index}"),
      Some(id.clone()),
      &child_path(path, &child.name),
      folder,
      items,
      truncated,
    );
  }
}

/// The explorer's marks: each instance's main file says what it becomes,
/// and so does the folder of an `init` file.
pub fn marks(root: &Node, folder: &str) -> Vec<FileMark> {
  let mut marks = Vec::new();
  let root_path = if root.class_name == "DataModel" {
    "game".to_owned()
  } else {
    root.name.clone()
  };
  add_marks(root, &root_path, folder, &mut marks);
  marks
}

fn mark(path: String, badge: Option<&str>, tooltip: String) -> FileMark {
  FileMark {
    path,
    badge: badge.map(str::to_owned),
    color: None,
    tooltip: Some(tooltip),
    faded: false,
    inherit: false,
    propagate: false,
    priority: 0,
  }
}

fn add_marks(node: &Node, path: &str, folder: &str, marks: &mut Vec<FileMark>) {
  if let Some(file) = main_file(node).filter(|file| !file.ends_with(".project.json")) {
    let badge = badge(&node.class_name, file);
    let tooltip = format!("{} {path}", node.class_name);
    let name = file_name(file);
    // `init.server.luau`, `init.meta.json`: the folder is the instance.
    if name.starts_with("init.")
      && let Some((parent, _)) = file.rsplit_once(['/', '\\'])
    {
      marks.push(mark(join(folder, parent), badge, tooltip.clone()));
    }
    if !name.ends_with(".meta.json") {
      marks.push(mark(join(folder, file), badge, tooltip));
    }
  }
  for child in &node.children {
    add_marks(child, &child_path(path, &child.name), folder, marks);
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  /// What `rojo sourcemap --include-non-scripts` writes for `rojo init`'s
  /// place.
  const PLACE: &str = r#"{"name":"rojo-place","className":"DataModel","filePaths":["default.project.json"],"children":[{"name":"Lighting","className":"Lighting"},{"name":"ReplicatedStorage","className":"ReplicatedStorage","children":[{"name":"Shared","className":"Folder","children":[{"name":"Hello","className":"ModuleScript","filePaths":["src/shared/Hello.luau"]}]}]},{"name":"ServerScriptService","className":"ServerScriptService","children":[{"name":"Server","className":"Script","filePaths":["src/server/init.server.luau"]}]},{"name":"StarterPlayer","className":"StarterPlayer","children":[{"name":"StarterPlayerScripts","className":"StarterPlayerScripts","children":[{"name":"Client","className":"LocalScript","filePaths":["src/client/init.client.luau"]}]}]},{"name":"Workspace","className":"Workspace","children":[{"name":"Baseplate","className":"Part"},{"name":"My Part","className":"Part","filePaths":["src/My Part.model.json"]}]}]}"#;

  #[test]
  fn lists_a_places_services_and_their_instances() {
    let root = parse(PLACE).expect("parses");
    let (items, truncated) = items(&root, "C:/game");
    assert!(!truncated);
    let labels: Vec<&str> = items
      .iter()
      .filter(|item| item.parent.is_none())
      .map(|item| item.label.as_str())
      .collect();
    assert_eq!(
      labels,
      [
        "Lighting",
        "ReplicatedStorage",
        "ServerScriptService",
        "StarterPlayer",
        "Workspace"
      ]
    );
    let hello = items
      .iter()
      .find(|item| item.label == "Hello")
      .expect("Hello");
    assert_eq!(hello.parent.as_deref(), Some("1/0"));
    assert_eq!(
      hello.argument.as_deref(),
      Some("C:/game/src/shared/Hello.luau")
    );
    assert_eq!(hello.icon, ItemIcon::File("Hello.luau".into()));
    assert!(
      hello
        .tooltip
        .as_deref()
        .is_some_and(|t| t.starts_with("game.ReplicatedStorage.Shared.Hello\nModuleScript"))
    );
    let part = items
      .iter()
      .find(|item| item.label == "My Part")
      .expect("part");
    assert!(
      part
        .tooltip
        .as_deref()
        .is_some_and(|t| t.starts_with("game.Workspace[\"My Part\"]"))
    );
    let baseplate = items
      .iter()
      .find(|item| item.label == "Baseplate")
      .expect("baseplate");
    assert_eq!(
      baseplate.command, None,
      "made by the project, without a file"
    );
  }

  #[test]
  fn marks_scripts_and_init_folders() {
    let root = parse(PLACE).expect("parses");
    let marks = marks(&root, "C:/game");
    let found: Vec<(&str, Option<&str>)> = marks
      .iter()
      .map(|mark| (mark.path.as_str(), mark.badge.as_deref()))
      .collect();
    assert_eq!(
      found,
      [
        ("C:/game/src/shared/Hello.luau", Some("MS")),
        ("C:/game/src/server", Some("SS")),
        ("C:/game/src/server/init.server.luau", Some("SS")),
        ("C:/game/src/client", Some("LS")),
        ("C:/game/src/client/init.client.luau", Some("LS")),
        ("C:/game/src/My Part.model.json", None),
      ]
    );
    assert_eq!(
      marks[0].tooltip.as_deref(),
      Some("ModuleScript game.ReplicatedStorage.Shared.Hello")
    );
  }

  #[test]
  fn client_scripts_with_a_run_context_are_local() {
    assert_eq!(badge("Script", "src/a.client.luau"), Some("LS"));
    assert_eq!(badge("Script", "src/a.server.luau"), Some("SS"));
    assert_eq!(badge("Part", "src/a.model.json"), None);
  }

  #[test]
  fn models_list_their_root() {
    let root = parse(r#"{"name":"Lib","className":"ModuleScript","filePaths":["src/init.luau"],"children":[{"name":"Util","className":"ModuleScript","filePaths":["src/Util.luau"]}]}"#).expect("parses");
    let (items, _) = items(&root, "/lib");
    assert_eq!(items[0].label, "Lib");
    assert_eq!(items[0].expansion, Expansion::Expanded);
    assert_eq!(items[1].parent.as_deref(), Some("0"));
  }
}
