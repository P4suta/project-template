use ci_policy::local::PushUpdate;

#[test]
fn push_validation_uses_the_actual_ref_identities() {
    let sha = "3d3c42e5aac5ba805825da76410c181273ba90b1";
    let update = PushUpdate::parse(&format!("refs/heads/main {sha} refs/heads/main {sha}"))
        .expect("valid update");
    assert_eq!(update.local_revision(), sha);
    assert_eq!(update.remote_revision(), Some(sha));
    let update = PushUpdate::parse(&format!(
        "refs/heads/change {sha} refs/heads/change {}",
        "0".repeat(40)
    ))
    .expect("new branch");
    assert_eq!(update.remote_revision(), None);
    assert!(PushUpdate::parse(&format!("HEAD {sha} refs/heads/main {sha}")).is_ok());
}

#[test]
fn malformed_or_deleted_refs_cannot_be_treated_as_verified_pushes() {
    for input in [
        "",
        "refs/heads/a HEAD refs/heads/a main",
        "refs/heads/a 0000000000000000000000000000000000000000 refs/heads/a 0000000000000000000000000000000000000000",
    ] {
        assert!(PushUpdate::parse(input).is_err());
    }
}
