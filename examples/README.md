# 예제

예제는 목적별로 나눠져 있습니다.

| 폴더 | 내용 |
| --- | --- |
| `basics/` | 정식 `contract` 문법과 기본 산술·비트 연산 |
| `basics/increment_fn.ax` | 명시적 구현을 verifier가 확인하는 `fn` |
| `basics/external_counter.ax` | runtime adapter 경계인 `extern fn` |
| `properties/` | 구현 코드를 쓰지 않고 결과의 조건만 적는 property-first 계약 |
| `friendly/` | `program`, `gives`, `number`, `plus`처럼 읽기 쉬운 문법 |
| `errors/` | 문법 오류·타입 오류·검증 실패를 재현하는 예제. `unsat_precondition.ax`와 `unsat_postcondition.ax`는 v4 상태 모델 회귀 fixture다. |
| `web-counter/` | 여러 `.ax` 파일을 연결한 v2 웹 예제 |
| `v6-release/` | 배열·제한된 loop·compositional proof와 application effect의 static release fixture |
| `v3-status/` | `bool` 상태와 조건부 화면 |
| `v3-todo/` | 고정 항목 상태 앱 |
| `v3-todo-v31/` | 입력·record·list·reducer·persist를 포함한 Todo 앱 |

## 기본 실행

```bash
cargo run -p axiom-cli -- run examples/basics/mul8.ax -- 5
cargo run -p axiom-cli -- run examples/properties/property_max.ax -- 3 8
cargo run -p axiom-cli -- run examples/friendly/easy_min.ax -- 2 5
```

`errors/` 안의 파일은 일부러 실패하도록 만든 예제입니다. 실행 중 생성되는
WASM, Z3 trace, `.synthetix/`, `dist/`는 소스가 아니며 `.gitignore` 대상입니다.

웹 예제는 각 폴더에서 실행합니다.

```bash
cd examples/web-counter
cargo run -p axiom-cli -- dev
```
