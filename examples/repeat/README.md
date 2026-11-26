# Repeated steps (`repeat`)

A rule that starts from one value and applies a step a number of times. Each
`promise` holds at the start and every step keeps it, so it holds after any
number of steps (`proofs/lean/Repeat.lean`).

| Rule | What it proves |
| --- | --- |
| `savings` | Yearly interest, stopped at 100000000: never below the principal, never above 100000000 |
| `balance` | A loan with a monthly interest amount and a payment at least that: never above the loan |

```bash
axiom check                  # prove every rule
axiom build --emit python    # dist/contracts.py: savings(1000, 10, 2) == 1210
axiom build --lean           # check each step's proof with Lean
```
