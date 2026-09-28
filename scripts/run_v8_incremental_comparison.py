#!/usr/bin/env python3
"""Compare frozen v7.9 dev rebuilds with the current incremental project engine."""

from __future__ import annotations

import argparse
import hashlib
import importlib.util
import json
import os
import re
import subprocess
import tempfile
from datetime import datetime, timezone
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
BASELINE = ROOT / "scripts/run_v8_project_graph_baseline.py"
FIXTURES = ROOT / "benchmarks/v8/project-graph-fixtures.json"
V79_REPORT = ROOT / "docs/v7.9-benchmark.json"
SOURCE_FILES = [ROOT / "crates/axiom-project/src/lib.rs", ROOT / "crates/axiom-cli/src/web.rs"]

spec = importlib.util.spec_from_file_location("v8_baseline", BASELINE)
baseline = importlib.util.module_from_spec(spec)
assert spec.loader is not None
spec.loader.exec_module(baseline)


def sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def git(*args: str, cwd: Path = ROOT) -> str:
    return subprocess.check_output(["git", "-C", str(cwd), *args], text=True).strip()


def source_diff_sha256() -> str:
    diff = subprocess.check_output(
        ["git", "-C", str(ROOT), "diff", "--binary", "--", *[str(p.relative_to(ROOT)) for p in SOURCE_FILES]]
    )
    return hashlib.sha256(diff).hexdigest()


def cache_counts(row: dict) -> dict:
    if row["post_edit_output_class"] == "NO_REBUILD_EXPECTED":
        return {"available": True, "rebuild": False}
    project = re.search(
        r"project cache: (\d+) parsed, (\d+) reused; (\d+) contract\(s\) relinked, (\d+) reused",
        row["edit_log"],
    )
    proof = re.search(
        r"proof cache: (\d+) hit\(s\), (\d+) miss\(es\), (\d+) invalidation\(s\), (\d+)us",
        row["edit_log"],
    )
    return {
        "available": project is not None and proof is not None,
        "modules_parsed": int(project[1]) if project else None,
        "modules_reused": int(project[2]) if project else None,
        "contracts_relinked": int(project[3]) if project else None,
        "contracts_reused": int(project[4]) if project else None,
        "proof_hits": int(proof[1]) if proof else None,
        "proof_misses": int(proof[2]) if proof else None,
        "proof_invalidations": int(proof[3]) if proof else None,
        "proof_elapsed_us": int(proof[4]) if proof else None,
    }


def affected_modules(graph: dict, edit: str) -> list[str]:
    nodes = graph["nodes"]
    if edit in {"no_op", "manifest"}:
        return []
    if edit in {"import", "root"}:
        return ["entry"]

    if edit == "unrelated" and "independent" in nodes:
        changed = "independent"
    else:
        changed = next((name for name, deps in nodes.items() if not deps), next(iter(nodes)))
    reverse = {name: set() for name in nodes}
    for caller, dependencies in nodes.items():
        for dependency in dependencies:
            reverse[dependency].add(caller)

    closure = {changed}
    pending = [changed]
    while pending:
        for caller in reverse[pending.pop()]:
            if caller not in closure:
                closure.add(caller)
                pending.append(caller)
    if closure.intersection(graph["roots"]):
        closure.add("entry")
    return sorted(closure)


def linked_contract_count(graph: dict, modules: list[str]) -> int:
    return sum(module in graph["nodes"] for module in modules) + (
        len(graph["roots"]) if "entry" in modules else 0
    )


def run_case(label: str, cli: Path, graph_name: str, graph: dict, edit: str, repeat: int) -> dict:
    with tempfile.TemporaryDirectory(prefix=f"synthetix-v8-{label}-") as temporary:
        project = Path(temporary) / "project"
        config, paths = baseline.materialize(project, graph)
        ids = baseline.contract_ids(project, graph, paths)
        row = baseline.run_dev_edit(cli, config, edit, graph_name, graph, paths, ids)
        errors: list[str] = []
        baseline.validate_run(f"{label}/{graph_name}/{edit}/{repeat}", row, errors, no_op=edit == "no_op")
        if errors:
            row["gate_errors"] = errors
        row["cache_counts"] = cache_counts(row)
        row["repeat"] = repeat
        return row


def summary(rows: list[dict], key: str) -> dict:
    return baseline.summarize([row.get(key) for row in rows])


def timing_delta_percent(before: dict, after: dict) -> float | None:
    old = before.get("median_ns")
    new = after.get("median_ns")
    if old in (None, 0) or new is None:
        return None
    return round((new - old) * 100 / old, 2)


def pair_case(graph_name: str, graph: dict, edit: str, repeat: int, baseline_cli: Path, current_cli: Path) -> dict:
    old = run_case("v79", baseline_cli, graph_name, graph, edit, repeat)
    new = run_case("incremental", current_cli, graph_name, graph, edit, repeat)
    old_ids = sorted(item.rsplit("::", 1)[-1] for item in old["observed_recomputed_contract_ids"])
    new_ids = sorted(item.rsplit("::", 1)[-1] for item in new["observed_recomputed_contract_ids"])
    old["observed_recomputed_contract_names"] = old_ids
    new["observed_recomputed_contract_names"] = new_ids
    old["baseline_full_reload_counts"] = {
        "modules_parsed_per_rebuild": old["module_count"],
        "contracts_relinked_per_rebuild": old["contract_count"],
        "basis": "frozen v7.9 axiom dev calls stateless axiom_project::load on each rebuild",
    }
    errors = old.get("gate_errors", []) + new.get("gate_errors", [])
    counts = new["cache_counts"]
    affected_module_names = affected_modules(graph, edit)
    if edit != "no_op":
        if not counts["available"]:
            errors.append("incremental rebuild did not report loader and proof cache counters")
        else:
            parsed_modules = 0 if edit == "manifest" else 1
            expected_relinked = linked_contract_count(graph, affected_module_names)
            expected_proofs = len(set(new["expected_affected_contract_ids"]))
            total_contracts = new["contract_count"]
            expected_counts = {
                "modules_parsed": parsed_modules,
                "modules_reused": new["module_count"] - parsed_modules,
                "contracts_relinked": expected_relinked,
                "contracts_reused": total_contracts - expected_relinked,
                "proof_misses": expected_proofs,
                "proof_invalidations": expected_proofs,
            }
            new["expected_project_cache_counts"] = expected_counts
            for name, expected in expected_counts.items():
                if counts.get(name) != expected:
                    errors.append(f"incremental {name} mismatch: {counts.get(name)} != {expected}")
    new["affected_module_names_fixture_derived"] = affected_module_names
    if old["artifact_sha256"] != new["artifact_sha256"]:
        errors.append("v7.9 and incremental generated artifact SHA-256 values differ")
    if old_ids != new_ids:
        errors.append(f"recomputed contract names differ: {old_ids} != {new_ids}")
    return {
        "graph_shape": graph_name,
        "edit_class": edit,
        "repeat": repeat,
        "baseline_v7_9": old,
        "incremental": new,
        "artifact_hashes_match": old["artifact_sha256"] == new["artifact_sha256"],
        "recomputed_contract_names_match": old_ids == new_ids,
        "gate_errors": errors,
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--baseline-cli", required=True, type=Path)
    parser.add_argument("--baseline-source", required=True, type=Path)
    parser.add_argument("--current-cli", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    baseline_cli = args.baseline_cli.resolve()
    current_cli = args.current_cli.resolve()
    baseline_source = args.baseline_source.resolve()
    for label, path in [("baseline", baseline_cli), ("current", current_cli)]:
        if not path.is_file() or not os.access(path, os.X_OK):
            parser.error(f"--{label}-cli must be executable: {path}")

    reference = json.loads(V79_REPORT.read_text(encoding="utf-8"))
    baseline_head = git("rev-parse", "HEAD", cwd=baseline_source)
    baseline_dirty = git("status", "--short", cwd=baseline_source)
    expected_head = reference["hosted_gate"]["source_commit"]
    expected_cli = reference["local_checks"]["same_toolchain_double_clean_release_cli_sha256"]
    expected_cli_sha = expected_cli["first"]
    if baseline_dirty or baseline_head != expected_head or expected_cli["status"] != "PASS":
        parser.error("baseline source must be the clean v7.9 frozen source commit")
    if expected_cli["first"] != expected_cli["second"] or sha256(baseline_cli) != expected_cli_sha:
        parser.error("baseline CLI does not match the v7.9 double-clean binary SHA")
    if baseline_cli == current_cli:
        parser.error("baseline and current CLI paths must differ")

    fixtures = json.loads(FIXTURES.read_text(encoding="utf-8"))
    pairs = []
    errors = []
    repeats = baseline.REPEATS
    for graph_name, graph in fixtures["graphs"].items():
        for repeat in range(1, repeats + 1):
            pair = pair_case(graph_name, graph, "manifest", repeat, baseline_cli, current_cli)
            pair["case_kind"] = "warm_process_cache_rebuild"
            pairs.append(pair)
            errors.extend(pair["gate_errors"])
    edit_graph = fixtures["graphs"]["shared_callee"]
    for edit in fixtures["edit_classes"]:
        for repeat in range(1, repeats + 1):
            pair = pair_case("shared_callee", edit_graph, edit, repeat, baseline_cli, current_cli)
            pair["case_kind"] = "edit_class"
            pairs.append(pair)
            errors.extend(pair["gate_errors"])

    groups = []
    for graph_name in fixtures["graphs"]:
        rows = [row for row in pairs if row["graph_shape"] == graph_name and row["case_kind"] == "warm_process_cache_rebuild"]
        groups.append({
            "case_kind": "warm_process_cache_rebuild",
            "graph_shape": graph_name,
            "baseline_edit_loop": baseline.summarize([r["baseline_v7_9"]["edit_loop_ns"] for r in rows]),
            "incremental_edit_loop": baseline.summarize([r["incremental"]["edit_loop_ns"] for r in rows]),
            "edit_loop_change_percent": timing_delta_percent(
                baseline.summarize([r["baseline_v7_9"]["edit_loop_ns"] for r in rows]),
                baseline.summarize([r["incremental"]["edit_loop_ns"] for r in rows]),
            ),
        })
    for edit in fixtures["edit_classes"]:
        rows = [row for row in pairs if row["graph_shape"] == "shared_callee" and row["edit_class"] == edit and row["case_kind"] == "edit_class"]
        groups.append({
            "case_kind": "edit_class",
            "graph_shape": "shared_callee",
            "edit_class": edit,
            "baseline_edit_loop": baseline.summarize([r["baseline_v7_9"]["edit_loop_ns"] for r in rows]),
            "incremental_edit_loop": baseline.summarize([r["incremental"]["edit_loop_ns"] for r in rows]),
            "baseline_startup": baseline.summarize([r["baseline_v7_9"]["startup_ns"] for r in rows]),
            "incremental_startup": baseline.summarize([r["incremental"]["startup_ns"] for r in rows]),
            "edit_loop_change_percent": timing_delta_percent(
                baseline.summarize([r["baseline_v7_9"]["edit_loop_ns"] for r in rows]),
                baseline.summarize([r["incremental"]["edit_loop_ns"] for r in rows]),
            ),
            "startup_change_percent": timing_delta_percent(
                baseline.summarize([r["baseline_v7_9"]["startup_ns"] for r in rows]),
                baseline.summarize([r["incremental"]["startup_ns"] for r in rows]),
            ),
        })

    payload = {
        "schema_version": 1,
        "milestone": "v8.3",
        "status": "PASS" if not errors else "FAIL",
        "generated_at_utc": datetime.now(timezone.utc).isoformat(),
        "provenance": {
            "baseline_source_head": baseline_head,
            "baseline_source_clean": not bool(baseline_dirty),
            "baseline_source_commit": expected_head,
            "baseline_cli_sha256": sha256(baseline_cli),
            "v7_9_report_sha256": sha256(V79_REPORT),
            "current_source_head": git("rev-parse", "HEAD"),
            "current_source_branch": git("branch", "--show-current"),
            "current_source_dirty": git("status", "--short"),
            "current_source_files_sha256": {str(path.relative_to(ROOT)): sha256(path) for path in SOURCE_FILES},
            "current_source_diff_sha256": source_diff_sha256(),
            "current_cli_sha256": sha256(current_cli),
            "current_cli_path": str(current_cli),
            "harness_sha256": sha256(Path(__file__).resolve()),
            "baseline_harness_sha256": sha256(BASELINE),
            "fixture_sha256": sha256(FIXTURES),
            "toolchain": subprocess.run(["rustc", "--version"], text=True, capture_output=True).stdout.strip(),
            "host": subprocess.run(["rustc", "-vV"], text=True, capture_output=True).stdout.splitlines(),
            "profile": "release",
            "repeats": repeats,
        },
        "baseline_model": "Each v7.9 dev rebuild uses stateless axiom_project::load, so every reachable module is parsed and every linked contract is relinked; proof cache hits remain reported separately.",
        "measurement_notes": {
            "edit_loop": "File edit to dev version endpoint change; includes 300 ms watcher polling and load/link/proof/build.",
            "no_op": "No mutation; version stability and observation window are reported, with no edit-loop time.",
            "project_counts": "Current counts come from retained-loader runtime logs. Baseline full parse/link counts are derived from the frozen v7.9 call path and fixture graph size.",
            "incremental_count_granularity": "Project parse/link counters are module-level: a changed module relinks all contracts declared in each affected module, including multiple entry wrappers. Proof misses and invalidations are checked separately against the exact affected ContractId set.",
            "artifact_comparison": "All generated dev artifact file SHA-256 values must match across baseline and incremental output for each paired case.",
            "performance_claim": "Paired repeated timings are reported; no improvement is claimed unless the end-to-end measurements support it.",
        },
        "comparisons": pairs,
        "summaries": groups,
        "errors": errors,
    }
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(payload, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    print(f"wrote {args.output}; status={payload['status']}; errors={len(errors)}")
    return 0 if payload["status"] == "PASS" else 1


if __name__ == "__main__":
    raise SystemExit(main())
