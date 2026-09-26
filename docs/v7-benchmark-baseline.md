# v7.0 Clean Performance and Architecture Baseline

Measurements are observations, not performance guarantees.

## Environment

- Source: `2e355cb91c872963048fe6a6dd167be75568e469` (clean before and after trials)
- Platform: `macOS-26.5.1-arm64-arm-64bit-Mach-O` / `arm64`
- Rust: `rustc 1.98.1 (48a229cea 2026-09-01)`; Cargo: `cargo 1.98.1 (797e8a9bc 2026-08-05)`; Node: `v24.19.0`
- Historical v6 timing values were not reused: the v6 report was generated from a dirty tree.
- `NO_CANDIDATE` was not observed among the 54 scenarios.
- Hosted CI wall-clock duration, actual RSS, and allocator-level allocations are not measured.
- Runtime/WASM rows use the existing Node DOM shim; this is not browser QA.

## Harness profiles

- Scenario harness: release profile, 3 repeats per scenario.
- Proof-cache cases: `debug cargo test profile`.
- Generated static/runtime rows: single run per trial.
- Baseline: 2 trials (fresh target, then warm target).

## Trial command timings

### Trial 1 (fresh target)

| Command | Exit | Duration (ms) |
|---|---:|---:|
| `cargo build --workspace` | 0 | 173019.268 |
| `cargo test --workspace` | 0 | 37818.095 |
| `cargo build --release -p axiom-cli` | 0 | 173682.159 |
| `/private/tmp/synthetix-v7-baseline-source-20260925/target/v7-baseline-0kpp4teb/release/axiom check examples/basics/mul8.ax` × 10 (process start + contract check) | all 0 | median 5.422; range 5.090–5.596 |

### Trial 2 (warm target)

| Command | Exit | Duration (ms) |
|---|---:|---:|
| `cargo build --workspace` | 0 | 44.176 |
| `cargo test --workspace` | 0 | 29031.139 |
| `cargo build --release -p axiom-cli` | 0 | 91.521 |
| `/private/tmp/synthetix-v7-baseline-source-20260925/target/v7-baseline-0kpp4teb/release/axiom check examples/basics/mul8.ax` × 10 (process start + contract check) | all 0 | median 5.205; range 4.973–5.796 |

## Scenario comparison

All 54 scenario names, bounds, results, failure classifications, and resource statuses matched the manifest and both trials.

## Scenario measurements

Per-case timing and counts are recorded in the JSON. This table shows stable identity, bounds, and outcome.

| Name | Category | Source | Bounds | Result | Failure | Resource |
|---|---|---|---|---|---|---|
| mul8 | synthesis | `examples/basics/mul8.ax` | `{"timeout_ms":null,"max_candidates":50000,"max_memory_bytes":67108864}` | FOUND | — | — |
| property_max | synthesis | `examples/properties/property_max.ax` | `{"timeout_ms":null,"max_candidates":50000,"max_memory_bytes":67108864}` | FOUND | — | — |
| property_clamp | synthesis | `examples/properties/property_clamp.ax` | `{"timeout_ms":null,"max_candidates":50000,"max_memory_bytes":67108864}` | FOUND | — | — |
| minimum | synthesis | `benchmarks/v6/synthesis/minimum.ax` | `{"timeout_ms":null,"max_candidates":50000,"max_memory_bytes":67108864}` | FOUND | — | — |
| maximum_3 | synthesis | `benchmarks/v6/synthesis/maximum_3.ax` | `{"timeout_ms":null,"max_candidates":50000,"max_memory_bytes":67108864}` | FOUND | — | — |
| bitwise_combo | synthesis | `benchmarks/v6/synthesis/bitwise_combo.ax` | `{"timeout_ms":null,"max_candidates":50000,"max_memory_bytes":67108864}` | FOUND | — | — |
| sum_3 | synthesis | `benchmarks/v6/synthesis/sum_3.ax` | `{"timeout_ms":null,"max_candidates":50000,"max_memory_bytes":67108864}` | FOUND | — | — |
| sum_8 | synthesis | `benchmarks/v6/synthesis/sum_8.ax` | `{"timeout_ms":null,"max_candidates":50000,"max_memory_bytes":67108864}` | FOUND | — | — |
| cegis_hidden_nine | synthesis | `benchmarks/v6/synthesis/cegis_hidden_nine.ax` | `{"timeout_ms":null,"max_candidates":50000,"max_memory_bytes":268435456}` | RESOURCE_LIMIT | explicit_search_resource_limit | RESOURCE_LIMIT |
| power_5_bounded | synthesis | `benchmarks/v6/synthesis/power_5.ax` | `{"timeout_ms":2000,"max_candidates":1000,"max_memory_bytes":67108864}` | FOUND | — | — |
| explicit_timeout | synthesis | `examples/basics/mul8.ax` | `{"timeout_ms":0,"max_candidates":100,"max_memory_bytes":67108864}` | TIMEOUT | explicit_search_timeout | — |
| explicit_candidate_resource_limit | synthesis | `examples/basics/mul8.ax` | `{"timeout_ms":null,"max_candidates":0,"max_memory_bytes":67108864}` | RESOURCE_LIMIT | sample_cartesian_product_or_zero_budget_limit | RESOURCE_LIMIT |
| z3_sum_params_1 | z3_verification | `generated:u32 sum with 1 scalar parameters` | `{"timeout_ms":null,"max_candidates":null,"max_memory_bytes":null}` | VERIFIED | — | — |
| z3_sum_params_2 | z3_verification | `generated:u32 sum with 2 scalar parameters` | `{"timeout_ms":null,"max_candidates":null,"max_memory_bytes":null}` | VERIFIED | — | — |
| z3_sum_params_4 | z3_verification | `generated:u32 sum with 4 scalar parameters` | `{"timeout_ms":null,"max_candidates":null,"max_memory_bytes":null}` | VERIFIED | — | — |
| z3_sum_params_8 | z3_verification | `generated:u32 sum with 8 scalar parameters` | `{"timeout_ms":null,"max_candidates":null,"max_memory_bytes":null}` | VERIFIED | — | — |
| z3_sum_params_16 | z3_verification | `generated:u32 sum with 16 scalar parameters` | `{"timeout_ms":null,"max_candidates":null,"max_memory_bytes":null}` | VERIFIED | — | — |
| z3_branch_count_1 | z3_verification | `generated nested conditional with 1 branches` | `{"timeout_ms":null,"max_candidates":null,"max_memory_bytes":null}` | VERIFIED | — | — |
| z3_branch_count_2 | z3_verification | `generated nested conditional with 2 branches` | `{"timeout_ms":null,"max_candidates":null,"max_memory_bytes":null}` | VERIFIED | — | — |
| z3_branch_count_4 | z3_verification | `generated nested conditional with 4 branches` | `{"timeout_ms":null,"max_candidates":null,"max_memory_bytes":null}` | VERIFIED | — | — |
| z3_branch_count_8 | z3_verification | `generated nested conditional with 8 branches` | `{"timeout_ms":null,"max_candidates":null,"max_memory_bytes":null}` | VERIFIED | — | — |
| counterexample_property_max_arg0 | z3_verification | `examples/properties/property_max.ax` | `{"timeout_ms":null,"max_candidates":null,"max_memory_bytes":null}` | COUNTEREXAMPLE | solver_found_counterexample | — |
| compositional_call_depth_1 | project_compositional | `generated single-module summary chain depth=1; identity contracts` | `{"timeout_ms":null,"max_candidates":50000,"max_memory_bytes":67108864}` | VERIFIED | — | — |
| compositional_call_depth_2 | project_compositional | `generated single-module summary chain depth=2; identity contracts` | `{"timeout_ms":null,"max_candidates":50000,"max_memory_bytes":67108864}` | VERIFIED | — | — |
| compositional_call_depth_4 | project_compositional | `generated single-module summary chain depth=4; identity contracts` | `{"timeout_ms":null,"max_candidates":50000,"max_memory_bytes":67108864}` | VERIFIED | — | — |
| compositional_call_depth_8 | project_compositional | `generated single-module summary chain depth=8; identity contracts` | `{"timeout_ms":null,"max_candidates":50000,"max_memory_bytes":67108864}` | VERIFIED | — | — |
| proof_cache_five_contract_cold | incremental_proof_cache | `cargo test -p axiom-cli proof_cache_ -- --nocapture` | `{"timeout_ms":null,"max_candidates":50000,"max_memory_bytes":67108864}` | CACHE_COLD_VERIFIED | — | — |
| proof_cache_five_contract_warm | incremental_proof_cache | `cargo test -p axiom-cli proof_cache_ -- --nocapture` | `{"timeout_ms":null,"max_candidates":50000,"max_memory_bytes":67108864}` | CACHE_WARM_HIT | — | — |
| proof_cache_leaf_change | incremental_proof_cache | `cargo test -p axiom-cli proof_cache_ -- --nocapture` | `{"timeout_ms":null,"max_candidates":50000,"max_memory_bytes":67108864}` | DEPENDENTS_REVERIFIED | — | — |
| proof_cache_root_change | incremental_proof_cache | `cargo test -p axiom-cli proof_cache_ -- --nocapture` | `{"timeout_ms":null,"max_candidates":50000,"max_memory_bytes":67108864}` | ROOT_REVERIFIED | — | — |
| proof_cache_32_module_cold | incremental_proof_cache | `cargo test -p axiom-cli proof_cache_ -- --nocapture` | `{"timeout_ms":null,"max_candidates":50000,"max_memory_bytes":67108864}` | CACHE_COLD_VERIFIED | — | — |
| proof_cache_32_module_warm | incremental_proof_cache | `cargo test -p axiom-cli proof_cache_ -- --nocapture` | `{"timeout_ms":null,"max_candidates":50000,"max_memory_bytes":67108864}` | CACHE_WARM_HIT | — | — |
| call_site_precondition_source_diagnostic | compositional_diagnostic | `generated imported positive(value) call without caller requires` | `{"timeout_ms":null,"max_candidates":null,"max_memory_bytes":null}` | EXPECTED_CALL_PRECONDITION_DIAGNOSTIC | — | — |
| circular_call_rejection | compositional_diagnostic | `generated two-module first <-> second import/call cycle` | `{"timeout_ms":null,"max_candidates":null,"max_memory_bytes":null}` | EXPECTED_CIRCULAR_CALL_REJECTION | — | — |
| application_value_boundary_size_0 | application_data_serialization | `ApplicationValue tagged JSON list + record + text, measured without WASM lowering` | `{"timeout_ms":null,"max_candidates":null,"max_memory_bytes":null}` | APPLICATION_DATA_ONLY | — | — |
| application_value_boundary_size_10 | application_data_serialization | `ApplicationValue tagged JSON list + record + text, measured without WASM lowering` | `{"timeout_ms":null,"max_candidates":null,"max_memory_bytes":null}` | APPLICATION_DATA_ONLY | — | — |
| application_value_boundary_size_100 | application_data_serialization | `ApplicationValue tagged JSON list + record + text, measured without WASM lowering` | `{"timeout_ms":null,"max_candidates":null,"max_memory_bytes":null}` | APPLICATION_DATA_ONLY | — | — |
| application_value_boundary_size_1000 | application_data_serialization | `ApplicationValue tagged JSON list + record + text, measured without WASM lowering` | `{"timeout_ms":null,"max_candidates":null,"max_memory_bytes":null}` | APPLICATION_DATA_ONLY | — | — |
| application_value_boundary_size_10000 | application_data_serialization | `ApplicationValue tagged JSON list + record + text, measured without WASM lowering` | `{"timeout_ms":null,"max_candidates":null,"max_memory_bytes":null}` | APPLICATION_DATA_ONLY | — | — |
| static_runtime_list_0_text_0 | static_bundle_runtime_and_persistence | `benchmarks/v6/application-template built by release axiom CLI; generated app.js and app.wasm executed in Node DOM shim` | `{"timeout_ms":null,"max_candidates":50000,"max_memory_bytes":67108864}` | VERIFIED_WASM_RUNTIME_AND_PERSISTENCE | — | — |
| static_runtime_list_10_text_0 | static_bundle_runtime_and_persistence | `benchmarks/v6/application-template built by release axiom CLI; generated app.js and app.wasm executed in Node DOM shim` | `{"timeout_ms":null,"max_candidates":50000,"max_memory_bytes":67108864}` | VERIFIED_WASM_RUNTIME_AND_PERSISTENCE | — | — |
| static_runtime_list_100_text_0 | static_bundle_runtime_and_persistence | `benchmarks/v6/application-template built by release axiom CLI; generated app.js and app.wasm executed in Node DOM shim` | `{"timeout_ms":null,"max_candidates":50000,"max_memory_bytes":67108864}` | VERIFIED_WASM_RUNTIME_AND_PERSISTENCE | — | — |
| static_runtime_list_1000_text_0 | static_bundle_runtime_and_persistence | `benchmarks/v6/application-template built by release axiom CLI; generated app.js and app.wasm executed in Node DOM shim` | `{"timeout_ms":null,"max_candidates":50000,"max_memory_bytes":67108864}` | VERIFIED_WASM_RUNTIME_AND_PERSISTENCE | — | — |
| static_runtime_list_10_text_0 | static_bundle_runtime_and_persistence | `benchmarks/v6/application-template built by release axiom CLI; generated app.js and app.wasm executed in Node DOM shim` | `{"timeout_ms":null,"max_candidates":50000,"max_memory_bytes":67108864}` | VERIFIED_WASM_RUNTIME_AND_PERSISTENCE | — | — |
| static_runtime_list_10_text_1024 | static_bundle_runtime_and_persistence | `benchmarks/v6/application-template built by release axiom CLI; generated app.js and app.wasm executed in Node DOM shim` | `{"timeout_ms":null,"max_candidates":50000,"max_memory_bytes":67108864}` | VERIFIED_WASM_RUNTIME_AND_PERSISTENCE | — | — |
| static_runtime_list_10_text_16384 | static_bundle_runtime_and_persistence | `benchmarks/v6/application-template built by release axiom CLI; generated app.js and app.wasm executed in Node DOM shim` | `{"timeout_ms":null,"max_candidates":50000,"max_memory_bytes":67108864}` | VERIFIED_WASM_RUNTIME_AND_PERSISTENCE | — | — |
| static_runtime_list_10_text_65536 | static_bundle_runtime_and_persistence | `benchmarks/v6/application-template built by release axiom CLI; generated app.js and app.wasm executed in Node DOM shim` | `{"timeout_ms":null,"max_candidates":50000,"max_memory_bytes":67108864}` | VERIFIED_WASM_RUNTIME_AND_PERSISTENCE | — | — |
| v6_release_array_loop_recursion_application | static_release_runtime_and_effect | `examples/v6-release built by release CLI; generated bundle served over local HTTP and executed in Node DOM shim` | `{"timeout_ms":null,"max_candidates":50000,"max_memory_bytes":67108864}` | VERIFIED_RELEASE_PATH | — | — |
| verified_array_parameter_boundary | language_expressiveness | `benchmarks/v6/expressiveness/array_boundary.ax` | `{"timeout_ms":null,"max_candidates":null,"max_memory_bytes":null}` | EXPECTED_INVALID | — | — |
| unrolled_validation_4 | synthesis | `benchmarks/v6/expressiveness/validate_4.ax` | `{"timeout_ms":null,"max_candidates":50000,"max_memory_bytes":67108864}` | FOUND | — | — |
| unrolled_bounded_search_4 | synthesis | `benchmarks/v6/expressiveness/bounded_search_4.ax` | `{"timeout_ms":null,"max_candidates":50000,"max_memory_bytes":67108864}` | FOUND | — | — |
| fixed_array_sum4 | verified_feature_proof | `benchmarks/v6/final/array_sum4.ax` | `{"timeout_ms":null,"max_candidates":null,"max_memory_bytes":null}` | VERIFIED | — | — |
| bounded_loop_sum4 | verified_feature_proof | `benchmarks/v6/final/bounded_loop_sum4.ax` | `{"timeout_ms":null,"max_candidates":null,"max_memory_bytes":null}` | VERIFIED | — | — |
| bounded_recursion_sum_to | verified_feature_proof | `benchmarks/v6/final/recursive_sum_to.ax` | `{"timeout_ms":null,"max_candidates":null,"max_memory_bytes":null}` | VERIFIED | — | — |

## Required milestone checks (clean v6.9 worktree)

All five required commands passed at source commit `2e355cb91c872963048fe6a6dd167be75568e469`, with the working tree clean. A target directory isolated inside the baseline worktree prevented artifact reuse from the v7.1 checkout.

| Command | Result | Evidence |
|---|---|---|
| `cargo test --workspace` | PASS | 146 passed, 0 failed |
| `cargo clippy --workspace --all-targets -- -D warnings` | PASS | Completed without warnings |
| `cargo fmt --all -- --check` | PASS | Clean |
| `git diff --check` | PASS | Clean |
| `cargo build --release -p axiom-cli` | PASS | Completed successfully |
