const assert = require("node:assert/strict");
const Module = require("node:module");

const registeredCommands = [];
const disposable = () => ({ dispose() {} });
const diagnostics = { set() {}, delete() {}, dispose() {} };
const output = { appendLine() {}, dispose() {} };
const vscode = {
  StatusBarAlignment: { Right: 2 },
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
    onDidOpenTextDocument: disposable,
    onDidSaveTextDocument: disposable,
    onDidCloseTextDocument: disposable,
  },
};

const load = Module._load;
Module._load = function (request, parent, isMain) {
  return request === "vscode" ? vscode : load.call(this, request, parent, isMain);
};

const extension = require("./extension.js");
const context = { subscriptions: [] };
extension.activate(context);
assert.deepEqual(registeredCommands.sort(), ["synthetix.dev", "synthetix.run"]);
assert.ok(context.subscriptions.length >= 8);
process.stdout.write("VS Code extension activation registered its commands and providers\n");
