//! `axiom run <project> --action <button>` (M6): one page action runs headlessly.
//! A rule answer comes from the verified WASM; an effect answer comes from the
//! adapter and is refused unless it matches the declared type.

use std::{fs, path::PathBuf, process::Command};

const EFFECT_PAGE: &str = r#"effect lookup_price(id: u32) -> u32
double(n)
  need n < 1000
  always -> n * 2

page Shop
  item = 0
  price = 0

  ask "Item" for item
  show price
  when "Load" is clicked
    price = lookup_price(item)
"#;

const COUNTER_PAGE: &str = r#"double(n)
  need n < 1000
  always -> n * 2

page Counter
  count = 0
  show count
  when "Add" is clicked
    count = double(count)
"#;

fn project(name: &str, main: &str, adapter: Option<&str>) -> PathBuf {
    let root = std::env::temp_dir().join(format!("axiom-page-run-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    fs::write(
        root.join("synthetix.toml"),
        "[project]\nentry = \"main.ax\"\ntarget = \"web\"\n",
    )
    .unwrap();
    fs::write(root.join("main.ax"), main).unwrap();
    if let Some(adapter) = adapter {
        fs::write(root.join("adapter.js"), adapter).unwrap();
    }
    root
}

fn run(dir: &PathBuf, args: &[&str]) -> (bool, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_axiom"))
        .arg("run")
        .arg(dir)
        .args(args)
        .output()
        .unwrap();
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

#[test]
fn page_run_applies_an_effect_answer_and_prints_the_state() {
    let dir = project(
        "applied",
        EFFECT_PAGE,
        Some("module.exports = async (request) => request.inputs[0] + 40;\n"),
    );
    let adapter = dir.join("adapter.js");
    let (ok, stdout, stderr) = run(
        &dir,
        &[
            "--action",
            "Load",
            "--set",
            "item=2",
            "--adapter",
            adapter.to_str().unwrap(),
        ],
    );
    assert!(ok, "{stderr}");
    assert!(stdout.contains("transition: APPLIED\n"), "{stdout}");
    assert!(
        stdout.contains("effect: SUCCEEDED lookup_price\n"),
        "{stdout}"
    );
    assert!(
        stdout.ends_with("state: {\"item\":2,\"price\":42}\n"),
        "{stdout}"
    );
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn page_run_keeps_the_state_when_the_effect_has_no_valid_answer() {
    let dir = project(
        "refused",
        EFFECT_PAGE,
        Some("module.exports = async () => -1;\n"),
    );
    let adapter = dir.join("adapter.js");
    let adapter_path = adapter.to_str().unwrap();
    let (ok, _, stderr) = run(
        &dir,
        &[
            "--action",
            "Load",
            "--set",
            "item=2",
            "--adapter",
            adapter_path,
        ],
    );
    assert!(!ok);
    assert!(stderr.contains("FAILED"), "{stderr}");
    assert!(
        stderr.contains("external response does not match u32"),
        "{stderr}"
    );

    // Without an adapter the call never reaches host code, and the state stays declared.
    let (ok, _, stderr) = run(&dir, &["--action", "Load", "--set", "item=2"]);
    assert!(!ok);
    assert!(
        stderr.contains("No effect adapter registered for lookup_price"),
        "{stderr}"
    );
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn page_run_answers_a_rule_button_from_the_verified_wasm() {
    let dir = project("rule", COUNTER_PAGE, None);
    let (ok, stdout, stderr) = run(&dir, &["--action", "Add", "--set", "count=5"]);
    assert!(ok, "{stderr}");
    assert!(stdout.ends_with("state: {\"count\":10}\n"), "{stdout}");
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn page_run_rejects_unknown_buttons_states_and_options() {
    let dir = project("bad-options", EFFECT_PAGE, None);
    for (args, message) in [
        (
            vec!["--set", "item=2"],
            "a page run needs --action <button label>",
        ),
        (vec!["--action", "Nope"], "no button named Nope"),
        (
            vec!["--action", "Load", "--set", "missing=1"],
            "no state named missing",
        ),
        (
            vec!["--action", "Load", "--set", "=1"],
            "--set expects name=value",
        ),
        (
            vec!["--action", "Load", "--set", "item=abc"],
            "item: input is outside u32",
        ),
        (
            vec!["--action", "Load", "--wait-ms", "0"],
            "--wait-ms takes a number from 1 to 60000",
        ),
        (
            vec!["--action", "Load", "--unknown"],
            "unknown option for a page run: --unknown",
        ),
    ] {
        let (ok, _, stderr) = run(&dir, &args);
        assert!(!ok, "{args:?}");
        assert!(stderr.contains(message), "{args:?}: {stderr}");
    }
    fs::remove_dir_all(dir).unwrap();
}
