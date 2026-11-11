# Support scope and known limitations

This page describes what the current Synthetix release supports and what it
does not. The detailed rules are in [semantics.md](semantics.md) and
[application-boundary.md](application-boundary.md).

## Supported

- **Platforms:** CLI binaries for Linux x64, Windows x64, macOS arm64, and
  macOS x64. The VSCode extension bundles all four CLIs.
- **Verified values:** `u32`, `i32` (32-bit wrapping arithmetic), and `bool`.
  Proof inputs may also be fixed arrays `[u32; N]`.
- **Verified control:** conditionals, static loops of at most 16 iterations,
  and bounded structural recursion.
- **Web target:** `synthetix.toml` projects with pages, state, buttons,
  persisted state with schema migration, and adapters through the application
  boundary. Start one with `axiom new <dir>`.

## Known limitations

- **Bounded search.** Synthesis enumerates candidates up to a budget (default
  50,000 candidates and 64 MiB, adjustable with `--max-candidates`,
  `--max-memory-bytes`, `--timeout-ms`). A result of `NOT_FOUND` means no
  candidate within that budget satisfied the contract; the contract may be
  contradictory, underconstrained, or outside the search space.
- **Underspecified contracts are accepted.** If `ensures` admits several
  results (for example only `result <= cap`), Synthetix picks one that
  satisfies it. The contract, not the tool, decides what is correct.
- **No text, lists, or records in verified code.** These values exist only at
  the application boundary and in adapters, which are not verified.
- **Adapters and FFI are trusted.** HTTP, storage, and other effects run
  outside the proof; their results are validated against types only.
- **Loops:** no nested loops, runtime-dependent bounds, or loops in contracts,
  clauses, or page expressions.
- **No published packages.** Releases are GitHub Release assets only; there is
  no crates.io, npm, or VSCode Marketplace distribution.
- **Not validated with real users.** Ergonomics and error messages reflect
  internal testing only.
