use ci_policy::{Conclusion, gate, immutable_reference, same_revision};

#[test]
fn installation_identity_cannot_accept_an_old_or_unidentified_binary() {
    let current = b"3d3c42e5aac5ba805825da76410c181273ba90b1";
    let stale = b"c2a87611a18de5b3828c5652fe268e992400cb5c";
    assert!(same_revision(current, current));
    assert!(!same_revision(current, stale));
    assert!(!same_revision(current, b"development"));
    assert!(!same_revision(&current[..39], current));
}

#[test]
fn a_missing_or_skipped_required_check_cannot_pass() {
    assert!(!gate(&[]));
    for result in [
        Conclusion::Failure,
        Conclusion::Cancelled,
        Conclusion::Skipped,
    ] {
        assert!(!gate(&[Conclusion::Success, result]));
    }
    assert!(gate(&[Conclusion::Success, Conclusion::Success]));
}

#[test]
fn action_references_require_an_exact_immutable_identity() {
    for reference in [
        "actions/checkout@v7",
        "owner/action@main",
        "owner/action@abc123",
        "docker://alpine:latest",
        "../outside/action",
        "./../outside",
    ] {
        assert!(!immutable_reference(reference), "{reference}");
    }
    assert!(immutable_reference(
        "actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1"
    ));
    assert!(immutable_reference("./.github/actions/build"));
    assert!(immutable_reference("./"));
    assert!(immutable_reference("$/.github/actions/build"));
    assert!(!immutable_reference("$/../outside"));
    assert!(immutable_reference(
        "docker://alpine@sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
    ));
}
