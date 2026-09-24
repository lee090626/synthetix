# Nested compositional benchmark comparison

- Before: `docs/v6-benchmark-baseline.json`; commit `00969e005abf37d6aa28a44fc6416742e9b13b36`, generated `1790188583142`.
- After: `docs/v6-benchmark-compositional-after.json`; commit `00969e005abf37d6aa28a44fc6416742e9b13b36`, generated `1790230600422`.
- After environment: macos/aarch64, rustc 1.98.1 (48a229cea 2026-09-01), repeats=3.
- Both runs use the same generated identity-chain shape without `requires`; the after production regression separately covers nested requires.
- Before timing verifies one root summary and emits one WASM module. After timing verifies every dependency-first summary and includes a release CLI build into a cleared temporary `dist`; total and artifact timings therefore have different scope.

| Depth | Before | Before summary ms | After | Summary count | After all-summary ms | Release CLI build ms | After total ms | WASM bytes |
| ---: | --- | ---: | --- | ---: | ---: | ---: | ---: | ---: |
| 1 | VERIFIED | 0.42975 | VERIFIED | 2 | 1.107083 | 14.702458 | 16.973208 | 58 |
| 2 | FAILED | 0.10475 | VERIFIED | 3 | 1.363334 | 11.401834 | 13.110583 | 78 |
| 4 | FAILED | 0.1045 | VERIFIED | 5 | 2.182292 | 14.987208 | 17.411792 | 118 |
| 8 | FAILED | 0.096708 | VERIFIED | 9 | 3.828625 | 22.906208 | 27.214833 | 198 |

## Failure details

- Before depth 1: project load -> production summary verifier with fixed identity candidate -> WASM emission on proof success; candidate search excluded; contract_count=2; failure=none
- After depth 1: project load -> dependency-first verified-summary API with fixed identity candidates -> release axiom build in a fresh output directory; direct candidate search excluded, production build includes synthesis; contract_count=2; summary_count=2; failure=none
- Before depth 2: project load -> production summary verifier with fixed identity candidate -> WASM emission on proof success; candidate search excluded; contract_count=3; failure=unresolved contract call: level0; link the project before verification
- After depth 2: project load -> dependency-first verified-summary API with fixed identity candidates -> release axiom build in a fresh output directory; direct candidate search excluded, production build includes synthesis; contract_count=3; summary_count=3; failure=none
- Before depth 4: project load -> production summary verifier with fixed identity candidate -> WASM emission on proof success; candidate search excluded; contract_count=5; failure=unresolved contract call: level2; link the project before verification
- After depth 4: project load -> dependency-first verified-summary API with fixed identity candidates -> release axiom build in a fresh output directory; direct candidate search excluded, production build includes synthesis; contract_count=5; summary_count=5; failure=none
- Before depth 8: project load -> production summary verifier with fixed identity candidate -> WASM emission on proof success; candidate search excluded; contract_count=9; failure=unresolved contract call: level6; link the project before verification
- After depth 8: project load -> dependency-first verified-summary API with fixed identity candidates -> release axiom build in a fresh output directory; direct candidate search excluded, production build includes synthesis; contract_count=9; summary_count=9; failure=none
