# Synthetix Target Architecture

## 1. System Overview

Synthetix is a Cargo workspace with six decoupled crates. The first usable
vertical slice now covers parsing, bounded synthesis, Z3 verification, direct
WASM emission, embedded WASM execution, project linking, and a browser dev
loop. The implementations remain intentionally narrow: the proof core is pure
`u32`, while the web layer provides a small declarative page surface.

```text
┌─────────────────────────────────────────────────────────────┐
│                axiom-parser (implemented)                   │
│ Parses .ax modules, imports, contracts, and small web pages  │
└──────────────────────────────┬──────────────────────────────┘
                               │ Module AST
                               ▼
┌─────────────────────────────────────────────────────────────┐
│                axiom-project (v2)                           │
│ Loads synthetix.toml, resolves imports, and retains both    │
│ runtime-expanded contracts and verification summaries       │
└──────────────────────────────┬──────────────────────────────┘
                               │ Contract AST
                               ▼
┌─────────────────────────────────────────────────────────────┐
│                 axiom-ami (implemented)                     │
│   Axiom Machine IR: Tree-structured 32-bit bit-vector IR     │
│   Provides: Expr enum and wrapping interpreter              │
└──────────────┬──────────────────────────────▲───────────────┘
               │                              │
               │ Candidate Expr               │ Counterexamples
               ▼                              │
┌──────────────────────────────┐              │
│       axiom-synth (bounded)  │              │
│  Candidate Enumerator        │              │
│  Verification Loop           │              │
└──────────────┬───────────────┘              │
               │                              │
               │ (Candidate, Contract)        │
               ▼                              │
┌─────────────────────────────────────────────┴───────────────┐
│      axiom-verify (initial)  │              │
│  Translates Spec & AMI to SMT-LIB2 bit-vector formulas      │
│  Queries Z3: Is there ANY input where Candidate != Spec?     │
│    - SAT   => returns concrete Counterexample               │
│    - UNSAT => marks candidate as VERIFIED                   │
└──────────────────────────────┬──────────────────────────────┘
                               │ Verified AMI
                               ▼
┌─────────────────────────────────────────────────────────────┐
│       axiom-wasm             │
│  Direct emission of one or more named WebAssembly functions  │
│  No WAT, LLVM, or clang dependency                           │
└─────────────────────────────────────────────────────────────┘
                               │
                               ▼
┌─────────────────────────────────────────────────────────────┐
│       axiom-cli web layer (v2)                              │
│  Static HTML/JS/CSS/WASM output, localhost dev server,       │
│  polling-based Live Reload                                   │
└─────────────────────────────────────────────────────────────┘
```

---

## 2. Detailed Crate Responsibilities

### `crates/axiom-ami` — implemented
- **Role**: Intermediate Representation (IR) for synthesis and verification.
- **Dependencies**: None (must stay zero-dependency).
- **Current scope**:
  - Expr: Tree representing computations (Arg, Const, arithmetic, unsigned
    comparisons, Boolean operators, and Select).
  - `eval(&self, args: &[u32]) -> u32`: High-speed concrete interpreter enforcing modulo $2^{32}$ wrap-around arithmetic.
- **Current scope**: candidate enumeration lives in `axiom-synth`; this crate
  exposes the WASM instruction cost used to order candidates.

### `crates/axiom-parser` — implemented, minimal
- **Role**: Front-end parser for the Axiom specification language (`.ax`).
- **Input**: UTF-8 source string.
- **Output**: Typed `Contract` AST containing:
  - Function signature: parameters, parameter types, return type.
  - `requires`: Pre-conditions constraining the valid input domain.
  - `ensures`: Post-conditions that must hold for `result`.
- **Current limitation**: grammar and semantic rules are intentionally limited to
  pure scalar `u32`/`i32`/`bool` values with finite conditional expressions;
  source errors now include line and column positions.

### `crates/axiom-project` — v2
- **Role**: Project manifest loading and module linking.
- **Input**: `synthetix.toml` plus the entry `.ax` file.
- **Behavior**: Follows explicit relative imports, enforces exports, detects
  cycles, and retains original contract summaries alongside the legacy
  runtime-expanded view.
- **Boundary**: Called contracts must currently have a single result equality
  and no requires clauses in the legacy runtime expansion. The verifier's
  compositional API uses the original summaries and discharges call-site
  preconditions before assuming callee postconditions.

### `crates/axiom-verify` — implemented subset
- **Role**: The oracle of correctness.
- **Key Invariant**: Must be completely independent of `axiom-synth`. It does not know *how* candidates are generated; it only validates or refutes them.
- **Mechanics**:
  - Formulates the verification condition:
    $$\Phi(x) = \text{Requires}(x) \land (\text{Candidate}(x) \neq \text{Ensures}(x))$$
  - Submits $\Phi(x)$ to Z3 SMT Solver (bit-vector theory `QF_BV`).
  - If UNSAT, the candidate satisfies the specification for all $2^{32}$ inputs.
- **Current limitation**: supports the current pure scalar expression subset and
  bundled Z3; it does not yet cover loops, memory, host effects, or external
  provenance signatures. CLI reports carry verification state and artifact
  integrity fingerprints, while correctness remains the `ProofResult` plus
  backend conformance contract.

### `crates/axiom-synth` — bounded
- **Role**: Enumerative program synthesizer with formal candidate verification.
- **Algorithm (v0.1)**:
  1. Maintain a set of concrete test cases (starts with e.g. `{0, 1, 2, u32::MAX}`).
  2. Enumerate candidate AMI expressions in increasing order of size/cost.
  3. Filter candidates against concrete test cases using `axiom-ami::eval()` (fast reject).
  4. Once a candidate passes all concrete inputs, invoke `axiom-verify`.
  5. If `axiom-verify` returns SAT with a counterexample, reject the candidate and resume search.
  6. If `axiom-verify` returns UNSAT, synthesis is complete.
- **Current limitation**: the search is bounded at depth two and a configurable
  candidate cap. A timeout is checked between candidate verifications; a
  running Z3 query is not forcibly cancelled.

### `crates/axiom-wasm` — emitter and runtime
- **Role**: Backend code generator.
- **Input**: Formally verified `axiom-ami::Expr`.
- **Output**: `Vec<u8>` containing a valid WebAssembly module.
- **Runtime**: executes the exported `run` function through `wasmi` for `u32`
  inputs and result values.
- **Design Rule**: Emits the binary directly without a WAT/text round-trip or an external WASM compiler dependency.
- **Current limitation**: emits and executes only the supported `i32` AMI subset;
  richer WASM types and host imports are not implemented.

### `crates/axiom-cli` — usable MVP
- **Role**: Validates contracts, builds WASM, and executes generated modules.
- **Usage**: `axiom check file.ax`, `axiom build file.ax`, and
  `axiom run file.ax -- 5`. Build and run can also write a JSON report
  with `--report report.json`, limit synthesis with `--timeout-ms N`, and
  cap enumeration with `--max-candidates N`.
- **v2 usage**: `axiom check`, `axiom build`, and `axiom dev` read
  `synthetix.toml`; `axiom dev` serves generated web files and watches the
  linked source files.
- Artifact reports carry the semantics version, synthesis bounds, proof state,
  and separate integrity fingerprints for the verification artifact and emitted
  WASM.
