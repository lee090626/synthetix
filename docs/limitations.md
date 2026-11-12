# Support scope and known limitations

This page describes what the current Synthetix release supports and what it
does not. The detailed rules are in [semantics.md](semantics.md) and
[application-boundary.md](application-boundary.md).

## Supported

- **Platforms:** CLI binaries for Linux x64, Windows x64, macOS arm64, and
  macOS x64. The VSCode extension bundles all four CLIs.
- **Values:** whole numbers from 0 to 4294967295. `+`, `*`, and `-` must be
  provably in range (see [writing-specs.md](writing-specs.md)).
- **Rules:** `need` lines, `condition -> value` lines read top to bottom, and a
  final `-> value`.
- **Web target:** `synthetix.toml` projects with pages, state, buttons,
  persisted state with schema migration, and adapters through the application
  boundary. Start one with `axiom new <dir>`.

## Known limitations

- **Bounded search.** Synthesis enumerates candidates up to a budget (default
  50,000 candidates and 64 MiB, adjustable with `--max-candidates`,
  `--max-memory-bytes`, `--timeout-ms`). A result of `NOT_FOUND` means no
  candidate within that budget satisfied the rule.
- **The rule decides what is correct.** Synthetix proves the app computes
  exactly what the rule says; whether the rule matches your intent is for you
  to read and judge.
- **Old syntax removed from user files.** Negative numbers (`i32`), arrays,
  loops, recursion, records, lists, text, and property-only results
  (`ensures`) exist in the engine but have no simple syntax yet.
- **No published packages.** Releases are GitHub Release assets only; there is
  no crates.io, npm, or VSCode Marketplace distribution.
- **Not validated with real users.** Ergonomics and error messages reflect
  internal testing only.
