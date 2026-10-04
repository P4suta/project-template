mod common;

use common::Repository;
use std::fs;

fn repository() -> Repository {
    let repository = Repository::new();
    repository.write(
        "Cargo.toml",
        b"[package]\nname = \"global-gate-fixture\"\nversion = \"0.1.0\"\nedition = \"2024\"\npublish = false\n",
    );
    repository.write(
        "Cargo.lock",
        b"version = 4\n\n[[package]]\nname = \"global-gate-fixture\"\nversion = \"0.1.0\"\n",
    );
    let source = b"pub const fn value() -> u8 {\n    1\n}\n\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn preserves_value() {\n        assert_eq!(super::value(), 1);\n    }\n}\n";
    repository.write("src/lib.rs", source);
    repository.entry("100644", &repository.blob(source), "src/lib.rs");
    repository.commit(&[]);
    repository
}

fn invoke(repository: &Repository, cache: &std::path::Path) -> std::process::Output {
    let configuration = tempfile::tempdir().unwrap();
    let global = configuration.path().join("config.toml");
    fs::write(&global, "").unwrap();
    repository
        .command(env!("CARGO_BIN_EXE_ci-policy"))
        .env("MISE_GLOBAL_CONFIG_FILE", &global)
        .env("MISE_CONFIG_DIR", configuration.path())
        .args(["project-check", "--root"])
        .arg(repository.path())
        .arg("--cache-directory")
        .arg(cache)
        .output()
        .expect("global project gate")
}

#[test]
fn workflow_checks_activate_transitive_tools_without_global_defaults() {
    let repository = repository();
    let workflow = "name: CI\non: [pull_request]\npermissions:\n  contents: read\nconcurrency:\n  group: 'ci-${{ github.ref }}'\n  cancel-in-progress: true\njobs:\n  quality:\n    runs-on: ubuntu-latest\n    timeout-minutes: 10\n    steps:\n      - run: cargo test --locked\n";
    repository.write(".github/workflows/ci.yml", workflow.as_bytes());
    let configuration = tempfile::tempdir().unwrap();
    let global = configuration.path().join("config.toml");
    fs::write(&global, "").unwrap();
    let mut command = repository.command(env!("CARGO_BIN_EXE_ci-policy"));
    command
        .env("MISE_GLOBAL_CONFIG_FILE", &global)
        .env("MISE_CONFIG_DIR", configuration.path())
        .args(["source-check", "--root"])
        .arg(repository.path());
    let output = command.output().unwrap();
    let diagnostic = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.status.success(), "{diagnostic}");
    repository.write(
        ".github/workflows/ci.yml",
        workflow
            .replace("cargo test --locked", "echo $UNQUOTED")
            .as_bytes(),
    );
    let output = command.output().unwrap();
    let diagnostic = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!output.status.success());
    assert!(diagnostic.contains("SC2086"), "{diagnostic}");
}

#[test]
fn a_new_rust_project_needs_no_hook_task_or_policy_configuration() {
    let repository = repository();
    let cache = tempfile::tempdir().unwrap();
    let output = invoke(&repository, cache.path());
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    for name in [".ci", "Justfile", "mise.toml", "lefthook.yml"] {
        assert!(!repository.path().join(name).exists());
    }
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("rust-clippy"));
    assert!(stdout.contains("rust-tests"));
    fs::write(
        repository.path().join("src/lib.rs"),
        "#[test]\nfn rejects_broken_behavior() {\n    assert_eq!(1, 2);\n}\n",
    )
    .unwrap();
    let output = invoke(&repository, cache.path());
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("rejects_broken_behavior"));
}

#[test]
fn a_new_ci_check_is_discovered_without_a_repository_contract() {
    let repository = repository();
    repository.write(
        ".github/workflows/ci.yml",
        format!(
            "name: CI\non: [pull_request]\npermissions:\n  contents: read\nconcurrency:\n  group: 'ci-${{{{ github.ref }}}}'\n  cancel-in-progress: true\njobs:\n  quality:\n    runs-on: {}-latest\n    timeout-minutes: 10\n    steps:\n      - run: cargo test --locked --definitely-invalid-ci-option\n",
            if cfg!(target_os = "linux") {
                "ubuntu"
            } else {
                std::env::consts::OS
            }
        )
        .as_bytes(),
    );
    let cache = tempfile::tempdir().unwrap();
    let output = invoke(&repository, cache.path());
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("definitely-invalid-ci-option"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn an_unknown_ci_operation_cannot_be_reported_as_verified() {
    let repository = repository();
    repository.write(
        ".github/workflows/ci.yml",
        b"on: [pull_request]\njobs:\n  custom:\n    runs-on: ubuntu-latest\n    steps:\n      - run: custom-verifier check\n",
    );
    let cache = tempfile::tempdir().unwrap();
    let output = invoke(&repository, cache.path());
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("uncovered CI operation"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(fs::read_dir(cache.path()).unwrap().count(), 0);
}

#[test]
fn ci_environment_and_directory_overrides_cannot_be_silently_ignored() {
    for (workflow, job, step) in [
        ("env:\n  REQUIRED_MODE: ci\n", "", ""),
        ("defaults:\n  run:\n    working-directory: nested\n", "", ""),
        ("", "    env:\n      REQUIRED_MODE: ci\n", ""),
        (
            "",
            "    defaults:\n      run:\n        working-directory: nested\n",
            "",
        ),
        ("", "", "        env:\n          REQUIRED_MODE: ci\n"),
        ("", "", "        working-directory: nested\n"),
    ] {
        let repository = repository();
        repository.write(
            ".github/workflows/ci.yml",
            format!("on: [pull_request]\n{workflow}jobs:\n  check:\n    runs-on: ubuntu-latest\n{job}    steps:\n      - run: cargo test --locked\n{step}").as_bytes(),
        );
        let cache = tempfile::tempdir().unwrap();
        let output = invoke(&repository, cache.path());
        assert!(!output.status.success());
        assert!(
            String::from_utf8_lossy(&output.stderr)
                .contains("environment requires a global adapter"),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(fs::read_dir(cache.path()).unwrap().count(), 0);
    }
}

#[test]
fn documentation_only_projects_receive_the_common_gate_without_setup() {
    let repository = Repository::new();
    repository.write("README.md", b"Usage.\n");
    repository.commit(&[]);
    let cache = tempfile::tempdir().unwrap();
    let output = invoke(&repository, cache.path());
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("common-source"));
}

#[test]
fn a_compiler_for_one_language_cannot_cover_another_languages_source() {
    let repository = repository();
    repository.write("other/input.py", b"def value():\n    return 1\n");
    let cache = tempfile::tempdir().unwrap();
    let output = invoke(&repository, cache.path());
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("uncovered language source"));
    assert_eq!(fs::read_dir(cache.path()).unwrap().count(), 0);
}
