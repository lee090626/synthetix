#!/usr/bin/env python3
"""Capture v8.8 DAG scale, mutation, and resource evidence on macOS."""

from __future__ import annotations

import argparse
import copy
import hashlib
import json
import os
import platform
import queue
import re
import shutil
import signal
import socket
import subprocess
import tempfile
import threading
import time
import urllib.request
from datetime import datetime, timezone
from pathlib import Path

from v8_dag_scale_fixtures import (
    MUTATIONS, SHAPES, SIZES, affected_ids, contract_ids, graph_for,
    materialize, record as graph_record, source_hashes,
)

ROOT = Path(__file__).resolve().parents[1]
TIME = Path("/usr/bin/time")
MAX_SECONDS = 30
MAX_RSS_BYTES = 268_435_456
WATCHER_SETTLE_SECONDS = 0.4
BOUNDS = {"max_candidates": 50_000, "max_memory_bytes": 67_108_864}
REPEATS = 5


def sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def write_json_atomic(path: Path, payload: dict) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_name(f".{path.name}.{os.getpid()}.tmp")
    temporary.write_text(json.dumps(payload, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    os.replace(temporary, path)


def artifact_hashes(project: Path) -> dict[str, str]:
    output = project / ".synthetix" / "dev"
    return {
        path.name: sha256(path) for path in sorted(output.glob("*")) if path.is_file()
    } if output.is_dir() else {}


def rss_bytes(text: str) -> int | None:
    found = re.search(r"(\d+)\s+maximum resident set size", text)
    return int(found.group(1)) if found else None


def classify(text: str, code: int | None) -> str:
    if "proof_result: VERIFIED" in text:
        return "FOUND/VERIFIED"
    if code == 0:
        return "SUCCESS_UNCLASSIFIED"
    return "LOAD_LINK_FAIL" if any(
        token in text for token in (
            "imported file not found:", "circular import:", "duplicate imported name:",
            "Expected token", "cannot read",
        )
    ) else "BUILD_FAIL"


def diagnostic(text: str, project: Path) -> tuple[str | None, str | None]:
    lines = [line.strip() for line in text.splitlines() if "error:" in line or "build error:" in line]
    if not lines:
        return None, None
    line = lines[-1]
    for root in sorted({str(project), str(project.resolve())}, key=len, reverse=True):
        line = line.replace(root, "<PROJECT>")
    line = re.sub(r"^(?:error|build error):\s*", "", line)
    match = re.search(r":(\d+):(\d+):", line)
    if match:
        location = f"{match.group(1)}:{match.group(2)}"
    else:
        line_only = re.search(r"\.ax:(\d+):", line)
        location = line_only.group(1) if line_only else None
    return line, location


def contract_names(text: str, *, recomputed_only: bool = False) -> list[str]:
    result = []
    for line in text.splitlines():
        match = re.fullmatch(r"verified: ([A-Za-z_]\w*)( \(cache hit\))?", line.strip())
        if match and (not recomputed_only or not match.group(2)):
            result.append(match.group(1))
    return result


def cold_run(cli: Path, project: Path) -> dict:
    start = time.perf_counter_ns()
    process = subprocess.Popen(
        [str(TIME), "-l", str(cli), "dev", "--once"],
        cwd=project, text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
        start_new_session=True,
    )
    try:
        output, _ = process.communicate(timeout=MAX_SECONDS)
        elapsed = time.perf_counter_ns() - start
        diag, location = diagnostic(output, project)
        return {
            "process_kind": "cold_cli",
            "outcome": classify(output, process.returncode), "exit_code": process.returncode,
            "diagnostic": diag, "source_location": location,
            "contract_order": contract_names(output),
            "recomputed_contract_names": contract_names(output, recomputed_only=True),
            "artifact_sha256": artifact_hashes(project), "wall_ns": elapsed,
            "peak_rss_bytes": rss_bytes(output), "log": output,
            "failure": None,
        }
    except subprocess.TimeoutExpired:
        try:
            os.killpg(process.pid, signal.SIGTERM)
        except ProcessLookupError:
            pass
        try:
            output, _ = process.communicate(timeout=2)
        except subprocess.TimeoutExpired:
            try:
                os.killpg(process.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            output, _ = process.communicate()
        return {
            "process_kind": "cold_cli",
            "outcome": "CASE_DEADLINE", "exit_code": None, "diagnostic": None,
            "source_location": None, "contract_order": [], "recomputed_contract_names": [],
            "artifact_sha256": artifact_hashes(project),
            "wall_ns": time.perf_counter_ns() - start, "peak_rss_bytes": rss_bytes(output),
            "log": output, "failure": f"cold run exceeded {MAX_SECONDS}s",
        }


def port_available() -> int:
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        return int(sock.getsockname()[1])


def get_http(port: int, route: str) -> str:
    with urllib.request.urlopen(f"http://127.0.0.1:{port}/{route}", timeout=0.5) as response:
        return response.read().decode("utf-8")


def read_output(stream, lines: queue.Queue) -> None:
    for line in iter(stream.readline, ""):
        lines.put(line.rstrip("\n"))
    lines.put(None)


def await_marker(lines: queue.Queue, marker: str, deadline: float, gathered: list[str]) -> bool:
    while time.monotonic() < deadline:
        try:
            line = lines.get(timeout=min(0.2, max(0.01, deadline - time.monotonic())))
        except queue.Empty:
            continue
        if line is None:
            return False
        gathered.append(line)
        if marker in line:
            return True
    return False


def start_dev(cli: Path, project: Path, port: int):
    process = subprocess.Popen(
        [str(TIME), "-l", str(cli), "dev", "--port", str(port), "--no-open"],
        cwd=project, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True,
        bufsize=1, start_new_session=True,
    )
    lines: queue.Queue = queue.Queue()
    thread = threading.Thread(target=read_output, args=(process.stdout, lines), daemon=True)
    thread.start()
    return process, lines, thread


def stop_dev(process: subprocess.Popen, lines: queue.Queue) -> str:
    try:
        children = subprocess.check_output(["pgrep", "-P", str(process.pid)], text=True, timeout=1)
        child_pid = int(children.splitlines()[0])
        try:
            os.kill(child_pid, signal.SIGTERM)
        except ProcessLookupError:
            pass
    except (OSError, subprocess.SubprocessError, ValueError, IndexError):
        try:
            os.killpg(process.pid, signal.SIGTERM)
        except ProcessLookupError:
            pass
    try:
        process.wait(timeout=5)
    except subprocess.TimeoutExpired:
        try:
            os.killpg(process.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        process.wait()
    output = []
    deadline = time.monotonic() + 2
    while time.monotonic() < deadline:
        try:
            line = lines.get(timeout=0.1)
        except queue.Empty:
            break
        if line is None:
            break
        if line is not None:
            output.append(line)
    return "\n".join(output)


def edit_result_decision(old_version: int, version: int, error: str, ignored_error: str | None, fresh_error_event: bool):
    if version > old_version:
        return "SUCCESS"
    if fresh_error_event or (error and error != ignored_error):
        return "ERROR"
    return None


def edit_result_decision_self_test() -> None:
    assert edit_result_decision(3, 4, "stale", "stale", False) == "SUCCESS"
    assert edit_result_decision(3, 3, "stale", "stale", False) is None
    assert edit_result_decision(3, 3, "stale", "stale", True) == "ERROR"
    assert edit_result_decision(3, 3, "", None, True) == "ERROR"
    assert edit_result_decision(3, 3, "new", "stale", False) == "ERROR"


def wait_edit_result(
    port: int,
    old_version: int,
    lines: queue.Queue,
    started_ns: int,
    *,
    ignore_error: str | None = None,
):
    gathered = []
    deadline = time.monotonic() + MAX_SECONDS
    while time.monotonic() < deadline:
        try:
            version = int(get_http(port, "__synthetix/version"))
            error = get_http(port, "__synthetix/error")
        except Exception:
            time.sleep(0.05)
            continue
        decision = edit_result_decision(old_version, version, error, ignore_error, False)
        if decision == "SUCCESS":
            await_marker(lines, "proof cache:", min(deadline, time.monotonic() + 3), gathered)
            return "SUCCESS", version, time.perf_counter_ns() - started_ns, gathered
        if decision == "ERROR":
            await_marker(lines, "build error:", min(deadline, time.monotonic() + 1), gathered)
            return "ERROR", version, time.perf_counter_ns() - started_ns, gathered
        try:
            line = lines.get(timeout=0.05)
        except queue.Empty:
            continue
        if line is None:
            break
        gathered.append(line)
        if "build error:" in line and edit_result_decision(
            old_version, version, error, ignore_error, True
        ) == "ERROR":
            return "ERROR", version, time.perf_counter_ns() - started_ns, gathered
    return "CASE_DEADLINE", old_version, time.perf_counter_ns() - started_ns, gathered


def counters(text: str) -> dict:
    loader = re.search(
        r"project cache: (\d+) parsed, (\d+) reused; (\d+) contract\(s\) relinked, (\d+) reused", text
    )
    proof = re.search(
        r"proof cache: (\d+) hit\(s\), (\d+) miss\(es\), (\d+) invalidation\(s\), (\d+)us", text
    )
    return {
        "modules_parsed": int(loader[1]) if loader else None,
        "modules_reused": int(loader[2]) if loader else None,
        "contracts_relinked": int(loader[3]) if loader else None,
        "contracts_reused": int(loader[4]) if loader else None,
        "proof_hits": int(proof[1]) if proof else None,
        "proof_misses": int(proof[2]) if proof else None,
        "proof_invalidations": int(proof[3]) if proof else None,
        "proof_elapsed_us": int(proof[4]) if proof else None,
    }


def copy_source(source: Path, destination: Path) -> None:
    shutil.copytree(source, destination, ignore=shutil.ignore_patterns(".synthetix", "dist"))


def mutate(project: Path, graph: dict, paths: dict[str, Path], edit: str):
    entry = project / "entry.ax"
    original_entry = entry.read_text(encoding="utf-8")
    graph.setdefault("_entry_wrapper_order", graph["roots"].copy())
    if edit in ("no_op", "manifest_only"):
        if edit == "manifest_only":
            (project / "synthetix.toml").write_text(
                '[project]\nentry = "entry.ax"\ntarget = "web"\nstorage_schema_version = 2\n',
                encoding="utf-8",
            )
        return graph, paths, [], lambda: None
    if edit in ("unrelated", "leaf"):
        name = graph["roots"][-1] if edit == "unrelated" else next(
            node for node, deps in graph["nodes"].items()
            if not deps and node != graph["roots"][-1]
        )
        path = paths[name]
        prior = path.read_bytes()
        path.write_bytes(prior + f"// {edit} edit\n".encode())
        return graph, paths, [name], lambda: path.write_bytes(prior)
    if edit == "root":
        entry.write_bytes(entry.read_bytes() + b"// root edit\n")
        seeds = [f"entry_{root}" for root in graph["roots"]]
        return graph, paths, seeds, lambda: entry.write_text(original_entry, encoding="utf-8")
    if edit == "import_reorder":
        lines = original_entry.splitlines(keepends=True)
        imports = [line for line in lines if line.startswith("import ")]
        body = [line for line in lines if not line.startswith("import ")]
        entry.write_text("".join(list(reversed(imports)) + body), encoding="utf-8")
        graph["roots"].reverse()
        def restore_order():
            graph["roots"].reverse()
            entry.write_text(original_entry, encoding="utf-8")
        seeds = [f"entry_{root}" for root in graph["roots"]]
        return graph, paths, seeds, restore_order
    if edit == "import_add":
        candidate = next(node for node in graph["nodes"] if node not in graph["roots"])
        graph["roots"].append(candidate)
        graph["_entry_wrapper_order"].append(candidate)
        lines = original_entry.splitlines(keepends=True)
        imports = [line for line in lines if line.startswith("import ")]
        body = [line for line in lines if not line.startswith("import ")]
        new_import = f'import {{ {candidate} }} from "./{candidate}.ax";\n'
        new_wrapper = (
            f"export contract entry_{candidate}(x: u32) -> u32 {{\n"
            f"  requires {candidate}(x) == x;\n  ensures result == x;\n}}\n"
        )
        entry.write_text("".join(imports + [new_import] + body + [new_wrapper]), encoding="utf-8")
        def restore_add():
            graph["roots"].remove(candidate)
            graph["_entry_wrapper_order"].remove(candidate)
            entry.write_text(original_entry, encoding="utf-8")
        seeds = [f"entry_{root}" for root in graph["roots"]]
        return graph, paths, seeds, restore_add
    if edit in ("import_remove", "module_delete", "module_rename"):
        independent = graph["roots"][-1]
        if edit == "module_rename":
            old = paths[independent]
            renamed = project / f"{independent}_renamed.ax"
            old.rename(renamed)
            paths[independent] = renamed
            entry.write_text(original_entry.replace(
                f"./{independent}.ax", f"./{independent}_renamed.ax"
            ), encoding="utf-8")
            def restore_rename():
                entry.write_text(original_entry, encoding="utf-8")
                renamed.rename(old)
                paths[independent] = old
            seeds = [independent, *[f"entry_{root}" for root in graph["roots"]]]
            return graph, paths, seeds, restore_rename
        root_index = graph["roots"].index(independent)
        graph["roots"].remove(independent)
        graph["_entry_wrapper_order"].remove(independent)
        deps = graph["nodes"].pop(independent)
        path = paths.pop(independent)
        wrapper = re.compile(
            rf"(?ms)^export contract entry_{re.escape(independent)}\(.*?^\}}\n?"
        )
        without_wrapper, wrapper_count = wrapper.subn("", original_entry, count=1)
        import_line = f'import {{ {independent} }} from "./{independent}.ax";\n'
        if wrapper_count != 1 or import_line not in without_wrapper:
            raise RuntimeError(f"cannot remove root import and wrapper for {independent}")
        entry.write_text(without_wrapper.replace(import_line, "", 1), encoding="utf-8")
        if edit == "module_delete":
            path.unlink()
        def restore_removed():
            graph["roots"].insert(root_index, independent)
            graph["_entry_wrapper_order"].insert(root_index, independent)
            graph["nodes"][independent] = deps
            paths[independent] = path
            entry.write_text(original_entry, encoding="utf-8")
        seeds = [f"entry_{root}" for root in graph["roots"]]
        return graph, paths, seeds, restore_removed
    if edit == "valid_invalid_recovered":
        name = next(
            node for node, deps in graph["nodes"].items()
            if deps and node not in graph["roots"]
        )
        path = paths[name]
        original = path.read_bytes()
        dependency = graph["nodes"][name][0]
        source = original.decode("utf-8")
        expected_import = f'from "./{dependency}.ax"'
        if expected_import not in source:
            raise RuntimeError(f"cannot find import edge {name} -> {dependency}")
        path.write_text(source.replace(expected_import, 'from "./missing.ax"', 1), encoding="utf-8")
        return graph, paths, [name], lambda: path.write_bytes(original)
    raise ValueError(f"unknown mutation: {edit}")


def copy_cold_project(project: Path, suffix: str) -> Path:
    cold = project.parent / suffix
    if cold.exists():
        shutil.rmtree(cold)
    copy_source(project, cold)
    return cold


def expected_order(graph: dict) -> list[str]:
    result, seen = [], set()

    def visit(module: str) -> None:
        if module in seen:
            return
        seen.add(module)
        for dependency in graph["nodes"][module]:
            visit(dependency)
        result.append(module)

    for root in sorted(graph["roots"]):
        visit(root)
        result.append(f"entry_{root}")
    return result


def expected_order_self_test() -> None:
    graph = graph_for("layered_shared_path", 8)
    original = expected_order(graph)
    graph["roots"].reverse()
    assert expected_order(graph) == original, "import order must not reorder verification"
    graph["roots"].append("n002")
    added = expected_order(graph)
    wrapper_order = [name for name in added if name.startswith("entry_")]
    assert wrapper_order == sorted(wrapper_order), "entry wrappers must stay name-sorted"
    assert "entry_n002" in added, "added root wrapper must be included"


def diagnostic_pair_matches(left: dict, right: dict) -> bool:
    fields = ("diagnostic", "source_location")
    return all(
        field in left and field in right and left[field] == right[field]
        for field in fields
    ) and left.get("diagnostic") is not None and right.get("diagnostic") is not None


def diagnostic_pair_self_test() -> None:
    expected = {"diagnostic": "duplicate import", "source_location": "1:2"}
    assert diagnostic_pair_matches(expected, expected)
    assert not diagnostic_pair_matches(expected, {**expected, "source_location": "2:1"})
    assert not diagnostic_pair_matches(expected, {**expected, "diagnostic": "other"})
    assert diagnostic_pair_matches(
        {"diagnostic": "circular import: cycle", "source_location": None},
        {"diagnostic": "circular import: cycle", "source_location": None},
    )
    assert not diagnostic_pair_matches(expected, {"diagnostic": None, "source_location": "1:2"})
    assert not diagnostic_pair_matches(expected, {"diagnostic": "duplicate import"})


def edit_case(cli: Path, project: Path, graph: dict, paths: dict[str, Path], edit: str) -> dict:
    before_graph = copy.deepcopy(graph)
    initial_hashes = {}
    port = port_available()
    process_started = time.perf_counter_ns()
    process, lines, reader_thread = start_dev(cli, project, port)
    started = time.perf_counter_ns()
    initial_lines: list[str] = []
    row = None
    recovered_cold_run = None
    try:
        if not await_marker(lines, "watching:", time.monotonic() + MAX_SECONDS, initial_lines):
            raise TimeoutError("initial dev build did not reach watching marker")
        startup_ns = time.perf_counter_ns() - started
        # The CLI prints this marker before starting its watcher thread.
        time.sleep(WATCHER_SETTLE_SECONDS)
        initial_hashes = artifact_hashes(project)
        if not initial_hashes:
            raise RuntimeError("initial dev build produced no hashed artifacts")
        initial_text = "\n".join(initial_lines)
        full_text = initial_text
        version_before = int(get_http(port, "__synthetix/version"))
        if edit == "no_op":
            observation_start = time.perf_counter_ns()
            time.sleep(0.4)
            version_after = int(get_http(port, "__synthetix/version"))
            after_hashes = artifact_hashes(project)
            observation_ns = time.perf_counter_ns() - observation_start
            outcome = "NO_REBUILD_EXPECTED" if (
                version_after == version_before and after_hashes == initial_hashes
            ) else "FAIL"
            no_op_total_ns = time.perf_counter_ns() - process_started
            expected, expected_names, edit_ns = [], [], None
            terminal_text, diag, location = initial_text, None, None
            diagnostic_match = outcome == "NO_REBUILD_EXPECTED"
            artifact_match = diagnostic_match
            recovery, recovery_match = None, None
            cold = None
            recompute_match = diagnostic_match
            expected_sequence = expected_order(before_graph)
            observed_names = []
            last_good_preserved = None
        else:
            no_op_total_ns = None
            edit_start = time.perf_counter_ns()
            after_graph, after_paths, seeds, restore = mutate(project, graph, paths, edit)
            terminal, version_after, edit_ns, event_lines = wait_edit_result(
                port, version_before, lines, edit_start
            )
            terminal_text = "\n".join(event_lines)
            full_text = terminal_text
            outcome = (
                "FOUND/VERIFIED" if terminal == "SUCCESS"
                else "LOAD_LINK_FAIL" if terminal == "ERROR" else terminal
            )
            after_ids = contract_ids(project, after_graph, after_paths)
            expected = affected_ids(before_graph, after_graph, after_ids, seeds)
            expected_names = sorted(name for name, cid in after_ids.items() if cid in expected)
            diag, location = diagnostic(terminal_text, project)
            cold_project = copy_cold_project(project, f"cold-{edit}")
            cold = cold_run(cli, cold_project)
            diagnostic_match = (
                diag == cold["diagnostic"]
                and location == cold["source_location"]
                and outcome == cold["outcome"]
            )
            if outcome == "FOUND/VERIFIED":
                artifact_match = artifact_hashes(project) == cold["artifact_sha256"]
                order_match = contract_names(terminal_text) == cold["contract_order"]
            else:
                artifact_match = True
                order_match = True
            expected_sequence = expected_order(after_graph)
            if edit == "valid_invalid_recovered":
                failed_hashes = artifact_hashes(project)
                last_good_preserved = (
                    version_after == version_before and failed_hashes == initial_hashes
                )
                stale_error = get_http(port, "__synthetix/error")
                restore()
                recover_start = time.perf_counter_ns()
                recovered, recovered_version, recover_ns, recover_lines = wait_edit_result(
                    port, version_after, lines, recover_start, ignore_error=stale_error
                )
                recovered_cold = copy_cold_project(project, "cold-recovered")
                clean_recovery = cold_run(cli, recovered_cold)
                recovered_cold_run = clean_recovery
                recovery_hashes = artifact_hashes(project)
                recovery = {
                    "outcome": "FOUND/VERIFIED" if recovered == "SUCCESS" else recovered,
                    "version": recovered_version,
                    "edit_loop_ns": recover_ns,
                    "artifact_sha256": recovery_hashes,
                    "cold_artifact_sha256": clean_recovery["artifact_sha256"],
                    "contract_order": contract_names("\n".join(recover_lines)),
                    "cold_contract_order": clean_recovery["contract_order"],
                    "contract_order_matches_cold": (
                        contract_names("\n".join(recover_lines)) == clean_recovery["contract_order"]
                    ),
                    "matches_cold": recovery_hashes == clean_recovery["artifact_sha256"],
                }
                full_text += "\n" + "\n".join(recover_lines)
                recovery_match = recovery["matches_cold"]
                artifact_match = last_good_preserved
            else:
                recovery, recovery_match = None, None
                last_good_preserved = None
            observed_names = contract_names(terminal_text, recomputed_only=True)
            recompute_match = (
                (outcome == "LOAD_LINK_FAIL" and not observed_names)
                or set(observed_names) == set(expected_names)
            )
            observation_ns = None

        if edit != "no_op":
            observed_names = contract_names(terminal_text, recomputed_only=True)
        active_ids = contract_ids(project, graph, paths)
        observed_ids = sorted(active_ids[name] for name in observed_names if name in active_ids)
        row = {
            "process_kind": "dev_cli",
            "edit_class": edit,
            "outcome": outcome,
            "initial_output_class": classify(initial_text, 0),
            "version_before": version_before,
            "version_after": version_after,
            "startup_ns": startup_ns,
            "edit_loop_ns": edit_ns,
            "observation_window_ns": observation_ns,
            "no_op_total_ns": no_op_total_ns,
            "initial_artifact_sha256": initial_hashes,
            "expected_affected_contract_ids": expected,
            "expected_affected_contract_names": expected_names,
            "observed_recomputed_contract_ids": observed_ids,
            "observed_recomputed_contract_names": observed_names,
            "recompute_set_matches_oracle": recompute_match,
            "expected_contract_order": expected_sequence,
            "incremental_contract_order": contract_names(terminal_text),
            "contract_order_matches_cold": (
                contract_names(terminal_text) == cold["contract_order"] if cold else None
            ),
            "cold_outcome": cold["outcome"] if cold else None,
            "cold_diagnostic": cold["diagnostic"] if cold else None,
            "cold_source_location": cold["source_location"] if cold else None,
            "cold_artifact_sha256": cold["artifact_sha256"] if cold else None,
            "cold_comparison_run": cold,
            "diagnostic": diag,
            "source_location": location,
            "diagnostic_matches_cold": diagnostic_match,
            "artifacts_match_cold_or_last_good": artifact_match,
            "last_good_version_and_artifacts_preserved": last_good_preserved,
            "recovery": recovery,
            "recovery_cold_run": recovered_cold_run,
            "recovery_matches_cold": recovery_match,
            "counters": counters(terminal_text) if edit != "no_op" else {},
            "artifact_sha256": artifact_hashes(project),
            "log": full_text,
            "failure": None,
        }
    except Exception as error:
        row = {
            "edit_class": edit, "outcome": "HARNESS_ERROR", "failure": str(error),
            "startup_ns": None, "edit_loop_ns": None, "observation_window_ns": None,
            "artifact_sha256": artifact_hashes(project), "log": "",
        }
    finally:
        time_text = stop_dev(process, lines)
        reader_thread.join(timeout=1)
        if row is not None:
            row["peak_rss_bytes"] = rss_bytes(time_text)
            row["time_output"] = time_text
    return row


def prepare_pathology(project: Path, kind: str):
    project.mkdir(parents=True, exist_ok=True)
    config = project / "synthetix.toml"
    config.write_text(
        '[project]\nentry = "entry.ax"\ntarget = "web"\nstorage_schema_version = 1\n',
        encoding="utf-8",
    )
    if kind == "cycle":
        names = [f"m{i:03}" for i in range(128)]
        nodes = {
            name: ([names[index + 1]] if index + 1 < len(names) else [])
            for index, name in enumerate(names)
        }
        paths = {name: project / f"{name}.ax" for name in names}
        for index, name in enumerate(names):
            imports = (
                f'import {{ {names[index + 1]} }} from "./{names[index + 1]}.ax";\n'
                if index + 1 < len(names) else ""
            )
            requires = (
                f"  requires {names[index + 1]}(x) == x;\n"
                if index + 1 < len(names) else ""
            )
            paths[name].write_text(
                imports
                + f"export contract {name}(x: u32) -> u32 {{\n{requires}  ensures result == x;\n}}\n",
                encoding="utf-8",
            )
        entry = project / "entry.ax"
        entry.write_text(
            'import { m000 } from "./m000.ax";\n'
            "export contract entry(x: u32) -> u32 {\n"
            "  requires m000(x) == x;\n  ensures result == x;\n}\n",
            encoding="utf-8",
        )
        graph = {"shape": "cycle_ring_128", "size": 128, "nodes": nodes, "roots": ["m000"]}
        path = paths["m127"]
        original = path.read_bytes()
        def apply():
            path.write_bytes(original + b'import { m000 } from "./m000.ax";\n')
            graph["nodes"]["m127"].append("m000")
        def restore():
            path.write_bytes(original)
            graph["nodes"]["m127"].remove("m000")
        return config, graph, paths, apply, restore
    if kind == "duplicate_binding":
        # Keep a valid initial graph; the edit imports the same local name on lines 1-2.
        graph = graph_for("layered_shared_path", 128)
        config, paths = materialize(project, graph)
        entry = project / "entry.ax"
        original = entry.read_text(encoding="utf-8")
        original_modules = {name: paths[name].read_bytes() for name in ("n000", "n001")}
        def apply():
            for name in ("n000", "n001"):
                source = original_modules[name].decode("utf-8")
                paths[name].write_text(
                    source.replace(f"export contract {name}(", "export contract first(", 1),
                    encoding="utf-8",
                )
            lines = original.splitlines(keepends=True)
            lines[0] = 'import { first } from "./n000.ax";\n'
            lines[1] = 'import { first } from "./n001.ax";\n'
            entry.write_text("".join(lines), encoding="utf-8")
        def restore():
            entry.write_text(original, encoding="utf-8")
            for name, source in original_modules.items():
                paths[name].write_bytes(source)
        return config, graph, paths, apply, restore
    raise ValueError(f"unknown pathology: {kind}")


def pathology_fixture_self_test() -> None:
    with tempfile.TemporaryDirectory(prefix="v88-pathology-self-test-") as temporary:
        root = Path(temporary)
        _, graph, paths, apply_cycle, restore_cycle = prepare_pathology(root / "cycle", "cycle")
        assert len(graph["nodes"]) == 128
        assert graph["nodes"]["m000"] == ["m001"]
        assert graph["nodes"]["m126"] == ["m127"]
        assert graph["nodes"]["m127"] == []
        assert 'import { m000 } from "./m000.ax";' in (root / "cycle" / "entry.ax").read_text()
        apply_cycle()
        assert graph["nodes"]["m127"] == ["m000"]
        assert 'import { m000 } from "./m000.ax";' in paths["m127"].read_text()
        restore_cycle()

        _, duplicate_graph, duplicate_paths, apply_duplicate, _ = prepare_pathology(
            root / "duplicate", "duplicate_binding"
        )
        assert len(duplicate_graph["nodes"]) == 128
        reachable, pending = set(), list(duplicate_graph["roots"])
        while pending:
            module = pending.pop()
            if module not in reachable:
                reachable.add(module)
                pending.extend(duplicate_graph["nodes"][module])
        assert len(reachable) == 128, "all duplicate fixture modules must be reachable from entry"
        assert all(f"export contract {name}(" in duplicate_paths[name].read_text() for name in ("n000", "n001"))
        assert all("export contract first(" not in duplicate_paths[name].read_text() for name in ("n000", "n001"))
        apply_duplicate()
        assert all("export contract first(" in duplicate_paths[name].read_text() for name in ("n000", "n001"))
        assert all(f"export contract {name}(" not in duplicate_paths[name].read_text() for name in ("n000", "n001"))
        imports = (root / "duplicate" / "entry.ax").read_text().splitlines()[:2]
        assert imports == [
            'import { first } from "./n000.ax";',
            'import { first } from "./n001.ax";',
        ]


def pathology_case(cli: Path, project: Path, kind: str) -> dict:
    config, graph, paths, apply, restore = prepare_pathology(project, kind)
    initial_cold = cold_run(cli, copy_cold_project(project, f"cold-{kind}-initial"))
    if initial_cold["outcome"] != "FOUND/VERIFIED":
        return {
            "kind": kind,
            "outcome": "HARNESS_ERROR",
            "diagnostic": None,
            "source_location": None,
            "cold_diagnostic": None,
            "cold_source_location": None,
            "failure_matches_cold": False,
            "last_good_version_and_hashes_preserved": False,
            "recovery": {"outcome": "HARNESS_ERROR", "matches_cold": False},
            "initial_cold": initial_cold,
            "failure": f"untouched pathology fixture cold build was {initial_cold['outcome']}",
            "peak_rss_bytes": initial_cold.get("peak_rss_bytes"),
            "failure_log": initial_cold.get("log", ""),
            "recovery_log": "",
            "log": initial_cold.get("log", ""),
        }
    port = port_available()
    process, lines, reader_thread = start_dev(cli, project, port)
    initial_lines: list[str] = []
    started = time.perf_counter_ns()
    row = None
    try:
        if not await_marker(lines, "watching:", time.monotonic() + MAX_SECONDS, initial_lines):
            raise TimeoutError("pathology initial project did not build")
        startup_ns = time.perf_counter_ns() - started
        # Wait for the watcher thread's first snapshot, which follows the marker.
        time.sleep(WATCHER_SETTLE_SECONDS)
        version_before = int(get_http(port, "__synthetix/version"))
        initial_hashes = artifact_hashes(project)
        if not initial_hashes:
            raise RuntimeError("initial dev build produced no hashed artifacts")
        edit_start = time.perf_counter_ns()
        apply()
        terminal, version_after, edit_ns, event_lines = wait_edit_result(
            port, version_before, lines, edit_start
        )
        event_text = "\n".join(event_lines)
        cold_project = copy_cold_project(project, f"cold-{kind}")
        cold = cold_run(cli, cold_project)
        diag, location = diagnostic(event_text, project)
        failed_hashes = artifact_hashes(project)
        failure_match = (
            terminal == "ERROR"
            and cold["outcome"] == "LOAD_LINK_FAIL"
            and diag == cold["diagnostic"]
            and location == cold["source_location"]
            and "proof_result:" not in event_text
            and "proof_result:" not in cold["log"]
        )
        preserved = version_after == version_before and failed_hashes == initial_hashes
        stale_error = get_http(port, "__synthetix/error")
        restore()
        recovery_start = time.perf_counter_ns()
        recovered, recovered_version, recovery_ns, recovery_lines = wait_edit_result(
            port, version_after, lines, recovery_start, ignore_error=stale_error
        )
        recovered_cold_project = copy_cold_project(project, f"cold-{kind}-recovered")
        recovered_cold = cold_run(cli, recovered_cold_project)
        recovered_hashes = artifact_hashes(project)
        row = {
            "process_kind": "dev_cli",
            "kind": kind,
            "initial_cold": initial_cold,
            "failure_cold_run": cold,
            "recovery_cold_run": recovered_cold,
            "outcome": "LOAD_LINK_FAIL" if terminal == "ERROR" else terminal,
            "cold_outcome": cold["outcome"],
            "diagnostic": diag,
            "source_location": location,
            "cold_diagnostic": cold["diagnostic"],
            "cold_source_location": cold["source_location"],
            "failure_matches_cold": failure_match,
            "last_good_version_and_hashes_preserved": preserved,
            "version_before": version_before,
            "version_after_failure": version_after,
            "startup_ns": startup_ns,
            "edit_loop_ns": edit_ns,
            "recovery": {
                "outcome": "FOUND/VERIFIED" if recovered == "SUCCESS" else recovered,
                "version": recovered_version,
                "edit_loop_ns": recovery_ns,
                "artifact_sha256": recovered_hashes,
                "cold_artifact_sha256": recovered_cold["artifact_sha256"],
                "contract_order": contract_names("\n".join(recovery_lines)),
                "cold_contract_order": recovered_cold["contract_order"],
                "contract_order_matches_cold": (
                    contract_names("\n".join(recovery_lines)) == recovered_cold["contract_order"]
                ),
                "matches_cold": recovered_hashes == recovered_cold["artifact_sha256"],
            },
            "failure_log": event_text,
            "recovery_log": "\n".join(recovery_lines),
            "artifact_sha256": failed_hashes,
            "log": event_text + "\n" + "\n".join(recovery_lines),
            "failure": None,
        }
    except Exception as error:
        row = {
            "process_kind": "dev_cli",
            "kind": kind,
            "outcome": "HARNESS_ERROR",
            "diagnostic": None,
            "source_location": None,
            "cold_diagnostic": None,
            "cold_source_location": None,
            "failure_matches_cold": False,
            "last_good_version_and_hashes_preserved": False,
            "recovery": {"outcome": "HARNESS_ERROR", "matches_cold": False},
            "failure": str(error),
            "failure_log": "",
            "recovery_log": "",
            "log": "",
        }
    finally:
        time_text = stop_dev(process, lines)
        reader_thread.join(timeout=1)
        row["peak_rss_bytes"] = rss_bytes(time_text)
        row["time_output"] = time_text
    return row


def add_error(errors: list[str], label: str, row: dict, *, require_found: bool | None = None) -> None:
    if row.get("failure"):
        errors.append(f"{label}: {row['failure']}")
    if row.get("outcome") in ("CASE_DEADLINE", "HARNESS_ERROR", "LOG_INCOMPLETE", "FAIL"):
        errors.append(f"{label}: terminal outcome {row.get('outcome')}")
    for field in ("wall_ns", "edit_loop_ns"):
        value = row.get(field)
        if value is not None and value > MAX_SECONDS * 1_000_000_000:
            errors.append(f"{label}: {field} exceeded {MAX_SECONDS}s")
    rss = row.get("peak_rss_bytes")
    if rss is None:
        errors.append(f"{label}: /usr/bin/time -l RSS was not observed")
    elif rss > MAX_RSS_BYTES:
        errors.append(f"{label}: direct CLI peak RSS {rss} exceeds {MAX_RSS_BYTES}")
    if require_found is True and row.get("outcome") != "FOUND/VERIFIED":
        errors.append(f"{label}: expected FOUND/VERIFIED, got {row.get('outcome')}")
    if require_found is False and row.get("outcome") != "LOAD_LINK_FAIL":
        errors.append(f"{label}: expected LOAD_LINK_FAIL, got {row.get('outcome')}")


def fixture_pair(graph: dict, repeat: int, baseline_cli: Path, current_cli: Path, errors: list[str]) -> dict:
    sides = {}
    for side, cli in (("v7_9_baseline", baseline_cli), ("current", current_cli)):
        with tempfile.TemporaryDirectory(prefix=f"v88-cold-{graph['shape']}-{graph['size']}-{side}-") as temporary:
            project = Path(temporary) / "project"
            materialize(project, graph)
            cold = cold_run(cli, project)
            cold["source_fixture_sha256"] = source_hashes(project)
            cold["graph_order_oracle"] = expected_order(graph)
            cold["graph_order_matches_oracle"] = cold["contract_order"] == expected_order(graph)
            sides[side] = {"cold": cold}
            add_error(errors, f"{graph['shape']}/{graph['size']}/{side}/cold/{repeat}", cold, require_found=True)
            if not cold["graph_order_matches_oracle"]:
                errors.append(f"{graph['shape']}/{graph['size']}/{side}/cold/{repeat}: contract order differs from oracle")
    base_cold, current_cold = sides["v7_9_baseline"]["cold"], sides["current"]["cold"]
    cold_match = (
        base_cold["contract_order"] == current_cold["contract_order"]
        and base_cold["artifact_sha256"] == current_cold["artifact_sha256"]
    )
    if not cold_match:
        errors.append(f"{graph['shape']}/{graph['size']}/cold/{repeat}: v7.9/current outputs differ")

    warm = {}
    for side, cli in (("v7_9_baseline", baseline_cli), ("current", current_cli)):
        with tempfile.TemporaryDirectory(prefix=f"v88-warm-{graph['shape']}-{graph['size']}-{side}-") as temporary:
            project = Path(temporary) / "project"
            _, paths = materialize(project, graph)
            warm_row = edit_case(cli, project, copy.deepcopy(graph), paths, "manifest_only")
            warm[side] = warm_row
            add_error(errors, f"{graph['shape']}/{graph['size']}/{side}/warm-manifest/{repeat}", warm_row, require_found=True)
            if not warm_row.get("diagnostic_matches_cold", False):
                errors.append(f"{graph['shape']}/{graph['size']}/{side}/warm-manifest/{repeat}: cold classification/diagnostic mismatch")
            if not warm_row.get("artifacts_match_cold_or_last_good", False):
                errors.append(f"{graph['shape']}/{graph['size']}/{side}/warm-manifest/{repeat}: output differs from cold")
            if not warm_row.get("contract_order_matches_cold", False):
                errors.append(f"{graph['shape']}/{graph['size']}/{side}/warm-manifest/{repeat}: order differs from cold")
            if warm_row.get("incremental_contract_order") != expected_order(graph):
                errors.append(f"{graph['shape']}/{graph['size']}/{side}/warm-manifest/{repeat}: order differs from fixture oracle")
            if side == "current":
                count = warm_row.get("counters", {})
                if any(count.get(key) != 0 for key in ("modules_parsed", "contracts_relinked")):
                    errors.append(f"{graph['shape']}/{graph['size']}/warm-manifest/{repeat}: manifest edit relinked source")
    return {
        "case_kind": "cold_and_warm_manifest",
        "repeat": repeat,
        "graph": graph_record(graph),
        "cold_outputs_match": cold_match,
        "cold": {side: values["cold"] for side, values in sides.items()},
        "warm_manifest": warm,
    }


def mutation_pair(graph: dict, edit: str, repeat: int, baseline_cli: Path, current_cli: Path, errors: list[str]) -> dict:
    sides = {}
    for side, cli in (("v7_9_baseline", baseline_cli), ("current", current_cli)):
        with tempfile.TemporaryDirectory(prefix=f"v88-edit-{edit}-{side}-") as temporary:
            project = Path(temporary) / "project"
            _, paths = materialize(project, graph)
            row = edit_case(cli, project, copy.deepcopy(graph), paths, edit)
            sides[side] = row
            add_error(errors, f"layered_shared_path/128/{edit}/{side}/{repeat}", row)
            if edit == "no_op":
                if row.get("edit_loop_ns") is not None or row.get("observation_window_ns") is None:
                    errors.append(f"{edit}/{side}/{repeat}: invalid no-op timing fields")
                if row.get("version_before") != row.get("version_after"):
                    errors.append(f"{edit}/{side}/{repeat}: no-op version changed")
                if row.get("artifact_sha256") != row.get("initial_artifact_sha256"):
                    errors.append(f"{edit}/{side}/{repeat}: no-op artifact hash changed")
            elif not row.get("recompute_set_matches_oracle", False):
                errors.append(f"{edit}/{side}/{repeat}: observed ContractIds differ from fixture closure")
            if not row.get("diagnostic_matches_cold", False):
                errors.append(f"{edit}/{side}/{repeat}: cold result/diagnostic/source location mismatch")
            if row.get("outcome") == "FOUND/VERIFIED":
                if not row.get("contract_order_matches_cold", False):
                    errors.append(f"{edit}/{side}/{repeat}: incremental order differs from cold")
                if row.get("incremental_contract_order") != row.get("expected_contract_order"):
                    errors.append(f"{edit}/{side}/{repeat}: incremental order differs from fixture oracle")
            if not row.get("artifacts_match_cold_or_last_good", False):
                errors.append(f"{edit}/{side}/{repeat}: artifact mismatch or last-good output changed")
            if edit == "valid_invalid_recovered" and not row.get(
                "last_good_version_and_artifacts_preserved", False
            ):
                errors.append(f"{edit}/{side}/{repeat}: failed edit changed last-good version or artifacts")
            if row.get("recovery") and not row.get("recovery_matches_cold", False):
                errors.append(f"{edit}/{side}/{repeat}: recovery artifacts differ from cold")
            if row.get("recovery") and row["recovery"].get("outcome") == "FOUND/VERIFIED":
                if not row["recovery"].get("contract_order_matches_cold", False):
                    errors.append(f"{edit}/{side}/{repeat}: recovery order differs from cold")
                if row["recovery"].get("contract_order") != row.get("expected_contract_order"):
                    errors.append(f"{edit}/{side}/{repeat}: recovery order differs from fixture oracle")
            if edit == "valid_invalid_recovered" and row.get("recovery", {}).get("outcome") != "FOUND/VERIFIED":
                errors.append(f"{edit}/{side}/{repeat}: recovery did not reach FOUND/VERIFIED")
            if edit == "no_op":
                if row.get("outcome") != "NO_REBUILD_EXPECTED":
                    errors.append(f"{edit}/{side}/{repeat}: expected NO_REBUILD_EXPECTED")
            elif row.get("outcome") == "LOAD_LINK_FAIL":
                add_error(errors, f"{edit}/{side}/{repeat}", row, require_found=False)
            else:
                add_error(errors, f"{edit}/{side}/{repeat}", row, require_found=True)
    if sides["v7_9_baseline"].get("diagnostic") != sides["current"].get("diagnostic"):
        errors.append(f"{edit}/{repeat}: baseline/current diagnostics differ")
    if sides["v7_9_baseline"].get("source_location") != sides["current"].get("source_location"):
        errors.append(f"{edit}/{repeat}: baseline/current source locations differ")
    if sides["v7_9_baseline"].get("artifact_sha256") != sides["current"].get("artifact_sha256"):
        errors.append(f"{edit}/{repeat}: baseline/current artifact hashes differ")
    if sides["v7_9_baseline"].get("observed_recomputed_contract_names") != sides["current"].get("observed_recomputed_contract_names"):
        errors.append(f"{edit}/{repeat}: baseline/current recomputed contract names differ")
    return {
        "case_kind": "mutation",
        "edit_class": edit,
        "repeat": repeat,
        "graph": graph_record(graph),
        "sides": sides,
        "artifact_hashes_match": (
            sides["v7_9_baseline"].get("artifact_sha256")
            == sides["current"].get("artifact_sha256")
        ),
        "recomputed_names_match": (
            sides["v7_9_baseline"].get("observed_recomputed_contract_names")
            == sides["current"].get("observed_recomputed_contract_names")
        ),
    }


def pathology_pair(kind: str, repeat: int, baseline_cli: Path, current_cli: Path, errors: list[str]) -> dict:
    sides = {}
    for side, cli in (("v7_9_baseline", baseline_cli), ("current", current_cli)):
        with tempfile.TemporaryDirectory(prefix=f"v88-pathology-{kind}-{side}-") as temporary:
            project = Path(temporary) / "project"
            row = pathology_case(cli, project, kind)
            sides[side] = row
            label = f"{kind}/{side}/{repeat}"
            if row.get("failure"):
                errors.append(f"{label}: {row['failure']}")
            if row.get("outcome") != "LOAD_LINK_FAIL" or not row.get("failure_matches_cold"):
                errors.append(f"{label}: retained failure differs from cold or emitted proof classification")
            if not row.get("last_good_version_and_hashes_preserved"):
                errors.append(f"{label}: failed pathology did not preserve last-good state")
            if not row.get("recovery", {}).get("matches_cold"):
                errors.append(f"{label}: recovery differs from cold")
            if row.get("recovery", {}).get("outcome") != "FOUND/VERIFIED":
                errors.append(f"{label}: recovery did not reach FOUND/VERIFIED")
            if not row.get("recovery", {}).get("contract_order_matches_cold"):
                errors.append(f"{label}: recovery order differs from cold")
            for field in ("diagnostic", "source_location"):
                if field not in row or (field == "diagnostic" and row.get(field) is None):
                    errors.append(f"{label}: missing {field}; cannot compare pathology result")
            if kind == "duplicate_binding" and row.get("source_location") != "2":
                errors.append(f"{label}: duplicate imported local name must be diagnosed on entry.ax line 2")
            add_error(errors, label, row, require_found=False)
    diagnostics_match = diagnostic_pair_matches(
        sides["v7_9_baseline"], sides["current"]
    )
    if not diagnostics_match:
        errors.append(
            f"pathology/{kind}/{repeat}: baseline/current diagnostics or source locations mismatch or unavailable"
        )
    return {
        "case_kind": "pathology",
        "pathology": kind,
        "repeat": repeat,
        "sides": sides,
        "diagnostics_match": diagnostics_match,
    }


def check_resource(row: dict, label: str, errors: list[str]) -> None:
    for key in ("wall_ns", "startup_ns", "edit_loop_ns", "no_op_total_ns"):
        value = row.get(key)
        if value is not None and value > MAX_SECONDS * 1_000_000_000:
            errors.append(f"{label}: {key} exceeded {MAX_SECONDS}s")
    if row.get("process_kind") in ("cold_cli", "dev_cli"):
        if row.get("failure"):
            errors.append(f"{label}: {row['failure']}")
        peak = row.get("peak_rss_bytes")
        if peak is None:
            errors.append(f"{label}: peak RSS was not reported by /usr/bin/time -l")
        elif peak > MAX_RSS_BYTES:
            errors.append(f"{label}: peak RSS {peak} exceeds {MAX_RSS_BYTES}")
    for key, value in row.items():
        if isinstance(value, dict):
            check_resource(value, f"{label}/{key}", errors)
        elif isinstance(value, list):
            for index, item in enumerate(value):
                if isinstance(item, dict):
                    check_resource(item, f"{label}/{key}/{index}", errors)


def resource_gate_self_test() -> None:
    errors = []
    check_resource(
        {"recovery": {"edit_loop_ns": MAX_SECONDS * 1_000_000_000 + 1}},
        "self-test/over-limit",
        errors,
    )
    assert errors == ["self-test/over-limit/recovery: edit_loop_ns exceeded 30s"]
    errors = []
    check_resource(
        {"recovery": {"edit_loop_ns": MAX_SECONDS * 1_000_000_000}},
        "self-test/at-limit",
        errors,
    )
    assert errors == []
    errors = []
    check_resource({"failure": "ignored without process_kind"}, "self-test/non-process", errors)
    assert errors == []


def process_rows(value):
    if isinstance(value, dict):
        if value.get("process_kind") in ("cold_cli", "dev_cli"):
            yield value
        for item in value.values():
            yield from process_rows(item)
    elif isinstance(value, list):
        for item in value:
            yield from process_rows(item)


def expected_process_count(case: dict) -> int:
    kind = case.get("case_kind")
    if kind in ("cold_and_warm_manifest", "edge_cap_layered_dag"):
        return 6
    if kind == "mutation":
        edit = case.get("edit_class")
        return 2 + (0 if edit == "no_op" else 2) + (2 if edit == "valid_invalid_recovered" else 0)
    if kind == "pathology":
        return 8
    return 0


def run_capture(args) -> int:
    baseline_cli, current_cli = args.baseline_cli.resolve(), args.current_cli.resolve()
    repeats = 1 if args.smoke else args.repeat_count
    sizes = (8,) if args.smoke else SIZES
    shapes = ("layered_shared_path",) if args.smoke else SHAPES
    errors, cases = [], []
    provenance = {
        "branch": subprocess.run(
            ["git", "-C", str(ROOT), "branch", "--show-current"], text=True, capture_output=True
        ).stdout.strip(),
        "head": subprocess.run(
            ["git", "-C", str(ROOT), "rev-parse", "HEAD"], text=True, capture_output=True
        ).stdout.strip(),
        "dirty_scope": subprocess.run(
            ["git", "-C", str(ROOT), "status", "--short"], text=True, capture_output=True
        ).stdout.splitlines(),
        "qualified_source_sha256": {
            relative: sha256(ROOT / relative)
            for relative in (
                "crates/axiom-cli/src/web.rs",
                "crates/axiom-project/src/lib.rs",
            )
        },
        "harness_sha256": sha256(Path(__file__).resolve()),
        "fixture_oracle_sha256": sha256(Path(__file__).with_name("v8_dag_scale_fixtures.py")),
        "baseline_cli_sha256": sha256(baseline_cli),
        "current_cli_sha256": sha256(current_cli),
        "baseline_cli_path": str(baseline_cli),
        "current_cli_path": str(current_cli),
        "rustc_version": subprocess.run(["rustc", "--version"], text=True, capture_output=True).stdout.strip(),
        "rustc_host": subprocess.run(["rustc", "-vV"], text=True, capture_output=True).stdout.splitlines(),
        "host": platform.platform(),
        "architecture": platform.machine(),
        "profile": "release",
        "proof_bounds": BOUNDS,
        "repeat_count": repeats,
        "per_case_deadline_seconds": MAX_SECONDS,
        "per_case_rss_ceiling_bytes": MAX_RSS_BYTES,
    }
    partial_path = args.output.with_name(args.output.name + ".partial")
    partial_base = {
        "schema_version": 1,
        "milestone": "v8.8",
        "capture_mode": "smoke_only" if args.smoke else "full",
        "capture_state": "RUNNING",
        "capture_complete": False,
        "started_at_utc": datetime.now(timezone.utc).isoformat(),
        "provenance": provenance,
    }

    def checkpoint() -> None:
        write_json_atomic(partial_path, {**partial_base, "cases": cases, "errors": errors})

    checkpoint()
    for shape in shapes:
        for size in sizes:
            graph = graph_for(shape, size)
            for repeat in range(1, repeats + 1):
                case = fixture_pair(graph, repeat, baseline_cli, current_cli, errors)
                cases.append(case)
                for side, value in case["cold"].items():
                    check_resource(value, f"{shape}/{size}/{side}/cold/{repeat}", errors)
                for side, value in case["warm_manifest"].items():
                    check_resource(value, f"{shape}/{size}/{side}/warm/{repeat}", errors)
                checkpoint()
    if not args.smoke:
        edge_graph = graph_for("layered_shared_path", 128, edge_cap=True)
        for repeat in range(1, repeats + 1):
            case = fixture_pair(edge_graph, repeat, baseline_cli, current_cli, errors)
            case["case_kind"] = "edge_cap_layered_dag"
            cases.append(case)
            if case["graph"]["edge_count_including_entry"] != 512:
                errors.append(f"edge-cap case {repeat}: expected 512 edges")
            for side, value in case["cold"].items():
                check_resource(value, f"edge-cap/{side}/cold/{repeat}", errors)
            for side, value in case["warm_manifest"].items():
                check_resource(value, f"edge-cap/{side}/warm/{repeat}", errors)
            checkpoint()
        graph = graph_for("layered_shared_path", 128)
        for edit in MUTATIONS:
            for repeat in range(1, repeats + 1):
                case = mutation_pair(graph, edit, repeat, baseline_cli, current_cli, errors)
                cases.append(case)
                for side, value in case["sides"].items():
                    check_resource(value, f"mutation/{edit}/{side}/{repeat}", errors)
                checkpoint()
        for kind in ("cycle", "duplicate_binding"):
            for repeat in range(1, repeats + 1):
                case = pathology_pair(kind, repeat, baseline_cli, current_cli, errors)
                cases.append(case)
                for side, value in case["sides"].items():
                    check_resource(value, f"pathology/{kind}/{side}/{repeat}", errors)
                checkpoint()
    expected_processes = sum(expected_process_count(case) for case in cases)
    recorded_processes = sum(1 for case in cases for _ in process_rows(case))
    if recorded_processes != expected_processes:
        errors.append(
            f"direct CLI process evidence count {recorded_processes} != expected {expected_processes}"
        )
    payload = {
        "schema_version": 1,
        "milestone": "v8.8",
        "capture_mode": "smoke_only" if args.smoke else "full",
        "status": "PASS" if not errors else "FAIL",
        "generated_at_utc": datetime.now(timezone.utc).isoformat(),
        "provenance": provenance,
        "notes": {
            "rss_boundary": "macOS /usr/bin/time -l wraps the Axiom CLI directly; the Python harness is not the measured child.",
            "process_measurement_coverage": {
                "recorded_direct_cli_processes": recorded_processes,
                "expected_direct_cli_processes": expected_processes,
                "complete": recorded_processes == expected_processes,
            },
            "no_speedup_claim": True,
            "preliminary_numbers": "No unpreserved feasibility-smoke measurements are used as baseline or limit evidence.",
            "exact_closure": "Observed verified ContractId names are compared with the fixture reverse-import oracle; aggregate cache counters are not used as closure evidence.",
            "watch_paths": "Bounded failed-load watch paths are asserted by the focused CLI regression test; they are not inferred from runtime counters.",
        },
        "cases": cases,
        "errors": errors,
    }
    write_json_atomic(args.output, payload)
    partial_path.unlink(missing_ok=True)
    print(f"wrote {args.output}; status={payload['status']}; cases={len(cases)}; errors={len(errors)}")
    return 0 if not errors else 1


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--baseline-cli", type=Path, required=True)
    parser.add_argument("--current-cli", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--repeat-count", type=int, default=REPEATS)
    parser.add_argument("--smoke", action="store_true", help="one 8-node graph only; not qualification")
    parser.add_argument("--self-test", action="store_true", help="validate fixture oracle only")
    args = parser.parse_args()
    from v8_dag_scale_fixtures import self_test

    self_test()
    if args.self_test:
        expected_order_self_test()
        diagnostic_pair_self_test()
        edit_result_decision_self_test()
        pathology_fixture_self_test()
        resource_gate_self_test()
        print("B1 fixture oracle PASS")
        return 0
    if args.repeat_count < 1:
        parser.error("--repeat-count must be positive")
    if platform.system() != "Darwin" or not TIME.is_file():
        parser.error("qualification capture requires macOS /usr/bin/time -l")
    for label, cli in (("baseline", args.baseline_cli), ("current", args.current_cli)):
        if not cli.resolve().is_file() or not os.access(cli.resolve(), os.X_OK):
            parser.error(f"--{label}-cli must be executable")
    return run_capture(args)


if __name__ == "__main__":
    raise SystemExit(main())
