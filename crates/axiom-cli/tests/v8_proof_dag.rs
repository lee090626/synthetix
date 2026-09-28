use std::{
    collections::{BTreeMap, HashMap},
    fs,
    path::PathBuf,
    process::Command,
    sync::atomic::{AtomicUsize, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

use axiom_ami::Expr;
use axiom_parser::SpecExpr;
use axiom_project::{ContractId, LinkedContract, Project, load};
use serde_json::{Value, json};

static NEXT_TEMP: AtomicUsize = AtomicUsize::new(0);
const FIXTURE: &str = include_str!("../../../benchmarks/v8/proof-dag-fixtures.json");

struct TempProject(PathBuf);

impl TempProject {
    fn new() -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let sequence = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "synthetix-v8-proof-dag-{}-{nonce}-{sequence}",
            std::process::id()
        ));
        let fixture: Value = serde_json::from_str(FIXTURE).unwrap();
        for (relative, contents) in fixture.as_object().unwrap() {
            let path = root.join(relative);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, contents.as_str().unwrap()).unwrap();
        }
        fs::write(
            root.join("synthetix.toml"),
            "[project]\nentry = \"entry.ax\"\ntarget = \"web\"\n",
        )
        .unwrap();
        Self(root)
    }
}

impl Drop for TempProject {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn linked<'a>(project: &'a Project, name: &str) -> &'a LinkedContract {
    project
        .contracts
        .iter()
        .find(|item| item.contract.name == name)
        .unwrap()
}

fn id(linked: &LinkedContract) -> ContractId {
    ContractId::from(linked)
}

fn call_count(expr: &SpecExpr) -> usize {
    match expr {
        SpecExpr::Call(_, args) => 1 + args.iter().map(call_count).sum::<usize>(),
        SpecExpr::Binary(_, lhs, rhs) => call_count(lhs) + call_count(rhs),
        SpecExpr::List(items) => items.iter().map(call_count).sum(),
        SpecExpr::Record { fields, .. } => fields.iter().map(|(_, value)| call_count(value)).sum(),
        SpecExpr::Field(value, _) => call_count(value),
        SpecExpr::Index(value, index) => call_count(value) + call_count(index),
        SpecExpr::ArrayUpdate(value, index, replacement) => {
            call_count(value) + call_count(index) + call_count(replacement)
        }
        SpecExpr::Let(_, value, body) => call_count(value) + call_count(body),
        SpecExpr::If(condition, then_branch, else_branch) => {
            call_count(condition) + call_count(then_branch) + call_count(else_branch)
        }
        SpecExpr::For {
            initial,
            invariant,
            body,
            ..
        } => call_count(initial) + call_count(invariant) + call_count(body),
        SpecExpr::Ident(_) | SpecExpr::Number(_) | SpecExpr::Bool(_) | SpecExpr::Text(_) => 0,
    }
}

#[derive(Debug, Clone)]
struct OracleCall {
    callee: String,
    requires_satisfied: bool,
    ensures_satisfied: bool,
}

fn oracle_bool(
    expr: &SpecExpr,
    contract: &axiom_parser::Contract,
    args: &[u32],
    result: u32,
) -> Result<bool, String> {
    match axiom_verify::reference::eval_spec(expr, contract, args, result)? {
        axiom_verify::reference::Value::Bool(value) => Ok(value),
        _ => Err("oracle fixture expression was not boolean".to_string()),
    }
}

fn resolve_calls(
    expr: &SpecExpr,
    caller: &LinkedContract,
    project: &Project,
    args: &[u32],
    result: u32,
    calls: &mut Vec<OracleCall>,
) -> Result<SpecExpr, String> {
    match expr {
        SpecExpr::Call(name, call_args) => {
            let local_ids = project
                .verification_callee_ids
                .get(&caller.source)
                .ok_or_else(|| format!("missing source scope for {}", caller.contract.name))?;
            let callee_id = local_ids
                .get(name)
                .ok_or_else(|| format!("missing imported identity for {name}"))?;
            let callee = project
                .verification_summary(callee_id)
                .ok_or_else(|| format!("missing source summary for {name}"))?;
            let mut callee_args = Vec::with_capacity(call_args.len());
            for argument in call_args {
                let resolved = resolve_calls(argument, caller, project, args, result, calls)?;
                let value =
                    axiom_verify::reference::eval_spec(&resolved, &caller.contract, args, result)?;
                callee_args.push(match value {
                    axiom_verify::reference::Value::U32(value) => value,
                    axiom_verify::reference::Value::I32(value) => value as u32,
                    axiom_verify::reference::Value::Bool(value) => u32::from(value),
                    axiom_verify::reference::Value::Array(_) => {
                        return Err("array-valued oracle call argument is unsupported".to_string());
                    }
                });
            }
            if callee_args.len() != 1 || callee.contract.params.len() != 1 {
                return Err(format!("unsupported oracle arity for {name}"));
            }
            let callee_result = axiom_verify::reference::eval_ami(&Expr::Arg(0), &callee_args)?;
            let mut callee_requires = Vec::new();
            for clause in &callee.contract.requires {
                let resolved =
                    resolve_calls(clause, callee, project, &callee_args, callee_result, calls)?;
                callee_requires.push(oracle_bool(
                    &resolved,
                    &callee.contract,
                    &callee_args,
                    callee_result,
                )?);
            }
            let requires_satisfied = callee_requires.iter().all(|value| *value);
            let mut callee_ensures = Vec::new();
            for clause in &callee.contract.ensures {
                let resolved =
                    resolve_calls(clause, callee, project, &callee_args, callee_result, calls)?;
                callee_ensures.push(oracle_bool(
                    &resolved,
                    &callee.contract,
                    &callee_args,
                    callee_result,
                )?);
            }
            calls.push(OracleCall {
                callee: name.clone(),
                requires_satisfied,
                ensures_satisfied: callee_ensures.iter().all(|value| *value),
            });
            Ok(SpecExpr::Number(callee_result))
        }
        SpecExpr::Binary(op, lhs, rhs) => Ok(SpecExpr::Binary(
            op.clone(),
            Box::new(resolve_calls(lhs, caller, project, args, result, calls)?),
            Box::new(resolve_calls(rhs, caller, project, args, result, calls)?),
        )),
        SpecExpr::Ident(_) | SpecExpr::Number(_) | SpecExpr::Bool(_) => Ok(expr.clone()),
        _ => Err(format!("unsupported oracle AST node: {expr:?}")),
    }
}

fn oracle_sample(project: &Project, contract_id: &ContractId, input: u32) -> Result<Value, String> {
    let caller = project
        .verification_summary(contract_id)
        .ok_or_else(|| format!("missing source summary for {}", contract_id.name))?;
    if caller.contract.params.len() != 1 {
        return Err(format!("unsupported oracle arity for {}", contract_id.name));
    }
    let args = [input];
    let result = axiom_verify::reference::eval_ami(&Expr::Arg(0), &args)?;
    let mut calls = Vec::new();
    let mut requires = Vec::new();
    for clause in &caller.contract.requires {
        let resolved = resolve_calls(clause, caller, project, &args, result, &mut calls)?;
        requires.push(oracle_bool(&resolved, &caller.contract, &args, result)?);
    }
    let caller_requires_satisfied = requires.iter().all(|value| *value);
    let mut ensures = Vec::new();
    for clause in &caller.contract.ensures {
        let resolved = resolve_calls(clause, caller, project, &args, result, &mut calls)?;
        ensures.push(oracle_bool(&resolved, &caller.contract, &args, result)?);
    }
    let violation = caller_requires_satisfied && calls.iter().any(|call| !call.requires_satisfied);
    let callee_postconditions_hold = calls.iter().all(|call| call.ensures_satisfied);
    let sample_class = if violation {
        "CALL_PRECONDITION_VIOLATION"
    } else if !caller_requires_satisfied {
        "INACTIVE_PRECONDITION"
    } else if !callee_postconditions_hold || ensures.iter().any(|value| !value) {
        "COUNTEREXAMPLE"
    } else {
        "VERIFIED_SAMPLE"
    };
    Ok(json!({
        "contract": contract_id.name,
        "input": input,
        "identity_candidate_output": result,
        "caller_requires_satisfied": caller_requires_satisfied,
        "ensures_satisfied": ensures.iter().all(|value| *value),
        "callee_calls": calls.iter().map(|call| json!({"callee": call.callee, "requires_satisfied": call.requires_satisfied, "ensures_satisfied": call.ensures_satisfied})).collect::<Vec<_>>(),
        "sample_class": sample_class,
    }))
}

fn emit(rows: &mut Vec<Value>, case: &str, expected: Value, observed: Value) {
    let status = if expected == observed { "PASS" } else { "FAIL" };
    rows.push(json!({"case": case, "status": status, "expected": expected, "observed": observed}));
    assert_eq!(
        rows.last().unwrap()["expected"],
        rows.last().unwrap()["observed"],
        "{case}"
    );
}

#[test]
fn v8_2_shared_callee_proof_evidence() {
    let root = TempProject::new();
    let project = load(root.0.join("synthetix.toml")).unwrap();
    let mut rows = Vec::new();
    let ids = project.contracts.iter().map(id).collect::<Vec<_>>();
    let edge_count = project
        .contracts
        .iter()
        .map(|item| project.verification_dependencies(item).unwrap().len())
        .sum::<usize>();
    emit(
        &mut rows,
        "shared_callee_graph_shape",
        json!({"contracts": 7, "shared_positive_callers": 3, "dependency_edges": 4}),
        json!({
            "contracts": ids.len(),
            "shared_positive_callers": project.contracts.iter().filter(|item| item.contract.name == "alpha" || item.contract.name == "beta" || item.contract.name == "unsafe_root").count(),
            "dependency_edges": edge_count,
        }),
    );

    let positive = linked(&project, "positive");
    let (positive_proof, positive_metrics) = project
        .verify_compositional_with_verified_summaries_and_metrics(
            positive,
            &Expr::Arg(0),
            &HashMap::new(),
        )
        .unwrap();
    let mut z3_classes = BTreeMap::new();
    z3_classes.insert(
        "positive".to_string(),
        positive_proof
            .proof
            .as_ref()
            .map(|proof| proof.status_name())
            .unwrap_or("NO_PROOF")
            .to_string(),
    );
    emit(
        &mut rows,
        "shared_callee_proof_and_metrics",
        json!({"proof": "VERIFIED", "solver_invocations_positive": true, "solver_invocations": positive_metrics.solver_invocations}),
        json!({"proof": positive_proof.proof.as_ref().map(|proof| proof.status_name()), "solver_invocations_positive": positive_metrics.solver_invocations > 0, "solver_invocations": positive_metrics.solver_invocations}),
    );
    let mut verified = HashMap::from([(id(positive), positive.contract.clone())]);
    let mut alpha_metrics = 0;
    let mut beta_metrics = 0;

    for name in ["alpha", "beta"] {
        let caller = linked(&project, name);
        let (proof, metrics) = project
            .verify_compositional_with_verified_summaries_and_metrics(
                caller,
                &Expr::Arg(0),
                &verified,
            )
            .unwrap();
        let source_summary = project.verification_summary(&id(caller)).unwrap();
        let nested_requires_call_count = source_summary
            .contract
            .requires
            .iter()
            .map(call_count)
            .sum::<usize>();
        if name == "alpha" {
            assert!(
                nested_requires_call_count >= 2,
                "alpha must retain both imported call sites in requires"
            );
        }
        if name == "alpha" {
            alpha_metrics = metrics.solver_invocations;
        } else {
            beta_metrics = metrics.solver_invocations;
        }
        z3_classes.insert(
            name.to_string(),
            proof
                .proof
                .as_ref()
                .map(|proof| proof.status_name())
                .unwrap_or("NO_PROOF")
                .to_string(),
        );
        emit(
            &mut rows,
            &format!("sibling_{name}_compositional_proof"),
            json!({"proof": "VERIFIED", "solver_invocations_positive": true, "nested_requires_call_count": if name == "alpha" { nested_requires_call_count } else { 0 }}),
            json!({
                "proof": proof.proof.as_ref().map(|proof| proof.status_name()),
                "solver_invocations_positive": metrics.solver_invocations > 0,
                "nested_requires_call_count": nested_requires_call_count,
            }),
        );
        if proof.proof == Some(axiom_verify::ProofResult::Verified) && proof.diagnostic.is_none() {
            verified.insert(id(caller), linked(&project, name).contract.clone());
        }
    }

    let unsafe_root = linked(&project, "unsafe_root");
    let (unsafe_proof, unsafe_metrics) = project
        .verify_compositional_with_verified_summaries_and_metrics(
            unsafe_root,
            &Expr::Arg(0),
            &verified,
        )
        .unwrap();
    let diagnostic = unsafe_proof.diagnostic.as_ref().map(|diagnostic| match diagnostic {
        axiom_verify::Diagnostic::CallPreconditionViolation { callee, span } => {
            json!({"kind": "CALL_PRECONDITION_VIOLATION", "callee": callee, "line": span.map(|span| span.line)})
        }
        other => json!({"kind": other.status_name()}),
    });
    z3_classes.insert(
        "unsafe_root".to_string(),
        unsafe_proof
            .diagnostic
            .as_ref()
            .map(|diagnostic| diagnostic.status_name())
            .unwrap_or_else(|| {
                unsafe_proof
                    .proof
                    .as_ref()
                    .map(|proof| proof.status_name())
                    .unwrap_or("NO_PROOF")
            })
            .to_string(),
    );
    emit(
        &mut rows,
        "unsatisfied_call_site_precondition_source_location",
        json!({"proof": null, "diagnostic": {"kind": "CALL_PRECONDITION_VIOLATION", "callee": "positive", "line": 3}, "solver_invocations_positive": true}),
        json!({"proof": unsafe_proof.proof.as_ref().map(|proof| proof.status_name()), "diagnostic": diagnostic, "solver_invocations_positive": unsafe_metrics.solver_invocations > 0}),
    );

    let liar = linked(&project, "liar");
    let (liar_proof, liar_metrics) = project
        .verify_compositional_with_verified_summaries_and_metrics(
            liar,
            &Expr::Arg(0),
            &HashMap::new(),
        )
        .unwrap();
    z3_classes.insert(
        "liar".to_string(),
        liar_proof
            .proof
            .as_ref()
            .map(|proof| proof.status_name())
            .unwrap_or("NO_PROOF")
            .to_string(),
    );
    emit(
        &mut rows,
        "failed_callee_counterexample",
        json!({"proof": "COUNTEREXAMPLE", "solver_invocations_positive": true}),
        json!({"proof": liar_proof.proof.as_ref().map(|proof| proof.status_name()), "solver_invocations_positive": liar_metrics.solver_invocations > 0}),
    );
    let dependent = linked(&project, "depends_on_failed");
    let forwarding_error = project
        .verify_compositional_with_verified_summaries_and_metrics(
            dependent,
            &Expr::Arg(0),
            &verified,
        )
        .unwrap_err();
    emit(
        &mut rows,
        "failed_summary_not_forwarded",
        json!({"error": "unverified callee summary `liar` required by `depends_on_failed`"}),
        json!({"error": forwarding_error}),
    );

    let mut reference_samples = BTreeMap::new();
    let mut reference_classes = BTreeMap::new();
    for name in ["positive", "alpha", "beta", "unsafe_root", "liar"] {
        let contract_id = id(linked(&project, name));
        let samples = [0_u32, 1, 7]
            .into_iter()
            .map(|input| {
                oracle_sample(&project, &contract_id, input).unwrap_or_else(|error| {
                    panic!("reference oracle does not support {name} input {input}: {error}")
                })
            })
            .collect::<Vec<_>>();
        let sample_classes = samples
            .iter()
            .filter_map(|sample| sample["sample_class"].as_str())
            .collect::<Vec<_>>();
        let reference_class = if sample_classes.contains(&"CALL_PRECONDITION_VIOLATION") {
            "CALL_PRECONDITION_VIOLATION"
        } else if sample_classes.contains(&"COUNTEREXAMPLE") {
            "COUNTEREXAMPLE"
        } else {
            "VERIFIED"
        };
        reference_classes.insert(name.to_string(), reference_class.to_string());
        reference_samples.insert(name.to_string(), samples);
    }
    emit(
        &mut rows,
        "reference_oracle_matches_z3_classes_on_fixture_samples",
        json!({"classification_by_contract": reference_classes}),
        json!({"classification_by_contract": z3_classes}),
    );

    println!(
        "V8_PROOF_EVIDENCE={}",
        json!({"graph": {"contracts": ids.len(), "dependency_edges": edge_count}, "reference_samples": reference_samples, "reference_oracle_scope": "finite concrete samples over loaded verification_summaries; calls resolved using the fixed identity candidate and independently checked callee requires/ensures with reference evaluator; not exhaustive", "solver_metrics": {
            "positive": positive_metrics.solver_invocations,
            "alpha": alpha_metrics,
            "beta": beta_metrics,
            "unsafe_root": unsafe_metrics.solver_invocations,
            "liar": liar_metrics.solver_invocations,
        }, "rows": rows})
    );
}

#[test]
fn v8_2_cli_build_fail_closed_evidence() {
    let mut rows = Vec::new();
    let positive = TempProject::new();
    let entry_path = positive.0.join("entry.ax");
    let entry = fs::read_to_string(&entry_path).unwrap();
    fs::write(
        &entry_path,
        entry
            .lines()
            .filter(|line| !line.contains("unsafe_root") && !line.contains("depends_on_failed"))
            .collect::<Vec<_>>()
            .join("\n"),
    )
    .unwrap();
    let positive_result = run_cli_build(&positive);
    emit(
        &mut rows,
        "cli_build_verified_shared_dag",
        json!({"exit_success": true, "proof_result": "VERIFIED", "diagnostic_class": null, "diagnostic_source": null, "diagnostic": "", "artifact_exists": true, "false_verified_on_failure": false}),
        positive_result,
    );

    let unsafe_case = TempProject::new();
    let entry_path = unsafe_case.0.join("entry.ax");
    let entry = fs::read_to_string(&entry_path).unwrap();
    fs::write(
        &entry_path,
        entry
            .lines()
            .filter(|line| !line.contains("depends_on_failed"))
            .collect::<Vec<_>>()
            .join("\n"),
    )
    .unwrap();
    let unsafe_result = run_cli_build(&unsafe_case);
    let expected_unsafe_diagnostic = format!(
        "{}:3:2: call-site precondition for positive in unsafe_root is not established",
        fs::canonicalize(unsafe_case.0.join("unsafe.ax"))
            .unwrap()
            .display()
    );
    assert_eq!(
        unsafe_result["diagnostic_class"], "CALL_PRECONDITION_VIOLATION",
        "{unsafe_result}"
    );
    assert_eq!(unsafe_result["diagnostic"], expected_unsafe_diagnostic);
    let unsafe_expected = json!({"exit_success": false, "proof_result": null, "diagnostic_class": "CALL_PRECONDITION_VIOLATION", "diagnostic_source": "unsafe.ax", "diagnostic": expected_unsafe_diagnostic, "artifact_exists": false, "false_verified_on_failure": false});
    emit(
        &mut rows,
        "cli_build_rejects_unsafe_caller",
        unsafe_expected,
        unsafe_result,
    );

    let failed_callee = TempProject::new();
    let shared_path = failed_callee.0.join("shared.ax");
    let shared = fs::read_to_string(&shared_path).unwrap();
    fs::write(
        &shared_path,
        shared.replace(
            "ensures result == value +% 1;",
            "requires value > 0;\n requires value == 0;\n ensures result == value;",
        ),
    )
    .unwrap();
    let failed_result = run_cli_build(&failed_callee);
    let expected_failed_diagnostic = format!(
        "{}:liar: UNSATISFIABLE_PRECONDITION: requires clauses are contradictory",
        fs::canonicalize(&shared_path).unwrap().display()
    );
    assert_eq!(
        failed_result["diagnostic_class"], "UNSATISFIABLE_PRECONDITION",
        "{failed_result}"
    );
    assert_eq!(failed_result["diagnostic"], expected_failed_diagnostic);
    let failed_expected = json!({"exit_success": false, "proof_result": null, "diagnostic_class": "UNSATISFIABLE_PRECONDITION", "diagnostic_source": "shared.ax", "diagnostic": expected_failed_diagnostic, "artifact_exists": false, "false_verified_on_failure": false});
    emit(
        &mut rows,
        "cli_build_rejects_failing_callee",
        failed_expected,
        failed_result,
    );

    println!("V8_PROOF_CLI_EVIDENCE={}", json!({"cases": rows}));
}

fn run_cli_build(project: &TempProject) -> Value {
    let output = Command::new(env!("CARGO_BIN_EXE_axiom"))
        .arg("build")
        .current_dir(&project.0)
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    let diagnostic = stderr
        .lines()
        .find(|line| line.starts_with("error: "))
        .unwrap_or_default()
        .trim_start_matches("error: ")
        .to_string();
    let proof_result = stdout
        .lines()
        .find_map(|line| line.strip_prefix("proof_result: "));
    let diagnostic_class = if diagnostic.contains("CALL_PRECONDITION_VIOLATION")
        || diagnostic.contains("call-site precondition")
    {
        "CALL_PRECONDITION_VIOLATION"
    } else if diagnostic.contains("UNSATISFIABLE_PRECONDITION")
        || diagnostic.contains("unsatisfiable precondition")
    {
        "UNSATISFIABLE_PRECONDITION"
    } else {
        "BUILD_ERROR"
    };
    let diagnostic_source = if diagnostic.contains("unsafe.ax") {
        "unsafe.ax"
    } else if diagnostic.contains("shared.ax") {
        "shared.ax"
    } else {
        ""
    };
    let artifact_exists = project.0.join("dist/app.proof.json").exists();
    let false_verified_on_failure = !output.status.success()
        && (proof_result == Some("VERIFIED")
            || stdout.contains("verified: true")
            || stderr.contains("proof_result: VERIFIED")
            || stderr.contains("verified: true")
            || artifact_exists);
    json!({
        "exit_success": output.status.success(),
        "proof_result": proof_result,
        "diagnostic_class": (!output.status.success()).then_some(diagnostic_class),
        "diagnostic_source": (!output.status.success()).then_some(diagnostic_source),
        "diagnostic": diagnostic,
        "artifact_exists": artifact_exists,
        "false_verified_on_failure": false_verified_on_failure,
    })
}
