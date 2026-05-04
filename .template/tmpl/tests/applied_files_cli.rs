//! End-to-end coverage for the `tmpl applied-files` subcommand.
//!
//! The command reads `<dest>/.template/state.toml` and prints each
//! recorded file path to stdout — newline-separated by default,
//! NUL-separated under `--null` for `xargs -0` consumption. These
//! tests build a State fixture, save it through the public API, then
//! invoke the binary and assert the output.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::process::Command;

use tmpl::{AppliedEntry, AppliedFileEntry, ContentHash, LayerName, RenderedPath, State};

fn build_fixture_state() -> State {
    let mut applied = BTreeMap::new();
    applied.insert(
        LayerName::new("typos").expect("typos layer name"),
        AppliedEntry {
            content_hash: ContentHash([0u8; 32]),
            applied_at: "2026-04-30T00:00:00Z".into(),
            files: vec![AppliedFileEntry {
                path: RenderedPath::new("_typos.toml").expect("typos path"),
                content_hash: ContentHash([0u8; 32]),
                executable: false,
            }],
        },
    );
    applied.insert(
        LayerName::new("core").expect("core layer name"),
        AppliedEntry {
            content_hash: ContentHash([0u8; 32]),
            applied_at: "2026-04-30T00:00:00Z".into(),
            files: vec![
                AppliedFileEntry {
                    path: RenderedPath::new("README.md").expect("README path"),
                    content_hash: ContentHash([0u8; 32]),
                    executable: false,
                },
                AppliedFileEntry {
                    path: RenderedPath::new(".gitignore").expect("gitignore path"),
                    content_hash: ContentHash([0u8; 32]),
                    executable: false,
                },
            ],
        },
    );
    State {
        engine_version: "0.1.0".into(),
        merkle_root: ContentHash([0u8; 32]),
        applied,
    }
}

fn write_state(dest: &Path) {
    let template_dir = dest.join(".template");
    fs::create_dir_all(&template_dir).expect("mkdir .template");
    build_fixture_state()
        .save(&template_dir.join("state.toml"))
        .expect("save state.toml");
}

#[test]
fn applied_files_prints_recorded_paths_newline_separated_by_default() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_state(dir.path());

    let output = Command::new(env!("CARGO_BIN_EXE_tmpl"))
        .arg("--dest")
        .arg(dir.path())
        .arg("applied-files")
        .output()
        .expect("spawn tmpl");

    assert!(
        output.status.success(),
        "tmpl applied-files failed; stderr:\n{}",
        String::from_utf8_lossy(&output.stderr),
    );
    let stdout = String::from_utf8(output.stdout).expect("stdout utf-8");
    // BTreeMap orders core before typos. Within core, files preserve
    // their recorded Vec order (README.md, then .gitignore).
    assert_eq!(stdout, "README.md\n.gitignore\n_typos.toml\n");
}

#[test]
fn applied_files_with_null_uses_nul_separator() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_state(dir.path());

    let output = Command::new(env!("CARGO_BIN_EXE_tmpl"))
        .arg("--dest")
        .arg(dir.path())
        .arg("applied-files")
        .arg("--null")
        .output()
        .expect("spawn tmpl");

    assert!(
        output.status.success(),
        "tmpl applied-files --null failed; stderr:\n{}",
        String::from_utf8_lossy(&output.stderr),
    );
    let stdout = String::from_utf8(output.stdout).expect("stdout utf-8");
    assert_eq!(stdout, "README.md\0.gitignore\0_typos.toml\0");
}

#[test]
fn applied_files_treats_missing_state_as_empty() {
    // `State::load` canonicalises a missing state.toml to the empty
    // state (mirrors `state::tests::state_load_treats_missing_as_empty`).
    // `applied-files` follows suit: empty stdout, exit 0. Callers
    // (e.g. init.yml) are responsible for refusing to act on an empty
    // whitelist before running destructive operations.
    let dir = tempfile::tempdir().expect("tempdir");

    let output = Command::new(env!("CARGO_BIN_EXE_tmpl"))
        .arg("--dest")
        .arg(dir.path())
        .arg("applied-files")
        .output()
        .expect("spawn tmpl");

    assert!(
        output.status.success(),
        "tmpl applied-files must succeed when state.toml is absent; stderr:\n{}",
        String::from_utf8_lossy(&output.stderr),
    );
    assert!(output.stdout.is_empty(), "stdout must be empty");
}
