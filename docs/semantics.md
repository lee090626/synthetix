# Synthetix Target Semantics & Bit-Vector Specification

This document defines the semantics that all pipeline components must share.
The frozen v4.1 scalar baseline covers pure `u32`/`i32`/`bool` expressions,
comparisons, conditional values, and `let` bindings. Implemented v6 extensions
add fixed `[u32; N]` proof inputs, statically bounded loops in verified function
bodies, and bounded structural recursion; these do not change the emitted
`v4.1-scalar-baseline` identifier.

## v4.1 Semantic Baseline

v4.1 fixes the meaning of the current scalar slice before adding more synthesis
power. The reference evaluator in `axiom-verify::reference` is intentionally
independent from both `axiom-ami::Expr::eval` and Z3. It is the executable
comparison point for the AMI and WASM conformance tests.

### Scalar values

| Type | Meaning | Boundary rule |
| --- | --- | --- |
| `u32` | 32-bit bit pattern interpreted with unsigned order | arithmetic wraps modulo `2^32` |
| `i32` | the same 32-bit bit pattern interpreted as two's-complement signed | arithmetic still wraps modulo `2^32`; ordering is signed |
| `bool` | logical `false` or `true` | encoded as `0` or `1`; the verifier constrains boolean parameters to those values |

### Operator baseline

| Operation | Rule | v4.1 status |
| --- | --- | --- |
| `+%`, `-%`, `*%` | 32-bit wrapping arithmetic | implemented and tested across reference, Z3, and WASM |
| `<<` | left shift with shift count masked by `31` | implemented and tested across all three layers |
| `&`, `|`, `^` | bitwise operation on the 32-bit pattern | implemented and tested across all three layers |
| `==`, `!=` | bit equality for numeric values; logical equality for booleans | implemented |
| `<`, `<=`, `>`, `>=` | unsigned for `u32`, signed for `i32` | implemented and boundary-tested |
| `&&`, `||` | boolean conjunction/disjunction | implemented; only boolean operands are valid |
| `>>` | not yet defined | no parser, AMI, Z3, or WASM support; adding it is outside v4.1 |
| `/`, `%` | not yet defined | no parser, AMI, Z3, or WASM support; adding it is outside v4.1 |
| division by zero | no current runtime meaning | must be specified before division is added |

Overflow is therefore not an error in the current wrapping operators. A future
checked operator may add a separate precondition, but v4.1 does not add one.

### AMI decision

`axiom-ami::Expr` remains the canonical scalar candidate representation. A
contract supplies the parameter and result type context, while the expression
variants encode the operation's signedness (`Lt` versus `SLt`, for example).
This is sufficient for the current scalar pipeline, so v4.1 does not introduce
a second Core IR.

AMI is not a self-describing typed tree, but the v4.3 typed kernel can infer
candidate types from the contract context without introducing a second IR. If a
later backend needs metadata that cannot be supplied by the contract context,
that evidence must justify a minimal typed wrapper rather than an automatic IR
rewrite.

### Precondition satisfiability

`requires` is checked independently of candidate verification. An inconsistent
conjunction such as `x > 10` and `x < 5` is reported as
`UNSATISFIABLE_PRECONDITION`; it must not be accepted as a vacuously verified
contract. A satisfiable precondition proceeds to normal candidate verification.

### Conformance scope

The v4.1 tests compare the independent reference evaluator, the existing AMI
evaluator, Z3 verification, and WASM execution over arithmetic, shifts,
bitwise operations, comparisons, boolean values, conditionals, and boundary
inputs. `max`, `min`, and `clamp` remain regression examples.

## v4.9 Frozen Surface

The v4.9 freeze applies to the scalar proof boundary described above. The
stable semantics identifier emitted in artifact reports is
`v4.1-scalar-baseline`; changing its rules requires a new semantics version.

The frozen v4 surface contains:

- scalar `u32`, `i32`, and `bool` contracts and finite conditional values;
- `contract` declarations for synthesis, `fn` declarations for verified user
  implementations, and `extern fn` declarations for runtime adapters;
- the `ContractCheck`, `SynthesisResult`, and `ProofResult` states;
- AMI, reference evaluator, Z3, and WASM conformance for the supported scalar
  operators.

The freeze does not define division, remainder, right shift, loops, arrays,
collections, recursion, host imports, or server/database/authentication
internals. Those features require a new semantics decision and are not
implicitly supported by the v4 artifact format.

## v6.1 Bounded Fixed Arrays

v6.1 adds a separate, bounded proof-input model without changing the frozen
`v4.1-scalar-baseline` identifier. A verified parameter may use `[u32; N]` for
`1 <= N <= 16`. The proof and WASM ABI flatten each element into one `u32`
argument; `.length` is the compile-time length. Verified functions still return
only scalar values, and `extern` declarations, application state, page
expressions, and dynamic `List` values do not accept fixed arrays.

`values[index]` reads an element. `array_update(values, index, value)` denotes
an immutable update that returns a new array value; the original array is
unchanged. Indices may be `u32` or `i32`, but bounds use the raw 32-bit unsigned
index value, so negative `i32` values are out of bounds. An access is defined
only when `index < values.length`.

Every `requires` expression must be defined for every input. Candidate and
`ensures` accesses must be defined for every input satisfying the conjunction
of `requires`. Short-circuit `&&`, `||`, and conditional expressions only
require the selected/right-hand path to be defined. A contract that cannot
prove these bounds is rejected; AMI/reference evaluation returns an error and
WASM traps for an out-of-bounds access.

This is a fixed-vector proof model, not heap memory, a runtime collection, or
an application `List`. v6.0 measured the unsupported verified collection
parameter as an expressiveness boundary; it did not show application list
performance to be a bottleneck. The benchmark's priority recommendation for
compositional summary depth and synthesis scaling remains unchanged.

## v6.2 Bounded Loops

The supported form is a `for` accumulator expression in a `fn` implementation:

```ax
for i in 0..4 with acc = 0 invariant acc == acc {
  acc +% values[i]
}
```

The range is static and half-open. `end - start` must be between `0` and `16`
inclusive. `i` is a `u32`; `acc` is initialized once, and each body evaluation
must return the same type as `acc`. The invariant is a boolean expression over
the current index and accumulator. Nested loops, runtime-dependent bounds, and
loops in contracts, clauses, or page expressions are rejected.

The verifier proves the invariant and expression definedness for the initial
state, each reachable iteration, and the exit state under the function's
`requires`; finite iterations are then unrolled into AMI and WASM. Thus no
runtime timeout or stack behavior serves as a termination argument. A failed
invariant makes the function contract invalid, and executing a failed guard
traps. A zero-iteration loop still checks its invariant at entry/exit and does
not evaluate the body.

This is a deliberately static bounded model. It makes repeated verified
implementations expressible, but unrolling does not claim to reduce Z3 or
synthesis cost for equivalent manually expanded expressions.

## v6.3 Bounded Structural Recursion

The v4 scalar baseline remains frozen. v6.3 adds a separate function-body
extension for one `u32 -> u32` self-recursive `fn`: exactly one self-call must
appear in the nonzero `else` branch of `if n == 0`, and its argument must be
`n -% 1`. A `u32` measure has lower bound zero; the base branch prevents the
wrapping decrement from running at zero, so each permitted recursive edge
strictly decreases the measure. Other functions, mutual recursion, multiple
self-calls, shadowing the measure, and combining recursion with a bounded loop
are rejected.

The verifier statically unrolls up to 16 recursive edges (17 active frames when
counting the initial invocation). A call beyond that bound carries an undefined
guard. Verification must prove that guard unreachable under `requires`; if it
cannot, the `fn` is invalid. At runtime a reached guard traps. Termination and
resource safety therefore come from the syntactic decrease, proof obligation,
and fixed unroll bound, not from WASM stack overflow behavior.

## 1. Modulo $2^{32}$ Arithmetic Equivalence

A critical design requirement of Synthetix is that **the specification language, the intermediate representation, the SMT solver, and the target execution environment share identical bit-level semantics**.

Any mismatch between these layers introduces a **soundness bug** (where a program is proved correct in SMT, but behaves differently when executed as WebAssembly).

| Layer | Operation | Syntax / Expression | Semantic Rule |
| :--- | :--- | :--- | :--- |
| **Axiom DSL** | Wrapping Mul | `x *% 8` | $x \times 8 \pmod{2^{32}}$ |
| **Axiom DSL** | Wrapping Add | `x +% y` | $(x + y) \pmod{2^{32}}$ |
| **Axiom DSL** | Wrapping Sub | `x -% y` | $(x - y) \pmod{2^{32}}$ |
| **Axiom DSL** | Bitwise AND | `x & y` | Per-bit AND over 32 bits |
| **Axiom DSL** | Bitwise OR | `x | y` | Per-bit OR over 32 bits |
| **Axiom DSL** | Bitwise XOR | `x ^ y` | Per-bit XOR over 32 bits |
| **Axiom AMI** | `Expr::Mul` | `lhs.eval(args).wrapping_mul(rhs.eval(args))` | Two's complement wrap |
| **Axiom AMI** | `Expr::Sub` | `lhs.eval(args).wrapping_sub(rhs.eval(args))` | Two's complement wrap |
| **Axiom AMI** | `Expr::BitAnd` / `BitOr` / `BitXor` | `lhs.eval(args) &/|/^ rhs.eval(args)` | 32-bit bitwise operation |
| **Axiom AMI** | `Expr::Shl` | `lhs.eval(args).wrapping_shl(rhs.eval(args) & 31)` | Shift count masked to 5 bits |
| **Z3 SMT** *(implemented)* | Bit-Vector Mul | `(bvmul x (_ bv8 32))` | 32-bit unsigned/signed bit-vector |
| **Z3 SMT** *(implemented)* | Bit-Vector Sub / And / Or / Xor | `bvsub` / `bvand` / `bvor` / `bvxor` | Same 32-bit bit pattern |
| **Z3 SMT** *(implemented)* | Bit-Vector Shl | `(bvshl x (bvand shift (_ bv31 32)))` | Standard SMT-LIB2 `bvshl` |
| **WASM** *(implemented)* | `i32.mul` | `i32.mul` | Wraps around modulo $2^{32}$ |
| **WASM** *(implemented)* | `i32.sub` / `i32.and` / `i32.or` / `i32.xor` | Native i32 operations | Same 32-bit bit pattern |
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

Multiple `ensures` clauses are interpreted as a conjunction. For example, a
property-only maximum does not describe an `if` implementation:

```ax
contract maximum(first: u32, second: u32) -> u32 {
  ensures result >= first;
  ensures result >= second;
  ensures result == first || result == second;
}
```

The current synthesizer can find this two-input select shape, then Z3 proves
all three postconditions for the complete `u32` input space.

The same property-first approach supports a bounded three-region clamp:

```ax
contract clamp(value: u32, lower: u32, upper: u32) -> u32 {
  requires lower <= upper;
  ensures value < lower || value > upper || result == value;
  ensures value >= lower || result == lower;
  ensures value <= upper || result == upper;
}
```

The current search may synthesize one nested `select` for this shape. Deeper
trees and arbitrary expression branches remain outside this slice. Small
constants from `0` through `16` are currently available in simple conditional
candidates.

An `ensures` clause must mention `result`; a condition about only the inputs
does not constrain the implementation output and is rejected early. If no
bounded candidate satisfies the clauses, Synthetix reports that the contract
may be contradictory, underconstrained, or outside the current search boundary.

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
