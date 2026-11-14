# Fresh checkout setup

Synthetix needs Rust and the bundled Z3 build; no system Z3 installation is
required.

```bash
git clone <repository-url> synthetix
cd synthetix
rustup show
cargo test --workspace
cargo fmt --all -- --check
cargo build --release -p axiom-cli
./target/release/axiom run examples/rules/fit_price.ax -- 10 20 80
```

Expected result:

```text
result: 20
```

Published CLI archives and the all-platform VSCode extension are attached to
GitHub Releases. The release workflow runs the workspace tests on Linux,
Windows, and macOS before creating the release assets.

A `v*` tag also publishes the CLI to npm (`npm install -g synthetix-cli`, then
`axiom`) and the extension to the VSCode Marketplace. npm uses trusted
publishing: each of `synthetix-cli` and `synthetix-cli-<platform>` (Windows: `@lee090626/synthetix-cli-win32-x64`) names this
repository and `release.yml` as its trusted publisher on npmjs.com, so no npm
token is stored. The Marketplace publishes only when the `VSCE_PAT` secret (an
Azure DevOps token with Marketplace *Manage* scope for the `publisher` in
`editors/vscode/package.json`) is set; without it, upload the release's `.vsix`
at marketplace.visualstudio.com/manage for larger updates. The npm version comes from the tag
(`v0.15.0` → `0.15.0`); the extension version comes from its `package.json`.

To use the VSCode play button, build the CLI first, then package and install
the local extension:

```bash
cd editors/vscode
npx --yes @vscode/vsce package --out /tmp/synthetix-vscode.vsix
node ./check-vsix.js /tmp/synthetix-vscode.vsix
code --install-extension /tmp/synthetix-vscode.vsix --force
```

For the browser workflow, create `synthetix.toml` with an `entry` file and run
`axiom dev`. This starts the local web development server and watches the
imported `.ax` files.

Start a new project from the web counter template:

```bash
axiom new my-counter
cd my-counter
axiom check
axiom dev
axiom build
```

The static build writes `dist/app.config.js`. Replace it for staging or
production with browser-visible settings such as `environment` and
`apiBaseUrl`; adapters can read them from `window.synthetix.config`. Do not put
secrets in this file because it is served to every browser.

Open the repository in VSCode, open an `.ax` file, save it once to populate
Problems diagnostics, then click `Run Axiom`.
