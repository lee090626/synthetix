//! `repeat` rules (issue #214): the code emitted for JS, Python, Go and Rust
//! and `axiom run` give the answers of a loop written here by hand.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

fn example() -> String {
    fs::read_to_string(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/repeat/rules.ax"),
    )
    .unwrap()
}

fn savings(principal: u64, rate: u64, years: u64) -> String {
    if years > 100 || principal > 100_000_000 || rate > 20 {
        return "ERR".into();
    }
    let mut value = principal;
    for _ in 0..years {
        value = if value * rate / 100 > 100_000_000 - value {
            100_000_000
        } else {
            value + value * rate / 100
        };
    }
    value.to_string()
}

fn balance(loan: u64, interest: u64, payment: u64, months: u64) -> String {
    if months > 600
        || loan > 1_000_000_000
        || interest > 1_000_000
        || payment < interest
        || payment > 1_000_000_000
    {
        return "ERR".into();
    }
    let mut value = loan;
    for _ in 0..months {
        value = (value + interest).saturating_sub(payment);
    }
    value.to_string()
}

/// `name a b c` lines and the answers expected for them.
fn cases() -> (String, Vec<String>) {
    let mut input = String::new();
    let mut answers = Vec::new();
    for (p, r, y) in [
        (1000, 10, 2),
        (90_000_000, 20, 5),
        (5, 0, 100),
        (100_000_000, 20, 100),
        (1, 1, 101),
        (0, 20, 30),
        (777, 7, 0),
    ] {
        input += &format!("savings {p} {r} {y}\n");
        answers.push(savings(p, r, y));
    }
    for (l, i, p, m) in [
        (1000, 10, 300, 3),
        (100, 5, 200, 2),
        (1_000_000_000, 1_000_000, 1_000_000, 600),
        (500, 50, 40, 1),
        (10, 0, 0, 601),
    ] {
        input += &format!("balance {l} {i} {p} {m}\n");
        answers.push(balance(l, i, p, m));
    }
    assert!(answers.iter().any(|a| a == "ERR"));
    assert!(answers.iter().any(|a| a == "100000000"));
    (input, answers)
}

fn build(emit: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("axiom-repeat-{emit}-{}", std::process::id()));
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
fn js_matches_the_loop() {
    let (input, answers) = cases();
    let dir = build("js");
    fs::write(
        dir.join("harness.mjs"),
        "import { readFileSync } from \"fs\";\nconst c = await import(process.argv[2] + \"/dist/contracts.js\");\nfor (const line of readFileSync(0, \"utf8\").trim().split(\"\\n\")) {\n  const [name, ...args] = line.split(\" \");\n  try { console.log(String(c[name](...args.map(Number)))); } catch (e) { console.log(\"ERR\"); }\n}\n",
    )
    .unwrap();
    let dir_arg = dir.display().to_string();
    assert_eq!(
        run("node", &["harness.mjs", &dir_arg], &dir, &input),
        answers
    );
}

#[test]
fn python_matches_the_loop() {
    let (input, answers) = cases();
    let dir = build("python");
    let script = "import sys\nsys.path.insert(0, sys.argv[1])\nimport contracts as c\nfor line in sys.stdin:\n    name, *args = line.split()\n    try:\n        print(getattr(c, name)(*map(int, args)))\n    except c.AxiomError:\n        print(\"ERR\")\n";
    let dist = dir.join("dist").display().to_string();
    assert_eq!(
        run("python3", &["-c", script, &dist], &dir, &input),
        answers
    );
}

#[test]
fn rust_matches_the_loop() {
    let (input, answers) = cases();
    let dir = build("rust");
    let main = "mod contracts;\nuse contracts::*;\nuse std::io::BufRead;\nfn show(r: Result<u32, AxiomError>) {\n    match r { Ok(v) => println!(\"{v}\"), Err(_) => println!(\"ERR\") }\n}\nfn main() {\n    for line in std::io::stdin().lock().lines() {\n        let line = line.unwrap();\n        let f: Vec<u32> = line.split(' ').skip(1).map(|v| v.parse().unwrap()).collect();\n        if line.starts_with(\"savings\") { show(savings(f[0], f[1], f[2])) } else { show(balance(f[0], f[1], f[2], f[3])) }\n    }\n}\n";
    fs::copy(dir.join("dist/contracts.rs"), dir.join("contracts.rs")).unwrap();
    fs::write(dir.join("main.rs"), main).unwrap();
    let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".into());
    let out = Command::new(rustc)
        .args(["--edition", "2024", "main.rs", "-o", "main"])
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
fn go_matches_the_loop() {
    let (input, answers) = cases();
    let dir = build("go");
    let main = "package main\n\nimport (\n\t\"bufio\"\n\t\"fmt\"\n\t\"os\"\n\t\"strconv\"\n\t\"strings\"\n\t\"t/contracts\"\n)\n\nfunc main() {\n\tin := bufio.NewScanner(os.Stdin)\n\tfor in.Scan() {\n\t\tf := strings.Fields(in.Text())\n\t\tv := []uint32{}\n\t\tfor _, s := range f[1:] {\n\t\t\tn, _ := strconv.ParseUint(s, 10, 32)\n\t\t\tv = append(v, uint32(n))\n\t\t}\n\t\tvar r uint32\n\t\tvar err error\n\t\tif f[0] == \"savings\" {\n\t\t\tr, err = contracts.Savings(v[0], v[1], v[2])\n\t\t} else {\n\t\t\tr, err = contracts.Balance(v[0], v[1], v[2], v[3])\n\t\t}\n\t\tif err != nil {\n\t\t\tfmt.Println(\"ERR\")\n\t\t} else {\n\t\t\tfmt.Println(r)\n\t\t}\n\t}\n}\n";
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
fn axiom_run_repeats_the_step() {
    let dir = std::env::temp_dir().join(format!("axiom-repeat-run-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    let source = example();
    let savings_rule = &source[..source.find("\n# 대출").unwrap()];
    fs::write(dir.join("savings.ax"), savings_rule).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_axiom"))
        .args(["run", "savings.ax", "--", "1000", "10", "2"])
        .current_dir(&dir)
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "{stdout}{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        stdout.contains(&format!("result: {}", savings(1000, 10, 2))),
        "{stdout}"
    );
}
