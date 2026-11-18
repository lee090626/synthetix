//! `with big numbers`: 64-bit rules are proven with 64-bit words, and are
//! refused where output is still 32-bit.

use std::{fs, path::Path, process::Command};

fn project(name: &str, rules: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("axiom-big-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("rules.ax"), rules).unwrap();
    fs::write(
        dir.join("synthetix.toml"),
        "[project]\nentry = \"rules.ax\"\n",
    )
    .unwrap();
    dir
}

fn axiom(dir: &Path, args: &[&str]) -> (bool, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_axiom"))
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    let text =
        String::from_utf8_lossy(&out.stdout).to_string() + &String::from_utf8_lossy(&out.stderr);
    (out.status.success(), text)
}

// 1조 won times 1000 does not fit in 32 bits but fits in 64.
const BIG: &str = "\
total(price, quantity) with big numbers
  need price <= 1000000000000
  need quantity <= 1000
  always -> price * quantity

drift(now, before) can be negative with big numbers
  need now >= 0 and now <= 1000000000000
  need before >= 0 and before <= 1000000000000
  always -> now - before
  promise answer >= -1000000000000
";

#[test]
fn big_rules_are_proven_with_64_bit_words() {
    let dir = project("ok", BIG);
    let (ok, out) = axiom(&dir, &["check"]);
    assert!(ok, "{out}");
    assert!(out.contains("verified: total"), "{out}");
    assert!(out.contains("verified: drift"), "{out}");
}

#[test]
fn big_rules_still_check_ranges_and_promises() {
    let over = "f(a, b) with big numbers\n  need a <= 10000000000\n  need b <= 10000000000\n  always -> a * b\n";
    let (ok, out) = axiom(&project("over", over), &["check"]);
    assert!(
        !ok && out.contains("`*` may go outside 0 to 18446744073709551615"),
        "{out}"
    );

    let broken = "f(a) with big numbers\n  need a <= 10000000000\n  always -> a + 1\n  promise answer <= 10000000000\n";
    let (ok, out) = axiom(&project("promise", broken), &["check"]);
    assert!(!ok && out.contains("a=10000000000"), "{out}");
}

#[test]
fn big_numbers_need_the_name_line_and_matching_callers() {
    let (ok, out) = axiom(
        &project("literal", "f(a)\n  need a <= 10000000000\n  always -> a\n"),
        &["check"],
    );
    assert!(!ok && out.contains("with big numbers"), "{out}");

    let mixed = "g(a) with big numbers\n  need a <= 10\n  always -> a\n\nf(a)\n  need a <= 10\n  always -> g(a)\n";
    let (ok, out) = axiom(&project("mixed", mixed), &["check"]);
    assert!(
        !ok && out.contains("must both use `with big numbers` or neither"),
        "{out}"
    );

    let (ok, out) = axiom(&project("build", BIG), &["build"]);
    assert!(!ok && out.contains("not built yet"), "{out}");
}
