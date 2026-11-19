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

모든 규칙이 `with big numbers`(64비트)라서 금액은 1조 원 이하(상품별 수수료 분배는
1억 원 이하)입니다. `fee_first`는 범위 검사가 `fee`의 전체 범위로 판단해 더 좁습니다.
64비트 규칙은 아직 페이지와 `axiom app-run`에서 쓸 수 없습니다(`axiom run`은 됩니다).

다른 언어에서: `axiom build --emit js`(`bigint`), `--emit rust`(`u64`/`i64`), `--emit python`(`int`), `--emit go`(`uint64`/`int64`).

## 앱에서 쓰기

`app/`에 같은 청구서(월 49,000원 플랜, 31일 중 12일 사용, PG 수수료 3.3%)를
TypeScript와 Rust로 계산하는 예제가 있습니다. 이 폴더에서:

```sh
axiom build --emit js && node app/main.ts        # Node 23.6 이상
axiom build --emit rust && rustc --edition 2024 app/main.rs -o bill && ./bill
```

둘 다 같은 결과를 냅니다:

```text
청구액 18967
공급가액 17242 세액 1725
PG 수수료 625 정산액 18342
거부: prorate: input violates a requires clause
```

## 손으로 짜면 생기는 버그를 증명이 잡는 사례

아래 규칙들은 흔한 손구현을 그대로 옮기고, 지켜야 할 성질을 `promise`로 적은
것입니다. `axiom check`가 각 버그를 깨는 입력을 찾아 보여 줍니다.

**일할 계산: 나누기를 먼저 하면 원 미만이 매번 버려짐**

```text
prorate(price, used, days)
  need days >= 28 and days <= 31
  need used <= days
  need price <= 10000000
  always -> price / days * used
  promise answer * days <= price * used
  promise answer * days + days > price * used
```

```text
promise 2 does not hold for price=9388880, used=31, days=31 (answer 9388877)
```

한 달을 다 썼는데 3원이 사라집니다. 이 예제의 `prorate`는 `price * used / days`
(곱하기 먼저)로 증명을 통과합니다.

**수수료 분배: 상품마다 따로 떼면 합계가 1원 틀림**

```text
second_fee(a, b, rate)
  ...
  always -> fee(b, rate)
  promise fee(a, rate) + answer is fee(a + b, rate)
```

```text
promise 1 does not hold for a=7084, b=8897, rate=943 (answer 838)
```

따로 떼면 668 + 838 = 1,506원, 한 번에 결제한 수수료는 1,507원입니다. 이 예제의
`fee_first`는 둘째 몫을 `fee(a + b, rate) - fee_first(a, b, rate)`로 정해 합계가
항상 맞습니다.

**부가세 분리: 합계에서 10%를 빼면 공급가액이 틀림**

```text
supply_of(total)
  need total <= 400000000
  always -> total - total / 10
  promise answer + answer / 10 <= total
  promise answer + answer / 10 + 1 >= total
```

```text
promise 2 does not hold for total=369098700 (answer 332188830)
```

공급가액은 합계의 10/11입니다. 이 예제의 `supply_of`는 `total * 10 / 11`로 두
약속을 모두 지킵니다.
