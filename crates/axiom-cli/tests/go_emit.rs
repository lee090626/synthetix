//! `axiom build --emit go`: the emitted package compiles with `go vet`, and
//! matches the AMI evaluator on 32-bit list rules and `settle`, and on 64-bit
//! (`with big numbers`) invoice rules, including `need` rejections.
//! Uses `$GO` when set, else `go` on the PATH.

use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use axiom_parser::{Contract, Type, parse_module};
use axiom_verify::implementation_to_ami;

fn go() -> String {
    std::env::var("GO").unwrap_or_else(|_| "go".into())
}

/// Builds `rules` with `--emit go` and lays out a module `t` with the
/// package in `contracts/` and `main` in the root.
fn app(name: &str, rules: &str, main: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("axiom-go-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join("app/contracts")).unwrap();
    fs::write(dir.join("rules.ax"), rules).unwrap();
    fs::write(
        dir.join("synthetix.toml"),
        "[project]\nentry = \"rules.ax\"\n",
    )
    .unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_axiom"))
        .args(["build", "--emit", "go"])
        .current_dir(&dir)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    fs::copy(
        dir.join("dist/contracts.go"),
        dir.join("app/contracts/contracts.go"),
    )
    .unwrap();
    fs::write(dir.join("app/go.mod"), "module t\n\ngo 1.21\n").unwrap();
    fs::write(dir.join("app/main.go"), main).unwrap();
    let app = dir.join("app");
    let vet = Command::new(go())
        .args(["vet", "./..."])
        .current_dir(&app)
        .env("GOCACHE", dir.join("cache"))
        .output()
        .unwrap();
    assert!(
        vet.status.success(),
        "{}",
        String::from_utf8_lossy(&vet.stderr)
    );
    app
}

fn run(app: &Path, input: &str) -> Vec<String> {
    let mut child = Command::new(go())
        .args(["run", "."])
        .current_dir(app)
        .env("GOCACHE", app.parent().unwrap().join("cache"))
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

const SETTLE_MAIN: &str = r#"package main

import (
	"bufio"
	"fmt"
	"os"
	"strconv"
	"strings"
	"t/contracts"
)

func main() {
	p, _ := contracts.Part([4]uint32{1, 1, 1, 0}, [4]uint32{1, 1, 2, 1})
	s, _ := contracts.Share(10000, p, [4]uint32{}, 100)
	_, err := contracts.Share(1, [4]uint32{}, [4]uint32{}, 1)
	fmt.Println(p, s, err)
	in := bufio.NewScanner(os.Stdin)
	for in.Scan() {
		var owe []int32
		for _, f := range strings.Fields(in.Text()) {
			v, _ := strconv.ParseInt(f, 10, 32)
			owe = append(owe, int32(v))
		}
		out, err := contracts.Settle(owe)
		if err != nil {
			panic(err)
		}
		fmt.Println(out)
	}
}
"#;

#[test]
fn go_lists_and_settle_match_the_evaluator() {
    let example = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/group-settle");
    let rules = fs::read_to_string(example.join("rules.ax")).unwrap() + "\nuse settle for 4\n";
    let app = app("settle", &rules, SETTLE_MAIN);

    let module = parse_module("use settle for 4\n").unwrap();
    let mut seed = 11u64;
    let mut rows = Vec::new();
    let mut expected =
        vec!["[1 1 2 0] [2500 2500 5000 0] share_1: input violates a requires clause".to_string()];
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
        expected.push(format!("[{}]", sends.join(" ")));
    }
    assert_eq!(run(&app, &(rows.join("\n") + "\n")), expected);
}

/// `fee_first` -> `FeeFirst`, as the emitter names Go functions.
fn go_name(name: &str) -> String {
    name.split('_')
        .map(|p| p[..1].to_uppercase() + &p[1..])
        .collect()
}

/// A `main` that reads `name a b c` lines and prints the answer or `ERR`.
fn dispatch(contracts: &[&Contract]) -> String {
    let mut cases = String::new();
    for c in contracts {
        let args: Vec<String> = c
            .params
            .iter()
            .enumerate()
            .map(|(i, p)| match p.ty {
                Type::I32 => format!("int64(a[{i}])"),
                _ => format!("uint64(a[{i}])"),
            })
            .collect();
        cases += &format!(
            "\t\tcase \"{}\":\n\t\t\tr, err := contracts.{}({})\n\t\t\tshow(r, err)\n",
            c.name,
            go_name(&c.name),
            args.join(", ")
        );
    }
    format!(
        r#"package main

import (
	"bufio"
	"fmt"
	"os"
	"strconv"
	"strings"
	"t/contracts"
)

func show[T any](r T, err error) {{
	if err != nil {{
		fmt.Println("ERR")
	}} else {{
		fmt.Println(r)
	}}
}}

func main() {{
	in := bufio.NewScanner(os.Stdin)
	for in.Scan() {{
		f := strings.Fields(in.Text())
		var a []int64
		for _, v := range f[1:] {{
			n, _ := strconv.ParseInt(v, 10, 64)
			a = append(a, n)
		}}
		switch f[0] {{
{cases}		}}
	}}
}}
"#
    )
}

#[test]
fn go_big_number_rules_match_the_evaluator() {
    let rules = include_str!("../../../examples/invoice/rules.ax");
    let module = parse_module(rules).unwrap();
    // Rules that call other rules need the project linker; their callees are covered here.
    let compared: Vec<(&Contract, axiom_ami::Expr)> = module
        .contracts
        .iter()
        .filter_map(|c| {
            axiom_synth::synthesize_with_options(c, Default::default())
                .ok()
                .map(|code| (c, code))
        })
        .collect();
    let names: Vec<&Contract> = compared.iter().map(|(c, _)| *c).collect();
    let app = app("invoice", rules, &dispatch(&names));

    let mut seed = 7u64;
    let mut input = String::new();
    let mut expected = Vec::new();
    for (contract, code) in &compared {
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
    assert_eq!(run(&app, &input), expected);
}
