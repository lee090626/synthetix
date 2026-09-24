#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

cargo build --release -p axiom-cli
cargo run --release -p axiom-cli --example v6_benchmark -- \
  --json docs/v6-benchmark-baseline.json \
  --markdown docs/v6-benchmark.md
