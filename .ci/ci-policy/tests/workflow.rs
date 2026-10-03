use ci_policy::workflow::{Rule, Source, analyze, parse};

fn workflow(steps: &str) -> Source {
    Source {
        name: "check.yml".to_owned(),
        text: format!(
            "name: Check\non: [pull_request]\npermissions: {{contents: read}}\nconcurrency: {{group: check, cancel-in-progress: true}}\njobs:\n  check:\n    runs-on: ubuntu-latest\n    timeout-minutes: 10\n    steps:\n{steps}"
        ),
    }
}

#[test]
fn read_only_checkout_requires_discarded_credentials() {
    let source =
        workflow("      - uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1\n");
    let summary = analyze("owner/repo", &source).expect("valid YAML");
    assert!(
        summary
            .findings
            .iter()
            .any(|finding| finding.rule == Rule::CheckoutCredentials)
    );
    for value in ["false", "'false'", "\"false\""] {
        let source = workflow(&format!(
            "      - uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1\n        with: {{persist-credentials: {value}}}\n"
        ));
        assert!(
            analyze("owner/repo", &source)
                .expect("valid YAML")
                .findings
                .is_empty()
        );
    }
}

#[test]
fn unrelated_write_access_does_not_allow_checkout_credentials() {
    let source =
        workflow("      - uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1\n");
    for permission in ["packages", "security-events", "id-token", "actions"] {
        let source = Source {
            name: source.name.clone(),
            text: source.text.replace(
                "permissions: {contents: read}",
                &format!("permissions: {{contents: read, {permission}: write}}"),
            ),
        };
        assert!(
            analyze("owner/repo", &source)
                .expect("valid workflow")
                .findings
                .iter()
                .any(|finding| finding.rule == Rule::CheckoutCredentials)
        );
    }
}

#[test]
fn reusable_jobs_cannot_hide_write_all_permissions() {
    let source = Source { name: "required.yml".to_owned(), text: "name: Required\non: workflow_call\npermissions: {contents: read}\njobs:\n  check:\n    permissions: write-all\n    uses: $/.github/workflows/check.yml\n".to_owned() };
    assert!(
        analyze("owner/repo", &source)
            .expect("valid workflow")
            .findings
            .iter()
            .any(|finding| finding.rule == Rule::ExcessivePermissions)
    );
}

#[test]
fn empty_success_placeholder_is_reported() {
    let source = Source { name: "required.yml".to_owned(), text: "name: Required\non: pull_request\npermissions: {}\njobs:\n  required:\n    runs-on: ubuntu-latest\n    timeout-minutes: 5\n    steps:\n      - run: echo nothing to aggregate\n".to_owned() };
    let summary = analyze("owner/repo", &source).expect("valid YAML");
    assert!(
        summary
            .findings
            .iter()
            .any(|finding| finding.rule == Rule::NoOpRequired)
    );
}

#[test]
fn a_diagnostic_before_real_verification_is_not_a_placeholder() {
    for command in [
        "echo checking; cargo test",
        "echo checking\n          cargo test",
        "echo $(cargo test)",
    ] {
        let source = Source {
            name: "required.yml".to_owned(),
            text: format!(
                "name: Required\non: pull_request\npermissions: {{}}\njobs:\n  required:\n    runs-on: ubuntu-latest\n    timeout-minutes: 5\n    steps:\n      - run: |\n          {command}\n"
            ),
        };
        let summary = analyze("owner/repo", &source).expect("valid YAML");
        assert!(
            !summary
                .findings
                .iter()
                .any(|finding| finding.rule == Rule::NoOpRequired)
        );
    }
}

#[test]
fn duplicate_keys_and_missing_work_fail_closed() {
    assert!(parse("on: push\non: pull_request\njobs: {}\n").is_err());
    assert!(
        analyze(
            "owner/repo",
            &Source {
                name: "check.yml".to_owned(),
                text: "on: push\njobs: {}\n".to_owned()
            }
        )
        .is_err()
    );
    assert!(parse("---\non: push\n---\non: pull_request\n").is_err());
}

#[test]
fn self_repository_uses_the_running_commit() {
    let source = workflow("      - uses: $/.github/actions/test\n");
    assert!(
        analyze("owner/repo", &source)
            .expect("valid YAML")
            .findings
            .is_empty()
    );
}
