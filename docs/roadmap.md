# Synthetix 공식 로드맵

## 한 문장 목표

Synthetix는 범용(general-purpose) programming language가 아니라,
**명세 → 합성 → 증명 → 실행**을 하나의 흐름으로 제공하는 verified
programming language다.

사용자는 `.ax` 파일에 구현 방법보다 반드시 성립해야 하는 조건을 적는다.
Synthetix는 후보 구현을 찾고, Z3로 검증한 뒤, 검증된 결과만 WASM으로
방출한다.

```text
.ax contract
  → parse / type check
  → candidate synthesis
  → Z3 proof
  → WASM lowering
  → runtime execution
```

WASM은 언어의 정체성이 아니라 검증된 순수 로직을 실행하는 기본 backend다.
웹 runtime은 범용 웹 프레임워크가 아니라 verified core와 상태·effect 세계를
연결하는 얇은 계층이다.

## 버전별 중심축

| 버전 | 중심 목표 |
| --- | --- |
| **v4** | Synthetix의 증명 결과를 신뢰할 수 있게 만든다. |
| **v5** | 검증된 코어와 웹·상태·effect·API 경계를 연결한다. |
| **v6** | 증명 가능한 프로그램의 크기와 구조를 확장한다. |

기능 수보다 soundness와 명확한 verification boundary를 우선한다. 서버,
DB, 인증 내부까지 Synthetix가 증명하는 것은 목표가 아니다.

## 현재 위치

| 영역 | 현재 상태 |
| --- | --- |
| v1 | 단일 contract의 합성·Z3 검증·WASM 실행 완료 |
| v2 | 여러 `.ax` 파일, import/export, `axiom dev`, 정적 web bundle 1차 구현 |
| v3.1 | 상태·record·list·text·reducer·persist 기반 애플리케이션 1차 구현 |
| v4.0 | property-first `max`·`min`·`clamp`·조건부 후보 합성 1차 구현 |
| **현재 다음 단계** | **v5.0 Application Boundary Baseline** |

v4.0은 방향을 검증한 실험적 slice다. v4.9 안정 버전으로 보기 전에
의미론과 backend 보존성을 먼저 고정해야 한다.

## 공통 완료 기준

각 버전은 코드가 존재하는 것만으로 완료하지 않는다.

- **static:** parser, type checker, IR, 문서의 계약이 일치한다.
- **proof:** Z3 결과와 실패 상태가 구분된다.
- **runtime:** WASM 또는 웹 runtime에서 같은 의미가 실행된다.
- **regression:** 기존 contract와 실패 예제가 계속 같은 결과를 낸다.
- **boundary:** 검증되는 영역과 검증되지 않는 외부 effect가 문서화된다.

---

# v4 — 증명 신뢰성

v4는 synthesis 기능을 많이 추가하는 버전이 아니다. Synthetix가 생성한
결과를 사용자가 믿을 수 있도록 의미론, 결과 상태, 검색 경계, backend 보존을
고정하는 버전이다.

## v4.1 — Semantic Baseline

### 목표

동일한 AMI 프로그램이 reference evaluator, Z3, WASM에서 같은 의미를 갖는지
확인한다.

### 필요한 이유

Z3가 증명한 식과 WASM이 실행한 식의 의미가 다르면, 검증은 실제 프로그램을
증명한 것이 아니다. 특히 overflow, signed 연산, division, remainder, shift는
backend마다 차이가 생기기 쉬운 경계다.

### 주요 작업

1. `u32`, `i32`, `bool`의 의미 표 작성
2. overflow, division, remainder, shift, signed/unsigned comparison 규칙 확정
3. 현재 `axiom-ami::Expr`를 canonical typed IR로 사용할 수 있는지 검토
4. reference evaluator 구현
5. AMI evaluator의 의미 확인
6. AMI ↔ Z3 ↔ WASM differential/conformance test 추가
7. `requires`의 satisfiability 검사 추가
8. 기존 `max`, `min`, `clamp` regression test 고정

새 Core IR은 무조건 만들지 않는다. 현재 AMI가 타입·의미·source 정보까지
감당하지 못한다는 근거가 있을 때만 추가한다.

### 완료 기준

- 같은 AMI 프로그램의 reference evaluator와 WASM 실행 결과가 일치한다.
- Z3가 사용하는 bit-vector 의미와 reference evaluator가 일치한다.
- overflow와 signed/unsigned 경계값이 회귀 테스트에 포함된다.
- 모순된 `requires`가 `UNSATISFIABLE_PRECONDITION`으로 식별된다.
- 기존 property 예제가 의미론 변경 없이 계속 통과한다.

### 의존성과 범위 밖

v4의 모든 후속 단계가 이 단계에 의존한다. 새 synthesis 기능, CEGIS,
ambiguity detection, 검색 resource limit은 v4.1 범위에 넣지 않는다.

### v4.1 구현 기록

상태: **완료**

- reference evaluator를 추가하고 AMI evaluator와 분리했다.
- `axiom-ami::Expr`를 현재 scalar pipeline의 canonical candidate IR로 유지하되,
  self-describing typed IR은 아니라는 한계를 문서화했다.
- `requires` satisfiability를 Z3로 별도 검사하고 모순된 계약을
  `UNSATISFIABLE_PRECONDITION`으로 거부한다.
- reference evaluator, AMI, Z3, WASM의 boundary/conformance 테스트와
  `max`·`min`·`clamp` regression을 추가했다.

검증: `cargo test --workspace`, `cargo fmt --all -- --check`,
`git diff --check` 통과.

남은 위험: `Expr` 자체에는 타입 metadata가 없다. browser runtime의 JavaScript
evaluator는 page/UI expression만 계산하며 verified contract body는 WASM export로
실행한다. v4.8 conformance test가 이 호출 경로를 검증한다.

## v4.2 — Contract/Proof/Synthesis 상태 모델

### 목표

계약 자체의 문제, synthesis 검색 결과, proof 결과를 서로 다른 상태로
표현한다.

### 필요한 이유

현재의 “verified candidate를 찾지 못함”만으로는 모순된 명세, 검색 공간
부족, timeout을 구분할 수 없다. 사용자가 다음 행동을 결정할 수 있는 오류
모델이 필요하다.

### 상태 모델

```text
ContractCheck:
  VALID
  INVALID
  UNSATISFIABLE_PRECONDITION
  UNSATISFIABLE_POSTCONDITION

SynthesisResult:
  FOUND
  NO_CANDIDATE
  TIMEOUT
  RESOURCE_LIMIT

ProofResult:
  VERIFIED
  COUNTEREXAMPLE
  SOLVER_UNKNOWN

Diagnostic:
  AMBIGUOUS_CANDIDATES
```

`UNSATISFIABLE_POSTCONDITION`은 검색 공간을 다 써버렸다는 뜻이 아니라,
유효한 입력 조건에서 `ensures`를 만족할 수 없는 계약 자체를 뜻한다.
`AMBIGUOUS_CANDIDATES`는 `SynthesisResult`가 아니라 현재 검색 공간에서
서로 다른 verified candidate를 여러 개 발견했다는 별도 diagnostic/warning이다.
strict mode에서는 이 diagnostic을 build failure로 승격할 수 있지만,
일반 모드의 `SynthesisResult`는 `FOUND`로 유지한다.

### 완료 기준

CLI, VSCode Problems/Output, JSON report가 같은 상태 모델을 사용한다.
상태마다 원인·사용자 대응·재현 입력을 표시한다.

### 의존성과 범위 밖

v4.1의 의미론과 precondition 검사 이후에 진행한다. 후보 grammar나 검색
알고리즘을 크게 늘리는 일은 v4.3 이후로 미룬다.

### v4.2 구현 기록

상태: **완료**

- `ContractCheck`에 `VALID`, `INVALID`,
  `UNSATISFIABLE_PRECONDITION`, `UNSATISFIABLE_POSTCONDITION`을 추가했다.
- `ProofResult`를 `VERIFIED`, `COUNTEREXAMPLE`, `SOLVER_UNKNOWN`으로 고정하고
  기존 `VerificationResult` 이름은 호환 alias로 유지했다.
- `SynthesisResult`에 `FOUND`, `NO_CANDIDATE`, `TIMEOUT`, `RESOURCE_LIMIT`을
  추가하고 후보 제한·timeout을 실제 결과로 보존했다.
- CLI, project web build/check, VSCode Problems/Output, JSON report에 같은
  상태 이름과 reason/next action/reproduction input을 연결했다.
- `AMBIGUOUS_CANDIDATES`는 diagnostic 타입만 정의하고 아직 검출하지 않았다.

검증: workspace test, `cargo clippy --workspace --all-targets -- -D warnings`,
format, CLI 성공 report와 `RESOURCE_LIMIT` 실패 report를 확인했다.

남은 위험: synthesis 실패 report의 proof 상태는 실제 solver가 `UNKNOWN`을
반환한 경우에만 채워진다. ambiguity 검출과 strict mode 승격은 v4.3 범위다.

## v4.3 — Typed Synthesis Kernel

### 목표

후보를 늘리기 전에 bounded synthesis의 검색 공간과 자원 사용을 통제한다.

### 필요한 이유

중첩 조건과 연산을 단순 enumeration으로 계속 추가하면 후보 수가 폭발한다.
검색 결과의 재현성과 종료 보장이 먼저 필요하다.

### 주요 작업과 완료 기준

- 타입별 candidate grammar
- canonicalization과 duplicate 제거
- 후보 비용 및 deterministic ordering
- counterexample 재사용
- 명시적인 CEGIS 구조
- 후보 수·시간·메모리 제한
- 검색 공간과 후보 수 benchmark

현재 검색 공간에서 서로 다른 verified candidate를 여러 개 발견하면
`AMBIGUOUS_CANDIDATES` diagnostic을 낸다. 이것을 의미론적
`UNDERCONSTRAINED`로 확정하지는 않는다. strict mode는 현재 검색 공간의
ambiguity diagnostic을 실패로 승격하는 수준으로 제한한다.

### 의존성과 범위 밖

v4.2 상태 모델에 의존한다. 새로운 loop, array, recursive search, ML-guided
search는 범위 밖이다.

### v4.3 구현 기록

상태: **완료**

- \`Expr\`에 구조적 ordering을 추가하고 contract context에서 후보 타입을
  추론·필터링하는 typed candidate kernel을 구현했다.
- 교환법칙이 있는 연산을 canonicalize하고 중복 후보를 제거했다. 비용순
  deterministic ordering으로 같은 입력이 같은 후보를 선택하도록 고정했다.
- \`CegisState\`에 sample과 마지막 counterexample을 모아 후보 검증마다
  재사용하도록 했다.
- 후보 수·timeout·보수적 후보 메모리 추정치를 각각 \`RESOURCE_LIMIT\`와
  \`TIMEOUT\`으로 보존했다.
- 첫 검증 후보 이후 다른 verified candidate가 발견되면
  \`SynthesisResult::FOUND\`와 별도로 \`AMBIGUOUS_CANDIDATES\` diagnostic을
  반환한다. 이는 의미론적 \`UNDERCONSTRAINED\` 판정이 아니다.
- WASM lowering과 CLI JSON report가 위 diagnostic을 전달한다.

검증: \`cargo test --workspace\`, \`cargo fmt --all -- --check\`,
\`cargo clippy --workspace --all-targets -- -D warnings\`, \`git diff --check\`,
CLI ambiguity report 수동 확인.

남은 위험: 메모리 제한은 allocator 실사용량이 아니라 후보 트리의
보수적 추정치다. strict mode의 정책 승격과 더 큰 검색 최적화는 후속 단계로
남긴다.

## v4.4 — Property Synthesis 확장

### 목표

구현 코드 없이 작성한 property에서 유용한 순수 scalar 구현을 찾는다.

### 주요 작업

- `max`, `min`, `clamp`의 일반화
- range 제한과 validation
- default branch와 constant branch
- bit mask
- wrapping subtraction
- semantics와 backend conformance가 확보된 뒤 division, remainder, right shift

### 완료 기준

각 property 예제에 대해 후보 생성, Z3 검증, WASM 실행, 경계값 회귀 테스트가
모두 존재한다. 검색 공간 밖의 실패는 `NO_CANDIDATE`로 보고하며 계약 모순과
섞지 않는다.

### 의존성과 범위 밖

v4.1의 의미론과 v4.3의 typed search kernel에 의존한다. 배열·반복문·대규모
자료구조 synthesis는 v6 연구 범위다.

나눗셈, 나머지, right shift는 현재 의미 표가 미정인 연산이므로 v4.4에서
추가하지 않는다. 별도 semantics 결정과 AMI·Z3·WASM conformance가 끝난 뒤에만
추가 후보가 된다.

### v4.4 구현 기록

상태: **완료**

- `max`, `min`, `clamp`, constant branch, default branch property를
  기존 typed kernel 위에서 회귀 고정했다.
- `property_at_least_ten`으로 range 조건과 constant branch를 검증했다.
- bit mask와 wrapping subtraction property를 AMI·Z3·WASM 실행으로 확인했다.
- `requires` 범위 검사를 synthesis 전에 수행하는 경계를 테스트했다.

검증: property별 unit test와 WASM 실행, workspace test, format, clippy,
`git diff --check`, CLI property 실행 통과.

남은 위험: division, remainder, right shift는 의미론과 backend 계약이 아직
없다. 임의의 후보 문법을 먼저 늘리지 않고 v4.5 이후 별도 semantics 작업으로
미룬다.

## v4.5 — `contract` / `fn` / `extern`

### 목표

명세를 합성하는 선언, 사용자가 작성한 구현, 외부 effect의 경계를 언어에
고정한다.

### 의미

```text
contract
  구현 없음
  synthesis 대상

fn
  사용자가 구현
  verifier가 검증

extern
  구현이 외부에 있음
  검증 경계 밖
```

`adapter`는 언어의 순수 함수 종류가 아니라 runtime/project 연결 설정으로
둔다. `program`은 별도 실행 모델이 아니라 `contract`의 쉬운 문법 alias로
취급한다.

### 완료 기준

세 종류가 parser, type checker, verifier, project linker, 문서에서 같은
의미를 갖는다. `contract`와 `fn`이 단순히 이름만 다른 함수가 아니다.

```text
fn increment(value: u32) -> u32 {
  return value +% 1;
  ensures result == value +% 1;
}

extern fn read_count(key: u32) -> u32;
```

`fn`은 하나의 명시적 `return` 표현식을 가져야 하며, 그 표현식은 같은 AMI로
내려가 Z3 검증을 통과해야 한다. `extern fn`은 서명만 기록하고 `check`는
허용하지만, runtime adapter 없이는 `build`/WASM 방출 대상이 아니다.

### 의존성과 범위 밖

v4.1 의미론과 v4.2 결과 모델에 의존한다. 외부 HTTP·DB·브라우저 API의
실제 구현은 v5 범위다.

### v4.5 구현 기록

상태: **완료**

- AST에 `Contract`·`Function`·`Extern` declaration kind와 `fn`
  implementation을 추가했다.
- parser와 type checker가 `fn return`과 `extern fn` 경계를 검사한다.
- 명시적 `fn` 구현을 AMI로 낮추고 기존 Z3 verifier와 WASM backend를
  재사용한다. 구현이 계약을 깨면 `INVALID`와 반례를 반환한다.
- `extern`은 `check`에서 유효한 경계 선언으로 남기고 synthesis/build에서는
  `EXTERN_OUTSIDE_VERIFICATION_BOUNDARY`로 방출을 거부한다.
- project linker와 VSCode syntax/completion이 세 선언 종류를 보존한다.

검증: parser·project linker·verifier·synth unit test, explicit `fn` CLI
check/run, extern CLI check 및 build 거부, workspace test/clippy/format 통과.

남은 위험: 함수 호출은 아직 contract를 inline하는 기존 project 경로에
머물러 있다. `requires` 증명과 `ensures` 가정으로 바꾸는 작업은 v4.6이다.

## v4.6 — Compositional Verification

### 목표

계약 호출 시 callee 구현을 계속 펼치지 않고 계약 요약으로 검증한다.

### 주요 작업

```text
caller가 callee.requires를 증명
callee.ensures를 caller 안에서 가정
```

- imported contract signature
- precondition discharge
- postcondition assumption
- contract summary
- 모듈 간 타입 검사
- 순환 호출 오류와 source 위치

### 완료 기준

callee의 구현을 inline하지 않아도 `requires`와 `ensures`만으로 caller를
검증할 수 있다. call-site의 precondition을 증명하지 못하면 상위 contract
diagnostic으로 노출한다. 내부 증명 과정에서 반례가 발견된 경우에만
`ProofResult.COUNTEREXAMPLE`을 근거로 첨부하며, call-site 진단을
`ProofResult` 자체로 대체하지 않는다.

### 의존성과 범위 밖

v4.5에서 contract의 의미와 외부 경계를 먼저 고정해야 한다. 일반 재귀,
loop invariant, 무제한 함수 호출은 v6 범위다.

### v4.6 구현 기록

상태: **완료**

- axiom-verify에 compositional verification API를 추가했다. caller는 callee
  requires를 call-site마다 증명하고, callee ensures를 summary constraint로
  가정한다.
- call-site precondition 실패는 ProofResult와 분리된
  CALL_PRECONDITION_VIOLATION diagnostic으로 보고하고 source line/column을
  보존한다. 실제 postcondition 모델만 ProofResult::COUNTEREXAMPLE이다.
- Project는 기존 web/runtime 호환용 expanded view와 원본
  verification_summaries를 함께 보존한다.
- project API가 원본 summary로 검증하며 web check/build의 실제 compile 경로가
  이 검증을 통과해야 WASM을 생성한다. expanded view는 candidate search에만 쓴다.
- callee 구현을 펼치지 않는 add_one 호출 검증과 precondition 실패 테스트를
  추가하고 caller/callee의 requires/ensures 조합을 project 및 web compile
  경로에서 검증한다.

검증: compositional verifier unit test, project summary 보존 test, workspace
test/clippy/format 통과.

남은 위험: summary resolution은 module import scope의 contract 이름을 사용하며,
재귀/loop는 지원하지 않는다.

## v4.7 — Verification Artifact

### 목표

검증 결과와 생성 산출물을 재현하고 추적할 수 있게 한다.

### 주요 작업

report에 다음을 기록한다.

- contract 이름과 source 위치
- candidate
- semantics version
- synthesis bound와 사용 자원
- counterexample
- solver 결과
- `verification_artifact_hash`
- `wasm_hash`

hash는 correctness proof가 아니다. `verification_artifact_hash`는 artifact
무결성, `wasm_hash`는 생성 파일 무결성을 나타낸다. correctness는
`ProofResult.VERIFIED`와 backend conformance가 담당한다.

### 완료 기준

같은 source·semantics version·bound로 실행하면 같은 report와 candidate를
재현할 수 있다. 검증되지 않은 candidate는 WASM으로 방출하지 않는다.

### v4.7 구현 기록

상태: **완료**

- CLI 성공/실패 report가 같은 v4 artifact schema를 사용한다. contract,
  candidate, semantics version, synthesis bounds, candidate_count, timeout/resource
  상태, counterexample, solver/proof result, 두 integrity hash를 기록하고,
  적용되지 않는 값은 null로 둔다.
- verification artifact 입력과 생성 WASM에 각각
  verification_artifact_hash와 wasm_hash를 기록한다.
- 두 hash는 artifact/file integrity fingerprint일 뿐 correctness proof가
  아니며, correctness는 ProofResult와 backend conformance가 담당한다.
- 동일한 source, bound, candidate로 다시 실행하면 report의 핵심 필드와
  candidate가 재현된다.

검증: CLI report 수동 확인, hash 필드 확인, workspace test/clippy/format 통과.

남은 위험: 현재 fingerprint는 외부 서명이나 provenance 체인을 제공하지
않는다. 배포 서명과 장기 artifact 저장은 v5 이후 release 작업이다.

### 의존성과 범위 밖

v4.2의 상태 모델과 v4.3의 resource 정보에 의존한다. proof explorer UI는
v6.8 이전에는 필수가 아니다.

## v4.8 — Backend Conformance Suite

### 목표

v4.1의 개별 differential test를 전체 backend 보존성 검증 체계로 확장한다.

### v4.1과의 차이

- **v4.1:** 의미론이 맞는지 확인하는 최소 기준선
- **v4.8:** 전체 candidate·ABI·runtime·browser 경로의 보존성 검증

### 주요 작업과 완료 기준

- AMI evaluator ↔ Z3 모델 ↔ WASM 실행 결과 비교
- random 및 boundary differential test
- zero/one/multi-argument ABI 검증
- signed/unsigned와 overflow 회귀
- generated WASM의 export·입력·출력 검증
- 순수 계산은 JS가 재구현하지 않고 WASM 호출
- web runtime에서 source contract와 실행 artifact 추적

### v4.8 구현 기록

상태: **완료**

- `axiom-synth`에 zero/one/multi-argument ABI, deterministic input matrix,
  unsigned wrapping, signed boundary, bool 결과를 함께 비교하는 conformance
  test를 추가했다.
- reference evaluator, AMI evaluator, Z3, WASM 실행 결과가 같은 candidate와
  입력에서 일치하는지 고정했다.
- WASM backend에 zero-argument export 실행 회귀를 추가했다.
- Node.js JS runtime에서 생성 `app.js`와 실제 generated WASM을 instantiate하고,
  generated button 경로로 export를 호출한다. overflow, signed/unsigned boundary,
  bool ABI 결과를 reference/AMI/Z3/WASM과 비교한다.

검증: workspace 테스트 및 Node.js browser-runtime integration test 통과.

남은 위험: 실제 브라우저별 최종 화면과 WASM 실행은 이 자동 conformance가
대체하지 않는다. v4.9에서는 release fixture와 수동 브라우저 smoke test를
동결된 검증 기준에 포함한다.

### 의존성과 범위 밖

v4.1의 기준선과 v4.7의 artifact 정보에 의존한다. 새로운 언어 기능,
WASI·Node·Edge backend 추가는 이 단계의 목표가 아니다.

## v4.9 — Freeze

### 목표

v4의 의미론과 검증 경계를 안정된 공개 계약으로 동결한다.

### 완료 기준

- semantics 문서 동결
- contract/result 상태 동결
- supported grammar 동결
- negative fixture와 regression corpus 확보
- CLI·VSCode·WASM release 검증
- 성능 기준과 검색 한계 공개
- migration 문서 작성

### v4.9 구현 기록

상태: **완료**

- `docs/semantics.md`에 `v4.1-scalar-baseline`의 frozen surface와 변경 시
  새 semantics version이 필요하다는 규칙을 기록했다.
- `ContractCheck`, `SynthesisResult`, `ProofResult`의 공개 상태와
  `AMBIGUOUS_CANDIDATES`·`CALL_PRECONDITION_VIOLATION` diagnostic 경계를
  동결했다.
- `examples/errors/unsat_precondition.ax`와
  `examples/errors/unsat_postcondition.ax`를 negative regression fixture로
  고정했다.
- CLI release build, 기본·signed WASM 실행, generated browser runtime의 실제
  WASM instantiate/export 호출, VSCode VSIX/source hash 및 activation smoke,
  전체 workspace test/clippy/format/diff 검증을 완료했다.
- `docs/v4-performance-baseline.json`에 benchmark별 candidate 수, 시간, search
  bounds를 기록하고 release 명령으로 재생성할 수 있게 했다.

검증 기준은 `docs/setup.md`의 fresh-checkout smoke test와 이 문서의 v4.8
backend conformance suite다. 상세한 호환성 규칙은
[`docs/v4-migration.md`](v4-migration.md)에 기록했다. v4.9 이후 기능 추가는
v4 semantics version과 freeze 기준을 깨지 않는 별도 버전 작업으로만 진행한다.

### 범위 밖

v4.9 이후에야 대규모 자료구조, loop, recursion, 새로운 runtime target을
검토한다. freeze 전에 기능을 추가해 의미론을 다시 흔들지 않는다.

---

# v5 — 검증 코어와 웹·effect·API 경계

v5는 이미 구현된 v3.1 기능을 단순히 다시 추가하는 버전이 아니다. 기존
`record`, `list`, `text`, `reducer`, `persist`를 verified core,
application runtime, effect boundary로 재분류하고 안정화한다.

## v5.0 — Application Boundary Baseline

| 항목 | 내용 |
| --- | --- |
| 목표·필요성 | 기존 기능이 무엇을 검증하고 무엇을 검증하지 않는지 고정한다. |
| 주요 작업·완료 | scalar contract, UI data, browser effect를 문서·타입·runtime에서 같은 경계로 분리한다. |
| 의존성 | v4.9의 의미론과 `contract/fn/extern` 정의 이후 진행한다. |
| 범위 밖 | 새 collection 알고리즘, 서버 구현, DB·인증 검증. |

### v5.0 구현 기록

상태: **완료**

- `axiom-application`에서 `ApplicationValue`, `VerifiedComputation`,
  `ApplicationCall`, `EffectRequest`를 분리해 pure computation, application
  call, 외부 effect 경계를 타입으로 표현한다.
- `app-run`이 application 입력을 검증·lowering하고 verified WASM을 실행한
  뒤 결과를 application value로 lifting한다. `mul8(5)`와 signed `-1` 실행을
  실제 release CLI에서 확인했다.
- 정수/boolean 간 암묵 변환과 범위 밖 입력을 거부하고, `extern` 실행,
  precondition, WASM runtime, effect, 결과 lifting 실패를 proof result와
  분리한다.
- [Application Boundary 문서](application-boundary.md)와 architecture에
  trust/dependency/value/failure 경계를 기록했다. v4 core/semantics 파일은
  수정하지 않았다.

검증:

- `cargo test --workspace`: 105개 통과
- `cargo clippy --workspace --all-targets -- -D warnings` 통과
- `cargo fmt --all -- --check`, `git diff --check` 통과
- `cargo build --release -p axiom-cli` 통과

추가로 Cargo metadata에서 app → core 방향과 core의 application/browser/
HTTP/storage dependency 부재를 확인했다.

경계 테스트: 잘못된 scalar 범위, text/list/record, integer-to-bool coercion,
잘못된 boolean output, extern declaration, precondition, WASM runtime, effect
실패를 확인하고 CLI integration test로 positive/negative 흐름을 실행했다.

## v5.1 — Thin Web Runtime

| 항목 | 내용 |
| --- | --- |
| 목표·필요성 | verified core와 브라우저를 연결하되 범용 웹 프레임워크가 되지 않는다. |
| 주요 작업·완료 | WASM 호출, 상태 연결, 화면 갱신, effect 실행, source 오류 전달을 얇은 runtime으로 고정한다. |
| 의존성 | v5.0 경계 분류 이후 진행한다. |
| 범위 밖 | React 대체, 자체 component ecosystem, DOM 전체 증명. |

## v5.2 — State Transition

| 항목 | 내용 |
| --- | --- |
| 목표·필요성 | 상태 변경이 어떤 순서와 결과를 갖는지 예측 가능하게 만든다. |
| 주요 작업·완료 | event, reducer, 초기 상태, 실패한 전이, 검증된 scalar contract 호출을 테스트한다. |
| 의존성 | v5.1 runtime bridge가 필요하다. |
| 범위 밖 | 분산 상태, 서버 동시성, 무제한 actor model. |

## v5.3 — Data Boundary

| 항목 | 내용 |
| --- | --- |
| 목표·필요성 | `record`, `list`, `text`가 화면 데이터인지 proof 데이터인지 혼동되지 않게 한다. |
| 주요 작업·완료 | serialization, record/list 상태, scalar contract로 넘길 수 있는 값의 경계를 문서화한다. |
| 의존성 | v5.0의 boundary 분류와 v5.2 상태 모델에 의존한다. |
| 범위 밖 | 임의의 list/map 알고리즘에 대한 완전한 Z3 검증. |

## v5.4 — Browser Effect와 Persistence

| 항목 | 내용 |
| --- | --- |
| 목표·필요성 | 브라우저 외부 상태가 숨겨진 부작용으로 들어오지 않게 한다. |
| 주요 작업·완료 | `persist`, localStorage 오류, 복원 실패, schema migration, 비 persist 상태를 명시한다. |
| 의존성 | v5.1 runtime과 v5.3 data boundary가 필요하다. |
| 범위 밖 | DB transaction과 서버 storage의 증명. |

## v5.5 — External Adapter

| 항목 | 내용 |
| --- | --- |
| 목표·필요성 | HTTP와 browser API를 verified core와 안전하게 연결한다. |
| 주요 작업·완료 | `extern`, request/response 타입, loading/success/error, API client adapter를 정의한다. |
| 의존성 | v4.5의 `extern` 의미와 v5.4 effect 모델에 의존한다. |
| 범위 밖 | 외부 서버 내부 구현의 correctness 증명. |

## v5.6 — Async와 실패 흐름

| 항목 | 내용 |
| --- | --- |
| 목표·필요성 | 실제 API 지연·재시도·취소·부분 실패를 명확히 처리한다. |
| 주요 작업·완료 | stale response, retry, cancel, effect 결과의 상태 반영을 테스트한다. |
| 의존성 | v5.2 state transition과 v5.5 adapter가 필요하다. |
| 범위 밖 | 분산 시스템 전체의 formal verification. |

## v5.7 — Web Integration Test

| 항목 | 내용 |
| --- | --- |
| 목표·필요성 | generated bundle이 proof 결과와 화면 동작을 함께 보존하는지 확인한다. |
| 주요 작업·완료 | WASM/UI 비교, 입력 오류, API/storage 실패, Live Reload, source mapping을 테스트한다. |
| 의존성 | v5.1~v5.6 runtime과 effect 흐름 이후 진행한다. |
| 범위 밖 | DOM/CSS 시각 결과의 수학적 증명. |

## v5.8 — Packaging과 Deployment

| 항목 | 내용 |
| --- | --- |
| 목표·필요성 | 검증된 웹앱을 재현 가능하게 빌드하고 배포한다. |
| 주요 작업·완료 | 정적 bundle, 환경 설정, template, CLI/VSCode package, reproducible build를 제공한다. |
| 의존성 | v5.7의 runtime test 통과가 선행 조건이다. |
| 범위 밖 | Synthetix 자체 cloud, managed DB, 인증 서비스. |

## v5.9 — Freeze

| 항목 | 내용 |
| --- | --- |
| 목표·필요성 | 검증된 scalar core를 실제 웹앱 제작 흐름으로 안정화한다. |
| 주요 작업·완료 | contract → proof → WASM → state/effect/API → static deployment 전체 흐름을 release test로 고정한다. |
| 의존성 | v5.0~v5.8 전체 완료 이후 진행한다. |
| 범위 밖 | Python급 범용 언어, 자체 backend framework. |

---

# v6 — 증명 가능한 프로그램 규모 확장

v6의 loop, recursion, array, map, set은 단순한 문법 추가가 아니다. 각각
검색 공간, 종료 증명, 메모리 모델, solver 비용을 크게 바꾸는 연구 과제다.
따라서 v6.0에서 benchmark로 필요성을 확인하고, 한 번에 하나의 확장만
진행한다.

## v6.0 — 규모 확장 Benchmark

| 항목 | 내용 |
| --- | --- |
| 목표·필요성 | 어떤 프로그램 규모에서 현재 모델이 실제로 막히는지 측정한다. |
| 주요 작업·완료 | 후보 수, Z3 시간, 호출 깊이, 데이터 크기, loop/collection 필요 사례를 benchmark로 고정한다. |
| 의존성 | v5.9의 안정된 runtime과 v4.9의 proof 기준이 필요하다. |
| 범위 밖 | benchmark 없이 e-graph, ML, loop를 바로 도입하는 것. |

## v6.1 — 제한된 배열

| 항목 | 내용 |
| --- | --- |
| 목표·필요성 | 메모리와 collection 증명의 가장 작은 형태를 검증한다. |
| 주요 작업·완료 | 고정 크기 배열, index 범위, length, 읽기·쓰기, out-of-bounds를 지원한다. |
| 의존성 | v6.0에서 배열이 실제 병목으로 확인되어야 한다. |
| 범위 밖 | 임의 크기 동적 메모리와 포인터 산술. |

## v6.2 — 제한된 반복문

| 항목 | 내용 |
| --- | --- |
| 목표·필요성 | 반복 계산을 증명 가능한 형태로 확장한다. |
| 주요 작업·완료 | bounded loop, 종료 조건, loop invariant, 반복 횟수 제한을 지원한다. |
| 의존성 | v6.0 benchmark와 v6.1의 제한된 memory 모델이 필요하다. |
| 범위 밖 | 종료가 보장되지 않는 일반 loop. |

## v6.3 — 구조적 재귀

| 항목 | 내용 |
| --- | --- |
| 목표·필요성 | recursion을 허용하되 종료와 자원 사용을 증명한다. |
| 주요 작업·완료 | 입력 크기가 줄어드는 재귀, termination proof, stack/resource bound를 지원한다. |
| 의존성 | v6.2의 invariant·resource 모델 이후 진행한다. |
| 범위 밖 | 일반 재귀와 임의의 self-modifying control flow. |

## v6.4 — 검증된 Collection

| 항목 | 내용 |
| --- | --- |
| 목표·필요성 | 화면용 collection을 넘어 순수 collection 계산을 증명한다. |
| 주요 작업·완료 | list 계산, search, fold, filter를 먼저 검토하고 map/set은 별도 benchmark 후 결정한다. |
| 의존성 | v6.1 배열과 v6.2 loop 또는 v6.3 recursion 중 필요한 모델이 선행되어야 한다. |
| 범위 밖 | 모든 자료구조를 한 번에 지원하는 범용 collection library. |

## v6.5 — 대규모 Modular Proof

| 항목 | 내용 |
| --- | --- |
| 목표·필요성 | 큰 import graph를 매번 처음부터 증명하지 않는다. |
| 주요 작업·완료 | contract summary, proof cache, dependency invalidation, 변경 모듈만 재검증한다. |
| 의존성 | v4.6 compositional verification과 v4.7 artifact가 필요하다. |
| 범위 밖 | 검증되지 않은 module을 verified graph에 자동 포함하는 것. |

## v6.6 — Synthesis Scaling

| 항목 | 내용 |
| --- | --- |
| 목표·필요성 | benchmark에서 synthesis가 병목일 때 검색 규모를 확장한다. |
| 주요 작업·완료 | 병렬 CEGIS, solver-guided search, 더 큰 typed grammar, 필요 시 e-graph를 도입한다. |
| 의존성 | v4.3 kernel과 v6.0 benchmark가 필수다. |
| 범위 밖 | benchmark 없는 ML-guided synthesis. |

## v6.7 — 추가 Runtime Target

| 항목 | 내용 |
| --- | --- |
| 목표·필요성 | WASM 외 실행 환경이 실제 제품 요구가 될 때 연결한다. |
| 주요 작업·완료 | WASI, Node, Edge adapter 중 필요한 target만 추가하고 의미 보존을 검증한다. |
| 의존성 | v4.8 backend conformance와 실제 배포 수요가 필요하다. |
| 범위 밖 | WASM을 버리고 JS/TypeScript 생성으로 방향을 바꾸는 것. |

## v6.8 — FFI와 Unsafe Boundary

| 항목 | 내용 |
| --- | --- |
| 목표·필요성 | 외부 함수와 검증되지 않은 영역을 명시적으로 격리한다. |
| 주요 작업·완료 | FFI 선언, effect 표시, 입력·출력 타입, 가정과 audit 경계를 제공한다. |
| 의존성 | v4.5 `extern`과 v5.5 adapter 모델이 필요하다. |
| 범위 밖 | 외부 함수 내부까지 자동으로 verified라고 표시하는 것. |

## v6.9 — Freeze

| 항목 | 내용 |
| --- | --- |
| 목표·필요성 | 더 큰 프로그램에서도 증명 가능성과 경계 명확성을 유지한다. |
| 주요 작업·완료 | 배열·제한된 loop·필요한 collection·incremental proof·외부 경계를 안정화한다. |
| 의존성 | 선택된 v6 연구 트랙의 실제 완료 결과에 의존한다. |
| 범위 밖 | Python 수준의 모든 기능, 무제한 recursion, 자동 ML 최적화. |

---

# 기존 문서와의 정리

기존 `docs/roadmap.md`는 v0.1부터 v4 1차 기능까지를 phase와 체크박스로
정리한 문서였다. 이 문서가 다음 부분에서 새 로드맵과 충돌한다.

1. 기존 Phase 9는 v4를 scalar 연산과 property synthesis 확장으로 설명했다.
   새 로드맵에서는 그 내용을 **v4.0 실험적 slice**로 낮추고, v4.1에서
   의미론과 backend 일치성을 먼저 검증한다.
2. 기존 문서는 division, e-graph, array, loop, Web Studio, ML을 한 묶음의
   이후 작업으로 나열했다. 새 로드맵에서는 division은 v4 의미론·property
   범위에서 먼저 다루고, array·loop·recursion·collection은 v6 연구 트랙으로
   분리한다.
3. 기존 문서는 `contract`, 일반 구현 함수, 외부 effect의 의미를 별도
   단계로 나누지 않았다. 새 로드맵은 v4.5에서 이 경계를 먼저 고정한 뒤
   v4.6에서 compositional verification을 진행한다.
4. 기존 synthesis 실패 메시지는 모순, under-constrained, 검색 경계 밖을
   하나의 실패로 설명했다. 새 로드맵은 v4.2에서 ContractCheck,
   SynthesisResult, ProofResult를 분리한다.
5. `docs/v2.md`와 `docs/v3.md`는 웹과 상태 기능의 현재 경계를 설명하는
   사양 문서로 유지한다. 다만 v3의 `record`, `list`, `text`는 문서에 적힌
   것처럼 Z3/WASM proof type이 아니라 application runtime data이며, 새
   v5.0에서 이 경계를 공식적으로 재확인한다.

따라서 이 문서는 기존 phase 체크리스트를 대체하는 **공식 v4~v6 방향 문서**다.
`docs/v2.md`, `docs/v3.md`, `docs/semantics.md`, `docs/architecture.md`는
각 버전의 상세 사양과 현재 구현 경계를 설명하는 하위 문서로 계속 사용한다.

## 현재 다음 작업

v4.1 Semantic Baseline부터 v4.9 Freeze와 v5.0 Application Boundary
Baseline까지 완료되었다. 다음 milestone은 v5.1 Thin Web Runtime이다.

```text
v4.1 완료
  → v4.2 완료
  → v4.3 완료
  → v4.4 완료
  → v4.5 완료
  → v4.6 완료
  → v4.7 완료
  → v4.8 완료
  → v4.9 완료
  → v5.0 완료
  → v5.1 Thin Web Runtime
```

v4.9에서 의미론, 결과 상태, supported grammar, negative fixture, release
검증 기준을 동결했고 v5.0에서 core와 application/effect 책임을 분리했다.
v5.1 runtime 구현은 이 milestone 작업 범위에 포함되지 않는다. 일반 재귀,
loop invariant, 무제한 호출도 시작하지 않는다.
