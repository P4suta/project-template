use ci_policy::proof::validate_results;
use serde_json::json;

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
