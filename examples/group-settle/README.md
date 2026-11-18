# Group settle (모임 정산)

Four people, any number of payments. For each payment enter the amount, who
paid how much, who joined (check box), each person's ratio (1 = normal,
2 = double, up to 4), and any amount set aside for someone, then press
결제 추가. The page keeps the running totals and lists who sends whom how much
("철수 → 민수 10000"). 복사 copies the list; 기록 저장 keeps it below as a past
result; 새 모임 starts over.

| Rule | What it proves |
| --- | --- |
| `part` | Each person's ratio, 0 if they did not join |
| `share` | Set-aside amount first, the rest split by ratio, rounded up to the unit (1, 10, or 100 won) |
| `add_up` | Running totals of shares and of money paid |
| `owe` | What each person still owes (below 0: is owed) |
| `settle` (built in) | Each transfer is at least 0, at most what the sender owes, and only from someone who owes to someone who is owed |

```bash
axiom check && axiom build   # prove 38 rules (about 15 seconds) and emit dist/
axiom dev                    # live app
```

Rounding up can make the shares add up to a little more than was paid. That
extra is not sent anywhere: the last person who owes sends a little less.
