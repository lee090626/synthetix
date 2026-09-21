# VSCode Integration

The VSCode integration is intentionally a thin wrapper around the Rust CLI.
The `.ax` language and the CLI remain the source of truth; the extension adds
syntax highlighting, completion, a contract runner, and a web development
command.

## Development

1. Build the CLI from the repository root when changing the CLI. The packaged
   extension contains a platform-specific CLI fallback:

   ```bash
   cd ~/Developer/synthetix
   cargo build --release -p axiom-cli
   ```

To install the local extension in a normal VSCode window, package it and
install the generated VSIX:

```bash
cd ~/Developer/synthetix/editors/vscode
mkdir -p bin/darwin-arm64
cp ../../target/release/axiom bin/darwin-arm64/axiom
npx --yes @vscode/vsce package --out /tmp/synthetix-vscode.vsix
code --install-extension /tmp/synthetix-vscode.vsix --force
```

2. Open either `editors` or `editors/vscode` in VSCode.
3. Start the Extension Development Host. If `editors` is open, its launch
   configuration loads the extension from `editors/vscode`.
4. Open an `.ax` file and click `Run Web` in the status bar, the globe icon in
   the editor title, or run `Synthetix: Start Web Dev Server` from the Command
   Palette. The command requires a `synthetix.toml` in the workspace or one of
   its parents.
5. Enter a `u32` input. The extension runs the contract and shows the CLI
   output in `Output → Synthetix`. Saving the file also runs `axiom check`
   and places parser/type errors in the Problems panel.

`Synthetix: Run Contract` remains available for the legacy single-file flow.
The web command starts `axiom dev` in an integrated terminal, opens the local
browser page, and lets the CLI handle rebuilds and Live Reload.

The extension uses the bundled CLI for the current platform first, then looks
for `target/debug/axiom` and `target/release/axiom` in the workspace, and
finally falls back to `axiom` on `PATH`. The current local artifact includes macOS Apple Silicon
(`darwin-arm64`); other platforms need their matching binary in `bin/`.
It writes the generated
WASM under `.synthetix/build/` while preserving the source's relative path,
then executes its `run` export. For example,
`examples/easy_mul8.ax` produces
`.synthetix/build/examples/easy_mul8.wasm`.

Typing in an `.ax` file opens completion suggestions for Axiom keywords,
types, parameters, `result`, operators, records, lists, inputs, reducers,
Todo snippets, and conditional web-state snippets. Hover explains common
types, contracts, and page states, and the built-in formatter applies a small
two-space indentation style.

If you run the binary manually from `editors`, use `../target/debug/axiom`; from
the repository root, use `./target/debug/axiom`.

## Deliberate boundary

The extension intentionally does not implement a full Language Server. It
provides static completion, hover, formatting, and snippets for the current
bounded grammar; signature-help and project-wide semantic completion remain
deferred.
