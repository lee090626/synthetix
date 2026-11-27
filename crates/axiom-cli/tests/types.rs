//! Types (issue #226): the code emitted for JS, Python, Go and Rust and
//! `axiom run` give the answers written here by hand, and each language's
//! choice constants (`Size.large`) are the values' places.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

fn example() -> String {
    fs::read_to_string(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/types/rules.ax"),
    )
    .unwrap()
}

const SIZES: [&str; 3] = ["small", "medium", "large"];

fn shipping(size: u64, weight: u64) -> Option<u64> {
    if weight > 100_000 || size > 2 {
        return None;
    }
    Some([3000, 5000, 5000 + weight / 1000 * 100][size as usize])
}

fn subtotal(price: u64, quantity: u64, size: u64) -> Option<u64> {
    (price <= 10_000_000 && quantity <= 100 && size <= 2).then_some(price * quantity)
}

fn total(price: u64, quantity: u64, size: u64, weight: u64) -> Option<u64> {
    let goods = subtotal(price, quantity, size)?;
    if weight > 100_000 {
        return None;
    }
    Some(if goods >= 100_000 {
        goods
    } else {
        goods + shipping(size, weight)?
    })
}

fn wrap_fee(size: u64) -> Option<u64> {
    (size <= 2).then_some(if size == 2 { 2000 } else { 0 })
}

/// A size token: a value name (read through the emitted constants) or a number.
fn size_token(size: u64) -> String {
    SIZES
        .get(size as usize)
        .map_or(size.to_string(), |s| (*s).to_string())
}

/// `name a b c` lines and the answers expected for them.
fn cases() -> (String, Vec<String>) {
    let show = |answer: Option<u64>| answer.map_or("ERR".to_string(), |a| a.to_string());
    let mut input = String::new();
    let mut answers = Vec::new();
    for (size, weight) in [
        (0, 0),
        (1, 5000),
        (2, 0),
        (2, 100_000),
        (2, 12_345),
        (3, 1),
        (0, 100_001),
    ] {
        input += &format!("shipping {} {weight}\n", size_token(size));
        answers.push(show(shipping(size, weight)));
    }
    for (price, quantity, size, weight) in [
        (1000, 3, 0, 500),
        (1000, 3, 2, 70_000),
        (50_000, 2, 2, 100_000),
        (99_999, 1, 1, 0),
        (10_000_000, 100, 2, 100_000),
        (10_000_001, 1, 0, 0),
        (5, 101, 1, 0),
        (5, 5, 3, 0),
    ] {
        input += &format!("subtotal {price} {quantity} {}\n", size_token(size));
        answers.push(show(subtotal(price, quantity, size)));
        input += &format!("total {price} {quantity} {} {weight}\n", size_token(size));
        answers.push(show(total(price, quantity, size, weight)));
        input += &format!("wrap_fee {price} {quantity} {}\n", size_token(size));
        answers.push(show(wrap_fee(size)));
    }
    assert!(answers.iter().any(|a| a == "ERR"));
    assert!(answers.iter().any(|a| a == "15000"));
    (input, answers)
}

fn build(emit: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("axiom-types-{emit}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("rules.ax"), example()).unwrap();
    fs::write(
        dir.join("synthetix.toml"),
        "[project]\nentry = \"rules.ax\"\n",
    )
    .unwrap();
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

fn run(program: &str, args: &[&str], dir: &Path, input: &str) -> Vec<String> {
    let mut child = Command::new(program)
        .args(args)
        .current_dir(dir)
        .env("GOCACHE", dir.join("gocache"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
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
    let (input, answers) = cases();
    let dir = build("js");
    fs::write(
        dir.join("harness.mjs"),
        "import { readFileSync } from \"fs\";\nconst c = await import(process.argv[2] + \"/dist/contracts.js\");\nfor (const line of readFileSync(0, \"utf8\").trim().split(\"\\n\")) {\n  const [name, ...args] = line.split(\" \");\n  const values = args.map((a) => (a in c.Size ? c.Size[a] : Number(a)));\n  try { console.log(String(c[name](...values))); } catch (e) { console.log(\"ERR\"); }\n}\n",
    )
    .unwrap();
    let dir_arg = dir.display().to_string();
    assert_eq!(
        run("node", &["harness.mjs", &dir_arg], &dir, &input),
        answers
    );
    let dts = fs::read_to_string(dir.join("dist/contracts.d.ts")).unwrap();
    assert!(
        dts.contains("export declare const Size: { readonly small: 0; readonly medium: 1; readonly large: 2 };"),
        "{dts}"
    );
}

#[test]
fn python_matches_by_hand() {
    let (input, answers) = cases();
    let dir = build("python");
    let script = "import sys\nsys.path.insert(0, sys.argv[1])\nimport contracts as c\nfor line in sys.stdin:\n    name, *args = line.split()\n    values = [getattr(c.Size, a) if hasattr(c.Size, a) else int(a) for a in args]\n    try:\n        print(getattr(c, name)(*values))\n    except c.AxiomError:\n        print(\"ERR\")\n";
    let dist = dir.join("dist").display().to_string();
    assert_eq!(
        run("python3", &["-c", script, &dist], &dir, &input),
        answers
    );
}

#[test]
fn rust_matches_by_hand() {
    let (input, answers) = cases();
    let dir = build("rust");
    let main = "mod contracts;\nuse contracts::*;\nuse std::io::BufRead;\nfn show(r: Result<u32, AxiomError>) {\n    match r { Ok(v) => println!(\"{v}\"), Err(_) => println!(\"ERR\") }\n}\nfn value(a: &str) -> u32 {\n    match a { \"small\" => Size::small, \"medium\" => Size::medium, \"large\" => Size::large, n => n.parse().unwrap() }\n}\nfn main() {\n    for line in std::io::stdin().lock().lines() {\n        let line = line.unwrap();\n        let f: Vec<u32> = line.split(' ').skip(1).map(value).collect();\n        match line.split(' ').next().unwrap() {\n            \"shipping\" => show(shipping(f[0], f[1])),\n            \"subtotal\" => show(subtotal(f[0], f[1], f[2])),\n            \"total\" => show(total(f[0], f[1], f[2], f[3])),\n            _ => show(wrap_fee(f[0], f[1], f[2])),\n        }\n    }\n}\n";
    fs::copy(dir.join("dist/contracts.rs"), dir.join("contracts.rs")).unwrap();
    fs::write(dir.join("main.rs"), main).unwrap();
    let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".into());
    let out = Command::new(rustc)
        .args([
            "--edition",
            "2024",
            "-D",
            "warnings",
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
    assert_eq!(run(&main, &[], &dir, &input), answers);
}

#[test]
fn go_matches_by_hand() {
    let (input, answers) = cases();
    let dir = build("go");
    let main = "package main\n\nimport (\n\t\"bufio\"\n\t\"fmt\"\n\t\"os\"\n\t\"strconv\"\n\t\"strings\"\n\t\"t/contracts\"\n)\n\nvar sizes = map[string]uint32{\"small\": contracts.SizeSmall, \"medium\": contracts.SizeMedium, \"large\": contracts.SizeLarge}\n\nfunc main() {\n\tin := bufio.NewScanner(os.Stdin)\n\tfor in.Scan() {\n\t\tf := strings.Fields(in.Text())\n\t\tv := []uint32{}\n\t\tfor _, s := range f[1:] {\n\t\t\tif n, ok := sizes[s]; ok {\n\t\t\t\tv = append(v, n)\n\t\t\t\tcontinue\n\t\t\t}\n\t\t\tn, _ := strconv.ParseUint(s, 10, 32)\n\t\t\tv = append(v, uint32(n))\n\t\t}\n\t\tvar r uint32\n\t\tvar err error\n\t\tswitch f[0] {\n\t\tcase \"shipping\":\n\t\t\tr, err = contracts.Shipping(v[0], v[1])\n\t\tcase \"subtotal\":\n\t\t\tr, err = contracts.Subtotal(v[0], v[1], v[2])\n\t\tcase \"total\":\n\t\t\tr, err = contracts.Total(v[0], v[1], v[2], v[3])\n\t\tdefault:\n\t\t\tr, err = contracts.WrapFee(v[0], v[1], v[2])\n\t\t}\n\t\tif err != nil {\n\t\t\tfmt.Println(\"ERR\")\n\t\t} else {\n\t\t\tfmt.Println(r)\n\t\t}\n\t}\n}\n";
    fs::create_dir_all(dir.join("app/contracts")).unwrap();
    fs::copy(
        dir.join("dist/contracts.go"),
        dir.join("app/contracts/contracts.go"),
    )
    .unwrap();
    fs::write(dir.join("app/go.mod"), "module t\n\ngo 1.21\n").unwrap();
    fs::write(dir.join("app/main.go"), main).unwrap();
    let go = std::env::var("GO").unwrap_or_else(|_| "go".into());
    let app = dir.join("app");
    assert_eq!(run(&go, &["run", "."], &app, &input), answers);
}

#[test]
fn axiom_run_reads_value_names() {
    let dir = std::env::temp_dir().join(format!("axiom-types-run-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    let source = example();
    let rule = &source[..source.find("\n# 상품 금액").unwrap()];
    fs::write(dir.join("shipping.ax"), rule).unwrap();
    let axiom = |args: &[&str]| {
        let out = Command::new(env!("CARGO_BIN_EXE_axiom"))
            .args([&["run", "shipping.ax", "--"], args].concat())
            .current_dir(&dir)
            .output()
            .unwrap();
        (
            out.status.success(),
            format!(
                "{}{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            ),
        )
    };
    let (ok, out) = axiom(&["large", "12345"]);
    assert!(
        ok && out.contains(&format!("result: {}", shipping(2, 12_345).unwrap())),
        "{out}"
    );
    let (ok, out) = axiom(&["1", "0"]);
    assert!(ok && out.contains("result: 5000"), "{out}");
    let (ok, out) = axiom(&["huge", "0"]);
    assert!(
        !ok && out.contains("`huge` is not a value of `Size` (small, medium, large)"),
        "{out}"
    );
    let (ok, _) = axiom(&["3", "0"]);
    assert!(!ok);
}
