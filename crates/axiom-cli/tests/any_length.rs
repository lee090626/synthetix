//! Lists of any length (`prices[n]`): `--emit js|python|rust|go` all give the
//! AMI evaluator's answer on random lists (each item rule, added up, then the
//! core rule), including empty lists, lists past the length limit, and items
//! past their `need` lines. Uses `$GO` when set, else `go` on the PATH.

use std::{
    collections::HashMap,
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use axiom_parser::{Contract, Fold, ListRule, Type, parse_module};

fn example() -> String {
    fs::read_to_string(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/any-length/rules.ax"),
    )
    .unwrap()
}

fn build(emit: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("axiom-any-{emit}-{}", std::process::id()));
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

fn next(seed: &mut u64) -> u64 {
    *seed = seed
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    *seed >> 11
}

/// A value as a word of the rule's width.
fn word(v: i128, bits: u32) -> u64 {
    (v as u64) & if bits == 32 { 0xFFFF_FFFF } else { u64::MAX }
}

/// A rule's `need` check and its proven code, compiled once.
struct Compiled {
    wide: bool,
    requires: Option<axiom_ami::Expr>,
    code: axiom_ami::Expr,
}

impl Compiled {
    fn new(c: &Contract) -> Self {
        Compiled {
            wide: c.wide,
            requires: axiom_synth::lower_requires(c).unwrap(),
            code: axiom_synth::synthesize_with_options(c, Default::default()).unwrap(),
        }
    }

    /// Whether the `need` lines accept `args`, and the answer.
    fn eval(&self, args: &[u64]) -> Option<u64> {
        let bits = if self.wide { 64 } else { 32 };
        if let Some(r) = &self.requires
            && r.try_eval_bits(args, bits).unwrap() == 0
        {
            return None;
        }
        Some(self.code.try_eval_bits(args, bits).unwrap())
    }
}

/// The answer a caller should get, or `ERR`.
fn expected(
    compiled: &HashMap<String, Compiled>,
    core: &Contract,
    rule: &ListRule,
    values: &[Vec<i128>],
) -> String {
    let bits = if core.wide { 64 } else { 32 };
    let n = values
        .iter()
        .zip(&rule.params)
        .find(|(_, (_, list))| *list)
        .map_or(0, |(v, _)| v.len());
    if (n as u64) < rule.least || n as u64 > rule.max {
        return "ERR".into();
    }
    let signed = |v: u64| match core.return_type {
        Type::I32 => i128::from(if bits == 32 {
            v as u32 as i32 as i64
        } else {
            v as i64
        }),
        _ => i128::from(v),
    };
    let mut sums = Vec::new();
    for (item, fold) in rule.items.iter().zip(&rule.folds) {
        let contract = &compiled[item];
        let mut total = 0u64;
        for i in 0..n {
            let args: Vec<u64> = values
                .iter()
                .zip(&rule.params)
                .map(|(v, (_, list))| word(if *list { v[i] } else { v[0] }, bits))
                .collect();
            let Some(value) = contract.eval(&args) else {
                return "ERR".into();
            };
            total = match fold {
                Fold::Sum => word(i128::from(total) + i128::from(value), bits),
                Fold::Min if i > 0 && signed(total) <= signed(value) => total,
                Fold::Max if i > 0 && signed(total) >= signed(value) => total,
                _ => value,
            };
        }
        sums.push(total);
    }
    let show = |v: u64| match (&core.return_type, bits) {
        (Type::I32, 32) => (v as u32 as i32).to_string(),
        (Type::I32, _) => (v as i64).to_string(),
        _ => v.to_string(),
    };
    let core = &compiled[&core.name];
    if rule.each {
        // A list answer, shown as `1,2,3`: the core once per item.
        let mut answers = Vec::new();
        for i in 0..n {
            let mut args: Vec<u64> = values
                .iter()
                .zip(&rule.params)
                .map(|(v, (_, list))| word(if *list { v[i] } else { v[0] }, bits))
                .collect();
            args.push(n as u64);
            args.extend(&sums);
            let Some(v) = core.eval(&args) else {
                return "ERR".into();
            };
            answers.push(show(v));
        }
        return answers.join(",");
    }
    let mut args: Vec<u64> = values
        .iter()
        .zip(&rule.params)
        .filter(|(_, (_, list))| !*list)
        .map(|(v, _)| word(v[0], bits))
        .collect();
    args.push(n as u64);
    args.extend(sums);
    core.eval(&args).map_or("ERR".into(), show)
}

/// `name [1,2] 7` lines, and the answers expected for them.
fn cases() -> (String, Vec<String>) {
    let module = parse_module(&example()).unwrap();
    // Synthesis is the slow part: once per rule, not once per case.
    let compiled: HashMap<String, Compiled> = module
        .contracts
        .iter()
        .map(|c| (c.name.clone(), Compiled::new(c)))
        .collect();
    let mut seed = 5u64;
    let mut input = String::new();
    let mut answers = Vec::new();
    for core in module.contracts.iter().filter(|c| c.list.is_some()) {
        let rule = core.list.as_ref().unwrap();
        let signed = core.return_type == Type::I32;
        for case in 0..60 {
            let n = match case {
                0 => 0,
                1 => rule.max as usize,
                2 => rule.max as usize + 1,
                _ => (next(&mut seed) % 12) as usize,
            };
            // Every few cases, items small enough for every `need` line.
            let small = case % 4 == 3;
            let value = |seed: &mut u64| -> i128 {
                if small {
                    return i128::from(1 + next(seed) % 100);
                }
                let v = match next(seed) % 5 {
                    0 => next(seed) % 2_000_000,
                    1 => next(seed) % 1_100_000_000_000,
                    2 => next(seed) % 200_000,
                    _ => next(seed) % 110_000,
                };
                let v = i128::from(if core.wide { v } else { v % 1_100_000 });
                if signed && next(seed).is_multiple_of(8) {
                    -v
                } else {
                    v
                }
            };
            let values: Vec<Vec<i128>> = rule
                .params
                .iter()
                .map(|(_, list)| {
                    let count = if *list { n } else { 1 };
                    (0..count).map(|_| value(&mut seed)).collect()
                })
                .collect();
            let shown: Vec<String> = values
                .iter()
                .zip(&rule.params)
                .map(|(v, (_, list))| {
                    let items: Vec<String> = v.iter().map(i128::to_string).collect();
                    if *list {
                        format!("[{}]", items.join(","))
                    } else {
                        items[0].clone()
                    }
                })
                .collect();
            input += &format!("{} {}\n", rule.name, shown.join(" "));
            answers.push(expected(&compiled, core, rule, &values));
        }
    }
    assert!(answers.iter().any(|a| a == "ERR"));
    assert!(answers.iter().any(|a| a.starts_with('-')));
    assert!(answers.iter().any(|a| a.contains(',')));
    (input, answers)
}

const JS: &str = r#"
const c = await import(process.argv[2] + "/dist/contracts.js");
const wide = new Set(process.argv.slice(3));
const lines = require("fs").readFileSync(0, "utf8").trim().split("\n");
for (const line of lines) {
  const [name, ...args] = line.split(" ");
  const num = wide.has(name) ? BigInt : Number;
  const parsed = args.map((a) => a.startsWith("[") ? (a === "[]" ? [] : a.slice(1, -1).split(",").map(num)) : num(a));
  try { console.log(String(c[name](...parsed))); } catch (e) { console.log("ERR"); }
}
"#;

#[test]
fn js_matches_the_evaluator() {
    let (input, answers) = cases();
    let dir = build("js");
    fs::write(dir.join("harness.mjs"), format!("import {{ createRequire }} from \"module\";\nconst require = createRequire(import.meta.url);\n{JS}")).unwrap();
    let module = parse_module(&example()).unwrap();
    let mut args = vec!["harness.mjs".to_string(), dir.display().to_string()];
    args.extend(
        module
            .contracts
            .iter()
            .filter_map(|c| c.list.as_ref().filter(|_| c.wide).map(|r| r.name.clone())),
    );
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    assert_eq!(run("node", &args, &dir, &input), answers);
}

const PY: &str = r#"
import sys
sys.path.insert(0, sys.argv[1])
import contracts as c
for line in sys.stdin:
    name, *args = line.split()
    parsed = [[int(v) for v in a[1:-1].split(",") if v] if a.startswith("[") else int(a) for a in args]
    try:
        r = getattr(c, name)(*parsed)
        print(",".join(map(str, r)) if isinstance(r, list) else r)
    except (c.AxiomError, TypeError):
        print("ERR")
"#;

#[test]
fn python_matches_the_evaluator() {
    let (input, answers) = cases();
    let dir = build("python");
    fs::write(dir.join("harness.py"), PY).unwrap();
    let dist = dir.join("dist").display().to_string();
    assert_eq!(
        run("python3", &["-I", "harness.py", &dist], &dir, &input),
        answers
    );
}

/// Per rule: its typed call, built from each input token `f[k]`, and
/// whether it answers with a list.
fn calls(
    convert: impl Fn(&str, bool, usize) -> String,
    name: impl Fn(&str) -> String,
) -> Vec<(String, String, bool)> {
    let module = parse_module(&example()).unwrap();
    module
        .contracts
        .iter()
        .filter_map(|c| c.list.as_ref().map(|r| (c, r)))
        .map(|(c, r)| {
            let ty = match (&c.params[0].ty, c.wide) {
                (Type::I32, false) => "i32",
                (Type::I32, true) => "i64",
                (_, false) => "u32",
                (_, true) => "u64",
            };
            let args: Vec<String> = r
                .params
                .iter()
                .enumerate()
                .map(|(k, (_, list))| convert(ty, *list, k + 1))
                .collect();
            (
                r.name.clone(),
                format!("{}({})", name(&r.name), args.join(", ")),
                r.each,
            )
        })
        .collect()
}

#[test]
fn rust_matches_the_evaluator() {
    let (input, answers) = cases();
    let dir = build("rust");
    let arms: String = calls(
        |ty, list, k| {
            if list {
                format!("&f[{k}].trim_matches(|c| c == '[' || c == ']').split(',').filter(|v| !v.is_empty()).map(|v| v.parse::<{ty}>().unwrap()).collect::<Vec<_>>()")
            } else {
                format!("f[{k}].parse::<{ty}>().unwrap()")
            }
        },
        |n| n.to_string(),
    )
    .into_iter()
    .map(|(name, call, each)| {
        let call = if each {
            format!("{call}.map(|v| v.iter().map(|x| x.to_string()).collect::<Vec<_>>().join(\",\"))")
        } else {
            call
        };
        format!("            \"{name}\" => show({call}),\n")
    })
    .collect();
    let main = format!(
        "mod contracts;\nuse contracts::*;\nuse std::io::BufRead;\nfn show<T: std::fmt::Display>(r: Result<T, AxiomError>) {{\n    match r {{ Ok(v) => println!(\"{{v}}\"), Err(_) => println!(\"ERR\") }}\n}}\nfn main() {{\n    for line in std::io::stdin().lock().lines() {{\n        let line = line.unwrap();\n        let f: Vec<&str> = line.split(' ').collect();\n        match f[0] {{\n{arms}            _ => unreachable!(),\n        }}\n    }}\n}}\n"
    );
    fs::copy(dir.join("dist/contracts.rs"), dir.join("contracts.rs")).unwrap();
    fs::write(dir.join("main.rs"), main).unwrap();
    let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".into());
    let out = Command::new(rustc)
        .args(["--edition", "2024", "-O", "main.rs", "-o", "main"])
        .current_dir(&dir)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        run(&dir.join("main").display().to_string(), &[], &dir, &input),
        answers
    );
}

#[test]
fn go_matches_the_evaluator() {
    let (input, answers) = cases();
    let dir = build("go");
    let go_ty = |ty: &str| match ty {
        "i32" => "int32",
        "i64" => "int64",
        "u32" => "uint32",
        _ => "uint64",
    };
    let cases: String = calls(
        |ty, list, k| {
            let t = go_ty(ty);
            if list {
                format!("nums[{t}](f[{k}])")
            } else {
                format!("{t}(num(f[{k}]))")
            }
        },
        |n| {
            let mut s = n.to_string();
            s[..1].make_ascii_uppercase();
            format!("contracts.{s}")
        },
    )
    .into_iter()
    .map(|(name, call, each)| {
        let show = if each { "showList" } else { "show" };
        format!("\t\tcase \"{name}\":\n\t\t\t{show}({call})\n")
    })
    .collect();
    let main = format!(
        "package main\n\nimport (\n\t\"bufio\"\n\t\"fmt\"\n\t\"os\"\n\t\"strconv\"\n\t\"strings\"\n\t\"t/contracts\"\n)\n\nfunc num(s string) int64 {{\n\tif v, err := strconv.ParseInt(s, 10, 64); err == nil {{\n\t\treturn v\n\t}}\n\tv, _ := strconv.ParseUint(s, 10, 64)\n\treturn int64(v)\n}}\n\nfunc nums[T int32 | int64 | uint32 | uint64](s string) []T {{\n\tout := []T{{}}\n\tfor _, v := range strings.Split(strings.Trim(s, \"[]\"), \",\") {{\n\t\tif v != \"\" {{\n\t\t\tout = append(out, T(num(v)))\n\t\t}}\n\t}}\n\treturn out\n}}\n\nfunc show[T any](r T, err error) {{\n\tif err != nil {{\n\t\tfmt.Println(\"ERR\")\n\t}} else {{\n\t\tfmt.Println(r)\n\t}}\n}}\n\nfunc showList[T any](r []T, err error) {{\n\tif err != nil {{\n\t\tfmt.Println(\"ERR\")\n\t}} else {{\n\t\tfmt.Println(strings.ReplaceAll(strings.Trim(fmt.Sprint(r), \"[]\"), \" \", \",\"))\n\t}}\n}}\n\nfunc main() {{\n\tin := bufio.NewScanner(os.Stdin)\n\tin.Buffer(make([]byte, 1<<20), 1<<26)\n\tfor in.Scan() {{\n\t\tf := strings.Fields(in.Text())\n\t\tswitch f[0] {{\n{cases}\t\t}}\n\t}}\n}}\n"
    );
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
fn axiom_run_adds_up_a_list() {
    let dir = std::env::temp_dir().join(format!("axiom-any-run-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    let cart: String = example()
        .lines()
        .take(8)
        .map(|l| format!("{l}\n"))
        .collect();
    fs::write(dir.join("cart.ax"), cart).unwrap();
    let run = |list: &str| {
        let out = Command::new(env!("CARGO_BIN_EXE_axiom"))
            .args(["run", "cart.ax", "--", list])
            .current_dir(&dir)
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).to_string() + &String::from_utf8_lossy(&out.stderr)
    };
    assert!(run("[1,2,3]").contains("result: 6"));
    assert!(run("[100000,100000,100000]").contains("result: 200000"));
    assert!(run("[]").contains("result: 0"));
    assert!(run("[100001]").contains("input violates a requires clause"));
}

#[test]
fn axiom_run_answers_with_a_list() {
    let dir = std::env::temp_dir().join(format!("axiom-any-each-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    let source = example();
    let share = source[source.find("share(").unwrap()..]
        .split("\n\n")
        .next()
        .unwrap();
    fs::write(dir.join("share.ax"), share).unwrap();
    let run = |list: &str| {
        let out = Command::new(env!("CARGO_BIN_EXE_axiom"))
            .args(["run", "share.ax", "--", list, "1000"])
            .current_dir(&dir)
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).to_string() + &String::from_utf8_lossy(&out.stderr)
    };
    assert!(run("[1,2,1]").contains("result: [250,500,250]"));
    assert!(run("[]").contains("input violates a requires clause"));
}
