#!/usr/bin/env python3
"""Capture isolated macOS RSS runs of the prebuilt v7.5 measurement example."""

import argparse
import hashlib
import json
import os
import platform
import re
import subprocess
import sys
from datetime import datetime, timezone
from pathlib import Path

SCENARIOS = {
    "cegis_hidden_nine": ["benchmarks/v6/synthesis/cegis_hidden_nine.ax"],
    "fixed_array_sum4": ["benchmarks/v6/final/array_sum4.ax"],
    "bounded_loop_sum4": ["benchmarks/v6/final/bounded_loop_sum4.ax"],
    "bounded_recursion_sum_to": ["benchmarks/v6/final/recursive_sum_to.ax"],
}
EXPECTED = {
    "cegis_hidden_nine": {"scenario": "cegis_hidden_nine", "status": "RESOURCE_LIMIT",
                          "classification": "explicit_search_resource_limit", "timeout_ms": None,
                          "max_candidates": 50000, "estimated_memory_limit_bytes": 268435456},
    "fixed_array_sum4": {"scenario": "fixed_array_sum4", "status": "VERIFIED",
                          "classification": "verified_feature_proof", "timeout_ms": None,
                          "max_candidates": None, "estimated_memory_limit_bytes": None},
    "bounded_loop_sum4": {"scenario": "bounded_loop_sum4", "status": "VERIFIED",
                          "classification": "verified_feature_proof", "timeout_ms": None,
                          "max_candidates": None, "estimated_memory_limit_bytes": None},
    "bounded_recursion_sum_to": {"scenario": "bounded_recursion_sum_to", "status": "VERIFIED",
                                 "classification": "verified_feature_proof", "timeout_ms": None,
                                 "max_candidates": None, "estimated_memory_limit_bytes": None},
}
TIME = "/usr/bin/time"


def run(root: Path, executable: Path, repeats: int) -> dict:
    if platform.system() != "Darwin":
        raise RuntimeError("/usr/bin/time -l RSS capture requires macOS")
    executable = executable.resolve(strict=True)
    expected_directory = (root / "target" / "release" / "examples").resolve()
    if executable.parent != expected_directory or not executable.is_file() or not os.access(executable, os.X_OK):
        raise RuntimeError(f"executable must be a direct executable binary under {expected_directory}: {executable}")
    def git(*args: str) -> str:
        return subprocess.run(["git", *args], cwd=root, check=True, text=True, capture_output=True).stdout

    status = git("status", "--porcelain=v1", "--untracked-files=all")
    snapshot_hash = hashlib.sha256()
    snapshot_hash.update(status.encode())
    for relative in sorted(path for path in git("ls-files", "-co", "--exclude-standard", "-z").split("\0") if path):
        path = root / relative
        if path.is_file():
            snapshot_hash.update(relative.encode() + b"\0")
            snapshot_hash.update(path.read_bytes())
    snapshot = snapshot_hash.hexdigest()
    fixtures = {name: {rel: hashlib.sha256((root / rel).read_bytes()).hexdigest() for rel in paths}
                for name, paths in SCENARIOS.items()}
    executable_sha256 = hashlib.sha256(executable.read_bytes()).hexdigest()
    version = subprocess.run(["rustc", "--version"], cwd=root, check=True, text=True, capture_output=True).stdout.strip()
    cargo_version = subprocess.run(["cargo", "--version"], cwd=root, check=True, text=True, capture_output=True).stdout.strip()
    runs = []
    failures = []
    for scenario in SCENARIOS:
        for repeat in range(1, repeats + 1):
            command = [TIME, "-l", str(executable.resolve()), scenario]
            proc = subprocess.run(command, cwd=root, text=True, capture_output=True)
            rss_matches = re.findall(r"([0-9]+)\s+maximum resident set size", proc.stderr)
            try:
                report = json.loads(proc.stdout.strip())
            except json.JSONDecodeError:
                report = None
            runs.append({"scenario": scenario, "repeat": repeat, "command": command,
                         "exit_status": proc.returncode, "stdout": proc.stdout,
                         "stderr_and_time_output": proc.stderr,
                         "peak_rss_bytes": int(rss_matches[-1]) if rss_matches else None,
                         "structured_report": report})
            if proc.returncode != 0 or not rss_matches or report is None:
                failures.append(f"{scenario}#{repeat}: exit={proc.returncode}, rss={bool(rss_matches)}, json={report is not None}")
            else:
                differences = {
                    key: {"expected": value, "actual": report.get(key)}
                    for key, value in EXPECTED[scenario].items()
                    if report.get(key) != value
                }
                if differences:
                    failures.append(f"{scenario}#{repeat}: report fields differ: {differences}")
    return {"format_version": 1, "captured_at_utc": datetime.now(timezone.utc).isoformat(),
            "host": {"os": platform.platform(), "architecture": platform.machine()},
            "toolchain": {"rustc": version, "cargo": cargo_version, "profile": "release"},
            "executable": str(executable.resolve()), "executable_sha256": executable_sha256,
            "expected_outcomes": EXPECTED,
            "source_snapshot_sha256": snapshot,
            "git_head": git("rev-parse", "HEAD").strip(), "working_tree_status": status.splitlines(),
            "repeat_count_per_scenario": repeats,
            "bounds": {"cegis_hidden_nine": {"timeout": None, "max_candidates": 50000,
                     "max_estimated_memory_bytes": 268435456},
                     "rss_is_observed_process_maximum_and_has_no_configured_cap": True},
            "fixture_sha256": fixtures, "runs": runs, "run_errors": failures}


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, default=Path(__file__).resolve().parents[1])
    parser.add_argument("--executable", type=Path, required=True, help="direct prebuilt release example executable")
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--repeats", type=int, default=5)
    args = parser.parse_args()
    if args.repeats < 1:
        parser.error("--repeats must be at least 1")
    try:
        result = run(args.root.resolve(), args.executable, args.repeats)
    except (OSError, RuntimeError, subprocess.CalledProcessError) as error:
        print(f"memory capture failed: {error}", file=sys.stderr)
        return 1
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(result, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    print(f"wrote {args.output} ({len(result['runs'])} isolated runs)")
    if result["run_errors"]:
        print("memory capture had failed runs: " + "; ".join(result["run_errors"]), file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
