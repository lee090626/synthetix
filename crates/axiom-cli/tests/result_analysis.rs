use std::{fs, path::Path, process::Command, time::SystemTime};

fn fixture(name: &str) -> String {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/first-product")
        .join(format!("{name}.ax"))
        .display()
        .to_string()
}

fn analysis(name: &str, extra: &[&str]) -> serde_json::Value {
    let output = Command::new(env!("CARGO_BIN_EXE_axiom"))
        .args(["analyze", &fixture(name)])
        .args(extra)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
fn analysis_separates_output_uniqueness_from_existing_candidate_diagnostics() {
    let weak = analysis("underspecified", &[]);
    assert_eq!(weak["status"], "NON_UNIQUE");
    let inputs = weak["witness"]["inputs"].as_array().unwrap();
    let results = weak["witness"]["results"].as_array().unwrap();
    assert_ne!(results[0], results[1]);
    for result in results {
        let result = result.as_u64().unwrap();
        assert!(inputs.iter().all(|input| result <= input.as_u64().unwrap()));
    }
    let complete = analysis("capped", &[]);
    assert_eq!(complete["status"], "AT_MOST_ONE");
    assert_eq!(complete["result_existence"], "NOT_CHECKED");
    assert!(complete["exactly_one"].is_null());
    assert!(complete.get("proof_result").is_none());
    assert!(complete.get("verified").is_none());
    assert_eq!(analysis("floor_cap", &[])["status"], "AT_MOST_ONE");
}

#[test]
fn empty_domain_impossible_output_and_timeout_cannot_claim_exactly_one() {
    assert_eq!(
        analysis("empty_domain", &[])["status"],
        "UNSATISFIABLE_PRECONDITION"
    );
    let impossible = analysis("impossible_result", &[]);
    assert_eq!(impossible["status"], "AT_MOST_ONE");
    assert_eq!(impossible["result_existence"], "NOT_CHECKED");
    assert!(impossible["exactly_one"].is_null());
    let unknown = analysis("capped", &["--timeout-ms", "0"]);
    assert_eq!(unknown["status"], "UNKNOWN");
    assert!(unknown["reason"].as_str().unwrap().contains("budget"));
    assert!(unknown["witness"].is_null());
}

#[test]
fn report_matches_stdout_and_analysis_does_not_emit_wasm() {
    let root = std::env::temp_dir().join(format!(
        "synthetix-analysis-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    let source = root.join("partial.ax");
    let report = root.join("report.json");
    fs::write(
        &source,
        "contract partial(x: u32) -> u32 { requires x <= 10; ensures x == 5 && result == 100; }",
    )
    .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_axiom"))
        .args([
            "analyze",
            source.to_str().unwrap(),
            "--report",
            report.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let artifact: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(artifact["status"], "AT_MOST_ONE");
    assert_eq!(artifact["result_existence"], "NOT_CHECKED");
    assert!(artifact["exactly_one"].is_null());
    assert_eq!(fs::read(report).unwrap(), output.stdout);
    assert!(!source.with_extension("wasm").exists());
    let original = fs::read(&source).unwrap();
    let rejected = Command::new(env!("CARGO_BIN_EXE_axiom"))
        .args([
            "analyze",
            source.to_str().unwrap(),
            "--report",
            source.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(!rejected.status.success());
    assert!(String::from_utf8_lossy(&rejected.stderr).contains("must not overwrite"));
    assert_eq!(fs::read(&source).unwrap(), original);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn invalid_analysis_options_are_rejected() {
    for extra in [
        ["--timeout-ms", "-1"],
        ["--timeout-ms", "4294967296"],
        ["--max-candidates", "1"],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_axiom"))
            .args(["analyze", &fixture("capped")])
            .args(extra)
            .output()
            .unwrap();
        assert!(!output.status.success());
    }
}
