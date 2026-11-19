---
name: write-axiom-rules
description: Write or fix Synthetix `.ax` rules until `axiom check` proves them. Use when turning a calculation (prices, tax, fees, splits) into a proven rule, or when `axiom check` fails.
---

# Write Synthetix rules

A rule is done only when `axiom check` proves it. Work in a **loop**: write, check, read the report, fix, check again.

The syntax lives in `docs/writing-specs.md` (Rules, Big numbers, Promise, Numbers must stay in range). Read those sections before writing the first rule.

## Steps

1. **Project.** Put rules in a project: a folder with `synthetix.toml` (`[project]` / `entry = "rules.ax"`). `axiom check file.ax` accepts a file with exactly one rule; a project checks many.
2. **Write.** One rule per calculation. For each input write a `need` line that bounds it (`need price <= 1000000000000`). Add `with big numbers` to the name line when any amount can pass 4294967295. State each property the caller relies on as a `promise` line (`promise answer + fee(amount, rate) is amount`).
3. **Check.** In the project folder run `axiom check --report check.json`.
4. **Read the report.** `{"ok": true}` ends the loop. Otherwise `error` holds `message` and, when known, `file`, `rule`, `line`, `promise`, `inputs`, `answer`, `hint`:
   - `may go outside ... add need x <= limit`: add the named `need` lines, or narrow existing ones.
   - `may divide by 0`: add `need divisor >= 1`.
   - `promise N does not hold` with `inputs` and `answer`: work the rule by hand on those inputs. Decide which is wrong for the real requirement: the `->` line, the promise, or the allowed inputs. Change that one.
   - `UNSATISFIABLE_PRECONDITION`: the `need` lines contradict each other.
5. **Repeat** steps 3–4 until the report is `{"ok": true}`. Keep every promise the requirement states; removing a promise to get `ok` hides a bug.
6. **Use it.** `axiom build --emit js` (TS, `bigint` for big numbers) or `--emit rust` writes the proven functions to `dist/`.

Report the final `axiom check` output and every change made to a promise or `need` line, with the counterexample that caused it.
