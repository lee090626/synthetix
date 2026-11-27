# Types (`type`, `match`)

A choice (`Size`: small, medium, large) and a record (`Order`: price, quantity,
size). Each rule is proven like any rule: underneath, a choice value is its
place from 0 and a record is one input per field.

| Rule | What it proves |
| --- | --- |
| `shipping` | A fee per size: always from 3000 to 15000 |
| `subtotal` | Price times quantity of an order |
| `total` | Goods plus shipping, free from 100000: never below the goods, at most 15000 above |
| `wrap_fee` | 2000 for a large box, else 0 |

```bash
axiom check                  # prove every rule
axiom build --lean           # check each proof with Lean
axiom build --emit python    # dist/contracts.py: total(1000, 3, Size.large, 70000) == 15000
```
