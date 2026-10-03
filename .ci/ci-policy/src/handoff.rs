use std::{collections::BTreeSet, fs, path::Path, process::Command};

use anyhow::{Context, Result, ensure};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Change {
    Write,
    Delete,
    Absent,
}

pub fn change(existing: bool, rendered: bool) -> Change {
    match (existing, rendered) {
        (_, true) => Change::Write,
        (true, false) => Change::Delete,
        (false, false) => Change::Absent,
    }
}

fn git(root: &Path, objects: Option<&Path>, arguments: &[&str]) -> Result<Vec<u8>> {
    let mut command = Command::new("git");
    command
        .current_dir(root)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .env_remove("GIT_OBJECT_DIRECTORY")
        .env_remove("GIT_ALTERNATE_OBJECT_DIRECTORIES")
        .args(arguments);
    if let Some(objects) = objects {
        command.env("GIT_ALTERNATE_OBJECT_DIRECTORIES", objects);
    }
    let output = command.output()?;
    ensure!(
        output.status.success(),
        "template patch Git operation failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(output.stdout)
}

pub fn patch(root: &Path, rendered: &Path, engine: &Path, output: &Path) -> Result<()> {
    let root = root.canonicalize()?;
    let rendered = rendered.canonicalize()?;
    let engine = engine.canonicalize()?;
    let listing = Command::new(engine)
        .current_dir(&root)
        .args(["--template-root", ".template", "--dest"])
        .arg(&rendered)
        .args(["applied-files", "--null"])
        .output()?;
    ensure!(
        listing.status.success(),
        "rendered-file inventory could not be established"
    );
    patch_files(
        &root,
        &rendered,
        &crate::local::paths(&listing.stdout)?,
        output,
    )
}

pub fn patch_files(
    root: &Path,
    rendered: &Path,
    rendered_names: &[String],
    output: &Path,
) -> Result<()> {
    let root = root.canonicalize()?;
    let rendered = rendered.canonicalize()?;
    let revision = git(&root, None, &["rev-parse", "HEAD^{commit}"])?;
    let revision = std::str::from_utf8(&revision)?.trim();
    ensure!(
        crate::exact_hash(revision.as_bytes(), 40),
        "template base identity is invalid"
    );
    let objects = git(&root, None, &["rev-parse", "--git-path", "objects"])?;
    let objects = root
        .join(std::str::from_utf8(&objects)?.trim())
        .canonicalize()?;
    let existing: BTreeSet<_> = crate::local::paths(&git(
        &root,
        None,
        &["ls-tree", "-r", "--name-only", "-z", revision],
    )?)?
    .into_iter()
    .collect();
    for name in rendered_names {
        ensure!(!name.contains('\0'), "rendered path contains a null byte");
        crate::local::paths(name.as_bytes())?;
    }
    let rendered_set: BTreeSet<_> = rendered_names.iter().cloned().collect();
    ensure!(
        !rendered_set.is_empty() && rendered_set.len() == rendered_names.len(),
        "rendered-file coverage is empty or duplicated"
    );
    ensure!(
        !rendered_set.iter().any(|name| Path::new(name)
            .components()
            .any(|part| part.as_os_str() == ".git")),
        "rendered files must not change Git's private state"
    );
    let repository = tempfile::tempdir()?;
    let temporary = repository.path();
    git(
        temporary,
        Some(&objects),
        &["init", "--template=", "--quiet"],
    )?;
    git(
        temporary,
        Some(&objects),
        &["config", "core.autocrlf", "false"],
    )?;
    git(temporary, Some(&objects), &["read-tree", revision])?;
    let mut bytes = 0_u64;
    for name in existing.union(&rendered_set) {
        match change(existing.contains(name), rendered_set.contains(name)) {
            Change::Write => {
                let source = rendered.join(name);
                let mut component = rendered.clone();
                for part in Path::new(name).components() {
                    component.push(part);
                    ensure!(
                        !fs::symlink_metadata(&component)?.file_type().is_symlink(),
                        "rendered path contains a link: {name}"
                    );
                }
                ensure!(
                    source.canonicalize()?.starts_with(&rendered),
                    "rendered file escapes its root: {name}"
                );
                let metadata = fs::symlink_metadata(&source)?;
                ensure!(metadata.is_file(), "rendered file must be regular: {name}");
                bytes = bytes
                    .checked_add(metadata.len())
                    .context("rendered size overflow")?;
                ensure!(
                    bytes <= 64 * 1024 * 1024,
                    "rendered patch exceeds its 64 MiB budget"
                );
                let target = temporary.join(name);
                fs::create_dir_all(target.parent().context("rendered path has no directory")?)?;
                fs::copy(source, target)?;
            }
            Change::Delete | Change::Absent => {}
        }
    }
    git(
        temporary,
        Some(&objects),
        &["add", "--all", "--force", "--", "."],
    )?;
    let actual: BTreeSet<_> =
        crate::local::paths(&git(temporary, Some(&objects), &["ls-files", "-z"])?)?
            .into_iter()
            .collect();
    ensure!(
        actual == rendered_set,
        "initialization patch contains residue or misses rendered files"
    );
    for name in &rendered_set {
        ensure!(
            git(temporary, Some(&objects), &["show", &format!(":{name}")])?
                == fs::read(rendered.join(name))?,
            "rendered content changed while staging: {name}"
        );
    }
    let patch = git(
        temporary,
        Some(&objects),
        &[
            "diff",
            "--cached",
            "--binary",
            "--full-index",
            revision,
            "--",
        ],
    )?;
    ensure!(
        !patch.is_empty(),
        "initialization patch contains no changes"
    );
    use std::io::Write;
    let mut file =
        tempfile::NamedTempFile::new_in(output.parent().context("patch output has no parent")?)?;
    file.write_all(&patch)?;
    file.as_file().sync_all()?;
    file.persist_noclobber(output)
        .context("initialization patch could not be published atomically")?;
    repository.close()?;
    println!("Preserved the initialization patch, including template deletions");
    Ok(())
}

#[cfg(kani)]
mod proofs {
    use super::{Change, change};

    #[kani::proof]
    fn initialization_keeps_exactly_rendered_files() {
        let existing: bool = kani::any();
        let rendered: bool = kani::any();
        let action = change(existing, rendered);
        assert_eq!(action == Change::Write, rendered);
        assert_eq!(action == Change::Delete, existing && !rendered);
        assert_eq!(action == Change::Absent, !existing && !rendered);
        kani::cover!(action == Change::Write);
        kani::cover!(action == Change::Delete);
    }
}
