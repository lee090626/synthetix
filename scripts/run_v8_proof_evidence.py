#!/usr/bin/env python3
"""Run focused v8.2 compositional-proof evidence with source provenance."""

import hashlib
import json
import platform
import subprocess
from datetime import datetime, timezone
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
MARKER = "V8_PROOF_EVIDENCE="
CLI_MARKER = "V8_PROOF_CLI_EVIDENCE="
EXPECTED_CASES = {
    "shared_callee_graph_shape",
    "shared_callee_proof_and_metrics",
    "sibling_alpha_compositional_proof",
    "sibling_beta_compositional_proof",
    "unsatisfied_call_site_precondition_source_location",
    "failed_callee_counterexample",
    "failed_summary_not_forwarded",
    "reference_oracle_matches_z3_classes_on_fixture_samples",
}
EXPECTED_CLI_CASES = {
    "cli_build_verified_shared_dag",
    "cli_build_rejects_unsafe_caller",
    "cli_build_rejects_failing_callee",
}


def sha256(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def git(*args):
    return subprocess.check_output(["git", "-C", str(ROOT), *args], text=True).strip()


def main():
    test_file = ROOT / "crates/axiom-cli/tests/v8_proof_dag.rs"
    fixture = ROOT / "benchmarks/v8/proof-dag-fixtures.json"
    command = ["cargo", "test", "-p", "axiom-cli", "--locked", "--test", "v8_proof_dag", "--", "--nocapture"]
    result = subprocess.run(command, cwd=ROOT, text=True, capture_output=True)
    evidence = []
    cli_evidence = []
    for line in (result.stdout + "\n" + result.stderr).splitlines():
        if MARKER in line:
            evidence.append(json.loads(line.split(MARKER, 1)[1]))
        if CLI_MARKER in line:
            cli_evidence.append(json.loads(line.split(CLI_MARKER, 1)[1]))
    rows = evidence[0].get("rows", []) if len(evidence) == 1 else []
    cli_rows = cli_evidence[0].get("cases", []) if len(cli_evidence) == 1 else []
    cases_ok = (
        len(evidence) == 1
        and len(rows) == len(EXPECTED_CASES)
        and {row["case"] for row in rows} == EXPECTED_CASES
        and all(row["status"] == "PASS" for row in rows)
        and len(cli_evidence) == 1
        and len(cli_rows) == len(EXPECTED_CLI_CASES)
        and {row["case"] for row in cli_rows} == EXPECTED_CLI_CASES
        and all(row["status"] == "PASS" for row in cli_rows)
    )
    cargo_version = subprocess.check_output(["rustc", "--version"], text=True).strip()
    payload = {
        "schema_version": 1,
        "generated_at_utc": datetime.now(timezone.utc).isoformat(),
        "milestone": "v8.2 DAG compositional proof",
        "status": "PASS" if result.returncode == 0 and cases_ok else "FAIL",
        "provenance": {
            "source_head": git("rev-parse", "HEAD"),
            "branch": git("branch", "--show-current"),
            "dirty_status": git("status", "--short"),
            "host": {"system": platform.system(), "release": platform.release(), "machine": platform.machine()},
            "rustc": cargo_version,
            "project_source_sha256": sha256(ROOT / "crates/axiom-project/src/lib.rs"),
            "verifier_source_sha256": sha256(ROOT / "crates/axiom-verify/src/lib.rs"),
            "test_harness_path": str(test_file.relative_to(ROOT)),
            "test_harness_sha256": sha256(test_file),
            "fixture_path": str(fixture.relative_to(ROOT)),
            "fixture_sha256": sha256(fixture),
            "runner_path": str(Path(__file__).resolve().relative_to(ROOT)),
            "runner_sha256": sha256(Path(__file__).resolve()),
            "command": " ".join(command),
            "search_bounds": "not applicable: direct compositional verifier with fixed Expr::Arg(0) candidates",
            "exit_code": result.returncode,
        },
        "performance_claim": None,
        "verified_summary_trust_boundary": "Project compositional APIs receive caller-supplied verified_summaries and do not attest proof provenance; production CLI build forwards only summaries it has verified. This evidence exercises both the API boundary and the CLI path.",
        "evidence": evidence[0] if len(evidence) == 1 else None,
        "cli_evidence": cli_evidence[0] if len(cli_evidence) == 1 else None,
        "stdout": result.stdout,
        "stderr": result.stderr,
    }
    output = ROOT / "docs/v8.2-proof.json"
    output.write_text(json.dumps(payload, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    print(f"{payload['status']}: {len(rows)} proof + {len(cli_rows)} CLI evidence case(s) -> {output}")
    return 0 if payload["status"] == "PASS" else 1


if __name__ == "__main__":
    raise SystemExit(main())
