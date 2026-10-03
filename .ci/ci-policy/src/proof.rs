use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
    process::Command,
};

use anyhow::{Context, Result, ensure};
use serde_json::Value;

use crate::{Conclusion, gate};

pub const HARNESSES: [&str; 9] = [
    "handoff::proofs::initialization_keeps_exactly_rendered_files",
    "workflow::proofs::read_only_checkout_cannot_retain_credentials",
    "proofs::installed_policy_requires_the_reviewed_revision",
    "proofs::gate_rejection_is_permanent",
    "proofs::gate_requires_every_selected_check",
    "proofs::commit_identity_rejects_any_non_hexadecimal_byte",
    "verification::proofs::adding_a_file_cannot_remove_a_required_check",
    "verification::proofs::every_check_has_a_unique_plan_position",
    "proof::proofs::proof_contracts_must_be_reachable",
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
            if function.starts_with("std::") && file.contains("/lib/rustlib/src/rust/library/") =>
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
    use super::{PropertySource, PropertyState, property_checked};
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
