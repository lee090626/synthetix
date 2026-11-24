//! `axiom build --emit python`: the emitted module matches the AMI evaluator
//! on 32-bit list rules and `settle`, and on 64-bit (`with big numbers`)
//! invoice rules, including `need` rejections and signed division.

use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use axiom_parser::{Type, parse_module};
use axiom_verify::implementation_to_ami;

/// Reads `name a b c` lines, calls `contracts.name(a, b, c)`, prints the answer
/// or `ERR` when a `need` line rejects the input.
const HARNESS: &str = r#"
import sys
sys.path.insert(0, sys.argv[1])
import contracts as c
for line in sys.stdin:
    name, *args = line.split()
    try:
        print(getattr(c, name)(*map(int, args)))
    except c.AxiomError:
        print("ERR")
"#;

fn build(dir: &Path, rules: &str) {
    let _ = fs::remove_dir_all(dir);
    fs::create_dir_all(dir).unwrap();
    fs::write(dir.join("rules.ax"), rules).unwrap();
    fs::write(
        dir.join("synthetix.toml"),
        "[project]\nentry = \"rules.ax\"\n",
    )
    .unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_axiom"))
        .args(["build", "--emit", "python"])
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn python(dir: &Path, script: &str, input: &str) -> Vec<String> {
    let mut child = Command::new("python3")
        .args(["-I", "-c", script, dir.join("dist").to_str().unwrap()])
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

fn next(seed: &mut u64) -> u64 {
    *seed = seed
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    *seed >> 11
}

fn temp(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("axiom-py-{name}-{}", std::process::id()))
}

#[test]
fn python_lists_and_settle_match_the_evaluator() {
    let dir = temp("settle");
    let example = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/group-settle");
    let rules = fs::read_to_string(example.join("rules.ax")).unwrap() + "\nuse settle for 4\n";
    build(&dir, &rules);

    let fixed = r#"
import sys
sys.path.insert(0, sys.argv[1])
import contracts as c
p = c.part([1, 1, 1, 0], [1, 1, 2, 1])
print(p, c.share(10000, p, [0] * 4, 100))
try:
    c.share(1, [0] * 4, [0] * 4, 1)
except c.AxiomError as e:
    print(e)
for line in sys.stdin:
    print(c.settle([int(v) for v in line.split()]))
"#;
    let module = parse_module("use settle for 4\n").unwrap();
    let mut seed = 11u64;
    let mut rows = Vec::new();
    let mut expected = vec![
        "[1, 1, 2, 0] [2500, 2500, 5000, 0]".to_string(),
        "share_1: input violates a requires clause".to_string(),
    ];
    for _ in 0..300 {
        let mut owe: Vec<i32> = (0..3)
            .map(|_| (next(&mut seed) % 2_000_001) as i32 - 1_000_000)
            .collect();
        // `settle` needs balances that add up to 0.
        owe.push(-owe.iter().sum::<i32>());
        let args: Vec<u64> = owe.iter().map(|v| u64::from(*v as u32)).collect();
        let sends: Vec<String> = (1..=4)
            .flat_map(|i| (1..=4).map(move |j| (i, j)))
            .map(|(i, j)| {
                let contract = module
                    .contracts
                    .iter()
                    .find(|c| c.name == format!("settle_4_{i}_{j}"))
                    .unwrap();
                let value = implementation_to_ami(contract)
                    .unwrap()
                    .try_eval_bits(&args, 32)
                    .unwrap();
                (value as u32 as i32).to_string()
            })
            .collect();
        rows.push(owe.iter().map(i32::to_string).collect::<Vec<_>>().join(" "));
        expected.push(format!("[{}]", sends.join(", ")));
    }
    assert_eq!(python(&dir, fixed, &(rows.join("\n") + "\n")), expected);
}

#[test]
fn python_big_number_rules_match_the_evaluator() {
    let dir = temp("invoice");
    let rules = include_str!("../../../examples/invoice/rules.ax");
    build(&dir, rules);
    let module = parse_module(rules).unwrap();

    let mut seed = 7u64;
    let mut input = String::new();
    let mut expected = Vec::new();
    for contract in &module.contracts {
        // Rules that call other rules need the project linker; their callees are covered here.
        let Ok(code) = axiom_synth::synthesize_with_options(contract, Default::default()) else {
            continue;
        };
        let requires = axiom_synth::lower_requires(contract).unwrap();
        for _ in 0..200 {
            // Mostly inside the `need` lines, sometimes past them.
            let args: Vec<u64> = contract
                .params
                .iter()
                .map(|_| match next(&mut seed) % 4 {
                    0 => next(&mut seed) % 40,
                    1 => next(&mut seed) % 1_100_000_000_000,
                    2 => next(&mut seed) % 10_000,
                    _ => next(&mut seed) % 20_000_000,
                })
                .collect();
            let allowed = requires
                .as_ref()
                .is_none_or(|r| r.try_eval_bits(&args, 64).unwrap() != 0);
            let answer = if allowed {
                let value = code.try_eval_bits(&args, 64).unwrap();
                match contract.return_type {
                    Type::I32 => (value as i64).to_string(),
                    _ => value.to_string(),
                }
            } else {
                "ERR".to_string()
            };
            let shown: Vec<String> = args.iter().map(u64::to_string).collect();
            input += &format!("{} {}\n", contract.name, shown.join(" "));
            expected.push(answer);
        }
    }
    assert!(expected.len() >= 1000, "too few rules compared");
    assert!(expected.iter().any(|a| a == "ERR"));
    assert!(expected.iter().any(|a| a.starts_with('-')));
    assert_eq!(python(&dir, HARNESS, &input), expected);
}
