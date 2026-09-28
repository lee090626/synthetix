#!/usr/bin/env python3
"""Capture the v8.0 project graph/edit baseline using a supplied frozen CLI."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import queue
import socket
import subprocess
import tempfile
import threading
import time
import urllib.request
from datetime import datetime, timezone
from pathlib import Path


REPEATS = 5
BOUNDS = {"timeout_ms": None, "max_candidates": 50_000, "max_memory_bytes": 67_108_864}


def sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def source_hashes(root: Path) -> dict[str, str]:
    return {
        str(path.relative_to(root)): sha256(path)
        for path in sorted(root.rglob("*"))
        if path.is_file() and ".synthetix" not in path.parts and "dist" not in path.parts
    }


def source_for(name: str, dependencies: list[str]) -> str:
    imports = "".join(
        f'import {{ {dependency} }} from "./{dependency}.ax";\n'
        for dependency in dependencies
    )
    requires = "".join(f"  requires {dependency}(x) == x;\n" for dependency in dependencies)
    return (
        imports
        + f"export contract {name}(x: u32) -> u32 {{\n"
        + requires
        + "  ensures result == x;\n}\n"
    )


def materialize(root: Path, graph: dict) -> tuple[Path, dict[str, str]]:
    root.mkdir(parents=True, exist_ok=True)
    nodes = graph["nodes"]
    source_paths: dict[str, str] = {}
    for name, dependencies in nodes.items():
        source_paths[name] = f"{name}.ax"
        contents = source_for(name, dependencies)
        (root / source_paths[name]).write_text(contents, encoding="utf-8")
    # One project module points at all graph roots; root identities stay explicit.
    root_imports = [name for name in graph["roots"]]
    imports = "".join(f'import {{ {name} }} from "./{name}.ax";\n' for name in root_imports)
    contracts = "".join(
        f"export contract project_root_{name}(x: u32) -> u32 {{\n"
        f"  requires {name}(x) == x;\n"
        "  ensures result == x;\n}\n"
        for name in root_imports
    )
    (root / "entry.ax").write_text(imports + contracts, encoding="utf-8")
    config = root / "synthetix.toml"
    config.write_text('[project]\nentry = "entry.ax"\ntarget = "web"\n', encoding="utf-8")
    return config, source_paths


def contract_ids(root: Path, graph: dict, paths: dict[str, str]) -> dict[str, str]:
    ids = {name: f"{(root / path).resolve()}::{name}" for name, path in paths.items()}
    ids.update(
        {
            f"project_root_{name}": f"{(root / 'entry.ax').resolve()}::project_root_{name}"
            for name in graph["roots"]
        }
    )
    return ids


def affected(graph: dict, edit: str) -> list[str]:
    nodes = graph["nodes"]
    if edit == "no_op":
        return []
    if edit == "manifest":
        return []
    if edit == "import":
        # The entry's import/dependency identity changes; existing graph nodes remain stable.
        return [f"project_root_{name}" for name in graph["roots"]]
    if edit == "root":
        return [f"project_root_{name}" for name in graph["roots"]]
    changed = "independent" if edit == "unrelated" else next(
        (name for name, dependencies in nodes.items() if not dependencies), None
    )
    if changed is None:
        changed = next(iter(nodes))
    reverse: dict[str, set[str]] = {name: set() for name in nodes}
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
    closure.update(f"project_root_{name}" for name in graph["roots"] if name in closure)
    return sorted(closure)


def free_port() -> int:
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


def http_text(url: str) -> str:
    with urllib.request.urlopen(url, timeout=1) as response:
        return response.read().decode("utf-8")


def read_output(stream, lines: queue.Queue[str]) -> None:
    for line in iter(stream.readline, ""):
        lines.put(line.rstrip("\n"))


def await_line(lines: queue.Queue[str], marker: str, timeout: float) -> str:
    deadline = time.monotonic() + timeout
    collected = []
    while time.monotonic() < deadline:
        try:
            line = lines.get(timeout=min(0.2, max(0.01, deadline - time.monotonic())))
        except queue.Empty:
            continue
        collected.append(line)
        if marker in line:
            return "\n".join(collected)
    raise TimeoutError(f"timed out waiting for {marker!r}; output={collected!r}")


def run_dev_edit(
    cli: Path,
    config: Path,
    edit: str,
    graph_name: str,
    graph: dict,
    paths: dict[str, str],
    ids: dict[str, str],
) -> dict:
    case_metadata = {
        "graph_shape": graph_name,
        "node_count": len(graph["nodes"]),
        "module_count": len(graph["nodes"]) + 1,
        "edge_count": sum(map(len, graph["nodes"].values())) + len(graph["roots"]),
        "contract_count": len(ids),
    }
    port = free_port()
    output_lines: queue.Queue[str] = queue.Queue()
    started = time.perf_counter_ns()
    process = subprocess.Popen(
        [str(cli), "dev", "--port", str(port), "--no-open"],
        cwd=config.parent,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
        bufsize=1,
    )
    reader = threading.Thread(target=read_output, args=(process.stdout, output_lines), daemon=True)
    reader.start()
    try:
        initial_log = await_line(output_lines, "watching:", 120)
        initial_output_class = classify(initial_log)
        startup_ns = time.perf_counter_ns() - started
        try:
            version = int(http_text(f"http://127.0.0.1:{port}/__synthetix/version"))
        except Exception as error:
            raise RuntimeError(f"dev server version endpoint unavailable: {error}; {initial_log}") from error
        affected_ids = affected(graph, edit)
        initial_version = version
        initial_observed = []
        for line in initial_log.splitlines():
            if line.startswith("verified: ") and "(cache hit)" not in line:
                name = line.removeprefix("verified: ").strip()
                if name in ids:
                    initial_observed.append(ids[name])
        target = config.parent / paths.get("root", "entry.ax")
        edit_start = time.perf_counter_ns()
        observation_window_ns = None
        if edit == "no_op":
            time.sleep(0.4)
            observation_window_ns = time.perf_counter_ns() - edit_start
            observed_version = int(http_text(f"http://127.0.0.1:{port}/__synthetix/version"))
            if observed_version != version:
                raise RuntimeError(
                    f"no-op unexpectedly rebuilt project: version {version} -> {observed_version}"
                )
            edit_elapsed_ns = None
            version = observed_version
            observed = []
            log = ""
        else:
            if edit == "unrelated":
                target = config.parent / "independent.ax"
                prior = target.read_bytes()
                target.write_bytes(prior + b"// independent branch edit\n")
            elif edit == "manifest":
                config.write_text('[project]\nentry = "entry.ax"\ntarget = "web"\nstorage_schema_version = 2\n', encoding="utf-8")
            elif edit == "import":
                entry = config.parent / "entry.ax"
                lines = entry.read_text(encoding="utf-8").splitlines(keepends=True)
                import_lines = [line for line in lines if line.startswith("import ")]
                body = [line for line in lines if not line.startswith("import ")]
                entry.write_text("".join(list(reversed(import_lines)) + body), encoding="utf-8")
                target = entry
            else:
                if edit == "leaf":
                    target = config.parent / next(
                        path for name, path in paths.items()
                        if not graph["nodes"][name]
                    )
                elif edit == "root":
                    target = config.parent / "entry.ax"
                prior = target.read_bytes() if target.exists() else b""
                target.write_bytes(prior + b"// v8 baseline edit\n")
            deadline = time.monotonic() + 90
            while time.monotonic() < deadline:
                try:
                    next_version = int(http_text(f"http://127.0.0.1:{port}/__synthetix/version"))
                except Exception:
                    time.sleep(0.05)
                    continue
                if next_version > version:
                    version = next_version
                    break
                time.sleep(0.05)
            else:
                raise TimeoutError(f"edit {edit} did not trigger project rebuild")
            edit_elapsed_ns = time.perf_counter_ns() - edit_start
            log = await_line(output_lines, "rebuilt successfully", 10)
            log += "\n" + await_line(output_lines, "proof cache:", 5)
            observed = []
            for line in log.splitlines():
                if line.startswith("verified: ") and "(cache hit)" not in line:
                    name = line.removeprefix("verified: ").strip()
                    candidates = [value for key, value in ids.items() if key == name]
                    observed.extend(candidates)
        return {
            **case_metadata,
            "startup_ns": startup_ns,
            "edit_loop_ns": edit_elapsed_ns,
            "observation_window_ns": observation_window_ns,
            "version_before": initial_version,
            "version_after": version,
            "initial_recomputed_contract_ids": sorted(set(initial_observed)),
            "expected_affected_contract_ids": sorted(ids.get(name, name) for name in affected_ids),
            "observed_recomputed_contract_ids": sorted(set(observed)),
            "observed_cache_lines": [line for line in log.splitlines() if "proof cache:" in line],
            "artifact_sha256": {
                path.name: sha256(path)
                for path in (config.parent / ".synthetix/dev").glob("*")
                if path.is_file()
            },
            "fixture_sha256": source_hashes(config.parent),
            "initial_output_class": initial_output_class,
            "post_edit_output_class": "NO_REBUILD_EXPECTED" if edit == "no_op" else classify(log),
            "diagnostics": [],
            "failure": None,
            "edit_log": log,
        }
    except Exception as error:
        return {
            **case_metadata,
            "startup_ns": None,
            "edit_loop_ns": None,
            "observation_window_ns": None,
            "version_before": None,
            "version_after": None,
            "initial_recomputed_contract_ids": [],
            "expected_affected_contract_ids": [],
            "observed_recomputed_contract_ids": [],
            "observed_cache_lines": [],
            "artifact_sha256": {},
            "fixture_sha256": source_hashes(config.parent),
            "initial_output_class": "UNOBSERVED",
            "post_edit_output_class": "UNOBSERVED",
            "diagnostics": [],
            "failure": str(error),
            "edit_log": "",
        }
    finally:
        process.terminate()
        try:
            process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait()


def classify(log: str) -> str:
    if "proof_result: VERIFIED" in log:
        return "FOUND/VERIFIED"
    if "build error:" in log or "error:" in log:
        return "FAIL"
    return "UNOBSERVED"


def summarize(values: list[int | None]) -> dict:
    clean = sorted(value for value in values if value is not None)
    if not clean:
        return {"median_ns": None, "range_ns": None}
    middle = len(clean) // 2
    median = clean[middle] if len(clean) % 2 else (clean[middle - 1] + clean[middle]) // 2
    return {"median_ns": median, "range_ns": [clean[0], clean[-1]]}


def validate_run(label: str, row: dict, errors: list[str], *, no_op: bool = False) -> None:
    expected = set(row.get("expected_affected_contract_ids", row.get("expected_contract_ids", [])))
    observed = set(row.get("observed_recomputed_contract_ids", []))
    output_class = row.get("post_edit_output_class") if "post_edit_output_class" in row else row.get("output_class")
    if row.get("failure"):
        errors.append(f"{label}: {row['failure']}")
    if no_op:
        if output_class != "NO_REBUILD_EXPECTED":
            errors.append(f"{label}: expected NO_REBUILD_EXPECTED, got {output_class}")
        if row.get("edit_loop_ns") is not None:
            errors.append(f"{label}: no-op must not report edit-loop runtime")
        if row.get("version_before") != row.get("version_after"):
            errors.append(f"{label}: no-op version changed {row.get('version_before')} -> {row.get('version_after')}")
        if row.get("observation_window_ns") is None:
            errors.append(f"{label}: no-op observation window is missing")
    elif output_class != "FOUND/VERIFIED":
        errors.append(f"{label}: expected FOUND/VERIFIED, got {output_class}")
    if expected != observed:
        errors.append(f"{label}: affected ContractId mismatch expected={sorted(expected)} observed={sorted(observed)}")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--cli", required=True, type=Path, help="prebuilt axiom CLI from the clean v7.9 source")
    parser.add_argument("--source", required=True, type=Path, help="source checkout used to identify the baseline commit")
    parser.add_argument("--output", required=True, type=Path, help="raw JSON output path")
    parser.add_argument(
        "--reference-report",
        type=Path,
        default=Path(__file__).resolve().parents[1] / "docs/v7.9-benchmark.json",
        help="frozen v7.9 provenance report",
    )
    parser.add_argument("--fixtures", type=Path, default=Path(__file__).resolve().parents[1] / "benchmarks/v8/project-graph-fixtures.json")
    args = parser.parse_args()
    cli = args.cli.resolve()
    source = args.source.resolve()
    fixture_file = args.fixtures.resolve()
    reference_report = args.reference_report.resolve()
    if not cli.is_file() or not os.access(cli, os.X_OK):
        parser.error(f"--cli must be an executable file: {cli}")
    fixtures = json.loads(fixture_file.read_text(encoding="utf-8"))
    head = subprocess.check_output(["git", "-C", str(source), "rev-parse", "HEAD"], text=True).strip()
    dirty = subprocess.check_output(["git", "-C", str(source), "status", "--short"], text=True).strip()
    if dirty:
        parser.error(f"--source checkout must be clean; found:\n{dirty}")
    try:
        reference = json.loads(reference_report.read_text(encoding="utf-8"))
        expected_source = reference["hosted_gate"]["source_commit"]
        clean_release = reference["local_checks"]["same_toolchain_double_clean_release_cli_sha256"]
        expected_cli_first = clean_release["first"]
        expected_cli_second = clean_release["second"]
        report_cli = sha256(cli)
        if clean_release["status"] != "PASS":
            raise ValueError("v7.9 double-clean CLI evidence status is not PASS")
        if head != expected_source:
            raise ValueError(f"source HEAD mismatch: {head} != {expected_source}")
        if expected_cli_first != expected_cli_second or report_cli != expected_cli_first:
            raise ValueError(
                "CLI SHA mismatch: "
                f"actual={report_cli}, first={expected_cli_first}, second={expected_cli_second}"
            )
    except (OSError, KeyError, TypeError, ValueError, json.JSONDecodeError) as error:
        parser.error(f"cannot establish v7.9 baseline provenance: {error}")
    shapes = []
    edits = []
    errors = []
    with tempfile.TemporaryDirectory(prefix="synthetix-v8-baseline-") as temp_name:
        temp = Path(temp_name)
        for graph_name, graph in fixtures["graphs"].items():
            repeat_rows = []
            for repeat in range(REPEATS):
                project_root = temp / f"{graph_name}-{repeat}"
                config, paths = materialize(project_root, graph)
                ids = contract_ids(project_root, graph, paths)
                started = time.perf_counter_ns()
                result = subprocess.run([str(cli), "build"], cwd=project_root, text=True, capture_output=True)
                elapsed = time.perf_counter_ns() - started
                log = result.stdout + result.stderr
                output_class = "FOUND/VERIFIED" if result.returncode == 0 and "proof_result: VERIFIED" in log else "FAIL"
                verified = []
                for line in log.splitlines():
                    if line.startswith("verified: ") and "(cache hit)" not in line:
                        name = line.removeprefix("verified: ").strip()
                        if name in ids:
                            verified.append(ids[name])
                row = {
                    "repeat": repeat + 1,
                    "elapsed_ns": elapsed,
                    "exit_code": result.returncode,
                    "output_class": output_class,
                    "diagnostics": [line for line in log.splitlines() if line.startswith(("error:", "build error:"))],
                    "observed_recomputed_contract_ids": sorted(verified),
                    "expected_contract_ids": sorted(ids.values()),
                    "node_count": len(graph["nodes"]),
                    "module_count": len(graph["nodes"]) + 1,
                    "contract_count": len(ids),
                    "edge_count": sum(map(len, graph["nodes"].values())) + len(graph["roots"]),
                    "cache_counters": {
                        "available": False,
                        "hits": None,
                        "misses": None,
                        "invalidations": None,
                        "reason": "axiom build does not expose proof-cache counters",
                    },
                    "artifact_sha256": {
                        path.name: sha256(path)
                        for path in (project_root / "dist").glob("*")
                        if path.is_file()
                    },
                    "fixture_sha256": source_hashes(project_root),
                    "stdout": result.stdout,
                    "stderr": result.stderr,
                }
                repeat_rows.append(row)
                validate_run(f"{graph_name} cold repeat {repeat + 1}", row, errors)
                warm = run_dev_edit(cli, config, "manifest", graph_name, graph, paths, ids)
                row["warm_process_cache_rebuild"] = warm
                validate_run(f"{graph_name} warm repeat {repeat + 1}", warm, errors)
            shapes.append(
                {
                    "graph_shape": graph_name,
                    "repeats": repeat_rows,
                    "elapsed_summary": summarize([row["elapsed_ns"] for row in repeat_rows]),
                    "warm_process_cache_rebuild_summary": summarize(
                        [row["warm_process_cache_rebuild"]["edit_loop_ns"] for row in repeat_rows]
                    ),
                }
            )
        edit_graph = fixtures["graphs"]["shared_callee"]
        for edit in fixtures["edit_classes"]:
            repeat_rows = []
            for repeat in range(REPEATS):
                project_root = temp / f"edit-{edit}-{repeat}"
                config, paths = materialize(project_root, edit_graph)
                ids = contract_ids(project_root, edit_graph, paths)
                row = run_dev_edit(cli, config, edit, "shared_callee", edit_graph, paths, ids)
                row["repeat"] = repeat + 1
                repeat_rows.append(row)
                validate_run(
                    f"edit {edit} repeat {repeat + 1}",
                    row,
                    errors,
                    no_op=edit == "no_op",
                )
            edits.append(
                {
                    "edit_class": edit,
                    "repeats": repeat_rows,
                    "edit_loop_summary": summarize([row["edit_loop_ns"] for row in repeat_rows]),
                    "observation_window_summary": summarize([row["observation_window_ns"] for row in repeat_rows]),
                    "expected_affected_contract_ids": repeat_rows[0]["expected_affected_contract_ids"],
                    "observed_recomputed_contract_ids": [row["observed_recomputed_contract_ids"] for row in repeat_rows],
                }
            )
    payload = {
        "schema_version": 1,
        "generated_at_utc": datetime.now(timezone.utc).isoformat(),
        "baseline": {
            "source_head": head,
            "source_branch": subprocess.check_output(["git", "-C", str(source), "branch", "--show-current"], text=True).strip(),
            "source_dirty_status": dirty,
            "v7_9_reference_report_path": str(reference_report),
            "v7_9_reference_report_sha256": sha256(reference_report),
            "v7_9_source_commit": expected_source,
            "v7_9_source_commit_matched": head == expected_source,
            "v7_9_release_cli_sha256_first": expected_cli_first,
            "v7_9_release_cli_sha256_second": expected_cli_second,
            "v7_9_cli_sha256_matched": report_cli == expected_cli_first == expected_cli_second,
            "cli_path": str(cli),
            "cli_sha256": sha256(cli),
            "toolchain": subprocess.run(["rustc", "--version"], text=True, capture_output=True).stdout.strip(),
            "host": subprocess.run(["rustc", "-vV"], text=True, capture_output=True).stdout.splitlines(),
            "profile": "release" if "release" in cli.parts else "prebuilt CLI profile not inferred",
            "search_bounds": BOUNDS,
            "fixture_file": str(fixture_file),
            "fixture_sha256": sha256(fixture_file),
            "harness_sha256": sha256(Path(__file__).resolve()),
            "repeats": REPEATS,
        },
        "measurement_notes": {
            "project_load_link_proof_build": "Combined CLI `axiom build` wall interval; production CLI does not expose exact component timings.",
            "edit_loop": "Existing `axiom dev` rebuild interval from edit write until version endpoint advances; includes watcher polling (300 ms), load/link/proof/build.",
            "warm_process_cache_rebuild": "For each graph shape, start a dev process (cold initial build) then make a storage_schema_version-only manifest edit; record the subsequent watcher+full reload/proof/build interval while source proof cache remains warm.",
            "no_op": "No source mutation is issued; no rebuild is expected. Record the observation window separately; edit_loop_ns is null.",
            "unrelated": "Edits the active `independent` contract branch and its entry wrapper; the shared-callee branch is expected to remain a cache hit.",
            "observed_recomputed_contract_ids": "Derived only from `verified: NAME` lines without `(cache hit)`; source paths are paired from unique fixture contract names. A failed build exposes no complete exact set.",
            "expected_affected_contract_ids": "Fixture graph transitive caller closure; each completed run is checked against the observed recomputed IDs.",
            "performance_claim": "No speedup claim; all timings are baseline-only.",
        },
        "graph_shapes": shapes,
        "edit_classes": edits,
        "errors": errors,
    }
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(payload, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    print(f"wrote {args.output}; errors={len(errors)}; harness_sha256={payload['baseline']['harness_sha256']}")
    return 1 if errors else 0


if __name__ == "__main__":
    raise SystemExit(main())
