//! Diagnostics for the engine. `TmplError` is the single error type that
//! crosses module boundaries; per-module wrappers feed into it.

use std::io;
use std::path::PathBuf;

use miette::Diagnostic;
use smol_str::SmolStr;
use thiserror::Error;

use crate::dag::ResolveError;

/// Engine-level error type. Each variant carries enough context to be
/// rendered by [`miette`] without further enrichment at the call site.
#[derive(Debug, Error, Diagnostic)]
pub enum TmplError {
    /// I/O failure with a labelled path.
    #[error("I/O error at {path}: {source}")]
    #[diagnostic(code(tmpl::io))]
    Io {
        /// The path the engine was reading or writing.
        path: PathBuf,
        /// The wrapped `std::io::Error`.
        #[source]
        source: io::Error,
    },

    /// TOML parse / serialise failure.
    #[error("TOML error in {path}: {source}")]
    #[diagnostic(code(tmpl::toml))]
    Toml {
        /// The TOML document the engine was processing.
        path: PathBuf,
        /// The underlying parse error.
        #[source]
        source: toml_edit::TomlError,
    },

    /// JSON Schema validation rejected the manifest.
    #[error("manifest schema violation in {path}: {message}")]
    #[diagnostic(code(tmpl::schema))]
    Schema {
        /// The manifest path that failed validation.
        path: PathBuf,
        /// A human-readable summary of the violation.
        message: String,
    },

    /// The layer DAG could not be resolved.
    #[error("DAG resolution failed: {kind}")]
    #[diagnostic(code(tmpl::dag))]
    Dag {
        /// Structured reason — see [`crate::dag::ResolveError`].
        kind: ResolveError,
    },

    /// `minijinja` failed while rendering a layer template.
    #[error("template render failed for layer {layer}, file {file}: {source}")]
    #[diagnostic(code(tmpl::render))]
    Render {
        /// The layer name being rendered.
        layer: SmolStr,
        /// The relative path within the layer.
        file: String,
        /// The underlying `minijinja` error.
        #[source]
        source: minijinja::Error,
    },

    /// State file parse / encode failure.
    #[error("state file {path} is corrupt: {message}")]
    #[diagnostic(code(tmpl::state))]
    State {
        /// The state file path.
        path: PathBuf,
        /// What the engine could not interpret.
        message: String,
    },

    /// A user edit conflicts with a pending re-apply (Phase B).
    #[error("re-apply would overwrite local edits in {path}")]
    #[diagnostic(code(tmpl::drift))]
    Drift {
        /// The conflicting file.
        path: PathBuf,
    },
}

impl From<ResolveError> for TmplError {
    fn from(kind: ResolveError) -> Self {
        Self::Dag { kind }
    }
}

#[cfg(test)]
mod tests {
    use std::io;

    use super::*;
    use crate::layer::LayerName;

    #[test]
    fn resolve_error_lifts_into_tmpl_error() {
        let r = ResolveError::UnknownLayer(LayerName::new("ghost").expect("valid name"));
        let e: TmplError = r.into();
        assert!(matches!(e, TmplError::Dag { .. }));
    }

    #[test]
    fn io_variant_displays_path_and_message() {
        let e = TmplError::Io {
            path: PathBuf::from("/tmp/missing"),
            source: io::Error::new(io::ErrorKind::NotFound, "vanished"),
        };
        let rendered = format!("{e}");
        assert!(rendered.contains("/tmp/missing"));
        assert!(rendered.contains("vanished"));
    }

    #[test]
    fn schema_variant_displays_message() {
        let e = TmplError::Schema {
            path: PathBuf::from("/tmp/m.toml"),
            message: "missing required field".into(),
        };
        let rendered = format!("{e}");
        assert!(rendered.contains("/tmp/m.toml"));
        assert!(rendered.contains("missing required field"));
    }

    #[test]
    fn drift_variant_displays_path() {
        let e = TmplError::Drift {
            path: PathBuf::from("/tmp/drifted"),
        };
        assert!(format!("{e}").contains("/tmp/drifted"));
    }

    #[test]
    fn state_variant_displays_message() {
        let e = TmplError::State {
            path: PathBuf::from("/tmp/state.toml"),
            message: "bad shape".into(),
        };
        assert!(format!("{e}").contains("bad shape"));
    }
}
