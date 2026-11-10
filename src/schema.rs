//! Schemas for Rojo's project files (`*.project.json`) and instance files
//! (`*.meta.json`, `*.model.json`), made from Roblox's API: class names
//! complete and are checked, and each class's properties (inherited ones
//! included) complete under `$properties` with their types, enums' items,
//! and Roblox's documentation.
//!
//! The API comes from Roblox-Client-Tracker's API dump, and the
//! documentation from luau-lsp's (the Luau extension downloads the same
//! file for luau-lsp); Flareite downloads both and keeps them for a day.
//! Until they're had, or without the network permission, the schemas check
//! the files' structure only.
//!
//! A class's properties are a definition that names its superclass's with
//! `allOf`, and an instance's `$className` picks its class's with `if` and
//! `then`, which Flareite follows when completing; a service's are picked
//! by its name, under a `DataModel`.

use std::collections::{HashMap, HashSet};

use serde::Deserialize;
use serde_json::{Map, Value as Json, json};

/// Roblox's API: classes, their members, and enums.
pub const API_DUMP: &str =
  "https://raw.githubusercontent.com/MaximumADHD/Roblox-Client-Tracker/roblox/Mini-API-Dump.json";
/// Documentation for the API, by luau-lsp's keys.
pub const API_DOCS: &str = "https://luau-lsp.pages.dev/api-docs/en-us.json";

/// Longest description taken from the documentation.
const MAX_DOC_CHARS: usize = 400;

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Dump {
  classes: Vec<DumpClass>,
  enums: Vec<DumpEnum>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct DumpClass {
  name: String,
  superclass: String,
  #[serde(default)]
  tags: Vec<Json>,
  #[serde(default)]
  members: Vec<DumpMember>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct DumpMember {
  member_type: String,
  name: String,
  value_type: Option<ValueType>,
  #[serde(default)]
  tags: Vec<Json>,
  serialization: Option<Serialization>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct ValueType {
  category: String,
  name: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Serialization {
  can_load: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct DumpEnum {
  name: String,
  items: Vec<DumpItem>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct DumpItem {
  name: String,
  #[serde(default)]
  tags: Vec<Json>,
}

#[derive(Deserialize)]
struct DocEntry {
  documentation: Option<String>,
  learn_more_link: Option<String>,
}

fn has_tag(tags: &[Json], tag: &str) -> bool {
  tags.iter().any(|t| t.as_str() == Some(tag))
}

/// A property an instance file can set.
struct Property {
  name: String,
  /// Its type's name, such as `Vector3` or `Material`.
  kind: String,
  schema: Json,
  deprecated: bool,
  doc: Option<String>,
}

struct Class {
  name: String,
  superclass: Option<String>,
  service: bool,
  /// Can be `$className`: creatable, a service, or the DataModel (a
  /// place's root).
  usable: bool,
  deprecated: bool,
  doc: Option<String>,
  properties: Vec<Property>,
}

struct Enum {
  name: String,
  /// Names, and their documentation.
  items: Vec<(String, Option<String>)>,
}

/// The API, as the schemas need it.
#[derive(Default)]
pub struct Api {
  classes: Vec<Class>,
  enums: Vec<Enum>,
}

impl Api {
  /// Reads the API dump, and the documentation if there is any.
  pub fn parse(dump: &[u8], docs: Option<&[u8]>) -> Result<Self, String> {
    let dump: Dump = serde_json::from_slice(dump)
      .map_err(|error| format!("Roblox's API isn't readable: {error}"))?;
    let docs: HashMap<String, DocEntry> = docs
      .and_then(|docs| serde_json::from_slice(docs).ok())
      .unwrap_or_default();
    let doc = |key: String| docs.get(&key).and_then(describe);
    let enum_names: HashSet<String> = dump.enums.iter().map(|e| e.name.clone()).collect();
    let classes = dump
      .classes
      .into_iter()
      .map(|class| {
        let mut properties: Vec<Property> = class
          .members
          .into_iter()
          .filter(|member| member.member_type == "Property")
          .filter(|member| {
            !["ReadOnly", "Hidden", "NotScriptable"]
              .iter()
              .any(|tag| has_tag(&member.tags, tag))
              && member.serialization.as_ref().is_none_or(|s| s.can_load)
          })
          .filter_map(|member| {
            let value = member.value_type?;
            Some(Property {
              schema: value_schema(&value, &enum_names),
              kind: value.name,
              deprecated: has_tag(&member.tags, "Deprecated"),
              doc: doc(format!("@roblox/globaltype/{}.{}", class.name, member.name)),
              name: member.name,
            })
          })
          .collect();
        // Every instance has tags and attributes, which Rojo sets as
        // properties.
        if class.name == "Instance" {
          properties.push(Property {
            name: "Tags".into(),
            kind: "Tags".into(),
            schema: json!({ "type": "array", "items": { "type": "string" } }),
            deprecated: false,
            doc: Some("The instance's CollectionService tags.".into()),
          });
          properties.push(Property {
            name: "Attributes".into(),
            kind: "Attributes".into(),
            schema: json!({ "type": "object" }),
            deprecated: false,
            doc: Some("The instance's attributes, by name (or use `$attributes`).".into()),
          });
        }
        properties.sort_by(|a, b| a.name.cmp(&b.name));
        let service = has_tag(&class.tags, "Service");
        Class {
          superclass: (class.superclass != "<<<ROOT>>>").then_some(class.superclass),
          service,
          usable: service || class.name == "DataModel" || !has_tag(&class.tags, "NotCreatable"),
          deprecated: has_tag(&class.tags, "Deprecated"),
          doc: doc(format!("@roblox/globaltype/{}", class.name)),
          properties,
          name: class.name,
        }
      })
      .collect();
    let enums = dump
      .enums
      .into_iter()
      .map(|e| Enum {
        items: e
          .items
          .into_iter()
          .filter(|item| !has_tag(&item.tags, "Deprecated"))
          .map(|item| {
            let doc = doc(format!("@roblox/enum/{}.{}", e.name, item.name));
            (item.name, doc)
          })
          .collect(),
        name: e.name,
      })
      .collect();
    Ok(Self { classes, enums })
  }

  /// How many classes it knows.
  pub fn class_count(&self) -> usize {
    self.classes.len()
  }
}

/// A documentation entry as a Markdown description: its first paragraph,
/// and a link to the reference.
fn describe(entry: &DocEntry) -> Option<String> {
  let text = entry.documentation.as_deref()?.trim();
  let first = text.split("\n\n").next().unwrap_or(text).trim();
  if first.is_empty() {
    return None;
  }
  let mut text = first
    .replace("&times;", "×")
    .replace("&nbsp;", " ")
    .replace("&lt;", "<")
    .replace("&gt;", ">")
    .replace("&quot;", "\"")
    .replace("&#39;", "'")
    .replace("&amp;", "&");
  if text.chars().count() > MAX_DOC_CHARS {
    text = text.chars().take(MAX_DOC_CHARS).collect::<String>() + "…";
  }
  if let Some(link) = &entry.learn_more_link {
    text.push_str(&format!("\n\n[Reference]({link})"));
  }
  Some(text)
}

/// The explicit form any property can be written in, `{ "Vector3": [...] }`.
fn explicit() -> Json {
  json!({
    "type": "object",
    "description": "The explicit form: `{ \"<type>\": value }`, such as `{ \"Vector3\": [0, 1, 0] }`."
  })
}

fn numbers(count: usize) -> Json {
  json!({ "type": "array", "items": { "type": "number" }, "minItems": count, "maxItems": count })
}

/// What a property of this type can be in a project file, as Rojo reads
/// it: the implicit forms it knows, or the explicit one. Types it has no
/// implicit form for take anything.
fn value_schema(value: &ValueType, enums: &HashSet<String>) -> Json {
  let implicit = match (value.category.as_str(), value.name.as_str()) {
    ("Primitive", "bool") => json!({ "type": "boolean" }),
    ("Primitive", "int" | "int64") => json!({ "type": "integer" }),
    ("Primitive", "float" | "double") => json!({ "type": "number" }),
    ("Primitive", "string")
    | ("DataType", "Content" | "ContentId" | "BinaryString" | "ProtectedString") => {
      json!({ "type": "string" })
    }
    ("Enum", name) if enums.contains(name) => {
      return json!({ "$ref": format!("#/definitions/enum/{name}") });
    }
    ("DataType", "Vector3" | "Vector3int16") => numbers(3),
    ("DataType", "Vector2" | "Vector2int16") => numbers(2),
    ("DataType", "Color3" | "Color3uint8") => {
      let mut color = numbers(3);
      color["description"] = json!("Red, green, and blue, from 0 to 1.");
      color
    }
    ("DataType", "UDim" | "NumberRange") => numbers(2),
    ("DataType", "UDim2" | "Rect") => {
      json!({ "type": "array", "items": numbers(2), "minItems": 2, "maxItems": 2 })
    }
    ("DataType", "BrickColor") => json!({ "type": ["integer", "string"] }),
    ("DataType", "Font") => json!({
      "type": "object",
      "properties": {
        "family": { "type": "string", "description": "The font family's asset, such as `rbxasset://fonts/families/SourceSansPro.json`." },
        "weight": { "type": "string", "examples": ["Regular", "Bold"] },
        "style": { "enum": ["Normal", "Italic"] }
      }
    }),
    ("DataType", "Faces" | "Axes") => json!({ "type": "array", "items": { "type": "string" } }),
    _ => return json!({}),
  };
  json!({ "anyOf": [implicit, explicit()] })
}

/// The definitions both schemas share: class names, each class's
/// properties, and enums.
fn definitions(api: &Api) -> Map<String, Json> {
  let mut defs = Map::new();
  let usable: Vec<&Class> = api.classes.iter().filter(|class| class.usable).collect();
  defs.insert(
    "className".into(),
    if usable.is_empty() {
      json!({ "type": "string", "description": "The instance's class, such as `Folder` or `Part`." })
    } else {
      json!({
        "type": "string",
        "description": "The instance's class, such as `Folder` or `Part`.",
        "enum": usable.iter().map(|class| class.name.as_str()).collect::<Vec<_>>(),
        "markdownEnumDescriptions": usable
          .iter()
          .map(|class| {
            let mut doc = class.doc.clone().unwrap_or_default();
            if class.deprecated {
              doc = format!("Deprecated. {doc}");
            }
            doc
          })
          .collect::<Vec<_>>(),
      })
    },
  );
  let mut props = Map::new();
  for class in &api.classes {
    let mut properties = Map::new();
    for property in &class.properties {
      let mut schema = property.schema.clone();
      let description = match &property.doc {
        Some(doc) => format!("**{}**. {doc}", property.kind),
        None => format!("**{}**.", property.kind),
      };
      if let Json::Object(map) = &mut schema {
        map.insert("markdownDescription".into(), json!(description));
        if property.deprecated {
          map.insert("deprecated".into(), json!(true));
          map.insert(
            "deprecationMessage".into(),
            json!("Roblox deprecated this property."),
          );
        }
      }
      properties.insert(property.name.clone(), schema);
    }
    let mut def = json!({ "type": "object", "properties": properties });
    if let Some(superclass) = &class.superclass {
      def["allOf"] = json!([{ "$ref": format!("#/definitions/props/{superclass}") }]);
    }
    props.insert(class.name.clone(), def);
  }
  defs.insert("props".into(), Json::Object(props));
  let mut enums = Map::new();
  for e in &api.enums {
    enums.insert(
      e.name.clone(),
      json!({
        "anyOf": [
          {
            "enum": e.items.iter().map(|(name, _)| name.as_str()).collect::<Vec<_>>(),
            "markdownEnumDescriptions": e
              .items
              .iter()
              .map(|(_, doc)| doc.clone().unwrap_or_default())
              .collect::<Vec<_>>(),
          },
          { "type": "integer", "description": "The item's value." },
          explicit(),
        ]
      }),
    );
  }
  defs.insert("enum".into(), Json::Object(enums));
  defs
}

/// For each class: when `class_key` names it, `properties_key` is its
/// properties.
fn by_class(api: &Api, class_key: &str, properties_key: &str) -> Vec<Json> {
  api
    .classes
    .iter()
    .filter(|class| class.usable)
    .map(|class| {
      json!({
        "if": { "properties": { class_key: { "const": class.name } }, "required": [class_key] },
        "then": {
          "properties": { properties_key: { "$ref": format!("#/definitions/props/{}", class.name) } }
        }
      })
    })
    .collect()
}

/// The schema for `*.project.json` files.
pub fn project_schema(api: &Api) -> String {
  let mut defs = definitions(api);
  let mut instance = json!({
    "type": "object",
    "description": "An instance: its children are its keys that don't start with `$`.",
    "properties": {
      "$className": {
        "$ref": "#/definitions/className",
        "description": "The instance's class. Without it, a `$path` decides it, or the name does for a service under a DataModel; otherwise it's a Folder."
      },
      "$path": {
        "anyOf": [
          { "type": "string" },
          {
            "type": "object",
            "required": ["optional"],
            "properties": { "optional": { "type": "string", "description": "A path that may not exist." } }
          }
        ],
        "description": "A file or folder, relative to the project file, that the instance is made from."
      },
      "$properties": {
        "type": "object",
        "description": "The instance's properties, by name: `\"Anchored\": true`, `\"Size\": [4, 1, 2]`, `\"Material\": \"Plastic\"`."
      },
      "$attributes": { "type": "object", "description": "The instance's attributes, by name." },
      "$ignoreUnknownInstances": {
        "type": "boolean",
        "description": "Keep instances in Studio that the project doesn't have, rather than removing them when syncing. On by default for instances without `$path`."
      },
      "$id": {
        "type": "string",
        "description": "A name other instances' properties refer to this one by."
      }
    },
    "patternProperties": { "^[^$]": { "$ref": "#/definitions/instance" } },
    "additionalProperties": false
  });
  instance["allOf"] = json!(by_class(api, "$className", "$properties"));
  defs.insert("instance".into(), instance);
  // Services, by name, under a DataModel.
  let mut services = Map::new();
  let mut service_defs = Map::new();
  for class in api.classes.iter().filter(|class| class.service) {
    services.insert(
      class.name.clone(),
      json!({ "$ref": format!("#/definitions/service/{}", class.name) }),
    );
    let mut def = json!({
      "allOf": [{ "$ref": "#/definitions/instance" }],
      "properties": {
        "$properties": { "$ref": format!("#/definitions/props/{}", class.name) }
      }
    });
    if let Some(doc) = &class.doc {
      def["markdownDescription"] = json!(doc);
    }
    service_defs.insert(class.name.clone(), def);
  }
  defs.insert("service".into(), Json::Object(service_defs));
  defs.insert(
    "tree".into(),
    json!({
      "allOf": [
        { "$ref": "#/definitions/instance" },
        {
          "if": { "properties": { "$className": { "const": "DataModel" } }, "required": ["$className"] },
          "then": { "properties": services }
        }
      ]
    }),
  );
  let schema = json!({
    "$schema": "http://json-schema.org/draft-07/schema#",
    "title": "Rojo project",
    "description": "A Rojo project: the instances it makes, from files and from this one.",
    "type": "object",
    "required": ["name", "tree"],
    "properties": {
      "$schema": { "type": "string" },
      "name": { "type": "string", "description": "The project's name: its root instance's, and a model's when built." },
      "tree": {
        "$ref": "#/definitions/tree",
        "description": "The root instance: a DataModel for a place (with services as children), or any instance for a model or plugin."
      },
      "servePort": {
        "type": "integer",
        "minimum": 1,
        "maximum": 65535,
        "description": "The port `rojo serve` listens on (34872 by default)."
      },
      "serveAddress": { "type": "string", "description": "The address `rojo serve` listens on (127.0.0.1 by default)." },
      "serveAllowedHosts": {
        "type": "array",
        "items": { "type": "string" },
        "description": "Extra `Host` and `Origin` values `rojo serve` accepts, beyond localhost and its address."
      },
      "servePlaceIds": {
        "type": "array",
        "items": { "type": "integer" },
        "description": "The only places the Studio plugin may sync this project into."
      },
      "placeId": { "type": "integer", "description": "The place Studio sets its place id to while syncing." },
      "gameId": { "type": "integer", "description": "The experience Studio sets its game id to while syncing." },
      "globIgnorePaths": {
        "type": "array",
        "items": { "type": "string" },
        "description": "Files and folders Rojo leaves out, as globs, such as `**/*.spec.luau`."
      },
      "emitLegacyScripts": {
        "type": "boolean",
        "description": "Make `.server` and `.client` scripts Scripts and LocalScripts (the default), rather than Scripts with a RunContext."
      },
      "syncRules": {
        "type": "array",
        "description": "Which files become which instances, beyond Rojo's own rules.",
        "items": {
          "type": "object",
          "required": ["pattern", "use"],
          "properties": {
            "pattern": { "type": "string", "description": "The files, as a glob, such as `*.data.json`." },
            "use": {
              "type": "string",
              "description": "What they become, such as `json`, `text`, `moduleScript`, or `ignore`.",
              "examples": ["moduleScript", "serverScript", "clientScript", "json", "toml", "text", "csv", "jsonModel", "rbxm", "rbxmx", "project", "ignore"]
            },
            "exclude": { "type": "string", "description": "Files the pattern leaves out, as a glob." },
            "suffix": { "type": "string", "description": "The end of the file name that isn't the instance's name, such as `.data.json`." }
          }
        }
      },
      "syncbackRules": {
        "type": "object",
        "description": "What `rojo syncback` leaves alone: `ignoreTrees`, `ignorePaths`, `ignoreProperties`, and the like."
      }
    },
    "definitions": defs
  });
  schema.to_string()
}

/// The schema for `*.meta.json` and `*.model.json` files.
pub fn instance_file_schema(api: &Api) -> String {
  let defs = definitions(api);
  let schema = json!({
    "$schema": "http://json-schema.org/draft-07/schema#",
    "title": "Rojo instance file",
    "description": "A `.model.json` file is an instance and its children; a `.meta.json` file adds to the instance its file or folder makes.",
    "type": "object",
    "properties": {
      "$schema": { "type": "string" },
      "className": {
        "$ref": "#/definitions/className",
        "description": "The instance's class. In a `.meta.json` beside a folder's `init` file, or for a folder, it changes the folder's class."
      },
      "name": { "type": "string", "description": "A child's name in a `.model.json` file (the file's name otherwise)." },
      "properties": { "type": "object", "description": "The instance's properties, by name." },
      "attributes": { "type": "object", "description": "The instance's attributes, by name." },
      "ignoreUnknownInstances": {
        "type": "boolean",
        "description": "Keep instances in Studio that Rojo doesn't have, rather than removing them when syncing."
      },
      "id": { "type": "string", "description": "A name other instances' properties refer to this one by." },
      "children": {
        "type": "array",
        "items": { "$ref": "#" },
        "description": "In a `.model.json` file: the instance's children."
      }
    },
    "additionalProperties": false,
    "allOf": by_class(api, "className", "properties"),
    "definitions": defs
  });
  schema.to_string()
}

/// Class names, by name, for tests.
#[cfg(test)]
fn class_names(api: &Api) -> std::collections::BTreeMap<&str, &Class> {
  api
    .classes
    .iter()
    .map(|class| (class.name.as_str(), class))
    .collect()
}

#[cfg(test)]
mod tests {
  use super::*;

  const DUMP: &str = r#"{
    "Version": 1,
    "Classes": [
      { "Name": "Instance", "Superclass": "<<<ROOT>>>", "Tags": ["NotCreatable"], "Members": [
        { "MemberType": "Property", "Name": "Name", "ValueType": { "Category": "Primitive", "Name": "string" },
          "Serialization": { "CanLoad": true, "CanSave": true } },
        { "MemberType": "Property", "Name": "ClassName", "ValueType": { "Category": "Primitive", "Name": "string" },
          "Tags": ["ReadOnly", "NotReplicated"] },
        { "MemberType": "Function", "Name": "Destroy" }
      ] },
      { "Name": "Part", "Superclass": "Instance", "Members": [
        { "MemberType": "Property", "Name": "Size", "ValueType": { "Category": "DataType", "Name": "Vector3" } },
        { "MemberType": "Property", "Name": "Material", "ValueType": { "Category": "Enum", "Name": "Material" } }
      ] },
      { "Name": "Workspace", "Superclass": "Instance", "Tags": ["NotCreatable", "Service"], "Members": [
        { "MemberType": "Property", "Name": "Gravity", "ValueType": { "Category": "Primitive", "Name": "float" } }
      ] },
      { "Name": "DataModel", "Superclass": "Instance", "Tags": ["NotCreatable"], "Members": [] }
    ],
    "Enums": [{ "Name": "Material", "Items": [{ "Name": "Plastic", "Value": 256 }, { "Name": "Old", "Value": 1, "Tags": ["Deprecated"] }] }]
  }"#;

  const DOCS: &str = r#"{
    "@roblox/globaltype/Part": { "documentation": "A physical brick.\n\nMore." },
    "@roblox/globaltype/Part.Size": { "documentation": "How big it is &times; 2.", "learn_more_link": "https://example.com/size" }
  }"#;

  #[test]
  fn reads_settable_properties_and_documentation() {
    let api = Api::parse(DUMP.as_bytes(), Some(DOCS.as_bytes())).expect("parses");
    let classes = class_names(&api);
    let instance = classes["Instance"];
    let names: Vec<&str> = instance
      .properties
      .iter()
      .map(|p| p.name.as_str())
      .collect();
    assert_eq!(
      names,
      ["Attributes", "Name", "Tags"],
      "ClassName is read-only"
    );
    assert!(!instance.usable);
    assert!(
      classes["Workspace"].usable,
      "services are, though not creatable"
    );
    assert_eq!(classes["Part"].doc.as_deref(), Some("A physical brick."));
    let size = &classes["Part"].properties[1];
    assert_eq!(size.name, "Size");
    assert_eq!(
      size.doc.as_deref(),
      Some("How big it is × 2.\n\n[Reference](https://example.com/size)")
    );
    assert_eq!(api.enums[0].items.len(), 1, "deprecated items are left out");
  }

  #[test]
  fn project_schemas_pick_properties_by_class_and_service() {
    let api = Api::parse(DUMP.as_bytes(), None).expect("parses");
    let schema: Json = serde_json::from_str(&project_schema(&api)).expect("JSON");
    let defs = &schema["definitions"];
    assert_eq!(
      defs["className"]["enum"],
      json!(["Part", "Workspace", "DataModel"]),
      "usable classes"
    );
    assert_eq!(
      defs["props"]["Part"]["allOf"][0]["$ref"],
      "#/definitions/props/Instance"
    );
    assert_eq!(
      defs["props"]["Part"]["properties"]["Material"]["$ref"],
      "#/definitions/enum/Material"
    );
    assert_eq!(
      defs["instance"]["allOf"][0]["then"]["properties"]["$properties"]["$ref"],
      "#/definitions/props/Part"
    );
    assert_eq!(
      defs["tree"]["allOf"][1]["then"]["properties"]["Workspace"]["$ref"],
      "#/definitions/service/Workspace"
    );
    let file: Json = serde_json::from_str(&instance_file_schema(&api)).expect("JSON");
    assert_eq!(
      file["allOf"][1]["then"]["properties"]["properties"]["$ref"],
      "#/definitions/props/Workspace"
    );
    assert_eq!(file["allOf"].as_array().map(Vec::len), Some(3));
  }

  #[test]
  fn without_the_api_only_the_structure_is_checked() {
    let api = Api::default();
    let schema: Json = serde_json::from_str(&project_schema(&api)).expect("JSON");
    assert_eq!(schema["definitions"]["className"]["type"], "string");
    assert!(schema["definitions"]["className"].get("enum").is_none());
    assert_eq!(schema["required"], json!(["name", "tree"]));
  }

  /// With `ROJO_API_DIR` naming a folder with the real Mini-API-Dump.json
  /// and en-us.json: the schemas fit Flareite's limit of 4 MiB.
  #[test]
  fn real_schemas_fit_the_limit() {
    let Ok(dir) = std::env::var("ROJO_API_DIR") else {
      return;
    };
    let dir = std::path::Path::new(&dir);
    let dump = std::fs::read(dir.join("Mini-API-Dump.json")).expect("dump");
    let docs = std::fs::read(dir.join("docs.json")).expect("docs");
    let api = Api::parse(&dump, Some(&docs)).expect("parses");
    let project = project_schema(&api);
    let file = instance_file_schema(&api);
    eprintln!(
      "{} classes; project schema {} KiB, instance file schema {} KiB",
      api.class_count(),
      project.len() / 1024,
      file.len() / 1024
    );
    assert!(project.len() < 4 * 1024 * 1024);
    assert!(file.len() < 4 * 1024 * 1024);
  }
}
