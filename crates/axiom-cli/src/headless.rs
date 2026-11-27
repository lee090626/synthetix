//! `axiom run <project> --action <button>` (issue #225, M6): runs one page
//! action without a browser. The project is built into `.synthetix/run`, then
//! the generated runtime runs under Node.js, the same code the browser runs.
//! Rules run through their verified WASM. Effects are answered by the
//! `--adapter` file, which is the user's own host code and is not verified;
//! its answers are checked against the declared type, as in the browser.
//!
//! Arguments reach Node as argv entries, never as code, so a label or value
//! cannot change the driver script. Saved values are not read or written: each
//! run starts from the declared values.

use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

/// The driver: a DOM stub, the generated page, then one click.
const DRIVER: &str = r#"
const fs = require('node:fs');
const vm = require('node:vm');
const [action, adapter, waitText, ...sets] = process.argv.slice(1);
const waitMs = Number(waitText);
const bytes = fs.readFileSync(process.env.AXIOM_WASM);
class Element {
  constructor(tagName) { this.tagName = tagName; this.children = []; this.listeners = {}; this.dataset = {}; this.hidden = true; this.textContent = ''; }
  append(...items) { this.children.push(...items); }
  replaceChildren() { this.children = []; }
  addEventListener(name, callback) { this.listeners[name] = callback; }
}
const root = new Element('main');
const errorBox = new Element('div');
global.window = {};
global.location = { reload() {} };
global.localStorage = { getItem() { return null; }, setItem() {} };
global.document = {
  querySelector(selector) { return selector === '#app' ? root : errorBox; },
  createElement(tagName) { return new Element(tagName); },
};
global.setInterval = () => 0;
global.fetch = async (url) => url === './app.wasm'
  ? { ok: true, arrayBuffer: async () => bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength) }
  : { ok: false, text: async () => '' };
vm.runInThisContext(fs.readFileSync(process.env.AXIOM_JS, 'utf8'));
const fail = (message) => { process.stderr.write(`${message}\n`); process.exit(1); };
const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
const findButton = (node, label) => {
  if (node.tagName === 'button' && node.textContent === label) return node;
  for (const child of node.children) {
    const found = findButton(child, label);
    if (found) return found;
  }
  return null;
};
const findAction = (elements, label) => {
  for (const element of elements ?? []) {
    if ((element.kind === 'button' || element.kind === 'reducer') && element.label === label) return element;
    const nested = findAction(element.elements, label);
    if (nested) return nested;
  }
  return null;
};
const report = (synthetix, transition) => {
  if (transition) process.stdout.write(`transition: ${transition.status}\n`);
  const effect = synthetix.lastEffect;
  if (effect) process.stdout.write(`effect: ${effect.status} ${effect.name}\n`);
  process.stdout.write(`state: ${JSON.stringify(synthetix.state)}\n`);
  process.exit(0);
};
(async () => {
  try {
    await window.synthetixReady;
  } catch (error) {
    fail(`page did not start: ${error.message}`);
  }
  const synthetix = window.synthetix;
  if (adapter) {
    try {
      synthetix.registerEffectAdapter(require(adapter));
    } catch (error) {
      fail(`effect adapter: ${error.message}`);
    }
  }
  for (const pair of sets) {
    const split = pair.indexOf('=');
    const name = pair.slice(0, split);
    const text = pair.slice(split + 1);
    const declared = __page.states.find((state) => state.name === name);
    if (!declared) fail(`no state named ${name}`);
    if (declared.type === 'bool' && text !== 'true' && text !== 'false') fail(`${name} takes true or false`);
    try {
      synthetix.state[name] = parseInputValue({ value: text, checked: text === 'true' }, declared.type, decimalsOf(name));
    } catch (error) {
      fail(`${name}: ${error.message}`);
    }
  }
  const element = findAction(__page.elements, action);
  if (!element) fail(`no button named ${action}`);
  const effectful = element.kind === 'button'
    ? Object.hasOwn(__externs, element.function)
    : element.updates.some((update) => Object.hasOwn(__externs, update.function));
  const before = synthetix.lastTransition;
  const button = findButton(root, action);
  if (!button) fail(`button ${action} is not rendered`);
  button.listeners.click();
  if (!effectful) {
    if (synthetix.lastError) fail(synthetix.lastError.message);
    report(synthetix, null);
  }
  const deadline = Date.now() + waitMs;
  while (synthetix.lastTransition === before && Date.now() < deadline) await sleep(5);
  const transition = synthetix.lastTransition;
  if (transition === before) fail(`no answer from the effect within ${waitMs} ms`);
  if (transition.status !== 'APPLIED') {
    fail(`${transition.status}: ${transition.error?.message ?? 'the effect was cancelled before it answered'}`);
  }
  report(synthetix, transition);
})().catch((error) => { console.error(error); process.exit(1); });
"#;

/// Runs `args[0]` (a project folder) with the page options that follow.
pub fn run(args: &[String]) -> Result<(), String> {
    let project = PathBuf::from(&args[0]);
    let mut action = None;
    let mut adapter = None;
    let mut sets: Vec<&str> = Vec::new();
    let mut wait_ms: u64 = 5000;
    let mut rest = args[1..].iter();
    while let Some(arg) = rest.next() {
        match arg.as_str() {
            "--action" => {
                action = Some(rest.next().ok_or("--action needs a button label")?.as_str());
            }
            "--set" => {
                let pair = rest.next().ok_or("--set needs name=value")?;
                match pair.split_once('=') {
                    Some((name, _)) if !name.is_empty() => sets.push(pair.as_str()),
                    _ => return Err(format!("--set expects name=value, got {pair}")),
                }
            }
            "--adapter" => {
                let path = rest.next().ok_or("--adapter needs a JavaScript file")?;
                adapter = Some(
                    fs::canonicalize(path)
                        .map_err(|error| format!("cannot read adapter {path}: {error}"))?,
                );
            }
            "--wait-ms" => {
                wait_ms = rest
                    .next()
                    .and_then(|text| text.parse().ok())
                    .filter(|ms| (1..=60_000).contains(ms))
                    .ok_or("--wait-ms takes a number from 1 to 60000")?;
            }
            other => return Err(format!("unknown option for a page run: {other}")),
        }
    }
    let action = action.ok_or("a page run needs --action <button label>")?;
    let config = project.join("synthetix.toml");
    if !config.is_file() {
        return Err(format!("no synthetix.toml in {}", project.display()));
    }
    let output = project.join(".synthetix").join("run");
    crate::web::build_with_options(&config, &output, None, false)?;

    let mut node = Command::new("node");
    node.arg("-e")
        .arg(DRIVER)
        .arg("--")
        .arg(action)
        .arg(adapter.as_deref().unwrap_or(Path::new("")))
        .arg(wait_ms.to_string())
        .args(&sets)
        .env("AXIOM_JS", output.join("app.js"))
        .env("AXIOM_WASM", output.join("app.wasm"));
    let result = node
        .output()
        .map_err(|error| format!("a page run needs Node.js on PATH: {error}"))?;
    print!("{}", String::from_utf8_lossy(&result.stdout));
    if !result.status.success() {
        return Err(String::from_utf8_lossy(&result.stderr).trim().to_string());
    }
    Ok(())
}
