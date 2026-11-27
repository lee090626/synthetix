//! Rules that call themselves (issue #225, M2): the code emitted for JS,
//! Python, Go and Rust and `axiom run` give the answers written here by hand.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

fn example() -> String {
    fs::read_to_string(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/recursion/rules.ax"),
    )
    .unwrap()
}

fn triangle(n: u64) -> String {
    if n > 1000 {
        return "ERR".into();
    }
    (1..=n).sum::<u64>().to_string()
}

fn stairs(fee: u64, n: u64) -> String {
    if n > 100 || fee > 10_000 {
        return "ERR".into();
    }
    (1..=n)
        .map(|k| if k > 10 { fee / 2 } else { fee })
        .sum::<u64>()
        .to_string()
}

/// `name a b` lines and the answers expected for them.
fn cases() -> (String, Vec<String>) {
    let mut input = String::new();
    let mut answers = Vec::new();
    for n in [0, 1, 4, 100, 1000, 1001] {
        input += &format!("triangle {n}\n");
        answers.push(triangle(n));
    }
    for (fee, n) in [
        (1000, 0),
        (1000, 10),
        (1001, 11),
        (10_000, 100),
        (7, 12),
        (10_001, 1),
        (5, 101),
    ] {
        input += &format!("stairs {fee} {n}\n");
        answers.push(stairs(fee, n));
    }
    assert!(answers.iter().any(|a| a == "ERR"));
    assert!(answers.iter().any(|a| a == "500500"));
    (input, answers)
}

fn build(emit: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("axiom-recursion-{emit}-{}", std::process::id()));
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
fn js_matches_the_sum() {
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
fn python_matches_the_sum() {
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
fn rust_matches_the_sum() {
    let (input, answers) = cases();
    let dir = build("rust");
    let main = "mod contracts;\nuse contracts::*;\nuse std::io::BufRead;\nfn show(r: Result<u32, AxiomError>) {\n    match r { Ok(v) => println!(\"{v}\"), Err(_) => println!(\"ERR\") }\n}\nfn main() {\n    for line in std::io::stdin().lock().lines() {\n        let line = line.unwrap();\n        let f: Vec<u32> = line.split(' ').skip(1).map(|v| v.parse().unwrap()).collect();\n        if line.starts_with(\"triangle\") { show(triangle(f[0])) } else { show(stairs(f[0], f[1])) }\n    }\n}\n";
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
fn go_matches_the_sum() {
    let (input, answers) = cases();
    let dir = build("go");
    let main = "package main\n\nimport (\n\t\"bufio\"\n\t\"fmt\"\n\t\"os\"\n\t\"strconv\"\n\t\"strings\"\n\t\"t/contracts\"\n)\n\nfunc main() {\n\tin := bufio.NewScanner(os.Stdin)\n\tfor in.Scan() {\n\t\tf := strings.Fields(in.Text())\n\t\tv := []uint32{}\n\t\tfor _, s := range f[1:] {\n\t\t\tn, _ := strconv.ParseUint(s, 10, 32)\n\t\t\tv = append(v, uint32(n))\n\t\t}\n\t\tvar r uint32\n\t\tvar err error\n\t\tif f[0] == \"triangle\" {\n\t\t\tr, err = contracts.Triangle(v[0])\n\t\t} else {\n\t\t\tr, err = contracts.Stairs(v[0], v[1])\n\t\t}\n\t\tif err != nil {\n\t\t\tfmt.Println(\"ERR\")\n\t\t} else {\n\t\t\tfmt.Println(r)\n\t\t}\n\t}\n}\n";
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
fn axiom_run_calls_down_to_0() {
    let dir = std::env::temp_dir().join(format!("axiom-recursion-run-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    let source = example();
    let stairs_rule = &source[source.find("# 계단").unwrap()..];
    fs::write(dir.join("stairs.ax"), stairs_rule).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_axiom"))
        .args(["run", "stairs.ax", "--", "1001", "12"])
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
        stdout.contains(&format!("result: {}", stairs(1001, 12))),
        "{stdout}"
    );
}
