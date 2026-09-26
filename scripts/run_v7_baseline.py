#!/usr/bin/env python3
"""Capture a clean v7.0 baseline without changing the benchmark source."""

import argparse
import hashlib
import json
import os
import platform
import statistics
import subprocess
import sys
import tempfile
import time
from datetime import datetime, timezone
from pathlib import Path

FREEZE_SHA = "2e355cb91c872963048fe6a6dd167be75568e469"
ROOT = Path(__file__).resolve().parents[1]
MANIFEST = ROOT / "docs/v6-benchmark-final.json"
REPEATED_FIELDS = ("name", "category", "source", "search_bounds", "result", "failure_classification", "resource_status")


def command(args, cwd, env=None, timed=False):
    start = time.perf_counter()
    result = subprocess.run(args, cwd=cwd, env=env, text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
    elapsed = (time.perf_counter() - start) * 1000
    value = {"command": args, "exit_code": result.returncode, "duration_ms": round(elapsed, 3)}
    if not timed:
        value["output"] = result.stdout[-4000:]
    return value


def git(source, *args):
    result = subprocess.run(["git", *args], cwd=source, text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE, check=True)
    return result.stdout.strip()


def run_trial(source, index, target, output_dir):
    env = os.environ.copy()
    env["CARGO_TARGET_DIR"] = str(target)
    commands = []
    for args in (["cargo", "build", "--workspace"], ["cargo", "test", "--workspace"], ["cargo", "build", "--release", "-p", "axiom-cli"]):
        result = command(args, source, env)
        commands.append(result)
        if result["exit_code"]:
            raise RuntimeError(f"trial {index}: {args} failed ({result['exit_code']})\n{result['output']}")

    json_path, md_path = output_dir / f"trial-{index}.json", output_dir / f"trial-{index}.md"
    harness = command(["cargo", "run", "--release", "-p", "axiom-cli", "--example", "v6_benchmark", "--", "--json", str(json_path), "--markdown", str(md_path), "--title", f"v7.0 baseline trial {index}"], source, env)
    if harness["exit_code"]:
        raise RuntimeError(f"trial {index}: benchmark failed\n{harness['output']}")
    if not json_path.exists():
        raise RuntimeError(f"trial {index}: harness did not produce {json_path}")

    cli = []
    binary = target / "release/axiom"
    for attempt in range(10):
        result = command([str(binary), "check", "examples/basics/mul8.ax"], source, env)
        cli.append({"attempt": attempt + 1, "exit_code": result["exit_code"], "duration_ms": result["duration_ms"]})
        if result["exit_code"]:
            raise RuntimeError(f"trial {index}: direct CLI check failed: {result}")
    durations = [entry["duration_ms"] for entry in cli]
    return {
        "index": index,
        "target_state": "fresh" if index == 1 else "warm",
        "cargo_commands": commands,
        "harness_command": {k: v for k, v in harness.items() if k != "output"},
        "scenario_report": "temporary harness JSON consumed for the canonical data and not retained",
        "cli_command": [str(binary), "check", "examples/basics/mul8.ax"],
        "cli_measurement": "process-start plus contract-check latency; not pure startup",
        "cli_invocations": cli,
        "cli_summary_ms": {"median": round(statistics.median(durations), 3), "min": min(durations), "max": max(durations)},
        "measurements": json.loads(json_path.read_text())["measurements"],
    }


def compare(manifest, first, second):
    expected = manifest["measurements"]
    results = []
    for label, rows in (("trial-1 vs manifest", first), ("trial-2 vs manifest", second), ("trial-1 vs trial-2", second)):
        left = expected if label.endswith("manifest") else first
        by_name_left, by_name_right = {}, {}
        for row in left:
            by_name_left.setdefault(row["name"], []).append(row)
        for row in rows:
            by_name_right.setdefault(row["name"], []).append(row)
        missing, extra = [], []
        mismatches = []
        for name in sorted(by_name_left.keys() | by_name_right.keys()):
            left_rows, right_rows = by_name_left.get(name, []), by_name_right.get(name, [])
            missing.extend([name] * max(0, len(left_rows) - len(right_rows)))
            extra.extend([name] * max(0, len(right_rows) - len(left_rows)))
            for occurrence, (left_row, right_row) in enumerate(zip(left_rows, right_rows), 1):
                for field in REPEATED_FIELDS:
                    if left_row.get(field) != right_row.get(field):
                        mismatches.append({"name": name, "occurrence": occurrence, "field": field, "expected": left_row.get(field), "actual": right_row.get(field)})
        result = {"comparison": label, "pass": not missing and not extra and not mismatches, "missing": missing, "extra": extra, "mismatches": mismatches}
        results.append(result)
        if not result["pass"]:
            raise RuntimeError(f"scenario comparison failed: {json.dumps(result)}")
    if len(expected) != 54 or any("NO_CANDIDATE" == row["result"] for row in first + second):
        raise RuntimeError("expected exactly 54 cases and NO_CANDIDATE to remain unobserved")
    return results


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--source-root", type=Path, required=True)
    args = parser.parse_args()
    source = args.source_root.resolve()
    if git(source, "rev-parse", "HEAD") != FREEZE_SHA:
        raise RuntimeError("source root is not the exact v6.9 freeze commit")
    if git(source, "status", "--porcelain"):
        raise RuntimeError("source root is dirty")
    manifest = json.loads(MANIFEST.read_text())
    initial_hashes = {"manifest_sha256": hashlib.sha256(MANIFEST.read_bytes()).hexdigest()}
    with tempfile.TemporaryDirectory(prefix="synthetix-v7-target-") as target_temp, tempfile.TemporaryDirectory(prefix="synthetix-v7-baseline-") as temp:
        target = Path(target_temp)
        output_dir = Path(temp)
        trials = [run_trial(source, 1, target, output_dir), run_trial(source, 2, target, output_dir)]
        comparisons = compare(manifest, trials[0]["measurements"], trials[1]["measurements"])
        source_status = git(source, "status", "--porcelain")
        if source_status:
            raise RuntimeError(f"source worktree became dirty: {source_status}")
        baseline = {
            "format_version": 1,
            "generated_at": datetime.now(timezone.utc).isoformat(),
            "objective": "v7.0 Clean Performance and Architecture Baseline",
            "environment": {
                "source_commit": FREEZE_SHA,
                "source_worktree_status": "clean before and after trials (build output ignored)",
                "operating_system": platform.platform(),
                "architecture": platform.machine(),
                "python": sys.version.split()[0],
                "rustc": subprocess.check_output(["rustc", "--version"], text=True).strip(),
                "cargo": subprocess.check_output(["cargo", "--version"], text=True).strip(),
                "node": subprocess.run(["node", "--version"], text=True, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL).stdout.strip() or None,
                "historical_manifest_source": "docs/v6-benchmark-final.json (scenario names/bounds/outcomes only)",
                "historical_v6_timing_note": "The historical v6 benchmark was generated from a dirty tree; its timing values were not used as v7 evidence.",
            },
            "scope_notes": [
                "NO_CANDIDATE was not observed in this 54-case corpus.",
                "Hosted CI wall-clock duration, actual RSS, and allocator-level allocations are not measured.",
                "Timing observations are host-dependent measurements, not guarantees.",
                "Harness WASM/runtime rows use the existing Node DOM shim and are not browser QA.",
            ],
            "benchmark_profiles": {
                "harness": "release",
                "harness_repeats_per_scenario": 3,
                "proof_cache_cases": "debug cargo test profile",
                "generated_static_runtime_rows": "single run",
                "baseline_trials": 2,
            },
            "default_search_bounds": manifest["default_search_bounds"],
            "scenario_count": 54,
            "comparison_results": comparisons,
            "trials": trials,
            "source_status_after": source_status,
            "reference_manifest_sha256": initial_hashes["manifest_sha256"],
        }
    json_path, md_path = ROOT / "docs/v7-benchmark-baseline.json", ROOT / "docs/v7-benchmark-baseline.md"
    json_path.write_text(json.dumps(baseline, indent=2) + "\n")
    rows = trials[0]["measurements"]
    lines = [
        "# v7.0 Clean Performance and Architecture Baseline", "",
        "Measurements are observations, not performance guarantees.", "",
        "## Environment", "",
        f"- Source: `{FREEZE_SHA}` (clean before and after trials)",
        f"- Platform: `{baseline['environment']['operating_system']}` / `{baseline['environment']['architecture']}`",
        f"- Rust: `{baseline['environment']['rustc']}`; Cargo: `{baseline['environment']['cargo']}`; Node: `{baseline['environment']['node'] or 'not installed'}`",
        "- Historical v6 timing values were not reused: the v6 report was generated from a dirty tree.",
        "- `NO_CANDIDATE` was not observed among the 54 scenarios.",
        "- Hosted CI wall-clock duration, actual RSS, and allocator-level allocations are not measured.",
        "- Runtime/WASM rows use the existing Node DOM shim; this is not browser QA.", "",
        "## Harness profiles", "",
        "- Scenario harness: release profile, 3 repeats per scenario.",
        "- Proof-cache cases: debug `cargo test` profile.",
        "- Generated static/runtime rows: single run per trial.",
        "- Baseline: 2 trials (fresh target, then warm target).", "",
        "## Trial command timings", "",
    ]
    for trial in trials:
        lines += [f"### Trial {trial['index']} ({trial['target_state']} target)", "", "| Command | Exit | Duration (ms) |", "|---|---:|---:|"]
        for item in trial["cargo_commands"]:
            lines.append(f"| `{' '.join(item['command'])}` | {item['exit_code']} | {item['duration_ms']:.3f} |")
        lines.append(f"| `{ ' '.join(trial['cli_command']) }` × 10 (process start + contract check) | all 0 | median {trial['cli_summary_ms']['median']:.3f}; range {trial['cli_summary_ms']['min']:.3f}–{trial['cli_summary_ms']['max']:.3f} |")
        lines.append("")
    lines += ["## Scenario comparison", "", "All 54 scenario names, bounds, results, failure classifications, and resource statuses matched the manifest and both trials.", "", "## Scenario measurements", "", "Per-case timing and counts are recorded in the JSON. This table shows stable identity, bounds, and outcome.", "", "| Name | Category | Source | Bounds | Result | Failure | Resource |", "|---|---|---|---|---|---|---|"]
    for row in rows:
        bounds = json.dumps(row["search_bounds"], separators=(",", ":"))
        lines.append(f"| {row['name']} | {row['category']} | `{row['source']}` | `{bounds}` | {row['result']} | {row['failure_classification'] or '—'} | {row['resource_status'] or '—'} |")
    md_path.write_text("\n".join(lines) + "\n")
    print(f"wrote {json_path}")
    print(f"wrote {md_path}")


if __name__ == "__main__":
    try:
        main()
    except (OSError, subprocess.SubprocessError, RuntimeError, KeyError, ValueError) as error:
        print(f"v7.0 baseline failed: {error}", file=sys.stderr)
        sys.exit(1)
