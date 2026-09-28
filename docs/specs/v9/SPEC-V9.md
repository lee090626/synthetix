# SPEC-V9 — Proof Provenance & Trust

**범위:** v9.0~v9.9 · **상태:** DRAFT · **SPEC 버전:** 0.1. v8.9 Freeze 후 실제 baseline을 다시 확인하고 Main이 구현 전 승인한다. 이 초안은 v9 구현이나 PASS를 뜻하지 않는다.

## Goal

Source, dependency, semantics, candidate, proof outcome, WASM과 external assumption을 canonical evidence로 연결하고, 각 trust claim을 검증 가능한 범위로 제한한다.

## Scope / Out of Scope

- 범위: evidence inventory, canonical record, exact identities, failure outcomes, extern assumptions, artifact consistency, same-environment reproducibility, tamper detection, claim audit와 누적 Freeze.
- 범위 밖: source semantics 변경, v10의 reader compatibility·schema migration, 서명·supply-chain attestation·solver proof certificate(R9-A 연구 gate 전), 외부 구현·network/DB fact의 correctness 증명, 새 cloud/signing service.
- 승인 범위는 SPEC 재현에 필요한 의존 변경으로 판단하되 사용자의 명시적 좁은 제한을 우선한다.

## Baseline

- v8.9 PASS 및 v7.9 [Freeze 근거](../../v7.9-benchmark.md)를 선행한다. 실제 시작 시 checkout의 source commit/branch/dirty 범위와 CLI report, `app.proof.json`, WASM, semantics id, cache, extern producer/reader를 inventory한다.
- 기존 CLI `stable_hash`는 FNV-1a 64-bit fingerprint이며 cryptographic authenticity가 아니다. Web manifest schema `4`는 proof/effect 구분을 담지만 semantics/source/WASM/tool build identity가 빠져 있다. Extern implementation은 `UNVERIFIED`, audit metadata는 `NOT_PROVIDED`다. 이는 기존 계획의 조사 결과이며 실행 시 재확인한다.

## Invariants

- Provenance는 solver나 translation의 독립 수학적 증명이 아니다. `VERIFIED`는 verified core로만 한정하고 extern implementation, host data, browser/application failures를 proof fact로 승격하지 않는다.
- `INVALID`, `TIMEOUT`, `RESOURCE_LIMIT`, `NO_CANDIDATE`, counterexample, verified 결과 분류와 synthesis bounds를 유지한다. 실패에는 verified/WASM output을 주장하지 않는다.
- Digest mismatch와 tamper는 fail-closed한다. Same-environment determinism을 요구하되 서로 다른 OS/compiler 간 byte identity는 근거 없이 약속하지 않는다.

## Required Behavior

1. 모든 evidence field의 producer, source of truth, proof scope와 trust claim을 inventory한다.
2. Canonical evidence가 format/model version과 outcome vocabulary를 round-trip하고 exact source/dependency/candidate/WASM/build identity를 연결한다.
3. External effect assumptions와 host-provided evidence를 verified proof와 분리한다.
4. Build consistency, same-environment reproducibility, field-level tamper fixture와 trust claim audit를 수행한다.

## Acceptance Criteria

- v9.0~v9.4: field classification, canonical round-trip, digest match/mismatch, 모든 proof outcome fixture, extern 정상·오류·누락 경계가 원본 milestone 기준대로 PASS다.
- v9.5~v9.8: clean/tampered artifact 판정, 동일 환경 byte identity, 각 identity field의 tamper 검출, machine-readable evidence 또는 documented assumption에 연결된 claim이 PASS다.
- v9.9: 모든 required v9 gate와 workspace regression, 네 플랫폼 CLI, VSIX/browser/static, clean/reproducible build가 최종 작업본에서 PASS다. 필수 CONDITIONAL·FAIL·PENDING은 Freeze 전 닫는다.

## Benchmark / Provenance

- v9.2 artifact size, v9.3 manifest size·serialization time, v9.5 consistency 비용, v9.6 reproducibility와 v9.9 qualification을 원본 계획대로 측정한다. 같은 source/config/toolchain/host/profile의 before/after raw repeats, median·range, source·fixture·artifact SHA-256과 schema/version identity를 남긴다.
- Cryptographic claim이 필요한 경우 표준 SHA-256 구현을 사용한다. FNV-1a를 authenticity로 표현하지 않는다. Main이 raw 수치, SHA, JSON↔Markdown, status/link를 먼저 일괄 점검하고 Reviewer가 완성 diff와 모든 claim을 검토한다.

## Allowed Repair

- 승인 범위의 동일 root cause 코드·동작 repair 최대 2회; affected verification과 review를 다시 수행한다.
- Source-supported 문서·metadata 오류는 일괄 audit/정정하며 횟수에서 제외한다. Raw benchmark나 expected classification을 편의상 바꾸지 않는다.

## BLOCKED Conditions

- Semantics·proof classification·search bounds·acceptance 변경, scope 밖 기능, 사용자 trade-off, 외부 권한·환경 변경이 실제 필요하거나 동일 root cause repair 2회가 실패했다.
- Integrity mismatch, trust overclaim, extern의 잘못된 `VERIFIED` 승격 등은 먼저 FAIL gate로 수리한다. 위 조건 없이는 사소한 정합성 문제를 BLOCKED로 만들지 않는다.

## Verification Plan

1. v8.9 baseline과 field/claim inventory를 고정하고 성공·실패·tamper fixtures 및 expected classification을 기록한다.
2. Parser/CLI/web artifact round-trip, extern runtime error separation, clean/tamper/reproducibility, reference/Z3/WASM regression과 artifact benchmark를 수행한다.
3. Main preflight → Reviewer 전체 diff·provenance audit → proof-critical/Freeze에서 필요한 경우 독립 Verifier → 최종 작업본 누적 qualification 순서로 판단한다.

## Phases

| Phase | 작업 단위 | 추적 milestone | Gate |
| --- | --- | --- | --- |
| P1 — Evidence contract | Field/claim inventory, canonical outcome와 identity, extern assumption | v9.0~v9.4 | Evidence scope·round-trip·분류 PASS |
| P2 — Integrity and Freeze | Consistency, reproducibility, tamper, trust audit와 누적 qualification | v9.5~v9.9 | 모든 필수 evidence와 플랫폼 gate PASS |

R9-A proof certificate/signature/attestation은 외부 consumer, producer/checker/key ownership, update/revocation 및 build provenance 근거가 있을 때만 별도 연구·승격한다. 이 SPEC의 numbered milestone을 암묵적으로 확대하지 않는다.

## 관련 Research Gate

Research gate는 numbered milestone이 아니다. Named consumer·reproducible benchmark 또는 concrete blocker와 isolated experiment가 있어야 별도 revision에서 채택을 결정한다. 현재 SPEC의 scope와 acceptance를 중간에 재정의하지 않는다.

| Gate | 질문과 채택 기준 | 채택하지 않을 조건 |
| --- | --- | --- |
| R9-A Proof certificate, signature, supply-chain attestation | External consumer가 Z3 solver를 신뢰하지 않고 manifest/artifact를 independently validate해야 하는가? Producer, checker, key ownership, update/revocation, build provenance까지 설계돼야 한다. | Hash/manifest consumer만 있고 proof certificate checker가 없으면 signatures를 추가하지 않는다. |

원본의 “중단”이나 `PARTIAL`은 자동 `BLOCKED`가 아니다. 아래 문구는 목표·PASS·의존·제외·규모의 추적 근거이며, 현재 gate 상태는 이 SPEC의 FAIL/PENDING/BLOCKED 기준으로 판정한다.

## 원본 milestone 기준

### v9.0 — Provenance and Trust Claim Inventory

- **목표·근거·작업:** CLI report, web proof manifest, WASM, semantics ids의 producer·field·proof scope와 현재 claim의 생성 경로를 inventory한다. 장기 reader support와 external consumer compatibility는 v10에서 결정한다.
- **PASS / 검증·측정:** 모든 field가 source of truth, cache-only, UI/runtime hint,
  integrity fingerprint 중 하나로 분류되고, trust claim과 current evidence를 나눈다.
- **의존·범위 밖·중단:** v8.9 의존. 서명, supply-chain attestation, solver proof certificate는
  R9-A 채택 전 범위 밖이다. 어떤 field가 어떤 proof claim을 뒷받침하는지 확인할 수 없으면
  canonical record를 설계하지 않는다.
- **계획:** 규모 S · 위험 낮음 · 구조 영향 낮음 · 회귀 위험 낮음 · benchmark 불필요.

### v9.1 — Canonical Proof Evidence

- **목표·근거·작업:** CLI report와 web proof manifest의 format version, semantics/model
  version, verified scope, synthesis bounds를 서로 다른 필드로 정의한다. Success와 failure
  result vocabulary를 고정하고, 현재 external boundary 구분을 보존한다.
- **PASS / 검증·측정:** 새 canonical evidence record가 required proof fields와 outcome vocabulary를 round-trip하며 보존한다. Legacy version-4 reader/migration 지원은 v10의 결정과 완료 기준이다.
- **의존·범위 밖·중단:** v9.0 의존. Source semantics 변경은 범위 밖이다. `VERIFIED`가
  `UNVERIFIED` extern implementation까지 확장되거나 canonical record의 required evidence가
  조용히 잘리면 중단한다.
- **계획:** 규모 M · 위험 높음 · 구조 영향 중간 · 회귀 위험 높음 · benchmark 불필요.

### v9.2 — Source, Semantics, Candidate, and WASM Identity

- **목표·근거·작업:** Web manifest에 project-relative source, exact source/dependency
  digests, candidate identity, generated WASM digest, semantics id와 build identity를 연결한다.
  현재 CLI FNV-1a 64-bit hash는 cryptographic authenticity가 아님을 표현한다.
- **PASS / 검증·측정:** Static fixture의 source/dependency/WASM bytes를 manifest field와
  다시 비교해 match/mismatch를 판정한다. Digest 알고리즘은 consumer threat model에 따라
  결정하고, cryptographic claim을 하면 표준 SHA-256 implementation을 사용한다.
- **의존·범위 밖·중단:** v9.1 의존. Signed provenance는 R9-A 연구 gate의 채택 기준을
  통과하기 전에는 범위 밖이다. Digest mismatch가 build success로 표시되면 중단한다.
- **계획:** 규모 M · 위험 높음 · 구조 영향 중간 · 회귀 위험 높음 · artifact-size benchmark 필요.

### v9.3 — Complete Proof Outcome Records

- **목표·근거·작업:** CLI already reports `INVALID`, `TIMEOUT`, `RESOURCE_LIMIT`, counterexample
  및 verified result를 나눠 기록한다. Project artifact에서도 source별 outcome, synthesis
  bounds, diagnostic, candidate/proof status가 실패 때도 혼동 없이 나타나는지 통일한다.
- **PASS / 검증·측정:** 각 state fixture에서 machine parser가 같은 classification을 읽고,
  실패 case는 verified/WASM output을 주장하지 않는다. Manifest size와 serialization time을
  기존 결과와 비교한다.
- **의존·범위 밖·중단:** v9.1–v9.2 의존. Speculation 기반 “불가능” 판정은 범위 밖이다.
  `NO_CANDIDATE`/`RESOURCE_LIMIT`를 contradiction으로 표현하면 중단한다.
- **계획:** 규모 M · 위험 중간 · 구조 영향 낮음 · 회귀 위험 중간 · benchmark 필요.

### v9.4 — External Effect Assumption Records

- **목표·근거·작업:** `extern` implementation은 현재 UNVERIFIED이고 response는 declared
  type만 검사하며 audit status는 `NOT_PROVIDED`다. Manifest는 declaration, source,
  effect class, response type validation, host-supplied optional evidence를 각각 표시한다.
- **PASS / 검증·측정:** Adapter 미등록/오류/잘못된 응답과 정상 응답 모두 proof result와
  application runtime error를 분리한다. Optional host evidence가 빠지면 `NOT_PROVIDED`를
  유지하며 이를 VERIFIED로 승격하지 않는다.
- **의존·범위 밖·중단:** v9.1 의존. Network/DB/external facts의 correctness를 증명하는 일은
  범위 밖이다. Host data를 proof fact로 조용히 승격하면 중단한다.
- **계획:** 규모 M · 위험 높음 · 구조 영향 중간 · 회귀 위험 높음 · benchmark 불필요.

### v9.5 — Artifact Consistency Check

- **목표·근거·작업:** Build integration가 emitted static files, proof manifest, source snapshots,
  WASM exports and hashes가 한 build에 속하는지 cross-check한다. 기존 `axiom build`의
  output contract를 유지한다.
- **PASS / 검증·측정:** Clean build와 deliberate tamper fixtures가 각각 consistent/invalid로
  분류된다. 검사는 Z3를 다시 실행하지 않으며 `proof verified`의 독립 재증명이라 부르지 않는다.
- **의존·범위 밖·중단:** v9.1–v9.4 의존. New cloud/signing service는 범위 밖이다. Hash,
  contract name, source path 간 하나라도 불일치하면 release output을 거부한다.
- **계획:** 규모 M · 위험 높음 · 구조 영향 중간 · 회귀 위험 높음 · benchmark 필요.

### v9.6 — Reproducible Manifest and Bundle

- **목표·근거·작업:** v6.9가 동일 host의 두 project directory에서 static files를 비교한 근거를
  반복하고, 새 manifest fields가 output determinism을 깨지 않는지 확인한다.
- **PASS / 검증·측정:** 동일 clean source, locked dependencies, toolchain, config에서 두 번
  생성한 static files와 manifest가 byte-identical하다. Timestamp/build path 같은 의도적
  nondeterministic fields는 명시하거나 산출물에서 제외한다.
- **의존·범위 밖·중단:** v9.2–v9.5 의존. 서로 다른 compiler/OS 간 byte identity를
  약속하지 않는다. Same-environment output mismatch가 설명되지 않으면 freeze를 멈춘다.
- **계획:** 규모 M · 위험 중간 · 구조 영향 낮음 · 회귀 위험 중간 · benchmark 필수.

### v9.7 — Tamper and Integrity Boundaries

- **목표·근거·작업:** source/dependency/candidate/WASM evidence를 변조하는 fixture로 integrity verifier의 감지 범위와 한계를 고정한다.
- **PASS / 검증·측정:** source, dependency, semantics, candidate, outcome, WASM의 각 field 단위 변조가 검출되고, digest 검사는 source identity proof나 solver 재검증을 대신하지 않는다.
- **의존·범위 밖·중단:** v9.2–v9.6 의존. Consumer migration policy는 v10 범위다. 검증 가능한 bytes의 tamper를 통과시키면 중단한다.
- **계획:** 규모 M · 위험 높음 · 구조 영향 낮음 · 회귀 위험 높음 · benchmark 불필요.

### v9.8 — Provenance and Trust Claim Audit

- **목표·근거·작업:** manifest, CLI output, release notes가 `VERIFIED`의 정확한 범위, solver
  assumption, WASM conformance, source hash, external boundary를 과장하지 않는지 검토한다.
- **PASS / 검증·측정:** 각 claim이 machine-readable evidence 또는 documented assumption에
  연결된다. `FNV1a64`는 digest only, Z3 proof result는 solver와 translation TCB에 의존,
  external adapter는 type-checked only라고 분명하다.
- **의존·범위 밖·중단:** v9.0–v9.7 의존. Marketing-only claim은 범위 밖이다. Claim에
  반례가 있으면 v9.9 freeze를 막는다.
- **계획:** 규모 S · 위험 중간 · 구조 영향 낮음 · 회귀 위험 낮음 · benchmark 불필요.

### v9.9 — Provenance and Trust Freeze

- **목표·근거·작업:** canonical evidence, identities, outcomes, external assumptions, tamper checks, reproducibility와 trust claims를 freeze한다. Consumer compatibility is explicitly deferred to v10.
- **PASS / 검증·측정:** evidence fixtures, tamper cases, clean/reproducible build, workspace regression, four-target CLI, VSIX and browser/static qualification pass; required items have no `PARTIAL`/`FAIL`.
- **의존·범위 밖·중단:** v9.0–v9.8 의존. Reader migration과 long-term schema support는 범위 밖이며 v10. Unimplemented proof certificate를 claim하지 않는다. Integrity gap이 있으면 v10을 멈춘다.
- **계획:** 규모 M · 위험 높음 · 구조 영향 중간 · 회귀 위험 높음 · benchmark 필수.
