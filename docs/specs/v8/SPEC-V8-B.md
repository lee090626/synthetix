# SPEC-V8-B — DAG Scale Qualification and Freeze

**범위:** v8.8~v8.9 · **상태:** APPROVED · **SPEC 버전:** 0.5. v0.3의 graph label/no-op 경계 정정에 더해, v0.4 independent review가 general invalid-edit의 cold/retained diagnostic 비교를 명시하라고 요청했다. v0.5는 SPEC-A의 error-location·diagnostic 불변조건이 모든 invalid edit에 적용됨을 분명히 한다. 전체 Reviewer review PASS 후 Main이 v8.8 구현 계약으로 승인했다. Graph topology/counts, scale envelope, repeats, edge budget, 30초·256 MiB 한도, proof bounds, milestone 범위와 gate order는 그대로다. v8.4~v8.7은 의도적 공백이다.

Generated raw JSON captures are excluded from GitHub; checksums are listed in the [capture manifest](../../benchmark-captures.sha256).

## Goal

SPEC-V8-A의 engine을 측정된 DAG scale envelope에서 qualification하고, v8 전체 결과를 최종 작업본 기준으로 누적 검증해 Freeze한다.

## Scope / Out of Scope

- 범위: graph size·shape·edit class·pathological cases, deterministic resource bounds, clean/incremental differential, workspace regression, 네 플랫폼 CLI, VSIX와 browser/static qualification.
- 범위 밖: 무제한 graph/size 보장, persistent/remote cache, R8-D가 채택되지 않은 parallel proof. R8-A~D research gate는 자동 milestone이 아니다.
- 재현에 필요한 v7.9 및 SPEC-V8-A 의존 변경을 포함한다. 사용자의 명시적 제한이 더 좁으면 우선한다.

## Baseline

- [SPEC-V8-A](SPEC-V8-A.md)의 PASS 증거와 [v7.9 Freeze](../../history.md#history-v7-9-benchmark)를 선행 기준으로 삼는다. 실행 시 final source commit, branch/HEAD, dirty 범위, graph fixture SHA, benchmark harness, toolchain·host·profile과 플랫폼 artifact identities를 새로 기록한다.
- 기존 chain 결과는 DAG scale 성능 claim의 before 값이 아니다. Scale baseline은 같은 graph shape와 환경에서 측정한다.
- v8.3 기준 작업본은 branch `release/v7.9.0`, HEAD `16eedfa0a7206bc3651f0cf308584f3ee2d8bf14`다. 승인 시점 project-loader SHA-256은 `c7165057588d5b2928363f02f3a5967c092afa8fd41a584f8768d568e17f5c40`, dev-loop SHA-256은 `8d67f1254be0322951c07409be6e77888da2ec070af810629fdfdcfa24acd97c`다. v7.9 comparator는 clean commit `ca47543e1407a94df89744b014d54ece761ea09b`와 CLI SHA-256 `f3321f55bbabb541529e192a458b8221ff30e0c646d6cbcb4bb382fb7cb85012`다. `v8.3-benchmark.json` (raw capture omitted from GitHub)는 fixture·harness·toolchain·host와 current dirty range를 기록한다.
- 이전 planning note에는 8/32/128-node feasibility smoke의 0.774초·33,046,528-byte 최대값이 적혀 있었지만 raw capture, 정확한 fixture/harness/binary identities와 RSS 측정 절차가 보존되지 않았다. 따라서 이 값은 검증 불가로 취급하고 v8.8 baseline, resource 한도의 근거, acceptance/performance evidence에서 제외한다. v8.3 raw capture에는 scale RSS 자료가 없다. 재현 가능한 v8.8 harness가 아래 기준으로 baseline과 current를 새로 측정한다.

## Invariants

- SPEC-V8-A의 semantics, proof classification, call-site obligation, identity, last-good output, clean/incremental equality를 유지한다.
- Resource bound와 diagnostics는 pathological graph에서도 결정적이다. Cache나 scale 최적화가 stale verified result를 만들지 않는다.
- Proof search bounds는 v8.0 baseline의 `max_candidates=50,000`, `max_memory_bytes=67,108,864`를 유지한다. 두 값은 proof search 설정이며 process-wide RSS ceiling이 아니다.
- Freeze는 마지막 diff만이 아니라 v8.0~v8.3과 v8.8의 모든 필수 계약을 현재 전체 작업본에서 다시 확인한다.

## Required Behavior

1. DAG shape·size·edit class와 pathological fixture에서 clean/incremental outputs, graph order, resource behavior를 재현한다.
2. 측정된 envelope 밖의 크기나 parallelism 성능을 claim하지 않는다.
3. Full regression과 네 플랫폼 CLI 및 VSIX source/package hash·activation, browser/static smoke를 최종 source에 연결한다.

## Acceptance Criteria

- v8.8: 8, 32, 128 **graph nodes** (아래 fixture 계약에 따르며 synthetic `entry.ax` 제외) 각각에서 chain, wide fan-out, shared-leaf fan-in, layered shared-path DAG, independent roots의 다섯 shape를 qualification한다. Entry imports를 포함한 edge 수는 `4 × graph nodes` 이하로 한다. 각 shape/size의 cold build와 warm manifest rebuild는 5회 반복한다. 128-node layered shared-path DAG의 mutation matrix는 no-op, unrelated, leaf, root, import add, import remove, import reorder, module rename, module delete, manifest-only, valid→invalid→recovered이며 각 항목을 5회 반복한다. Depth-128 chain, 127-way fan-in/out, 정확히 `4 × graph nodes` edge인 layered DAG, 128-node import cycle과 duplicate-binding fixture도 포함한다.
- v8.8 PASS: valid graph와 성공 edit에서 clean/incremental graph identity/order, proof classification, diagnostics, static artifact hashes와 fixture로부터 독립 계산한 정확한 invalidation closure가 일치한다. Mutation matrix의 모든 invalid edit는 cold load/build와 retained edit가 같은 result class, 정규화된 diagnostic, source location을 내야 한다. Cycle·duplicate-binding fixture는 특히 양쪽에서 같은 load/link failure를 내고 proof classification을 만들지 않는다. 실패 edit는 last-good output/version을 보존하고 복구 결과가 cold build와 일치한다. No-op은 rebuild하지 않는다. 각 cold-build case는 30초 내 종료하고, 각 edit case는 30초 내 terminal build/error/recovery 결과를 낸다. 각 케이스의 Axiom CLI direct-process peak RSS는 macOS arm64 qualification host에서 256 MiB (268,435,456 bytes) 이하여야 한다. 반복 edit 후 proof cache에는 active `ContractId`만 남고, failed load의 watch path set은 이전 graph·latest attempt·config 경로의 고유 합집합을 넘지 않는다. 이 gate는 범위 내 correctness/resource qualification이며 속도 향상을 요구하거나 주장하지 않는다.
- v8.9: 최종 작업본의 의도된 누적 v8 변경 전체로 DAG mutation corpus, clean-vs-incremental differential, repeated scale benchmark, workspace regression, Linux x64·Windows x64·macOS arm64·macOS x64 CLI, VSIX source/package SHA-256·activation smoke, browser/static qualification이 모두 PASS다. 필수 항목의 CONDITIONAL·FAIL·PENDING은 Freeze 전에 닫는다.
- 각 결과에 source commit, fixture/harness hash, raw 측정과 artifact identity가 연결된다. Unsupported scale claim이나 stale result가 있으면 Freeze하지 않는다.

## Benchmark / Provenance

- 동일 clean source/config/toolchain/host/profile/search bounds의 DAG shape별 반복값, median·range, cold/warm, edit class, size, resource usage, clean/incremental output hash를 기록한다. Chain과 DAG를 분리한다. Before/after timing은 v7.9 clean comparator와 v8 current source를 같은 fixture와 환경에서 짝지었을 때만 성능 비교에 사용한다.
- Graph node는 생성된 비-entry `.ax` module 하나이며 각 정규 scale node는 고유 contract 하나를 export한다. Directed edge `A → B`는 A의 import가 B를 가리키는 module dependency다. `entry.ax`는 node 수에서 제외하고 entry의 import edge는 edge 수에 포함한다. Root는 entry가 직접 import하는 node다. Raw case는 sorted node/edge/root 목록과 node/module/edge/root/contract 수를 기록한다.
- 다섯 shape의 oracle은 importer→dependency 방향에서 다음과 같다: `chain`은 N개 node의 단일 path와 한 root(N−1 module edge); `wide fan-out`은 한 root가 N−1개 leaf를 import(N−1 module edge); `shared-leaf fan-in`은 N−1개 root가 동일 leaf를 import(N−1 module edge); `layered shared-path`는 두 path root, N−5개 branch, 한 join, 한 shared leaf, 별도 independent root로 구성하며 두 path root가 모든 branch를 import하고 각 branch는 join, join은 leaf를 import(세 entry roots, 총 3N−11 edge); `independent roots`는 module dependency edge가 없는 N개 root(Entry import N개)다. `edge-cap layered DAG`는 N개 node를 index 순서로 두고 entry가 전부 import하며, 가능한 `(source index, dependency index)` 쌍을 사전식으로 선택해 정확히 3N개의 module edge를 추가한다. 따라서 총 edge는 정확히 4N이고 방향은 항상 낮은 index에서 높은 index다. 모든 dependency import는 corresponding contract의 `requires`에서 호출해 proof dependency도 같은 topology를 따르게 한다.
- Pathology oracle: 128-node cycle은 entry가 `m000`을 import하고 `m000 → m001 → … → m127 → m000` import ring을 만든다. Duplicate-binding은 128개 node가 모두 entry에서 도달 가능해야 하며 `entry.ax`의 1·2행이 같은 imported local name `first`를 서로 다른 module path로 import하게 한다. 각각 clean load/build는 nonzero load/link failure와 `circular import:` 또는 `entry.ax:2: duplicate imported name: first` diagnostic을 내며 proof result를 생성하지 않는다. 비교용 diagnostic은 canonical project root를 `<PROJECT>`로 치환해 정규화한다. Retained edit에서도 동일 diagnostic을 내고 last-good output/version을 보존한 뒤 원래 import를 복원하면 cold build와 일치해야 한다.
- 각 raw case에 exit/result class, graph order and identity, diagnostic, proof class/affected `ContractId`s, parse/link/proof counters, artifact SHA-256, wall time, peak RSS를 기록한다. 동일 host·toolchain·profile·bounds에서 baseline/current를 짝지어 5회 실행하고 median/range를 요약한다. 기록되지 않은 사전 smoke 수치는 baseline이나 resource headroom 근거로 재사용하지 않는다. 승인된 30초·256 MiB 한도는 macOS arm64 qualification host의 per-case target이며 해당 envelope 밖의 보장을 뜻하지 않는다. 다른 host 결과를 동등하다고 추론하지 않고 별도 기록한다.
- Resource measurement boundary: cold build wall time은 Axiom CLI process launch부터 exit까지의 monotonic elapsed time이다. Edit wall time은 파일 mutation 직전부터 matching build/error/recovery result marker까지이며 initial dev startup은 별도로 기록한다. 각 edit case는 fresh `axiom dev --no-open` process에서 시작한다. `/usr/bin/time -l`의 `maximum resident set size`를 해당 direct Axiom CLI process의 peak RSS로 기록하며 Python harness와 무관한 process는 포함하지 않는다. `dev` case는 terminal marker 관측 직후 종료해 RSS를 수집한다. Deadline 초과는 별도 `CASE_DEADLINE` harness outcome으로 기록하고 proof `TIMEOUT` 분류와 혼합하지 않는다.
- No-op boundary: fresh `axiom dev --no-open` process가 initial `FOUND/VERIFIED` build와 `watching:` marker에 도달한 뒤 version을 snapshot하고 관찰을 시작한다. 기존 v8.3 baseline protocol과 동일하게 400 ms 관찰하며, `NO_REBUILD_EXPECTED`, version 불변, `startup_ns`와 `observation_window_ns` 기록, `edit_loop_ns = null`을 성공 marker로 사용한다. No-op case는 process launch부터 성공 marker까지 30초 이내여야 한다. 성공 marker 직후 process를 종료해 `/usr/bin/time -l`로 direct Axiom CLI process의 전체 lifetime peak RSS를 수집한다. 이 관찰 시간은 edit-loop timing이나 성능 개선 주장에 포함하지 않는다.
- Hosted run commit과 네 플랫폼 native CLI 결과, VSIX source/package SHA-256 및 activation smoke의 원본 증거를 연결한다. Node DOM shim과 실제 browser smoke를 구분한다.
- Main이 SHA·수치·JSON↔Markdown·status·link를 묶어 대조하고 Reviewer가 완성 diff와 모든 근거를 검토한다. Freeze에서 독립 재현이 필요할 때만 Verifier를 사용한다.

## Allowed Repair

- 승인된 scope에서 동일 root cause의 코드·동작 repair 최대 2회; 수정 후 affected benchmark·regression·hosted gate를 다시 실행한다.
- 문서·metadata의 source-supported 불일치는 같은 범주를 일괄 audit/정정하며 repair 횟수에 넣지 않는다. Raw 측정이나 기대 결과를 근거 없이 바꾸지 않는다.

## BLOCKED Conditions

- Semantics·proof classification·search bounds·proof obligation 또는 acceptance 변경, scope 밖 기능, 사용자 trade-off 결정, 외부 권한·환경 변경이 실제로 필요하다.
- 동일 root cause repair 2회 실패. Pathological graph의 correctness/resource failure와 stale result는 먼저 FAIL로 수리하며, 위 조건이 성립할 때 BLOCKED다.

## Verification Plan

1. B1에서 fixture/harness/bounds를 고정하고 다섯 graph shape와 세 size, 128-node mutation/pathological corpus를 5회 반복한다. Main은 raw values와 resource ceilings, JSON↔Markdown provenance를 대조하고 Reviewer는 B1 diff와 claims를 검토한다.
2. B1 PASS 이후에만 B2를 시작한다. 최종 전체 작업본에서 workspace tests, reference/Z3/WASM 및 clean/incremental differential, CLI/VSIX/browser gates를 누적 검증한다.
3. B2에서 Main의 benchmark/provenance preflight → Reviewer 전체 diff·claim 검토 → 필요한 경우 독립 Verifier 재현 순서로 판단한다. 외부 검증 중에는 PENDING, 전부 통과하면 PASS다.

## Phase

| Phase | 작업 단위 | 추적 milestone | Gate |
| --- | --- | --- | --- |
| B1 — DAG scale qualification | v8.8 measured envelope와 deterministic resource gate | v8.8 | 전체 scale/edit corpus와 resource limits PASS |
| B2 — Cumulative Freeze | v8.0~v8.3 및 v8.8을 최종 작업본에서 재검증하고 hosted/platform gates 수행 | v8.9 | 모든 누적 acceptance PASS일 때만 Freeze |

아래 원본 milestone 문구는 기존 상세 계획의 목표·PASS·의존·제외·중단·규모를 의미 손실 없이 옮긴 추적 근거다. 원문의 “중단”이나 `PARTIAL`은 자동 `BLOCKED`가 아니며, 현재 gate는 이 SPEC의 FAIL/PENDING/BLOCKED 기준으로 판정한다.

## 관련 Research Gate

Research gate는 numbered milestone이 아니다. Named consumer·reproducible benchmark 또는 concrete blocker와 isolated experiment가 있어야 별도 revision에서 채택을 결정한다. 현재 SPEC의 scope와 acceptance를 중간에 재정의하지 않는다.

| Gate | 질문과 채택 기준 | 채택하지 않을 조건 |
| --- | --- | --- |
| R8-D Parallel independent-module verification | named consumer or measured DAG bottleneck and an isolated experiment showing deterministic resource scheduling and net benefit are required. No such evidence is currently present. | Serial verification meets measured needs or parallelism complicates solver/resource determinism without repeatable gain. |

## 원본 milestone 기준

### v8.8 — DAG Scale Qualification

- **목표·근거·작업:** DAG fixtures, graph sizes and edit classes를 대상으로 scaling behavior와 pathological cases를 측정한다. 기존 chain 결과는 DAG performance claim에 쓰지 않는다.
- **PASS / 검증·측정:** clean/incremental outputs와 graph order가 결정적이고, resource bound가 지켜진다. 성능 개선은 baseline 대비 재현된 graph shape에만 주장한다.
- **의존·범위 밖·중단:** v8.0–v8.3 의존. 무제한 크기 보장과 parallel proof implementation은 R8-D가 채택되지 않는 한 범위 밖이다. pathological graph에서 correctness 또는 resource bound가 무너지면 freeze를 중단한다.
- **계획:** 규모 M · 위험 높음 · 구조 영향 중간 · 회귀 위험 높음 · benchmark 필수.

### v8.9 — Incremental Project Engine Freeze

- **목표·근거·작업:** graph identity, compositional proof, incremental parse/link/proof, invalidation/recovery, diagnostics와 measured scale envelope를 하나의 frozen engine contract로 확정한다.
- **PASS / 검증·측정:** DAG mutation corpus, clean-vs-incremental differential, repeated scale benchmark, workspace regression, four-platform CLI, VSIX/browser static qualification이 통과하고 필수 항목에 `PARTIAL`/`FAIL`이 없다.
- **의존·범위 밖·중단:** v8.0–v8.3 및 v8.8 의존. Persistent cache는 별도 research gate다. Stale result나 unsupported scale claim이 있으면 v9를 시작하지 않는다.
- **계획:** 규모 M · 위험 매우 높음 · 구조 영향 중간 · 회귀 위험 매우 높음 · benchmark 필수.
