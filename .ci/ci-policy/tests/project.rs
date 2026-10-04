use std::{fs, process::Command};

use serde_json::{Value, json};

mod common;
use common::Repository;

fn fixture() -> (Repository, tempfile::TempDir) {
    let repository = Repository::new();
    repository.write("src/lib.rs", b"pub fn value() -> u8 { 1 }\n");
    repository.write("mise.toml", b"[tools]\nrust = \"1.99.0\"\n");
    repository.write(
        "Justfile",
        b"check suite=\"local\":\n    ci-policy project-check --suite {{quote(suite)}}\n",
    );
    repository.write(
        ".github/workflows/ci.yml",
        format!("name: CI\non: [pull_request]\njobs:\n  quality:\n    runs-on: {}-latest\n    steps:\n      - run: mise x -- just check ci.yml::quality\n", if cfg!(target_os = "linux") { "ubuntu" } else { std::env::consts::OS }).as_bytes(),
    );
    repository.write(
        ".ci/verification.json",
        &serde_json::to_vec(&contract()).expect("contract JSON"),
    );
    repository.commit(&[]);
    (repository, tempfile::tempdir().expect("private receipts"))
}

fn contract() -> Value {
    json!({
        "version": 1,
        "checks": [{
            "id": "types",
            "phase": "commit",
            "platforms": [std::env::consts::OS],
            "command": ["rustc", "--version"],
            "tools": ["rust@1.99.0"],
            "inputs": ["src/"],
            "environment": [],
            "cache": "content",
            "timeout_seconds": 20
        }],
        "ci": [{
            "workflow": "ci.yml",
            "job": "quality",
            "checks": ["types"]
        }]
    })
}

fn invoke(
    repository: &Repository,
    cache: &tempfile::TempDir,
    extra: &[&str],
) -> std::process::Output {
    invocation(repository, cache, extra)
        .output()
        .expect("real project gate")
}

fn invocation(repository: &Repository, cache: &tempfile::TempDir, extra: &[&str]) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_ci-policy"));
    command
        .args(["project-check", "--root"])
        .arg(repository.path())
        .arg("--cache-directory")
        .arg(cache.path())
        .args(extra);
    for key in [
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_INDEX_FILE",
        "GIT_COMMON_DIR",
        "GIT_OBJECT_DIRECTORY",
        "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    ] {
        command.env_remove(key);
    }
    command
}

fn accepted(output: &std::process::Output) {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn unchanged_checked_inputs_reuse_success_but_changed_source_runs_again() {
    let (repository, cache) = fixture();
    let first = invoke(&repository, &cache, &[]);
    accepted(&first);
    assert!(String::from_utf8_lossy(&first.stdout).contains("\"outcome\":\"ran\""));
    let same = invoke(&repository, &cache, &[]);
    accepted(&same);
    assert!(String::from_utf8_lossy(&same.stdout).contains("\"outcome\":\"reused\""));
    fs::write(repository.path().join("README.md"), "Usage.\n").expect("unrelated prose");
    let prose = invoke(&repository, &cache, &[]);
    accepted(&prose);
    assert!(String::from_utf8_lossy(&prose.stdout).contains("\"outcome\":\"reused\""));
    fs::write(
        repository.path().join("src/lib.rs"),
        "pub fn value() -> u8 { 2 }\n",
    )
    .expect("new source");
    let changed = invoke(&repository, &cache, &[]);
    accepted(&changed);
    assert!(String::from_utf8_lossy(&changed.stdout).contains("\"outcome\":\"ran\""));
}

#[test]
fn fixed_runner_settings_do_not_invalidate_effective_environment() {
    let (repository, cache) = fixture();
    let first = invocation(&repository, &cache, &[])
        .env("MISE_AUTO_INSTALL", "true")
        .output()
        .unwrap();
    accepted(&first);
    let second = invocation(&repository, &cache, &[])
        .env("MISE_AUTO_INSTALL", "false")
        .output()
        .unwrap();
    accepted(&second);
    assert!(String::from_utf8_lossy(&second.stdout).contains("\"outcome\":\"reused\""));
}

#[test]
fn large_working_trees_do_not_expand_the_git_command_line() {
    let (repository, cache) = fixture();
    let directory = repository.path().join("src/many");
    fs::create_dir_all(&directory).unwrap();
    for index in 0..4096 {
        fs::write(
            directory.join(format!("{index:04}-{}.rs", "x".repeat(80))),
            "pub struct Input;\n",
        )
        .unwrap();
    }
    accepted(&invoke(&repository, &cache, &[]));
}

#[cfg(unix)]
#[test]
fn working_paths_keep_newlines_and_leading_options_literal() {
    let (repository, cache) = fixture();
    let path = repository.path().join("-literal\npath.rs");
    fs::write(&path, "pub struct Input;\n").unwrap();
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    let mut value = contract();
    value["checks"][0]["command"] = json!(["git", "ls-tree", "-r", "HEAD"]);
    value["checks"][0]["inputs"] = json!(["."]);
    repository.write(
        ".ci/verification.json",
        &serde_json::to_vec(&value).unwrap(),
    );
    let output = invoke(&repository, &cache, &[]);
    accepted(&output);
    let listing = String::from_utf8(output.stdout).unwrap();
    assert!(listing.contains("100755 blob "));
    assert!(listing.contains("\"-literal\\npath.rs\""));
}

#[test]
fn a_failed_check_never_creates_success_evidence() {
    let (repository, cache) = fixture();
    let mut value = contract();
    value["checks"][0]["command"] = json!(["rustc", "--definitely-not-a-rustc-option"]);
    fs::write(
        repository.path().join(".ci/verification.json"),
        serde_json::to_vec(&value).unwrap(),
    )
    .unwrap();
    assert!(!invoke(&repository, &cache, &[]).status.success());
    assert_eq!(fs::read_dir(cache.path()).unwrap().count(), 0);
}

#[test]
fn removing_the_ci_invocation_or_project_contract_rejects_the_gate() {
    let (repository, cache) = fixture();
    fs::write(repository.path().join(".github/workflows/ci.yml"), "on: [pull_request]\njobs:\n  quality:\n    runs-on: macos-latest\n    steps:\n      - run: echo done\n").unwrap();
    assert!(!invoke(&repository, &cache, &[]).status.success());
    fs::remove_file(repository.path().join(".ci/verification.json")).unwrap();
    assert!(!invoke(&repository, &cache, &[]).status.success());
}

#[test]
fn a_non_regular_owned_input_cannot_produce_a_success_receipt() {
    let (repository, cache) = fixture();
    let object = repository.blob(b"../external-source");
    repository.entry("120000", &object, "src/external.rs");
    assert!(!invoke(&repository, &cache, &["--index"]).status.success());
    assert_eq!(fs::read_dir(cache.path()).unwrap().count(), 0);
}

#[test]
fn a_new_ci_job_cannot_escape_the_declared_local_contract() {
    let (repository, cache) = fixture();
    let path = repository.path().join(".github/workflows/ci.yml");
    let mut source = fs::read_to_string(&path).unwrap();
    source.push_str("  additional-check:\n    runs-on: ubuntu-latest\n    steps:\n      - run: rustc --version\n");
    fs::write(path, source).unwrap();
    assert!(!invoke(&repository, &cache, &[]).status.success());
    assert_eq!(fs::read_dir(cache.path()).unwrap().count(), 0);
}

#[test]
fn staged_commands_observe_the_staged_tree_through_head() {
    let (repository, cache) = fixture();
    let head = repository.git(&["rev-parse", "HEAD"]);
    let mut value = contract();
    value["checks"][0]["command"] = json!(["git", "cat-file", "-e", "HEAD:src/new.rs"]);
    repository.write("src/new.rs", b"pub struct Staged;\n");
    repository.write(
        ".ci/verification.json",
        &serde_json::to_vec(&value).unwrap(),
    );
    let index = repository.git(&["write-tree"]);
    accepted(&invoke(&repository, &cache, &["--index"]));
    assert_eq!(repository.git(&["rev-parse", "HEAD"]), head);
    assert_eq!(repository.git(&["write-tree"]), index);
}

#[test]
fn automatic_phase_budget_bounds_the_complete_plan() {
    let (repository, cache) = fixture();
    let mut value = contract();
    let mut additional = value["checks"][0].clone();
    additional["id"] = json!("second");
    value["checks"].as_array_mut().unwrap().push(additional);
    value["ci"][0]["checks"] = json!(["types", "second"]);
    repository.write(
        ".ci/verification.json",
        &serde_json::to_vec(&value).unwrap(),
    );
    assert!(
        !invoke(&repository, &cache, &["--phase", "commit"])
            .status
            .success()
    );
    assert_eq!(fs::read_dir(cache.path()).unwrap().count(), 0);
}

#[test]
fn ci_cannot_replace_the_local_entrypoint_with_a_successful_noop() {
    let (repository, cache) = fixture();
    repository.write("Justfile", b"check suite=\"local\":\n    echo done\n");
    assert!(!invoke(&repository, &cache, &[]).status.success());
    assert_eq!(fs::read_dir(cache.path()).unwrap().count(), 0);
}

#[test]
fn ci_cannot_replace_the_interpreter_with_a_successful_noop() {
    let (repository, cache) = fixture();
    repository.write("Justfile", b"set shell := [\"echo\"]\ncheck suite=\"local\":\n    ci-policy project-check --suite {{quote(suite)}}\n");
    assert!(!invoke(&repository, &cache, &[]).status.success());
    assert_eq!(fs::read_dir(cache.path()).unwrap().count(), 0);
}

#[test]
fn a_newer_worktree_cannot_hide_the_pushed_revisions_failure() {
    let (repository, cache) = fixture();
    let mut value = contract();
    value["checks"][0]["command"] = json!(["git", "cat-file", "-e", "HEAD:src/new.rs"]);
    repository.write(
        ".ci/verification.json",
        &serde_json::to_vec(&value).unwrap(),
    );
    let rejected = repository.commit(&[]);
    repository.write("src/new.rs", b"pub struct Newer;\n");
    let head = repository.commit(&[&rejected]);
    accepted(&invoke(&repository, &cache, &[]));
    assert!(
        !invoke(&repository, &cache, &["--revision", &rejected])
            .status
            .success()
    );
    assert_eq!(
        repository.git(&["rev-parse", "HEAD"]),
        format!("{head}\n").as_bytes()
    );
}

fn descendant_fixture() -> (Repository, tempfile::TempDir, std::path::PathBuf) {
    let (repository, cache) = fixture();
    for (name, bytes) in [
        (
            "xtask/Cargo.toml",
            include_bytes!("fixtures/xtask/Cargo.toml").as_slice(),
        ),
        (
            "xtask/Cargo.lock",
            include_bytes!("fixtures/xtask/Cargo.lock").as_slice(),
        ),
        (
            "xtask/src/main.rs",
            include_bytes!("fixtures/xtask/src/main.rs").as_slice(),
        ),
    ] {
        repository.write(name, bytes);
    }
    let target = cache.path().join("build");
    let build = Command::new("mise")
        .current_dir(repository.path())
        .env("MISE_AUTO_INSTALL", "false")
        .env("MISE_TRUSTED_CONFIG_PATHS", repository.path())
        .env("CARGO_TARGET_DIR", &target)
        .args([
            "x",
            "rust@1.99.0",
            "--",
            "cargo",
            "build",
            "--locked",
            "--jobs",
            "2",
            "--manifest-path",
            "xtask/Cargo.toml",
        ])
        .output()
        .unwrap();
    accepted(&build);
    let mut value = contract();
    value["checks"][0]["command"] = json!([
        "cargo",
        "run",
        "--locked",
        "--manifest-path",
        "xtask/Cargo.toml",
        "--",
        "check"
    ]);
    value["checks"][0]["inputs"] = json!(["."]);
    value["checks"][0]["environment"] = json!(["CI_POLICY_TEST_MARKER", "CI_POLICY_TEST_EXIT"]);
    value["checks"][0]["timeout_seconds"] = json!(5);
    repository.write(
        ".ci/verification.json",
        &serde_json::to_vec(&value).unwrap(),
    );
    (repository, cache, target)
}

#[test]
fn timeout_terminates_descendants_and_cannot_publish_success() {
    let (repository, cache, target) = descendant_fixture();
    let marker = cache.path().join("started");
    let output = invocation(&repository, &cache, &[])
        .env("CI_POLICY_TEST_MARKER", &marker)
        .env("CARGO_TARGET_DIR", &target)
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("exceeded its declared budget"),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(marker.exists(), "the real descendant must have started");
    std::thread::sleep(std::time::Duration::from_secs(9));
    assert!(!marker.with_extension("late").exists());
    assert!(!fs::read_dir(cache.path()).unwrap().any(|entry| {
        entry
            .unwrap()
            .path()
            .extension()
            .is_some_and(|extension| extension == "json")
    }));
}

fn completed_descendants(status: &str, success: bool) {
    let (repository, cache, target) = descendant_fixture();
    let marker = cache.path().join("started");
    let output = invocation(&repository, &cache, &[])
        .env("CI_POLICY_TEST_MARKER", &marker)
        .env("CI_POLICY_TEST_EXIT", status)
        .env("CARGO_TARGET_DIR", &target)
        .output()
        .unwrap();
    assert_eq!(
        output.status.success(),
        success,
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(marker.exists(), "the real descendant must have started");
    std::thread::sleep(std::time::Duration::from_secs(9));
    assert!(
        !marker.with_extension("late").exists(),
        "a completed command left a live descendant"
    );
    assert_eq!(
        fs::read_dir(cache.path()).unwrap().any(|entry| {
            entry
                .unwrap()
                .path()
                .extension()
                .is_some_and(|extension| extension == "json")
        }),
        success
    );
}

#[test]
fn successful_checks_terminate_remaining_descendants_before_recording_success() {
    completed_descendants("0", true);
}

#[test]
fn failed_checks_terminate_remaining_descendants_without_recording_success() {
    completed_descendants("1", false);
}

#[test]
fn mutable_environment_requires_fresh_evidence() {
    let (repository, cache) = fixture();
    let mut value = contract();
    value["checks"][0]["cache"] = json!("always");
    repository.write(
        ".ci/verification.json",
        &serde_json::to_vec(&value).unwrap(),
    );
    accepted(&invoke(&repository, &cache, &[]));
    let again = invoke(&repository, &cache, &[]);
    accepted(&again);
    assert!(String::from_utf8_lossy(&again.stdout).contains("\"outcome\":\"ran\""));
}

#[test]
fn ci_cannot_suppress_a_failure_through_an_expression() {
    let (repository, cache) = fixture();
    let path = repository.path().join(".github/workflows/ci.yml");
    let source = fs::read_to_string(&path).unwrap();
    fs::write(
        path,
        source.replace(
            "    steps:",
            "    continue-on-error: ${{ true }}\n    steps:",
        ),
    )
    .unwrap();
    assert!(!invoke(&repository, &cache, &[]).status.success());
    assert_eq!(fs::read_dir(cache.path()).unwrap().count(), 0);
}
