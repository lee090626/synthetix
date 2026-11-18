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

    let page =
        format!("{BIG}\npage P\n  a, b, t = 0\n  when \"Go\" is clicked\n    t = total(a, b)\n");
    let (ok, out) = axiom(&project("page", &page), &["build"]);
    assert!(!ok && out.contains("pages cannot use yet"), "{out}");
}

// Inputs near the 32-bit and 64-bit edges.
const ROWS: [[i64; 2]; 4] = [
    [1_000_000_000_000, 1000],
    [4_294_967_296, 3],
    [0, 1_000_000_000_000],
    [999_999_999_999, 7],
];

#[test]
fn rust_and_js_output_use_64_bit_words() {
    let dir = project("emit", BIG);
    let (ok, out) = axiom(&dir, &["build", "--emit", "rust"]);
    assert!(ok, "{out}");
    let bin = dir.join("bin");
    fs::create_dir_all(&bin).unwrap();
    fs::copy(dir.join("dist/contracts.rs"), bin.join("contracts.rs")).unwrap();
    let rows: Vec<String> = ROWS.iter().map(|[a, b]| format!("({a}, {b})")).collect();
    fs::write(
        bin.join("main.rs"),
        format!(
            "mod contracts;\nfn main() {{ for (a, b) in [{}] {{ println!(\"{{:?}} {{:?}}\", contracts::total(a as u64, b as u64), contracts::drift(a, b)); }} }}\n",
            rows.join(", ")
        ),
    )
    .unwrap();
    let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".into());
    let out = Command::new(rustc)
        .args(["--edition", "2024", "main.rs", "-o", "main"])
        .current_dir(&bin)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let got = String::from_utf8_lossy(&Command::new(bin.join("main")).output().unwrap().stdout)
        .to_string();
    let expected: String = ROWS
        .iter()
        .map(|[a, b]| {
            let total = if *b <= 1000 {
                format!("Ok({})", a * b)
            } else {
                "Err(AxiomError(\"total: input violates a requires clause\"))".to_string()
            };
            format!("{total} Ok({})\n", a - b)
        })
        .collect();
    assert_eq!(got, expected);

    if Command::new("node").arg("--version").output().is_err() {
        eprintln!("SKIPPED big_numbers js: node not found");
        return;
    }
    let (ok, out) = axiom(&dir, &["build", "--emit", "js"]);
    assert!(ok, "{out}");
    let script = format!(
        "import('{}').then(m => {{ for (const [a, b] of [{}]) {{ let t; try {{ t = String(m.total(BigInt(a), BigInt(b))); }} catch (e) {{ t = 'ERR'; }} console.log(t, String(m.drift(BigInt(a), BigInt(b)))); }} }})",
        dir.join("dist/contracts.js").display(),
        ROWS.iter()
            .map(|[a, b]| format!("['{a}', '{b}']"))
            .collect::<Vec<_>>()
            .join(", ")
    );
    let out = Command::new("node").args(["-e", &script]).output().unwrap();
    let got = String::from_utf8_lossy(&out.stdout).to_string();
    let expected: String = ROWS
        .iter()
        .map(|[a, b]| {
            let total = if *b <= 1000 {
                (a * b).to_string()
            } else {
                "ERR".to_string()
            };
            format!("{total} {}\n", a - b)
        })
        .collect();
    assert_eq!(got, expected, "{}", String::from_utf8_lossy(&out.stderr));
}
