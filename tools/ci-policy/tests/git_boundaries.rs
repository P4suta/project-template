mod common;

use ci_policy::local::{
    PushUpdate, indexed_files, introduced_commits, introduced_history_options, pushed_files,
};
use common::Repository;

#[test]
fn a_push_cannot_omit_unchanged_workflows_from_the_common_ci_gate() {
    let repository = Repository::new();
    repository.write(
        ".github/workflows/codeql.yml",
        b"permissions:\n  security-events: write\n",
    );
    repository.write("README.md", b"Original\n");
    let base = repository.commit(&[]);
    repository.git(&["update-ref", "refs/remotes/origin/main", &base]);
    repository.write("README.md", b"Updated\n");
    let candidate = repository.commit(&[&base]);
    let update = PushUpdate::parse(&format!(
        "refs/heads/main {candidate} refs/heads/main {base}"
    ))
    .unwrap();
    let files = pushed_files(repository.path(), "origin", &update).unwrap();
    assert!(
        files
            .iter()
            .any(|file| file.checked && file.path == ".github/workflows/codeql.yml")
    );
}

#[test]
fn old_external_links_do_not_block_an_unrelated_change() {
    let repository = Repository::new();
    repository.write("README.md", b"Original\n");
    let first = repository.commit(&[]);
    repository.entry(
        "120000",
        &repository.blob(b"../external/source"),
        "scripts/old.sh",
    );
    repository.entry("160000", &first, "vendor");
    let base = repository.commit(&[&first]);
    repository.write("README.md", b"Updated\n");
    let files = indexed_files(repository.path()).expect("changed scope");
    assert_eq!(
        files
            .iter()
            .map(|file| file.path.as_str())
            .collect::<Vec<_>>(),
        ["README.md"]
    );
    let candidate = repository.commit(&[&base]);
    repository.entry("160000", &candidate, "vendor");
    let files = indexed_files(repository.path()).expect("new external pointer");
    assert_eq!(files.len(), 1);
    assert_eq!(
        files[0].bytes,
        format!("Gitlink revision: {candidate}\n").as_bytes()
    );
    repository.entry("160000", &candidate, ".github/actions/external");
    assert!(indexed_files(repository.path()).is_err());
}

#[cfg(unix)]
#[test]
fn unchanged_nonportable_legacy_paths_do_not_expand_the_checked_scope() {
    let repository = Repository::new();
    repository.write("old:name.md", b"Legacy\n");
    repository.write("old:name.sh", b"#!/bin/sh\nexit 0\n");
    repository.commit(&[]);
    repository.write("README.md", b"Current\n");
    let files = indexed_files(repository.path()).expect("only changed and support paths");
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].path, "README.md");
}

#[test]
fn pushed_identity_is_checked_even_when_head_and_worktree_are_newer() {
    let repository = Repository::new();
    repository.write("config.json", b"{}\n");
    let base = repository.commit(&[]);
    repository.git(&["update-ref", "refs/remotes/origin/main", &base]);
    repository.write("config.json", br#"{"duplicate":true,"duplicate":false}"#);
    let candidate = repository.commit(&[&base]);
    repository.write("config.json", b"{}\n");
    repository.commit(&[&candidate]);
    let update = PushUpdate::parse(&format!(
        "refs/heads/candidate {candidate} refs/heads/candidate {base}"
    ))
    .expect("push fixture");
    let files = pushed_files(repository.path(), "origin", &update).expect("exact candidate");
    assert_eq!(files.len(), 1);
    assert!(ci_policy::json::parse(&files[0].bytes).is_err());
}

#[test]
fn remote_merge_history_is_trusted_without_rescanning_old_commits() {
    let repository = Repository::new();
    repository.write("README.md", b"Base\n");
    let base = repository.commit(&[]);
    repository.write("remote.txt", b"Already on the remote\n");
    let remote = repository.commit(&[&base]);
    repository.git(&["update-ref", "refs/remotes/origin/main", &remote]);
    repository.write("candidate.txt", b"Local change\n");
    let merge = repository.commit(&[&base, &remote]);
    let update = PushUpdate::parse(&format!(
        "refs/heads/candidate {merge} refs/heads/candidate {base}"
    ))
    .expect("existing branch");
    assert_eq!(
        introduced_commits(repository.path(), "origin", &update)
            .expect("history boundary")
            .as_slice(),
        std::slice::from_ref(&merge)
    );
    let update = PushUpdate::parse(&format!(
        "refs/heads/candidate {merge} refs/heads/candidate {}",
        "0".repeat(40)
    ))
    .expect("new branch");
    assert_eq!(
        introduced_commits(repository.path(), "origin", &update).expect("history boundary"),
        [merge]
    );
    assert!(introduced_commits(repository.path(), "origin*", &update).is_err());
}

#[test]
fn merge_only_added_content_is_included_in_introduced_history_scans() {
    let repository = Repository::new();
    repository.write("README.md", b"Base\n");
    let base = repository.commit(&[]);
    repository.write("remote.txt", b"Published\n");
    let remote = repository.commit(&[&base]);
    repository.git(&["update-ref", "refs/remotes/origin/main", &remote]);
    repository.git(&["read-tree", "--reset", "-u", &base]);
    repository.write("local.txt", b"Local\n");
    let local = repository.commit(&[&base]);
    repository.write("remote.txt", b"Published\n");
    repository.write("merge-only.txt", b"Unique merge-only content\n");
    let merge = repository.commit(&[&local, &remote]);
    repository.git(&["rm", "--", "merge-only.txt"]);
    let final_revision = repository.commit(&[&merge]);
    let update = PushUpdate::parse(&format!(
        "refs/heads/candidate {final_revision} refs/heads/candidate {base}"
    ))
    .expect("candidate");
    let options = introduced_history_options("origin", &update).expect("history options");
    let mut arguments = vec!["log", "-p", "-U0"];
    arguments.extend(options.split_whitespace());
    let output = repository.git(&arguments);
    assert!(
        String::from_utf8(output)
            .expect("patches")
            .contains("+Unique merge-only content")
    );
}

#[test]
fn fixture_git_does_not_target_the_parent_hook_repository() {
    const MARKER: &str = "CI_POLICY_FIXTURE_ISOLATION_TEST";
    if std::env::var_os(MARKER).is_some() {
        let repository = Repository::new();
        repository.write("fixture.txt", b"Fixture\n");
        let object = repository.blob(b"Fixture object\n");
        assert_eq!(object.len(), 40);
        repository.commit(&[]);
        assert!(repository.path().join(".git").is_dir());
        return;
    }
    let outer = Repository::new();
    outer.write("owner.txt", b"Owner\n");
    let original = outer.commit(&[]);
    let status = std::process::Command::new(std::env::current_exe().expect("test executable"))
        .args([
            "--exact",
            "fixture_git_does_not_target_the_parent_hook_repository",
            "--nocapture",
        ])
        .env(MARKER, "1")
        .env("GIT_DIR", outer.path().join(".git"))
        .env("GIT_WORK_TREE", outer.path())
        .env("GIT_INDEX_FILE", outer.path().join(".git/index"))
        .env("GIT_OBJECT_DIRECTORY", outer.path().join(".git/objects"))
        .env(
            "GIT_ALTERNATE_OBJECT_DIRECTORIES",
            outer.path().join(".git/objects"),
        )
        .env("GIT_COMMON_DIR", outer.path().join(".git"))
        .status()
        .expect("child test");
    assert!(status.success());
    assert_eq!(
        String::from_utf8(outer.git(&["rev-parse", "HEAD"]))
            .expect("owner revision")
            .trim(),
        original
    );
    assert_eq!(outer.git(&["ls-files", "-z"]), b"owner.txt\0");
}
