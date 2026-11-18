# Group settle (모임 정산)

Four people, two rounds (1차, 2차). For each round enter the amount, who paid
how much, who joined (check box), each person's ratio (1 = normal, 2 = double,
up to 4), and any amount set aside for someone. Pick the rounding unit (1, 10,
or 100 won), then press 정산: the page lists who sends whom how much
("철수 → 민수 10000"). 복사 copies the list; 기록 저장 keeps it below as a past
result.

| Rule | What it proves |
| --- | --- |
| `part` | Each person's ratio, 0 if they did not join |
| `share` | Set-aside amount first, the rest split by ratio, rounded up to the unit |
| `balance` | What each person still owes (below 0: is owed) |
| `settle` (built in) | Transfers that bring every balance to 0 |

```bash
axiom check && axiom build   # prove 28 rules (about 30 seconds) and emit dist/
axiom dev                    # live app
```

Rounding up can make the shares add up to a little more than was paid. That
extra is not sent anywhere: the last person who owes sends a little less.
