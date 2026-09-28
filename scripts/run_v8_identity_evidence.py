#!/usr/bin/env python3
"""Run focused v8.1 identity tests and write their raw provenance evidence."""

import hashlib
import json
import subprocess
from datetime import datetime, timezone
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
MARKER = "V8_IDENTITY_EVIDENCE="
EXPECTED_CASES = {
    "canonical_alias_shared_dependency_and_disjoint_imports",
    "same_name_contracts_have_distinct_project_ids",
    "web_duplicate_name_rejection_preserved",
    "fresh_graph_rename_replaces_path_identity",
    "duplicate_local_binding_fails_deterministically",
    "import_cycle_fails_deterministically",
    "deterministic_order_source_mapping_and_repeat_diagnostics",
}


def sha256(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def git(*args):
    return subprocess.check_output(["git", "-C", str(ROOT), *args], text=True).strip()


def main():
    test_file = ROOT / "crates/axiom-cli/tests/v8_identity.rs"
    fixture = ROOT / "benchmarks/v8/identity-fixtures.json"
    result = subprocess.run(
        ["cargo", "test", "-p", "axiom-cli", "--locked", "--test", "v8_identity", "--", "--nocapture"],
        cwd=ROOT,
        text=True,
        capture_output=True,
    )
    cases = []
    for line in (result.stdout + "\n" + result.stderr).splitlines():
        if MARKER in line:
            cases.append(json.loads(line.split(MARKER, 1)[1]))
    files = sorted(path for path in fixture.parent.rglob("*") if path.is_file())
    payload = {
        "schema_version": 1,
        "generated_at_utc": datetime.now(timezone.utc).isoformat(),
        "milestone": "v8.1 stable module and dependency identity",
        "status": "PASS" if result.returncode == 0 and len(cases) == len(EXPECTED_CASES) and {case["case"] for case in cases} == EXPECTED_CASES and all(case["status"] == "PASS" for case in cases) else "FAIL",
        "provenance": {
            "source_head": git("rev-parse", "HEAD"),
            "branch": git("branch", "--show-current"),
            "dirty_status": git("status", "--short"),
            "project_source_sha256": sha256(ROOT / "crates/axiom-project/src/lib.rs"),
            "web_source_sha256": sha256(ROOT / "crates/axiom-cli/src/web.rs"),
            "test_harness_path": str(test_file.relative_to(ROOT)),
            "test_harness_sha256": sha256(test_file),
            "fixture_path": str(fixture.relative_to(ROOT)),
            "fixture_sha256": sha256(fixture),
            "fixture_files_sha256": {str(path.relative_to(ROOT)): sha256(path) for path in files},
            "runner_path": str(Path(__file__).resolve().relative_to(ROOT)),
            "runner_sha256": sha256(Path(__file__).resolve()),
            "command": "cargo test -p axiom-cli --locked --test v8_identity -- --nocapture",
            "exit_code": result.returncode,
        },
        "performance_claim": None,
        "cases": cases,
        "stdout": result.stdout,
        "stderr": result.stderr,
    }
    output = ROOT / "docs/v8.1-identity.json"
    output.write_text(json.dumps(payload, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    print(f"{payload['status']}: {len(cases)} identity evidence case(s) -> {output}")
    return 0 if payload["status"] == "PASS" else 1


if __name__ == "__main__":
    raise SystemExit(main())
