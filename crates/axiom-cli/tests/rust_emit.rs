//! `axiom build --emit rust`: the emitted module compiles with rustc, list
//! rules take arrays, and `settle` matches the AMI evaluator on random balances.

use std::{fs, path::PathBuf, process::Command};

use axiom_parser::parse_module;
use axiom_verify::{implementation_to_ami, reference::eval_ami};

const MAIN: &str = r#"mod contracts;
use contracts::*;
use std::io::BufRead;
fn main() {
    let p = part([1, 1, 1, 0], [1, 1, 2, 1]).unwrap();
    println!("{:?} {:?} {:?}", p, share(10000, p, [0; 4], 100), share(1, [0; 4], [0; 4], 1));
    for line in std::io::stdin().lock().lines() {
        let owe: Vec<i32> = line.unwrap().split(' ').map(|v| v.parse().unwrap()).collect();
        println!("{:?}", settle(&owe).unwrap());
    }
}
"#;

#[test]
fn rust_module_matches_the_evaluator() {
    let dir = std::env::temp_dir().join(format!("axiom-rust-emit-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join("bin")).unwrap();
    let example = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/group-settle");
    let rules = fs::read_to_string(example.join("rules.ax")).unwrap() + "\nuse settle for 4\n";
    fs::write(dir.join("rules.ax"), &rules).unwrap();
    fs::write(
        dir.join("synthetix.toml"),
        "[project]\nentry = \"rules.ax\"\n",
    )
    .unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_axiom"))
        .args(["build", "--emit", "rust"])
        .current_dir(&dir)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    fs::copy(dir.join("dist/contracts.rs"), dir.join("bin/contracts.rs")).unwrap();
    fs::write(dir.join("bin/main.rs"), MAIN).unwrap();
    let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".into());
    let out = Command::new(rustc)
        .args(["--edition", "2024", "-O", "main.rs", "-o", "main"])
        .current_dir(dir.join("bin"))
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );

    let module = parse_module("use settle for 4\n").unwrap();
    let code = |from: usize, to: usize| {
        let contract = module
            .contracts
            .iter()
            .find(|c| c.name == format!("settle_4_{from}_{to}"))
            .unwrap();
        implementation_to_ami(contract).unwrap()
    };
    let mut seed = 11u64;
    let mut rows = Vec::new();
    let mut expected = vec![
        "[1, 1, 2, 0] Ok([2500, 2500, 5000, 0]) Err(AxiomError(\"share_1: input violates a requires clause\"))".to_string(),
    ];
    for _ in 0..300 {
        let mut owe: Vec<i32> = (0..3)
            .map(|_| {
                seed = seed
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                ((seed >> 33) % 2_000_001) as i32 - 1_000_000
            })
            .collect();
        // `settle` needs balances that add up to 0.
        owe.push(-owe.iter().sum::<i32>());
        let args: Vec<u32> = owe.iter().map(|v| *v as u32).collect();
        let sends: Vec<String> = (1..=4)
            .flat_map(|i| (1..=4).map(move |j| (i, j)))
            .map(|(i, j)| (eval_ami(&code(i, j), &args).unwrap() as i32).to_string())
            .collect();
        rows.push(owe.iter().map(i32::to_string).collect::<Vec<_>>().join(" "));
        expected.push(format!("[{}]", sends.join(", ")));
    }
    let mut child = Command::new(dir.join("bin/main"))
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    use std::io::Write;
    child
        .stdin
        .take()
        .unwrap()
        .write_all((rows.join("\n") + "\n").as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    let got: Vec<String> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(String::from)
        .collect();
    assert_eq!(got, expected);
}
