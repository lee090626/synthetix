# Split bill (더치페이)

Enter the total and the number of people. `per_person` is what each person
pays, rounded up to 100 won; `leftover` is what is left after everyone pays.
For 10,001 won and 3 people: 3,400 won each, 199 won left.

```bash
axiom check && axiom build   # prove both rules and emit dist/
axiom dev                    # live app
```
