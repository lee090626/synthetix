# SPEC-V10 — Stable Language & Ecosystem Contract

**범위:** v10.0~v10.9 · **상태:** DRAFT · **SPEC 버전:** 0.1. v9.9 Freeze 후 실제 consumer와 baseline을 확인해 Main이 구현 전 승인한다. 이 초안은 v10 구현이나 1.0 release 선언이 아니다.

## Goal

Source edition, semantics profile, proof artifact, CLI/VSCode, application/storage, WASM ABI와 release mapping의 supported surface, compatibility window, reader/migration, deprecation 및 breaking-change 경로를 확정한다.

## Scope / Out of Scope

- 범위: surface/consumer inventory, version dimensions, explicit edition, artifact reader/migration, CLI·VSCode contract, application/storage, WASM ABI, compatibility corpus, release qualification, 최종 stability Freeze.
- 범위 밖: 현재 의미를 바꾸는 새 operator/collection/무제한 recursion, database/auth persistence, extern implementation proof, WASI/Node/Edge deployment, 승인되지 않은 publish/release. Rust public API 안정성은 R10-A research gate를 통과한 외부 consumer가 없으면 internal이다.
- SPEC 재현에 필요한 선행 변경은 포함하되 사용자의 명시적 좁은 제한이 우선한다.

## Baseline

- v9.9 PASS와 v7.9 [Freeze 근거](../../history.md#history-v7-9-benchmark)를 선행한다. 실제 시작 시 source commit/branch/dirty 상태, external consumer, `.ax`·manifest, semantics ids, proof/CLI schema, storage schema, CLI/VSIX/tag/Cargo version mapping을 다시 inventory한다.
- 기존 계획에는 `v4.1-scalar-baseline`, `v6.3-bounded-program-model`, report/manifest format `4`, 별도 storage schema, Cargo crate `0.1.0`, VSIX `0.14.1`, tag `v0.14.0`이 서로 다른 축으로 기록돼 있다. 현재 값이라고 가정하지 말고 실행 때 재확인한다.

## Invariants

- 기존 source는 explicit migration 없이 다른 meaning을 얻지 않는다. Proof artifact의 corrupt/unknown major/incomplete evidence는 fail-closed한다. Cache/profile 변경은 stale verified hit를 만들지 않는다.
- Application/storage·host/runtime failure는 ProofResult로 바뀌지 않는다. ABI mismatch는 명시적으로 reject 또는 migration한다.
- 실제 consumer 없는 Rust API나 새 runtime의 stability를 약속하지 않는다. v10.9 Freeze는 1.0 release나 publication을 뜻하지 않는다.

## Required Behavior

1. 각 surface를 stable/experimental/internal로 분류하고 consumer evidence와 support/deprecation policy를 연결한다.
2. Semantics/capability/schema/cache version axes, source edition, artifact reader/migration, CLI/VSCode/release mapping, storage·WASM ABI compatibility를 명시한다.
3. Supported/unsupported 조합의 fixture와 expected result를 compatibility corpus로 고정하고 clean candidate를 네 플랫폼에서 qualification한다.

## Acceptance Criteria

- v10.0~v10.6: 각 surface의 승인된 tier와 consumer evidence, 필요한 invalidation, legacy source/result 보존, fail-closed artifact migration, CLI/VSCode mapping, storage error separation, ABI conformance가 원본 milestone 기준대로 PASS다.
- v10.7: 모든 supported matrix entry가 재현 가능한 fixture로 통과하고 unsupported 조합은 명시적으로 거부된다. Silent migration/data loss, stale verified artifact와 ABI mismatch 통과는 허용하지 않는다.
- v10.8: clean candidate가 workspace regression, 네 플랫폼 CLI, VSIX package/activation, static browser load/call/persistence, artifact consistency, same-toolchain reproducibility를 통과한다.
- v10.9: required milestones가 모두 PASS이고 migration guide와 supported surface matrix가 최종 clean checkout에서 검증된다. `NOT NEEDED`는 사전 정의된 consumer/benchmark 근거가 있어야 한다.

## Benchmark / Provenance

- v10.1 version bump/invalidation, v10.6 ABI profile, v10.8 clean qualification과 v10.9 freeze 증거를 기록한다. Source commit/branch, fixture 및 artifact SHA-256, version matrix, toolchain/host/profile, benchmark input·bounds·raw repeats와 before/after를 연결한다.
- 성능 claim은 동일 환경·workload의 반복 측정과 median·range가 있을 때만 한다. Main이 SHA·수치·JSON↔Markdown·status/link를 preflight하고 Reviewer가 완성 diff와 migration/provenance claim을 검토한다.

## Allowed Repair

- 승인 범위에서 동일 root cause의 코드·동작 repair는 최대 2회이며 affected conformance, corpus, benchmark 및 review를 다시 수행한다.
- Source-supported 문서·metadata 불일치는 일괄 audit/정정하며 횟수에 포함하지 않는다. Raw result, supported matrix나 acceptance를 조용히 바꾸지 않는다.

## BLOCKED Conditions

- Semantics·proof classification·search bounds·acceptance 변경, scope 밖 기능, 사용자 trade-off, 외부 권한·환경 변경이 실제로 필요하거나 동일 root cause repair 2회가 실패했다.
- Breaking mismatch, silent migration/data loss, stale verified artifact, unsupported ABI 통과는 먼저 FAIL로 수리한다. 외부 hosted gate 진행 중은 PENDING이다.

## Verification Plan

1. v9.9 baseline과 실제 consumer/surface/version matrix를 승인하고 supported/unsupported fixture를 고정한다.
2. 각 surface의 parser/type/proof/ABI/application/CLI·VSCode reader와 migration regression을 실행하고 v10.7 corpus를 확정한다.
3. v10.8 clean candidate의 workspace, 네 플랫폼 CLI·VSIX, browser/static, artifact/reproducibility를 검증한다. Main preflight, Reviewer 전체 diff·claim review, Freeze에서 필요한 독립 Verifier 뒤 최종 작업본을 누적 판정한다.

## Phases

| Phase | 작업 단위 | 추적 milestone | Gate |
| --- | --- | --- | --- |
| C1 — Surface contract | Consumer, version dimensions, editions, artifact readers | v10.0~v10.3 | Surface·version·migration 계약 PASS |
| C2 — Runtime and tooling compatibility | CLI/VSCode, application/storage, WASM ABI와 matrix | v10.4~v10.7 | Supported/unsupported corpus PASS |
| C3 — Qualification and Freeze | Clean candidate와 누적 stability contract | v10.8~v10.9 | 모든 필수 evidence PASS |

R10-A Rust API 안정성과 R10-B 추가 runtime/backend는 실제 consumer와 measured blocker가 있을 때만 별도 research/promotion을 검토한다. 채택되지 않은 기능을 현 v10 numbered milestone에 끼워 넣지 않는다.

## 관련 Research Gate

Research gate는 numbered milestone이 아니다. Named consumer·reproducible benchmark 또는 concrete blocker와 isolated experiment가 있어야 별도 revision에서 채택을 결정한다. 현재 SPEC의 scope와 acceptance를 중간에 재정의하지 않는다.

| Gate | 질문과 채택 기준 | 채택하지 않을 조건 |
| --- | --- | --- |
| R10-A Stable Rust library API | Repository 외부 downstream crate가 특정 public Rust APIs에 의존하는 증거가 있는가? | 소비자가 없으면 API stability를 약속하지 않고 internal implementation surface로 둔다. |
| R10-B New runtime/backend | Named deployment consumer와 measured blocker가 있는가? | Node test host 사용만으로 Node deployment target을 추가하지 않는다. |

## Stability surfaces

| Stability surface | v10 계약 범위 |
| --- | --- |
| Source Language / Edition | Edition 선택과 기존 source 해석 유지 (v10.2) |
| Semantics Profile | Capability/version 조합과 cache invalidation (v10.1) |
| Proof Artifact | Schema reader, migration, fail-closed 규칙 (v10.3) |
| CLI / Release Mapping | 명령·출력 계약과 binary/tag/package 버전 관계 (v10.4) |
| VSCode / Tooling | Extension activation과 bundled CLI 지원 관계 (v10.4) |
| Application / Storage | Application 오류 경계와 storage migration 소유권 (v10.5) |
| WASM Runtime ABI | Export/import/calling convention 호환성 (v10.6) |
| Rust API | 실제 downstream consumer가 확인될 때만 R10-A Research Gate로 평가 |

원본의 “중단”이나 `PARTIAL`은 자동 `BLOCKED`가 아니다. 아래 문구는 목표·PASS·의존·제외·규모의 추적 근거이며, 현재 gate 상태는 이 SPEC의 FAIL/PENDING/BLOCKED 기준으로 판정한다.

## 원본 milestone 기준

### v10.0 — Stable Surface and Consumer Decision

- **목표·근거·작업:** `.ax` grammar, `synthetix.toml`, CLI commands/options/output,
  `window.synthetix` runtime ABI, `app.proof.json`, semantics profile, application storage,
  VSCode/tooling, release/version mapping, Rust crate exports를 surface별로 inventory하고,
  consumer evidence, stability tier, support/deprecation policy 초안을 정한다.
- **PASS / 검증·측정:** 각 surface가 stable, experimental, internal 중 하나로 승인되고
  consumer evidence가 기록된다. Cargo crate `0.1.0`을 외부 Rust API 안정성으로 오해하지
  않도록 policy를 둔다.
- **의존·범위 밖·중단:** v9.9 의존. Evidence 없는 crates.io/support promise는 범위 밖이다.
  실제 consumer가 누락된 public breaking change가 나오면 version plan을 멈춘다.
- **계획:** 규모 M · 위험 중간 · 구조 영향 중간 · 회귀 위험 중간 · benchmark 불필요.

### v10.1 — Semantics and Capability Version Matrix

- **목표·근거·작업:** scalar frozen model, bounded arrays/loops/recursion, verifier lowering,
  WASM ABI, report schema와 proof cache의 version identifiers가 서로 무엇을 무효화하는지
  선언한다. `v4.1-scalar-baseline`과 `v6.3-bounded-program-model`의 역할을 설명한다.
- **PASS / 검증·측정:** Cache keys, CLI report, web proof manifest, benchmark metadata가
  올바른 version dimension을 기록하고, version bump tests가 필요한 cache/schema invalidation만
  일으킨다.
- **의존·범위 밖·중단:** v9.1, v9.2 의존. 구현 semantics를 바꾸는 일은 범위 밖이다.
  같은 semantic profile의 stale cache가 hit하면 중단한다.
- **계획:** 규모 M · 위험 높음 · 구조 영향 중간 · 회귀 위험 높음 · benchmark 필요.

### v10.2 — Source Edition Selection

- **목표·근거·작업:** 미래에 source grammar/meaning이 바뀌는 경우 기존 files를 조용히
  재해석하지 않도록 project-level language edition 규칙을 정의한다. 필요하다면 기존 manifest
  default를 legacy edition으로 유지하고 unknown edition을 거부한다.
- **PASS / 검증·측정:** Legacy `.ax` files의 parse/type/proof result가 이전 edition과 같고,
  new syntax는 명시한 edition에서만 허용된다. Missing/unknown edition behavior가 fixtures로
  고정된다.
- **의존·범위 밖·중단:** v10.1 의존. New operators, collections, unrestricted recursion은
  범위 밖이다. Existing source가 edition migration 없이 다른 meaning을 얻으면 중단한다.
- **계획:** 규모 M · 위험 높음 · 구조 영향 중간 · 회귀 위험 높음 · benchmark 불필요.

### v10.3 — Proof Artifact Schema Migration Policy

- **목표·근거·작업:** v4 report/proof manifests와 v9 canonical evidence schema 사이의
  reader, migration, rejection rules를 고정한다. Application의 `storage_schema_version`은
  별도 surface로 두고 migration 책임은 v10.5에서 정한다.
- **PASS / 검증·측정:** Supported legacy schema는 deterministic migration/reading을 하고,
  unknown major/corrupt/incomplete fields는 fail-closed한다. Migration은 proof semantics를
  재해석하지 않는다.
- **의존·범위 밖·중단:** v9.9, v10.1 의존. Browser storage migration은 범위 밖이다.
  Corrupt input이 verified status로 올라가면 중단한다.
- **계획:** 규모 M · 위험 높음 · 구조 영향 중간 · 회귀 위험 높음 · benchmark 불필요.

### v10.4 — CLI, VSCode, and Tooling Contract

- **목표·근거·작업:** CLI binary version/report versions, Cargo crate versions, Git tags,
  VSIX version 및 bundled target CLIs의 mapping을 정의한다. `axiom --version`과 machine-readable
  report version이 없다면 approved command/field로 제공하고 error/exit behavior를 고정한다.
- **PASS / 검증·측정:** Release CI가 manifest의 approved mapping과 모든 bundled platform CLI를
  확인하고, VSIX activation smoke를 실행한다. Legacy `check/build/run/app-run/dev` 및 flags는
  별도 migration notice 없이 깨지지 않는다.
- **의존·범위 밖·중단:** v10.0–v10.3 의존. Release/publish는 이 계획 문서의 범위 밖이다.
  Cargo package, tag, VSIX가 의도치 않게 서로 다른 CLI를 지시하면 release gate를 중단한다.
- **계획:** 규모 M · 위험 높음 · 구조 영향 낮음 · 회귀 위험 높음 · benchmark 불필요.

### v10.5 — Application and Storage Compatibility

- **목표·근거·작업:** Host effects, application JSON values, persisted
  `storage_schema_version`의 support와 migration contract를 proof evidence/schema reader와
  분리한다. Verified core change가 browser storage나 external response를 자동으로 trust하지
  않게 한다.
- **PASS / 검증·측정:** v5.9 state/effect path와 current restore/migration fixtures가 유지되고,
  storage migration failure가 application error로 남으며 ProofResult로 변하지 않는다.
- **의존·범위 밖·중단:** v10.0–v10.1 의존. Database/auth persistence와 extern
  implementation verification은 범위 밖이다. Storage failure가 proof classification으로 바뀌면
  중단한다.
- **계획:** 규모 M · 위험 높음 · 구조 영향 중간 · 회귀 위험 높음 · benchmark 불필요.

### v10.6 — WASM Runtime ABI Compatibility

- **목표·근거·작업:** Verified scalar calling convention, WASM export names/signatures,
  host imports와 ABI version identity를 surface matrix에 연결한다. ABI change가 semantics profile,
  artifact identity, host adapter에 미치는 breaking boundary를 명시한다.
- **PASS / 검증·측정:** 각 supported ABI profile에 reference/AMI/WASM/runtime conformance fixture가
  있고 ABI mismatch가 명시적으로 reject되거나 migration된다. Host/runtime failure는 proof outcome으로
  바뀌지 않는다.
- **의존·범위 밖·중단:** v10.0–v10.2 의존. WASI/Node/Edge deployment 추가는 범위 밖이다. ABI
  mismatch가 verified candidate의 meaning을 바꾸면 중단한다.
- **계획:** 규모 M · 위험 높음 · 구조 영향 중간 · 회귀 위험 매우 높음 · benchmark 필요.

### v10.7 — Compatibility Regression Corpus

- **목표·근거·작업:** v10.1–v10.6에서 선언한 source edition, semantics profile, proof artifact
  schema/reader, CLI, VSCode, storage migration, WASM ABI의 전체 compatibility matrix를 회귀
  corpus로 고정한다. Rust API stability는 현 v10 numbered milestone과 이 matrix에서 제외하고
  R10-A Research Gate로만 유지한다. Gate가 채택 기준을 통과해도 promotion policy에 따라 다음
  roadmap revision 또는 major에서 정식 milestone로 제안하며, 현재 v10 번호를 재정의하지 않는다.
- **PASS / 검증·측정:** 선언한 모든 supported 조합에서 기존 source/result, artifact read/migration,
  CLI·VSCode 계약, storage migration, WASM ABI conformance가 유지된다. 모든 unsupported 조합은
  명시적으로 거부되며 각 matrix entry에 재현 가능한 fixture와 기대 결과가 있다.
- **의존·범위 밖·중단:** v10.0–v10.6 의존. Release candidate/platform qualification은 v10.8이다.
  누락된 supported entry, silent migration/data loss, stale verified artifact 또는 ABI mismatch가
  통과하면 중단한다.
- **계획:** 규모 L · 위험 높음 · 구조 영향 중간 · 회귀 위험 매우 높음 · benchmark 불필요.

### v10.8 — Release Qualification

- **목표·근거·작업:** v10.7 compatibility corpus가 통과한 clean release candidate를 전체
  supported platform/tooling matrix에서 qualification한다.
- **PASS / 검증·측정:** Clean checkout에서 workspace tests, Linux x64/Windows x64/macOS arm64/macOS x64
  CLI smoke, VSIX package/activation, static browser load/call/persistence, artifact consistency,
  same-toolchain reproducibility evidence가 모두 통과한다.
- **의존·범위 밖·중단:** v10.0–v10.7 의존. v10.9 판정 전 publish/release creation은 하지 않는다.
  Trust/compatibility mismatch가 있으면 candidate를 중단한다.
- **계획:** 규모 L · 위험 높음 · 구조 영향 중간 · 회귀 위험 매우 높음 · benchmark 필수.

### v10.9 — Stable Language & Ecosystem Contract Freeze

- **목표·근거·작업:** source edition, semantics profiles, artifact schema/readers, CLI,
  VSCode/tooling, application/storage, runtime ABI, release/version mapping과 Rust API의
  stable/experimental/internal 범위 및 breaking-change migration 절차를 freeze한다. Rust API는
  향후 roadmap revision에서 별도 채택되지 않으면 internal로 남긴다. 이 계약은 1.0 release
  선언이 아니다.
- **PASS / 검증·측정:** v10.0–v10.8 evidence가 닫히고 required milestones 모두 PASS이며,
  `NOT NEEDED`는 사용자/consumer/benchmark evidence를 포함한다. Migration guide와 supported
  surface matrix가 clean checkout에서 검증된다.
- **의존·범위 밖·중단:** 모든 v10 milestone 의존. Unreleased proof-certificate, dynamic
  collection, extra runtime target을 freeze claim에 넣지 않는다. 남은 `PARTIAL`/`FAIL`이
  있으면 major freeze를 선언하지 않는다.
- **계획:** 규모 M · 위험 매우 높음 · 구조 영향 높음 · 회귀 위험 매우 높음 · benchmark 필수.
