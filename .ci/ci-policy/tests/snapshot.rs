use std::{fs, path::Path, process::Command};

use ci_policy::local::indexed_files;

fn git(root: &Path, args: &[&str]) {
    let status = Command::new("git")
        .current_dir(root)
        .args(args)
        .status()
        .expect("Git is required");
    assert!(status.success());
}

#[test]
fn source_and_configuration_come_from_the_index_even_when_worktree_is_valid() {
    let directory = tempfile::tempdir().expect("temporary fixture");
    let root = directory.path();
    git(root, &["init", "--template=", "--quiet"]);
    fs::write(
        root.join("config.json"),
        br#"{"broken":true,"broken":false}"#,
    )
    .expect("fixture");
    fs::write(
        root.join(".typos.toml"),
        "[default.extend-words]\nmispeled = 'mispeled'\n",
    )
    .expect("fixture");
    git(root, &["add", "config.json", ".typos.toml"]);
    fs::write(root.join("config.json"), "{}").expect("unstaged fix");
    fs::write(root.join(".typos.toml"), "").expect("unstaged configuration");
    let files = indexed_files(root).expect("immutable staged snapshot");
    let source = files
        .iter()
        .find(|file| file.path == "config.json")
        .expect("source");
    assert!(ci_policy::json::parse(&source.bytes).is_err());
    let config = files
        .iter()
        .find(|file| file.path == ".typos.toml")
        .expect("configuration");
    assert!(
        std::str::from_utf8(&config.bytes)
            .expect("text")
            .contains("extend-words")
    );
    assert_eq!(
        fs::read(root.join("config.json")).expect("owner work"),
        b"{}"
    );
}

#[test]
fn action_metadata_cannot_be_replaced_by_unstaged_content() {
    let directory = tempfile::tempdir().expect("temporary fixture");
    let root = directory.path();
    git(root, &["init", "--template=", "--quiet"]);
    fs::create_dir_all(root.join(".github/actions/check")).expect("fixture path");
    let path = ".github/actions/check/action.yml";
    fs::write(root.join(path), "name: Check\ninputs:\n  required-input:\n    required: true\nruns:\n  using: composite\n  steps: []\n").expect("fixture");
    git(root, &["add", path]);
    fs::write(root.join(path), "name: Altered\n").expect("unstaged change");
    let files = indexed_files(root).expect("snapshot");
    assert!(String::from_utf8_lossy(&files[0].bytes).contains("required-input"));
}
