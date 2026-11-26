# More types (`-> Bill`, records in records, choice answers)

A rule answering a record, a record holding a record, a choice answer and a
list of choices. Every rule is proven like any rule.

| Rule | What it proves |
| --- | --- |
| `quote` | A `Bill`: goods and tax (each with its own range) and the box size |
| `pay` | Goods plus tax, never below the goods |
| `box_for` | A `Size` by weight |
| `delivery` | A fee by zone and box size, at most 4900 |
| `order_total` | An order's price plus delivery to its address |
| `large_boxes` | How many boxes are large: at most the count |

```bash
axiom check                  # prove every rule
axiom build --lean           # check each proof with Lean
axiom build --emit python    # dist/contracts.py: quote(1000, 12) == {"goods": 12000, "tax": 1200, "size": 1}
```
