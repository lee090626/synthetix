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
| Name a value | `name = value` | Gives a part of the computation a name; must come before the `->` lines |
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

- Operators: `+ - * / ( )`, `mod`, `< <= > >=`, `is`, `is not`, `and`, `or`.
- `min(a, b)` and `max(a, b)` give the smaller and larger value: `-> max(lo, min(price, hi))`.
- `/` divides and rounds down (`7 / 2` is 3). For a percent, write `price * rate / 100`.
- `a mod b` is what is left after dividing (`7 mod 2` is 1).
- `round(a / b)` divides and rounds half up (`round(7 / 2)` is 4).
- `=` is never a comparison. It names a value in a rule (`left = total - used`) or sets values on pages. Write `x is 0` to compare.
- A named value is used by its name in later lines:

  ```
  leftover(total, used)
    need used <= total
    left = total - used
    left > 10 -> left * 2
    -> left
  ```
- A rule can use another rule from the same file: `-> fit_price(base_amount(price, quantity), min, max)`.
- Every value is a whole number from 0 to 4294967295, unless the name line
  says otherwise (below).
- A file may hold several rules, each starting with an unindented name line.

### Negative numbers

Add `can be negative` to the name line. Values then go from -2147483648 to
2147483647, and `-` may go below 0:

```
change(now, before) can be negative
  need now <= 1000000 and now >= 0
  need before <= 1000000 and before >= 0
  -> now - before
```

Write negative numbers with a minus sign: `-5`, `x < -100`, `-x`. In these
rules `/` and `mod` round toward 0 (`-7 / 2` is -3, `-7 mod 2` is -1), and the
divisor must stay above 0 or below 0. `round` rounds half away from 0
(`round(-7 / 2)` is -4).

### Lists

`prices[3]` is a list of three values. Use `sum(prices)`, `min(prices)`,
`max(prices)`, `count(prices)`, or one item, counted from 1: `prices[1]`.
A `need` line about the list holds for every item:

```
cart(prices[3])
  need prices <= 100000
  sum(prices) > 200000 -> 200000
  -> sum(prices)
```

A list has a fixed size from 1 to 16.

### Decimals

Add `with 2 decimals` (1 to 6 places) to the name line. Every value in the rule,
inputs and answer, then has that many decimal places, and `*` and `/` round
down to them:

```
vat(price) with 2 decimals
  need price <= 100000
  -> price * 0.1
```

`vat(12.50)` is `1.25`. `round(a / b)` rounds half up to the last place
(`round(10.01 / 3)` is `3.34`), and `a mod b` works on decimals (`7.50 mod 2` is
`1.50`). A rule may use a rule from the same file with other places: the inputs
are moved to its places and the answer back, rounding down when places are
dropped (`0.135` into 2 places is `0.13`). A rule can be both:
`drift(price) can be negative with 2 decimals`; then `*` and `/` round toward 0.

### Text answers

Put each answer in quotes to answer with text:

```
grade(score)
  score >= 90 -> "A"
  score >= 80 -> "B"
  -> "C"
```

Every `->` line then needs a text answer. A page shows it (below); other rules
cannot use it.

### Numbers must stay in range

- `+` and `*` must not go above 4294967295 (or outside -2147483648 to
  2147483647 in `can be negative` rules): add `need name <= limit` lines for
  their inputs.
- `/` must not divide by 0: add `need divisor >= 1`, unless the divisor is a
  number like `100`.
- `-` must not go below 0. Synthetix proves this for every allowed input,
  using `need` lines and the rule's own condition (and the failed conditions
  above it). `price - price * rate / 100` with `need rate <= 100` is fine;
  `a - b` with nothing about `a` and `b` is rejected with an input that breaks
  it.

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
| `show "Text" if a > 100` | Show the text only while the condition holds |
| `save prices[3] = 0` | A list of three values (`prices_1` to `prices_3`) |
| `ask prices "Price"` | One input box per item: "Price 1", "Price 2", ... |
| `save diff = 0 can be negative` | Values that may go below 0 |
| `save price = 0 with 2 decimals` | Values typed and shown with 2 decimal places |
| `save d = 0 can be negative with 2 decimals` | Both |
| `save grade = ""` | A text value, for a rule with text answers |
| `button "Label" -> a = rule(...)` | On click, set `a` to the rule's answer |

Pass a list to a rule by name: `button "Total" -> total = cart(prices)`.
A value with decimals goes only into a rule with the same places. On a page,
values can be calculated and compared (`show price + tax`,
`show "Pricey" if price > 10.5`); values with fewer places are raised to the
most places used (`1.5 + 0.25` is `1.75`). Values that can be
negative are only shown or compared, and calculated in a rule. A whole number
up to 2147483647 may go into a `can be negative` rule. A rule with text answers
sets a text value: `button "Grade" -> grade = grade_of(score)`.

## When it fails

| Message | What to do |
| --- | --- |
| `the last line must be -> value` | Add a final `-> value` line for when no rule matches |
| `may go outside 0 to 4294967295` | Add `need name <= limit` lines for the inputs |
| `may divide by 0` | Add `need divisor >= 1` |
| `is not defined for inputs [...]` | A `-` can go below 0 for those inputs; add a `need` line or a condition |
| `is a reserved word` | Rename the input |
| `has N decimal places but ...` | A button, or a rule from another file, needs the same `with N decimals` |
| `use is to compare` | Write `x is 1` / `x is not 1` instead of `=` / `!=` |
| `call-site precondition ... is not established` | The values passed to another rule may break its `need` lines; add `need` lines or a condition |
| `UNSATISFIABLE_PRECONDITION` | The `need` lines contradict each other |
| `old syntax, which is no longer supported` | Rewrite the file in the forms above |

On a failed build, earlier verified output is removed, so a broken rule never
keeps serving an old result. Fix the file and save; `axiom dev` rebuilds and
reloads.

## Checking what a rule does

`axiom check` prints one example per `->` line, counted from the top of the
rule, so you can compare it with what you meant:

```text
example: fit_price(1, 20, 100) = 20   (`->` line 1)
example: fit_price(50, 1, 5) = 5   (`->` line 2)
example: fit_price(5, 0, 80) = 5   (`->` line 3)
```

It also warns when a `->` line can never apply because an earlier line or a
`need` line always matches first:

```text
warning: f: `->` line 2 never applies; an earlier line or a `need` line always matches first
```

## Workflow

```bash
axiom new my-app
cd my-app
axiom check      # rules are valid and proven
axiom dev        # live app; edits to .ax files rebuild it
axiom build      # static output in dist/
```

Use `axiom run file.ax -- <inputs>` to try a single rule.
