# v6 Final Performance Comparison

This comparison keeps the original v6.0 baseline intact and links it to the
nested-summary follow-up and the final working-tree run:

- [v6.0 baseline JSON](v6-benchmark-baseline.json)
- [Nested-summary follow-up JSON](v6-benchmark-compositional-after.json)
- [Final measurements JSON](v6-benchmark-final.json)
- [Final measurements Markdown](v6-benchmark-final.md)

Reproduce the final run with `bash scripts/run-v6-final-benchmarks.sh`. The final
run used HEAD `103e073a816535d2dbfd614c8705edf093aca4d9` plus the dirty v6 working
tree, macOS arm64, `rustc 1.98.1`, Node `v24.19.0`, and three repeats with median
timings. Default synthesis bounds were no timeout, 50,000 candidates, and 64 MiB
estimated memory. Incremental-cache measurements invoke the existing proof-cache
tests in the debug test profile; all other harness measurements use release.

## Nested compositional verification

The v6.0 baseline measured the direct project path only and failed beyond depth 1.
The follow-up and final runs include dependency-first summaries and a release CLI
build. The v6.0 column therefore has no comparable release-build timing.

| Depth | v6.0 result / direct total ms | Nested-summary result / total ms | Final result / total ms | Final summary ms | Final release build ms |
| ---: | --- | --- | --- | ---: | ---: |
| 1 | VERIFIED / 0.486 | VERIFIED / 16.973 | VERIFIED / 18.029 | 1.804 | 15.180 |
| 2 | FAILED / 0.151 | VERIFIED / 13.111 | VERIFIED / 16.141 | 1.919 | 13.965 |
| 4 | FAILED / 0.174 | VERIFIED / 17.412 | VERIFIED / 21.933 | 3.198 | 18.303 |
| 8 | FAILED / 0.161 | VERIFIED / 27.215 | VERIFIED / 34.935 | 5.521 | 28.685 |

These timings are local observations; the final source adds summary accounting and
release artifact work, so elapsed-time differences do not establish a regression
or speedup by themselves. Depths 2/4/8 changed from unresolved-call failures to
verified release builds.

## Synthesis scaling cases

All three snapshots use the same default candidate and memory bounds. The final
run includes the proof-checked fallback recorded for v6.6.

| Case | v6.0 | Nested-summary follow-up | Final |
| --- | --- | --- | --- |
| `maximum_3` | RESOURCE_LIMIT; 0 checked, 138,136 generated; 142.747 ms | RESOURCE_LIMIT; 0 checked, 138,136 generated; 147.417 ms | FOUND; 1 checked, 138,137 generated, 138,136 rejected; 238.523 ms |
| `bitwise_combo` | RESOURCE_LIMIT; 0 checked, 96,370 generated; 65.004 ms | RESOURCE_LIMIT; 0 checked, 96,370 generated; 67.059 ms | FOUND; 1 checked, 96,371 generated, 96,370 rejected; 76.516 ms |
| `sum_8` | RESOURCE_LIMIT; 0 checked, 0 generated; 0.947 ms | RESOURCE_LIMIT; 0 checked, 0 generated; 0.957 ms | FOUND; 1 checked, 1 generated, 0 rejected; 1.482 ms |

The final synthesis cases complete with verified candidates, but take longer in
these samples because they prove the fallback after the original bounded search
reaches its resource limit. This is added bounded coverage, not a speed claim.

## Arrays, loops, recursion, and release path

| Case | Proof result | Z3 time / calls | WASM bytes | Bounds or inputs |
| --- | --- | ---: | ---: | --- |
| `fixed_array_sum4` | VERIFIED | 0.036 ms / 4 | 272 | Fixed array length 4; one reference/WASM input |
| `bounded_loop_sum4` | VERIFIED | 0.041 ms / 4 | 1,650 | Loop bound 4; one reference/WASM input |
| `bounded_recursion_sum_to` | VERIFIED | 3.466 ms / 5 | 1,457 | Maximum depth 16; inputs 0, 1, and 16 |

These rows verify fixed source implementations and compare generated WASM with the
reference evaluator; they are not synthesis-search timings.

The release fixture ran project check/build and served its generated static files
over local HTTP to the Node DOM-shim runtime for three measured builds:

| Release measurement | Median |
| --- | ---: |
| CLI build | 167.789 ms |
| Runtime boot | 2.894 ms |
| State transition | 0.116 ms |
| External effect adapter | 0.235 ms |
| Persistence restore | 0.040 ms |
| Static artifacts | 44,029 bytes |
| WASM | 3,181 bytes |

The release fixture reports four proof-artifact contracts and checks arrays,
bounded loops, bounded recursion, modular calls, an unverified extern adapter,
state transition, persistence, and rendered state. Its measured effect adapter
run uses Node through local HTTP.

A separate manual browser smoke loaded the generated static bundle from local
HTTP in the Codex in-app Chromium browser. The page loaded its JS and WASM with
HTTP 200 responses; clicking `increment` changed the visible value from 0 to 1,
and reloading restored 1 from persisted state. The static server returned 404
for the development-only `/__synthetix/version` poll every 500 ms; the runtime
ignores non-OK responses and the app remained usable. This is browser execution
evidence, but it is not an automated browser regression test, and the timed
effect adapter measurement above remains a Node DOM-shim run.

## Static bundle reproducibility

The release CLI was built from the clean snapshot's empty Cargo target. The
same `axiom build` command and fixture were run from two separately copied,
clean temporary project directories on macOS arm64; all six generated files
matched byte-for-byte. These are artifact hashes from that run, not a claim that
the native CLI binary or VSIX is bit-for-bit reproducible across toolchains.

| Artifact | SHA-256 |
| --- | --- |
| `index.html` | `491d6ac4428d4bb9ca1fdaf09feca66f27fafb272f466dee2d48e3acf435adc2` |
| `app.config.js` | `a4ddef50d58e7c9a86bce785d8579c573d6eb84464513c0063d695162f31561e` |
| `app.js` | `24df3c4118c1e6c998d1d86c125f686939f6db8858064b9b49280bc624e432d9` |
| `app.css` | `b7f634dd0f4ff14dae26fc91160096a845a67397a23870d15de9f7662483e23a` |
| `app.wasm` | `30db9c09a7a6b82b7d7e81240226adf66a2d1468d057f50890cdc2ed1f669f93` |
| `app.proof.json` | `e42009757311fdfd5089f4f77797eea30c17d9830a0a9b41402f2e06a3163f48` |

## Incremental proof-cache measurements

Each value is the median of three runs of
`cargo test -p axiom-cli proof_cache_ -- --nocapture`. These debug-profile timers
include project loading and verification/cache lookup, and exclude fixture
creation and source edits.

| Scenario | Contracts/modules | Median | Hits / misses / invalidations |
| --- | ---: | ---: | ---: |
| Five-contract cold | 5 | 1,401.386 ms | 0 / 5 / 0 |
| Five-contract warm | 5 | 0.625 ms | 5 / 0 / 0 |
| Change leaf `C` | 5 | 1,362.314 ms | 1 / 4 / 4 |
| Change root | 5 | 341.026 ms | 4 / 1 / 1 |
| 32-module cold | 32 | 486.586 ms | 0 / 32 / 0 |
| 32-module warm | 32 | 3.772 ms | 32 / 0 / 0 |

The same tests assert that changing an unrelated module keeps the chain cached,
semantics-version and synthesis-option changes invalidate affected keys, and a
failed proof is never stored as VERIFIED. The unrelated-change scenario is not
timed. These single-host observations are not performance guarantees.

## Scope limits

Verified dynamic collections remain deferred: current arrays and bounded loops
cover the measured fixed-bound aggregation and search cases, while no workload
requires variable-length verified output. Application `List` stays outside the
verified array model. A non-browser WASI/Node/Edge deployment target also remains
deferred because no product requirement was established.
