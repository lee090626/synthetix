# v4 Synthesis Performance Baseline

Run `cargo run --release -p axiom-synth --example search_baseline` from the
repository root to reproduce the JSON measurements in
[`v4-performance-baseline.json`](v4-performance-baseline.json). Parsing time and
release compilation are excluded; each `elapsed_ms` measures one synthesis run.

The recorded run uses release defaults: 50,000 candidates, 64 MiB, and no
timeout. Candidate counts and elapsed times are a baseline for comparison, not a
performance guarantee. Re-record the JSON on the target host when comparing a
change.
