# Invoice

청구서 계산 규칙 (#150). `axiom check`로 모든 규칙이 모든 입력에 대해 증명됩니다.

| 규칙 | 계산 |
| --- | --- |
| `prorate(price, used, days)` | 한 달 요금 중 쓴 날만큼, 원 미만 버림 |
| `change(old, new, used, days)` | 달 중간 플랜 변경: 남은 날만큼 더 낼 돈 (음수면 환불) |
| `vat(supply)` | 부가세 10%, 원 미만 버림 |
| `supply_of(total)`, `tax_of(total)` | 부가세 포함 금액을 공급가액과 세액으로 |
| `income_tax(pay)`, `local_tax(pay)` | 원천징수 3%, 지방소득세 0.3% (각각 10원 미만 버림) |
| `fee(amount, rate)` | PG 수수료, `rate`는 1만분의 1 단위 (330 = 3.3%) |
| `payout(amount, rate)` | 가맹점 정산액. `promise`: 수수료와 합치면 정확히 결제 금액 |
| `fee_first(a, b, rate)` | 한 번 결제한 두 상품의 수수료 중 첫 상품 몫. 둘째 몫은 `fee(a + b, rate) - fee_first(a, b, rate)` |

금액은 4억 원 이하(수수료는 400만 원, 상품별 분배는 1만 원 이하)입니다. 더 크면
`*`가 넘칠 수 있어 증명이 거부합니다. 숫자가 32비트라서 생기는 한계이고,
`fee_first`는 범위 검사가 `fee`의 전체 범위로 판단해 더 좁습니다.

다른 언어에서: `axiom build --emit js` 또는 `--emit rust`.
