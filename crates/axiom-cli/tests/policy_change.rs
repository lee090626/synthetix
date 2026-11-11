//! C1 policy change (docs/first-product/requirements.md): replace the cap-only policy
//! with the floor/cap policy by editing only `.ax` specs, then break and restore the spec.
//! Contract results are checked by running the emitted `dist/contracts.js` in node;
//! skipped (loudly) when node is unavailable.

use std::{fs, path::Path, process::Command};

const CAPPED: &str = r#"export contract capped(total: u32, cap: u32) -> u32 {
    requires total <= 1000000;
    requires cap <= 1000000;
    ensures result <= total;
    ensures result <= cap;
    ensures result == total || result == cap;
}
"#;

const FLOOR_CAP: &str = r#"export contract floor_cap(total: u32, floor: u32, cap: u32) -> u32 {
    requires total <= 1000000;
    requires floor <= cap;
    requires cap <= 1000000;
    ensures total < floor || total > cap || result == total;
    ensures total >= floor || result == floor;
    ensures total <= cap || result == cap;
}
"#;

// No u32 can be both above and below `total`.
const BROKEN: &str = r#"export contract floor_cap(total: u32, floor: u32, cap: u32) -> u32 {
    requires total <= 1000000;
    ensures result > total;
    ensures result < total;
}
"#;

fn page(call: &str, name: &str) -> String {
    format!(
        r#"import {{ {name} }} from "./policy.ax";

page Policy {{
    state total: number = 0;
    state floor: number = 0;
    state cap: number = 0;
    state result: number = 0;

    input total placeholder "합계";
    input floor placeholder "하한";
    input cap placeholder "상한";

    show result;
    button "정책 적용" updates result with {call};
}}
"#
    )
}

fn write_spec(dir: &Path, policy: &str, name: &str, call: &str) {
    fs::write(dir.join("policy.ax"), policy).unwrap();
    fs::write(dir.join("main.ax"), page(call, name)).unwrap();
}

fn project(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("axiom-c1-{}-{tag}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("synthetix.toml"),
        "[project]\nentry = \"main.ax\"\ntarget = \"web\"\n",
    )
    .unwrap();
    dir
}

fn build(dir: &Path) -> bool {
    Command::new(env!("CARGO_BIN_EXE_axiom"))
        .args(["build", "--emit", "js"])
        .current_dir(dir)
        .output()
        .unwrap()
        .status
        .success()
}

fn fingerprint(dir: &Path) -> String {
    let html = fs::read_to_string(dir.join("dist/index.html")).unwrap();
    let start = html.find("명세 <code>").unwrap() + "명세 <code>".len();
    let fp = html[start..][..html[start..].find("</code>").unwrap()].to_string();
    let proof = fs::read_to_string(dir.join("dist/app.proof.json")).unwrap();
    assert!(proof.contains(&format!("\"source_fingerprint\": \"{fp}\"")));
    fp
}

/// Evaluates `expr` against the emitted module `m`; throws become the error class name.
fn eval(dir: &Path, expr: &str) -> String {
    let script = format!(
        "import('{}').then(m=>{{try{{console.log(String({expr}))}}catch(e){{console.log(e.constructor.name)}}}})",
        dir.join("dist/contracts.js").display()
    );
    let out = Command::new("node")
        .args(["--input-type=module", "-e", &script])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

#[test]
fn policy_change_resynthesizes_and_recovers() {
    if Command::new("node").arg("--version").output().is_err() {
        eprintln!("SKIPPED policy_change: node not found");
        return;
    }
    let dir = project("change");
    fs::write(dir.join("package.json"), "{\"type\":\"module\"}").unwrap();

    // 1. Cap-only policy.
    write_spec(&dir, CAPPED, "capped", "capped(total, cap)");
    assert!(build(&dir));
    let before = fingerprint(&dir);
    assert_eq!(eval(&dir, "m.capped(10, 80)"), "10");
    assert_eq!(eval(&dir, "m.capped(100, 80)"), "80");

    // 2. Requirement changes: same inputs, minimum-cost policy, new fingerprint.
    write_spec(&dir, FLOOR_CAP, "floor_cap", "floor_cap(total, floor, cap)");
    assert!(build(&dir));
    let after = fingerprint(&dir);
    assert_ne!(before, after);
    assert_eq!(eval(&dir, "m.floor_cap(10, 20, 80)"), "20");
    assert_eq!(eval(&dir, "m.floor_cap(100, 20, 80)"), "80");
    assert_eq!(eval(&dir, "typeof m.capped"), "undefined");

    // 3. Unsatisfiable spec: build fails and no verified output survives.
    write_spec(&dir, BROKEN, "floor_cap", "floor_cap(total, floor, cap)");
    assert!(!build(&dir));
    for file in [
        "app.wasm",
        "app.js",
        "index.html",
        "app.proof.json",
        "contracts.js",
        "contracts.d.ts",
    ] {
        assert!(!dir.join("dist").join(file).exists(), "stale dist/{file}");
    }

    // 4. Restore: identical to a clean build of the same spec.
    write_spec(&dir, FLOOR_CAP, "floor_cap", "floor_cap(total, floor, cap)");
    assert!(build(&dir));
    assert_eq!(fingerprint(&dir), after);
    assert_eq!(eval(&dir, "m.floor_cap(10, 20, 80)"), "20");

    let clean = project("clean");
    write_spec(
        &clean,
        FLOOR_CAP,
        "floor_cap",
        "floor_cap(total, floor, cap)",
    );
    assert!(build(&clean));
    for file in [
        "index.html",
        "app.js",
        "app.wasm",
        "app.proof.json",
        "contracts.js",
        "contracts.d.ts",
    ] {
        assert_eq!(
            fs::read(dir.join("dist").join(file)).unwrap(),
            fs::read(clean.join("dist").join(file)).unwrap(),
            "dist/{file} differs from clean build"
        );
    }
    fs::remove_dir_all(dir).ok();
    fs::remove_dir_all(clean).ok();
}
