//! Records and choices beyond #226 (issue #225): a rule answering a record,
//! a record inside a record, a choice answer and a list of choices. The code
//! emitted for JS, Python, Rust and Go gives the answers written here by hand.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// What every harness prints, one line per call.
const EXPECTED: &[&str] = &[
    "12000 1200 1", // quote(1000, 12)
    "15 1 0",       // quote(5, 3)
    "ERR",          // quote(10001, 1)
    "13200",        // pay(1000, 12)
    "1",            // box_for(6000)
    "0",            // box_for(10)
    "4800",         // order_total(500, 3, large)
    "2",            // large_boxes([small, large, large])
];

fn build(emit: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("axiom-records-{emit}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let example = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/records");
    fs::copy(example.join("rules.ax"), dir.join("rules.ax")).unwrap();
    fs::copy(example.join("synthetix.toml"), dir.join("synthetix.toml")).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_axiom"))
        .args(["build", "--emit", emit])
        .current_dir(&dir)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    dir
}

fn run(program: &str, args: &[&str], dir: &Path) -> Vec<String> {
    let out = Command::new(program)
        .args(args)
        .current_dir(dir)
        .env("GOCACHE", dir.join("gocache"))
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(String::from)
        .collect()
}

#[test]
fn js_matches_by_hand() {
    let dir = build("js");
    fs::write(
        dir.join("harness.mjs"),
        "const c = await import(process.argv[2] + \"/dist/contracts.js\");\nconst q = (p, n) => { try { const b = c.quote(p, n); console.log(`${b.goods} ${b.tax} ${b.size}`); } catch { console.log(\"ERR\"); } };\nq(1000, 12); q(5, 3); q(10001, 1);\nconsole.log(c.pay(1000, 12));\nconsole.log(c.box_for(6000)); console.log(c.box_for(10));\nconsole.log(c.order_total(500, 3, c.Size.large));\nconsole.log(c.large_boxes([c.Size.small, c.Size.large, c.Size.large]));\n",
    )
    .unwrap();
    let dir_arg = dir.display().to_string();
    assert_eq!(run("node", &["harness.mjs", &dir_arg], &dir), EXPECTED);
    let dts = fs::read_to_string(dir.join("dist/contracts.d.ts")).unwrap();
    assert!(
        dts.contains("export function quote(p0: number, p1: number): { goods: number; tax: number; size: number };"),
        "{dts}"
    );
}

#[test]
fn python_matches_by_hand() {
    let dir = build("python");
    let script = "import sys\nsys.path.insert(0, sys.argv[1])\nimport contracts as c\ndef q(p, n):\n    try:\n        b = c.quote(p, n)\n        print(b['goods'], b['tax'], b['size'])\n    except c.AxiomError:\n        print('ERR')\nq(1000, 12); q(5, 3); q(10001, 1)\nprint(c.pay(1000, 12))\nprint(c.box_for(6000)); print(c.box_for(10))\nprint(c.order_total(500, 3, c.Size.large))\nprint(c.large_boxes([c.Size.small, c.Size.large, c.Size.large]))\n";
    let dist = dir.join("dist").display().to_string();
    assert_eq!(run("python3", &["-c", script, &dist], &dir), EXPECTED);
}

#[test]
fn rust_matches_by_hand() {
    let dir = build("rust");
    let main = "mod contracts;\nuse contracts::*;\nfn q(p: u32, n: u32) {\n    match quote(p, n) { Ok(b) => println!(\"{} {} {}\", b.goods, b.tax, b.size), Err(_) => println!(\"ERR\") }\n}\nfn main() {\n    q(1000, 12); q(5, 3); q(10001, 1);\n    println!(\"{}\", pay(1000, 12).unwrap());\n    println!(\"{}\", box_for(6000).unwrap()); println!(\"{}\", box_for(10).unwrap());\n    println!(\"{}\", order_total(500, 3, Size::large).unwrap());\n    println!(\"{}\", large_boxes(&[Size::small, Size::large, Size::large]).unwrap());\n}\n";
    fs::copy(dir.join("dist/contracts.rs"), dir.join("contracts.rs")).unwrap();
    fs::write(dir.join("main.rs"), main).unwrap();
    let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".into());
    let out = Command::new(rustc)
        // Code for lists of any length already compares `n >= 0` and leaves a
        // loop index unused (`examples/any-length` too); only those are allowed.
        .args([
            "--edition",
            "2024",
            "-D",
            "warnings",
            "-A",
            "unused_comparisons",
            "-A",
            "unused_variables",
            "main.rs",
            "-o",
            "main",
        ])
        .current_dir(&dir)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let main = dir.join("main").display().to_string();
    assert_eq!(run(&main, &[], &dir), EXPECTED);
}

#[test]
fn go_matches_by_hand() {
    let dir = build("go");
    let main = "package main\n\nimport (\n\t\"fmt\"\n\t\"t/contracts\"\n)\n\nfunc q(p, n uint32) {\n\tb, err := contracts.Quote(p, n)\n\tif err != nil {\n\t\tfmt.Println(\"ERR\")\n\t\treturn\n\t}\n\tfmt.Println(b.Goods, b.Tax, b.Size)\n}\n\nfunc must(v uint32, err error) uint32 {\n\tif err != nil {\n\t\tpanic(err)\n\t}\n\treturn v\n}\n\nfunc main() {\n\tq(1000, 12)\n\tq(5, 3)\n\tq(10001, 1)\n\tfmt.Println(must(contracts.Pay(1000, 12)))\n\tfmt.Println(must(contracts.BoxFor(6000)))\n\tfmt.Println(must(contracts.BoxFor(10)))\n\tfmt.Println(must(contracts.OrderTotal(500, 3, contracts.SizeLarge)))\n\tfmt.Println(must(contracts.LargeBoxes([]uint32{contracts.SizeSmall, contracts.SizeLarge, contracts.SizeLarge})))\n}\n";
    fs::create_dir_all(dir.join("app/contracts")).unwrap();
    fs::copy(
        dir.join("dist/contracts.go"),
        dir.join("app/contracts/contracts.go"),
    )
    .unwrap();
    fs::write(dir.join("app/go.mod"), "module t\n\ngo 1.21\n").unwrap();
    fs::write(dir.join("app/main.go"), main).unwrap();
    let go = std::env::var("GO").unwrap_or_else(|_| "go".into());
    assert_eq!(run(&go, &["run", "."], &dir.join("app")), EXPECTED);
}
