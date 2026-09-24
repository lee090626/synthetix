const assert = require("node:assert/strict");
const fs = require("node:fs");
const http = require("node:http");
const path = require("node:path");
const { performance } = require("node:perf_hooks");
const vm = require("node:vm");

const dist = path.resolve(process.argv[2] ?? "examples/v6-release/dist");
const nativeFetch = global.fetch;
const requested = new Set();

class Element {
  constructor(tagName) {
    this.tagName = tagName;
    this.children = [];
    this.listeners = {};
    this.dataset = {};
    this.hidden = true;
    this.textContent = "";
  }
  append(...items) { this.children.push(...items); }
  replaceChildren() { this.children = []; }
  addEventListener(name, callback) { this.listeners[name] = callback; }
  click() { this.listeners.click?.({ target: this }); }
}

async function run() {
  const server = http.createServer((request, response) => {
    const pathname = new URL(request.url, "http://127.0.0.1").pathname;
    const relative = pathname === "/" ? "index.html" : decodeURIComponent(pathname.slice(1));
    const file = path.resolve(dist, relative);
    requested.add(pathname);
    if (!file.startsWith(`${dist}${path.sep}`)) {
      response.writeHead(400).end("invalid path");
      return;
    }
    fs.readFile(file, (error, contents) => {
      if (error) {
        response.writeHead(404).end("not found");
        return;
      }
      const contentType = file.endsWith(".wasm")
        ? "application/wasm"
        : file.endsWith(".css")
          ? "text/css"
          : "text/html; charset=utf-8";
      response.writeHead(200, { "content-type": contentType }).end(contents);
    });
  });
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  const address = server.address();
  const origin = `http://127.0.0.1:${address.port}`;

  try {
    const response = await nativeFetch(`${origin}/`);
    assert.equal(response.status, 200);
    const html = await response.text();
    assert.match(html, /app\.config\.js/);
    assert.match(html, /app\.js/);

    const [configResponse, scriptResponse, cssResponse, wasmResponse, proofResponse] =
      await Promise.all(["app.config.js", "app.js", "app.css", "app.wasm", "app.proof.json"]
        .map((file) => nativeFetch(`${origin}/${file}`)));
    for (const asset of [configResponse, scriptResponse, cssResponse, wasmResponse, proofResponse])
      assert.equal(asset.status, 200);
    const [config, script, _css, wasmBuffer, proofText] = await Promise.all([
      configResponse.text(),
      scriptResponse.text(),
      cssResponse.text(),
      wasmResponse.arrayBuffer(),
      proofResponse.text(),
    ]);
    const proof = JSON.parse(proofText);
    assert.equal(proof.verification.scope, "WASM_CONTRACTS_ONLY");
    assert.equal(proof.verification.proof_result, "VERIFIED");
    const contractNames = proof.contracts.map((contract) => contract.name);
    for (const name of ["sum4", "plus_one", "sum_to", "increment"]) {
      const contract = proof.contracts.find((candidate) => candidate.name === name);
      assert.ok(contract, `proof artifact omitted ${name}`);
      assert.equal(contract.proof_result, "VERIFIED");
    }
    const boundary = proof.external_boundaries.balance;
    assert.equal(boundary.source, "api.ax");
    assert.deepEqual(boundary.params, ["u32"]);
    assert.equal(boundary.result, "u32");
    assert.equal(boundary.effect, "HOST_EFFECT");
    assert.equal(boundary.verification_status, "UNVERIFIED");
    assert.equal(boundary.implementation_verified, false);
    assert.equal(boundary.trust_boundary, "EXTERNAL_IMPLEMENTATION");
    assert.equal(boundary.response_validation, "DECLARED_TYPE_ONLY");
    assert.equal(boundary.audit_metadata.status, "NOT_PROVIDED");
    assert.ok(boundary.assumptions.length >= 2);

    const wasm = Buffer.from(wasmBuffer);
    assert.deepEqual([...wasm.subarray(0, 4)], [0, 97, 115, 109]);
    const instance = new WebAssembly.Instance(new WebAssembly.Module(wasm), {});
    const exports = instance.exports;
    const arrayTotal = exports.sum4(1, 2, 3, 4);
    assert.equal(arrayTotal, 10);
    const modularTotal = exports.plus_one(arrayTotal);
    assert.equal(modularTotal, 11);
    const recursiveTotal = exports.sum_to(16);
    assert.equal(recursiveTotal, 16);
    assert.equal(exports.balance, undefined);

    const root = new Element("main");
    const error = new Element("div");
    const storage = new Map();
    global.window = {};
    global.location = { reload() {} };
    global.localStorage = {
      getItem(key) { return storage.get(key) ?? null; },
      setItem(key, value) { storage.set(key, String(value)); },
    };
    global.document = {
      querySelector(selector) { return selector === "#app" ? root : error; },
      createElement(tagName) { return new Element(tagName); },
    };
    global.setInterval = () => 0;
    global.fetch = (input, options) => nativeFetch(new URL(input, origin), options);
    const bootStarted = performance.now();
    vm.runInThisContext(config);
    vm.runInThisContext(script);
    for (let attempt = 0; attempt < 100 && !window.synthetix?.state; attempt++)
      await new Promise((resolve) => setTimeout(resolve, 0));
    assert.ok(window.synthetix?.state, "generated browser runtime did not initialize");
    const runtimeBootTimeMs = performance.now() - bootStarted;

    const incremented = window.synthetix.call("increment", [arrayTotal]);
    assert.equal(incremented, 11);
    const button = root.children.find((element) => element.tagName === "button");
    assert.ok(button, "generated page did not render its state transition");
    const transitionStarted = performance.now();
    button.click();
    const transitionTimeMs = performance.now() - transitionStarted;
    assert.equal(window.synthetix.state.count, 1);

    window.synthetix.registerEffectAdapter(async (request) => {
      assert.equal(request.name, "balance");
      assert.equal(request.source, "api.ax");
      assert.equal(request.inputs[0], incremented);
      assert.equal(request.effect, "HOST_EFFECT");
      assert.equal(request.verificationStatus, "UNVERIFIED");
      assert.equal(request.trustBoundary, "EXTERNAL_IMPLEMENTATION");
      return request.inputs[0] * 2;
    });
    const effectStarted = performance.now();
    assert.equal(await window.synthetix.requestExtern("balance", [incremented], "remote"), 22);
    const effectTimeMs = performance.now() - effectStarted;
    assert.equal(window.synthetix.state.remote, 22);
    assert.equal(window.synthetix.lastEffect.status, "SUCCEEDED");

    const envelope = JSON.parse(storage.get("synthetix:Summary:state"));
    assert.equal(envelope.values.count, 1);
    assert.equal(envelope.values.remote, 22);
    window.synthetix.state.remote = 0;
    const restoreStarted = performance.now();
    const restored = await window.synthetix.restore();
    const restoreTimeMs = performance.now() - restoreStarted;
    assert.equal(restored.remote, 22);
    const rendered = root.children.find((element) =>
      element.tagName === "output" && element.dataset.state === "remote");
    assert.equal(rendered.textContent, "22");

    for (const asset of ["/", "/app.config.js", "/app.js", "/app.css", "/app.wasm", "/app.proof.json"])
      assert.ok(requested.has(asset), `static server did not serve ${asset}`);
    process.stdout.write(JSON.stringify({
      status: "PASS",
      array_sum: arrayTotal,
      modular_call: modularTotal,
      recursion: recursiveTotal,
      increment: incremented,
      api: 22,
      persisted: true,
      runtime_boot_time_ms: runtimeBootTimeMs,
      transition_time_ms: transitionTimeMs,
      effect_time_ms: effectTimeMs,
      persistence_restore_time_ms: restoreTimeMs,
      wasm_bytes: wasm.length,
    }) + "\n");
  } finally {
    await new Promise((resolve) => server.close(resolve));
  }
}

run().catch((error) => {
  console.error(error);
  process.exitCode = 1;
});
