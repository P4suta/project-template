use ci_policy::proof::validate_results;
use serde_json::json;

#[test]
fn standalone_source_cannot_consume_unbound_inputs() {
    use ci_policy::proof::validate_source;

    assert!(validate_source(b"pub const VALUE: u8 = 1;").is_ok());
    assert!(validate_source(b"mod proofs { fn check() { assert!(matches!(1, 1)); } }").is_ok());
    for source in [
        "mod external;",
        "#[path = \"/tmp/external.rs\"] mod external;",
        "#[cfg_attr(kani, path = \"/tmp/external.rs\")] mod external {}",
        "#[cfg_attr(kani, doc = include_str!(\"/tmp/input\"))] const VALUE: u8 = 1;",
        "const VALUE: &str = include_str!(\"/tmp/input\");",
        "const VALUE: &str = env!(\"PROOF_INPUT\");",
        "fn check() { assert!(include!(\"/tmp/assertion.rs\")); }",
        "macro_rules! hidden { () => { include!(\"/tmp/input\") } }",
        "fn check() { custom_proof!(); }",
        "use std::include_str as assert; fn check() { assert!(\"/tmp/input\"); }",
        "extern crate external;",
        "unsafe extern \"C\" { fn foreign(); }",
        "unsafe extern \"Rust\" { static FOREIGN: u8; }",
    ] {
        assert!(validate_source(source.as_bytes()).is_err(), "{source}");
    }
}

#[test]
fn comparison_operators_do_not_hide_or_become_macro_inputs() {
    use ci_policy::proof::validate_source;
    assert!(validate_source(b"fn check(left: u8, right: u8) { assert!(left != right); }").is_ok());
    assert!(validate_source(b"fn check(left: u8) { assert!(left != hidden!()); }").is_err());
}

#[test]
fn standalone_proof_inventory_requires_unique_positive_and_negative_contracts() {
    let positive = vec!["production::proofs::required".to_owned()];
    let negative = "production::proofs::counterexample";
    assert!(ci_policy::proof::source_harnesses(&positive, negative).is_ok());
    assert!(ci_policy::proof::source_harnesses(&[], negative).is_err());
    assert!(ci_policy::proof::source_harnesses(&positive, &positive[0]).is_err());
    assert!(
        ci_policy::proof::source_harnesses(&[positive[0].clone(), positive[0].clone()], negative)
            .is_err()
    );
    for name in [
        "--flag",
        "proofs::required",
        "production::::required",
        "production::../../file",
    ] {
        assert!(ci_policy::proof::source_harnesses(&[name.to_owned()], negative).is_err());
    }
}

#[test]
fn standalone_proofs_reject_missing_and_nonregular_sources_before_running_tools() {
    let root = tempfile::tempdir().unwrap();
    let required = ["production::proofs::required".to_owned()];
    let negative = "production::proofs::counterexample";
    assert!(
        ci_policy::proof::prove_source(&root.path().join("missing.rs"), &required, negative)
            .is_err()
    );
    assert!(ci_policy::proof::prove_source(root.path(), &required, negative).is_err());
}

#[test]
fn empty_or_forged_proof_receipts_do_not_complete_the_gate() {
    assert!(validate_results(&json!({}), &[], false).is_err());
    let empty = json!({"verification_results":{"summary":{"status":"completed","executed":0,"failed":0,"successful":0},"results":[]}});
    assert!(validate_results(&empty, &["required"], false).is_err());
    let forged = json!({"verification_results":{"summary":{"status":"completed","executed":1,"failed":0,"successful":1},"results":[{"harness_id":"required","status":"Success","checks":[]}]}});
    assert!(validate_results(&forged, &["required"], false).is_err());
    assert!(validate_results(&forged, &["required"], true).is_err());
}

#[test]
fn unreachable_contract_is_not_confused_with_an_unreachable_library_guard() {
    let mut receipt = json!({"verification_results":{"summary":{"status":"completed","executed":1,"failed":0,"successful":1},"results":[{"harness_id":"required","status":"Success","checks":[{"category":"assertion","status":"Success"},{"category":"cover","status":"Satisfied"},{"category":"cover","status":"Satisfied"},{"category":"assertion","status":"Unreachable","function":"required","location":{"file":"src/lib.rs"}}]}]}});
    assert!(validate_results(&receipt, &["required"], false).is_err());
    let property = &mut receipt["verification_results"]["results"][0]["checks"][3];
    property["function"] = json!("kani::rustc_intrinsics::ptr_offset_from::<u8>");
    property["location"]["file"] = json!("library/kani_core/src/models.rs");
    assert!(validate_results(&receipt, &["required"], false).is_ok());
}

#[test]
fn standard_library_guards_require_the_library_namespace_and_source() {
    let mut receipt = json!({"verification_results":{"summary":{"status":"completed","executed":1,"failed":0,"successful":1},"results":[{"harness_id":"required","status":"Success","checks":[{"category":"assertion","status":"Success"},{"category":"cover","status":"Satisfied"},{"category":"cover","status":"Satisfied"},{"category":"assertion","status":"Unreachable","function":"core::num::<impl usize>::is_multiple_of","location":{"file":"/toolchain/lib/rustlib/src/rust/library/core/src/num/uint_macros.rs"}}]}]}});
    for function in ["core::num::guard", "alloc::vec::guard", "std::ptr::guard"] {
        receipt["verification_results"]["results"][0]["checks"][3]["function"] = json!(function);
        assert!(
            validate_results(&receipt, &["required"], false).is_ok(),
            "{function}"
        );
    }
    receipt["verification_results"]["results"][0]["checks"][3]["function"] =
        json!("production::core::guard");
    assert!(validate_results(&receipt, &["required"], false).is_err());
    receipt["verification_results"]["results"][0]["checks"][3]["function"] = json!("core::guard");
    receipt["verification_results"]["results"][0]["checks"][3]["location"]["file"] =
        json!("src/lib.rs");
    assert!(validate_results(&receipt, &["required"], false).is_err());
}
