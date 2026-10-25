mod common;

use common::Repository;
use std::{fs, path::Path};

fn copy(repository: &Repository, package: &Path, path: &Path, prefix: &Path) {
    let metadata = fs::symlink_metadata(path).unwrap();
    assert!(!metadata.is_symlink());
    if metadata.is_dir() {
        for entry in fs::read_dir(path).unwrap() {
            copy(repository, package, &entry.unwrap().path(), prefix);
        }
    } else {
        assert!(metadata.is_file());
        let relative = prefix.join(path.strip_prefix(package).unwrap());
        let relative = relative.to_str().unwrap().replace('\\', "/");
        let bytes = fs::read(path).unwrap();
        repository.write(&relative, &bytes);
        repository.entry("100644", &repository.blob(&bytes), &relative);
    }
}

#[test]
fn the_actual_policy_installs_from_git_and_checks_a_repository_without_setup() {
    let repository = Repository::new();
    let package = Path::new(env!("CARGO_MANIFEST_DIR"));
    let root = package.parent().unwrap().parent().unwrap();
    let prefix = package.strip_prefix(root).unwrap();
    for name in ["Cargo.toml", "Cargo.lock", "tools.json", "src"] {
        copy(&repository, package, &package.join(name), prefix);
    }
    let revision = repository.commit(&[]);
    let install = tempfile::tempdir().unwrap();
    let parent_binary = Path::new(env!("CARGO_BIN_EXE_ci-policy"));
    let original = fs::read(parent_binary).unwrap();
    let source = repository.path().to_str().unwrap().replace('\\', "/");
    let source = format!("file:///{}", source.trim_start_matches('/'));
    let output = repository
        .command("cargo")
        .env("CI_POLICY_REVISION", &revision)
        .args(["install", "--debug", "--locked", "--git", &source, "--rev"])
        .arg(&revision)
        .arg("--root")
        .arg(install.path())
        .arg("--target-dir")
        .arg(install.path().join("build"))
        .arg("ci-policy")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        fs::read(parent_binary).unwrap() == original,
        "installation replaced the original test-runner binary"
    );
    let candidate = Repository::new();
    candidate.write("README.md", b"Usage.\n");
    let cache = tempfile::tempdir().unwrap();
    let mut command = candidate.command(install.path().join(if cfg!(windows) {
        "bin/ci-policy.exe"
    } else {
        "bin/ci-policy"
    }));
    command
        .args(["project-check", "--root"])
        .arg(candidate.path())
        .arg("--cache-directory")
        .arg(cache.path());
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    candidate.write("data.json", br#"{"name":1,"name":2}"#);
    let output = command.output().unwrap();
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("duplicate JSON key"),
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
