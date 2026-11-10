//! Rokit's `rokit.toml`, and telling when a Rokit shim's tool isn't
//! installed for the project.

/// A tool `rokit.toml` lists.
#[derive(Debug, Clone, PartialEq)]
pub struct Tool {
  /// What it's run as, such as `rojo`.
  pub name: String,
  /// `owner/repository@version`.
  pub spec: String,
  /// `owner/repository`, what `rokit trust` takes.
  pub repository: String,
}

/// The tools in a `rokit.toml`'s `[tools]` table.
pub fn rokit_tools(text: &str) -> Vec<Tool> {
  let mut tools = Vec::new();
  let mut in_tools = false;
  for line in text.lines() {
    let line = line.split('#').next().unwrap_or_default().trim();
    if line.starts_with('[') {
      in_tools = line == "[tools]";
      continue;
    }
    if !in_tools {
      continue;
    }
    let Some((name, value)) = line.split_once('=') else {
      continue;
    };
    let name = name.trim().trim_matches('"').to_owned();
    let spec = value.trim().trim_matches(['"', '\'']).to_owned();
    let Some((repository, _version)) = spec.split_once('@') else {
      continue;
    };
    if !name.is_empty() && repository.contains('/') {
      tools.push(Tool {
        name,
        repository: repository.to_owned(),
        spec,
      });
    }
  }
  tools
}

/// Whether a line of output says the tool run isn't installed: Rokit's
/// shims run a tool only where a manifest lists it.
pub fn says_missing(line: &str) -> bool {
  let line = line.to_lowercase();
  line.contains("failed to find tool") || line.contains("is not installed")
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn reads_the_tools_table() {
    let text = "# Tools\n[tools]\nrojo = \"rojo-rbx/rojo@7.4.4\" # comment\nwally = 'UpliftGames/wally@0.3.2'\nbad = \"nope\"\n\n[other]\nx = \"a/b@1\"\n";
    let tools = rokit_tools(text);
    assert_eq!(tools.len(), 2);
    assert_eq!(tools[0].name, "rojo");
    assert_eq!(tools[0].spec, "rojo-rbx/rojo@7.4.4");
    assert_eq!(tools[0].repository, "rojo-rbx/rojo");
    assert_eq!(tools[1].repository, "UpliftGames/wally");
  }

  #[test]
  fn shims_without_the_tool_are_missing() {
    assert!(says_missing(
      "\u{1b}[31mERROR\u{1b}[0m Failed to find tool 'wally' in any project manifest file."
    ));
    assert!(!says_missing("Rojo server listening:"));
  }
}
