//! Manifest model — `.template/manifest.toml` as a typed Rust value.
//!
//! The manifest enumerates the layers the engine knows about and the
//! variables the render context expects. It is validated against
//! [`SCHEMA`] (a JSON Schema embedded at compile time) before the rest
//! of the engine sees it.

use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};
use smol_str::SmolStr;

use crate::ctx::AnswerValue;
use crate::error::TmplError;
use crate::layer::LayerName;

/// JSON Schema for `.template/manifest.toml` (loaded as JSON after
/// `toml_edit` parsing). Embedded at compile time so the engine can
/// validate without accessing the filesystem.
pub const SCHEMA: &str = include_str!("../schema/manifest.schema.json");

/// Top-level manifest structure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    /// Manifest schema version. Bumped when [`SCHEMA`] changes shape.
    pub schema_version: u32,
    /// Engine version this manifest is known to be compatible with.
    /// Informational; the engine surfaces a warning on mismatch but
    /// still attempts to load.
    pub engine_version: SmolStr,
    /// Default layer selection — what `tmpl apply` uses if `--layers`
    /// is omitted. Useful for `init.yml` to call `tmpl apply` with no
    /// flags.
    #[serde(default)]
    pub default_selection: Vec<LayerName>,
    /// Variables that the render context will surface as Jinja globals
    /// under `answers.*`.
    #[serde(default)]
    pub variables: Vec<VariableDef>,
}

/// One context variable declared by the manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VariableDef {
    /// Variable name, surfaced to templates as `answers.<name>`.
    pub name: SmolStr,
    /// One-line human description shown by interactive prompts.
    pub description: SmolStr,
    /// Variable type — controls validation and prompt UI.
    #[serde(rename = "type")]
    pub kind: VariableKind,
    /// Default value, used when the user omits an answer.
    #[serde(default)]
    pub default: Option<AnswerValue>,
    /// Whether the engine refuses to render without an answer.
    #[serde(default)]
    pub required: bool,
}

/// Variable types accepted by the manifest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VariableKind {
    /// A free-form string.
    String,
    /// A yes/no toggle.
    Bool,
    /// An integer.
    Int,
    /// A multi-value list.
    List,
}

impl Manifest {
    /// Load and validate a manifest from a `.toml` file.
    ///
    /// # Errors
    ///
    /// * [`TmplError::Io`] — filesystem read failed.
    /// * [`TmplError::Toml`] — TOML parse failed.
    /// * [`TmplError::Schema`] — JSON Schema validation rejected the
    ///   document.
    pub fn load(path: &Path) -> Result<Self, TmplError> {
        let text = fs::read_to_string(path).map_err(|source| TmplError::Io {
            path: path.to_owned(),
            source,
        })?;

        // Parse TOML → serde_json::Value via a round-trip — boon (the JSON
        // Schema validator) operates on `serde_json::Value`, but the
        // authoring surface is TOML. The two formats are isomorphic for
        // the schema's data model.
        let toml_value: toml_edit::DocumentMut =
            text.parse().map_err(|source| TmplError::Toml {
                path: path.to_owned(),
                source,
            })?;
        let json_text =
            serde_json::to_string(&toml_to_json(toml_value.as_item())).map_err(|e| {
                TmplError::Schema {
                    path: path.to_owned(),
                    message: format!("intermediate JSON conversion failed: {e}"),
                }
            })?;
        let json_value: serde_json::Value =
            serde_json::from_str(&json_text).map_err(|e| TmplError::Schema {
                path: path.to_owned(),
                message: format!("intermediate JSON parse failed: {e}"),
            })?;

        // Compile and run the schema validator.
        let mut compiler = boon::Compiler::new();
        let schema_value: serde_json::Value =
            serde_json::from_str(SCHEMA).map_err(|e| TmplError::Schema {
                path: path.to_owned(),
                message: format!("embedded schema parse failed: {e}"),
            })?;
        compiler
            .add_resource("manifest.schema.json", schema_value)
            .map_err(|e| TmplError::Schema {
                path: path.to_owned(),
                message: format!("schema registration failed: {e}"),
            })?;
        let mut schemas = boon::Schemas::new();
        let sch = compiler
            .compile("manifest.schema.json", &mut schemas)
            .map_err(|e| TmplError::Schema {
                path: path.to_owned(),
                message: format!("schema compile failed: {e}"),
            })?;
        if let Err(err) = schemas.validate(&json_value, sch) {
            return Err(TmplError::Schema {
                path: path.to_owned(),
                message: format!("{err}"),
            });
        }

        // Final deserialise into the typed model.
        let manifest: Self = serde_json::from_value(json_value).map_err(|e| TmplError::Schema {
            path: path.to_owned(),
            message: format!("typed deserialise failed: {e}"),
        })?;
        Ok(manifest)
    }
}

/// Convert a `toml_edit::Item` into a `serde_json::Value` for schema
/// validation.
///
/// TOML and JSON share a data model — string / int / float / bool /
/// array / table — so the conversion is mechanical.
fn toml_to_json(item: &toml_edit::Item) -> serde_json::Value {
    use serde_json::Value;
    match item {
        toml_edit::Item::None => Value::Null,
        toml_edit::Item::Value(v) => toml_value_to_json(v),
        toml_edit::Item::Table(t) => {
            let mut m = serde_json::Map::new();
            for (k, v) in t {
                m.insert(k.to_owned(), toml_to_json(v));
            }
            Value::Object(m)
        }
        toml_edit::Item::ArrayOfTables(arr) => Value::Array(
            arr.iter()
                .map(|t| {
                    let mut m = serde_json::Map::new();
                    for (k, v) in t {
                        m.insert(k.to_owned(), toml_to_json(v));
                    }
                    Value::Object(m)
                })
                .collect(),
        ),
    }
}

fn toml_value_to_json(v: &toml_edit::Value) -> serde_json::Value {
    use serde_json::Value;
    match v {
        toml_edit::Value::String(s) => Value::String(s.value().clone()),
        toml_edit::Value::Integer(i) => Value::Number((*i.value()).into()),
        toml_edit::Value::Float(f) => {
            serde_json::Number::from_f64(*f.value()).map_or(Value::Null, Value::Number)
        }
        toml_edit::Value::Boolean(b) => Value::Bool(*b.value()),
        toml_edit::Value::Datetime(d) => Value::String(d.to_string()),
        toml_edit::Value::Array(arr) => Value::Array(arr.iter().map(toml_value_to_json).collect()),
        toml_edit::Value::InlineTable(t) => {
            let mut m = serde_json::Map::new();
            for (k, v) in t {
                m.insert(k.to_owned(), toml_value_to_json(v));
            }
            Value::Object(m)
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn write(dir: &Path, body: &str) -> PathBuf {
        let p = dir.join("manifest.toml");
        fs::write(&p, body).expect("write manifest");
        p
    }

    #[test]
    fn load_minimal_manifest() {
        let dir = tempfile::tempdir().expect("tempdir");
        let p = write(
            dir.path(),
            "schema_version = 1\nengine_version = \"0.1.0\"\n",
        );
        let m = Manifest::load(&p).expect("minimal manifest is valid");
        assert_eq!(m.schema_version, 1);
        assert_eq!(m.engine_version.as_str(), "0.1.0");
        assert!(m.default_selection.is_empty());
        assert!(m.variables.is_empty());
    }

    #[test]
    fn load_full_manifest_with_variables() {
        let dir = tempfile::tempdir().expect("tempdir");
        let body = r#"
schema_version = 1
engine_version = "0.2.3"
default_selection = ["core", "typos"]

[[variables]]
name = "license_choice"
description = "SPDX shortlist"
type = "string"
default = "MIT"
required = false

[[variables]]
name = "ship_dockerfile"
description = "include a Dockerfile?"
type = "bool"
default = true
required = false

[[variables]]
name = "min_node_major"
description = "minimum node major version"
type = "int"
default = 22
required = false

[[variables]]
name = "topics"
description = "GitHub topics list"
type = "list"
default = ["rust", "template"]
required = false
"#;
        let p = write(dir.path(), body);
        let m = Manifest::load(&p).expect("full manifest is valid");
        assert_eq!(m.default_selection.len(), 2);
        assert_eq!(m.variables.len(), 4);
        assert!(matches!(m.variables[0].kind, VariableKind::String));
        assert!(matches!(m.variables[1].kind, VariableKind::Bool));
        assert!(matches!(m.variables[2].kind, VariableKind::Int));
        assert!(matches!(m.variables[3].kind, VariableKind::List));
    }

    #[test]
    fn load_rejects_missing_file_with_io_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        let p = dir.path().join("nope.toml");
        let err = Manifest::load(&p).expect_err("must fail");
        assert!(matches!(err, TmplError::Io { .. }));
    }

    #[test]
    fn load_rejects_invalid_toml_syntax() {
        let dir = tempfile::tempdir().expect("tempdir");
        let p = write(dir.path(), "not = valid = toml = [[\n");
        let err = Manifest::load(&p).expect_err("must fail");
        assert!(matches!(err, TmplError::Toml { .. }));
    }

    #[test]
    fn load_rejects_schema_violation_missing_required_field() {
        let dir = tempfile::tempdir().expect("tempdir");
        // Missing `engine_version`.
        let p = write(dir.path(), "schema_version = 1\n");
        let err = Manifest::load(&p).expect_err("must fail");
        assert!(matches!(err, TmplError::Schema { .. }));
    }

    #[test]
    fn load_rejects_schema_violation_bad_layer_name() {
        let dir = tempfile::tempdir().expect("tempdir");
        // Uppercase letter violates the layer-name pattern.
        let body =
            "schema_version = 1\nengine_version = \"0.1.0\"\ndefault_selection = [\"BAD-NAME\"]\n";
        let p = write(dir.path(), body);
        let err = Manifest::load(&p).expect_err("must fail");
        assert!(matches!(err, TmplError::Schema { .. }));
    }

    #[test]
    fn toml_value_to_json_handles_each_variant() {
        // Build a toml document that exercises every leaf kind.
        let src = r#"
schema_version = 1
engine_version = "0.1.0"

[[variables]]
name = "x"
description = "x"
type = "string"

[[variables]]
name = "y"
description = "y"
type = "int"
"#;
        // Indirectly exercises toml_to_json + toml_value_to_json by
        // round-tripping through Manifest::load.
        let dir = tempfile::tempdir().expect("tempdir");
        let p = write(dir.path(), src);
        let m = Manifest::load(&p).expect("valid manifest");
        assert_eq!(m.variables.len(), 2);
    }

    #[test]
    fn toml_to_json_preserves_floats_and_datetime() {
        // Floats and datetimes don't appear in the manifest schema, but
        // the helper supports them for future variable types. Drive the
        // helpers directly so the variants are covered.
        let doc: toml_edit::DocumentMut = "f = 1.5\n[t]\ndate = 1979-05-27T07:32:00Z\n"
            .parse()
            .expect("valid toml");
        let json = toml_to_json(doc.as_item());
        let obj = json.as_object().expect("object");
        let f = obj.get("f").expect("f").as_f64().expect("float");
        assert!((f - 1.5).abs() < f64::EPSILON);
        let t = obj.get("t").expect("t").as_object().expect("object");
        assert!(t.get("date").is_some());
    }

    #[test]
    fn toml_to_json_handles_inline_table_and_array() {
        // Inline tables and arrays-of-leaves go through
        // `toml_value_to_json`, not `toml_to_json`. Cover them.
        let doc: toml_edit::DocumentMut = "x = [1, 2, 3]\ny = { a = 1, b = 2 }\n"
            .parse()
            .expect("valid toml");
        let json = toml_to_json(doc.as_item());
        let obj = json.as_object().expect("object");
        assert_eq!(
            obj.get("x").and_then(|v| v.as_array()).map(Vec::len),
            Some(3)
        );
        assert!(obj.get("y").and_then(|v| v.as_object()).is_some());
    }

    #[test]
    fn toml_to_json_handles_array_of_tables() {
        let doc: toml_edit::DocumentMut = "[[items]]\nname = \"a\"\n\n[[items]]\nname = \"b\"\n"
            .parse()
            .expect("valid toml");
        let json = toml_to_json(doc.as_item());
        let items = json
            .as_object()
            .expect("object")
            .get("items")
            .expect("items key")
            .as_array()
            .expect("array");
        assert_eq!(items.len(), 2);
    }

    #[test]
    fn toml_to_json_handles_boolean() {
        let doc: toml_edit::DocumentMut = "flag = true\n".parse().expect("valid");
        let json = toml_to_json(doc.as_item());
        assert_eq!(
            json.as_object()
                .and_then(|o| o.get("flag"))
                .and_then(serde_json::Value::as_bool),
            Some(true),
        );
    }

    #[test]
    fn toml_to_json_handles_none() {
        // An empty document parses to an Item::None at the root key
        // when accessed via `.get(...)`. We can exercise the None arm
        // via an empty toml table reference.
        let doc: toml_edit::DocumentMut = "".parse().expect("valid empty toml");
        let json = toml_to_json(doc.as_item());
        // Empty root parses to an empty table — confirm and then drive
        // the None arm via a missing index.
        assert!(json.is_object());
        let missing = doc
            .as_item()
            .get("nope")
            .cloned()
            .unwrap_or(toml_edit::Item::None);
        let json_none = toml_to_json(&missing);
        assert!(json_none.is_null());
    }
}
