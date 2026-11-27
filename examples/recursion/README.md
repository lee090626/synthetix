# A rule that calls itself (`f(n - 1)`)

The first `->` line answers for 0; the other lines may call the rule with the
same inputs and `n - 1` for `n`. Each `promise` holds for 0, and for `n`
whenever it holds for `n - 1`, so it holds for every `n`
(`rec_inv` in `proofs/lean/Repeat.lean`).

| Rule | What it proves |
| --- | --- |
| `triangle` | 1 + 2 + ... + n is exactly n * (n + 1) / 2 |
| `stairs` | A fee per floor, half above floor 10: never above n * fee |

```bash
axiom check                  # prove every rule
axiom build --emit python    # dist/contracts.py: triangle(100) == 5050
axiom build --lean           # check each step's proof with Lean
```
