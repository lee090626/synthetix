//! `axiom build --lean` (issue #211): the Lean theorem written for a rule
//! matches the checked-in copy that CI's `lean proofs` job checks with Lean.
//! Run with `UPDATE_LEAN=1` to rewrite the copies.

use std::path::Path;
use std::process::Command;

fn check(source: &str, golden: &str) {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let dir = std::env::temp_dir().join(format!("axiom-lean-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let wasm = dir.join(format!("{golden}.wasm"));
    let out = Command::new(env!("CARGO_BIN_EXE_axiom"))
        .arg("build")
        .arg(root.join(source))
        .arg("-o")
        .arg(&wasm)
        .arg("--lean")
        .env("PATH", "")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "{stdout}{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(stdout.contains("lean_result: SKIPPED"), "{stdout}");
    let written = std::fs::read_to_string(wasm.with_extension("lean")).unwrap();
    let golden = root.join(format!("proofs/lean/generated/{golden}.lean"));
    if std::env::var_os("UPDATE_LEAN").is_some() {
        std::fs::write(&golden, &written).unwrap();
    }
    assert_eq!(written, std::fs::read_to_string(&golden).unwrap());
}

#[test]
fn fit_price_lean_theorem_matches_the_checked_copy() {
    check("examples/rules/fit_price.ax", "fit_price");
}

#[test]
fn web_counter_lean_theorem_matches_the_checked_copy() {
    check("examples/web-counter/logic.ax", "web_counter");
}
