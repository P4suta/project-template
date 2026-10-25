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
    let mut command = repository.command(env!("CARGO_BIN_EXE_ci-policy"));
    command
        .args(["project-check", "--root"])
        .arg(repository.path())
        .arg("--cache-directory")
        .arg(cache.path())
        .args(extra);
    command
}

fn accepted(output: &std::process::Output) {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn candidate_source(
    repository: &Repository,
    revision: &str,
    controller: &str,
) -> std::process::Output {
    repository
        .command(env!("CARGO_BIN_EXE_ci-policy"))
        .args(["candidate-source", "--root"])
        .arg(repository.path())
        .args([
            "--revision",
            revision,
            "--controller-ref",
            controller,
            "--default-branch",
            "main",
        ])
        .output()
        .expect("candidate source validation")
}

#[test]
fn candidate_source_accepts_exact_commits_from_the_default_branch_controller() {
    let (repository, _) = fixture();
    let revision = String::from_utf8(repository.git(&["rev-parse", "HEAD"]))
        .unwrap()
        .trim()
        .to_owned();
    repository.git(&["update-ref", "refs/remotes/origin/main", &revision]);
    let output = candidate_source(&repository, &revision, "refs/heads/main");
    accepted(&output);
    assert_eq!(String::from_utf8(output.stdout).unwrap().trim(), revision);
    for invalid in [
        "main",
        "1234567",
        "zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz",
    ] {
        assert!(
            !candidate_source(&repository, invalid, "refs/heads/main")
                .status
                .success()
        );
    }
    assert!(
        !candidate_source(&repository, &revision, "refs/heads/topic")
            .status
            .success()
    );
    repository.write("src/lib.rs", b"pub fn value() -> u8 { 2 }\n");
    let unrelated = repository.commit(&[]);
    assert!(
        !candidate_source(&repository, &unrelated, "refs/heads/main")
            .status
            .success()
    );
}

#[test]
fn candidate_phase_refuses_working_tree_and_index_scopes() {
    let (repository, cache) = fixture();
    for arguments in [
        vec!["--phase", "candidate"],
        vec!["--phase", "candidate", "--index"],
    ] {
        let output = invoke(&repository, &cache, &arguments);
        assert!(!output.status.success());
        assert!(
            String::from_utf8_lossy(&output.stderr)
                .contains("candidate checks require revision scope")
        );
    }
}

fn qualification(value: &Value, revision: &str) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_ci-policy"))
        .args([
            "candidate-gate",
            "--revision",
            revision,
            "--needs",
            &value.to_string(),
        ])
        .output()
        .expect("candidate qualification")
}

#[test]
fn candidate_qualification_requires_every_role_to_succeed_for_one_exact_source() {
    let revision = "1234567890abcdef1234567890abcdef12345678";
    let mut value = json!({});
    for role in ["source", "routine", "proofs"] {
        value[role] = json!({"result": "success", "outputs": {"source_sha": revision}});
    }
    accepted(&qualification(&value, revision));
    for role in ["source", "routine", "proofs"] {
        for result in ["failure", "skipped", "cancelled", "unknown"] {
            let mut invalid = value.clone();
            invalid[role]["result"] = json!(result);
            assert!(!qualification(&invalid, revision).status.success());
        }
        let mut invalid = value.clone();
        invalid.as_object_mut().unwrap().remove(role);
        assert!(!qualification(&invalid, revision).status.success());
        invalid = value.clone();
        invalid[role]["outputs"]["source_sha"] = json!("abcdef1234567890abcdef1234567890abcdef12");
        assert!(!qualification(&invalid, revision).status.success());
    }
    assert!(!qualification(&value, "main").status.success());
    let mut extra = value.clone();
    extra["unexpected"] = value["source"].clone();
    assert!(!qualification(&extra, revision).status.success());
}

fn candidate_fixture() -> (Repository, tempfile::TempDir) {
    let (repository, cache) = fixture();
    repository.git(&[
        "remote",
        "add",
        "origin",
        "https://github.com/P4suta/project-template.git",
    ]);
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    for workflow in [
        "tmpl-verify.yml",
        "ci-policy-checks.yml",
        "common-policy.yml",
        "candidate-verification.yml",
    ] {
        let path = format!(".github/workflows/{workflow}");
        let mut source = fs::read_to_string(root.join(&path)).unwrap();
        if workflow == "ci-policy-checks.yml" {
            source = source.replace(
                "os: [ubuntu-latest, macos-latest]",
                &format!(
                    "os: [{}-latest]",
                    if cfg!(target_os = "linux") {
                        "ubuntu"
                    } else {
                        std::env::consts::OS
                    }
                ),
            );
        }
        repository.write(&path, source.as_bytes());
    }
    repository.git(&["rm", ".github/workflows/ci.yml"]);
    repository.write("Justfile", &fs::read(root.join("Justfile")).unwrap());
    let mut value: Value =
        serde_json::from_slice(&fs::read(root.join(".ci/verification.json")).unwrap()).unwrap();
    let types = contract()["checks"][0].clone();
    let mut coverage = types.clone();
    coverage["id"] = json!("coverage");
    coverage["platforms"] = json!(["linux"]);
    let mut proof = types.clone();
    proof["id"] = json!("proof");
    proof["phase"] = json!("candidate");
    value["checks"] = json!([types, coverage, proof]);
    value["ci"][0]["checks"] = json!(["proof"]);
    value["ci"][1]["checks"] = json!(["types"]);
    value["ci"][2]["checks"] = json!(["coverage"]);
    value["hosted"]
        .as_array_mut()
        .unwrap()
        .retain(|hosted| !matches!(hosted["workflow"].as_str(), Some("init.yml" | "codeql.yml")));
    repository.write(
        ".ci/verification.json",
        &serde_json::to_vec(&value).unwrap(),
    );
    (repository, cache)
}

#[test]
fn candidate_workflow_rejects_broken_source_paths_and_incomplete_summaries() {
    let (repository, cache) = candidate_fixture();
    accepted(&invoke(&repository, &cache, &[]));
    for (path, original, changed) in [
        (
            "candidate-verification.yml",
            "source_sha: ${{ needs.source.outputs.source_sha }}",
            "source_sha: ${{ inputs.revision }}",
        ),
        (
            "candidate-verification.yml",
            "needs: [source, routine, proofs]",
            "needs: [source, routine]",
        ),
        (
            "candidate-verification.yml",
            "--controller-ref \"$CONTROLLER_REF\"",
            "--controller-ref refs/heads/main",
        ),
        (
            "ci-policy-checks.yml",
            "ref: ${{ inputs.source_sha }}",
            "ref: ${{ github.sha }}",
        ),
        (
            "ci-policy-checks.yml",
            "mise x -- just check-candidate \"$CANDIDATE_SHA\"",
            "mise x -- just check",
        ),
        (
            "ci-policy-checks.yml",
            "- run: mise x -- just check-candidate",
            "- env: { CANDIDATE_SHA: 1234567890abcdef1234567890abcdef12345678 }\n        run: mise x -- just check-candidate",
        ),
        (
            "ci-policy-checks.yml",
            "value: ${{ jobs.verified.outputs.source_sha }}",
            "value: ${{ inputs.source_sha }}",
        ),
        (
            "tmpl-verify.yml",
            "needs: [engine, coverage, policy]",
            "needs: [engine, coverage]",
        ),
        (
            "tmpl-verify.yml",
            "uses: $/.github/workflows/common-policy.yml",
            "uses: $/.github/workflows/ci-policy-checks.yml",
        ),
        (
            "common-policy.yml",
            "ref: ${{ inputs.source_sha || github.sha }}",
            "ref: ${{ github.sha }}",
        ),
    ] {
        let path = repository.path().join(format!(".github/workflows/{path}"));
        let source = fs::read_to_string(&path).unwrap();
        assert!(source.contains(original));
        fs::write(&path, source.replace(original, changed)).unwrap();
        assert!(
            !invoke(&repository, &cache, &[]).status.success(),
            "accepted {original} -> {changed}"
        );
        fs::write(&path, source).unwrap();
    }
}

#[test]
fn direct_candidate_suites_need_no_justfile_and_reject_step_environment_overrides() {
    let (repository, cache) = candidate_fixture();
    fs::remove_file(repository.path().join("Justfile")).unwrap();
    let routine_path = repository.path().join(".github/workflows/tmpl-verify.yml");
    let routine = fs::read_to_string(&routine_path).unwrap();
    fs::write(
        &routine_path,
        routine.replace(
            "mise x -- just check tmpl-verify.yml::",
            "ci-policy project-check --suite tmpl-verify.yml::",
        ),
    )
    .unwrap();
    let proof_path = repository
        .path()
        .join(".github/workflows/ci-policy-checks.yml");
    let proof = fs::read_to_string(&proof_path).unwrap();
    let original = "mise x -- just check-candidate \"$CANDIDATE_SHA\" ci-policy-checks.yml::proofs";
    assert!(proof.contains(original));
    for command in [
        "ci-policy project-check --revision \"$CANDIDATE_SHA\" --phase candidate --suite ci-policy-checks.yml::proofs",
        "mise x -- ci-policy project-check --revision \"$CANDIDATE_SHA\" --phase candidate --suite ci-policy-checks.yml::proofs",
    ] {
        let direct = proof.replace(original, command);
        fs::write(&proof_path, &direct).unwrap();
        accepted(&invoke(&repository, &cache, &[]));
        let overridden = direct.replace(
            &format!("- run: {command}"),
            &format!("- env: {{ CANDIDATE_SHA: 1234567890abcdef1234567890abcdef12345678 }}\n        run: {command}"),
        );
        assert_ne!(direct, overridden);
        fs::write(&proof_path, overridden).unwrap();
        let invalid = invoke(&repository, &cache, &[]);
        assert!(!invalid.status.success());
        assert!(String::from_utf8_lossy(&invalid.stderr).contains("authoritative project gate"));
    }
}

#[test]
fn mixed_suites_require_only_the_just_recipes_they_invoke() {
    for direct_candidate in [true, false] {
        let (repository, cache) = candidate_fixture();
        let just_path = repository.path().join("Justfile");
        let just = fs::read_to_string(&just_path).unwrap();
        let (workflow, original, direct, unused) = if direct_candidate {
            (
                "ci-policy-checks.yml",
                "mise x -- just check-candidate \"$CANDIDATE_SHA\" ci-policy-checks.yml::proofs",
                "ci-policy project-check --revision \"$CANDIDATE_SHA\" --phase candidate --suite ci-policy-checks.yml::proofs",
                "check-candidate revision suite=\"local\":\n    ci-policy project-check --revision {{quote(revision)}} --phase candidate --suite {{quote(suite)}}\n",
            )
        } else {
            (
                "tmpl-verify.yml",
                "mise x -- just check tmpl-verify.yml::",
                "ci-policy project-check --suite tmpl-verify.yml::",
                "check suite=\"local\":\n    ci-policy project-check --suite {{quote(suite)}}\n",
            )
        };
        assert!(just.contains(unused));
        let just = just.replace(unused, "");
        fs::write(&just_path, &just).unwrap();
        let workflow_path = repository
            .path()
            .join(format!(".github/workflows/{workflow}"));
        let source = fs::read_to_string(&workflow_path).unwrap();
        assert!(source.contains(original));
        fs::write(&workflow_path, source.replace(original, direct)).unwrap();
        accepted(&invoke(&repository, &cache, &[]));
        fs::write(
            &just_path,
            just.replace("ci-policy project-check", "rustc --version"),
        )
        .unwrap();
        let invalid = invoke(&repository, &cache, &[]);
        assert!(!invalid.status.success());
        assert!(String::from_utf8_lossy(&invalid.stderr).contains("must invoke"));
    }
}

#[test]
fn candidate_phase_runs_later_checks_only_from_the_exact_committed_source() {
    let (repository, cache) = candidate_fixture();
    let path = repository.path().join(".ci/verification.json");
    let mut value: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    value["checks"][2]["command"] = json!(["rustc", "--invalid-candidate-verification-flag"]);
    repository.write(
        ".ci/verification.json",
        &serde_json::to_vec(&value).unwrap(),
    );
    accepted(&invoke(&repository, &cache, &[]));
    let revision = repository.commit(&[]);
    let output = invoke(
        &repository,
        &cache,
        &["--phase", "candidate", "--revision", &revision],
    );
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("invalid-candidate-verification-flag")
    );
    let output = invoke(
        &repository,
        &cache,
        &["--suite", "ci-policy-checks.yml::proofs"],
    );
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("declared verification phase"));
}

#[test]
fn candidate_phase_selection_exhausts_the_complete_phase_table() {
    use ci_policy::project::protocol::{Cache, Decision, Phase, decide};
    let phases = [Phase::Commit, Phase::Development, Phase::Candidate];
    for (selected, phase) in phases.iter().enumerate() {
        for (required, check) in phases.iter().enumerate() {
            assert_eq!(
                decide(*phase, *check, true, Cache::Always, false),
                if selected >= required {
                    Decision::Run
                } else {
                    Decision::NotSelected
                }
            );
        }
    }
}

#[test]
fn candidate_contract_rejects_missing_proofs_platforms_and_revision_entrypoints() {
    let (repository, cache) = candidate_fixture();
    let path = repository.path().join(".ci/verification.json");
    let original = fs::read(&path).unwrap();
    let value: Value = serde_json::from_slice(&original).unwrap();
    let mut missing = value.clone();
    missing["checks"].as_array_mut().unwrap().pop();
    fs::write(&path, serde_json::to_vec(&missing).unwrap()).unwrap();
    let output = invoke(&repository, &cache, &[]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("unknown check"));
    let mut incomplete = value;
    incomplete["checks"][2]["platforms"] = json!(["linux", "macos", "windows"]);
    fs::write(&path, serde_json::to_vec(&incomplete).unwrap()).unwrap();
    let output = invoke(&repository, &cache, &[]);
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("every declared project check and platform")
    );
    fs::write(&path, original).unwrap();
    let path = repository.path().join("Justfile");
    let original = fs::read_to_string(&path).unwrap();
    fs::write(
        &path,
        original.replace("check-candidate revision", "unbound-candidate revision"),
    )
    .unwrap();
    let output = invoke(&repository, &cache, &[]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("exact revision entrypoint"));
}

#[test]
fn an_explicit_project_contract_can_bind_ci_without_a_justfile() {
    let (repository, cache) = fixture();
    fs::remove_file(repository.path().join("Justfile")).unwrap();
    let path = repository.path().join(".github/workflows/ci.yml");
    let source = fs::read_to_string(&path).unwrap().replace(
        "mise x -- just check ci.yml::quality",
        "mise x -- ci-policy project-check --suite ci.yml::quality",
    );
    fs::write(&path, &source).unwrap();
    accepted(&invoke(&repository, &cache, &[]));
    for command in [
        "ci-policy project-check --suite ci.yml::quality --phase commit",
        "ci-policy project-check --suite ci.yml::other",
        "ci-policy project-check --suite ci.yml::quality || true",
    ] {
        fs::write(
            &path,
            source.replace(
                "mise x -- ci-policy project-check --suite ci.yml::quality",
                command,
            ),
        )
        .unwrap();
        assert!(!invoke(&repository, &cache, &[]).status.success());
    }
}

#[test]
fn another_repository_can_use_the_immutable_shared_aggregate_action() {
    let (repository, cache) = fixture();
    let path = repository.path().join(".github/workflows/ci.yml");
    let source = fs::read_to_string(&path).unwrap();
    let reference = "P4suta/project-template/.github/actions/ci-policy@0a0bc78cfe9370c14beecbeca8f230f824218790";
    let aggregate = format!(
        "  required:\n    if: always()\n    needs: [quality]\n    runs-on: ubuntu-latest\n    steps:\n      - id: policy\n        uses: {reference}\n      - env:\n          POLICY: ${{{{ steps.policy.outputs.executable }}}}\n          RESULTS: ${{{{ toJSON(needs) }}}}\n        run: '\"$POLICY\" gate --needs \"$RESULTS\" --require-json ''[\"quality\"]'''\n"
    );
    fs::write(&path, format!("{source}{aggregate}")).unwrap();
    let mut value = contract();
    value["hosted"] = json!([{
        "workflow": "ci.yml", "job": "required", "capability": "aggregate"
    }]);
    repository.write(
        ".ci/verification.json",
        &serde_json::to_vec(&value).unwrap(),
    );
    accepted(&invoke(&repository, &cache, &[]));
    for action in [
        "P4suta/project-template/.github/actions/ci-policy@main",
        "another-owner/project-template/.github/actions/ci-policy@0a0bc78cfe9370c14beecbeca8f230f824218790",
        "P4suta/project-template/.github/actions/other@0a0bc78cfe9370c14beecbeca8f230f824218790",
    ] {
        fs::write(
            &path,
            format!("{source}{}", aggregate.replace(reference, action)),
        )
        .unwrap();
        assert!(!invoke(&repository, &cache, &[]).status.success());
    }
}

#[test]
fn a_pinned_cargo_extension_runs_the_actual_project_tests() {
    let (repository, cache) = fixture();
    repository.write("Cargo.toml", b"[package]\nname = \"extension-fixture\"\nversion = \"0.1.0\"\nedition = \"2024\"\npublish = false\n");
    repository.write(
        "Cargo.lock",
        b"version = 4\n\n[[package]]\nname = \"extension-fixture\"\nversion = \"0.1.0\"\n",
    );
    repository.write(
        "src/lib.rs",
        b"#[test]\nfn real_project_behavior() {\n    assert_eq!(1 + 1, 2);\n}\n",
    );
    let mut value = contract();
    value["checks"][0]["command"] = json!(["cargo", "nextest", "run", "--locked"]);
    value["checks"][0]["tools"] = json!(["rust@1.99.0", "cargo:cargo-nextest@0.9.146"]);
    repository.write(
        ".ci/verification.json",
        &serde_json::to_vec(&value).unwrap(),
    );
    let output = invoke(&repository, &cache, &[]);
    accepted(&output);
    assert!(String::from_utf8_lossy(&output.stderr).contains("real_project_behavior"));
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
fn cached_success_does_not_admit_zero_compiler_jobs() {
    let (repository, cache) = fixture();
    let first = invocation(&repository, &cache, &[])
        .env("CARGO_BUILD_JOBS", "1")
        .output()
        .unwrap();
    accepted(&first);
    assert!(String::from_utf8_lossy(&first.stdout).contains("\"outcome\":\"ran\""));
    let invalid = invocation(&repository, &cache, &[])
        .env("CARGO_BUILD_JOBS", "0")
        .output()
        .unwrap();
    assert!(!invalid.status.success());
    assert!(String::from_utf8_lossy(&invalid.stderr).contains("CARGO_BUILD_JOBS"));
    assert!(!String::from_utf8_lossy(&invalid.stdout).contains("\"outcome\":\"reused\""));
    let reused = invocation(&repository, &cache, &[])
        .env("CARGO_BUILD_JOBS", "1")
        .output()
        .unwrap();
    accepted(&reused);
    assert!(String::from_utf8_lossy(&reused.stdout).contains("\"outcome\":\"reused\""));
}

#[test]
fn self_policy_action_requires_the_original_repository_identity() {
    let (repository, cache) = fixture();
    let path = repository.path().join(".github/workflows/ci.yml");
    let source = fs::read_to_string(&path).unwrap();
    let aggregate = "  required:\n    if: always()\n    needs: [quality]\n    runs-on: ubuntu-latest\n    steps:\n      - id: policy\n        uses: $/.github/actions/ci-policy\n      - env:\n          POLICY: ${{ steps.policy.outputs.executable }}\n          RESULTS: ${{ toJSON(needs) }}\n        run: '\"$POLICY\" gate --needs \"$RESULTS\" --require-json ''[\"quality\"]'''\n";
    repository.write(
        ".github/workflows/ci.yml",
        format!("{source}{aggregate}").as_bytes(),
    );
    repository.write(".github/actions/ci-policy/action.yml", b"name: Untrusted local policy\nruns:\n  using: composite\n  steps:\n    - shell: bash\n      run: echo unchecked\n");
    let mut value = contract();
    value["hosted"] = json!([{"workflow":"ci.yml","job":"required","capability":"aggregate"}]);
    repository.write(
        ".ci/verification.json",
        &serde_json::to_vec(&value).unwrap(),
    );
    let unknown = invocation(&repository, &cache, &[])
        .env("GITHUB_REPOSITORY", "P4suta/project-template")
        .output()
        .unwrap();
    assert!(!unknown.status.success());
    assert!(String::from_utf8_lossy(&unknown.stderr).contains("hosted capability"));
    repository.git(&[
        "remote",
        "add",
        "origin",
        "https://github.com/another-owner/project-template.git",
    ]);
    let foreign = invoke(&repository, &cache, &[]);
    assert!(!foreign.status.success());
    assert!(String::from_utf8_lossy(&foreign.stderr).contains("hosted capability"));
    for origin in [
        "https://github.com/P4suta/project-template.git?ref=main",
        "https://github.com.example/P4suta/project-template.git",
        "https://github.com/another-owner/project-template.git",
        "git@custom.example:P4suta/project-template.git",
    ] {
        repository.git(&["remote", "set-url", "origin", origin]);
        let invalid = invoke(&repository, &cache, &[]);
        assert!(!invalid.status.success());
        assert!(String::from_utf8_lossy(&invalid.stderr).contains("hosted capability"));
    }
    for origin in [
        "https://github.com/P4suta/project-template",
        "https://github.com/P4suta/project-template.git",
        "git@github.com:P4suta/project-template",
        "git@github.com:P4suta/project-template.git",
        "ssh://git@github.com/P4suta/project-template",
        "ssh://git@github.com/P4suta/project-template.git",
    ] {
        repository.git(&["remote", "set-url", "origin", origin]);
        accepted(&invoke(&repository, &cache, &[]));
    }
    for origins in [
        [
            "https://github.com/P4suta/project-template.git",
            "https://github.com/another-owner/project-template.git",
        ],
        [
            "https://github.com/another-owner/project-template.git",
            "https://github.com/P4suta/project-template.git",
        ],
    ] {
        repository.git(&["config", "--unset-all", "remote.origin.url"]);
        for origin in origins {
            repository.git(&["config", "--add", "remote.origin.url", origin]);
        }
        let ambiguous = invoke(&repository, &cache, &[]);
        assert!(!ambiguous.status.success());
        assert!(String::from_utf8_lossy(&ambiguous.stderr).contains("hosted capability"));
    }
    repository.git(&["config", "--unset-all", "remote.origin.url"]);
    repository.git(&[
        "config",
        "--add",
        "remote.origin.url",
        "https://github.com/P4suta/project-template.git",
    ]);
    accepted(&invoke(&repository, &cache, &[]));
}

#[test]
fn consumers_cannot_replace_the_shared_policy_with_a_local_reusable_workflow() {
    let (repository, cache) = fixture();
    let path = repository.path().join(".github/workflows/ci.yml");
    let source = fs::read_to_string(&path).unwrap();
    let local = format!("{source}  policy:\n    uses: $/.github/workflows/common-policy.yml\n");
    repository.write(".github/workflows/ci.yml", local.as_bytes());
    repository.write(".github/workflows/common-policy.yml", b"name: Local approval\non: workflow_call\njobs:\n  approval:\n    runs-on: ubuntu-latest\n    environment: policy-approval\n    steps:\n      - run: echo unchecked\n");
    let mut value = contract();
    value["hosted"] = json!([
        {"workflow":"ci.yml","job":"policy","capability":"policy"},
        {"workflow":"common-policy.yml","job":"approval","capability":"approval"}
    ]);
    repository.write(
        ".ci/verification.json",
        &serde_json::to_vec(&value).unwrap(),
    );
    let unknown = invoke(&repository, &cache, &[]);
    assert!(!unknown.status.success());
    assert!(String::from_utf8_lossy(&unknown.stderr).contains("hosted capability"));
    repository.git(&[
        "remote",
        "add",
        "origin",
        "https://github.com/another-owner/project-template.git",
    ]);
    let foreign = invoke(&repository, &cache, &[]);
    assert!(!foreign.status.success());
    assert!(String::from_utf8_lossy(&foreign.stderr).contains("hosted capability"));
    let shared = local.replace("$/.github/workflows/common-policy.yml", "P4suta/project-template/.github/workflows/common-policy.yml@0a0bc78cfe9370c14beecbeca8f230f824218790");
    repository.write(".github/workflows/ci.yml", shared.as_bytes());
    accepted(&invoke(&repository, &cache, &[]));
}

#[test]
fn candidate_self_policy_steps_require_the_original_repository() {
    let (repository, cache) = candidate_fixture();
    accepted(&invoke(&repository, &cache, &[]));
    repository.git(&[
        "remote",
        "set-url",
        "origin",
        "https://github.com/another-owner/project-template.git",
    ]);
    let foreign = invoke(&repository, &cache, &[]);
    assert!(!foreign.status.success());
    assert!(String::from_utf8_lossy(&foreign.stderr).contains("hosted capability"));
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
fn the_native_runner_honors_lower_compiler_limits_and_rejects_zero() {
    let (repository, cache, target) = descendant_fixture();
    let marker = cache.path().join("compiler-jobs");
    let path = repository.path().join(".ci/verification.json");
    let mut value: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    value["checks"][0]["environment"] = json!(["CI_POLICY_TEST_MARKER", "CI_POLICY_TEST_MODE"]);
    value["checks"][0]["cache"] = json!("always");
    fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
    for (requested, expected) in [("1", "1"), ("32", "2")] {
        let output = invocation(&repository, &cache, &[])
            .env("CARGO_BUILD_JOBS", requested)
            .env("CARGO_TARGET_DIR", &target)
            .env("CI_POLICY_TEST_MARKER", &marker)
            .env("CI_POLICY_TEST_MODE", "environment")
            .output()
            .unwrap();
        accepted(&output);
        assert_eq!(fs::read_to_string(&marker).unwrap(), expected);
    }
    fs::remove_file(&marker).unwrap();
    let output = invocation(&repository, &cache, &[])
        .env("CARGO_BUILD_JOBS", "0")
        .env("CARGO_TARGET_DIR", &target)
        .env("CI_POLICY_TEST_MARKER", &marker)
        .env("CI_POLICY_TEST_MODE", "environment")
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(!marker.exists());
}

#[test]
fn public_advisory_fetches_keep_https_without_modifying_the_owners_git_policy() {
    let (repository, cache) = fixture();
    repository.write("Cargo.toml", b"[package]\nname = \"audit-fixture\"\nversion = \"0.1.0\"\nedition = \"2024\"\nlicense = \"MIT\"\npublish = false\n");
    repository.write(
        "Cargo.lock",
        b"version = 4\n\n[[package]]\nname = \"audit-fixture\"\nversion = \"0.1.0\"\n",
    );
    let configuration = tempfile::NamedTempFile::new().unwrap();
    let original =
        b"[url \"ssh://unavailable.invalid/\"]\n    insteadOf = https://github.com/RustSec/\n";
    fs::write(configuration.path(), original).unwrap();
    let mut value = contract();
    value["checks"][0]["command"] = json!(["cargo", "deny", "--locked", "check", "advisories"]);
    value["checks"][0]["phase"] = json!("development");
    value["checks"][0]["tools"] = json!(["rust@1.99.0", "cargo:cargo-deny@0.20.2"]);
    value["checks"][0]["environment"] = json!(["GIT_CONFIG_GLOBAL"]);
    value["checks"][0]["timeout_seconds"] = json!(60);
    repository.write(
        ".ci/verification.json",
        &serde_json::to_vec(&value).unwrap(),
    );
    let output = invocation(&repository, &cache, &[])
        .env("GIT_CONFIG_GLOBAL", configuration.path())
        .output()
        .unwrap();
    accepted(&output);
    assert_eq!(fs::read(configuration.path()).unwrap(), original);
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
