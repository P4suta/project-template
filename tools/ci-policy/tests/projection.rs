use ci_policy::workflow::{Source, actionlint_source, parse};

#[test]
fn self_reference_projection_preserves_the_execution_source() {
    let source = Source {
        name: "check.yml".to_owned(),
        text: "# Keep this rationale.\non: push\npermissions: {}\njobs:\n  check:\n    runs-on: ubuntu-latest\n    timeout-minutes: 5\n    steps:\n      - uses: '$/.github/actions/setup' # same running commit\n      - run: echo '$/ is data here'\n".to_owned(),
    };
    let original = source.text.clone();
    let projected = actionlint_source(&source).expect("valid projection");
    assert_eq!(source.text, original);
    assert!(projected.contains("# Keep this rationale."));
    assert!(projected.contains("# same running commit"));
    let document = parse(&projected).expect("valid projected YAML");
    assert_eq!(
        document["jobs"]["check"]["steps"][0]["uses"].as_str(),
        Some("./.github/actions/setup")
    );
    assert_eq!(
        document["jobs"]["check"]["steps"][1]["run"].as_str(),
        Some("echo '$/ is data here'")
    );
}

#[test]
fn unsupported_identity_is_rejected_before_projecting() {
    let source = Source {
        name: "check.yml".to_owned(),
        text: "on: push\njobs:\n  check:\n    uses: $/../outside.yml\n".to_owned(),
    };
    assert!(actionlint_source(&source).is_err());
}
