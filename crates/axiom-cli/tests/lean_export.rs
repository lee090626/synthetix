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

#[test]
fn project_build_writes_one_lean_theorem_per_rule() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/web-counter");
    let dir = std::env::temp_dir().join(format!("axiom-lean-project-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    for name in ["synthetix.toml", "main.ax", "logic.ax"] {
        std::fs::copy(root.join(name), dir.join(name)).unwrap();
    }
    let out = Command::new(env!("CARGO_BIN_EXE_axiom"))
        .args(["build", "--lean"])
        .current_dir(&dir)
        .env("PATH", "")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "{stdout}{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        stdout.contains("lean: increment SKIPPED (lean not found)"),
        "{stdout}"
    );
    assert!(stdout.contains("lean_result: 0 proved, 0 not covered, 0 unproven, 0 failed"));
    assert!(dir.join("dist/lean/increment.lean").exists());
}

/// Builds an example project with `--lean` (without `lean`) and compares the
/// named rules' theorems with the checked-in copies CI checks with Lean.
fn project_golden(example: &str, rules: &[&str]) {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let source = root.join("examples").join(example);
    let dir = std::env::temp_dir().join(format!("axiom-lean-{example}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    for entry in std::fs::read_dir(&source).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_some_and(|e| e == "ax" || e == "toml") {
            std::fs::copy(&path, dir.join(path.file_name().unwrap())).unwrap();
        }
    }
    let out = Command::new(env!("CARGO_BIN_EXE_axiom"))
        .args(["build", "--lean"])
        .current_dir(&dir)
        .env("PATH", "")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "{stdout}{}",
        String::from_utf8_lossy(&out.stderr)
    );
    for rule in rules {
        let written = std::fs::read_to_string(dir.join(format!("dist/lean/{rule}.lean"))).unwrap();
        let golden = root.join(format!("proofs/lean/generated/{rule}.lean"));
        if std::env::var_os("UPDATE_LEAN").is_some() {
            std::fs::write(&golden, &written).unwrap();
        }
        assert_eq!(written, std::fs::read_to_string(&golden).unwrap(), "{rule}");
    }
}

/// A rule calling another (`fee_first` calls `fee`, issue #213) is proven with
/// the call expanded, and with a `hint` (#215).
#[test]
fn a_rule_calling_another_is_proven_through_the_call() {
    project_golden("invoice", &["fee_first", "payout"]);
}

/// List rules (#212): the core and the share item with its automatic hint.
#[test]
fn list_rules_are_proven_by_lean() {
    project_golden("any-length", &["split__core", "split__item2", "cart__core"]);
}

/// `repeat` (#214): the step keeps every promise.
#[test]
fn repeat_steps_are_proven_by_lean() {
    project_golden(
        "repeat",
        &["savings__start", "savings__step", "balance__step"],
    );
}

/// With `--lean`, a rule Lean cannot take fails the build (issue #211):
/// here a `hint` naming a lemma that does not exist.
#[test]
fn a_rule_lean_cannot_take_fails_the_build() {
    let dir = std::env::temp_dir().join(format!("axiom-lean-strict-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("f.ax"),
        "f(x)\n  need x <= 10\n  always -> x + 1\n  hint nope(x)\n",
    )
    .unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_axiom"))
        .args(["build", "f.ax", "-o", "f.wasm", "--lean"])
        .current_dir(&dir)
        .env("PATH", "")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(!out.status.success(), "{stdout}");
    assert!(
        stdout.contains("lean_result: NOT_COVERED (hint `nope`: no such lemma)"),
        "{stdout}"
    );
    assert!(String::from_utf8_lossy(&out.stderr).contains("Lean did not prove f"));
}
