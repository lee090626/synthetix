# I1 결과 비유일성 분석 구현 기록

상태: 제한된 verifier/CLI 기능 구현과 검증. 실제 사용자 명세 보완과 Reference App 통합은 아직 완료하지 않았다.
## 사용법

저장소 루트에서 실행한다.

```sh
cargo build --locked -p axiom-cli
./target/debug/axiom analyze tests/fixtures/first-product/underspecified.ax
./target/debug/axiom analyze tests/fixtures/first-product/capped.ax --report /tmp/capped-analysis.json
./target/debug/axiom analyze tests/fixtures/first-product/capped.ax --timeout-ms 0
```

`analyze`는 JSON을 stdout에 출력한다. `--report`는 같은 JSON을 지정한 파일에 저장한다. 후보를 합성하거나 WASM을 생성하지 않는다. 기존 build/run의 v4 보고서와 `AMBIGUOUS_CANDIDATES`는 유지한다. 새 분석 보고서는 독립적인 version 1이다.

지원하는 입력은 단독 `contract`의 u32와 기존 parser가 허용하는 길이 1~16의 고정 u32 배열이고, 결과는 u32다. 현재 제품은 그중 길이 4의 배열만 사용한다. 기존 SMT 번역으로 local 연산·조건·고정 배열을 처리한다. 함수 호출과 프로젝트 연결, fn/extern, signed/bool 입력·결과는 이 분석의 범위 밖이며 오류로 거부한다. 결과에 의존하는 전제나 `result` 이름의 shadowing도 거부한다.

## 판정 의미

| status | 의미 |
| --- | --- |
| NON_UNIQUE | 같은 유효 입력에서 서로 다른 두 결과가 조건을 만족한다. witness에 입력과 두 결과를 제공한다. |
| AT_MOST_ONE | 같은 유효 입력에 가능한 결과는 최대 하나다. 모든 입력에 결과가 존재한다는 뜻은 아니다. |
| UNSATISFIABLE_PRECONDITION | 유효 입력이 없다. 유일성 성공으로 판정하지 않는다. |
| UNKNOWN | solver unknown 또는 시간 예산 소진으로 판정하지 못했다. reason을 제공한다. |

프로세스 exit 0은 분석 보고서를 정상적으로 전달했다는 뜻이다. 유일성 성공 판정은 status를 확인해야 한다. 지원 밖 문법·타입·정의되지 않은 배열 접근과 파일·보고서 오류는 exit 1이다.

`result_existence`는 항상 `NOT_CHECKED`, `exactly_one`은 null이다. 별도의 범용 결과 존재성 검사를 추가하지 않았다. 예를 들어 다음 계약은 AT_MOST_ONE이지만 x=3에서 결과가 없다.

```axiom
contract partial(x: u32) -> u32 {
    requires x <= 10;
    ensures x == 5 && result == 100;
}
```

동일한 명세·입력 범위에 대해 기존 verifier가 모든 유효 입력에서 정의된 후보의 후조건을 증명하면, 그 후보가 결과 존재성의 근거가 될 수 있다. 분석 결과만으로 이를 추정하지 않는다. 위 부분 계약은 Const(100)의 기존 후보 증명에서 반례를 반환하는 것도 테스트했다. 후보 증명과 AT_MOST_ONE을 결합한 앱 표시는 후속 사용자 흐름에서 연결해야 한다.

## 검사 과정과 시간 예산

타입과 지원 범위를 확인하고, 전제의 배열 접근이 정의되는지 검사한 뒤 유효 입력의 존재 여부를 확인한다. 후조건의 배열 접근도 전제 아래 두 결과에 대해 정의되는지 확인한다. 이후 동일 입력·서로 다른 결과·두 후조건을 기존 SMT 번역으로 검사한다.

입력과 결과의 solver 심볼은 fresh로 생성한다. 사용자 입력 이름이나 배열을 펼친 이름이 서로 alias되어 입력 영역이 잘못 축소되는 것을 막는다. witness 입력은 선언 순서이고 배열은 원소 순서로 펼친다. 보고서의 input_layout을 함께 확인한다.

기본 예산은 10,000ms다. 검증 단계마다 남은 예산을 solver에 전달한다. 0ms는 즉시 판정 불가로 처리한다. 파일 파싱, SMT 식 생성, solver 정리 등 비 solver 처리를 강제로 중단하는 전체 프로세스 wall-clock deadline은 아니다. 하위 밀리초는 solver 설정에서 1ms로 반올림할 수 있다. 컴파일러 의미 보존이나 사용자 의도는 이 분석에서 증명하지 않는다.

## 검증

- Verifier unit tests cover witness replay, at-most-one, independent array/scalar inputs, missing outputs, empty domains, timeouts, supported syntax, and array bounds.
- CLI integration tests cover status and witnesses, empty/impossible domains, zero timeout, report output, no WASM emission, source overwrite protection, and invalid options.

재현 명령:

```sh
cargo test --locked -p axiom-verify uniqueness
cargo test --locked -p axiom-cli --test result_analysis
cargo test --locked --workspace
cargo clippy --locked --workspace --all-targets -- -D warnings
```

소스: [verifier 분석](../../crates/axiom-verify/src/uniqueness.rs), [CLI](../../crates/axiom-cli/src/main.rs), [integration tests](../../crates/axiom-cli/tests/result_analysis.rs).

## 남은 작업

실제 사용자에게 witness가 이해되고 부족한 조건을 보완할 수 있는지 확인해야 한다. 제품 계산 차등 검증 코드는 [product_differential.rs](../../crates/axiom-cli/tests/product_differential.rs)에 있다. Reference App, 입력·저장·검증 변경 복구, 외부 사용자 완주는 아직 완료하지 않았다. 결과 존재성 자동 판정과 브라우저 UI를 이번 기능에 함께 넣지 않았다.
