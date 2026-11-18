# Invoice

청구서 계산 규칙 (#150). `axiom check`로 모든 규칙이 모든 입력에 대해 증명됩니다.

| 규칙 | 계산 |
| --- | --- |
| `prorate(price, used, days)` | 한 달 요금 중 쓴 날만큼, 원 미만 버림 |
| `change(old, new, used, days)` | 달 중간 플랜 변경: 남은 날만큼 더 낼 돈 (음수면 환불) |
| `vat(supply)` | 부가세 10%, 원 미만 버림 |
| `supply_of(total)`, `tax_of(total)` | 부가세 포함 금액을 공급가액과 세액으로 |
| `income_tax(pay)`, `local_tax(pay)` | 원천징수 3%, 지방소득세 0.3% (각각 10원 미만 버림) |

금액은 4억 원 이하입니다. 더 크면 `*`가 넘칠 수 있어 증명이 거부합니다.

다른 언어에서: `axiom build --emit js` 또는 `--emit rust`.
