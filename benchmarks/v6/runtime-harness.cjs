const fs = require('node:fs');
const vm = require('node:vm');
const path = require('node:path');

const dist = path.resolve(process.argv[2]);
const transitions = Number(process.argv[3]);
const bytes = fs.readFileSync(path.join(dist, 'app.wasm'));
const storage = new Map();

class Element {
  constructor(tagName) {
    this.tagName = tagName;
    this.children = [];
    this.listeners = {};
    this.dataset = {};
    this.textContent = '';
    this.hidden = true;
  }
  append(...items) { this.children.push(...items); }
  replaceChildren(...items) { this.children = [...items]; }
  addEventListener(name, callback) { this.listeners[name] = callback; }
  click() { return this.listeners.click?.(); }
}

const root = new Element('main');
const error = new Element('div');
global.window = {};
global.localStorage = {
  getItem(key) { return storage.get(key) ?? null; },
  setItem(key, value) { storage.set(key, String(value)); },
};
global.document = {
  querySelector(selector) { return selector === '#app' ? root : error; },
  createElement(tag) { return new Element(tag); },
};
global.setInterval = () => 0;
global.fetch = async (url) => url === './app.wasm'
  ? { ok: true, arrayBuffer: async () => bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength) }
  : { ok: false, text: async () => '' };

const now = () => process.hrtime.bigint();
const millis = (start) => Number(now() - start) / 1e6;

(async () => {
  const bootStarted = now();
  vm.runInThisContext(fs.readFileSync(path.join(dist, 'app.config.js'), 'utf8'));
  vm.runInThisContext(fs.readFileSync(path.join(dist, 'app.js'), 'utf8'));
  for (let attempt = 0; attempt < 1000 && !window.synthetix; attempt++)
    await new Promise((resolve) => setTimeout(resolve, 0));
  if (!window.synthetix) throw new Error('generated static runtime did not instantiate WASM');
  const bootMs = millis(bootStarted);

  const transitionStarted = now();
  for (let index = 0; index < transitions; index++) {
    const button = root.children.find((element) => element.tagName === 'button' && element.textContent === 'step');
    if (!button) throw new Error('generated state transition button is missing');
    button.click();
  }
  const transitionMs = millis(transitionStarted);
  const state = window.synthetix.state;
  const saved = storage.get('synthetix:Scale:state');
  if (!saved || state.count !== transitions) throw new Error('state update or persistence failed');
  const envelope = JSON.parse(saved);
  const listSize = envelope.values.values.length;
  const textBytes = Buffer.byteLength(envelope.values.note, 'utf8');

  const restoreStarted = now();
  const restored = await window.synthetix.restore();
  const restoreMs = millis(restoreStarted);
  if (restored.count !== transitions || restored.values.length !== listSize || Buffer.byteLength(restored.note, 'utf8') !== textBytes)
    throw new Error('persisted application state did not round-trip');

  process.stdout.write(JSON.stringify({
    transitions,
    listSize,
    textBytes,
    bootMs,
    transitionMs,
    transitionMsPerAction: transitions ? transitionMs / transitions : 0,
    restoreMs,
    storageBytes: Buffer.byteLength(saved, 'utf8'),
    renderedListItems: root.children.find((element) => element.tagName === 'ul')?.children.length ?? 0,
  }));
})().catch((error) => {
  console.error(error);
  process.exitCode = 1;
});
