//! Layer trait and its supporting newtypes.
//!
//! A *layer* is one composable unit of project scaffolding: a set of
//! template files paired with a metadata block that declares the layer's
//! place in the DAG. The trait keeps the surface narrow so that tests can
//! drive the engine with hand-built mock layers and the production path
//! can serve filesystem-loaded layers through the same API.

use std::fmt;

use camino::Utf8PathBuf;
use serde::{Deserialize, Serialize};
use smol_str::SmolStr;

use crate::ctx::Context;
use crate::error::TmplError;

// ---------------------------------------------------------------------------
// Newtypes — encode invariants in the type system rather than at call sites.
// ---------------------------------------------------------------------------

/// A layer's identifier. Stored as a `SmolStr` so the common case (short
/// lower-case-kebab names like `core` / `rust-workspace`) lives inline
/// without heap allocation.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct LayerName(SmolStr);

impl LayerName {
    /// Construct a layer name. Names must be non-empty and contain only
    /// `[a-z0-9-]` — keeps cross-platform path compatibility.
    ///
    /// # Errors
    ///
    /// * [`NameError::Empty`] if the input is empty.
    /// * [`NameError::Charset`] if any character is outside `[a-z0-9-]`.
    pub fn new(s: impl Into<SmolStr>) -> Result<Self, NameError> {
        let s = s.into();
        if s.is_empty() {
            return Err(NameError::Empty);
        }
        if !s
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        {
            return Err(NameError::Charset(s));
        }
        Ok(Self(s))
    }

    /// Borrow the underlying string.
    #[must_use]
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl fmt::Display for LayerName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0.as_str())
    }
}

/// A capability a layer can `provides` or `requires`.
///
/// Capabilities are the abstract resources that bind layers together
/// (e.g. `container-runtime`, `cargo-workspace`, `git-hooks`); only
/// one layer in any selection may `provides` a given capability — see
/// [`crate::dag::resolve`].
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Capability(SmolStr);

impl Capability {
    /// Construct a capability. Same charset rules as [`LayerName`].
    ///
    /// # Errors
    ///
    /// Same as [`LayerName::new`].
    pub fn new(s: impl Into<SmolStr>) -> Result<Self, NameError> {
        let s = s.into();
        if s.is_empty() {
            return Err(NameError::Empty);
        }
        if !s
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        {
            return Err(NameError::Charset(s));
        }
        Ok(Self(s))
    }

    /// Borrow the underlying string.
    #[must_use]
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl fmt::Display for Capability {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0.as_str())
    }
}

/// Parse failure for [`LayerName`] / [`Capability`]. Held separately so
/// `TmplError` can wrap it with extra context where the parse happens.
#[derive(Debug, thiserror::Error)]
pub enum NameError {
    /// The name was empty.
    #[error("name must not be empty")]
    Empty,
    /// The name contained a forbidden character.
    #[error("name {0:?} contains characters outside [a-z0-9-]")]
    Charset(SmolStr),
}

/// A relative, normalised, UTF-8 path inside the destination repository.
/// Absolute paths and `..` traversal are rejected at construction time.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct RenderedPath(Utf8PathBuf);

impl RenderedPath {
    /// Validate and wrap a path. The path must be relative, non-empty,
    /// and must not contain a `..` component.
    ///
    /// # Errors
    ///
    /// * [`PathError::Empty`] for empty input.
    /// * [`PathError::Absolute`] for absolute paths.
    /// * [`PathError::Traversal`] when the path contains `..`.
    pub fn new(p: impl Into<Utf8PathBuf>) -> Result<Self, PathError> {
        let p = p.into();
        if p.as_str().is_empty() {
            return Err(PathError::Empty);
        }
        if p.is_absolute() {
            return Err(PathError::Absolute(p));
        }
        if p.components()
            .any(|c| matches!(c, camino::Utf8Component::ParentDir))
        {
            return Err(PathError::Traversal(p));
        }
        Ok(Self(p))
    }

    /// Borrow the underlying path.
    #[must_use]
    pub fn as_path(&self) -> &camino::Utf8Path {
        self.0.as_path()
    }
}

/// Validation failure for [`RenderedPath`].
#[derive(Debug, thiserror::Error)]
pub enum PathError {
    /// The path was empty.
    #[error("rendered path must not be empty")]
    Empty,
    /// The path was absolute.
    #[error("rendered path {0} must be relative")]
    Absolute(Utf8PathBuf),
    /// The path contained a `..` component.
    #[error("rendered path {0} must not contain `..`")]
    Traversal(Utf8PathBuf),
}

// ---------------------------------------------------------------------------
// Layer trait + Patch
// ---------------------------------------------------------------------------

/// Static, declared metadata about a layer. Persisted as
/// `.template/layers/<name>/layer.toml`.
///
/// Dependencies between layers are mediated by *capabilities* rather than
/// by layer names, so a generated repository can swap one implementation
/// of a capability for another without touching consumers. The orphan
/// rule for `provides` (one provider per capability per selection) makes
/// the swap explicit at resolution time rather than at runtime.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LayerMeta {
    /// Layer name (must match the directory name).
    pub name: LayerName,
    /// One-line human-readable description.
    pub description: SmolStr,
    /// Capabilities this layer needs from some other layer in the
    /// selection. Each entry must be `provides`d by exactly one selected
    /// layer.
    #[serde(default)]
    pub requires: Vec<Capability>,
    /// Capabilities this layer makes available to dependents.
    #[serde(default)]
    pub provides: Vec<Capability>,
    /// Layer names this layer cannot coexist with. Mutual exclusion is
    /// expressed at the layer level (not the capability level) so that
    /// "either-or" choices like Docker-vs-bare-metal can be authored
    /// without inventing pseudo-capabilities.
    #[serde(default, rename = "conflicts-with")]
    pub conflicts_with: Vec<LayerName>,
}

/// A rendered, in-memory file produced by a layer. Disk I/O is the
/// concern of [`crate::template::Template::apply`]; the render phase is
/// pure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RenderedFile {
    /// Destination path relative to the repository root.
    pub path: RenderedPath,
    /// File content after `minijinja` evaluation.
    pub content: String,
    /// Whether the file should be marked executable on POSIX systems.
    #[serde(default)]
    pub executable: bool,
}

/// The complete render output for a single layer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Patch {
    /// The layer that produced this patch.
    pub layer: LayerName,
    /// Rendered files in deterministic (path-sorted) order.
    pub files: Vec<RenderedFile>,
}

/// A layer is anything that can declare its metadata and render itself
/// into a [`Patch`] given a [`Context`].
///
/// Implementations must be pure: the same `(layer, ctx)` pair must
/// produce the same `Patch`.
pub trait Layer: fmt::Debug + Send + Sync {
    /// Metadata describing the layer's place in the DAG.
    fn meta(&self) -> &LayerMeta;
    /// Render the layer against `ctx`. Side-effect-free.
    ///
    /// # Errors
    ///
    /// Returns [`TmplError::Render`] if a template fails to evaluate;
    /// other variants of [`TmplError`] for I/O / schema failures
    /// during file collection.
    fn render(&self, ctx: &Context) -> Result<Patch, TmplError>;
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layer_name_accepts_canonical_form() {
        let n = LayerName::new("rust-workspace").expect("valid name");
        assert_eq!(n.as_str(), "rust-workspace");
    }

    #[test]
    fn layer_name_rejects_empty() {
        assert!(matches!(LayerName::new(""), Err(NameError::Empty)));
    }

    #[test]
    fn layer_name_rejects_uppercase() {
        let err = LayerName::new("RustWorkspace").expect_err("must reject uppercase");
        assert!(matches!(err, NameError::Charset(_)));
    }

    #[test]
    fn layer_name_rejects_underscore() {
        let err = LayerName::new("rust_workspace").expect_err("must reject underscore");
        assert!(matches!(err, NameError::Charset(_)));
    }

    #[test]
    fn capability_validates_same_rules() {
        let cap = Capability::new("container-runtime")
            .expect("'container-runtime' is the canonical capability shape and must validate");
        assert_eq!(cap.as_str(), "container-runtime");
        assert!(matches!(Capability::new(""), Err(NameError::Empty)));
        assert!(matches!(Capability::new("Bad"), Err(NameError::Charset(_))));
    }

    #[test]
    fn layer_name_displays_as_underlying_string() {
        let n = LayerName::new("rust-workspace").expect("valid");
        assert_eq!(format!("{n}"), "rust-workspace");
    }

    #[test]
    fn capability_displays_as_underlying_string() {
        let c = Capability::new("container-runtime").expect("valid");
        assert_eq!(format!("{c}"), "container-runtime");
    }

    #[test]
    fn name_error_displays_human_readably() {
        assert_eq!(format!("{}", NameError::Empty), "name must not be empty");
        let err = LayerName::new("BAD").expect_err("rejected");
        let rendered = format!("{err}");
        assert!(rendered.contains("characters outside"));
    }

    #[test]
    fn path_error_displays_human_readably() {
        let abs = RenderedPath::new("/etc/passwd").expect_err("absolute rejected");
        assert!(format!("{abs}").contains("must be relative"));
        let trav = RenderedPath::new("foo/../etc").expect_err("traversal rejected");
        assert!(format!("{trav}").contains("must not contain"));
        assert_eq!(
            format!("{}", PathError::Empty),
            "rendered path must not be empty"
        );
    }

    #[test]
    fn rendered_path_rejects_absolute() {
        let err = RenderedPath::new("/etc/passwd").expect_err("must reject absolute");
        assert!(matches!(err, PathError::Absolute(_)));
    }

    #[test]
    fn rendered_path_rejects_parent_traversal() {
        let err = RenderedPath::new("foo/../etc").expect_err("must reject `..`");
        assert!(matches!(err, PathError::Traversal(_)));
    }

    #[test]
    fn rendered_path_rejects_empty() {
        let err = RenderedPath::new("").expect_err("must reject empty");
        assert!(matches!(err, PathError::Empty));
    }

    #[test]
    fn rendered_path_accepts_relative() {
        let p = RenderedPath::new("docs/adr/0001.md").expect("relative path is fine");
        assert_eq!(p.as_path().as_str(), "docs/adr/0001.md");
    }
}
