# Synthetix

> **"No Implementation Code. Just Verified Contracts."**
> Target: machine-checked program synthesis from formal specifications into executable web applications.

---

## 1. Vision & Core Philosophy

In traditional software development and early AI coding tools, humans and LLMs produce unstable text strings (Python, JavaScript, C) that require manual debugging, syntax parsing, and testing.

**Synthetix replaces implementation code with mathematical contracts.**

Developers write only *what* must hold true (preconditions, invariants, postconditions). The v1 core contains CLI input, parsing, cost-ordered candidate synthesis, SMT verification, direct WASM emission, and embedded runtime execution. v2 extends that core into a browser-first workflow: multiple `.ax` modules, generated web assets, and a local `axiom dev` loop. A minimal VSCode extension runs the same CLI for `.ax` files.

> *"AI does not generate code. AI guesses search priorities; mathematics determines correctness."*

---

## 2. Target Pipeline

```text
Axiom Contract (.ax)
        │
        ▼
   axiom-parser
        │
        ▼
   Contract IR
        │
        ▼
   axiom-synth (bounded) ◄──────┐
        │                       │
        ▼ (candidate AMI)       │ (counterexample on SAT)
   axiom-verify (Z3) ───────────┘
        │
        ▼ (UNSAT: Formally Verified)
   axiom-wasm (direct binary)
        │
        ▼
Web bundle: HTML + JavaScript + CSS + WASM
```

1. **Axiom DSL (`axiom-parser`, implemented)**: Parses the initial contract syntax into a typed AST and performs initial semantic validation.
2. **Axiom Machine IR (`axiom-ami`, implemented)**: Provides a minimal bit-vector expression tree and wrapping evaluator.
3. **Enumerator & verification loop (`axiom-synth`, bounded)**: Enumerates base, depth-one, and bounded depth-two candidates in WASM instruction-cost order, then feeds counterexamples back into the concrete filter.
4. **Formal Verifier (`axiom-verify`, initial)**: Uses bundled Z3 to query:
   $$\exists x \in \text{BV32} .\; \text{Candidate}(x) \neq \text{Spec}(x)$$
   - **SAT**: Extracts a counterexample and rejects the candidate.
   - **UNSAT**: Formally proves the candidate holds for all $2^{32}$ inputs.
5. **WASM Lowering (`axiom-wasm`, implemented)**: Directly emits and executes the supported AMI expression subset without a WAT/text round-trip.

---

## 3. Initial Target: The `mul8` Milestone

This is the smallest end-to-end target:

```axiom
contract mul8(x: u32) -> u32 {
    ensures result == x *% 8;
}
```

- **Target synthesizer result**: `x << 3` (AMI: `Shl(Arg(0), Const(3))`)
- **Target verifier result**: $\forall x \in \text{u32}.\; (x \ll 3) = (x \times 8 \pmod{2^{32}})$
- **Target emitter result**: Native WASM binary (`local.get 0`, `i32.const 3`, `i32.shl`)
- **Currently verified here**: the contract parses, `x << 3` is synthesized, Z3 proves it equivalent to `x *% 8`, a WASM binary is emitted, and the embedded `wasmi` runtime executes it correctly.

---

## 4. Repository Structure

```text
synthetix/
├── Cargo.toml
├── docs/
│   ├── application-boundary.md
│   ├── first-product/     # Product assumptions and CLI analysis guide
│   ├── semantics.md
│   ├── setup.md
│   └── vscode.md
├── crates/
│   ├── axiom-ami/         # Implemented: AMI definition and interpreter
│   ├── axiom-parser/      # Implemented: Axiom DSL lexer, parser, AST
│   ├── axiom-project/     # v2: manifest, imports, and module linking
│   ├── axiom-verify/      # Implemented: SMT query generation & verification
│   ├── axiom-synth/       # Bounded: candidate enumeration & verification loop
│   ├── axiom-wasm/        # Implemented: AMI to WASM binary lowering
│   └── axiom-cli/         # Usable MVP: command-line entry point
├── editors/
│   └── vscode/             # Initial: .ax syntax and Run command
└── examples/
    ├── basics/         # 기본 계약과 산술 예제
    ├── properties/     # 구현 없이 속성만 적는 계약
    ├── friendly/       # 쉬운 단어 문법 예제
    ├── errors/         # 오류 메시지 확인용 예제
    ├── web-counter/       # v2: multi-file browser example
    ├── v3-status/         # v3: typed state and conditional UI
    ├── v3-todo/           # v3: state-based checklist
    └── v3-todo-v31/       # v3.1: input, records, lists, reducer, persistence
```

---

## 5. Quick Start

Ensure Rust is installed via `rustup`:

```bash
# Run these commands from the repository root.

# Run the workspace tests
cargo test --workspace

# Build the axiom command
cargo build -p axiom-cli

# Check a contract
cargo run -p axiom-cli -- check examples/basics/mul8.ax

# Synthesize and write the WASM result
cargo run -p axiom-cli -- build examples/basics/mul8.ax -o mul8.wasm

# Synthesize, verify, execute, and print a result
cargo run -p axiom-cli -- run examples/basics/mul8.ax -- 5

# Optional JSON report and synthesis timeout
cargo run -p axiom-cli -- run examples/basics/mul8.ax -- 5 --report /tmp/mul8.json --timeout-ms 10000
```

To check whether a declarative contract permits different outputs at the same input,
use `axiom analyze <contract.ax>`. `AT_MOST_ONE` does not prove an output exists
for every valid input. See [result analysis](docs/first-product/result-analysis.md)
for witnesses, supported types, and inconclusive outcomes.

For VSCode development, open `editors` as an extension project and start the
Extension Development Host. In the host, open an `.ax` file and click the play
button. The command asks for a `u32` input, runs the generated WASM, and shows
the result in `Output → Synthetix`. See [the VSCode guide](docs/vscode.md).

The proof core supports pure `u32`, signed `i32`, and `bool` values with finite conditional values.
The browser app layer adds records, lists, text inputs, components, and explicit
localStorage persistence around verified scalar reducers. See the
[application boundary](docs/application-boundary.md) for the web build boundary.

For the browser workflow:

```bash
cd examples/web-counter
cargo run -p axiom-cli -- dev
```

For the v3.1 dynamic Todo workflow:

```bash
cd examples/v3-todo-v31
cargo run -p axiom-cli -- dev
```

The dev command opens a local page, verifies the imported contract, and
rebuilds the page when an `.ax` file is saved.

For the v3 typed-state example:

```bash
cd examples/v3-status
cargo run -p axiom-cli -- dev
```

This example uses a verified `bool` contract to conditionally show a message.

The state-based checklist example is in `examples/v3-todo`.

Signed contracts and conditional numeric examples are available in
`examples/basics/signed_min.ax`, `examples/friendly/easy_min.ax`,
`examples/friendly/easy_max.ax`, and `examples/friendly/easy_clamp.ax`:

```bash
cargo run -p axiom-cli -- run examples/basics/signed_min.ax -- -1 1
```

예제 목록과 역할은 [examples/README.md](examples/README.md)에 정리되어 있습니다.

## Easy-word syntax

The original formal keywords remain supported, but beginners can use simpler
aliases:

```axiom
program mul8(x: number) -> number {
    gives answer equals x times 8;
}
```

`program`/`contract`, `when`/`requires`, `gives`/`ensures`,
`answer`/`result`, `number`/`u32`, and word operators such as `plus`
and `times` are equivalent.

Use `i32` when comparisons should use signed WebAssembly semantics. The CLI
accepts negative `i32` inputs and prints signed results.

Conditional and named expressions stay inside the specification:

```axiom
program minimum(first: number, second: number) -> number {
    gives answer equals if first < second { first } else { second };
}

program double_with_name(x: number) -> number {
    gives answer equals let doubled = x plus x in doubled;
}
```
