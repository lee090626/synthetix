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
  else -> price
```

| Part | Form | Meaning |
| --- | --- | --- |
| Name | `fit_price(price, min, max)` | Rule name and its inputs |
| Need | `need condition` | Only inputs that meet this are accepted |
| Rule | `condition -> value` | If the condition holds, this value is the answer |
| Name a value | `name = value` | Gives a part of the computation a name; must come before the `->` lines |
| Otherwise | `else -> value` | The answer when no line above matches; must be last |
| Only answer | `always -> value` | A rule with one answer and no conditions |
| Promise | `promise condition` | After the `->` lines: Synthetix proves `condition` holds for every allowed input; `answer` is the rule's answer |
| Note | `# text` | Ignored by the computation |

Rules are read top to bottom; the first one that matches gives the answer.
A rule with one answer writes `always -> value`:

```
base_amount(price, quantity)
  need price <= 10000
  need quantity <= 100

  always -> price * quantity
```

- Operators: `+ - * / ( )`, `mod`, `< <= > >=`, `is`, `is not`, `and`, `or`.
- `min(a, b)` and `max(a, b)` give the smaller and larger value: `-> max(lo, min(price, hi))`.
- `/` divides and rounds down (`7 / 2` is 3). For a percent, write `price * rate / 100`.
- `a mod b` is what is left after dividing (`7 mod 2` is 1).
- `round(a / b)` divides and rounds half up (`round(7 / 2)` is 4).
- `up(a / b)` divides and rounds up (`up(7 / 2)` is 4, `up(6 / 2)` is 3). For
  steps of 100, write `up(price / 100) * 100`. It works in rules without
  `can be negative` or decimals.
- `=` is never a comparison. It names a value in a rule (`left = total - used`) or sets values on pages. Write `x is 0` to compare.
- A named value is used by its name in later lines:

  ```
  leftover(total, used)
    need used <= total
    left = total - used
    left > 10 -> left * 2
    else -> left
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
  always -> now - before
```

Write negative numbers with a minus sign: `-5`, `x < -100`, `-x`. In these
rules `/` and `mod` round toward 0 (`-7 / 2` is -3, `-7 mod 2` is -1), and the
divisor must stay above 0 or below 0. `round` rounds half away from 0
(`round(-7 / 2)` is -4).

### Big numbers

Add `with big numbers` to the name line for amounts above 4294967295. Values
then go from 0 to 18446744073709551615, or, with `can be negative`, from
-9223372036854775808 to 9223372036854775807:

```
total(price, quantity) with big numbers
  need price <= 1000000000000
  need quantity <= 1000
  always -> price * quantity
```

Only these rules use 64-bit numbers, so other rules are proven as fast as
before. A rule with big numbers can use only other rules with big numbers.
`axiom build --emit js` gives them `bigint` inputs and answers
(`total(1000000000000n, 3n)`), `--emit rust` and `--emit go` give `u64`/`uint64` or `i64`/`int64`, and `--emit python` gives `int`. For now
pages and `axiom app-run` cannot use them (`axiom run` can), and they cannot have decimals.

### Lists

`prices[3]` is a list of three values. Use `sum(prices)`, `min(prices)`,
`max(prices)`, `count(prices)`, or one item, counted from 1: `prices[1]`.
A `need` line about the list holds for every item:

```
cart(prices[3])
  need prices <= 100000
  sum(prices) > 200000 -> 200000
  else -> sum(prices)
```

A list has a fixed size from 1 to 16 (for any length, see below).

Calculating with a whole list works item by item, and the answer is a list of
the same size. Lists used together must have the same size:

```
# What each person still has to pay
to_pay(paid[3], share[3]) can be negative
  need paid >= 0 and paid <= 1000000
  need share >= 0 and share <= 1000000
  always -> share - paid
```

A page stores such an answer in a list: `owe = to_pay(paid, share)`.

### Lists of any length

Write the length as a name instead of a number: `prices[n]` takes any number
of items. A `need n <= limit` line is required; Synthetix proves the rule for
every length from 0 to the limit:

```
cart(prices[n])
  need n <= 1000
  need prices <= 100000
  sum(prices) > 200000 -> 200000
  else -> sum(prices)
  promise answer <= sum(prices)
```

What you can write:

| Write | Means | Needs |
|---|---|---|
| `sum(prices)`, `sum(paid - share)` | Add up every item (of an item-by-item calculation) | |
| `min(prices)`, `max(prices)` | Smallest or largest item | `need n >= 1` |
| `count(prices)` | `n` | |
| `always -> prices - min(prices)` | An answer per item (a list answer) | |
| `position` | The item's place in a list answer, from 1 | A list answer |
| `sum(total * weights / sum(weights))` | A sum of items that use a list's own sum; the inner sum is added up first | Items at least 0 |
| `need prices <= 100000` | Holds for every item | Keep it apart from lines about `n` or `sum(...)` |
| `balance(paid[n], share[n])`, `order(prices[n], fee)` | Lists share one length; other inputs work as usual | |
| `total(items) * 2` in a rule over `items[n]`, where `total` is a rule over `prices[n]` whose answer is one `sum(prices)` | The caller adds up `items` once and calls `total`'s core; repeated calls share that sum | The caller's own `need`s for `items` and `n` (the callee's `need`s) |

What a `promise` can prove:

| Promise | Holds when |
|---|---|
| `answer <= n * 1000000` | It follows from each item's range: a sum lies between `n` times the smallest and `n` times the largest item; `min` and `max` lie between those, and `min <= every item <= max` |
| `answer <= sum(paid)` for `sum(paid - share)` | The sum uses only `+`, `-` and `*` of items: `sum(paid - 2 * share + fee)` is `sum(paid) - 2 * sum(share) + n * fee` |
| `answer <= prices` for a list answer | It holds for every item |
| `sum(answer) is sum(paid) - sum(share)` | The list answer is one `always -> ...` line; `sum(answer)` (also `min`, `max`) is that line inside `sum(...)` |
| `sum(answer) <= total`, `sum(answer) >= total - n + 1` | The answer is `always -> total * weights / sum(weights)`, `total` and the weights are at least 0, and for `>=` `need n >= 1`. Shares round down, losing less than 1 each |
| `sum(answer) is total` | The answer is `position <= total - sum(E) -> E + 1` then `else -> E`: the first items get the rest, 1 each. Holds when `0 <= total - sum(E) <= n`; for `E = total * weights / sum(weights)` with `need n >= 1` that is always so (`alloc_sum` in `proofs/lean/Lists.lean`) |

What does not work yet, and what to do instead:

| Not yet | Instead |
|---|---|
| A promise about `sum(prices / 2)` or `sum(a * b)` of two lists | A promise that follows from the ranges; the failing input may have sums no real list gives |
| `sum(answer)` with several answer lines (other than the `position` form above) | One `always -> ...` line |
| `total - n + 1` in a rule without `can be negative` when `total` can be below `n` | `need total >= 50` (at least the limit), or `can be negative` |
| A sum that can pass the number range (`need n <= 1000` and `need prices <= 100000` reach 100000000) | Lower the limit, limit the items with `need`, or add `with big numbers`; the error says when |
| `prices[1]`, decimals, pages, calling the rule from another rule, `repeat` over a list | |

How it is proven: each item is proven like a rule of its own, and the rest of
the rule is proven for every sum the items allow. The facts about sums, `min`,
`max` and shares are proven once, for any length, in Lean
(`proofs/lean/Lists.lean`).

`axiom run cart.ax -- "[1,2,3]"` runs a file with one such rule. `--emit`
gives a function taking arrays: `cart([1, 2, 3])` in JS and Python,
`cart(&[1, 2, 3])` in Rust, `Cart([]uint32{1, 2, 3})` in Go. Example:
`examples/any-length`.

### Repeated steps

`repeat` applies a step a number of times. `start ->` gives the first value,
`repeat years` names the input that counts the steps, and the `->` lines
after it are the step, where `value` is the answer so far:

```
savings(principal, rate, years)
  need years <= 100
  need principal <= 100000000
  need rate <= 20
  start -> principal
  repeat years
  value * rate / 100 > 100000000 - value -> 100000000
  else -> value + value * rate / 100
  promise answer >= principal
  promise answer <= 100000000
```

A `need years <= limit` line and at least one `promise` are required. Each
promise must hold for the start and survive one step from any `value` that
meets every promise; Synthetix proves those two, and then it holds after any
number of steps (`loop_inv` in `proofs/lean/Repeat.lean`). The promises are
also what keeps the step's numbers in range, so bound `value` from above.
`repeat` does not work with lists yet.

### A rule that calls itself

A rule may call itself with one less. The first `->` line answers for 0;
the other lines may call the rule with the same inputs and `n - 1` for `n`:

```
triangle(n)
  need n <= 1000
  n is 0 -> 0
  else -> n + triangle(n - 1)
  promise answer * 2 is n * (n + 1)
  promise answer <= 1000000
```

A `need n <= limit` line and at least one `promise` are required. Each
promise must hold for 0, and for `n` whenever it holds for the call's answer
(for `n - 1`); Synthetix proves those two, and then it holds for every `n`
(`rec_inv` in `proofs/lean/Repeat.lean`). As with `repeat`, the promises keep
the numbers in range, so bound the answer from above. A failing input names
`n__prev` (that is `n - 1`) and `value` (the call's answer). Example:
`examples/recursion`.

| Not yet | Instead |
|---|---|
| Calls with other inputs changed, or `n - 2` | Keep the other inputs; one less each call |
| A `need` about `n` other than `n <= limit` (`need n >= 3`) | A condition line (`n < 3 -> ...`) |
| Named values, lists, decimals, `can be negative`, a record answer | `->` lines on numbers |
| Calling the rule from another rule, or from a page | `--emit` or `axiom run` |

### Decimals

Add `with 2 decimals` (1 to 6 places) to the name line. Every value in the rule,
inputs and answer, then has that many decimal places, and `*` and `/` round
down to them:

```
vat(price) with 2 decimals
  need price <= 100000
  always -> price * 0.1
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
  else -> "C"
```

Every `->` line then needs a text answer. A page shows it (below); other rules
cannot use it.

To use texts in rules, name them in a type. Each text is a value, like a
word value, so it is proven like any choice:

```
type Grade = "A" | "B" | "C"

bonus(grade: Grade)
  grade is "A" -> 10
  grade is "B" -> 5
  else -> 0
```

`bonus("B")` gives 5. A `->` line that answers a text of the type makes the
rule answer that type, so another rule can use it. A type holds either words
or quoted texts, not both. A text may hold `|`. Texts are fixed in the type: a
rule cannot take any text.

A page uses it with a text state: `save grade = ""` and `ask "Grade" for grade`
show a box of the values (the first line is blank), and `result = bonus(grade)`
passes the chosen text. A literal works too: `result = bonus("B")`. A text that
is not a value is an error. A state given
to two types is an error. The page passes the value's place, which the browser
maps from the text; that mapping is checked by tests, not proven.

### Types

`type` declares a **choice** (one of a few named values) or a **record**
(several values that belong together). Write types before or between rules,
unindented:

```
type Size = small | medium | large

type Order
  price
  quantity
  size: Size
```

An input takes a type with `name: Type`. A record's fields are read with a
dot (`order.price`), and a record is passed to another rule of the same file
whole (`subtotal(order)`). `match` gives an answer per value of a choice:

```
shipping(size: Size, weight)
  need weight <= 100000
  match size
    small -> 3000
    medium -> 5000
    large -> 5000 + weight / 1000 * 100
  promise answer >= 3000

total(order: Order, weight)
  need order.price <= 10000000
  need order.quantity <= 100
  need weight <= 100000
  goods = subtotal(order)
  goods >= 100000 -> goods
  else -> goods + shipping(order.size, weight)
```

- A `match` names every value once (`small or medium -> ...` covers two), or
  ends with `else -> value`. A missing value is an error that names it.
  `match` comes after the other `->` lines; `promise` lines may follow it.
- A value can also be compared (`order.size is large`) or passed
  (`shipping(large, weight)`). A value names one thing in a file: no input,
  named value or other choice may use the same name.
- Calls are checked: a rule that takes a `Size` gets a `Size` value or input,
  a rule that takes an `Order` gets an `Order` input, and a rule that takes a
  number gets no record.
- Underneath, a value is its place from 0 (`large` is 2), and a choice input
  only accepts those places. A record input is one input per field, in the
  order declared. So a rule with types is proven like any other rule, Lean
  included.
- `axiom run` takes values by name or place: `axiom run shipping.ax -- large 12000`.
- `--emit` gives each record field as its own input, named `order__price`, and
  the values of each choice as constants: `Size.large` in JS and Python,
  `Size::large` in Rust, `contracts.SizeLarge` in Go. In JS a choice used only
  by rules with big numbers is a `bigint`; in Rust a choice used with several
  number types is a `u8` (`Size::large.into()`).

More types:

- **A record inside a record**: a field may hold another record
  (`to: Address`), read as `order.to.zone` and passed as `delivery(order.to)`.
  A record cannot hold itself; that is an error naming the records.
- **A choice answer**: when every `->` answer is a value of one choice
  (`-> small`, `-> large`), the answer is that choice. Callers may pass it
  where the choice is taken (`fee(box_for(w))`), and `axiom run` prints the
  value's name. Answers mixing values and numbers stay numbers.
- **A record answer**: `quote(price, qty) -> Bill` answers a `Bill`. Give
  each field its own lines under `field:`; there `answer` is that field, so
  its `promise` lines go there too. Lines above the fields (`need`, named
  values) are shared. Read a field with `quote(price, qty).goods`, or pass
  the whole answer to a rule that takes a `Bill`. Underneath, each field is
  its own rule (`quote__goods`), proven on its own; `--emit` adds
  `quote(price, qty)`, which gives an object in JS, a dict in Python, a
  `Bill` struct in Rust and Go.
- **Lists of types**: `sizes[n]: Size` is a list of choices; each item is a
  value. `lines[n]: Line` is a list of records: `sum(lines.price)` adds a
  field, `count(lines)` is the length. `--emit` takes one list per field
  (`lines__price`, ...).
- **Decimals**: a choice input works in a rule `with 2 decimals`; it keeps
  its place (pass `Size.large`, or `large` to `axiom run`). Passing it to a
  rule with other places is an error.
- **Types from another file**: `from shop: Order, Size, fee` brings the types
  and the rules. A type the named rules take comes too. Calls to `fee` are
  checked, and an `Order` is passed whole. A `from` line names a file next to
  the one it is in: `from lib/tax: charge` in `rules.ax`, `from money: fee` in
  `lib/tax.ax`. A type reaches a file through the files that name it, and the
  answer range of an imported rule bounds arithmetic on its answer, as inside
  one file (`fee(p) + p` needs `fee` to answer a bounded value). `as` gives
  a name here: `from shop: Order as Purchase, fee as price_fee`. Two files may
  not both define a rule of the same name, even with `as`. A rule that
  answers a record (`quote(price, qty) -> Bill`) comes with its fields, so
  `quote(price, qty).goods` works in the file that names it. Example:
  `examples/modules`.

| Not yet | Instead |
|---|---|
| A promise naming two fields of a record answer (`answer.tax <= answer.goods`) | A promise on each field |
| A record answer whose field is a record | Answer the inner record from its own rule |
| Lists of a fixed size holding a type (`sizes[3]: Size`) | A list of any length (`sizes[n]`) |
| Calling a rule from another rule when it takes a list and its answer is not one `sum`: `repeat`, recursion, `min`, `max`, a list answer, or a rule in another file | Call it from `--emit` or `axiom run` |
| A promise about `sum(lines.price * lines.qty)` (as for two lists, above) | A promise that follows from the ranges |

### Numbers must stay in range

- `+` and `*` must not go above 4294967295 (or outside -2147483648 to
  2147483647 in `can be negative` rules): add `need name <= limit` lines for
  their inputs. The error names the inputs that have no limit. A limit also
  carries through `need` lines that compare inputs: `need used <= days` and
  `need days <= 31` keep `used` at 31 or below.
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

  ask "Price" for price
  ask "Min" for min
  ask "Max" for max
  show result
  when "Fit" is clicked
    result = fit_price(price, min, max)
```

| Line | Meaning |
| --- | --- |
| `from logic: a, b` | Use rules `a` and `b` from `logic.ax` |
| `page Name` | Start a page |
| `save a, b = 0` | Values that start at 0 and are kept after reload |
| `a, b = 0` | Values that start at 0 and reset on reload |
| `ask "Label" for a` | An input box for `a` |
| `show a` | Show the value of `a`; any calculation or rule works too (`show price * quantity`) |
| `show "Text"` | Show fixed text |
| `show "Text" if a > 100` | Show the text only while the condition holds |
| `save prices[3] = 0` | A list of three values (`prices_1` to `prices_3`) |
| `ask "Price" for prices` | One input box per item: "Price 1", "Price 2", ... |
| `save diff = 0 can be negative` | Values that may go below 0 |
| `save price = 0 with 2 decimals` | Values typed and shown with 2 decimal places |
| `save d = 0 can be negative with 2 decimals` | Both |
| `save grade = ""` | A text value, for a rule with text answers |
| `save joined = yes` | A yes/no value (a check box); a rule gets 1 for yes, 0 for no |
| `save names[4] = ""` | Text values, like people's names |
| `    owe = to_pay(paid, share)` | (under `when`) Fill list `owe` from a rule with a list answer |
| `    sends = settle(owe)` | (under `when`) Who sends whom how much so every `owe` reaches 0 |
| `show transfers(sends, names)` | "name → name amount" lines, a copy button, and saved past results |
| `save payments = table of amount, paid` | A table: one column per value (a list gives one per item), kept after reload |
| `    add amount, paid to payments` | (under `when`) Add a row with the current values, columns in order |
| `    clear payments` | (under `when`) Remove every row |
| `show payments` | The table, one row per line |
| `when "Label" is clicked` | A button; on click, the lines below it run in order |
| `    a = rule(...)` | (under `when`) Set `a` to the rule's answer |

A `when` block may set several values; each line sees the values set above it,
and if one rule fails, none of them change:

```
when "Calc" is clicked
  total = cart(prices)
  tax = vat(total)
```

Pass a list to a rule by name: `total = cart(prices)`.

`settle(owe)` takes the list of what each person still owes (below 0: is owed)
and fills a list with one item per pair of people, so `owe[4]` needs
`sends[16]`; item `(i - 1) * 4 + j` is what person `i` sends person `j`.
People who owe pay in list order to people who are owed, in list order.
`settle` is built in and proven once, in Lean, for any number of people:
[proofs/lean/Settle.lean](../proofs/lean/Settle.lean) shows that when the
balances add up to 0, what each person sends is what they owe and what each
receives is what they are owed, so every balance ends at exactly 0; a call whose balances do not add up to 0 is refused. It is not
proven again on each build. Tests check the generated code against this on
random balances, and that there are at most n - 1 transfers. Up to 21 people,
each balance within ±100,000,000. It does not promise the fewest
transfers. See [examples/group-settle](../examples/group-settle).
A value with decimals goes only into a rule with the same places. On a page,
values can be calculated and compared (`show price + tax`,
`show "Pricey" if price > 10.5`); values with fewer places are raised to the
most places used (`1.5 + 0.25` is `1.75`). Values that can be
negative are only shown or compared, and calculated in a rule. A whole number
up to 2147483647 may go into a `can be negative` rule. A rule with text answers
sets a text value: `grade = grade_of(score)`.

## When it fails

| Message | What to do |
| --- | --- |
| `the last line must be else -> value` | Add a final `else -> value` line for when no line above matches |
| `` `->` needs a condition, `else`, or `always` before it `` | Write `else -> value` after other `->` lines, or `always -> value` for a rule with one answer |
| `may go outside 0 to 4294967295` | Add the `need` lines the error names |
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
reloads. It starts even when the first build fails and shows the error in the
browser until then.

A `promise` that some allowed input breaks is reported with that input:

```text
promise 2 does not hold for total=294896131 (answer 268087391)
```

`axiom check --report check.json` also writes the outcome as JSON for tools and
AI agents: `{"ok": true}`, or `{"ok": false, "error": {...}}` with `message`
and, when known, `file`, `rule`, `line`, `promise`, `inputs`, `answer`, `hint`.

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

## Proving with Lean

`axiom build --lean` also writes each rule's proof as a Lean theorem
(`dist/lean/<rule>.lean`, or next to the output for one file) and checks it
with `lean` when it is installed. Lean's kernel checks the answer, so the
proof no longer rests on Z3. Each rule prints one result:

| Result | Means |
|---|---|
| `PROVED` | Lean accepted the theorem |
| `FAILED` | Lean found an input the rule gets wrong; the build fails |
| `UNPROVEN` | Lean ran out of time; neither proven nor disproven. The build fails |
| `NOT_COVERED` | The rule uses something not translated yet (arrays, records, loops in `fn`). The build fails |
| `SKIPPED` | `lean` is not installed; the file is still written |

Lean decides each rule by turning its words into bits. Dividing or
multiplying by an input can make that too slow. Two things help:

- An input with at most 64 values (`need people >= 1`, `need people <= 50`)
  is tried one value at a time, automatically.
- A `hint` line names a fact Lean proves once and then uses in place of the
  division:

```
fee_first(a, b, rate) with big numbers
  ...
  always -> fee(a + b, rate) * a / (a + b)
  promise answer <= fee(a + b, rate)
  hint mul_div_le(fee(a + b, rate), a, a + b)
```

| Hint | Fact | Write it for |
|---|---|---|
| `mul_div_le(x, y, z)` | `x * y / z <= x` when `y <= z` and `x * y` stays in range | `x * y / z` exactly as the rule writes it |

Z3 and the generated code ignore `hint` lines; a wrong hint only makes the
Lean proof fail.

## Workflow

```bash
axiom new my-app
cd my-app
axiom check      # rules are valid and proven
axiom dev        # live app; edits to .ax files rebuild it
axiom build      # static output in dist/
```

Use `axiom run file.ax -- <inputs>` to try a single rule.

## Using rules from JavaScript, TypeScript, Rust, Python or Go

`axiom build --emit js` also writes `dist/contracts.js` and `dist/contracts.d.ts`,
one function per proven rule. A rule with a list answer takes and returns arrays:

```ts
import { share } from "./dist/contracts.js";
share(10000, [1, 1, 2, 0], [0, 0, 0, 0], 100); // [2500, 2500, 5000, 0]
```

A wrong array length or a value outside the `need` lines throws.

Add `use settle for 4` to a rules file to prove `settle` for 4 people there;
then `settle(owe)` takes the 4 balances and returns the 16 transfers, item
`(i - 1) * 4 + j` from person `i` to person `j`.

`axiom build --emit rust` writes `dist/contracts.rs` instead: put it in your
crate as `mod contracts;`. Each rule returns `Result<_, AxiomError>`, lists are
arrays (`share(10000, [1, 1, 2, 0], [0; 4], 100)`), and `settle(&owe)` returns
a `Vec<i32>`. It needs no other crate.

`axiom build --emit python` writes `dist/contracts.py`: `import contracts`.
Inputs are `int` (`bool` for yes/no inputs), lists are Python lists
(`share(10000, [1, 1, 2, 0], [0] * 4, 100)`), and `settle(owe)` returns a list.
A value outside the `need` lines raises `contracts.AxiomError`; a value of the
wrong type or size raises `TypeError`. It needs only the standard library.

`axiom build --emit go` writes `dist/contracts.go` (`package contracts`): put
it in a `contracts` folder of your module. Names are exported (`fee_first`
becomes `FeeFirst`), each rule returns `(answer, error)` with a
`*contracts.AxiomError` outside the `need` lines, lists are arrays
(`Share(10000, [4]uint32{1, 1, 2, 0}, [4]uint32{}, 100)`), and `Settle(owe)`
takes and returns `[]int32`. It needs only the standard library.
