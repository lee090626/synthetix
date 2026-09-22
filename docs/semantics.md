# Axiom Target Semantics & Bit-Vector Specification

This document defines the semantics that all pipeline components must share. The
current repository slice implements the DSL lexer/parser, AMI evaluator, SMT
translation, WASM emission, and embedded runtime for pure `u32`/`i32`
expressions, unsigned and signed comparisons, conditional values, and `let`
bindings; broader semantics are still a target.

## 1. Modulo $2^{32}$ Arithmetic Equivalence

A critical design requirement of Synthetix is that **the specification language, the intermediate representation, the SMT solver, and the target execution environment share identical bit-level semantics**.

Any mismatch between these layers introduces a **soundness bug** (where a program is proved correct in SMT, but behaves differently when executed as WebAssembly).

| Layer | Operation | Syntax / Expression | Semantic Rule |
| :--- | :--- | :--- | :--- |
| **Axiom DSL** | Wrapping Mul | `x *% 8` | $x \times 8 \pmod{2^{32}}$ |
| **Axiom DSL** | Wrapping Add | `x +% y` | $(x + y) \pmod{2^{32}}$ |
| **Axiom AMI** | `Expr::Mul` | `lhs.eval(args).wrapping_mul(rhs.eval(args))` | Two's complement wrap |
| **Axiom AMI** | `Expr::Shl` | `lhs.eval(args).wrapping_shl(rhs.eval(args) & 31)` | Shift count masked to 5 bits |
| **Z3 SMT** *(implemented)* | Bit-Vector Mul | `(bvmul x (_ bv8 32))` | 32-bit unsigned/signed bit-vector |
| **Z3 SMT** *(implemented)* | Bit-Vector Shl | `(bvshl x (bvand shift (_ bv31 32)))` | Standard SMT-LIB2 `bvshl` |
| **WASM** *(implemented)* | `i32.mul` | `i32.mul` | Wraps around modulo $2^{32}$ |
| **WASM** *(implemented)* | `i32.shl` | `i32.shl` | Implicitly masks RHS with `0x1F` |
| **Axiom DSL** | Unsigned comparison | `x < y` | Boolean result |
| **Axiom DSL** | Signed comparison | `x < y` with `i32` operands | Two's-complement signed result |
| **Axiom DSL** | Conditional value | `if c { a } else { b }` | a when c is true, otherwise b |

---

## 2. Shift Semantics (`shl`)

In x86, ARM, and WebAssembly, a 32-bit shift operation masks the shift count to 5 bits (`shift & 31`).
To ensure strict identity:
- `Expr::Shl` in `axiom-ami` explicitly applies `& 31`.
- The SMT translation in `axiom-verify` asserts `(bvand shift (_ bv31 32))` for the initial subset.
- The emitted WASM instruction `i32.shl` matches this natively for the initial subset.

Conditional values lower to the WASM `select` instruction. The condition is
verified as a Boolean formula by Z3 before the module is emitted.

## 3. Signed `i32` semantics

`u32`/`number` comparisons use unsigned bit-vector order. `i32` comparisons
use two's-complement signed order and lower to `i32.lt_s`, `i32.le_s`,
`i32.gt_s`, and `i32.ge_s`. Arithmetic still wraps at 32 bits, so the same
WASM bit pattern can be displayed as unsigned or signed depending on its type.

---

## 3. The `*%` and `+%` Operators in Axiom

In standard high-level programming languages, integer overflow may trigger undefined behavior (C/C++), runtime panics (debug Rust), or automatic promotion to BigInt (Python).

In Axiom, all arithmetic operators are explicitly typed:
- `+%`, `-%`, `*%`: Wrapping operations with modulo $2^N$ guarantees.
- Future versions will introduce checked operations (`+?`, `-?`) that generate preconditions requiring that no overflow occurs.
