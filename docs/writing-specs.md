# Writing specifications

A Synthetix contract says *what* a result must satisfy, not *how* to compute
it. Synthetix searches for an implementation, proves it with Z3 for every
allowed input, and emits WASM. This guide shows how to write contracts that
mean what you intend. Examples come from
[`examples/policy-calculator`](../examples/policy-calculator).

## Shape of a contract

```ax
export contract capped(total: u32, cap: u32) -> u32 {
    requires total <= 1000000;
    requires cap <= 1000000;
    ensures result <= total;
    ensures result <= cap;
    ensures result == total || result == cap;
}
```

- `requires` limits the inputs. Callers must respect it; the proof assumes it.
- `ensures` describes `result`. Every clause must mention `result`, and all
  clauses must hold together.
- `export` makes the contract importable from pages
  (`import { capped } from "./policy.ax";`).

## Two ways to write `ensures`

**Formula.** When you already know the calculation, state it directly:

```ax
ensures result == price *% quantity;
```

**Properties.** When you know the rule but not the formula, describe the
result and let Synthetix find it. `capped` above never says `min`; it says the
result is at most both inputs and is one of them.

For rules with several cases, write one clause per case in the form
"outside this case, or the result is X":

```ax
ensures total < floor || total > cap || result == total;
ensures total >= floor || result == floor;
ensures total <= cap || result == cap;
```

## Pin the result down completely

Synthetix accepts any implementation that satisfies the clauses. If the
clauses allow more than one answer, you may get an answer you did not intend,
and it will still be reported as `VERIFIED`.

```ax
contract capped(total: u32, cap: u32) -> u32 {
    ensures result <= cap;
}
```

This verifies, but `axiom run ... -- 10 80` prints `result: 80`: returning
`cap` always satisfies "at most cap". Adding `result <= total` alone would
still allow `0`. Only `result == total || result == cap` fixes the answer.

Check: for a few inputs, ask "is there any other value that satisfies every
clause?" If yes, add a clause.

## Arithmetic and ranges

- Values are 32-bit. `+%`, `-%`, `*%` wrap around on overflow, so give inputs
  `requires` bounds small enough that the real result fits (for example
  `price <= 10000` and `quantity <= 100`).
- `u32` (or `number`) compares unsigned; use `i32` for negative values.
- See [semantics.md](semantics.md) for every operator.

## When it fails

| Message | Meaning | What to do |
| --- | --- | --- |
| `UNSATISFIABLE_POSTCONDITION` | No result can satisfy all `ensures` (for example `result > x` and `result < x`). | Look for clauses that contradict each other or a `requires` that is too loose. |
| `NOT_FOUND` | No implementation was found within the search budget. | Make the rule simpler or more direct, or raise `--max-candidates` / `--timeout-ms`. See [limitations.md](limitations.md). |
| An `ensures` without `result` is rejected | The clause does not constrain the output. | Move input-only conditions to `requires`. |

On a failed build, earlier verified output is removed, so a broken
specification never keeps serving an old result. Fix the contract and save;
`axiom dev` rebuilds and reloads.

## Workflow

```bash
axiom new my-app
cd my-app
axiom check      # contracts are well-formed and satisfiable
axiom dev        # live app; edits to .ax files rebuild it
axiom build      # static output in dist/
```

Use `axiom run file.ax -- <inputs>` to try a single contract on concrete
inputs.
