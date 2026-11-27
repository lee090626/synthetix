# Modules (`from` across folders)

A `from` line names a file next to the one it is in. `rules.ax` takes
`charge` and `Order` from `lib/tax.ax`; `lib/tax.ax` takes `fee` and `Order`
from `lib/money.ax`, so `Order` reaches `rules.ax` through the chain. The
answer range of `fee` (at most 100) bounds `charge` in `lib/tax.ax`, and that
of `charge` bounds `bill`.

| Rule | What it proves |
| --- | --- |
| `fee` | A 1% fee: at most 100 for a price up to 10000 |
| `vat` | A 10% tax: at most 1000 |
| `charge` | Fee plus tax for one order |
| `bill` | Price times quantity, plus `charge`: never below the goods |

```bash
axiom check        # prove the rules' ranges and promises
axiom build --lean # check each proof with Lean
```
