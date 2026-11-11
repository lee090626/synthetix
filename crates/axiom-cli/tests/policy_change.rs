//! Roadmap stage 5: for each of the three first-product tasks, change the spec by editing
//! only `.ax`, then break it and restore it (docs/first-product/requirements.md, docs/roadmap.md).
//! Contract results are checked by running the emitted `dist/contracts.js` in node;
//! skipped (loudly) when node is unavailable.

use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

const VERIFIED: [&str; 6] = [
    "app.wasm",
    "app.js",
    "index.html",
    "app.proof.json",
    "contracts.js",
    "contracts.d.ts",
];

/// A spec version: the contract source and the call the page's button makes.
struct Spec {
    policy: &'static str,
    name: &'static str,
    params: &'static [&'static str],
}

/// One task's change scenario. `checks` are `(js expression, expected output)` pairs.
struct Scenario {
    tag: &'static str,
    before: Spec,
    before_checks: &'static [(&'static str, &'static str)],
    after: Spec,
    after_checks: &'static [(&'static str, &'static str)],
    broken: Spec,
}

fn page(spec: &Spec) -> String {
    let states: String = spec
        .params
        .iter()
        .map(|p| format!("    state {p}: number = 0;\n    input {p} placeholder \"{p}\";\n"))
        .collect();
    format!(
        "import {{ {name} }} from \"./policy.ax\";\n\npage Policy {{\n{states}    state result: number = 0;\n    show result;\n    button \"적용\" updates result with {name}({args});\n}}\n",
        name = spec.name,
        args = spec.params.join(", ")
    )
}

fn write_spec(dir: &Path, spec: &Spec) {
    fs::write(dir.join("policy.ax"), spec.policy).unwrap();
    fs::write(dir.join("main.ax"), page(spec)).unwrap();
}

fn project(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("axiom-stage5-{}-{tag}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("synthetix.toml"),
        "[project]\nentry = \"main.ax\"\ntarget = \"web\"\n",
    )
    .unwrap();
    fs::write(dir.join("package.json"), "{\"type\":\"module\"}").unwrap();
    dir
}

/// Builds and returns (success, stderr).
fn build(dir: &Path) -> (bool, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_axiom"))
        .args(["build", "--emit", "js"])
        .current_dir(dir)
        .output()
        .unwrap();
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

fn build_ok(dir: &Path) {
    let (ok, stderr) = build(dir);
    assert!(ok, "{stderr}");
}

fn fingerprint(dir: &Path) -> String {
    let html = fs::read_to_string(dir.join("dist/index.html")).unwrap();
    let start = html.find("명세 <code>").unwrap() + "명세 <code>".len();
    let fp = html[start..][..html[start..].find("</code>").unwrap()].to_string();
    let proof = fs::read_to_string(dir.join("dist/app.proof.json")).unwrap();
    assert!(proof.contains(&format!("\"source_fingerprint\": \"{fp}\"")));
    fp
}

/// Evaluates every expression against the emitted module `m`; a throw becomes its class name.
fn eval(dir: &Path, exprs: &[&str]) -> Vec<String> {
    let body: String = exprs
        .iter()
        .map(|e| {
            format!("try{{console.log(String({e}))}}catch(e){{console.log(e.constructor.name)}}")
        })
        .collect();
    let script = format!(
        "const m=await import('{}');{body}",
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
    String::from_utf8(out.stdout)
        .unwrap()
        .lines()
        .map(String::from)
        .collect()
}

fn assert_checks(dir: &Path, tag: &str, step: &str, checks: &[(&str, &str)]) {
    let exprs: Vec<&str> = checks.iter().map(|(e, _)| *e).collect();
    for ((expr, want), got) in checks.iter().zip(eval(dir, &exprs)) {
        assert_eq!(got, *want, "{tag} {step}: {expr}");
    }
}

fn run(s: &Scenario) {
    let dir = project(s.tag);

    // 1. Original spec.
    write_spec(&dir, &s.before);
    build_ok(&dir);
    let before = fingerprint(&dir);
    assert_checks(&dir, s.tag, "before", s.before_checks);

    // 2. Requirement change in `.ax` only: new results and a new fingerprint.
    write_spec(&dir, &s.after);
    build_ok(&dir);
    let after = fingerprint(&dir);
    assert_ne!(before, after, "{}: fingerprint must change", s.tag);
    assert_checks(&dir, s.tag, "after", s.after_checks);

    // 3. Invalid spec: the build fails semantically and no verified output survives.
    write_spec(&dir, &s.broken);
    let (ok, stderr) = build(&dir);
    assert!(!ok, "{}: broken spec must fail", s.tag);
    assert!(stderr.contains("UNSATISFIABLE_"), "{}: {stderr}", s.tag);
    for file in VERIFIED {
        assert!(
            !dir.join("dist").join(file).exists(),
            "{}: stale dist/{file}",
            s.tag
        );
    }

    // 4. Restore: identical to a clean build of the same spec.
    write_spec(&dir, &s.after);
    build_ok(&dir);
    assert_eq!(fingerprint(&dir), after, "{}", s.tag);
    assert_checks(&dir, s.tag, "restored", s.after_checks);

    let clean = project(&format!("{}-clean", s.tag));
    write_spec(&clean, &s.after);
    build_ok(&clean);
    for file in VERIFIED {
        assert_eq!(
            fs::read(dir.join("dist").join(file)).unwrap(),
            fs::read(clean.join("dist").join(file)).unwrap(),
            "{}: dist/{file} differs from clean build",
            s.tag
        );
    }
    fs::remove_dir_all(dir).ok();
    fs::remove_dir_all(clean).ok();
}

fn node_missing() -> bool {
    let missing = Command::new("node").arg("--version").output().is_err();
    if missing {
        eprintln!("SKIPPED policy_change: node not found");
    }
    missing
}

/// T1: the base-amount condition changes (quantity limit 100 → 200).
#[test]
fn base_amount_condition_change() {
    if node_missing() {
        return;
    }
    run(&Scenario {
        tag: "t1",
        before: Spec {
            policy: "export contract base_amount(price: u32, quantity: u32) -> u32 {
    requires price <= 10000;
    requires quantity <= 100;
    ensures result == price *% quantity;
}
",
            name: "base_amount",
            params: &["price", "quantity"],
        },
        before_checks: &[
            ("m.base_amount(0, 100)", "0"),
            ("m.base_amount(1, 1)", "1"),
            ("m.base_amount(25, 4)", "100"),
            ("m.base_amount(10000, 100)", "1000000"),
            ("m.base_amount(10, 150)", "RangeError"),
        ],
        after: Spec {
            policy: "export contract base_amount(price: u32, quantity: u32) -> u32 {
    requires price <= 10000;
    requires quantity <= 200;
    ensures result == price *% quantity;
}
",
            name: "base_amount",
            params: &["price", "quantity"],
        },
        after_checks: &[
            ("m.base_amount(25, 4)", "100"),
            ("m.base_amount(10, 150)", "1500"),
            ("m.base_amount(10000, 200)", "2000000"),
            ("m.base_amount(10, 201)", "RangeError"),
        ],
        broken: Spec {
            policy: "export contract base_amount(price: u32, quantity: u32) -> u32 {
    requires quantity <= 200;
    requires quantity > 200;
    ensures result == price *% quantity;
}
",
            name: "base_amount",
            params: &["price", "quantity"],
        },
    });
}

/// T2 → C1: the cap-only policy becomes the floor/cap minimum-cost policy.
#[test]
fn cap_policy_to_floor_cap_change() {
    if node_missing() {
        return;
    }
    run(&Scenario {
        tag: "c1",
        before: Spec {
            policy: "export contract capped(total: u32, cap: u32) -> u32 {
    requires total <= 1000000;
    requires cap <= 1000000;
    ensures result <= total;
    ensures result <= cap;
    ensures result == total || result == cap;
}
",
            name: "capped",
            params: &["total", "cap"],
        },
        before_checks: &[
            ("m.capped(0, 80)", "0"),
            ("m.capped(100, 80)", "80"),
            ("m.capped(40, 80)", "40"),
            ("m.capped(80, 80)", "80"),
            ("m.capped(1000000, 0)", "0"),
        ],
        after: Spec {
            policy: "export contract floor_cap(total: u32, floor: u32, cap: u32) -> u32 {
    requires total <= 1000000;
    requires floor <= cap;
    requires cap <= 1000000;
    ensures total < floor || total > cap || result == total;
    ensures total >= floor || result == floor;
    ensures total <= cap || result == cap;
}
",
            name: "floor_cap",
            params: &["total", "floor", "cap"],
        },
        after_checks: &[
            ("m.floor_cap(0, 20, 80)", "20"),
            ("m.floor_cap(40, 20, 80)", "40"),
            ("m.floor_cap(100, 20, 80)", "80"),
            ("m.floor_cap(20, 20, 80)", "20"),
            ("m.floor_cap(80, 20, 80)", "80"),
            ("m.floor_cap(99, 50, 50)", "50"),
            ("m.floor_cap(10, 81, 80)", "RangeError"),
            ("typeof m.capped", "undefined"),
        ],
        broken: Spec {
            // No u32 can be both above and below `total`.
            policy: "export contract floor_cap(total: u32, floor: u32, cap: u32) -> u32 {
    requires total <= 1000000;
    ensures result > total;
    ensures result < total;
}
",
            name: "floor_cap",
            params: &["total", "floor", "cap"],
        },
    });
}

/// T3: the item limit changes (each of the four items 250,000 → 100,000).
#[test]
fn items_limit_change() {
    if node_missing() {
        return;
    }
    run(&Scenario {
        tag: "t3",
        before: Spec {
            policy: "export contract items_total(a: u32, b: u32, c: u32, d: u32) -> u32 {
    requires a <= 250000;
    requires b <= 250000;
    requires c <= 250000;
    requires d <= 250000;
    ensures result == a +% b +% c +% d;
}
",
            name: "items_total",
            params: &["a", "b", "c", "d"],
        },
        before_checks: &[
            ("m.items_total(0, 0, 0, 0)", "0"),
            ("m.items_total(10, 20, 30, 40)", "100"),
            ("m.items_total(250000, 250000, 250000, 250000)", "1000000"),
            ("m.items_total(200000, 0, 0, 0)", "200000"),
        ],
        after: Spec {
            policy: "export contract items_total(a: u32, b: u32, c: u32, d: u32) -> u32 {
    requires a <= 100000;
    requires b <= 100000;
    requires c <= 100000;
    requires d <= 100000;
    ensures result == a +% b +% c +% d;
}
",
            name: "items_total",
            params: &["a", "b", "c", "d"],
        },
        after_checks: &[
            ("m.items_total(10, 20, 30, 40)", "100"),
            ("m.items_total(100000, 100000, 100000, 100000)", "400000"),
            ("m.items_total(200000, 0, 0, 0)", "RangeError"),
        ],
        broken: Spec {
            policy: "export contract items_total(a: u32, b: u32, c: u32, d: u32) -> u32 {
    requires a <= 100000;
    ensures result == a +% b +% c +% d;
    ensures result != a +% b +% c +% d;
}
",
            name: "items_total",
            params: &["a", "b", "c", "d"],
        },
    });
}
