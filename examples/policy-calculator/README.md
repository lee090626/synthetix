# Policy calculator (first-product vertical slice)

`policy.ax` states only result conditions (no implementation) for the T2 cap policy and the C1 floor/cap policy from
[requirements](../../docs/first-product/requirements.md). `main.ax` wires them to a browser page.

```bash
axiom check && axiom build   # synthesize, prove, emit dist/ (app.wasm + app.proof.json)
axiom dev                    # or serve dist/ with any static server
```

Observed in a real browser (static `dist/`): total=100, cap=80 → 80; total=100, floor=20, cap=80 → 80; inputs
survive reload; total=2,000,000 is rejected by the `requires` guard.

Known gap: a failing `axiom build` (e.g. contradictory `requires`) exits 1 but leaves the previous `dist/` untouched,
so an old, still-"VERIFIED" bundle can be mistaken for the current spec's result (roadmap stage 5).
