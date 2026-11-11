# Policy calculator (first-product vertical slice)

`policy.ax` states only result conditions (no implementation) for the three tasks in
[requirements](../../docs/first-product/requirements.md): T1 base amount (`base_amount`), T3 four-item total
(`items_total`), and the T2 cap policy (`capped`) or C1 floor/cap policy (`floor_cap`). `main.ax` wires them to a
browser page: T1 or T3 fills `total`, and a policy button applies T2 or C1 to it.

```bash
axiom check && axiom build   # synthesize, prove, emit dist/ (app.wasm + app.proof.json)
axiom build --emit js        # also write dist/contracts.js + contracts.d.ts (plain ES module)
axiom dev                    # or serve dist/ with any static server
```

Observed in a real browser (static `dist/`): total=100, cap=80 → 80; total=100, floor=20, cap=80 → 80; inputs
survive reload; total=2,000,000 is rejected by the `requires` guard.

A failing `axiom build` (e.g. contradictory `requires`) exits 1 and removes the verified outputs (`app.wasm`, `app.js`,
`index.html`, `app.proof.json`, `contracts.js`, `contracts.d.ts`) from `dist/`, so an older bundle cannot pass for the
current spec. `app.config.js` and `app.css` are kept. `axiom dev` is not affected by this rule.

Spec changes for each task (T1 quantity limit, T2 → C1, T3 item limit), a contradictory spec, and restoring it are
covered by [policy_change.rs](../../crates/axiom-cli/tests/policy_change.rs).

If stored state cannot be loaded (for example a persisted state was renamed in `main.ax`), the app starts from the
initial values, shows the load error, and stops saving so the stored data is not overwritten. Restore the old name, or
bump `storage_schema_version` and register a migration, to recover it.
