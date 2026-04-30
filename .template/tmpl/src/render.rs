//! Render — turn a [`Layer`] into a [`Patch`] of fully-substituted
//! file contents.
//!
//! ## Templating convention
//!
//! Files inside a layer's `files/` directory are interpreted as follows:
//!
//! * Files whose name ends with `.j2` are rendered through `minijinja`
//!   (Jinja2-compatible). The `.j2` suffix is stripped from the
//!   destination path. The render context is exposed under `project`
//!   and `answers` Jinja globals (see [`crate::ctx::Context`]).
//! * Files without `.j2` are copied verbatim.
//!
//! This separation keeps assets that legitimately contain `{{` (such
//! as GitHub Actions `${{ secrets }}`) from needing escape syntax —
//! they are simply not given the `.j2` suffix.

use std::fs;
use std::path::Path;

use camino::{Utf8Path, Utf8PathBuf};
use smol_str::SmolStr;
use toml_edit::de as toml_de;

use crate::ctx::Context;
use crate::error::TmplError;
use crate::layer::{Layer, LayerMeta, Patch, PathError, RenderedFile, RenderedPath};

/// A layer materialised from disk under
/// `.template/layers/<name>/{layer.toml,files/...}`.
#[derive(Debug, Clone)]
pub struct FilesystemLayer {
    /// Parsed `layer.toml`.
    meta: LayerMeta,
    /// One template entry per file under `files/`.
    files: Vec<TemplateFile>,
}

/// One rendered-on-demand file.
#[derive(Debug, Clone)]
struct TemplateFile {
    /// Destination path within the generated repository.
    dest: RenderedPath,
    /// Raw content (Jinja source if `templated`, otherwise verbatim).
    content: String,
    /// Whether the file should be passed through `minijinja`.
    templated: bool,
    /// Executable bit, mirrored from the source file's mode on Unix.
    executable: bool,
}

impl FilesystemLayer {
    /// Load the layer at `layer_dir`. Expects `layer.toml` and a
    /// `files/` subdirectory; both are required.
    ///
    /// # Errors
    ///
    /// * [`TmplError::Io`] on read failures.
    /// * [`TmplError::Schema`] when `layer.toml` is unparseable or
    ///   when a template file's path cannot be represented as a
    ///   [`RenderedPath`].
    pub fn load(layer_dir: &Path) -> Result<Self, TmplError> {
        let meta_path = layer_dir.join("layer.toml");
        let meta_text = fs::read_to_string(&meta_path).map_err(|source| TmplError::Io {
            path: meta_path.clone(),
            source,
        })?;
        let meta: LayerMeta = toml_de::from_str(&meta_text).map_err(|e| TmplError::Schema {
            path: meta_path.clone(),
            message: format!("{e}"),
        })?;

        let files_root = layer_dir.join("files");
        let mut files = Vec::new();
        if files_root.exists() {
            collect_files(&files_root, &files_root, &mut files)?;
        }
        files.sort_by(|a, b| a.dest.cmp(&b.dest));

        Ok(Self { meta, files })
    }
}

impl Layer for FilesystemLayer {
    fn meta(&self) -> &LayerMeta {
        &self.meta
    }

    fn render(&self, ctx: &Context) -> Result<Patch, TmplError> {
        let mut env = minijinja::Environment::new();
        // Preserve trailing newlines verbatim — files in a generated
        // repo are line-oriented and `keep_trailing_newline=false`
        // (the Jinja2 default) silently strips the final `\n`, which
        // tools downstream of `tmpl apply` (rustfmt, lefthook,
        // markdownlint) then flag as a missing-final-newline defect.
        env.set_keep_trailing_newline(true);
        let mut files = Vec::with_capacity(self.files.len());
        for f in &self.files {
            let content = if f.templated {
                env.render_str(&f.content, ctx)
                    .map_err(|source| TmplError::Render {
                        layer: SmolStr::new(self.meta.name.as_str()),
                        file: f.dest.as_path().as_str().to_owned(),
                        source,
                    })?
            } else {
                f.content.clone()
            };
            files.push(RenderedFile {
                path: f.dest.clone(),
                content,
                executable: f.executable,
            });
        }
        Ok(Patch {
            layer: self.meta.name.clone(),
            files,
        })
    }
}

/// Recursive directory walk.
///
/// Each entry is classified into `directory` / `symlink` / regular
/// file. Symlinks are intentionally skipped — they are out of scope
/// for layer templates (a layer's content should be self-contained
/// and inspectable without following references).
fn collect_files(root: &Path, dir: &Path, out: &mut Vec<TemplateFile>) -> Result<(), TmplError> {
    let read = fs::read_dir(dir).map_err(|source| TmplError::Io {
        path: dir.to_owned(),
        source,
    })?;
    for entry in read {
        let entry = entry.map_err(|source| TmplError::Io {
            path: dir.to_owned(),
            source,
        })?;
        let path = entry.path();
        let ft = entry.file_type().map_err(|source| TmplError::Io {
            path: path.clone(),
            source,
        })?;
        if ft.is_dir() {
            collect_files(root, &path, out)?;
            continue;
        }
        if ft.is_symlink() {
            // Layers carry their content directly; symlinks are out of scope.
            continue;
        }
        // Anything else (regular file, plus the rare FIFO / socket /
        // device which has no business inside a `files/` tree) is
        // treated as a regular file. The subsequent `read_to_string`
        // will surface non-UTF-8 / non-readable content as an error.
        out.push(load_template_file(root, &path)?);
    }
    Ok(())
}

fn load_template_file(root: &Path, file: &Path) -> Result<TemplateFile, TmplError> {
    let rel = file.strip_prefix(root).map_err(|_| TmplError::Schema {
        path: file.to_owned(),
        message: "file is not under the layer's files/ root".into(),
    })?;
    let rel_utf8 =
        Utf8PathBuf::from_path_buf(rel.to_path_buf()).map_err(|p| TmplError::Schema {
            path: p,
            message: "non-UTF-8 path inside layer".into(),
        })?;

    let (templated, dest_rel) = strip_j2_suffix(&rel_utf8);
    let dest = RenderedPath::new(dest_rel).map_err(|e: PathError| TmplError::Schema {
        path: file.to_owned(),
        message: format!("{e}"),
    })?;

    let content = fs::read_to_string(file).map_err(|source| TmplError::Io {
        path: file.to_owned(),
        source,
    })?;

    let executable = is_executable(file);

    Ok(TemplateFile {
        dest,
        content,
        templated,
        executable,
    })
}

/// Strip a trailing `.j2` extension from the *file name only* (not from
/// directory components). Returns `(templated, dest_path)`.
fn strip_j2_suffix(rel: &Utf8Path) -> (bool, Utf8PathBuf) {
    if rel.extension() == Some("j2") {
        let stem = rel.file_stem().unwrap_or("");
        let parent = rel.parent().unwrap_or_else(|| Utf8Path::new(""));
        let dest = if parent.as_str().is_empty() {
            Utf8PathBuf::from(stem)
        } else {
            parent.join(stem)
        };
        (true, dest)
    } else {
        (false, rel.to_path_buf())
    }
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    fs::metadata(path).is_ok_and(|m| m.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn is_executable(_path: &Path) -> bool {
    false
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_j2_suffix_handles_top_level() {
        let (t, d) = strip_j2_suffix(Utf8Path::new("LICENSE.j2"));
        assert!(t);
        assert_eq!(d.as_str(), "LICENSE");
    }

    #[test]
    fn strip_j2_suffix_handles_nested() {
        let (t, d) = strip_j2_suffix(Utf8Path::new("docs/README.md.j2"));
        assert!(t);
        assert_eq!(d.as_str(), "docs/README.md");
    }

    #[test]
    fn strip_j2_suffix_passes_non_template_through() {
        let (t, d) = strip_j2_suffix(Utf8Path::new(".gitignore"));
        assert!(!t);
        assert_eq!(d.as_str(), ".gitignore");
    }

    #[test]
    fn fs_layer_renders_jinja_and_copies_verbatim() {
        let dir = tempfile::tempdir().expect("tempdir");
        let layer_dir = dir.path().join("core");
        fs::create_dir_all(layer_dir.join("files/docs")).expect("create files/docs");

        fs::write(
            layer_dir.join("layer.toml"),
            "name = \"core\"\ndescription = \"core scaffolding\"\n",
        )
        .expect("write layer.toml");
        fs::write(
            layer_dir.join("files/LICENSE.j2"),
            "Copyright (c) {{ project.year }} {{ project.author }}\n",
        )
        .expect("write LICENSE.j2");
        fs::write(layer_dir.join("files/.gitignore"), "target/\n").expect("write .gitignore");

        let layer = FilesystemLayer::load(&layer_dir).expect("load layer");
        let ctx = Context::for_test("acme", "P4suta");
        let patch = layer.render(&ctx).expect("render");

        // .gitignore is copied verbatim, LICENSE is rendered.
        assert_eq!(patch.files.len(), 2);
        let gitignore = patch
            .files
            .iter()
            .find(|f| f.path.as_path() == ".gitignore")
            .expect(".gitignore must be present");
        assert_eq!(gitignore.content, "target/\n");
        let license = patch
            .files
            .iter()
            .find(|f| f.path.as_path() == "LICENSE")
            .expect("LICENSE must be present");
        assert_eq!(license.content, "Copyright (c) 2026 P4suta\n");
    }

    #[test]
    fn fs_layer_render_is_deterministic() {
        let dir = tempfile::tempdir().expect("tempdir");
        let layer_dir = dir.path().join("core");
        fs::create_dir_all(layer_dir.join("files")).expect("create files");
        fs::write(
            layer_dir.join("layer.toml"),
            "name = \"core\"\ndescription = \"x\"\n",
        )
        .expect("write layer.toml");
        fs::write(layer_dir.join("files/a.txt"), "alpha\n").expect("write a");
        fs::write(layer_dir.join("files/b.txt"), "beta\n").expect("write b");

        let layer = FilesystemLayer::load(&layer_dir).expect("load");
        let ctx = Context::for_test("p", "o");
        let p1 = layer.render(&ctx).expect("render 1");
        let p2 = layer.render(&ctx).expect("render 2");
        assert_eq!(p1, p2);
    }

    #[test]
    fn fs_layer_load_errors_when_layer_toml_missing() {
        let dir = tempfile::tempdir().expect("tempdir");
        let layer_dir = dir.path().join("ghost");
        fs::create_dir_all(&layer_dir).expect("mkdir");
        let err = FilesystemLayer::load(&layer_dir).expect_err("must fail without layer.toml");
        assert!(matches!(err, TmplError::Io { .. }));
    }

    #[test]
    fn fs_layer_load_errors_on_malformed_layer_toml() {
        let dir = tempfile::tempdir().expect("tempdir");
        let layer_dir = dir.path().join("broken");
        fs::create_dir_all(&layer_dir).expect("mkdir");
        fs::write(layer_dir.join("layer.toml"), "this isn't = valid = [[\n").expect("write");
        let err = FilesystemLayer::load(&layer_dir).expect_err("must fail on malformed toml");
        assert!(matches!(err, TmplError::Schema { .. }));
    }

    #[test]
    fn fs_layer_load_surfaces_io_error_when_files_root_is_a_regular_file() {
        // `collect_files` calls `fs::read_dir(dir)`. If the path that
        // claims to be `files/` is actually a regular file (someone
        // committed `files` instead of `files/`), the read fails and
        // we want the structured `TmplError::Io`.
        let dir = tempfile::tempdir().expect("tempdir");
        let layer_dir = dir.path().join("malformed");
        fs::create_dir_all(&layer_dir).expect("mkdir");
        fs::write(
            layer_dir.join("layer.toml"),
            "name = \"malformed\"\ndescription = \"x\"\n",
        )
        .expect("write layer.toml");
        // `files` is a regular file, not a directory.
        fs::write(
            layer_dir.join("files"),
            "I should be a directory but I am not.",
        )
        .expect("write files-as-file");

        let err = FilesystemLayer::load(&layer_dir)
            .expect_err("read_dir on a regular file must surface as Io");
        assert!(matches!(err, TmplError::Io { .. }));
    }

    #[test]
    fn fs_layer_load_handles_layer_with_no_files_dir() {
        // A layer with metadata but no files/ subdir should load to an
        // empty patch — this is the "metadata-only" shape that early
        // composition layers often take.
        let dir = tempfile::tempdir().expect("tempdir");
        let layer_dir = dir.path().join("empty");
        fs::create_dir_all(&layer_dir).expect("mkdir");
        fs::write(
            layer_dir.join("layer.toml"),
            "name = \"empty\"\ndescription = \"no files\"\n",
        )
        .expect("write layer.toml");
        let layer = FilesystemLayer::load(&layer_dir).expect("load");
        let patch = layer.render(&Context::for_test("p", "o")).expect("render");
        assert!(patch.files.is_empty());
    }

    #[test]
    fn fs_layer_render_preserves_executable_bit_on_unix() {
        // POSIX-only assertion. On non-Unix the helper returns false
        // unconditionally, so this test would still pass but does not
        // exercise the bit; gate it.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            let dir = tempfile::tempdir().expect("tempdir");
            let layer_dir = dir.path().join("scripted");
            fs::create_dir_all(layer_dir.join("files")).expect("mkdir");
            fs::write(
                layer_dir.join("layer.toml"),
                "name = \"scripted\"\ndescription = \"shellcheck me\"\n",
            )
            .expect("write layer.toml");
            let script = layer_dir.join("files/run.sh");
            fs::write(&script, "#!/usr/bin/env bash\nexit 0\n").expect("write script");
            let mut perms = fs::metadata(&script).expect("meta").permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&script, perms).expect("chmod");

            let layer = FilesystemLayer::load(&layer_dir).expect("load");
            let patch = layer.render(&Context::for_test("p", "o")).expect("render");
            let entry = patch
                .files
                .iter()
                .find(|f| f.path.as_path() == "run.sh")
                .expect("run.sh");
            assert!(
                entry.executable,
                "executable bit must propagate to the patch"
            );
        }
    }

    #[test]
    fn fs_layer_render_returns_error_for_invalid_jinja() {
        let dir = tempfile::tempdir().expect("tempdir");
        let layer_dir = dir.path().join("syntax-error");
        fs::create_dir_all(layer_dir.join("files")).expect("mkdir");
        fs::write(
            layer_dir.join("layer.toml"),
            "name = \"syntax-error\"\ndescription = \"x\"\n",
        )
        .expect("write layer.toml");
        // Unmatched `{%` is a Jinja syntax error.
        fs::write(
            layer_dir.join("files/broken.txt.j2"),
            "{% if missing_endif }\n",
        )
        .expect("write template");

        let layer = FilesystemLayer::load(&layer_dir).expect("load");
        let err = layer
            .render(&Context::for_test("p", "o"))
            .expect_err("must surface render error");
        assert!(matches!(err, TmplError::Render { .. }));
    }

    #[test]
    fn fs_layer_load_surfaces_io_error_for_unreadable_template_file() {
        // chmod 0000 a file inside files/ — `read_to_string` then
        // fails with PermissionDenied, surfacing as `TmplError::Io`.
        // Skip on root (where chmod 0000 doesn't actually deny reads).
        #[cfg(unix)]
        if !is_running_as_root() {
            use std::os::unix::fs::PermissionsExt;
            let dir = tempfile::tempdir().expect("tempdir");
            let layer_dir = dir.path().join("locked");
            fs::create_dir_all(layer_dir.join("files")).expect("mkdir");
            fs::write(
                layer_dir.join("layer.toml"),
                "name = \"locked\"\ndescription = \"x\"\n",
            )
            .expect("write layer.toml");
            let target = layer_dir.join("files/SECRET");
            fs::write(&target, "private\n").expect("write secret");
            fs::set_permissions(&target, fs::Permissions::from_mode(0o000)).expect("chmod");

            let err = FilesystemLayer::load(&layer_dir)
                .expect_err("unreadable template file must surface as Io");
            assert!(matches!(err, TmplError::Io { .. }));

            // Restore permissions so tempdir cleanup can delete it.
            fs::set_permissions(&target, fs::Permissions::from_mode(0o644)).ok();
        }
    }

    #[cfg(unix)]
    fn is_running_as_root() -> bool {
        // Avoid pulling in the `libc` crate just for a uid check;
        // read `/proc/self/status` instead. Returns false on
        // non-Linux POSIX (no procfs) — we accept the false negative
        // there because the test only matters on Linux CI.
        fs::read_to_string("/proc/self/status").is_ok_and(|s| {
            s.lines()
                .find_map(|l| l.strip_prefix("Uid:"))
                .and_then(|tail| tail.split_whitespace().next())
                .is_some_and(|uid| uid == "0")
        })
    }

    #[test]
    fn fs_layer_load_skips_symlinks() {
        // Symlinks are out of scope for templates. Build a layer with
        // one regular file and one symlink; expect the symlink to
        // disappear from the patch.
        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;

            let dir = tempfile::tempdir().expect("tempdir");
            let layer_dir = dir.path().join("symlinked");
            fs::create_dir_all(layer_dir.join("files")).expect("mkdir");
            fs::write(
                layer_dir.join("layer.toml"),
                "name = \"symlinked\"\ndescription = \"x\"\n",
            )
            .expect("write layer.toml");
            fs::write(layer_dir.join("files/real.txt"), "real\n").expect("write");
            symlink("real.txt", layer_dir.join("files/link.txt")).expect("symlink");

            let layer = FilesystemLayer::load(&layer_dir).expect("load");
            let patch = layer.render(&Context::for_test("p", "o")).expect("render");
            let names: Vec<&str> = patch
                .files
                .iter()
                .map(|f| f.path.as_path().as_str())
                .collect();
            assert!(names.contains(&"real.txt"));
            assert!(!names.contains(&"link.txt"));
        }
    }
}
