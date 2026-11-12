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
        validateDocument(document, diagnostics, output);
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
    completion("need", vscode.CompletionItemKind.Keyword, "need condition"),
    completion("is", vscode.CompletionItemKind.Keyword, "x is 1"),
    completion("not", vscode.CompletionItemKind.Keyword, "x is not 1"),
    completion("and", vscode.CompletionItemKind.Keyword),
    completion("or", vscode.CompletionItemKind.Keyword),
    completion("from", vscode.CompletionItemKind.Keyword, "from file: rule"),
    completion("page", vscode.CompletionItemKind.Keyword, "page Name"),
    completion("save", vscode.CompletionItemKind.Keyword, "save a = 0"),
    completion("ask", vscode.CompletionItemKind.Keyword, 'ask a "Label"'),
    completion("show", vscode.CompletionItemKind.Keyword, "show a"),
    completion("button", vscode.CompletionItemKind.Keyword, 'button "Label" -> a = rule(...)'),
  ];
  for (const operator of ["->", "+", "-", "*", "<", "<=", ">", ">="]) {
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
    .match(/^[A-Za-z_]\w*\s*\(([^)]*)\)\s*$/m)?.[1];
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

  const functionTemplate = new vscode.CompletionItem(
    "fn template",
    vscode.CompletionItemKind.Snippet
  );
  functionTemplate.insertText = new vscode.SnippetString(
    "fn ${1:name}(${2:x: u32}) -> u32 {\n\treturn ${3:x};\n\tensures result == ${3};\n}"
  );
  items.push(functionTemplate);

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
  ["need", "이 조건을 지키는 입력만 받습니다."],
  ["is", "같은지 비교합니다. `x is 1`은 x가 1이면 참입니다."],
  ["not", "`is not`: 다른지 비교합니다."],
  ["and", "두 조건이 모두 맞아야 합니다."],
  ["or", "두 조건 중 하나만 맞으면 됩니다."],
  ["from", "`from 파일: 규칙`: 다른 파일의 규칙을 가져옵니다."],
  ["page", "화면을 시작합니다."],
  ["save", "새로고침해도 남는 값입니다."],
  ["ask", "값을 입력받는 칸입니다."],
  ["show", "값이나 계산 결과를 화면에 보여줍니다."],
  ["button", "누르면 오른쪽 계산 결과를 값에 넣습니다."],
]);

/// Explains a rule line in Korean, e.g. `price < min -> min`.
function explainLine(text) {
  const line = text.replace(/#.*$/, "").trim();
  if (!line) return undefined;
  if (line.startsWith("need ")) return `입력 조건: ${line.slice(5)} 인 입력만 받습니다.`;
  if (line.startsWith("->")) return `위 줄이 하나도 맞지 않으면 답은 ${line.slice(2).trim()} 입니다.`;
  const arrow = line.indexOf("->");
  if (arrow > 0 && !line.startsWith("button")) {
    return `${line.slice(0, arrow).trim()} 이면 답은 ${line.slice(arrow + 2).trim()} 입니다. (위에서부터 처음 맞는 줄이 답)`;
  }
  return undefined;
}

function hoverFor(document, position) {
  const range = document.getWordRangeAtPosition(position, /->|[A-Za-z_][A-Za-z0-9_]*/);
  if (!range) return undefined;
  const word = document.getText(range);
  const lineText = document.lineAt(position.line).text;
  let detail = word === "->" ? explainLine(lineText) : hoverDocs.get(word);
  const rule = document
    .getText()
    .match(new RegExp(`^${escapeRegExp(word)}\\s*\\(([^)]*)\\)\\s*$`, "m"));
  if (rule) detail = `규칙: 입력 (${rule[1] || "없음"}) → 0 이상의 정수`;
  if (!detail) detail = explainLine(lineText);
  if (!detail) return undefined;
  return new vscode.Hover(new vscode.MarkdownString(`**${word}**  \n${detail}`), range);
}

function escapeRegExp(value) {
  return value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

/// Unindented headers, two-space bodies, and aligned `->` within each rule.
function formatDocument(document) {
  const lines = [];
  for (let index = 0; index < document.lineCount; index += 1) {
    lines.push(document.lineAt(index).text);
  }
  const formatted = formatSource(lines);
  return formatted.map((text, index) =>
    vscode.TextEdit.replace(document.lineAt(index).range, text)
  );
}

function formatSource(lines) {
  const out = lines.map((text) => {
    const trimmed = text.trim();
    if (!trimmed) return "";
    return /^\s/.test(text) ? `  ${trimmed}` : trimmed;
  });
  const isRule = (line) =>
    line.startsWith("  ") && !line.trim().startsWith("button") && line.includes("->");
  const align = (indexes) => {
    const parts = indexes.map((index) => {
      const arrow = out[index].indexOf("->");
      return [index, out[index].slice(0, arrow).trimEnd(), out[index].slice(arrow + 2).trim()];
    });
    const width = Math.max(...parts.map(([, left]) => left.length));
    for (const [index, left, right] of parts) {
      out[index] = `${left.trim() ? left.padEnd(width) : " ".repeat(width)} -> ${right}`;
    }
  };
  let block = [];
  for (let index = 0; index <= out.length; index += 1) {
    const line = out[index];
    if (line === undefined || (line && !line.startsWith("  "))) {
      if (block.length) align(block);
      block = [];
    } else if (isRule(line)) {
      block.push(index);
    }
  }
  return out;
}

function validateDocument(document, diagnostics, output) {
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
      output.appendLine(
        `contract_check: VALID (${path.basename(document.uri.fsPath)})`
      );
    } else {
      output.appendLine(
        `contract_check: ${statusFromMessage(stderr)}`
      );
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
      prompt: `scalar input ${index + 1}/${parameterCount}`,
      value:
        index === 0
          ? vscode.workspace
              .getConfiguration("synthetix")
              .get("runInput", "5")
          : "0",
      validateInput: validateScalarInput,
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

function validateScalarInput(value) {
  if (value === "true" || value === "false") return undefined;
  if (!/^-?(0|[1-9]\d*)$/.test(value)) {
    return "Enter a scalar value (number, signed integer, true, or false).";
  }
  const number = Number(value);
  if (!Number.isSafeInteger(number) || number < -2147483648 || number > 4294967295) {
    return "Value must fit a 32-bit scalar.";
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
  const bundled = bundledAxiomPath();
  if (bundled) {
    return bundled;
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

function statusFromMessage(message) {
  return (
    message.match(
      /\b(?:INVALID|UNSATISFIABLE_PRECONDITION|UNSATISFIABLE_POSTCONDITION|NO_CANDIDATE|TIMEOUT|RESOURCE_LIMIT|SOLVER_UNKNOWN|COUNTEREXAMPLE)\b/
    )?.[0] ?? "INVALID"
  );
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

module.exports = { activate, deactivate, formatSource, explainLine };
