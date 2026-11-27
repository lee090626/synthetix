//! Text choices (`type Grade = "A" | "B" | "C"`, issue #225 M3): a quoted
//! text value is its place, the examples prove, and the emitted constants
//! are named after the texts.

use std::fs;
use std::path::PathBuf;
use std::process::Command;

/// Loads the generated `app.js` and `app.wasm` in Node with a small DOM.
const DOM_PAGE: &str = r#"
const fs = require('node:fs');
const vm = require('node:vm');
const bytes = fs.readFileSync(process.env.AXIOM_TEST_WASM);
class Element {
  constructor(tagName) { this.tagName = tagName; this.children = []; this.listeners = {}; this.dataset = {}; this.value = ''; this.checked = false; this.hidden = true; this.textContent = ''; }
  append(...items) { this.children.push(...items); }
  replaceChildren() { this.children = []; }
  addEventListener(name, callback) { this.listeners[name] = callback; }
  dispatch(name) { this.listeners[name]?.({ target: this }); }
  click() { this.listeners.click?.(); }
}
const root = new Element('main');
const error = new Element('div');
global.window = {};
global.location = { reload() {} };
global.localStorage = { getItem() { return null; }, setItem() {} };
global.document = {
  querySelector(selector) { return selector === '#app' ? root : error; },
  createElement(tagName) { return new Element(tagName); },
};
global.setInterval = () => 0;
global.fetch = async (url) => url === './app.wasm'
  ? { ok: true, arrayBuffer: async () => bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength) }
  : { ok: false, text: async () => '' };
vm.runInThisContext(fs.readFileSync(process.env.AXIOM_TEST_JS, 'utf8'));
"#;

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("axiom-text-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn axiom(dir: &PathBuf, args: &[&str]) -> (bool, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_axiom"))
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    (out.status.success(), text)
}

#[test]
fn example_is_proved_and_emits_named_texts() {
    let dir = scratch("example");
    let example = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/texts");
    for file in ["rules.ax", "synthetix.toml"] {
        fs::copy(example.join(file), dir.join(file)).unwrap();
    }
    let (ok, text) = axiom(&dir, &["build", "--emit", "python"]);
    assert!(ok, "{text}");
    assert!(text.contains("verified: 3 contract(s)"), "{text}");
    let py = fs::read_to_string(dir.join("dist/contracts.py")).unwrap();
    assert!(py.contains("class Grade:"), "{py}");
    assert!(py.contains("    A = 0\n    B = 1\n    C = 2\n"), "{py}");

    // Generated Go and Rust compile, with warnings as errors.
    let (ok, text) = axiom(&dir, &["build", "--emit", "go"]);
    assert!(ok, "{text}");
    fs::write(dir.join("dist/go.mod"), "module contracts\n\ngo 1.21\n").unwrap();
    let go = Command::new("go")
        .args(["build", "-o", "/dev/null", "."])
        .current_dir(dir.join("dist"))
        .env("GOCACHE", dir.join("gocache"))
        .output()
        .unwrap();
    assert!(
        go.status.success(),
        "{}",
        String::from_utf8_lossy(&go.stderr)
    );
    let go_src = fs::read_to_string(dir.join("dist/contracts.go")).unwrap();
    assert!(go_src.contains("GradeA = 0"), "{go_src}");

    let (ok, text) = axiom(&dir, &["build", "--emit", "rust"]);
    assert!(ok, "{text}");
    let rustc = Command::new("rustc")
        .args(["--edition", "2024", "-D", "warnings", "--crate-type", "lib"])
        .args(["--crate-name", "contracts", "-o"])
        .arg(dir.join("libcontracts.rlib"))
        .arg(dir.join("dist/contracts.rs"))
        .output()
        .unwrap();
    assert!(
        rustc.status.success(),
        "{}",
        String::from_utf8_lossy(&rustc.stderr)
    );
}

#[test]
fn text_input_is_its_place_in_run() {
    let dir = scratch("run");
    fs::write(
        dir.join("bonus.ax"),
        "type Grade = \"A\" | \"B\" | \"C\"\n\nbonus(grade: Grade)\n  grade is \"A\" -> 10\n  grade is \"B\" -> 5\n  else -> 0\n  promise answer <= 10\n",
    )
    .unwrap();
    let (ok, text) = axiom(&dir, &["run", "bonus.ax", "--", "\"B\""]);
    assert!(ok, "{text}");
    assert!(text.contains("result: 5"), "{text}");
    // The input is echoed as the text the user typed, not its place.
    assert!(text.contains("input: B"), "{text}");
}

#[test]
fn text_with_bar_is_one_value() {
    let dir = scratch("bar");
    fs::write(
        dir.join("bar.ax"),
        "type Grade = \"A|x\" | \"B\"\n\nbonus(grade: Grade)\n  grade is \"A|x\" -> 10\n  else -> 0\n  promise answer <= 10\n",
    )
    .unwrap();
    let (ok, text) = axiom(&dir, &["run", "bar.ax", "--", "\"A|x\""]);
    assert!(ok, "{text}");
    assert!(text.contains("result: 10"), "{text}");
}

#[test]
fn page_text_state_is_a_select_of_the_type_values_and_runs() {
    let dir = scratch("page");
    fs::write(
        dir.join("rules.ax"),
        "type Grade = \"A\" | \"B\" | \"C\"\n\nbonus(grade: Grade)\n  grade is \"A\" -> 10\n  grade is \"B\" -> 5\n  else -> 0\n  promise answer <= 10\n\npage Bonus\n  save grade = \"\"\n  result = 0\n\n  ask \"Grade\" for grade\n  show result\n  when \"Go\" is clicked\n    result = bonus(grade)\n",
    )
    .unwrap();
    fs::copy(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/types/synthetix.toml"),
        dir.join("synthetix.toml"),
    )
    .unwrap();
    let (ok, text) = axiom(&dir, &["build"]);
    assert!(ok, "{text}");

    // The generated page runs in Node with a small DOM: the input is a select
    // of the values, a text picks its place, and an empty text is an error.
    let script = [DOM_PAGE, r#"
(async () => {
  for (let attempt = 0; attempt < 100 && !window.synthetix; attempt++)
    await new Promise((resolve) => setTimeout(resolve, 0));
  if (!window.synthetix) throw new Error('generated runtime did not start');
  const select = root.children.find((node) => node.tagName === 'select');
  const options = select.children.map((option) => option.value).join(',');
  const go = () => root.children.find((node) => node.tagName === 'button' && node.textContent === 'Go');
  select.value = 'B';
  select.dispatch('change');
  go().click();
  const picked = String(window.synthetix.state.result);
  select.value = '';
  select.dispatch('change');
  go().click();
  const blank = window.synthetix.lastError?.message ?? 'no error';
  select.value = 'C';
  select.dispatch('change');
  go().click();
  const last = String(window.synthetix.state.result);
  process.stdout.write([options, picked, blank, last].join('\n'));
})().catch((error) => { console.error(error); process.exitCode = 1; });
"#]
    .concat();
    let out = Command::new("node")
        .arg("-e")
        .arg(script)
        .env("AXIOM_TEST_JS", dir.join("dist/app.js"))
        .env("AXIOM_TEST_WASM", dir.join("dist/app.wasm"))
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines[0], ",A,B,C", "{stdout}");
    assert_eq!(lines[1], "5", "{stdout}");
    assert!(
        lines[2].contains("argument 1: \"\" is not a value of Grade (A, B, C)"),
        "{stdout}"
    );
    assert_eq!(lines[3], "0", "{stdout}");
}

#[test]
fn choice_mixes_words_and_texts_is_rejected() {
    let dir = scratch("mixed");
    fs::write(
        dir.join("bad.ax"),
        "type Grade = \"A\" | small\n\nbonus(grade: Grade)\n  always -> 0\n",
    )
    .unwrap();
    let (ok, text) = axiom(&dir, &["check", "bad.ax"]);
    assert!(!ok);
    assert!(text.contains("either names or quoted texts"), "{text}");
}

#[test]
fn page_text_state_given_to_two_types_is_rejected() {
    let dir = scratch("two-types");
    fs::write(
        dir.join("rules.ax"),
        "type Grade = \"A\" | \"B\"\n\ntype Size = \"S\" | \"M\"\n\nbonus(grade: Grade)\n  grade is \"A\" -> 10\n  else -> 0\n  promise answer <= 10\n\nfit(size: Size)\n  size is \"S\" -> 1\n  else -> 0\n  promise answer <= 1\n\npage Bonus\n  save pick = \"\"\n  result = 0\n\n  ask \"Pick\" for pick\n  show result\n  when \"Go\" is clicked\n    result = bonus(pick)\n  when \"Fit\" is clicked\n    result = fit(pick)\n",
    )
    .unwrap();
    fs::copy(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/types/synthetix.toml"),
        dir.join("synthetix.toml"),
    )
    .unwrap();
    let (ok, text) = axiom(&dir, &["build"]);
    assert!(!ok);
    assert!(text.contains("give it one type"), "{text}");
}

#[test]
fn page_literal_text_is_a_type_value() {
    let dir = scratch("literal");
    fs::write(
        dir.join("rules.ax"),
        "type Grade = \"A\" | \"B\" | \"C\"\n\nbonus(grade: Grade)\n  grade is \"A\" -> 10\n  grade is \"B\" -> 5\n  else -> 0\n  promise answer <= 10\n\npage Bonus\n  save result = 0\n\n  show result\n  when \"Go\" is clicked\n    result = bonus(\"B\")\n  when \"Bad\" is clicked\n    result = bonus(\"Z\")\n",
    )
    .unwrap();
    fs::copy(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/types/synthetix.toml"),
        dir.join("synthetix.toml"),
    )
    .unwrap();
    let (ok, text) = axiom(&dir, &["build"]);
    assert!(ok, "{text}");
    let script = [
        DOM_PAGE,
        r#"
(async () => {
  for (let attempt = 0; attempt < 100 && !window.synthetix; attempt++)
    await new Promise((resolve) => setTimeout(resolve, 0));
  if (!window.synthetix) throw new Error('generated runtime did not start');
const find = (label) => root.children.find((node) => node.tagName === 'button' && node.textContent === label);
find('Go').click();
const picked = String(window.synthetix.state.result);
find('Bad').click();
const bad = window.synthetix.lastError?.message ?? 'no error';
process.stdout.write([picked, bad].join('\n'));
})().catch((error) => { console.error(error); process.exitCode = 1; });
"#,
    ]
    .concat();
    let out = Command::new("node")
        .arg("-e")
        .arg(&script)
        .env("AXIOM_TEST_JS", dir.join("dist/app.js"))
        .env("AXIOM_TEST_WASM", dir.join("dist/app.wasm"))
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines[0], "5", "{stdout}");
    assert!(
        lines[1].contains("argument 1: \"Z\" is not a value of Grade"),
        "{stdout}"
    );
}
