# Synthetix Project Roadmap

## Product definition

Synthetix is a verified program synthesizer and application language, not a
general-purpose language. Users write `.ax` contracts, Synthetix searches for
implementations, proves them with Z3, emits WebAssembly, and can assemble them
into executable web applications.

The v1 target is:

```text
.ax contract → parse → synthesize → verify → WASM → run(input) → result
```

## Phase 0: Foundation — complete

- [x] Cargo workspace with AMI, parser, verifier, synthesizer, WASM, and CLI.
- [x] Wrapping `u32` arithmetic with modulo 2^32 semantics.
- [x] Initial contract AST and parser.
- [x] Z3 verification with UNSAT certification and SAT counterexamples.
- [x] Direct binary WASM emission.

## Phase 1: Usable vertical slice — complete

Goal: one command can validate, synthesize, execute, and write WASM.

- [x] `axiom check <file.ax>`.
- [x] `axiom build <file.ax> [-o output.wasm]`.
- [x] `axiom run <file.ax> -- <u32>`.
- [x] Embedded Rust WASM execution through `wasmi`.
- [x] Automated runtime tests for normal and overflow inputs.
- [x] Stable output naming, including the hidden `.ax` filename case.
- [x] Existing legacy CLI invocation remains compatible.

Exit condition:

```text
axiom run examples/mul8.ax -- 5 → result: 40
```

## Phase 2: Language MVP — complete

Goal: make the small `u32` contract language predictable and usable.

- [x] `u32` parameters and return values.
- [x] `requires` and `ensures` clauses.
- [x] Wrapping add, multiply, and shift operators.
- [x] Multiple parameters separated by commas.
- [x] Complete-input and unknown-identifier validation.
- [x] Example contracts for identity, add-seven, multiply-eight, and multiply-seventeen.
- [x] Source spans with line and column in parser errors.
- [x] Explicit type-check diagnostics with clause source positions.
- [x] Boolean conjunction/disjunction for compound preconditions.
- [x] Easy-word aliases for beginner-friendly contracts.
- [x] Unsigned comparisons: `<`, `<=`, `>`, `>=`.
- [x] Conditional values: `if condition { value } else { value }`.
- [x] Named pure expressions with `let name = value in body`.

The language remains intentionally limited to `u32` until this phase is stable.

## Phase 2.5: v1 verified core — complete

Goal: grow the language without losing the rule that every generated candidate
is proved before it becomes WASM.

- [x] Lower conditional specifications to AMI Select.
- [x] Verify conditional candidates over all u32 inputs with Z3.
- [x] Emit and execute conditional WASM through select.
- [x] Add VSCode completion and snippets for if, else, and let.
- [x] Add i32 values with signed comparison semantics.
- [x] Add a type-directed candidate lowering path for conditional synthesis.
- [x] Add min, max, clamp, and validation examples.

The v1 boundary remains pure and finite: no mutation, I/O, classes, or
unbounded loops.

## Phase 3: Bounded synthesis — complete

Goal: find useful implementations beyond the first depth-one candidates.

- [x] Base candidates and depth-one expressions.
- [x] Bounded depth-two expression search.
- [x] Concrete sample prefilter before Z3.
- [x] Counterexamples added back into the sample set.
- [x] Deterministic candidate deduplication and a hard search cap.
- [x] Configurable search timeout and bounded candidate limit.
- [x] Human-readable search failure and last-counterexample reports.
- [x] Candidate ordering based on emitted WASM instruction cost.

Do not add e-graphs until a benchmark shows that bounded search is the actual
bottleneck.

## Phase 4: WASM contract — complete

Goal: generated modules have a stable, testable ABI.

- [x] Export a `run` function.
- [x] Emit valid zero-, one-, and multi-parameter `i32` signatures.
- [x] Execute one or more `u32` arguments in the WASM library.
- [x] Verify generated modules with the embedded runtime.
- [x] Expose multiple inputs through the CLI.
- [x] Add a machine-readable JSON build/run report.

## Phase 5: VSCode product — complete locally

Goal: normal VSCode users do not need to know the CLI or F5.

- [x] `.ax` language registration and syntax highlighting.
- [x] Play button and `Synthetix: Run Contract` command.
- [x] Input prompt and Output panel integration.
- [x] Parent-directory discovery of the Rust CLI.
- [x] Bundle the current platform CLI as a VSIX fallback.
- [x] VSIX packaging and bundled-CLI smoke test.
- [x] Package versioned extension artifact `0.14.0`.
- [x] Publish the extension artifact in the repository release flow.
- [x] Add line-based parser/type diagnostics in the Problems panel.
- [x] Add small contract snippets after grammar and error spans stabilized.

## Phase 6: Release hardening — in progress

- [x] Fresh-checkout setup instructions.
- [x] Full workspace test, format, release build, and CLI smoke test.
- [x] Negative fixtures for malformed contracts and unsynthesizable contracts.
- [x] Cross-platform runtime check through the release matrix.
- [x] Align README, architecture, semantics, and VSCode docs with the CLI.

## Phase 7: v2 browser product — implemented first slice

Goal: build a small web application from a project of connected `.ax` files.

- [x] Add `synthetix.toml` with an arbitrary entry path and `web` target.
- [x] Resolve explicit `import`/`export` edges without imposing folders.
- [x] Resolve cross-file pure contract calls before synthesis and verification.
- [x] Produce a static web bundle: HTML, JavaScript, CSS, WASM, and proof metadata.
- [x] Add `axiom dev` with a localhost server and Live Reload.
- [x] Show source file and line information for module and proof errors.
- [x] Connect the VSCode Run action to project web development.
- [x] Add a multi-file browser counter example.

The first v2 slice intentionally keeps the proof core pure and bounded. Called
contracts must currently have one result equality and no `requires` clause;
the dev loop uses Live Reload rather than state-preserving HMR.

The full v2 boundary is documented in [v2.md](v2.md).

## Phase 8: v3 verified application language — v3.1 complete

Goal: keep the verified pure core while adding typed state and conditional web
applications. The v3 specification is documented in [v3.md](v3.md).

- [x] Define the v3 verification and effect boundary.
- [x] Add `bool` contract parameters and return values.
- [x] Preserve types through imported contract calls.
- [x] Add conditional page elements driven by verified boolean state.
- [x] Add v3 type and browser fixtures.
- [x] Add a state-based application example.
- [x] Add UI `record`, `list`, and `text` data types.
- [x] Add input binding, list rendering, and record insertion.
- [x] Add reducer actions that compose verified scalar contracts.
- [x] Add explicit `persist` localStorage effects.
- [x] Add reusable static components with `component` and `use`.
- [x] Add VSCode hover, basic formatting, and v3.1 completion snippets.

The v3.1 boundary still excludes network effects, databases, authentication,
arbitrary JavaScript, and record-valued WASM contracts. Browser storage is the
only implemented external effect and must be declared with `persist`.

## Phase 9: v4 and later — deferred

Only after the previous phases are stable:

1. More bit-vector operators.
2. Equality saturation/e-graphs if benchmarks justify them.
3. Arrays and memory.
4. Loops and inductive invariants.
5. Web Studio.
6. ML-guided search.

Arrays, loops, Web Studio, and ML are not part of the current MVP.
