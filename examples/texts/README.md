# Text choices (`type Grade = "A" | "B" | "C"`)

A choice whose values are quoted texts. A text value is its place, from 0, like
a word value. Inputs and answers take the type, and `grade is "A"` compares it.

| Rule | What it proves |
| --- | --- |
| `bonus` | A text grade gives 10, 5 or 0: never above 10 |
| `letter` | A score gives a grade |
| `total` | A score's grade gives its bonus: never above 10 |

```bash
axiom build --lean           # check each proof with Lean
axiom build --emit python    # dist/contracts.py: class Grade with A = 0, B = 1, C = 2
```
