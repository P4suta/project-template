//! Type-state machine that walks a template through its lifecycle.
//!
//! ```text
//! Template<Loaded> --validate-->
//! Template<Validated> --resolve-->
//! Template<Resolved> --render-->
//! Template<Rendered> --apply-->
//! Template<Applied>
//! ```
//!
//! Illegal transitions (e.g. `apply` on a `Loaded` template) are
//! rejected at compile time: there is simply no method named `apply`
//! on `Template<Loaded>`.

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};

use smol_str::SmolStr;

use crate::ctx::Context;
use crate::dag::{ResolvePlan, resolve, verify_registry};
use crate::error::TmplError;
use crate::layer::{Layer, LayerMeta, LayerName, Patch};
use crate::manifest::Manifest;
use crate::render::FilesystemLayer;
use crate::state::{
    AppliedEntry, ContentHash, State, applied_file_entries, hash_patch, merkle_root,
};

/// Sealed state marker — all template states implement this.
pub trait TemplateState: sealed::Sealed {}

mod sealed {
    pub trait Sealed {}
}

/// A template just loaded from disk; nothing has been validated.
#[derive(Debug)]
pub struct Loaded {
    /// Engine root directory (typically `.template/`).
    pub template_root: PathBuf,
    /// Parsed `manifest.toml`.
    pub manifest: Manifest,
    /// Layer registry — every layer the engine knows about, regardless
    /// of whether it ends up in the selection.
    pub layers: BTreeMap<LayerName, Box<dyn Layer>>,
}
impl sealed::Sealed for Loaded {}
impl TemplateState for Loaded {}

/// A template whose manifest passed schema validation. The layer
/// registry has been confirmed to be loadable end-to-end.
#[derive(Debug)]
pub struct Validated {
    /// Parsed manifest.
    pub manifest: Manifest,
    /// Layer registry.
    pub layers: BTreeMap<LayerName, Box<dyn Layer>>,
}
impl sealed::Sealed for Validated {}
impl TemplateState for Validated {}

/// A template with a satisfied selection — DAG resolved, no cycles, no
/// orphan-rule violations, no conflicts.
#[derive(Debug)]
pub struct Resolved {
    /// Layer registry retained so [`Self`] can be rendered.
    pub layers: BTreeMap<LayerName, Box<dyn Layer>>,
    /// Topological order + capability provider map.
    pub plan: ResolvePlan,
    /// Render context.
    pub ctx: Context,
}
impl sealed::Sealed for Resolved {}
impl TemplateState for Resolved {}

/// A template whose patches have been built in-memory; no disk I/O has
/// happened yet.
#[derive(Debug)]
pub struct Rendered {
    /// Patches in apply order.
    pub patches: Vec<Patch>,
    /// Resolution plan (preserved for state-file recording).
    pub plan: ResolvePlan,
    /// Render context (preserved so `apply` can use derived facts).
    pub ctx: Context,
}
impl sealed::Sealed for Rendered {}
impl TemplateState for Rendered {}

/// A template that has been written to disk and recorded in
/// `state.toml`.
#[derive(Debug)]
pub struct Applied {
    /// Updated state file contents.
    pub state: State,
}
impl sealed::Sealed for Applied {}
impl TemplateState for Applied {}

/// Generic template wrapper. The `S` parameter encodes the lifecycle
/// position; transition methods are implemented per state.
#[derive(Debug)]
pub struct Template<S: TemplateState> {
    /// State-specific payload.
    pub inner: S,
}

// --- Loaded -----------------------------------------------------------------

impl Template<Loaded> {
    /// Load a template from a directory, expecting:
    ///
    /// * `<root>/manifest.toml`
    /// * `<root>/layers/<name>/{layer.toml,files/...}` for each layer
    ///
    /// # Errors
    ///
    /// Bubbles up [`TmplError::Io`] / [`TmplError::Toml`] /
    /// [`TmplError::Schema`] for unreadable, unparseable, or
    /// schema-failing inputs.
    pub fn load(template_root: &Path) -> Result<Self, TmplError> {
        let manifest = Manifest::load(&template_root.join("manifest.toml"))?;
        let layers_dir = template_root.join("layers");
        let mut layers: BTreeMap<LayerName, Box<dyn Layer>> = BTreeMap::new();
        if layers_dir.exists() {
            let read = fs::read_dir(&layers_dir).map_err(|source| TmplError::Io {
                path: layers_dir.clone(),
                source,
            })?;
            for entry in read {
                let entry = entry.map_err(|source| TmplError::Io {
                    path: layers_dir.clone(),
                    source,
                })?;
                if !entry.file_type().is_ok_and(|t| t.is_dir()) {
                    continue;
                }
                let path = entry.path();
                let layer = FilesystemLayer::load(&path)?;
                let name = Layer::meta(&layer).name.clone();
                layers.insert(name, Box::new(layer));
            }
        }
        Ok(Self {
            inner: Loaded {
                template_root: template_root.to_owned(),
                manifest,
                layers,
            },
        })
    }

    /// Lift to the `Validated` state.
    ///
    /// Runs whole-registry sanity checks via
    /// [`crate::dag::verify_registry`] (asymmetric `conflicts-with`
    /// declarations, dangling references). Per-layer manifest schema
    /// validation already happened during [`Self::load`]; this gate
    /// catches the *cross-layer* invariants that no single layer can
    /// detect on its own.
    ///
    /// # Errors
    ///
    /// Returns [`TmplError::Dag`] wrapping the first cross-layer
    /// invariant violation. The full set is available via
    /// `verify_registry` if a caller wants to enumerate.
    pub fn validate(self) -> Result<Template<Validated>, TmplError> {
        let registry: HashMap<LayerName, LayerMeta> = self
            .inner
            .layers
            .iter()
            .map(|(name, l)| (name.clone(), l.meta().clone()))
            .collect();
        let report = verify_registry(&registry);
        if let Some(first) = report.errors.into_iter().next() {
            return Err(TmplError::Dag { kind: first });
        }
        Ok(Template {
            inner: Validated {
                manifest: self.inner.manifest,
                layers: self.inner.layers,
            },
        })
    }
}

// --- Validated --------------------------------------------------------------

impl Template<Validated> {
    /// Resolve a selection against the registry. Defaults to the
    /// manifest's `default_selection` when the input slice is empty.
    ///
    /// # Errors
    ///
    /// Returns [`TmplError::Dag`] for any DAG resolution failure.
    pub fn resolve(
        self,
        selection: &[LayerName],
        ctx: Context,
    ) -> Result<Template<Resolved>, TmplError> {
        let chosen: &[LayerName] = if selection.is_empty() {
            &self.inner.manifest.default_selection
        } else {
            selection
        };
        let registry: HashMap<LayerName, LayerMeta> = self
            .inner
            .layers
            .iter()
            .map(|(name, l)| (name.clone(), l.meta().clone()))
            .collect();
        let plan = resolve(chosen, &registry)?;
        Ok(Template {
            inner: Resolved {
                layers: self.inner.layers,
                plan,
                ctx,
            },
        })
    }
}

// --- Resolved ---------------------------------------------------------------

impl Template<Resolved> {
    /// Render every layer in topological order. Pure; no I/O.
    ///
    /// # Errors
    ///
    /// Returns [`TmplError::Render`] when a layer's template raises a
    /// `minijinja` error.
    ///
    /// # Panics
    ///
    /// Panics if the resolved plan references a layer that is not in
    /// the registry — this indicates an internal bug in
    /// [`crate::dag::resolve`] rather than a user-observable error.
    pub fn render(self) -> Result<Template<Rendered>, TmplError> {
        let Resolved { layers, plan, ctx } = self.inner;
        let mut patches = Vec::with_capacity(plan.order.len());
        for name in &plan.order {
            let layer = layers
                .get(name)
                .expect("plan only references registered layers");
            patches.push(layer.render(&ctx)?);
        }
        Ok(Template {
            inner: Rendered { patches, plan, ctx },
        })
    }
}

// --- Rendered ---------------------------------------------------------------

impl Template<Rendered> {
    /// Write every rendered patch to `dest` and record an updated
    /// state file.
    ///
    /// Phase A semantics: this is the *initial* apply. Re-applying
    /// over a pre-existing state file is reserved for `tmpl add` /
    /// `tmpl apply --update` (Phase B), which performs the 3-way
    /// merge dance.
    ///
    /// # Errors
    ///
    /// [`TmplError::Io`] for filesystem failures.
    /// # Errors
    ///
    /// Bubbles [`TmplError::Io`] for any filesystem-level failure
    /// (directory create, file write, permission update, state-file
    /// write).
    pub fn apply(self, dest: &Path) -> Result<Template<Applied>, TmplError> {
        let Rendered { patches, .. } = self.inner;

        let now = jiff::Timestamp::now()
            .strftime("%Y-%m-%dT%H:%M:%SZ")
            .to_string();
        let mut applied = BTreeMap::new();

        for patch in &patches {
            for f in &patch.files {
                let abs = dest.join(f.path.as_path().as_str());
                if let Some(parent) = abs.parent() {
                    fs::create_dir_all(parent).map_err(|source| TmplError::Io {
                        path: parent.to_owned(),
                        source,
                    })?;
                }
                fs::write(&abs, f.content.as_bytes()).map_err(|source| TmplError::Io {
                    path: abs.clone(),
                    source,
                })?;
                #[cfg(unix)]
                if f.executable {
                    use std::os::unix::fs::PermissionsExt;
                    let mut perms = fs::metadata(&abs)
                        .map_err(|source| TmplError::Io {
                            path: abs.clone(),
                            source,
                        })?
                        .permissions();
                    perms.set_mode(perms.mode() | 0o755);
                    fs::set_permissions(&abs, perms).map_err(|source| TmplError::Io {
                        path: abs.clone(),
                        source,
                    })?;
                }
            }
            let h = hash_patch(patch);
            applied.insert(
                patch.layer.clone(),
                AppliedEntry {
                    content_hash: h,
                    applied_at: SmolStr::new(&now),
                    files: applied_file_entries(patch),
                },
            );
        }

        let merkle = merkle_root(
            &applied
                .iter()
                .map(|(k, v)| (k.clone(), v.content_hash))
                .collect(),
        );

        let state = State {
            engine_version: SmolStr::new(env!("CARGO_PKG_VERSION")),
            merkle_root: merkle,
            applied,
        };

        let state_path = dest.join(".template").join("state.toml");
        if let Some(parent) = state_path.parent() {
            fs::create_dir_all(parent).map_err(|source| TmplError::Io {
                path: parent.to_owned(),
                source,
            })?;
        }
        state.save(&state_path)?;

        Ok(Template {
            inner: Applied { state },
        })
    }
}

// --- Applied ----------------------------------------------------------------

impl Template<Applied> {
    /// Borrow the persisted state file contents.
    #[must_use]
    pub fn state(&self) -> &State {
        &self.inner.state
    }

    /// Convenience accessor — Merkle root over the applied set.
    #[must_use]
    pub fn merkle_root(&self) -> ContentHash {
        self.inner.state.merkle_root
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn write_minimal_template(root: &Path) {
        fs::create_dir_all(root.join("layers/core/files")).unwrap();
        fs::write(
            root.join("manifest.toml"),
            r#"
schema_version = 1
engine_version = "0.1.0"
default_selection = ["core"]
"#,
        )
        .unwrap();
        fs::write(
            root.join("layers/core/layer.toml"),
            r#"
name = "core"
description = "core layer"
"#,
        )
        .unwrap();
        fs::write(
            root.join("layers/core/files/README.md.j2"),
            "# {{ project.name }}\n\n(c) {{ project.year }} {{ project.author }}\n",
        )
        .unwrap();
        fs::write(root.join("layers/core/files/.gitignore"), "target/\n").unwrap();
    }

    #[test]
    fn template_full_pipeline_writes_files_and_state() {
        let template_dir = tempfile::tempdir().unwrap();
        write_minimal_template(template_dir.path());

        let dest_dir = tempfile::tempdir().unwrap();

        let loaded = Template::<Loaded>::load(template_dir.path()).unwrap();
        let validated = loaded.validate().unwrap();
        let resolved = validated
            .resolve(&[], Context::for_test("acme", "P4suta"))
            .unwrap();
        let rendered = resolved.render().unwrap();
        let applied = rendered.apply(dest_dir.path()).unwrap();

        let readme = fs::read_to_string(dest_dir.path().join("README.md")).unwrap();
        assert!(readme.starts_with("# acme"));
        assert!(readme.contains("(c) 2026 P4suta"));

        let gitignore = fs::read_to_string(dest_dir.path().join(".gitignore")).unwrap();
        assert_eq!(gitignore, "target/\n");

        let state_path = dest_dir.path().join(".template/state.toml");
        assert!(state_path.exists());
        assert!(
            applied
                .state()
                .applied
                .contains_key(&LayerName::new("core").unwrap())
        );
    }

    #[test]
    fn validate_rejects_asymmetric_conflicts_with_dag_error() {
        // Two layers, A says it conflicts with B, B silent. The
        // resolver would let them coexist in selections that mention
        // only B; verify_registry must surface the inconsistency.
        let template_dir = tempfile::tempdir().expect("tempdir");
        fs::create_dir_all(template_dir.path().join("layers/a/files")).expect("mkdir a");
        fs::create_dir_all(template_dir.path().join("layers/b/files")).expect("mkdir b");
        fs::write(
            template_dir.path().join("manifest.toml"),
            "schema_version = 1\nengine_version = \"0.1.0\"\n",
        )
        .expect("write manifest");
        fs::write(
            template_dir.path().join("layers/a/layer.toml"),
            "name = \"a\"\ndescription = \"a\"\nconflicts-with = [\"b\"]\n",
        )
        .expect("write a layer");
        fs::write(
            template_dir.path().join("layers/b/layer.toml"),
            "name = \"b\"\ndescription = \"b\"\n",
        )
        .expect("write b layer");

        let loaded = Template::<Loaded>::load(template_dir.path()).expect("load");
        let err = loaded
            .validate()
            .expect_err("asymmetric conflict must be rejected");
        assert!(matches!(err, TmplError::Dag { .. }));
    }

    #[test]
    fn load_returns_io_error_when_template_root_lacks_manifest() {
        let template_dir = tempfile::tempdir().expect("tempdir");
        let err =
            Template::<Loaded>::load(template_dir.path()).expect_err("missing manifest must error");
        assert!(matches!(err, TmplError::Io { .. }));
    }

    #[test]
    fn load_propagates_filesystem_layer_failure() {
        // A layer subdirectory exists but has no `layer.toml` —
        // `FilesystemLayer::load` fails inside `Template::<Loaded>::load`'s
        // iteration loop. Confirm the error bubbles up.
        let template_dir = tempfile::tempdir().expect("tempdir");
        fs::create_dir_all(template_dir.path().join("layers/broken")).expect("mkdir broken layer");
        fs::write(
            template_dir.path().join("manifest.toml"),
            "schema_version = 1\nengine_version = \"0.1.0\"\n",
        )
        .expect("write manifest");
        let err = Template::<Loaded>::load(template_dir.path())
            .expect_err("missing layer.toml must surface");
        assert!(matches!(err, TmplError::Io { .. }));
    }

    #[test]
    fn apply_surfaces_io_error_when_destination_is_a_regular_file() {
        // `apply` joins each rendered file's relative path under
        // `dest`. If `dest` itself is a regular file rather than a
        // directory, the very first `fs::create_dir_all(parent)`
        // call fails — surface the error rather than panicking.
        let template_dir = tempfile::tempdir().expect("tempdir");
        write_minimal_template(template_dir.path());

        let scratch = tempfile::tempdir().expect("tempdir");
        let blocker = scratch.path().join("blocker");
        fs::write(&blocker, "I am a regular file, not a directory.").expect("write blocker");

        let err = Template::<Loaded>::load(template_dir.path())
            .expect("load")
            .validate()
            .expect("validate")
            .resolve(&[], Context::for_test("acme", "P4suta"))
            .expect("resolve")
            .render()
            .expect("render")
            .apply(&blocker)
            .expect_err("apply onto a regular file must error");
        assert!(matches!(err, TmplError::Io { .. }));
    }

    #[test]
    fn load_skips_non_directory_entries_in_layers_dir() {
        // A stray regular file under layers/ should be skipped
        // silently — only directories are interpreted as layers.
        let template_dir = tempfile::tempdir().expect("tempdir");
        fs::create_dir_all(template_dir.path().join("layers/core/files")).expect("mkdir");
        fs::write(
            template_dir.path().join("manifest.toml"),
            "schema_version = 1\nengine_version = \"0.1.0\"\ndefault_selection = [\"core\"]\n",
        )
        .expect("write manifest");
        fs::write(
            template_dir.path().join("layers/core/layer.toml"),
            "name = \"core\"\ndescription = \"x\"\n",
        )
        .expect("write layer.toml");
        // Stray file at layers/README.md — must be skipped.
        fs::write(template_dir.path().join("layers/README.md"), "# layers\n").expect("write");

        let loaded = Template::<Loaded>::load(template_dir.path()).expect("load");
        assert_eq!(loaded.inner.layers.len(), 1);
    }

    #[test]
    fn resolve_uses_default_selection_when_input_is_empty() {
        let template_dir = tempfile::tempdir().expect("tempdir");
        write_minimal_template(template_dir.path());
        let resolved = Template::<Loaded>::load(template_dir.path())
            .expect("load")
            .validate()
            .expect("validate")
            .resolve(&[], Context::for_test("acme", "P4suta"))
            .expect("resolve");
        assert_eq!(resolved.inner.plan.order.len(), 1);
        assert_eq!(resolved.inner.plan.order[0].as_str(), "core");
    }

    #[test]
    fn resolve_surfaces_unknown_layer_error() {
        let template_dir = tempfile::tempdir().expect("tempdir");
        write_minimal_template(template_dir.path());
        let validated = Template::<Loaded>::load(template_dir.path())
            .expect("load")
            .validate()
            .expect("validate");
        let bogus = vec![LayerName::new("ghost").expect("name")];
        let err = validated
            .resolve(&bogus, Context::for_test("acme", "P4suta"))
            .expect_err("must reject unknown layer");
        assert!(matches!(err, TmplError::Dag { .. }));
    }

    #[test]
    fn apply_propagates_executable_bit_on_unix() {
        // Only meaningful on Unix targets; the chmod branch is gated
        // behind cfg(unix) and we want it covered. On non-Unix the
        // helper short-circuits and no chmod is performed.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let template_dir = tempfile::tempdir().expect("tempdir");
            fs::create_dir_all(template_dir.path().join("layers/scripted/files"))
                .expect("mkdir layer");
            fs::write(
                template_dir.path().join("manifest.toml"),
                "schema_version = 1\nengine_version = \"0.1.0\"\ndefault_selection = [\"scripted\"]\n",
            )
            .expect("write manifest");
            fs::write(
                template_dir.path().join("layers/scripted/layer.toml"),
                "name = \"scripted\"\ndescription = \"shell script\"\n",
            )
            .expect("write layer.toml");
            let script = template_dir.path().join("layers/scripted/files/run.sh");
            fs::write(&script, "#!/usr/bin/env bash\necho hi\n").expect("write");
            let mut perms = fs::metadata(&script).expect("meta").permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&script, perms).expect("chmod");

            let dest = tempfile::tempdir().expect("tempdir");
            Template::<Loaded>::load(template_dir.path())
                .expect("load")
                .validate()
                .expect("validate")
                .resolve(&[], Context::for_test("acme", "P4suta"))
                .expect("resolve")
                .render()
                .expect("render")
                .apply(dest.path())
                .expect("apply");

            let dest_script = dest.path().join("run.sh");
            assert!(dest_script.exists());
            let mode = fs::metadata(&dest_script)
                .expect("meta")
                .permissions()
                .mode();
            assert_eq!(mode & 0o111, 0o111, "executable bit must propagate");
        }
    }

    #[test]
    fn apply_creates_nested_directories() {
        // Layer with a file under nested/ to drive the
        // create_dir_all branch.
        let template_dir = tempfile::tempdir().expect("tempdir");
        fs::create_dir_all(template_dir.path().join("layers/core/files/docs/adr")).expect("mkdir");
        fs::write(
            template_dir.path().join("manifest.toml"),
            "schema_version = 1\nengine_version = \"0.1.0\"\ndefault_selection = [\"core\"]\n",
        )
        .expect("write manifest");
        fs::write(
            template_dir.path().join("layers/core/layer.toml"),
            "name = \"core\"\ndescription = \"core\"\n",
        )
        .expect("write layer.toml");
        fs::write(
            template_dir
                .path()
                .join("layers/core/files/docs/adr/0001.md"),
            "# 1. Test\n",
        )
        .expect("write nested file");

        let dest_dir = tempfile::tempdir().expect("tempdir");
        Template::<Loaded>::load(template_dir.path())
            .expect("load")
            .validate()
            .expect("validate")
            .resolve(&[], Context::for_test("acme", "P4suta"))
            .expect("resolve")
            .render()
            .expect("render")
            .apply(dest_dir.path())
            .expect("apply");
        assert!(dest_dir.path().join("docs/adr/0001.md").exists());
    }

    #[test]
    fn idempotent_apply_produces_same_merkle_root() {
        // Apply twice into two separate destinations; the Merkle root
        // is a pure function of the layer set + context, so it must
        // match.
        let template_dir = tempfile::tempdir().unwrap();
        write_minimal_template(template_dir.path());

        let dst_a = tempfile::tempdir().unwrap();
        let dst_b = tempfile::tempdir().unwrap();
        let ctx = Context::for_test("acme", "P4suta");

        let run = |dst: &Path| {
            Template::<Loaded>::load(template_dir.path())
                .unwrap()
                .validate()
                .unwrap()
                .resolve(&[], ctx.clone())
                .unwrap()
                .render()
                .unwrap()
                .apply(dst)
                .unwrap()
                .merkle_root()
        };
        assert_eq!(run(dst_a.path()), run(dst_b.path()));
    }
}
