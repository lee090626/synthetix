const cp = require("child_process");
const fs = require("fs");
const path = require("path");
const vscode = require("vscode");

let devTerminal;

function activate(context) {
  const diagnostics = vscode.languages.createDiagnosticCollection("synthetix");
  const output = vscode.window.createOutputChannel("Synthetix");
  const validationTimers = new Map();
  const runButton = vscode.window.createStatusBarItem(
    vscode.StatusBarAlignment.Right,
    100
  );
  runButton.text = "$(globe) Run Web";
  runButton.tooltip = "Start Synthetix web development server";
  runButton.command = "synthetix.dev";
  context.subscriptions.push(runButton);
  context.subscriptions.push(diagnostics);
  context.subscriptions.push(output);
  context.subscriptions.push(
    vscode.commands.registerCommand("synthetix.run", () =>
      runContract(diagnostics, output)
    )
  );
  context.subscriptions.push(
    vscode.commands.registerCommand("synthetix.dev", () =>
      startDevServer(output)
    )
  );
  const completionProvider = vscode.languages.registerCompletionItemProvider(
    { language: "axiom" },
    {
      provideCompletionItems(document) {
        return completionItems(document);
      },
    },
    ..."abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ_"
  );
  context.subscriptions.push(completionProvider);
  context.subscriptions.push(
    vscode.languages.registerHoverProvider(
      { language: "axiom" },
      { provideHover: (document, position) => hoverFor(document, position) }
    )
  );
  context.subscriptions.push(
    vscode.languages.registerDocumentFormattingEditProvider(
      { language: "axiom" },
      { provideDocumentFormattingEdits: (document) => formatDocument(document) }
    )
  );
  const scheduleValidation = (document) => {
    if (document.languageId !== "axiom" || document.isDirty) return;
    const key = document.uri.toString();
    clearTimeout(validationTimers.get(key));
    validationTimers.set(
      key,
      setTimeout(() => {
        validationTimers.delete(key);
        validateDocument(document, diagnostics);
      }, 200)
    );
  };
  const updateButton = () => {
    const document = vscode.window.activeTextEditor?.document;
    if (document?.languageId === "axiom") {
      runButton.show();
      scheduleValidation(document);
    } else {
      runButton.hide();
    }
  };
  context.subscriptions.push(
    vscode.window.onDidChangeActiveTextEditor(updateButton)
  );
  context.subscriptions.push(
    vscode.workspace.onDidOpenTextDocument(scheduleValidation)
  );
  context.subscriptions.push(
    vscode.workspace.onDidSaveTextDocument(scheduleValidation)
  );
  context.subscriptions.push(
    vscode.workspace.onDidCloseTextDocument((document) => {
      const key = document.uri.toString();
      clearTimeout(validationTimers.get(key));
      validationTimers.delete(key);
      diagnostics.delete(document.uri);
    })
  );
  updateButton();
}

function completionItems(document) {
  const items = [
    completion("contract", vscode.CompletionItemKind.Keyword),
    completion("program", vscode.CompletionItemKind.Keyword),
    completion("import", vscode.CompletionItemKind.Keyword),
    completion("export", vscode.CompletionItemKind.Keyword),
    completion("from", vscode.CompletionItemKind.Keyword),
    completion("requires", vscode.CompletionItemKind.Keyword),
    completion("when", vscode.CompletionItemKind.Keyword),
    completion("ensures", vscode.CompletionItemKind.Keyword),
    completion("gives", vscode.CompletionItemKind.Keyword),
    completion("if", vscode.CompletionItemKind.Keyword),
    completion("else", vscode.CompletionItemKind.Keyword),
    completion("let", vscode.CompletionItemKind.Keyword),
    completion("in", vscode.CompletionItemKind.Keyword),
    completion("page", vscode.CompletionItemKind.Keyword),
    completion("record", vscode.CompletionItemKind.Keyword),
    completion("component", vscode.CompletionItemKind.Keyword),
    completion("use", vscode.CompletionItemKind.Keyword),
    completion("state", vscode.CompletionItemKind.Keyword),
    completion("show", vscode.CompletionItemKind.Keyword),
    completion("button", vscode.CompletionItemKind.Keyword),
    completion("updates", vscode.CompletionItemKind.Keyword),
    completion("adds", vscode.CompletionItemKind.Keyword),
    completion("runs", vscode.CompletionItemKind.Keyword),
    completion("reducer", vscode.CompletionItemKind.Keyword),
    completion("input", vscode.CompletionItemKind.Keyword),
    completion("placeholder", vscode.CompletionItemKind.Keyword),
    completion("persist", vscode.CompletionItemKind.Keyword),
    completion("list", vscode.CompletionItemKind.Keyword),
    completion("as", vscode.CompletionItemKind.Keyword),
    completion("with", vscode.CompletionItemKind.Keyword),
    completion("u32", vscode.CompletionItemKind.TypeParameter),
    completion("number", vscode.CompletionItemKind.TypeParameter),
    completion("bool", vscode.CompletionItemKind.TypeParameter),
    completion("text", vscode.CompletionItemKind.TypeParameter),
    completion("result", vscode.CompletionItemKind.Variable, "u32 result"),
    completion("answer", vscode.CompletionItemKind.Variable, "u32 result"),
    completion("true", vscode.CompletionItemKind.Value),
    completion("false", vscode.CompletionItemKind.Value),
  ];
  for (const operator of [
    "+%",
    "*%",
    "<<",
    "==",
    "!=",
    "<",
    "<=",
    ">",
    ">=",
    "&&",
    "||",
    "plus",
    "times",
    "shift_left",
    "equals",
    "not_equals",
    "and",
    "or",
  ]) {
    items.push(
      completion(
        operator,
        vscode.CompletionItemKind.Operator,
        "Axiom operator"
      )
    );
  }

  const parameterList = document
    .getText()
    .match(/\b(?:contract|program)\s+\w+\s*\(([^)]*)\)/)?.[1];
  for (const parameter of parameterList?.split(",") ?? []) {
    const name = parameter.trim().split(":")[0];
    if (name) {
      items.push(
        completion(name, vscode.CompletionItemKind.Variable, "u32 parameter")
      );
    }
  }

  const contract = new vscode.CompletionItem(
    "contract",
    vscode.CompletionItemKind.Snippet
  );
  contract.detail = "Axiom contract template";
  contract.insertText = new vscode.SnippetString(
    "contract ${1:name}(${2:x: u32}) -> u32 {\n\tensures result == ${3:x};\n}"
  );
  items.push(contract);

  const program = new vscode.CompletionItem(
    "program template",
    vscode.CompletionItemKind.Snippet
  );
  program.insertText = new vscode.SnippetString(
    "program ${1:name}(${2:x}: number) -> number {\n\tgives answer equals ${2} plus ${3:1};\n}"
  );
  items.push(program);

  const requires = new vscode.CompletionItem(
    "requires clause",
    vscode.CompletionItemKind.Snippet
  );
  requires.insertText = new vscode.SnippetString("requires ${1:x != 0};");
  items.push(requires);

  const when = new vscode.CompletionItem(
    "when clause",
    vscode.CompletionItemKind.Snippet
  );
  when.insertText = new vscode.SnippetString("when ${1:x is_not 0};");
  items.push(when);

  const ensures = new vscode.CompletionItem(
    "ensures clause",
    vscode.CompletionItemKind.Snippet
  );
  ensures.insertText = new vscode.SnippetString("ensures result == ${1:x};");
  items.push(ensures);

  const gives = new vscode.CompletionItem(
    "gives clause",
    vscode.CompletionItemKind.Snippet
  );
  gives.insertText = new vscode.SnippetString(
    "gives answer equals ${1:x};"
  );
  items.push(gives);

  const minimum = new vscode.CompletionItem(
    "minimum expression",
    vscode.CompletionItemKind.Snippet
  );
  minimum.insertText = new vscode.SnippetString(
    "gives answer equals if ${1:a} < ${2:b} { ${1} } else { ${2} };"
  );
  items.push(minimum);

  const letValue = new vscode.CompletionItem(
    "let expression",
    vscode.CompletionItemKind.Snippet
  );
  letValue.insertText = new vscode.SnippetString(
    "gives answer equals let ${1:value} = ${2:x} in ${1};"
  );
  items.push(letValue);

  for (const snippet of [
    ["record template", "record ${1:Todo} {\n\t${2:title}: text;\n\t${3:done}: bool;\n}"],
    ["list template", "list ${1:items} as ${2:item} {\n\tshow ${2};\n}"],
    ["input template", "input ${1:draft} placeholder \"${2:입력하세요}\";"],
    ["reducer template", "reducer ${1:refresh} {\n\tupdates ${2:count} with ${3:increment}(${2});\n}"],
  ]) {
    const item = new vscode.CompletionItem(
      snippet[0],
      vscode.CompletionItemKind.Snippet
    );
    item.insertText = new vscode.SnippetString(snippet[1]);
    items.push(item);
  }

  return items;
}

function completion(label, kind, detail) {
  const item = new vscode.CompletionItem(label, kind);
  item.detail = detail;
  item.insertText = label;
  return item;
}

const hoverDocs = new Map([
  ["number", "32비트 부호 없는 정수입니다. WASM에서는 i32로 실행됩니다."],
  ["u32", "number의 원래 타입 이름입니다. 모든 연산은 2^32에서 감쌉니다."],
  ["bool", "true 또는 false입니다. 검증 계약에서는 0 또는 1로 방출됩니다."],
  ["text", "웹 입력과 화면 문구에 사용하는 문자열입니다."],
  ["record", "여러 필드를 하나로 묶는 화면 데이터 타입입니다."],
  ["list", "같은 타입의 값을 여러 개 담는 화면 데이터 타입입니다."],
  ["reducer", "검증된 순수 계약 여러 개로 상태를 함께 갱신합니다."],
  ["persist", "해당 상태를 브라우저 localStorage에 저장합니다."],
  ["input", "text 상태와 연결되는 브라우저 입력창입니다."],
]);

function hoverFor(document, position) {
  const range = document.getWordRangeAtPosition(position, /[A-Za-z_][A-Za-z0-9_]*/);
  if (!range) return undefined;
  const word = document.getText(range);
  const source = document.getText();
  let detail = hoverDocs.get(word);
  const contract = source.match(
    new RegExp(`\\b(?:contract|program)\\s+${escapeRegExp(word)}\\s*\\(([^)]*)\\)\\s*->\\s*([A-Za-z_][A-Za-z0-9_]*)`)
  );
  if (contract) detail = `검증된 순수 계약: (${contract[1] || "인자 없음"}) → ${contract[2]}`;
  const state = source.match(
    new RegExp(`\\bstate\\s+${escapeRegExp(word)}\\s*:\\s*([A-Za-z_][A-Za-z0-9_]*(?:<[^>]+>)?)`)
  );
  if (state) detail = `페이지 상태: ${state[1]}`;
  if (!detail) return undefined;
  return new vscode.Hover(new vscode.MarkdownString(`**${word}**  \n${detail}`), range);
}

function escapeRegExp(value) {
  return value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

function formatDocument(document) {
  const edits = [];
  let indent = 0;
  for (let index = 0; index < document.lineCount; index += 1) {
    const line = document.lineAt(index);
    const trimmed = line.text.trim();
    if (!trimmed) {
      edits.push(vscode.TextEdit.replace(line.range, ""));
      continue;
    }
    if (trimmed.startsWith("}")) indent = Math.max(0, indent - 1);
    edits.push(
      vscode.TextEdit.replace(
        line.range,
        `${"  ".repeat(indent)}${trimmed}`
      )
    );
    if (trimmed.endsWith("{")) indent += 1;
  }
  return edits;
}

function validateDocument(document, diagnostics) {
  const workspace = vscode.workspace.getWorkspaceFolder(document.uri);
  const workspacePath =
    workspace?.uri.fsPath ?? path.dirname(document.uri.fsPath);
  const configuredPath = vscode.workspace
    .getConfiguration("synthetix")
    .get("axiomPath", "axiom");
  const command = resolveAxiomPath(configuredPath, workspacePath);
  const projectConfig = findProjectConfig(workspacePath);
  const cwd = projectConfig ? path.dirname(projectConfig) : workspacePath;
  const args = projectConfig ? ["check"] : ["check", document.uri.fsPath];
  const child = cp.spawn(command, args, {
    cwd,
    shell: false,
  });
  let stderr = "";
  child.stderr.on("data", (data) => {
    stderr += data.toString();
  });
  child.on("error", (error) => {
    setDiagnostic(
      diagnostics,
      document,
      `${error.message}. Run cargo build -p axiom-cli first.`
    );
  });
  child.on("close", (code) => {
    if (code === 0) {
      diagnostics.delete(document.uri);
    } else {
      setDiagnostic(
        diagnostics,
        document,
        stderr.trim() || `axiom exited with code ${code}`
      );
    }
  });
}

function startDevServer(output) {
  const document = vscode.window.activeTextEditor?.document;
  if (!document || document.languageId !== "axiom") {
    vscode.window.showErrorMessage("Open an .ax file first.");
    return;
  }

  const workspace = vscode.workspace.getWorkspaceFolder(document.uri);
  const workspacePath = workspace?.uri.fsPath ?? path.dirname(document.uri.fsPath);
  const projectConfig = findProjectConfig(workspacePath);
  if (!projectConfig) {
    vscode.window.showErrorMessage(
      "synthetix.toml not found. Create a project config before running the web app."
    );
    return;
  }

  if (devTerminal && devTerminal.exitStatus === undefined) {
    devTerminal.show();
    return;
  }

  const configuredPath = vscode.workspace
    .getConfiguration("synthetix")
    .get("axiomPath", "axiom");
  const command = resolveAxiomPath(configuredPath, path.dirname(projectConfig));
  devTerminal = vscode.window.createTerminal("Synthetix Dev");
  devTerminal.show(true);
  devTerminal.sendText(`${shellQuote(command)} dev`, true);
  output.appendLine(`Synthetix: axiom dev (${projectConfig})`);
  output.show(true);
}

async function runContract(diagnostics, output) {
  const document = vscode.window.activeTextEditor?.document;
  if (!document || document.languageId !== "axiom") {
    vscode.window.showErrorMessage("Open an .ax file first.");
    return;
  }

  if (!(await document.save())) {
    vscode.window.showErrorMessage("Could not save the .ax file.");
    return;
  }

  const workspace = vscode.workspace.getWorkspaceFolder(document.uri);
  const workspacePath = workspace?.uri.fsPath ?? path.dirname(document.uri.fsPath);
  const configuredPath = vscode.workspace
    .getConfiguration("synthetix")
    .get("axiomPath", "axiom");
  const command = resolveAxiomPath(configuredPath, workspacePath);
  const outputPath = wasmPath(document.uri.fsPath, workspacePath);
  try {
    fs.mkdirSync(path.dirname(outputPath), { recursive: true });
  } catch (error) {
    setError(
      diagnostics,
      document,
      `Could not create build directory: ${error.message}`
    );
    return;
  }
  const parameterCount = getParameterCount(document);
  const inputs = [];
  for (let index = 0; index < parameterCount; index += 1) {
    const input = await vscode.window.showInputBox({
      prompt: `u32 input ${index + 1}/${parameterCount}`,
      value:
        index === 0
          ? vscode.workspace
              .getConfiguration("synthetix")
              .get("runInput", "5")
          : "0",
      validateInput: validateU32,
    });
    if (input === undefined) return;
    inputs.push(input);
  }
  const args = ["run", document.uri.fsPath, "--", ...inputs, "-o", outputPath];

  const child = cp.spawn(command, args, {
    cwd: workspacePath,
    shell: false,
  });
  output.clear();
  output.appendLine(`Synthetix: ${path.basename(document.uri.fsPath)}`);
  output.appendLine(`command: ${command} ${args.join(" ")}`);
  output.show(true);
  child.stdout.on("data", (data) => output.append(data.toString()));
  let stderr = "";
  child.stderr.on("data", (data) => {
    stderr += data.toString();
    output.append(data.toString());
  });
  child.on("error", (error) => {
    output.appendLine(`error: ${error.message}`);
    setError(diagnostics, document, `${error.message}. Run cargo build -p axiom-cli first.`);
  });
  child.on("close", (code) => {
    if (code === 0) {
      diagnostics.delete(document.uri);
      output.appendLine(`finished: ${path.basename(outputPath)}`);
      vscode.window.showInformationMessage(
        `Synthetix ran ${path.basename(document.uri.fsPath)}.`
      );
    } else {
      output.appendLine(`axiom exited with code ${code}.`);
      setError(diagnostics, document, stderr.trim() || `axiom exited with code ${code}.`);
    }
  });
}

function getParameterCount(document) {
  const parameterList = document
    .getText()
    .match(/\b(?:contract|program)\s+\w+\s*\(([^)]*)\)/)?.[1]?.trim();
  return parameterList ? parameterList.split(",").length : 0;
}

function validateU32(value) {
  if (!/^(0|[1-9]\d*)$/.test(value)) {
    return "Enter an unsigned 32-bit integer.";
  }
  if (Number(value) > 0xffffffff) {
    return "Value must be at most 4294967295.";
  }
  return undefined;
}

function wasmPath(sourcePath, workspacePath) {
  const relativePath = path.relative(workspacePath, sourcePath);
  const safePath =
    relativePath.startsWith("..") || path.isAbsolute(relativePath)
      ? path.basename(sourcePath)
      : relativePath;
  const fileName = path.basename(safePath);
  const outputName =
    fileName === ".ax"
      ? "out.wasm"
      : fileName.endsWith(".ax")
        ? `${fileName.slice(0, -3)}.wasm`
        : `${fileName}.wasm`;
  const relativeOutput = path.join(path.dirname(safePath), outputName);
  return path.join(workspacePath, ".synthetix", "build", relativeOutput);
}

function findProjectConfig(startPath) {
  let current = fs.statSync(startPath).isFile()
    ? path.dirname(startPath)
    : startPath;
  for (;;) {
    const candidate = path.join(current, "synthetix.toml");
    if (fs.existsSync(candidate)) return candidate;
    const parent = path.dirname(current);
    if (parent === current) return undefined;
    current = parent;
  }
}

function shellQuote(value) {
  if (process.platform === "win32") {
    return `"${value.replaceAll('"', '\\"')}"`;
  }
  return `'${value.replaceAll("'", "'\\''")}'`;
}

function resolveAxiomPath(configuredPath, workspacePath) {
  if (configuredPath !== "axiom") {
    return configuredPath;
  }
  const bundled = bundledAxiomPath();
  if (bundled) {
    return bundled;
  }
  let current = workspacePath;
  for (let depth = 0; depth < 5; depth += 1) {
    for (const candidate of [
      path.join(current, "target", "debug", "axiom"),
      path.join(current, "target", "release", "axiom"),
    ]) {
      if (fs.existsSync(candidate)) {
        return candidate;
      }
    }
    const parent = path.dirname(current);
    if (parent === current) break;
    current = parent;
  }
  return configuredPath;
}

function bundledAxiomPath() {
  const executable = process.platform === "win32" ? "axiom.exe" : "axiom";
  const candidate = path.join(
    __dirname,
    "bin",
    `${process.platform}-${process.arch}`,
    executable
  );
  return fs.existsSync(candidate) ? candidate : undefined;
}

function setError(diagnostics, document, message) {
  setDiagnostic(diagnostics, document, message);
  vscode.window.showErrorMessage(message);
}

function setDiagnostic(diagnostics, document, message) {
  const cleanMessage = message.replace(/^error:\s*/, "");
  const match = cleanMessage.match(/(?:^|\n).*?(\d+):(\d+):\s*(.*)/);
  let range;
  let diagnosticMessage = cleanMessage;
  if (match) {
    const line = Math.min(
      document.lineCount - 1,
      Math.max(0, Number(match[1]) - 1)
    );
    const lineEnd = document.lineAt(line).range.end.character;
    range = new vscode.Range(
      new vscode.Position(line, 0),
      new vscode.Position(line, lineEnd)
    );
    diagnosticMessage = match[3];
  } else {
    const end = document.lineAt(document.lineCount - 1).range.end;
    range = new vscode.Range(new vscode.Position(0, 0), end);
  }
  diagnostics.set(document.uri, [
    new vscode.Diagnostic(
      range,
      diagnosticMessage,
      vscode.DiagnosticSeverity.Error
    ),
  ]);
}

function deactivate() {}

module.exports = { activate, deactivate };
