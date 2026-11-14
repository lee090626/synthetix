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

Ensure Rust is installed via `rustup`, then from the repository root:

```bash
cargo build -p axiom-cli

# Prove and run one rule
./target/debug/axiom run examples/rules/fit_price.ax -- 10 20 80   # result: 20

# Start a web project
./target/debug/axiom new my-app
cd my-app
../target/debug/axiom dev
```

A rule looks like this:

```
fit_price(price, min, max)
  need min <= max

  price < min -> min
  price > max -> max
  -> price
```

A page uses the rules:

```
from logic: fit_price

page Shop
  save price, min, max = 0
  result = 0

  ask "Price" for price
  ask "Min" for min
  ask "Max" for max
  show result
  when "Fit" is clicked
    result = fit_price(price, min, max)
```

See [writing specs](docs/writing-specs.md) for the full syntax and
[limitations](docs/limitations.md) for what is not supported. Examples:
`examples/rules`, `examples/web-counter`, and `examples/policy-calculator`.

The old contract syntax (`contract`, `ensures`, `{ }`) is no longer accepted in
user files. It remains only as the internal format and in `tests/legacy`.

## License

Licensed under either of [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your option.
