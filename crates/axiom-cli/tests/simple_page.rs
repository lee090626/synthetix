//! Builds a simple-syntax app with a list, a `can be negative` rule, and a
//! conditional `show`, then clicks through the generated page in node.

use std::{fs, process::Command};

const LOGIC: &str = "\
change(now, before) can be negative
  need now <= 1000000 and now >= 0
  need before <= 1000000 and before >= 0
  -> now - before

cart(prices[3])
  need prices <= 100000
  -> sum(prices)

vat(price) with 2 decimals
  need price <= 100000
  -> price * 0.1
";

const MAIN: &str = "\
from logic: change, cart, vat

page Shop
  save prices[3] = 0
  save total, last = 0
  save diff = 0 can be negative

  ask prices \"Price\"
  ask last \"Last\"
  button \"Total\" -> total = cart(prices)
  button \"Change\" -> diff = change(total, last)
  show total
  show diff
  show \"Over 200000\" if total > 200000

  save price, tax = 0 with 2 decimals
  ask price \"Price\"
  button \"Tax\" -> tax = vat(price)
  show tax
";

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
  const lines = [inputs().map((input) => input.placeholder).join(',')];
  type([1000, 2000, 3000, 9000]);
  press('Total');
  press('Change');
  lines.push(`${outputs().join(',')}|${texts().includes('Over 200000')}`);
  type([100000, 100000, 50000, 0]);
  press('Total');
  press('Change');
  lines.push(`${outputs().join(',')}|${texts().includes('Over 200000')}`);
  const price = inputs()[4];
  price.value = '12.5';
  price.dispatch('change');
  press('Tax');
  lines.push(`${window.synthetix.state.price},${outputs().at(-1)}`);
  price.value = '1.234';
  price.dispatch('change');
  lines.push(`${window.synthetix.state.price}`);
  process.stdout.write(lines.join('\n'));
})().catch((failure) => { console.error(failure); process.exitCode = 1; });
"#;

#[test]
fn list_negative_and_conditional_show_work_in_the_page() {
    if Command::new("node").arg("--version").output().is_err() {
        eprintln!("SKIPPED simple_page: node not found");
        return;
    }
    let dir = std::env::temp_dir().join(format!("axiom-simple-page-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("synthetix.toml"),
        "[project]\nentry = \"main.ax\"\ntarget = \"web\"\n",
    )
    .unwrap();
    fs::write(dir.join("logic.ax"), LOGIC).unwrap();
    fs::write(dir.join("main.ax"), MAIN).unwrap();

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
        "Price 1,Price 2,Price 3,Last,Price\n6000,-3000,0.00|false\n250000,250000,0.00|true\n1250,1.25\n1250"
    );
    fs::remove_dir_all(dir).ok();
}
