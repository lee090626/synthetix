//! Builds examples/group-settle and settles a two-round gathering in node:
//! several payments (A), people left out (B), transfers (C), ratio and
//! rounding (D), and copying and keeping the result (E).

use std::{fs, path::PathBuf, process::Command};

const RUNTIME: &str = r#"
const fs = require('node:fs');
const vm = require('node:vm');
const dist = process.env.AXIOM_TEST_DIST;
const bytes = fs.readFileSync(`${dist}/app.wasm`);
class Element {
  constructor(tagName) { this.tagName = tagName; this.children = []; this.listeners = {}; this.dataset = {}; this.value = ''; this.checked = false; this.hidden = true; this.textContent = ''; }
  append(...items) { this.children.push(...items); }
  replaceChildren() { this.children = []; }
  addEventListener(name, callback) { this.listeners[name] = callback; }
  dispatch(name) { this.listeners[name]?.({ target: this }); }
  click() { this.dispatch('click'); }
}
const root = new Element('main');
const error = new Element('div');
const storage = new Map();
global.window = {};
global.location = { reload() {} };
global.localStorage = {
  getItem(key) { return storage.get(key) ?? null; },
  setItem(key, value) { storage.set(key, String(value)); },
};
global.document = {
  querySelector(selector) { return selector === '#app' ? root : error; },
  createElement(tagName) { return new Element(tagName); },
};
global.setInterval = () => 0;
let copied = '';
Object.defineProperty(global, 'navigator', { value: { clipboard: { writeText(text) { copied = text; } } }, configurable: true });
global.fetch = async (url) => url === './app.wasm'
  ? { ok: true, arrayBuffer: async () => bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength) }
  : { ok: false, text: async () => '' };
vm.runInThisContext(fs.readFileSync(`${dist}/app.config.js`, 'utf8'));
vm.runInThisContext(fs.readFileSync(`${dist}/app.js`, 'utf8'));
const inputs = () => root.children.filter((node) => node.tagName === 'input');
const press = (label) => root.children.find((node) => node.tagName === 'button' && node.textContent === label).click();
const texts = () => root.children.filter((node) => node.tagName === 'p').map((node) => node.textContent);
const outputs = () => root.children.filter((node) => node.tagName === 'output').map((node) => node.textContent);
function type(values) {
  values.forEach((value, index) => {
    const input = inputs()[index];
    input.value = String(value);
    input.dispatch('change');
  });
}
(async () => {
  for (let attempt = 0; attempt < 100 && !window.synthetix; attempt++)
    await new Promise((resolve) => setTimeout(resolve, 0));
  if (!window.synthetix) throw new Error('runtime did not start');
  const set = (index, value) => {
    const input = inputs()[index];
    if (input.type === 'checkbox') input.checked = value; else input.value = String(value);
    input.dispatch('change');
  };
  ['민수', '지영', '철수', '영희'].forEach((name, index) => set(index, name));
  set(5, 40000);            // 1차 40000, 민수가 냄
  set(6, 40000);
  set(22, 30000);           // 2차 30000, 지영이 냄, 철수는 빠짐
  set(24, 30000);
  set(29, false);
  press('정산');
  const lines = [texts().join('|')];
  press('복사');
  lines.push(copied.split('\n').join('|'));
  press('기록 저장');
  lines.push(String(root.children.filter((node) => node.tagName === 'pre').length));
  set(31, 2);               // 2차에서 민수 몫 두 배, 10원 단위
  set(4, 10);
  press('정산');
  lines.push(texts().join('|'));
  process.stdout.write(lines.join('\n'));
})().catch((failure) => { console.error(failure); process.exitCode = 1; });
"#;

#[test]
fn group_settle_example_settles_a_gathering() {
    if Command::new("node").arg("--version").output().is_err() {
        eprintln!("SKIPPED group_settle: node not found");
        return;
    }
    let example = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/group-settle");
    let dir = std::env::temp_dir().join(format!("axiom-group-settle-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    for file in ["synthetix.toml", "main.ax", "rules.ax"] {
        fs::copy(example.join(file), dir.join(file)).unwrap();
    }
    let build = Command::new(env!("CARGO_BIN_EXE_axiom"))
        .arg("build")
        .env_remove("AXIOM_LEGACY_SYNTAX")
        .current_dir(&dir)
        .output()
        .unwrap();
    assert!(
        build.status.success(),
        "{}",
        String::from_utf8_lossy(&build.stderr)
    );
    let run = Command::new("node")
        .args(["-e", RUNTIME])
        .env("AXIOM_TEST_DIST", dir.join("dist"))
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(
        String::from_utf8(run.stdout).unwrap(),
        "철수 → 민수 10000|영희 → 민수 10000|영희 → 지영 10000\n\
         철수 → 민수 10000|영희 → 민수 10000|영희 → 지영 10000\n\
         1\n\
         철수 → 민수 10000|영희 → 민수 5000|영희 → 지영 12500"
    );
    fs::remove_dir_all(dir).ok();
}
