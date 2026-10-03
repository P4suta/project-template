#[test]
fn duplicate_keys_cannot_hide_failure_at_any_depth() {
    for input in [
        r#"{"check":{"result":"failure"},"check":{"result":"success"}}"#,
        r#"{"check":{"result":"failure","result":"success"}}"#,
        r#"[{"result":"failure","result":"success"}]"#,
    ] {
        assert!(ci_policy::json::parse(input.as_bytes()).is_err());
    }
    assert!(ci_policy::json::parse(br#"{"check":{"result":"success","outputs":{}}}"#).is_ok());
}
