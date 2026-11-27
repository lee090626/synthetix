//! A rule over a list calls another rule over a list (issue #225, M4/M5
//! groundwork): the caller sums the list once and calls the callee's core,
//! so the callee's loop is not repeated per call and its promises are reused.

use std::{fs, path::Path, process::Command};

fn project(name: &str, rules: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("axiom-list-calls-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("synthetix.toml"),
        "[project]\nentry = \"rules.ax\"\n",
    )
    .unwrap();
    fs::write(dir.join("rules.ax"), rules).unwrap();
    dir
}

fn check(dir: &Path) -> (bool, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_axiom"))
        .arg("check")
        .current_dir(dir)
        .output()
        .unwrap();
    let text =
        String::from_utf8_lossy(&out.stdout).to_string() + &String::from_utf8_lossy(&out.stderr);
    (out.status.success(), text)
}

const TOTAL: &str = "total(prices[n])\n  need n <= 10\n  need prices <= 1000\n  always -> sum(prices)\n  promise answer >= 0\n  promise answer <= 10000\n\n";

#[test]
fn a_list_rule_can_call_a_list_rule_with_the_same_needs() {
    let text = format!(
        "{TOTAL}copy_double(items[n])\n  need n <= 10\n  need items <= 1000\n  always -> total(items) * 2\n  promise answer <= 20000\n"
    );
    let dir = project("ok", &text);
    let (ok, output) = check(&dir);
    assert!(ok, "{output}");
    assert!(output.contains("verified: copy_double__core"), "{output}");
}

#[test]
fn a_list_call_without_the_callee_needs_is_rejected_by_the_caller() {
    let text = format!(
        "{TOTAL}copy_double(items[n])\n  need n <= 10\n  always -> total(items) * 2\n  promise answer <= 20000\n"
    );
    let dir = project("missing-need", &text);
    let (ok, output) = check(&dir);
    assert!(!ok, "{output}");
    assert!(output.contains("can reach"), "{output}");
}
