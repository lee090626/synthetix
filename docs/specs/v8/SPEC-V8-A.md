# SPEC-V8-A — Project Graph and Incremental Engine

**범위:** v8.0~v8.3 · **상태:** APPROVED · **SPEC 버전:** 0.2. Main은 v7.9 source와 현재 project/cache/dev 경로를 대조해 구현 전 계약으로 승인했다. 각 Phase의 PASS는 해당 gate만 닫으며 v8 전체 PASS를 뜻하지 않는다.

## Goal

큰 프로젝트의 graph identity와 DAG proof를 보존하며 변경된 부분만 다시 계산·검증하고, clean full build와 incremental build가 같은 결과를 내는 engine을 만든다.

## Scope / Out of Scope

- 범위: project load/link/proof/build 및 `axiom dev` edit loop의 graph·change baseline, module/dependency identity, shared-callee DAG call-site proof, change-local parse/link/proof, cache invalidation·recovery, project diagnostics와 last-good output.
- 범위 밖: 공개 import syntax 변경, module recursion, proof rule 확대, remote/disk cache, multi-user daemon, parallel scheduling, 무제한 graph 보장. R8-A/B/C는 별도 research gate이며 이 SPEC의 암묵적 구현 범위가 아니다.
- 범위는 이 SPEC을 재현하는 데 필요한 의존 변경 전체로 판단한다. 사용자가 더 좁게 지정한 파일·행동 제한이 우선한다.
- Phase 0은 관찰·fixture·benchmark baseline 전용이며 production engine 동작을 변경하지 않는다. 이후 Phase에서만 identity, DAG proof, incremental engine의 필요한 동작을 구현한다. Phase 0의 exact-set 계측이 꼭 필요하면 test/benchmark 전용으로 제한한다.

## Baseline

- v7.9 Freeze의 [결과](../../history.md#history-v7-9-benchmark)와 [raw evidence](../../v7.9-benchmark.json)를 출발점으로 삼는다. 실제 실행 시작 시 clean v7.9 source commit/HEAD, branch, dirty 범위, toolchain, host, profile과 fixture SHA를 다시 기록한다. 현재 문서 편집 작업본을 clean baseline으로 가정하지 않는다.
- 승인 시점 `release/v7.9.0` HEAD는 `16eedfa0a7206bc3651f0cf308584f3ee2d8bf14`이며 v7.9 검증 source `ca47543e1407a94df89744b014d54ece761ea09b`와 source 차이가 없다. 수정 전 `cargo test --workspace --locked`는 155개 PASS다. 문서 변경과 기존 `scripts/__pycache__/`는 clean benchmark snapshot에서 제외한다.
- v7.3 cache는 process-local이며 verified-only insertion과 clean-cache equality를 [기록](../../history.md#history-v7-3-benchmark)했다. 기존 v6 depth-8 call-chain 및 32-module caller-chain 수치는 DAG scale이나 parallel proof의 근거가 아니다.
- 현재 DAG shape별 parse/link 비용, invalidation closure, scale 및 parallel-verification benchmark는 없다. 이는 측정 공백이다.
- 현재 `axiom_project::load`와 `axiom dev`는 변경 때마다 전체 parse/link를 다시 수행하고 proof candidate만 process-local cache에서 재사용한다. Cache counters는 합계이며 affected `ContractId` 집합을 직접 제공하지 않는다. 현재 `ContractId`는 canonical source path와 contract name의 쌍이다.

## Invariants

- 같은 source·semantics·bounds는 같은 proof classification과 observable result를 낸다. `FOUND`, `NO_CANDIDATE`, `TIMEOUT`, `RESOURCE_LIMIT`, `VERIFIED`, `COUNTEREXAMPLE`, `SOLVER_UNKNOWN` 등의 분류를 성능을 위해 바꾸지 않는다.
- 각 call-site의 `requires` obligation이 증명된 경우에만 callee `ensures`를 caller proof에 사용한다. Cache는 unverified/stale result를 `VERIFIED`로 승격하지 않는다.
- Graph identity, dependency order, source mapping, diagnostics는 결정적이다. 잘못되거나 모호한 edge는 fail-closed한다. Invalid edit는 last-good output을 덮지 않는다.
- Search bounds와 proof obligation, v4~v7의 지원 언어 의미 및 application/extern trust boundary를 유지한다.
- Rename은 이전 path identity를 제거하고 새 path identity를 만든다. 서로 다른 module의 같은 이름은 project identity에서 구분하되, 현재 web export의 전역 중복 이름 오류는 유지한다.

## Required Behavior

1. Fan-in/fan-out, shared callee, multiple roots와 edit class별 affected closure를 재현한다.
2. Rename, same-name module, duplicate/cyclic edge와 source/dependency identity를 결정적으로 처리한다.
3. Shared-callee DAG의 모든 call site와 nested `requires`를 증명하고 source-aware diagnostic을 낸다.
4. No-op, leaf/root/import/manifest edit, delete/rename, valid→invalid→recovered cycle에서 change-local work와 cache hit/miss/invalidation을 정확히 추적한다.
5. Clean full build와 incremental build의 linked graph, proof classification, diagnostics, static outputs가 일치하고 long-running process의 stale state가 회수된다.

## Acceptance Criteria

- v8.0: cold/warm 및 edit class별 timing, graph shape, result class, 기대 affected closure와 관찰된 재검증 집합이 반복 가능하고 기존 chain evidence와 새 DAG coverage가 구분된다. 실패한 load/build에서 실제 재검증 집합을 관찰할 수 없으면 미관찰로 명시한다.
- v8.1: 동일 graph의 order/source mapping/diagnostics가 같으며 invalid·ambiguous edge가 fail-closed하고 graph mutation이 identity 충돌이나 valid edge 손실을 만들지 않는다.
- v8.2: call-site obligation을 증명한 경우에만 callee postcondition이 사용된다. Reference/Z3 결과와 source-aware diagnostics가 일치하고 graph shape별 solver invocation이 기록된다.
- v8.3: clean/incremental equality, 정확한 affected closure·cache counters·error location, bounded process state와 last-good 보존이 각 edit cycle에서 확인된다.
- 각 필수 gate는 evidence가 완결될 때만 PASS다. 범위 내 문서·metadata 정정은 CONDITIONAL, 검증 실패 후 제한된 수리는 FAIL, hosted 검증 진행 중은 PENDING이다. BLOCKED 조건은 아래에 한정한다.

## Benchmark / Provenance

- Clean source commit/dirty 상태, benchmark harness·fixture SHA-256, graph shape/size, edit class, toolchain·host·profile, search bounds, repeats와 raw 결과를 함께 남긴다. Cold/warm, project parse/link/proof/build, edit-loop timing, solver invocation, invalidation set 및 cache hit/miss를 분리한다.
- 동일 환경·동일 workload의 before/after 반복값과 median·range를 제시한 경우에만 속도 개선을 주장한다. Chain 결과를 DAG 성능 수치로 재사용하지 않는다. 결과 분류와 static artifact hash를 성능 수치와 별도로 대조한다.
- Benchmark JSON, Markdown 설명, source/fixture SHA, status와 링크를 Main이 일괄 대조한 뒤 Reviewer가 완성된 diff와 모든 주장을 함께 검토한다.
- Phase 0은 clean v7.9 source snapshot과 별도 문서 작업본을 구분하고, chain 대조군과 fan-in/fan-out·shared-callee·multiple-root DAG를 고정한다. No-op, unrelated, leaf, root, import, manifest edit를 각 5회 연속 반복한다. Raw run별 graph/node/edge, output class, diagnostics, cache hit/miss/invalidation, proof 재검증 `ContractId`(관찰 가능한 성공 case), static artifact SHA와 load/link/proof/build 및 edit-loop timing을 기록하고 median·range를 요약한다. 현재 출력으로 분리되지 않는 비용은 합쳐진 구간임을 표시한다.

## Allowed Repair

- 승인된 SPEC 범위 안에서 같은 root cause의 코드·동작 repair는 최대 2회다. 매번 관련 benchmark·regression과 review를 다시 수행한다.
- Source로 확인되는 문구·링크·SHA·JSON↔Markdown metadata 불일치는 한 번에 audit/정정하며 위 횟수에 넣지 않는다. 측정값·fixture·expected result를 임의로 고치지 않는다.
- SPEC 의미, acceptance, bounds 또는 proof rule을 바꿔야 한다면 repair로 처리하지 않는다.

## BLOCKED Conditions

- Semantics, proof classification, search bounds, proof obligation 또는 acceptance criterion 변경이 필요하다.
- 범위 밖 기능, 사용자 trade-off 결정, 외부 권한·환경 변경이 필요하여 의미 있는 진행이 불가능하다.
- 동일 root cause의 코드·동작 repair 2회가 실패했다.
- Under-invalidation, stale `VERIFIED`, 잘못된 precondition의 `VERIFIED`, clean/incremental mismatch 또는 회복 실패는 즉시 FAIL gate로 멈추고 원인을 수리한다. 위 BLOCKED 조건이 충족될 때만 BLOCKED로 올린다.

## Verification Plan

1. v7.9 baseline과 fixture identities를 고정한 뒤, graph/edit corpus와 기대 affected closure·classification을 먼저 기록한다. Phase 0에서는 production behavior 변경을 PASS 증거로 사용하지 않는다.
2. 각 Phase에서 relevant unit/integration, reference/Z3/WASM conformance, clean-vs-incremental differential 및 `axiom dev` recovery를 검증한다. Main은 workspace regression과 문서/provenance preflight를 수행한다.
3. Reviewer가 완성 diff, benchmark, docs, provenance를 한 번에 검토한다. v8.2처럼 proof-critical gate에서만 독립 Verifier를 추가할 수 있다. Static inspection을 runtime 검증이라 부르지 않는다.
4. Phase의 필수 acceptance가 PASS일 때만 다음 Phase로 간다. v8.0~v8.3 결과는 [SPEC-V8-B](SPEC-V8-B.md)의 선행 증거다.

## Phases

| Phase | 작업 단위 | 추적 milestone | Gate |
| --- | --- | --- | --- |
| Phase 0 — Graph baseline | Chain 대조군과 DAG/edit corpus, raw benchmark와 provenance 고정 | v8.0 | 측정·분류·affected-set 증거 PASS |
| Phase 1 — Stable identity | Module/dependency identity, rename·same-name·edge 진단과 결정적 order | v8.1 | Identity·source mapping·진단 PASS |
| Phase 2 — DAG proof | Shared-callee, multiple call sites, nested requires의 compositional proof | v8.2 | Call-site obligation·proof classification PASS |
| Phase 3 — Incremental engine | Change-local parse/link/proof, cache lifecycle, recovery와 differential | v8.3 | Clean/incremental equality·last-good·bounded state PASS |

아래 원본 milestone 문구는 기존 상세 계획의 목표·PASS·의존·제외·중단·규모를 의미 손실 없이 옮긴 추적 근거다. Milestone마다 별도 SPEC을 만들지 않는다. 원문의 “중단”은 다음 gate로 진행하지 않는다는 뜻이며, 상태는 이 SPEC의 FAIL/PENDING/BLOCKED 기준으로 판정한다.

## 관련 Research Gate

Research gate는 numbered milestone이 아니다. Named consumer·reproducible benchmark 또는 concrete blocker와 isolated experiment가 있어야 별도 revision에서 채택을 결정한다. 현재 SPEC의 scope와 acceptance를 중간에 재정의하지 않는다.

| Gate | 질문과 채택 기준 | 채택하지 않을 조건 |
| --- | --- | --- |
| R8-A Cross-process proof cache | Restart 간 candidate 재사용이 실제 consumer 시간을 줄이는가? Cache read가 source/semantics/dependency identity를 검증하고, proof result를 재검증 없이 신뢰하지 않는가? | Warm process cache로 충분하거나 disk cache가 correctness/maintenance cost를 넘으면 `NOT NEEDED`. |
| R8-B Full LSP/project semantic completion | 사용자 요청이나 repeated workflow evidence가 hover/completion/signature help의 현재 정적 구현 한계를 보여주는가? | 단지 일반적인 IDE parity만으로는 도입하지 않는다. |
| R8-C Dynamic verified collection | Real contract fixture가 fixed arrays/loops로 표현되지 않는 variable-length verified behavior를 요구하는가? Empty/out-of-bounds/termination/resource semantics와 WASM ABI를 먼저 정의해야 한다. | Application `List` 사용만으로는 proof collection 수요가 아니다. |

## 원본 milestone 기준

### v8.0 — Project Graph and Change Baseline

- **목표·근거·작업:** clean v7.9 checkout에서 project load/link/proof/build와 `axiom dev` edit loop를 baseline화한다. v6 chain evidence를 재현하고 fan-out/fan-in, shared callee, multiple roots, unrelated/leaf/root/import/manifest edits를 대표 graph fixtures로 추가한다.
- **PASS / 검증·측정:** cold/warm 및 edit class별 timing, graph shape, result class, invalidation set을 반복 생성하고 clean 상태와 함께 기록한다. 현재 call-chain 결과와 새 DAG coverage를 구분한다.
- **의존·범위 밖·중단:** v7.9 의존. 성능 목표치 약속, parallel scheduling, persistent cache는 범위 밖이다. 어떤 edit가 영향을 주는지 재현할 수 없으면 v8.1을 멈춘다.
- **계획:** 규모 M · 위험 낮음 · 구조 영향 낮음 · 회귀 위험 낮음 · benchmark 필수.

### v8.1 — Stable Module and Dependency Identity

- **목표·근거·작업:** project path/contract identity와 dependency order를 rename, same-name modules, shared dependencies, duplicate/cyclic edges에서 검증하고 deterministic identity/invalidation inputs를 고정한다.
- **PASS / 검증·측정:** 반복된 동일 graph가 같은 order, source mapping, diagnostics를 내고 잘못된/모호한 edge는 fail-closed한다. Graph mutations가 identity를 우연히 충돌시키지 않는다.
- **의존·범위 밖·중단:** v8.0 의존. 공개 import syntax 변경은 범위 밖이다. 다른 contract가 같은 identity가 되거나 valid edge가 사라지면 중단한다.
- **계획:** 규모 M · 위험 높음 · 구조 영향 중간 · 회귀 위험 높음 · benchmark 필요.

### v8.2 — DAG Compositional Proof

- **목표·근거·작업:** 현재 callee `requires`/`ensures` compositional proof를 chain에서 shared-callee DAG, multiple call sites, nested requires와 sibling modules로 확장 검증한다.
- **PASS / 검증·측정:** 각 call-site obligation을 증명한 경우에만 callee postcondition이 caller proof에 들어가며 reference/Z3 결과와 source-aware diagnostics가 일치한다. Graph shape별 solver invocation을 기록한다.
- **의존·범위 밖·중단:** v8.1 의존. Module recursion이나 proof rule widening은 범위 밖이다. 잘못된 precondition으로 VERIFIED가 나오면 v8을 중단한다.
- **계획:** 규모 L · 위험 높음 · 구조 영향 높음 · 회귀 위험 매우 높음 · benchmark 필수.

### v8.3 — Integrated Incremental Project Engine

- **목표·근거·작업:** 기존에 따로 제안했던 change-local parse/link, proof invalidation/recovery, cache lifetime, dev-loop failure isolation 및 precise project diagnostics를 한 end-to-end engine milestone으로 통합한다. 현재 watcher/load 경로와 process-local proof cache를 기반으로 변경 node와 영향 closure만 재계산하고 long-running process의 stale state를 회수한다.
- **PASS / 검증·측정:** clean full build와 incremental build가 linked graph, proof classification, diagnostics, static outputs에서 동일하다. no-op, leaf/root/import/manifest changes, delete/rename, valid→invalid→recovered edit cycle에서 정확한 affected closure, cache hit/miss, error location, bounded process state를 확인한다. Invalid input은 last good output을 덮지 않는다.
- **의존·범위 밖·중단:** v8.0–v8.2 및 v7.3 의존. Remote/disk cache, multi-user daemon, unbounded graph claims는 범위 밖이다. Under-invalidation, stale verified output, clean/incremental mismatch, unrecovered diagnostics가 있으면 engine qualification을 중단한다.
- **계획:** 규모 L · 위험 매우 높음 · 구조 영향 높음 · 회귀 위험 매우 높음 · benchmark 필수.
