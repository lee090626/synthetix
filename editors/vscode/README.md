# Synthetix for VSCode

This extension runs `.ax` contracts and web projects through the Synthetix CLI.

## Development

From the repository root:

```bash
cargo build -p axiom-cli
```

Open `editors` in VSCode and start the Extension Development Host. For a
project containing `synthetix.toml`, open an `.ax` file and click `Run Web`.
The extension starts `axiom dev` in an integrated terminal and opens the local
browser page. `Synthetix: Run Contract` remains available for the legacy
single-file flow.

The extension searches parent directories for `target/debug/axiom` and
`target/release/axiom` first, then uses the bundled platform CLI, before
falling back to `axiom` on `PATH`. This lets an open checkout run the current
workspace build while installed projects use the bundled fallback. Local VSIX
packaging requires copying the built CLI into `bin/<platform>/`; release VSIX
packages include the platform CLI binaries built by the release matrix.

Saving an `.ax` file runs the project check when `synthetix.toml` is present
and maps parser/type errors to the Problems panel. Typing opens completion
suggestions for modules, pages, records, lists, reducers, keywords, types,
parameters, operators, and small snippets. Hover and basic formatting are
included; it is not a full Language Server.
