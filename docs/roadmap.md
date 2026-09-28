# Synthetix 로드맵

Synthetix는 **명세 → 합성 → 증명 → 실행**을 하나의 흐름으로 제공하는 verified programming language다. 기능 수보다 soundness와 명확한 verification boundary를 우선한다. WASM은 검증된 순수 로직의 기본 backend이며, 웹 runtime은 verified core와 상태·effect 세계를 연결한다.

이 문서는 **어디로 갈지**와 major·milestone만 추적한다. **무엇을 만들고 어떻게 PASS할지**는 해당 SPEC의 계약이다. Phase는 SPEC 내부 작업 단위, Milestone은 진행·결과 추적, Freeze는 최종 작업본 전체에 대한 SPEC 누적 검증이다. 구현 전에 Main이 해당 SPEC을 승인하고, 사용자가 명시한 더 좁은 범위는 언제나 우선한다.

v4~v7 실행 기록은 [보존 기록](roadmap-v4-v7-record.md), v7 실행 근거와 v8~v10의 최초 상세 계획은 [계획 snapshot](roadmap-v7-v10-record.md)에 보존되어 있다. 이 기록은 과거 판단의 출처이며, 현재 구현·검증 계약은 아래 SPEC이 담당한다.

## v8 — Incremental Project Engine

**목표:** 큰 프로젝트에서 변경된 부분만 정확하게 다시 계산·검증한다. v7.9를 기반으로 graph identity, DAG compositional proof, incremental edit recovery와 측정된 scale envelope를 확보한다.

| SPEC | Milestone 요약 |
| --- | --- |
| [SPEC-V8-A](specs/v8/SPEC-V8-A.md) | v8.0 graph/change baseline → v8.1 stable identity → v8.2 DAG compositional proof → v8.3 integrated incremental engine |
| [SPEC-V8-B](specs/v8/SPEC-V8-B.md) | v8.8 DAG scale qualification → v8.9 cumulative engine Freeze |

v8.4~v8.7은 의도적 공백이다. 별도 milestone이나 SPEC을 배정하지 않는다.

## v9 — Proof Provenance & Trust

**목표:** 어떤 source·dependency·semantics·candidate가 어떤 proof outcome과 executable을 만들었는지 추적하고, trust claim의 범위를 검증한다. Reader compatibility와 schema migration은 v10의 책임이다.

[SPEC-V9](specs/v9/SPEC-V9.md)의 milestone: v9.0 provenance/trust inventory → v9.1 canonical proof evidence → v9.2 source/semantics/candidate/WASM identity → v9.3 outcome records → v9.4 external assumption records → v9.5 artifact consistency → v9.6 reproducibility → v9.7 tamper boundaries → v9.8 trust claim audit → v9.9 Freeze.

## v10 — Stable Language & Ecosystem Contract

**목표:** source edition, semantics profile, proof artifact, CLI/VSCode, application/storage, WASM ABI와 release mapping의 안정성·호환성·migration 계약을 확정한다. v10.9는 1.0 release 선언이 아니다.

[SPEC-V10](specs/v10/SPEC-V10.md)의 milestone: v10.0 stable surface/consumer decision → v10.1 semantics/capability version matrix → v10.2 source edition → v10.3 proof artifact migration → v10.4 CLI/VSCode/tooling contract → v10.5 application/storage compatibility → v10.6 WASM ABI compatibility → v10.7 compatibility regression corpus → v10.8 release qualification → v10.9 Freeze.

Rust API 안정성은 실제 외부 consumer 근거가 있을 때 R10-A research gate에서 평가하며, 현재 numbered milestone에는 포함하지 않는다. Tag, release, publish는 별도 승인 대상이다.
