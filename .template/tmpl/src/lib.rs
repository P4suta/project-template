//! `tmpl` — the layer-DAG template engine that drives `project-template`.
//!
//! The engine treats template expansion as the *pure functional evaluation
//! of a layer DAG*. The choices at each architectural seam map to concrete
//! algorithms and data structures rather than ad-hoc string substitution:
//!
//! | Concept                | Realised by                                 |
//! |------------------------|---------------------------------------------|
//! | Layer set              | A finite set of [`layer::Layer`] trait obj. |
//! | `requires` relation    | A directed acyclic graph (`petgraph`).      |
//! | Evaluation order       | Kahn's topological sort.                    |
//! | Cycle detection        | Tarjan's strongly connected components.     |
//! | `provides` uniqueness  | Capability orphan rule (one provider only). |
//! | Apply function         | `(manifest, sel, ctx) → Vec<Patch>` pure.   |
//! | Idempotency state      | BLAKE3 Merkle root + applied layer set.     |
//! | Drift over user edits  | (Phase B) git-style 3-way merge.            |
//!
//! The user-facing surface lives in [`template::Template`], a type-state
//! machine that walks `Loaded → Validated → Resolved → Rendered → Applied`.
//! Illegal transitions (`apply` on a not-yet-rendered template, etc.) are
//! rejected at compile time.

#![deny(missing_docs)]
#![deny(rustdoc::broken_intra_doc_links)]

pub mod ctx;
pub mod dag;
pub mod error;
pub mod layer;
pub mod manifest;
pub mod render;
pub mod state;
pub mod template;

pub use ctx::Context;
pub use dag::{ConflictReport, ResolvePlan};
pub use error::TmplError;
pub use layer::{Capability, Layer, LayerMeta, LayerName, Patch, RenderedFile, RenderedPath};
pub use manifest::Manifest;
pub use state::{
    AppliedEntry, AppliedFileEntry, ContentHash, DriftReport, State, applied_file_entries,
    detect_drift, hash_content,
};
pub use template::{Applied, Loaded, Rendered, Resolved, Template, Validated};
