use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{REVISION, exact_hash, local::PushUpdate, same_revision};

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Installation {
    revision: String,
    tools: String,
    hooks: PathBuf,
    digests: BTreeMap<String, String>,
}

pub fn home() -> Result<PathBuf> {
    let value = if cfg!(windows) {
        std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME"))
    } else {
        std::env::var_os("HOME")
    };
    value
        .map(PathBuf::from)
        .context("native user directory is unavailable")
}

fn installed(program: &str) -> Result<PathBuf> {
    Ok(home()?.join(".local/bin").join(if cfg!(windows) {
        format!("{program}.exe")
    } else {
        program.to_owned()
    }))
}

fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn git(root: &Path, args: &[&str]) -> Result<Vec<u8>> {
    let output = Command::new("git").current_dir(root).args(args).output()?;
    ensure!(
        output.status.success(),
        "required Git operation failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(output.stdout)
}

fn hook_directory(root: &Path) -> Result<PathBuf> {
    let bytes = git(root, &["config", "--path", "--get", "core.hooksPath"])?;
    let value = std::str::from_utf8(&bytes)?.trim();
    let path = PathBuf::from(value);
    ensure!(
        path.is_absolute(),
        "the effective global hook directory must be absolute"
    );
    let expected = home()?.join(if cfg!(windows) {
        ".config/git/template/hooks"
    } else {
        ".config/git/hooks"
    });
    ensure!(
        path.canonicalize()? == expected.canonicalize()?,
        "repository overrides the required native Git hooks"
    );
    Ok(expected.canonicalize()?)
}

fn hook_digests(directory: &Path) -> Result<BTreeMap<String, String>> {
    ["pre-commit", "pre-push"]
        .into_iter()
        .map(|name| {
            let path = directory.join(name);
            ensure!(
                fs::symlink_metadata(&path)?.file_type().is_file(),
                "required Git hook must be a regular file"
            );
            Ok((name.to_owned(), digest(&fs::read(path)?)))
        })
        .collect()
}

fn receipt_path() -> Result<PathBuf> {
    Ok(home()?.join(".config/ci-policy/installation.json"))
}

pub fn require_revision(expected: &str) -> Result<()> {
    ensure!(
        same_revision(expected.as_bytes(), REVISION.as_bytes()),
        "installed policy differs from the reviewed revision; apply this machine's native dotfiles profile"
    );
    Ok(())
}

pub fn activate(root: &Path, expected: &str) -> Result<()> {
    require_revision(expected)?;
    let hooks = hook_directory(root)?;
    for name in ["pre-commit", "pre-push"] {
        let source = fs::read_to_string(hooks.join(name))?;
        ensure!(
            source.contains("ci-policy") && source.contains(expected),
            "required Git hook is not connected to the reviewed policy"
        );
    }
    crate::tools::doctor()?;
    let receipt = Installation {
        revision: REVISION.to_owned(),
        tools: digest(include_bytes!("../tools.json")),
        digests: hook_digests(&hooks)?,
        hooks,
    };
    let target = receipt_path()?;
    let parent = target
        .parent()
        .context("installation receipt has no directory")?;
    fs::create_dir_all(parent)?;
    let file = tempfile::NamedTempFile::new_in(parent)?;
    serde_json::to_writer_pretty(file.as_file(), &receipt)?;
    file.as_file().sync_all()?;
    file.persist(&target)
        .context("cannot publish the verified installation receipt")?;
    println!("Activated the reviewed policy and both native Git hooks");
    Ok(())
}

pub fn doctor(root: &Path, expected: &str) -> Result<()> {
    require_revision(expected)?;
    let receipt: Installation = serde_json::from_value(crate::json::parse(
        &fs::read(receipt_path()?).context("required policy installation receipt is missing")?,
    )?)?;
    let hooks = hook_directory(root)?;
    ensure!(
        receipt.revision == REVISION
            && receipt.tools == digest(include_bytes!("../tools.json"))
            && receipt.hooks == hooks
            && receipt.digests == hook_digests(&hooks)?,
        "native policy installation drifted; reapply and verify the native dotfiles profile"
    );
    Ok(())
}

fn repository_hook(
    root: &Path,
    hook: &str,
    arguments: &[&str],
    input: Option<&[u8]>,
) -> Result<()> {
    let configured = [
        "lefthook.yml",
        "lefthook.yaml",
        "lefthook.toml",
        "lefthook.json",
        ".lefthook.yml",
        ".lefthook.yaml",
        ".lefthook.toml",
        ".lefthook.json",
    ]
    .into_iter()
    .any(|name| root.join(name).is_file());
    if !configured {
        return Ok(());
    }
    let output = Command::new("mise")
        .current_dir(root)
        .args(["which", "lefthook"])
        .output()?;
    ensure!(
        output.status.success(),
        "configured repository requires its pinned Lefthook executable"
    );
    let path = PathBuf::from(std::str::from_utf8(&output.stdout)?.trim());
    ensure!(
        path.is_absolute() && path.is_file(),
        "repository Lefthook executable is unavailable"
    );
    let output = Command::new(&path).current_dir(root).arg("dump").output()?;
    ensure!(
        output.status.success(),
        "repository hook configuration could not be loaded"
    );
    let configuration =
        yaml_rust2::YamlLoader::load_from_str(std::str::from_utf8(&output.stdout)?)?;
    ensure!(
        configuration.len() == 1 && configuration[0].as_hash().is_some(),
        "repository hook configuration is invalid"
    );
    if configuration[0][hook].is_badvalue() {
        return Ok(());
    }
    crate::local::execute(
        Command::new(path)
            .current_dir(root)
            .args(["run", "--no-auto-install", hook])
            .args(arguments),
        input,
    )
}

fn repository(root: &Path) -> Result<PathBuf> {
    let bytes = git(root, &["rev-parse", "--show-toplevel"])?;
    Ok(PathBuf::from(std::str::from_utf8(&bytes)?.trim()).canonicalize()?)
}

pub fn commit(root: &Path, expected: &str, guard: bool) -> Result<()> {
    let root = repository(root)?;
    doctor(&root, expected)?;
    if guard {
        crate::local::execute(
            Command::new(installed("dotguard")?)
                .current_dir(&root)
                .arg("pre-commit"),
            None,
        )?;
    }
    crate::local::execute(
        Command::new(installed("ocomment")?)
            .current_dir(&root)
            .args(["fix", "--tidy", "--staged", "--jobs", "2"]),
        None,
    )
    .context("global source prose gate failed")?;
    repository_hook(&root, "pre-commit", &[], None)?;
    crate::local::verify_index(&root)
}

pub fn check_push_hold() -> Result<()> {
    match fs::symlink_metadata(home()?.join(".config/git/push-paused")) {
        Ok(_) => anyhow::bail!(
            "push paused on this machine; inspect the CI budget and resume explicitly"
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).context("machine push hold could not be inspected"),
    }
}

pub fn push(root: &Path, expected: &str, remote: &str, url: &str, input: &str) -> Result<()> {
    let root = repository(root)?;
    doctor(&root, expected)?;
    check_push_hold()?;
    ensure!(
        input.len() <= 64 * 1024,
        "push input exceeds the verification budget"
    );
    ensure!(
        !remote.is_empty()
            && !remote.starts_with('-')
            && remote
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"._/-".contains(&byte)),
        "push remote must be literal"
    );
    let updates: Vec<_> = input
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(PushUpdate::parse)
        .collect::<Result<_>>()?;
    for update in updates {
        let revision = update.local_revision();
        let revisions = if let Some(previous) = update.remote_revision() {
            git(&root, &["merge-base", "--is-ancestor", previous, revision])
                .context("a non-fast-forward push is prohibited")?;
            git(
                &root,
                &[
                    "rev-list",
                    "--max-count=1001",
                    &format!("{previous}..{revision}"),
                ],
            )?
        } else {
            git(
                &root,
                &[
                    "rev-list",
                    "--max-count=1001",
                    revision,
                    "--not",
                    &format!("--remotes={remote}"),
                ],
            )?
        };
        let revisions = std::str::from_utf8(&revisions)?.lines().collect::<Vec<_>>();
        ensure!(
            revisions.len() <= 1000,
            "push exceeds the 1000-commit verification budget"
        );
        for revision in revisions {
            ensure!(
                exact_hash(revision.as_bytes(), 40),
                "Git returned an invalid commit identity"
            );
            git(&root, &["verify-commit", revision])
                .context("unsigned or unverifiable commits cannot be pushed")?;
        }
    }
    crate::local::verify_push(&root, remote, input)?;
    repository_hook(&root, "pre-push", &[remote, url], Some(input.as_bytes()))
}
