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
./target/release/axiom run examples/mul8.ax -- 5
./target/release/axiom run examples/signed_min.ax -- -1 1
```

Expected result:

```text
result: 40
```

The signed example should print `result: -1`.

Published CLI archives and the all-platform VSCode extension are attached to
GitHub Releases. The release workflow runs the workspace tests on Linux,
Windows, and macOS before creating the release assets.

To use the VSCode play button, build the CLI first, then package and install
the local extension:

```bash
cd editors/vscode
npx --yes @vscode/vsce package --out /tmp/synthetix-vscode.vsix
code --install-extension /tmp/synthetix-vscode.vsix --force
```

For the v2 browser workflow, create `synthetix.toml` with an `entry` file and
run `axiom dev`. This starts the local web development server and watches the
imported `.ax` files. See [the v2 specification](v2.md).

Open the repository in VSCode, open an `.ax` file, save it once to populate
Problems diagnostics, then click `Run Axiom`.
