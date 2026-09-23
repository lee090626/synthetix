use std::{fs, path::Path, process::Command, time::SystemTime};

fn mul8_fixture() -> String {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/basics/mul8.ax")
        .display()
        .to_string()
}

#[test]
fn app_run_executes_mul8_through_the_application_boundary() {
    let output = Command::new(env!("CARGO_BIN_EXE_axiom"))
        .args(["app-run", &mul8_fixture(), "--", "5"])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("boundary: APPLICATION -> VERIFIED_CORE -> APPLICATION"));
    assert!(stdout.contains("proof_result: VERIFIED"));
    assert!(stdout.contains("result: Integer(40)"));
}

#[test]
fn app_run_rejects_invalid_input_outside_the_proof_result_model() {
    let output = Command::new(env!("CARGO_BIN_EXE_axiom"))
        .args(["app-run", &mul8_fixture(), "4294967296"])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("outside u32"), "stderr: {stderr}");
    assert!(!stderr.contains("COUNTEREXAMPLE"), "stderr: {stderr}");
}

#[test]
fn app_run_rejects_text_instead_of_coercing_it_to_a_core_integer() {
    let output = Command::new(env!("CARGO_BIN_EXE_axiom"))
        .args(["app-run", &mul8_fixture(), "five"])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("expected u32, received text"),
        "stderr: {stderr}"
    );
    assert!(!stderr.contains("COUNTEREXAMPLE"), "stderr: {stderr}");
}

#[test]
fn v5_release_path_keeps_proof_wasm_state_effect_and_static_config_together() {
    let root = std::env::temp_dir().join(format!(
        "synthetix-v5-release-{}",
        SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    fs::write(
        root.join("api.ax"),
        "export extern fn balance(account: u32) -> u32;\n",
    )
    .unwrap();
    fs::write(
        root.join("main.ax"),
        r#"import { balance } from "./api.ax";
contract increment(value: u32) -> u32 { ensures result == value +% 1; }
page Account {
  state count: number = 0;
  persist count;
  show count;
  button "increment" updates count with increment(count);
}
"#,
    )
    .unwrap();
    fs::write(
        root.join("synthetix.toml"),
        "[project]\nentry = \"main.ax\"\ntarget = \"web\"\n",
    )
    .unwrap();

    let check = Command::new(env!("CARGO_BIN_EXE_axiom"))
        .args(["check"])
        .current_dir(&root)
        .output()
        .unwrap();
    assert!(
        check.status.success(),
        "check failed: {}",
        String::from_utf8_lossy(&check.stderr)
    );
    assert!(
        String::from_utf8(check.stdout)
            .unwrap()
            .contains("proof_result: VERIFIED")
    );

    let build = Command::new(env!("CARGO_BIN_EXE_axiom"))
        .args(["build"])
        .current_dir(&root)
        .output()
        .unwrap();
    assert!(
        build.status.success(),
        "build failed: {}",
        String::from_utf8_lossy(&build.stderr)
    );
    let dist = root.join("dist");
    for file in [
        "index.html",
        "app.config.js",
        "app.js",
        "app.wasm",
        "app.css",
        "app.proof.json",
    ] {
        assert!(dist.join(file).is_file(), "missing static output {file}");
    }
    let proof = fs::read_to_string(dist.join("app.proof.json")).unwrap();
    assert!(proof.contains("\"name\":\"increment\""));
    assert!(proof.contains("\"source\":\"main.ax\""));
    assert!(proof.contains("\"proof_result\":\"VERIFIED\""));
    assert!(!proof.contains("balance"));
    let html = fs::read_to_string(dist.join("index.html")).unwrap();
    assert!(html.find("app.config.js").unwrap() < html.find("app.js").unwrap());
    fs::write(
        dist.join("app.config.js"),
        "window.synthetixConfig = { environment: 'release-test', apiBaseUrl: 'https://api.example.invalid' };\n",
    )
    .unwrap();

    let runtime = r#"
const fs = require('node:fs');
const vm = require('node:vm');
const bytes = fs.readFileSync(process.env.AXIOM_TEST_WASM);
const proof = JSON.parse(fs.readFileSync(process.env.AXIOM_TEST_PROOF, 'utf8'));
if (proof.proof_result !== 'VERIFIED'
  || !proof.contracts.some((contract) => contract.name === 'increment'
    && contract.source === 'main.ax' && contract.proof_result === 'VERIFIED')
  || proof.contracts.some((contract) => contract.name === 'balance'))
  throw new Error('proof manifest does not match the verified project');
const wasmExportNames = WebAssembly.Module.exports(new WebAssembly.Module(bytes)).map((entry) => entry.name);
if (!wasmExportNames.includes('increment') || wasmExportNames.includes('balance')) throw new Error(`unexpected WASM exports: ${wasmExportNames}`);
class Element {
  constructor(tagName) { this.tagName = tagName; this.children = []; this.listeners = {}; this.dataset = {}; this.hidden = true; this.textContent = ''; }
  append(...items) { this.children.push(...items); }
  replaceChildren() { this.children = []; }
  addEventListener(name, callback) { this.listeners[name] = callback; }
  click() { this.listeners.click?.({ target: this }); }
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
vm.runInThisContext(fs.readFileSync(process.env.AXIOM_TEST_CONFIG, 'utf8'));
vm.runInThisContext(fs.readFileSync(process.env.AXIOM_TEST_JS, 'utf8'));
(async () => {
  for (let attempt = 0; attempt < 100 && !window.synthetix; attempt++)
    await new Promise((resolve) => setTimeout(resolve, 0));
  if (!window.synthetix) throw new Error('static runtime did not initialize');
  const button = root.children.find((element) => element.tagName === 'button' && element.textContent === 'increment');
  if (!button) throw new Error('generated page is missing the verified action');
  button.click();
  const stateAfterVerifiedAction = window.synthetix.state.count;
  const verifiedCall = window.synthetix.call('increment', [41]);
  window.synthetix.registerEffectAdapter(async (request) => {
    if (request.name !== 'balance' || request.source !== 'api.ax' || request.responseType !== 'u32')
      throw new Error('typed API request metadata mismatch');
    if (window.synthetix.config.apiBaseUrl !== 'https://api.example.invalid')
      throw new Error('deployment config unavailable to API adapter');
    return request.inputs[0] * 3;
  });
  const apiResult = await window.synthetix.requestExtern('balance', [7], 'count');
  const envelope = JSON.parse(storage.get('synthetix:Account:state'));
  if (envelope.version !== window.synthetix.storageSchemaVersion || envelope.values.count !== 21)
    throw new Error('API state update was not saved in the versioned storage envelope');
  window.synthetix.state.count = 0;
  const restored = await window.synthetix.restore();
  const rendered = root.children.find((element) => element.tagName === 'output' && element.dataset.state === 'count');
  process.stdout.write([
    stateAfterVerifiedAction,
    verifiedCall,
    apiResult,
    restored.count,
    window.synthetix.state.count,
    rendered.textContent,
    window.synthetix.lastEffect.status,
    window.synthetix.config.environment,
  ].join('|'));
})().catch((failure) => { console.error(failure); process.exitCode = 1; });
"#;
    let runtime_result = Command::new("node")
        .arg("-e")
        .arg(runtime)
        .env("AXIOM_TEST_CONFIG", dist.join("app.config.js"))
        .env("AXIOM_TEST_JS", dist.join("app.js"))
        .env("AXIOM_TEST_WASM", dist.join("app.wasm"))
        .env("AXIOM_TEST_PROOF", dist.join("app.proof.json"))
        .output()
        .expect("Node.js is required for the v5 release bundle smoke test");
    assert!(
        runtime_result.status.success(),
        "bundle runtime failed: {}",
        String::from_utf8_lossy(&runtime_result.stderr)
    );
    assert_eq!(
        String::from_utf8(runtime_result.stdout).unwrap(),
        "1|42|21|21|21|21|SUCCEEDED|release-test"
    );
    fs::remove_dir_all(root).unwrap();
}
