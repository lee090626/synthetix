# Lists of any length (`prices[n]`)

Rules over lists with any number of items, up to a limit set by `need n <= ...`.

| Rule | What it proves |
| --- | --- |
| `cart` | The total of up to 1000 prices, capped at 200000, is never above the sum |
| `balance` | What a group of up to 50 paid minus their shares stays within ±n × 1000000 |
| `order` | Prices (big numbers) plus a fee per item is never below the prices' sum |
| `spread` | The most expensive price minus the cheapest is never above 100000 |
| `worst` | The smallest paid-minus-share in the group is at most 1000000 |
| `above` | Each price minus the cheapest is never above that price (a list answer) |
| `net` | Prices minus discounts add up to the prices' sum minus the discounts' sum |

```bash
axiom check                     # prove every rule
axiom build --emit python       # dist/contracts.py: cart([1, 2, 3]) == 6
```

Each item is proven like a rule of its own; the rest of the rule is proven for
every sum the items allow. That a sum of any length stays in that range is
proven in Lean: `proofs/lean/Lists.lean`. See "Lists of any length" in
`docs/writing-specs.md`.
