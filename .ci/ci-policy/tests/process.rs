use std::process::Command;

use ci_policy::local::execute;

#[test]
fn an_unavailable_required_tool_cannot_pass() {
    let program = std::env::temp_dir().join(format!("ci-policy-absent-{}", std::process::id()));
    assert!(execute(&mut Command::new(program), None).is_err());
}

#[test]
fn the_real_child_exit_status_controls_the_gate() {
    let binary = env!("CARGO_BIN_EXE_ci-policy");
    assert!(execute(Command::new(binary).arg("--help"), None).is_ok());
    assert!(
        execute(
            Command::new(binary).args(["gate", "--needs", "{}", "--require", "check"]),
            None
        )
        .is_err()
    );
}
