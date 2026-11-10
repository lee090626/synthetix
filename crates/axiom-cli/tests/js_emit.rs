//! Differential check of `axiom build --emit js` against an independent Rust oracle.
//! Requires `node`; the test is skipped (loudly) when it is unavailable.

use std::{fs, path::PathBuf, process::Command};

const MAX: u64 = 1_000_000;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/first-product")
        .join(format!("{name}.ax"))
}

fn node_available() -> bool {
    Command::new("node").arg("--version").output().is_ok()
}

fn emit(name: &str, dir: &PathBuf) -> PathBuf {
    let wasm = dir.join(format!("{name}.wasm"));
    let out = Command::new(env!("CARGO_BIN_EXE_axiom"))
        .args([
            "build",
            fixture(name).to_str().unwrap(),
            "-o",
            wasm.to_str().unwrap(),
            "--emit",
            "js",
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(dir.join(format!("{name}.d.ts")).exists());
    dir.join(format!("{name}.js"))
}

/// Runs `fn(args)` for every row in node; each result is a number or "ERR:<name>".
fn run_node(js: &PathBuf, name: &str, rows: &[Vec<u64>]) -> Vec<String> {
    let script = format!(
        "import('{}').then(m=>{{const rows={};for(const r of rows){{try{{console.log(String(m.{name}(...r)))}}catch(e){{console.log('ERR:'+e.constructor.name)}}}}}})",
        js.display(),
        serde_json::to_string(rows).unwrap()
    );
    let out = Command::new("node").args(["-e", &script]).output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout)
        .unwrap()
        .lines()
        .map(String::from)
        .collect()
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("axiom-js-emit-{}-{name}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    // ESM needs a package boundary for .js files.
    fs::write(dir.join("package.json"), "{\"type\":\"module\"}").unwrap();
    dir
}

fn check(name: &str, valid: &[Vec<u64>], oracle: impl Fn(&[u64]) -> u64, rejected: &[Vec<u64>]) {
    if !node_available() {
        eprintln!("SKIPPED js_emit::{name}: node not found");
        return;
    }
    let dir = scratch(name);
    let js = emit(name, &dir);
    let rows: Vec<Vec<u64>> = valid.iter().chain(rejected).cloned().collect();
    let got = run_node(&js, name, &rows);
    for (row, actual) in valid.iter().zip(&got) {
        assert_eq!(actual, &oracle(row).to_string(), "{name}{row:?}");
    }
    for (row, actual) in rejected.iter().zip(&got[valid.len()..]) {
        assert!(
            actual.starts_with("ERR:"),
            "{name}{row:?} must be rejected, got {actual}"
        );
    }
    fs::remove_dir_all(dir).ok();
}

#[test]
fn capped_matches_oracle() {
    let edges = [0, 1, 79, 80, 81, 100, MAX - 1, MAX];
    let valid: Vec<Vec<u64>> = edges
        .iter()
        .flat_map(|&a| edges.iter().map(move |&b| vec![a, b]))
        .collect();
    check(
        "capped",
        &valid,
        |r| r[0].min(r[1]),
        &[
            vec![MAX + 1, 0],
            vec![0, MAX + 1],
            vec![u32::MAX as u64, 0],
            vec![4_294_967_296, 0],
            vec![0],
        ],
    );
}

#[test]
fn floor_cap_matches_oracle() {
    let edges = [0, 1, 20, 80, 99, MAX];
    let mut valid = Vec::new();
    for &t in &edges {
        for &f in &edges {
            for &c in &edges {
                if f <= c {
                    valid.push(vec![t, f, c]);
                }
            }
        }
    }
    check(
        "floor_cap",
        &valid,
        |r| r[0].clamp(r[1], r[2]),
        &[vec![0, 81, 80], vec![MAX + 1, 0, 0], vec![0, 0, MAX + 1]],
    );
}

#[test]
fn project_build_emits_all_contracts() {
    if !node_available() {
        eprintln!("SKIPPED js_emit::project: node not found");
        return;
    }
    let dir = scratch("project");
    let example =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/policy-calculator");
    for file in ["synthetix.toml", "main.ax", "policy.ax"] {
        fs::copy(example.join(file), dir.join(file)).unwrap();
    }
    let build = |args: &[&str]| {
        let out = Command::new(env!("CARGO_BIN_EXE_axiom"))
            .arg("build")
            .args(args)
            .current_dir(&dir)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    };
    build(&["--emit", "js"]);
    let js = dir.join("dist/contracts.js");
    let dts = fs::read_to_string(dir.join("dist/contracts.d.ts")).unwrap();
    assert!(dts.contains("export function capped(") && dts.contains("export function floor_cap("));
    let script = format!(
        "import('{}').then(m=>{{console.log(m.capped(100,80),m.floor_cap(10,20,80),m.floor_cap(100,20,80));try{{m.capped(1000001,0)}}catch(e){{console.log(e.constructor.name)}}}})",
        js.display()
    );
    let out = Command::new("node").args(["-e", &script]).output().unwrap();
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "80 20 80\nRangeError\n"
    );

    build(&[]);
    assert!(!js.exists(), "plain build must remove stale contracts.js");
    fs::remove_dir_all(dir).ok();
}
