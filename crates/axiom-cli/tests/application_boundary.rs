use std::{path::Path, process::Command};

fn mul8_fixture() -> String {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/basics/mul8.ax")
        .display()
        .to_string()
}

#[test]
fn app_run_executes_mul8_through_the_application_boundary() {
    let output = Command::new(env!("CARGO_BIN_EXE_axiom"))
        .args(["app-run", &mul8_fixture(), "--", "5"])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("boundary: APPLICATION -> VERIFIED_CORE -> APPLICATION"));
    assert!(stdout.contains("proof_result: VERIFIED"));
    assert!(stdout.contains("result: Integer(40)"));
}

#[test]
fn app_run_rejects_invalid_input_outside_the_proof_result_model() {
    let output = Command::new(env!("CARGO_BIN_EXE_axiom"))
        .args(["app-run", &mul8_fixture(), "4294967296"])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("outside u32"), "stderr: {stderr}");
    assert!(!stderr.contains("COUNTEREXAMPLE"), "stderr: {stderr}");
}

#[test]
fn app_run_rejects_text_instead_of_coercing_it_to_a_core_integer() {
    let output = Command::new(env!("CARGO_BIN_EXE_axiom"))
        .args(["app-run", &mul8_fixture(), "five"])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("expected u32, received text"),
        "stderr: {stderr}"
    );
    assert!(!stderr.contains("COUNTEREXAMPLE"), "stderr: {stderr}");
}
