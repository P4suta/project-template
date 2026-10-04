use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
    process::Command,
};

use anyhow::{Context, Result, ensure};
use serde_json::Value;

use crate::{Conclusion, gate};

mod source;
pub use source::validate_source;

pub const HARNESSES: [&str; 26] = [
    "hex::proofs::hexadecimal_identity_preserves_every_byte",
    "project::protocol::proofs::automatic_compilation_preserves_lower_owner_limits",
    "project::runtime::proofs::shared_aggregate_requires_the_policy_namespace_and_immutable_revision",
    "tools::proofs::every_tool_activates_its_complete_dependency_set",
    "verification::proofs::every_workflow_and_action_remains_required_on_push",
    "project::runtime::discovery::proofs::discovered_languages_have_supported_compiler_and_behavior_checks",
    "hooks::source::proofs::repository_hooks_always_use_the_owners_root",
    "project::index::proofs::index_input_preserves_paths_and_rejects_embedded_delimiters",
    "proof::proofs::library_namespaces_cannot_admit_project_functions",
    "proof::source::proofs::macro_recognition_requires_bang_and_delimited_arguments",
    "project::protocol::proofs::reuse_requires_applicable_exact_success",
    "project::protocol::proofs::uncovered_or_empty_ci_cannot_complete",
    "project::protocol::proofs::automatic_checks_are_bounded_by_their_phase",
    "project::protocol::proofs::cumulative_budget_cannot_overflow_or_expand",
    "project::protocol::proofs::completion_requires_success_within_the_budget",
    "project::command::proofs::cargo_operations_do_not_admit_publication",
    "handoff::proofs::initialization_keeps_exactly_rendered_files",
    "workflow::proofs::read_only_checkout_cannot_retain_credentials",
    "proofs::installed_policy_requires_the_reviewed_revision",
    "proofs::gate_rejection_is_permanent",
    "proofs::gate_requires_every_selected_check",
    "proofs::commit_identity_rejects_any_non_hexadecimal_byte",
    "verification::proofs::adding_a_file_cannot_remove_a_required_check",
    "verification::proofs::every_check_has_a_unique_plan_position",
    "proof::proofs::proof_contracts_must_be_reachable",
    "proof::source::proofs::only_closed_source_inputs_are_admitted",
];

#[derive(Clone, Copy, PartialEq, Eq)]
#[cfg_attr(kani, derive(kani::Arbitrary))]
pub enum PropertyState {
    Success,
    Failure,
    Unreachable,
    Unknown,
}

#[derive(Clone, Copy, PartialEq, Eq)]
#[cfg_attr(kani, derive(kani::Arbitrary))]
pub enum PropertySource {
    Contract,
    StandardLibrary,
    KaniModel,
}

pub fn property_checked(state: PropertyState, source: PropertySource) -> bool {
    state == PropertyState::Success
        || (state == PropertyState::Unreachable && source != PropertySource::Contract)
}

fn checked_property(check: &Value) -> bool {
    let state = match check["status"].as_str() {
        Some("Success") => PropertyState::Success,
        Some("Failure") => PropertyState::Failure,
        Some("Unreachable") => PropertyState::Unreachable,
        _ => PropertyState::Unknown,
    };
    let source = match (
        check["function"].as_str(),
        check["location"]["file"].as_str(),
    ) {
        (Some(function), Some(file))
            if library_function(function.as_bytes())
                && file.contains("/lib/rustlib/src/rust/library/") =>
        {
            PropertySource::StandardLibrary
        }
        (Some(function), Some(file))
            if function.starts_with("kani::") && file.starts_with("library/kani") =>
        {
            PropertySource::KaniModel
        }
        _ => PropertySource::Contract,
    };
    property_checked(state, source)
}

fn library_function(name: &[u8]) -> bool {
    name.starts_with(b"std::") || name.starts_with(b"core::") || name.starts_with(b"alloc::")
}

fn sources(root: &Path) -> Result<BTreeMap<std::path::PathBuf, Vec<u8>>> {
    fn visit(
        root: &Path,
        directory: &Path,
        files: &mut BTreeMap<std::path::PathBuf, Vec<u8>>,
    ) -> Result<()> {
        for entry in fs::read_dir(directory)? {
            let entry = entry?;
            let kind = entry.file_type()?;
            ensure!(
                !kind.is_symlink(),
                "proof source must not escape its package"
            );
            if kind.is_dir() {
                visit(root, &entry.path(), files)?;
            } else {
                ensure!(kind.is_file(), "proof source must be a regular file");
                files.insert(
                    entry.path().strip_prefix(root)?.to_owned(),
                    fs::read(entry.path())?,
                );
            }
        }
        Ok(())
    }
    let mut files = BTreeMap::new();
    for path in ["Cargo.toml", "Cargo.lock", "tools.json"] {
        files.insert(path.into(), fs::read(root.join(path))?);
    }
    visit(root, &root.join("src"), &mut files)?;
    ensure!(
        files
            .values()
            .try_fold(0_usize, |size, bytes| size.checked_add(bytes.len()))
            .is_some_and(|size| size <= 64 * 1024 * 1024),
        "proof source exceeds the verification budget"
    );
    Ok(files)
}

pub fn validate_results(value: &Value, expected: &[&str], counterexample: bool) -> Result<()> {
    ensure!(!expected.is_empty(), "required proof coverage is empty");
    let summary = &value["verification_results"]["summary"];
    ensure!(
        summary["status"] == "completed"
            && summary["executed"].as_u64() == Some(expected.len() as u64),
        "proof execution is incomplete"
    );
    let results = value["verification_results"]["results"]
        .as_array()
        .context("proof results are missing")?;
    let names: BTreeSet<_> = results
        .iter()
        .map(|result| {
            result["harness_id"]
                .as_str()
                .context("proof identity is missing")
        })
        .collect::<Result<_>>()?;
    ensure!(
        results.len() == expected.len() && names == expected.iter().copied().collect(),
        "proof inventory is missing, duplicated, or unexpected"
    );
    if counterexample {
        ensure!(
            summary["failed"].as_u64() == Some(expected.len() as u64),
            "counterexample was not rejected"
        );
        ensure!(
            results.iter().all(
                |result| result["checks"].as_array().is_some_and(|checks| checks
                    .iter()
                    .any(|check| check["category"] == "assertion" && check["status"] == "Failure"))
            ),
            "expected assertion counterexample is missing"
        );
    } else {
        ensure!(
            summary["failed"] == 0 && summary["successful"].as_u64() == Some(expected.len() as u64),
            "a required proof failed"
        );
        let mut conclusions = Vec::new();
        for result in results {
            let checks = result["checks"]
                .as_array()
                .context("proof properties are missing")?;
            let checked = result["status"] == "Success"
                && checks.iter().any(|check| check["category"] == "assertion")
                && checks
                    .iter()
                    .filter(|check| check["category"] == "cover" && check["status"] == "Satisfied")
                    .count()
                    >= 2
                && checks.iter().all(|check| {
                    if check["category"] == "cover" {
                        check["status"] == "Satisfied"
                    } else {
                        checked_property(check)
                    }
                });
            conclusions.push(if checked {
                Conclusion::Success
            } else {
                Conclusion::Failure
            });
        }
        ensure!(
            gate(&conclusions),
            "a proof property was skipped, unsupported, failed, or unreachable"
        );
    }
    Ok(())
}

#[cfg(kani)]
mod proofs {
    use super::{PropertySource, PropertyState, library_function, property_checked};

    #[kani::proof]
    #[kani::unwind(33)]
    fn library_namespaces_cannot_admit_project_functions() {
        let bytes: [u8; 32] = kani::any();
        let length: u8 = kani::any();
        kani::assume(length <= 32);
        let length = usize::from(length);
        let accepted = library_function(&bytes[..length]);
        assert_eq!(
            accepted,
            length >= 5 && bytes[..5] == *b"std::"
                || length >= 6 && bytes[..6] == *b"core::"
                || length >= 7 && bytes[..7] == *b"alloc::"
        );
        assert!(!library_function(b"production::core::guard"));
        kani::cover!(accepted);
        kani::cover!(!accepted);
    }
    #[kani::proof]
    fn proof_contracts_must_be_reachable() {
        let state: PropertyState = kani::any();
        let source: PropertySource = kani::any();
        assert_eq!(
            property_checked(state, PropertySource::Contract),
            state == PropertyState::Success
        );
        if matches!(state, PropertyState::Failure | PropertyState::Unknown) {
            assert!(!property_checked(state, source));
        }
        assert!(property_checked(PropertyState::Success, source));
        kani::cover!(property_checked(state, PropertySource::Contract));
        kani::cover!(!property_checked(state, PropertySource::Contract));
    }
}

fn command(manifest: &Path, directory: &Path) -> Command {
    let mut command = Command::new("cargo-kani");
    command
        .current_dir(directory)
        .env("CARGO_TARGET_DIR", directory.join("target"))
        .env("CARGO_BUILD_JOBS", "2")
        .args(["kani", "--manifest-path"])
        .arg(manifest)
        .arg("--lib");
    command
}

fn verify(manifest: &Path, counterexample: bool) -> Result<()> {
    let directory = tempfile::tempdir()?;
    let output = directory.path().join("results.json");
    let expected = if counterexample {
        &["proofs::reject_empty_gate_probe"][..]
    } else {
        &HARNESSES[..]
    };
    let mut command = command(manifest, directory.path());
    command
        .args([
            "--output-format",
            "terse",
            "-Z",
            "unstable-options",
            "--harness-timeout",
            "60s",
            "--export-json",
        ])
        .arg(&output);
    if counterexample {
        command.args([
            "--features",
            "counterexample",
            "--harness",
            expected[0],
            "--exact",
        ]);
    }
    let status = command
        .status()
        .context("required Kani verifier could not start")?;
    ensure!(
        status.success() != counterexample,
        "Kani returned an unexpected outcome: {status}"
    );
    validate_results(
        &crate::json::parse(&fs::read(output)?)?,
        expected,
        counterexample,
    )
}

pub fn source_harnesses<'a>(
    required: &'a [String],
    counterexample: &'a str,
) -> Result<Vec<&'a str>> {
    let valid = |name: &str| {
        name.starts_with("production::")
            && name.split("::").all(|part| {
                !part.is_empty()
                    && part
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
            })
    };
    ensure!(
        !required.is_empty(),
        "required source proof coverage is empty"
    );
    let mut names: Vec<_> = required.iter().map(String::as_str).collect();
    names.push(counterexample);
    ensure!(
        names.iter().all(|name| valid(name))
            && names.iter().copied().collect::<BTreeSet<_>>().len() == names.len(),
        "source proof names are invalid or duplicated"
    );
    Ok(names)
}

fn verify_source(source: &Path, expected: &[&str], counterexample: bool) -> Result<()> {
    let directory = tempfile::tempdir()?;
    let output = directory.path().join("results.json");
    let mut command = Command::new("kani");
    command
        .current_dir(directory.path())
        .arg(source)
        .args([
            "--exact",
            "--output-format",
            "terse",
            "-Z",
            "unstable-options",
            "--harness-timeout",
            "60s",
            "--export-json",
        ])
        .arg(&output)
        .arg("--target-dir")
        .arg(directory.path().join("models"));
    for name in expected {
        command.args(["--harness", name]);
    }
    let status = command
        .status()
        .context("required source verifier could not start")?;
    ensure!(
        status.success() != counterexample,
        "Kani returned an unexpected outcome: {status}"
    );
    validate_results(
        &crate::json::parse(&fs::read(output)?)?,
        expected,
        counterexample,
    )
}

pub fn prove_source(source: &Path, required: &[String], counterexample: &str) -> Result<()> {
    let expected = source_harnesses(required, counterexample)?;
    ensure!(
        cfg!(any(target_os = "macos", target_os = "linux")),
        "the native Kani gate requires Linux or Mac"
    );
    let metadata = fs::symlink_metadata(source)?;
    ensure!(
        metadata.is_file() && metadata.len() <= 64 * 1024 * 1024,
        "standalone proof source must be a bounded regular file"
    );
    let source = source.canonicalize()?;
    let original = fs::read(&source)?;
    validate_source(&original)?;
    ensure!(
        original.len() <= 64 * 1024 * 1024,
        "standalone proof source exceeds the verification budget"
    );
    let package = tempfile::tempdir()?;
    fs::write(package.path().join("production.rs"), &original)?;
    let wrapper = package.path().join("lib.rs");
    fs::write(&wrapper, b"#[path = \"production.rs\"]\nmod production;\n")?;
    let version = Command::new("kani").arg("--version").output()?;
    ensure!(
        version.status.success()
            && std::str::from_utf8(&version.stdout)?.contains("Kani Rust Verifier 0.68.0"),
        "required Kani 0.68.0 is unavailable"
    );
    let inventory = tempfile::tempdir()?;
    crate::local::execute(
        Command::new("kani")
            .current_dir(inventory.path())
            .args(["list", "--format", "json"])
            .arg(&wrapper),
        None,
    )?;
    let listed = crate::json::parse(&fs::read(inventory.path().join("kani-list.json"))?)?;
    ensure!(
        listed["kani-version"] == "0.68.0",
        "unexpected proof inventory version"
    );
    let declarations = listed["standard-harnesses"]
        .as_object()
        .context("proof declarations are missing")?;
    let names: Vec<_> = declarations
        .values()
        .map(|value| {
            value
                .as_array()
                .context("proof declarations must be arrays")
        })
        .collect::<Result<Vec<_>>>()?
        .into_iter()
        .flatten()
        .map(|value| {
            value
                .as_str()
                .context("proof declaration identity is invalid")
        })
        .collect::<Result<_>>()?;
    ensure!(
        names.len() == expected.len()
            && names.into_iter().collect::<BTreeSet<_>>() == expected.into_iter().collect(),
        "required source proof inventory drifted"
    );
    let required: Vec<_> = required.iter().map(String::as_str).collect();
    verify_source(&wrapper, &required, false)?;
    verify_source(&wrapper, &[counterexample], true)?;
    ensure!(
        fs::read(source)? == original,
        "production proof source changed during verification"
    );
    println!(
        "Verified every required source harness and the rejecting counterexample with fresh models"
    );
    Ok(())
}

pub fn prove(manifest: &Path) -> Result<()> {
    ensure!(
        cfg!(any(target_os = "linux", target_os = "macos")),
        "the native Kani gate requires Linux or Mac"
    );
    let manifest = manifest.canonicalize()?;
    let root = manifest
        .parent()
        .context("proof manifest has no directory")?;
    let original = sources(root)?;
    let package = tempfile::tempdir()?;
    for (path, bytes) in &original {
        let target = package.path().join(path);
        fs::create_dir_all(target.parent().context("proof source has no directory")?)?;
        fs::write(target, bytes)?;
    }
    let manifest = package.path().join("Cargo.toml");
    let version = Command::new("cargo-kani")
        .args(["kani", "--version"])
        .output()?;
    ensure!(
        version.status.success()
            && std::str::from_utf8(&version.stdout)?.contains("Kani Rust Verifier 0.68.0"),
        "required Kani 0.68.0 is unavailable"
    );
    let inventory = tempfile::tempdir()?;
    crate::local::execute(
        command(&manifest, inventory.path()).args(["list", "--format", "json"]),
        None,
    )?;
    let listed = crate::json::parse(&fs::read(inventory.path().join("kani-list.json"))?)?;
    ensure!(
        listed["kani-version"] == "0.68.0",
        "unexpected proof inventory version"
    );
    let declarations = listed["standard-harnesses"]
        .as_object()
        .context("proof declarations are missing")?;
    let names: Vec<_> = declarations
        .values()
        .map(|value| {
            value
                .as_array()
                .context("proof declarations must be arrays")
        })
        .collect::<Result<Vec<_>>>()?
        .into_iter()
        .flatten()
        .map(|value| {
            value
                .as_str()
                .context("proof declaration identity is invalid")
        })
        .collect::<Result<_>>()?;
    ensure!(
        names.len() == HARNESSES.len()
            && names.into_iter().collect::<BTreeSet<_>>() == HARNESSES.into_iter().collect(),
        "required proof inventory drifted"
    );
    verify(&manifest, false)?;
    verify(&manifest, true)?;
    ensure!(
        sources(root)? == original,
        "production proof source changed during verification"
    );
    println!(
        "Verified every production policy harness and the rejecting counterexample with fresh models"
    );
    Ok(())
}
