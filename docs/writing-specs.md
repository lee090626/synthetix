# Writing specs

Synthetix files (`.ax`) hold two things: **rules** that compute numbers, and
**pages** that use them. Synthetix proves every rule for every allowed input
before it builds the app.

## Rules

```
# Keep the price between min and max.
fit_price(price, min, max)
  need min <= max

  price < min -> min
  price > max -> max
  -> price
```

| Part | Form | Meaning |
| --- | --- | --- |
| Name | `fit_price(price, min, max)` | Rule name and its inputs |
| Need | `need condition` | Only inputs that meet this are accepted |
| Rule | `condition -> value` | If the condition holds, this value is the answer |
| Default | `-> value` | The answer when no rule above matches; must be last |
| Note | `# text` | Ignored by the computation |

Rules are read top to bottom; the first one that matches gives the answer.
A rule with only `-> value` always gives that value:

```
base_amount(price, quantity)
  need price <= 10000
  need quantity <= 100

  -> price * quantity
```

- Operators: `+ - * ( )`, `< <= > >=`, `is`, `is not`, `and`, `or`.
- `=` is never a comparison; it only sets values on pages. Write `x is 0`.
- A rule can use another rule from the same file: `-> fit_price(base_amount(price, quantity), min, max)`.
- Every value is a whole number from 0 to 4294967295.
- A file may hold several rules, each starting with an unindented name line.

### Numbers must stay in range

`+` and `*` must not go above 4294967295, and `-` must not go below 0.
Synthetix checks this before building and rejects the rule otherwise:

- For `+` and `*`, add `need name <= limit` lines for the inputs.
- For `a - b`, make sure `a` is at least `b`: a `need a >= b` line, number
  bounds such as `need a >= 10` and `need b <= 10`, or the rule's own
  condition (`a > b -> a - b`). On the default line, the failed conditions
  above count too (`a > b -> ...` then `-> b - a` is fine).

## Pages

```
from logic: fit_price

page Shop
  save price, min, max = 0
  result = 0

  ask price "Price"
  ask min "Min"
  ask max "Max"
  show result
  button "Fit" -> result = fit_price(price, min, max)
```

| Line | Meaning |
| --- | --- |
| `from logic: a, b` | Use rules `a` and `b` from `logic.ax` |
| `page Name` | Start a page |
| `save a, b = 0` | Values that start at 0 and are kept after reload |
| `a, b = 0` | Values that start at 0 and reset on reload |
| `ask a "Label"` | An input box for `a` |
| `show a` | Show the value of `a`; any calculation or rule works too (`show price * quantity`) |
| `show "Text"` | Show fixed text |
| `button "Label" -> a = rule(...)` | On click, set `a` to the rule's answer |

## When it fails

| Message | What to do |
| --- | --- |
| `the last line must be -> value` | Add a final `-> value` line for when no rule matches |
| `may exceed 4294967295` | Add `need name <= limit` lines for the inputs |
| `may go below 0` | Make the left side of `-` at least the right side (see above) |
| `is a reserved word` | Rename the input |
| `use is to compare` | Write `x is 1` / `x is not 1` instead of `=` / `!=` |
| `call-site precondition ... is not established` | The values passed to another rule may break its `need` lines; add `need` lines or a condition |
| `UNSATISFIABLE_PRECONDITION` | The `need` lines contradict each other |
| `old syntax, which is no longer supported` | Rewrite the file in the forms above |

On a failed build, earlier verified output is removed, so a broken rule never
keeps serving an old result. Fix the file and save; `axiom dev` rebuilds and
reloads.

## Workflow

```bash
axiom new my-app
cd my-app
axiom check      # rules are valid and proven
axiom dev        # live app; edits to .ax files rebuild it
axiom build      # static output in dist/
```

Use `axiom run file.ax -- <inputs>` to try a single rule.
