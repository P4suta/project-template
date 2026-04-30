//! Render context — the immutable input to every layer's `render`
//! call. Built once from repository metadata + user-provided answers,
//! then handed to every layer in the resolved order.
//!
//! The context is *the* place where pure-functional render tests
//! ground their fixtures: identical contexts produce identical
//! patches, every time.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use smol_str::SmolStr;

/// Top-level context exposed to templates as Jinja globals.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Context {
    /// Information about the project being scaffolded.
    pub project: ProjectInfo,
    /// Answers to manifest-declared variables, keyed by variable name.
    #[serde(default)]
    pub answers: BTreeMap<SmolStr, AnswerValue>,
}

/// Repository-level facts derived from GitHub metadata or supplied by
/// the local bootstrap script.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectInfo {
    /// Repository name (e.g. `my-project`).
    pub name: String,
    /// Repository owner / organisation login (e.g. `P4suta`).
    pub owner: String,
    /// One-line description.
    #[serde(default)]
    pub description: String,
    /// Calendar year used in copyright headers.
    pub year: u32,
    /// Display author / copyright holder.
    pub author: String,
    /// Canonical repository URL, if known.
    #[serde(default)]
    pub repository_url: Option<String>,
}

/// Variable answer values.
///
/// Matches the variable types declared in the manifest; the boundary
/// between manifest schema and engine type is kept narrow so the
/// manifest can grow without ripple-changing templates.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum AnswerValue {
    /// A free-form string answer.
    String(String),
    /// A boolean toggle.
    Bool(bool),
    /// An integer answer.
    Int(i64),
    /// A multi-select / list answer.
    List(Vec<String>),
}

impl Context {
    /// Convenience: synthesize a minimal context for snapshot tests.
    /// Production callers build a richer context from CLI options /
    /// GitHub Actions environment.
    #[must_use]
    pub fn for_test(name: &str, owner: &str) -> Self {
        Self {
            project: ProjectInfo {
                name: name.to_owned(),
                owner: owner.to_owned(),
                description: String::new(),
                year: 2026,
                author: owner.to_owned(),
                repository_url: None,
            },
            answers: BTreeMap::new(),
        }
    }
}
