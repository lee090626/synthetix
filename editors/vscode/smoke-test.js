const assert = require("node:assert/strict");
const { EventEmitter } = require("node:events");
const Module = require("node:module");

const registeredCommands = [];
const subscriptions = [];
const outputLines = [];
const diagnosticsByUri = new Map();
const events = {};
const disposable = () => ({ dispose() {} });
const output = {
  appendLine(line) { outputLines.push(line); },
  append(value) { outputLines.push(value); },
  clear() {},
  show() {},
  dispose() {},
};
const diagnostics = {
  set(uri, values) { diagnosticsByUri.set(uri.toString(), values); },
  delete(uri) { diagnosticsByUri.delete(uri.toString()); },
  dispose() {},
};
class Position {
  constructor(line, character) { this.line = line; this.character = character; }
}
class Range {
  constructor(start, end) { this.start = start; this.end = end; }
}
class Diagnostic {
  constructor(range, message, severity) { Object.assign(this, { range, message, severity }); }
}
const vscode = {
  StatusBarAlignment: { Right: 2 },
  DiagnosticSeverity: { Error: 0 },
  Position,
  Range,
  Diagnostic,
  window: {
    activeTextEditor: undefined,
    createOutputChannel: () => output,
    createStatusBarItem: () => ({ ...disposable(), show() {}, hide() {} }),
    onDidChangeActiveTextEditor: disposable,
  },
  commands: {
    registerCommand(name) {
      registeredCommands.push(name);
      return disposable();
    },
  },
  languages: {
    createDiagnosticCollection: () => diagnostics,
    registerCompletionItemProvider: disposable,
    registerHoverProvider: disposable,
    registerDocumentFormattingEditProvider: disposable,
  },
  workspace: {
    onDidOpenTextDocument(callback) { events.open = callback; return disposable(); },
    onDidSaveTextDocument(callback) { events.save = callback; return disposable(); },
    onDidCloseTextDocument(callback) { events.close = callback; return disposable(); },
    getWorkspaceFolder: () => undefined,
    getConfiguration: () => ({ get: (_key, fallback) => fallback }),
  },
};

const children = [];
const load = Module._load;
Module._load = function (request, parent, isMain) {
  if (request === "vscode") return vscode;
  if (request === "child_process") {
    return { spawn() {
      const child = new EventEmitter();
      child.stdout = new EventEmitter();
      child.stderr = new EventEmitter();
      child.kill = () => { child.killed = true; };
      children.push(child);
      return child;
    } };
  }
  return load.call(this, request, parent, isMain);
};

const extension = require("./extension.js");
extension.activate({ subscriptions });
assert.deepEqual(registeredCommands.sort(), ["synthetix.dev", "synthetix.run"]);
assert.ok(subscriptions.length >= 8);
assert.equal(typeof events.open, "function");

const text = "contract broken(x: u32) -> u32 {\n  ensures result == ;\n}\n";
const lines = text.split("\n");
const document = {
  languageId: "axiom",
  isDirty: false,
  uri: { fsPath: "/tmp/broken.ax", toString: () => "file:///tmp/broken.ax" },
  getText: () => text,
  get lineCount() { return lines.length; },
  lineAt(line) {
    const length = lines[line].length;
    return { range: { end: new Position(line, length) } };
  },
};

const wait = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
async function validate(stderr) {
  events.open(document);
  await wait(225);
  const child = children.at(-1);
  assert.ok(child, "document event did not spawn axiom");
  child.stderr.emit("data", Buffer.from(stderr));
  child.emit("close", 1);
  return diagnosticsByUri.get(document.uri.toString())[0];
}

(async () => {
  const parserError = "error: INVALID: 2:20: expected expression";
  const parserDiagnostic = await validate(parserError);
  assert.ok(outputLines.includes("contract_check: INVALID"));
  assert.equal(parserDiagnostic.message, "expected expression");
  assert.deepEqual(parserDiagnostic.range, new Range(new Position(1, 0), new Position(1, lines[1].length)));

  const simpleDiagnostic = await validate("error: INVALID: line 2: type error: requires clause must be boolean\n  2 |   ensures result == ;");
  assert.equal(simpleDiagnostic.message, "type error: requires clause must be boolean");
  assert.deepEqual(simpleDiagnostic.range, new Range(new Position(1, 0), new Position(1, lines[1].length)));

  const preconditionError = "UNSATISFIABLE_PRECONDITION: requires clauses are contradictory";
  const preconditionDiagnostic = await validate(preconditionError);
  assert.ok(outputLines.includes("contract_check: UNSATISFIABLE_PRECONDITION"));
  assert.equal(preconditionDiagnostic.message, preconditionError);
  assert.deepEqual(preconditionDiagnostic.range, new Range(new Position(0, 0), new Position(lines.length - 1, 0)));
  // An older check that finishes after a newer one must not overwrite its result.
  events.save(document);
  await wait(225);
  const older = children.at(-1);
  events.save(document);
  await wait(225);
  const newer = children.at(-1);
  assert.ok(older.killed);
  newer.emit("close", 0);
  older.stderr.emit("data", Buffer.from(parserError));
  older.emit("close", 1);
  assert.equal(diagnosticsByUri.has(document.uri.toString()), false);

  // Closing the document stops its check and its late result is dropped.
  events.save(document);
  await wait(225);
  const closing = children.at(-1);
  events.close(document);
  assert.ok(closing.killed);
  closing.emit("close", 1);
  assert.equal(diagnosticsByUri.has(document.uri.toString()), false);

  process.stdout.write("VS Code extension preserves parser and precondition diagnostic handoff\n");
})().catch((error) => {
  console.error(error);
  process.exitCode = 1;
});
