use ci_policy::verification::{Check, FileKind, checks, file_kind};

#[test]
fn every_changed_file_keeps_the_common_gates() {
    let plan = checks(&[FileKind::Other]);
    assert!(plan.contains(&Check::Spelling));
    assert!(plan.contains(&Check::Secrets));
    assert!(plan.contains(&Check::Skills));
    assert!(!plan.contains(&Check::Shell));
    assert!(!plan.contains(&Check::Workflow));
}

#[test]
fn workflows_require_both_semantic_and_security_checks() {
    let plan = checks(&[file_kind(".github/workflows/check.yaml", b"on: push\n")]);
    for check in [
        Check::Workflow,
        Check::Actionlint,
        Check::Zizmor,
        Check::Yaml,
    ] {
        assert!(plan.contains(&check), "{check:?}");
    }
    assert_eq!(
        file_kind(".github/workflows-example.yaml", b""),
        FileKind::Yaml
    );
    assert_eq!(
        file_kind("script", b"#!/usr/bin/env bash\n"),
        FileKind::Shell
    );
    assert_eq!(file_kind("Cargo.toml", b""), FileKind::Toml);
}

#[test]
fn empty_staging_does_not_disable_skill_maintenance() {
    assert_eq!(checks(&[]), vec![Check::Skills, Check::Secrets]);
}
