//! SMT verification for the initial u32 contract subset.

use std::{collections::HashMap, time::Instant};

use axiom_ami::Expr;
use axiom_parser::{BinaryOp, Contract, Span, SpecExpr, Type};
use z3::{
    SatResult, Solver,
    ast::{BV, Bool},
};

pub mod reference;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProofResult {
    Verified,
    Counterexample(Vec<u32>),
    SolverUnknown,
}

/// Compatibility name for callers that still use the pre-v4.2 API.
pub type VerificationResult = ProofResult;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContractCheck {
    Valid,
    Invalid(String),
    UnsatisfiablePrecondition,
    UnsatisfiablePostcondition,
}

impl ContractCheck {
    pub fn status_name(&self) -> &'static str {
        match self {
            Self::Valid => "VALID",
            Self::Invalid(_) => "INVALID",
            Self::UnsatisfiablePrecondition => "UNSATISFIABLE_PRECONDITION",
            Self::UnsatisfiablePostcondition => "UNSATISFIABLE_POSTCONDITION",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Diagnostic {
    AmbiguousCandidates,
    CallPreconditionViolation { callee: String, span: Option<Span> },
}

impl Diagnostic {
    pub fn status_name(&self) -> &'static str {
        match self {
            Self::AmbiguousCandidates => "AMBIGUOUS_CANDIDATES",
            Self::CallPreconditionViolation { .. } => "CALL_PRECONDITION_VIOLATION",
        }
    }
}

impl ProofResult {
    pub fn status_name(&self) -> &'static str {
        match self {
            Self::Verified => "VERIFIED",
            Self::Counterexample(_) => "COUNTEREXAMPLE",
            Self::SolverUnknown => "SOLVER_UNKNOWN",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreconditionResult {
    Satisfiable,
    UnsatisfiablePrecondition,
    SolverUnknown,
}

/// Optional timing and call-count data for benchmark runs. Normal verifier APIs
/// do not collect it, so production verification does not pay the timing cost.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct VerificationMetrics {
    pub solver_invocations: usize,
    pub solver_time_ns: u128,
}

impl VerificationMetrics {
    pub fn merge(&mut self, other: Self) {
        self.solver_invocations += other.solver_invocations;
        self.solver_time_ns += other.solver_time_ns;
    }
}

fn solver_check(solver: &Solver, metrics: &mut Option<&mut VerificationMetrics>) -> SatResult {
    let Some(metrics) = metrics.as_deref_mut() else {
        return solver.check();
    };
    let started = Instant::now();
    let result = solver.check();
    metrics.solver_invocations += 1;
    metrics.solver_time_ns += started.elapsed().as_nanos();
    result
}

/// Checks whether the conjunction of all `requires` clauses has at least one
/// valid scalar input.
pub fn check_preconditions(contract: &Contract) -> Result<PreconditionResult, String> {
    check_preconditions_inner(contract, &mut None)
}

pub fn check_preconditions_with_metrics(
    contract: &Contract,
) -> Result<(PreconditionResult, VerificationMetrics), String> {
    let mut metrics = VerificationMetrics::default();
    let result = check_preconditions_inner(contract, &mut Some(&mut metrics))?;
    Ok((result, metrics))
}

fn check_preconditions_inner(
    contract: &Contract,
    metrics: &mut Option<&mut VerificationMetrics>,
) -> Result<PreconditionResult, String> {
    let args = symbolic_args(contract)?;
    let result = BV::new_const("__axiom_result", 32);
    let requires = preconditions_to_bool(contract, &args, &result)?;
    let solver = Solver::new();
    constrain_bool_parameters(&solver, contract, &args);
    solver.assert(&requires);

    match solver_check(&solver, metrics) {
        SatResult::Sat => Ok(PreconditionResult::Satisfiable),
        SatResult::Unsat => Ok(PreconditionResult::UnsatisfiablePrecondition),
        SatResult::Unknown => Ok(PreconditionResult::SolverUnknown),
    }
}

/// Checks syntax/type validity and whether the contract has any model.
pub fn check_contract(contract: &Contract) -> Result<ContractCheck, String> {
    check_contract_inner(contract, &mut None)
}

pub fn check_contract_with_metrics(
    contract: &Contract,
) -> Result<(ContractCheck, VerificationMetrics), String> {
    let mut metrics = VerificationMetrics::default();
    let result = check_contract_inner(contract, &mut Some(&mut metrics))?;
    Ok((result, metrics))
}

fn check_contract_inner(
    contract: &Contract,
    metrics: &mut Option<&mut VerificationMetrics>,
) -> Result<ContractCheck, String> {
    if let Err(error) = axiom_parser::validate(contract) {
        return Ok(ContractCheck::Invalid(error));
    }

    if contract.kind == axiom_parser::DeclarationKind::Extern {
        return Ok(ContractCheck::Valid);
    }

    match check_preconditions_inner(contract, metrics)? {
        PreconditionResult::Satisfiable => {}
        PreconditionResult::UnsatisfiablePrecondition => {
            return Ok(ContractCheck::UnsatisfiablePrecondition);
        }
        PreconditionResult::SolverUnknown => {
            return Err("contract precondition satisfiability is unknown".to_string());
        }
    }

    let args = symbolic_args(contract)?;
    let result = BV::new_const("__axiom_result", 32);
    let requires = preconditions_to_bool(contract, &args, &result)?;
    let ensures = ensures_to_bool(contract, &args, &result)?;
    let solver = Solver::new();
    constrain_bool_parameters(&solver, contract, &args);
    solver.assert(Bool::and(&[&requires, &ensures]));

    match solver_check(&solver, metrics) {
        SatResult::Sat if contract.kind == axiom_parser::DeclarationKind::Function => {
            let candidate = match implementation_to_ami(contract) {
                Ok(candidate) => candidate,
                Err(error) => return Ok(ContractCheck::Invalid(error)),
            };
            match verify_inner(contract, &candidate, metrics)? {
                ProofResult::Verified => Ok(ContractCheck::Valid),
                ProofResult::Counterexample(values) => Ok(ContractCheck::Invalid(format!(
                    "fn implementation violates its contract at {values:?}"
                ))),
                ProofResult::SolverUnknown => Err("fn implementation proof is unknown".to_string()),
            }
        }
        SatResult::Sat => Ok(ContractCheck::Valid),
        SatResult::Unsat => Ok(ContractCheck::UnsatisfiablePostcondition),
        SatResult::Unknown => Err("contract postcondition satisfiability is unknown".to_string()),
    }
}

/// Lowers a `fn` return expression into the same AMI used by the verifier and
/// WASM backend. Calls remain outside this local lowering boundary until v4.6.
pub fn implementation_to_ami(contract: &Contract) -> Result<Expr, String> {
    if contract.kind != axiom_parser::DeclarationKind::Function {
        return Err("only fn declarations have a local implementation".to_string());
    }
    let implementation = contract
        .implementation
        .as_ref()
        .ok_or_else(|| "fn declaration has no return expression".to_string())?;
    let types = contract
        .params
        .iter()
        .map(|param| (param.name.clone(), param.ty.clone()))
        .collect::<HashMap<_, _>>();
    lower_implementation(implementation, contract, &types, &HashMap::new())
}

fn lower_implementation(
    expr: &SpecExpr,
    contract: &Contract,
    types: &HashMap<String, Type>,
    values: &HashMap<String, Expr>,
) -> Result<Expr, String> {
    match expr {
        SpecExpr::Ident(name) => {
            if name == "result" {
                return Err("result is not available in a fn return expression".to_string());
            }
            if let Some(value) = values.get(name) {
                return Ok(value.clone());
            }
            let index = contract
                .params
                .iter()
                .position(|param| param.name == *name)
                .ok_or_else(|| format!("unknown identifier: {name}"))?;
            Ok(Expr::Arg(index as u8))
        }
        SpecExpr::Number(value) => Ok(Expr::Const(*value)),
        SpecExpr::Bool(value) => Ok(Expr::Const(u32::from(*value))),
        SpecExpr::Let(name, value, body) => {
            let lowered = lower_implementation(value, contract, types, values)?;
            let value_type = axiom_parser::type_of_expr(value, types, None)?;
            let mut nested_types = types.clone();
            nested_types.insert(name.clone(), value_type);
            let mut nested_values = values.clone();
            nested_values.insert(name.clone(), lowered);
            lower_implementation(body, contract, &nested_types, &nested_values)
        }
        SpecExpr::If(condition, then_branch, else_branch) => Ok(Expr::Select {
            condition: Box::new(lower_implementation(condition, contract, types, values)?),
            then_branch: Box::new(lower_implementation(then_branch, contract, types, values)?),
            else_branch: Box::new(lower_implementation(else_branch, contract, types, values)?),
        }),
        SpecExpr::Binary(op, lhs, rhs) => {
            let lhs_type = axiom_parser::type_of_expr(lhs, types, None)?;
            let lhs = lower_implementation(lhs, contract, types, values)?;
            let rhs = lower_implementation(rhs, contract, types, values)?;
            let binary = match op {
                BinaryOp::AddWrap => Expr::Add(Box::new(lhs), Box::new(rhs)),
                BinaryOp::SubWrap => Expr::Sub(Box::new(lhs), Box::new(rhs)),
                BinaryOp::MulWrap => Expr::Mul(Box::new(lhs), Box::new(rhs)),
                BinaryOp::Shl => Expr::Shl(Box::new(lhs), Box::new(rhs)),
                BinaryOp::BitAnd => Expr::BitAnd(Box::new(lhs), Box::new(rhs)),
                BinaryOp::BitOr => Expr::BitOr(Box::new(lhs), Box::new(rhs)),
                BinaryOp::BitXor => Expr::BitXor(Box::new(lhs), Box::new(rhs)),
                BinaryOp::Eq => Expr::Eq(Box::new(lhs), Box::new(rhs)),
                BinaryOp::Ne => Expr::Ne(Box::new(lhs), Box::new(rhs)),
                BinaryOp::Lt if lhs_type == Type::I32 => Expr::SLt(Box::new(lhs), Box::new(rhs)),
                BinaryOp::Le if lhs_type == Type::I32 => Expr::SLe(Box::new(lhs), Box::new(rhs)),
                BinaryOp::Gt if lhs_type == Type::I32 => Expr::SGt(Box::new(lhs), Box::new(rhs)),
                BinaryOp::Ge if lhs_type == Type::I32 => Expr::SGe(Box::new(lhs), Box::new(rhs)),
                BinaryOp::Lt => Expr::Lt(Box::new(lhs), Box::new(rhs)),
                BinaryOp::Le => Expr::Le(Box::new(lhs), Box::new(rhs)),
                BinaryOp::Gt => Expr::Gt(Box::new(lhs), Box::new(rhs)),
                BinaryOp::Ge => Expr::Ge(Box::new(lhs), Box::new(rhs)),
                BinaryOp::And => Expr::And(Box::new(lhs), Box::new(rhs)),
                BinaryOp::Or => Expr::Or(Box::new(lhs), Box::new(rhs)),
            };
            Ok(binary)
        }
        SpecExpr::Text(_)
        | SpecExpr::List(_)
        | SpecExpr::Record { .. }
        | SpecExpr::Field(_, _)
        | SpecExpr::Call(_, _) => {
            Err("fn return expression is outside the scalar boundary".to_string())
        }
    }
}

#[derive(Clone)]
enum Value {
    U32(BV),
    I32(BV),
    Bool(Bool),
}

/// Proves that `candidate` satisfies every `ensures` clause under `requires`.
pub fn verify(contract: &Contract, candidate: &Expr) -> Result<ProofResult, String> {
    verify_inner(contract, candidate, &mut None)
}

pub fn verify_with_metrics(
    contract: &Contract,
    candidate: &Expr,
) -> Result<(ProofResult, VerificationMetrics), String> {
    let mut metrics = VerificationMetrics::default();
    let result = verify_inner(contract, candidate, &mut Some(&mut metrics))?;
    Ok((result, metrics))
}

fn verify_inner(
    contract: &Contract,
    candidate: &Expr,
    metrics: &mut Option<&mut VerificationMetrics>,
) -> Result<ProofResult, String> {
    match check_preconditions_inner(contract, metrics)? {
        PreconditionResult::Satisfiable => {}
        PreconditionResult::UnsatisfiablePrecondition => {
            return Err(
                "UNSATISFIABLE_PRECONDITION: requires clauses are contradictory".to_string(),
            );
        }
        PreconditionResult::SolverUnknown => {
            return Err("precondition satisfiability is unknown".to_string());
        }
    }

    if !matches!(contract.return_type, Type::U32 | Type::I32 | Type::Bool) {
        return Err("unsupported contract return type".to_string());
    }

    let args = symbolic_args(contract)?;

    let candidate_ast = expr_to_bv(candidate, &args)?;
    let empty_locals = HashMap::new();
    let requires = preconditions_to_bool(contract, &args, &candidate_ast)?;

    if contract.ensures.is_empty() {
        return Err("contract must have at least one ensures clause".to_string());
    }

    let ensures = contract
        .ensures
        .iter()
        .map(
            |expr| match spec_to_value(expr, contract, &args, &candidate_ast, &empty_locals)? {
                Value::Bool(value) => Ok(value),
                Value::U32(_) | Value::I32(_) => Err("ensures clause must be boolean".to_string()),
            },
        )
        .collect::<Result<Vec<_>, String>>()?;
    let refs: Vec<&Bool> = ensures.iter().collect();
    let violation = requires & Bool::and(&refs).not();

    let solver = Solver::new();
    constrain_bool_parameters(&solver, contract, &args);
    solver.assert(&violation);
    match solver_check(&solver, metrics) {
        SatResult::Unsat => Ok(ProofResult::Verified),
        SatResult::Unknown => Ok(ProofResult::SolverUnknown),
        SatResult::Sat => {
            let model = solver
                .get_model()
                .ok_or_else(|| "Z3 returned SAT without a model".to_string())?;
            let values = args
                .iter()
                .map(|arg| {
                    model
                        .eval(arg, true)
                        .and_then(|value| value.as_u64())
                        .map(|value| value as u32)
                        .ok_or_else(|| "Z3 model did not contain a u32 argument".to_string())
                })
                .collect::<Result<Vec<_>, String>>()?;
            Ok(ProofResult::Counterexample(values))
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompositionalProof {
    /// No proof result is produced when a call-site precondition fails. That
    /// diagnostic is an obligation failure, not a counterexample to ensures.
    pub proof: Option<ProofResult>,
    pub diagnostic: Option<Diagnostic>,
}

struct CallObligation {
    callee: String,
    violation: Bool,
    span: Option<Span>,
}

struct CallContext {
    values: HashMap<String, Value>,
    constraints: Vec<Bool>,
    obligations: Vec<CallObligation>,
    next_id: usize,
    source_span: Option<Span>,
}

/// Verifies a caller against callee summaries without expanding callee bodies.
/// The caller must prove each active callee precondition; only the callee
/// postcondition is assumed at the call boundary.
pub fn verify_compositional(
    contract: &Contract,
    candidate: &Expr,
    callees: &HashMap<String, Contract>,
) -> Result<CompositionalProof, String> {
    verify_compositional_inner(contract, candidate, callees, &mut None)
}

pub fn verify_compositional_with_metrics(
    contract: &Contract,
    candidate: &Expr,
    callees: &HashMap<String, Contract>,
) -> Result<(CompositionalProof, VerificationMetrics), String> {
    let mut metrics = VerificationMetrics::default();
    let proof = verify_compositional_inner(contract, candidate, callees, &mut Some(&mut metrics))?;
    Ok((proof, metrics))
}

fn verify_compositional_inner(
    contract: &Contract,
    candidate: &Expr,
    callees: &HashMap<String, Contract>,
    metrics: &mut Option<&mut VerificationMetrics>,
) -> Result<CompositionalProof, String> {
    if contract.ensures.is_empty() {
        return Err("contract must have at least one ensures clause".to_string());
    }
    let args = symbolic_args(contract)?;
    let result = expr_to_bv(candidate, &args)?;
    let empty_locals = HashMap::new();
    let true_path = Bool::from_bool(true);
    let mut calls = CallContext {
        values: HashMap::new(),
        constraints: Vec::new(),
        obligations: Vec::new(),
        next_id: 0,
        source_span: None,
    };

    let mut requires_values = Vec::new();
    for (index, clause) in contract.requires.iter().enumerate() {
        calls.source_span = contract.requires_spans.get(index).copied();
        let prepared = prepare_calls(
            clause,
            contract,
            &args,
            &result,
            &empty_locals,
            &true_path,
            callees,
            &mut calls,
        )?;
        let locals = merged_call_values(&empty_locals, &calls.values);
        match spec_to_value(&prepared, contract, &args, &result, &locals)? {
            Value::Bool(value) => requires_values.push(value),
            Value::U32(_) | Value::I32(_) => {
                return Err("requires clause must be boolean".to_string());
            }
        }
    }
    let requires_refs: Vec<&Bool> = requires_values.iter().collect();
    let requires = Bool::and(&requires_refs);
    let caller_solver = solver_with_calls(contract, &args, &calls.constraints);
    caller_solver.assert(&requires);
    match solver_check(&caller_solver, metrics) {
        SatResult::Unsat => {
            return Err(
                "UNSATISFIABLE_PRECONDITION: requires clauses are contradictory".to_string(),
            );
        }
        SatResult::Unknown => {
            return Ok(CompositionalProof {
                proof: Some(ProofResult::SolverUnknown),
                diagnostic: None,
            });
        }
        SatResult::Sat => {}
    }

    let mut ensures_values = Vec::new();
    for (index, clause) in contract.ensures.iter().enumerate() {
        calls.source_span = contract.ensures_spans.get(index).copied();
        let prepared = prepare_calls(
            clause,
            contract,
            &args,
            &result,
            &empty_locals,
            &true_path,
            callees,
            &mut calls,
        )?;
        let locals = merged_call_values(&empty_locals, &calls.values);
        match spec_to_value(&prepared, contract, &args, &result, &locals)? {
            Value::Bool(value) => ensures_values.push(value),
            Value::U32(_) | Value::I32(_) => {
                return Err("ensures clause must be boolean".to_string());
            }
        }
    }
    let ensures_refs: Vec<&Bool> = ensures_values.iter().collect();
    let ensures = Bool::and(&ensures_refs);

    for obligation in &calls.obligations {
        let solver = solver_with_calls(contract, &args, &calls.constraints);
        solver.assert(&requires);
        solver.assert(&obligation.violation);
        match solver_check(&solver, metrics) {
            SatResult::Sat => {
                return Ok(CompositionalProof {
                    proof: None,
                    diagnostic: Some(Diagnostic::CallPreconditionViolation {
                        callee: obligation.callee.clone(),
                        span: obligation.span,
                    }),
                });
            }
            SatResult::Unknown => {
                return Ok(CompositionalProof {
                    proof: Some(ProofResult::SolverUnknown),
                    diagnostic: None,
                });
            }
            SatResult::Unsat => {}
        }
    }

    let solver = solver_with_calls(contract, &args, &calls.constraints);
    solver.assert(requires & ensures.not());
    match solver_check(&solver, metrics) {
        SatResult::Unsat => Ok(CompositionalProof {
            proof: Some(ProofResult::Verified),
            diagnostic: None,
        }),
        SatResult::Unknown => Ok(CompositionalProof {
            proof: Some(ProofResult::SolverUnknown),
            diagnostic: None,
        }),
        SatResult::Sat => Ok(CompositionalProof {
            proof: Some(ProofResult::Counterexample(model_inputs(&solver, &args)?)),
            diagnostic: None,
        }),
    }
}

fn solver_with_calls(contract: &Contract, args: &[BV], constraints: &[Bool]) -> Solver {
    let solver = Solver::new();
    constrain_bool_parameters(&solver, contract, args);
    for constraint in constraints {
        solver.assert(constraint);
    }
    solver
}

fn model_inputs(solver: &Solver, args: &[BV]) -> Result<Vec<u32>, String> {
    let model = solver
        .get_model()
        .ok_or_else(|| "Z3 returned SAT without a model".to_string())?;
    args.iter()
        .map(|arg| {
            model
                .eval(arg, true)
                .and_then(|value| value.as_u64())
                .map(|value| value as u32)
                .ok_or_else(|| "Z3 model did not contain a u32 argument".to_string())
        })
        .collect()
}

fn merged_call_values(
    locals: &HashMap<String, Value>,
    calls: &HashMap<String, Value>,
) -> HashMap<String, Value> {
    let mut merged = locals.clone();
    merged.extend(calls.clone());
    merged
}

#[allow(clippy::too_many_arguments)]
fn prepare_calls(
    expr: &SpecExpr,
    contract: &Contract,
    args: &[BV],
    result: &BV,
    locals: &HashMap<String, Value>,
    path: &Bool,
    callees: &HashMap<String, Contract>,
    calls: &mut CallContext,
) -> Result<SpecExpr, String> {
    match expr {
        SpecExpr::Call(name, call_args) => {
            let callee = callees
                .get(name)
                .ok_or_else(|| format!("unknown callee: {name}"))?;
            if callee.kind == axiom_parser::DeclarationKind::Extern {
                return Err(format!(
                    "extern callee {name} requires a runtime adapter, not proof composition"
                ));
            }
            if callee.params.len() != call_args.len() {
                return Err(format!(
                    "callee {name} expects {} arguments, got {}",
                    callee.params.len(),
                    call_args.len()
                ));
            }
            let prepared_args = call_args
                .iter()
                .map(|arg| prepare_calls(arg, contract, args, result, locals, path, callees, calls))
                .collect::<Result<Vec<_>, _>>()?;
            let merged = merged_call_values(locals, &calls.values);
            let arg_values = prepared_args
                .iter()
                .map(|arg| spec_to_value(arg, contract, args, result, &merged))
                .collect::<Result<Vec<_>, _>>()?;
            let callee_args = arg_values.iter().map(value_as_bv).collect::<Vec<_>>();
            let call_result = BV::new_const(format!("__axiom_call_{}", calls.next_id), 32);
            calls.next_id += 1;
            let callee_requires = preconditions_to_bool(callee, &callee_args, &call_result)?;
            let callee_ensures = ensures_to_bool(callee, &callee_args, &call_result)?;
            let active = Bool::and(&[path, &callee_requires]);
            calls
                .constraints
                .push(Bool::or(&[&active.not(), &callee_ensures]));
            calls.obligations.push(CallObligation {
                callee: name.clone(),
                violation: Bool::and(&[path, &callee_requires.not()]),
                span: calls.source_span,
            });
            let symbol = format!("__axiom_call_value_{}", calls.next_id);
            let value = match callee.return_type {
                Type::Bool => Value::Bool(call_result.ne(BV::from_u64(0, 32))),
                Type::I32 => Value::I32(call_result),
                Type::U32 => Value::U32(call_result),
                _ => return Err(format!("callee {name} has a non-scalar return type")),
            };
            calls.values.insert(symbol.clone(), value);
            Ok(SpecExpr::Ident(symbol))
        }
        SpecExpr::Let(name, value, body) => Ok(SpecExpr::Let(
            name.clone(),
            Box::new(prepare_calls(
                value, contract, args, result, locals, path, callees, calls,
            )?),
            Box::new(prepare_calls(
                body, contract, args, result, locals, path, callees, calls,
            )?),
        )),
        SpecExpr::If(condition, then_branch, else_branch) => {
            let prepared_condition = prepare_calls(
                condition, contract, args, result, locals, path, callees, calls,
            )?;
            let merged = merged_call_values(locals, &calls.values);
            let condition_value =
                spec_to_value(&prepared_condition, contract, args, result, &merged)?;
            let condition = match condition_value {
                Value::Bool(value) => value,
                Value::U32(_) | Value::I32(_) => {
                    return Err("if condition must be boolean".to_string());
                }
            };
            let then_path = Bool::and(&[path, &condition]);
            let else_path = Bool::and(&[path, &condition.not()]);
            Ok(SpecExpr::If(
                Box::new(prepared_condition),
                Box::new(prepare_calls(
                    then_branch,
                    contract,
                    args,
                    result,
                    locals,
                    &then_path,
                    callees,
                    calls,
                )?),
                Box::new(prepare_calls(
                    else_branch,
                    contract,
                    args,
                    result,
                    locals,
                    &else_path,
                    callees,
                    calls,
                )?),
            ))
        }
        SpecExpr::Binary(op, lhs, rhs) => Ok(SpecExpr::Binary(
            op.clone(),
            Box::new(prepare_calls(
                lhs, contract, args, result, locals, path, callees, calls,
            )?),
            Box::new(prepare_calls(
                rhs, contract, args, result, locals, path, callees, calls,
            )?),
        )),
        SpecExpr::List(values) => Ok(SpecExpr::List(
            values
                .iter()
                .map(|value| {
                    prepare_calls(value, contract, args, result, locals, path, callees, calls)
                })
                .collect::<Result<_, _>>()?,
        )),
        SpecExpr::Record { name, fields } => Ok(SpecExpr::Record {
            name: name.clone(),
            fields: fields
                .iter()
                .map(|(field, value)| {
                    Ok((
                        field.clone(),
                        prepare_calls(value, contract, args, result, locals, path, callees, calls)?,
                    ))
                })
                .collect::<Result<_, String>>()?,
        }),
        SpecExpr::Field(value, field) => Ok(SpecExpr::Field(
            Box::new(prepare_calls(
                value, contract, args, result, locals, path, callees, calls,
            )?),
            field.clone(),
        )),
        SpecExpr::Ident(_) | SpecExpr::Number(_) | SpecExpr::Bool(_) | SpecExpr::Text(_) => {
            Ok(expr.clone())
        }
    }
}

fn value_as_bv(value: &Value) -> BV {
    match value {
        Value::U32(value) | Value::I32(value) => value.clone(),
        Value::Bool(value) => value.ite(&BV::from_u64(1, 32), &BV::from_u64(0, 32)),
    }
}

fn symbolic_args(contract: &Contract) -> Result<Vec<BV>, String> {
    contract
        .params
        .iter()
        .map(|param| {
            if !matches!(param.ty, Type::U32 | Type::I32 | Type::Bool) {
                return Err(format!("unsupported parameter type: {}", param.name));
            }
            Ok(BV::new_const(param.name.as_str(), 32))
        })
        .collect()
}

fn constrain_bool_parameters(solver: &Solver, contract: &Contract, args: &[BV]) {
    for (index, param) in contract.params.iter().enumerate() {
        if param.ty == Type::Bool {
            let zero = args[index].eq(BV::from_u64(0, 32));
            let one = args[index].eq(BV::from_u64(1, 32));
            solver.assert(Bool::or(&[&zero, &one]));
        }
    }
}

fn preconditions_to_bool(contract: &Contract, args: &[BV], result: &BV) -> Result<Bool, String> {
    if contract.requires.is_empty() {
        return Ok(Bool::from_bool(true));
    }

    let empty_locals = HashMap::new();
    let clauses = contract
        .requires
        .iter()
        .map(
            |expr| match spec_to_value(expr, contract, args, result, &empty_locals)? {
                Value::Bool(value) => Ok(value),
                Value::U32(_) | Value::I32(_) => Err("requires clause must be boolean".to_string()),
            },
        )
        .collect::<Result<Vec<_>, String>>()?;
    let refs: Vec<&Bool> = clauses.iter().collect();
    Ok(Bool::and(&refs))
}

fn ensures_to_bool(contract: &Contract, args: &[BV], result: &BV) -> Result<Bool, String> {
    if contract.ensures.is_empty() {
        return Err("contract must have at least one ensures clause".to_string());
    }

    let empty_locals = HashMap::new();
    let clauses = contract
        .ensures
        .iter()
        .map(
            |expr| match spec_to_value(expr, contract, args, result, &empty_locals)? {
                Value::Bool(value) => Ok(value),
                Value::U32(_) | Value::I32(_) => Err("ensures clause must be boolean".to_string()),
            },
        )
        .collect::<Result<Vec<_>, String>>()?;
    let refs: Vec<&Bool> = clauses.iter().collect();
    Ok(Bool::and(&refs))
}

fn expr_to_bv(expr: &Expr, args: &[BV]) -> Result<BV, String> {
    match expr {
        Expr::Arg(index) => args
            .get(*index as usize)
            .cloned()
            .ok_or_else(|| format!("candidate references missing argument {}", index)),
        Expr::Const(value) => Ok(BV::from_u64(*value as u64, 32)),
        Expr::Add(lhs, rhs) => Ok(&expr_to_bv(lhs, args)? + &expr_to_bv(rhs, args)?),
        Expr::Sub(lhs, rhs) => Ok(&expr_to_bv(lhs, args)? - &expr_to_bv(rhs, args)?),
        Expr::Mul(lhs, rhs) => Ok(&expr_to_bv(lhs, args)? * &expr_to_bv(rhs, args)?),
        Expr::Shl(lhs, rhs) => {
            let lhs = expr_to_bv(lhs, args)?;
            let rhs = expr_to_bv(rhs, args)?;
            Ok(lhs.bvshl(rhs.bvand(BV::from_u64(31, 32))))
        }
        Expr::BitAnd(lhs, rhs) => Ok(expr_to_bv(lhs, args)?.bvand(expr_to_bv(rhs, args)?)),
        Expr::BitOr(lhs, rhs) => Ok(expr_to_bv(lhs, args)?.bvor(expr_to_bv(rhs, args)?)),
        Expr::BitXor(lhs, rhs) => Ok(expr_to_bv(lhs, args)?.bvxor(expr_to_bv(rhs, args)?)),
        Expr::Eq(_, _)
        | Expr::Ne(_, _)
        | Expr::Lt(_, _)
        | Expr::Le(_, _)
        | Expr::Gt(_, _)
        | Expr::Ge(_, _)
        | Expr::SLt(_, _)
        | Expr::SLe(_, _)
        | Expr::SGt(_, _)
        | Expr::SGe(_, _)
        | Expr::And(_, _)
        | Expr::Or(_, _) => Ok(bool_to_bv(expr_to_bool(expr, args)?)),
        Expr::Select {
            condition,
            then_branch,
            else_branch,
        } => {
            let condition = expr_to_bool(condition, args)?;
            let then_branch = expr_to_bv(then_branch, args)?;
            let else_branch = expr_to_bv(else_branch, args)?;
            Ok(condition.ite(&then_branch, &else_branch))
        }
    }
}

fn expr_to_bool(expr: &Expr, args: &[BV]) -> Result<Bool, String> {
    match expr {
        Expr::Eq(lhs, rhs) => Ok(expr_to_bv(lhs, args)?.eq(expr_to_bv(rhs, args)?)),
        Expr::Ne(lhs, rhs) => Ok(expr_to_bv(lhs, args)?.ne(expr_to_bv(rhs, args)?)),
        Expr::Lt(lhs, rhs) => Ok(expr_to_bv(lhs, args)?.bvult(&expr_to_bv(rhs, args)?)),
        Expr::Le(lhs, rhs) => Ok(expr_to_bv(lhs, args)?.bvule(&expr_to_bv(rhs, args)?)),
        Expr::Gt(lhs, rhs) => Ok(expr_to_bv(lhs, args)?.bvugt(&expr_to_bv(rhs, args)?)),
        Expr::Ge(lhs, rhs) => Ok(expr_to_bv(lhs, args)?.bvuge(&expr_to_bv(rhs, args)?)),
        Expr::SLt(lhs, rhs) => Ok(expr_to_bv(lhs, args)?.bvslt(&expr_to_bv(rhs, args)?)),
        Expr::SLe(lhs, rhs) => Ok(expr_to_bv(lhs, args)?.bvsle(&expr_to_bv(rhs, args)?)),
        Expr::SGt(lhs, rhs) => Ok(expr_to_bv(lhs, args)?.bvsgt(&expr_to_bv(rhs, args)?)),
        Expr::SGe(lhs, rhs) => Ok(expr_to_bv(lhs, args)?.bvsge(&expr_to_bv(rhs, args)?)),
        Expr::And(lhs, rhs) => Ok(&expr_to_bool(lhs, args)? & &expr_to_bool(rhs, args)?),
        Expr::Or(lhs, rhs) => Ok(&expr_to_bool(lhs, args)? | &expr_to_bool(rhs, args)?),
        _ => Ok(expr_to_bv(expr, args)?.ne(BV::from_u64(0, 32))),
    }
}

fn bool_to_bv(value: Bool) -> BV {
    value.ite(&BV::from_u64(1, 32), &BV::from_u64(0, 32))
}

fn spec_to_value(
    expr: &SpecExpr,
    contract: &Contract,
    args: &[BV],
    result: &BV,
    locals: &HashMap<String, Value>,
) -> Result<Value, String> {
    match expr {
        SpecExpr::Ident(name) if name == "result" => {
            if contract.return_type == Type::Bool {
                Ok(Value::Bool(result.ne(BV::from_u64(0, 32))))
            } else if contract.return_type == Type::I32 {
                Ok(Value::I32(result.clone()))
            } else {
                Ok(Value::U32(result.clone()))
            }
        }
        SpecExpr::Ident(name) if locals.contains_key(name) => Ok(locals[name].clone()),
        SpecExpr::Ident(name) => {
            let index = contract
                .params
                .iter()
                .position(|param| param.name == *name)
                .ok_or_else(|| format!("unknown identifier: {name}"))?;
            let value = args
                .get(index)
                .cloned()
                .ok_or_else(|| format!("missing argument: {name}"))?;
            match contract.params[index].ty {
                Type::Bool => Ok(Value::Bool(value.ne(BV::from_u64(0, 32)))),
                Type::I32 => Ok(Value::I32(value)),
                _ => Ok(Value::U32(value)),
            }
        }
        SpecExpr::Number(value) => Ok(Value::U32(BV::from_u64(*value as u64, 32))),
        SpecExpr::Bool(value) => Ok(Value::Bool(Bool::from_bool(*value))),
        SpecExpr::Text(_) | SpecExpr::List(_) | SpecExpr::Record { .. } | SpecExpr::Field(_, _) => {
            Err("non-scalar expressions cannot be verified as contracts".to_string())
        }
        SpecExpr::Let(name, value, body) => {
            let value = spec_to_value(value, contract, args, result, locals)?;
            let mut nested = locals.clone();
            nested.insert(name.clone(), value);
            spec_to_value(body, contract, args, result, &nested)
        }
        SpecExpr::If(condition, then_branch, else_branch) => {
            let condition = match spec_to_value(condition, contract, args, result, locals)? {
                Value::Bool(value) => value,
                Value::U32(_) | Value::I32(_) => {
                    return Err("if condition must be boolean".to_string());
                }
            };
            let then_branch = spec_to_value(then_branch, contract, args, result, locals)?;
            let else_branch = spec_to_value(else_branch, contract, args, result, locals)?;
            match (then_branch, else_branch) {
                (Value::U32(then_branch), Value::U32(else_branch)) => {
                    Ok(Value::U32(condition.ite(&then_branch, &else_branch)))
                }
                (Value::Bool(then_branch), Value::Bool(else_branch)) => {
                    Ok(Value::Bool(condition.ite(&then_branch, &else_branch)))
                }
                (Value::I32(then_branch), Value::I32(else_branch)) => {
                    Ok(Value::I32(condition.ite(&then_branch, &else_branch)))
                }
                _ => Err("if branches must have the same type".to_string()),
            }
        }
        SpecExpr::Call(name, _) => Err(format!(
            "unresolved contract call: {name}; link the project before verification"
        )),
        SpecExpr::Binary(op, lhs, rhs) => {
            let lhs = spec_to_value(lhs, contract, args, result, locals)?;
            let rhs = spec_to_value(rhs, contract, args, result, locals)?;
            match (op, lhs, rhs) {
                (BinaryOp::AddWrap, Value::U32(lhs), Value::U32(rhs)) => {
                    Ok(Value::U32(&lhs + &rhs))
                }
                (BinaryOp::AddWrap, Value::I32(lhs), Value::I32(rhs))
                | (BinaryOp::AddWrap, Value::I32(lhs), Value::U32(rhs))
                | (BinaryOp::AddWrap, Value::U32(lhs), Value::I32(rhs)) => {
                    Ok(Value::I32(&lhs + &rhs))
                }
                (BinaryOp::SubWrap, Value::U32(lhs), Value::U32(rhs)) => {
                    Ok(Value::U32(&lhs - &rhs))
                }
                (BinaryOp::SubWrap, Value::I32(lhs), Value::I32(rhs))
                | (BinaryOp::SubWrap, Value::I32(lhs), Value::U32(rhs))
                | (BinaryOp::SubWrap, Value::U32(lhs), Value::I32(rhs)) => {
                    Ok(Value::I32(&lhs - &rhs))
                }
                (BinaryOp::MulWrap, Value::U32(lhs), Value::U32(rhs)) => {
                    Ok(Value::U32(&lhs * &rhs))
                }
                (BinaryOp::MulWrap, Value::I32(lhs), Value::I32(rhs))
                | (BinaryOp::MulWrap, Value::I32(lhs), Value::U32(rhs))
                | (BinaryOp::MulWrap, Value::U32(lhs), Value::I32(rhs)) => {
                    Ok(Value::I32(&lhs * &rhs))
                }
                (BinaryOp::Shl, Value::U32(lhs), Value::U32(rhs)) => {
                    Ok(Value::U32(lhs.bvshl(rhs.bvand(BV::from_u64(31, 32)))))
                }
                (BinaryOp::Shl, Value::I32(lhs), Value::I32(rhs))
                | (BinaryOp::Shl, Value::I32(lhs), Value::U32(rhs))
                | (BinaryOp::Shl, Value::U32(lhs), Value::I32(rhs)) => {
                    Ok(Value::I32(lhs.bvshl(rhs.bvand(BV::from_u64(31, 32)))))
                }
                (BinaryOp::BitAnd, Value::U32(lhs), Value::U32(rhs)) => {
                    Ok(Value::U32(lhs.bvand(rhs)))
                }
                (BinaryOp::BitAnd, Value::I32(lhs), Value::I32(rhs))
                | (BinaryOp::BitAnd, Value::I32(lhs), Value::U32(rhs))
                | (BinaryOp::BitAnd, Value::U32(lhs), Value::I32(rhs)) => {
                    Ok(Value::I32(lhs.bvand(rhs)))
                }
                (BinaryOp::BitOr, Value::U32(lhs), Value::U32(rhs)) => {
                    Ok(Value::U32(lhs.bvor(rhs)))
                }
                (BinaryOp::BitOr, Value::I32(lhs), Value::I32(rhs))
                | (BinaryOp::BitOr, Value::I32(lhs), Value::U32(rhs))
                | (BinaryOp::BitOr, Value::U32(lhs), Value::I32(rhs)) => {
                    Ok(Value::I32(lhs.bvor(rhs)))
                }
                (BinaryOp::BitXor, Value::U32(lhs), Value::U32(rhs)) => {
                    Ok(Value::U32(lhs.bvxor(rhs)))
                }
                (BinaryOp::BitXor, Value::I32(lhs), Value::I32(rhs))
                | (BinaryOp::BitXor, Value::I32(lhs), Value::U32(rhs))
                | (BinaryOp::BitXor, Value::U32(lhs), Value::I32(rhs)) => {
                    Ok(Value::I32(lhs.bvxor(rhs)))
                }
                (BinaryOp::Eq, Value::U32(lhs), Value::U32(rhs)) => Ok(Value::Bool(lhs.eq(rhs))),
                (BinaryOp::Ne, Value::U32(lhs), Value::U32(rhs)) => Ok(Value::Bool(lhs.ne(rhs))),
                (BinaryOp::Eq, Value::I32(lhs), Value::I32(rhs))
                | (BinaryOp::Eq, Value::I32(lhs), Value::U32(rhs))
                | (BinaryOp::Eq, Value::U32(lhs), Value::I32(rhs)) => Ok(Value::Bool(lhs.eq(rhs))),
                (BinaryOp::Ne, Value::I32(lhs), Value::I32(rhs))
                | (BinaryOp::Ne, Value::I32(lhs), Value::U32(rhs))
                | (BinaryOp::Ne, Value::U32(lhs), Value::I32(rhs)) => Ok(Value::Bool(lhs.ne(rhs))),
                (BinaryOp::Eq, Value::Bool(lhs), Value::Bool(rhs)) => Ok(Value::Bool(lhs.eq(rhs))),
                (BinaryOp::Ne, Value::Bool(lhs), Value::Bool(rhs)) => Ok(Value::Bool(lhs.ne(rhs))),
                (BinaryOp::Lt, Value::U32(lhs), Value::U32(rhs)) => {
                    Ok(Value::Bool(lhs.bvult(&rhs)))
                }
                (BinaryOp::Le, Value::U32(lhs), Value::U32(rhs)) => {
                    Ok(Value::Bool(lhs.bvule(&rhs)))
                }
                (BinaryOp::Gt, Value::U32(lhs), Value::U32(rhs)) => {
                    Ok(Value::Bool(lhs.bvugt(&rhs)))
                }
                (BinaryOp::Ge, Value::U32(lhs), Value::U32(rhs)) => {
                    Ok(Value::Bool(lhs.bvuge(&rhs)))
                }
                (BinaryOp::Lt, Value::I32(lhs), Value::I32(rhs))
                | (BinaryOp::Lt, Value::I32(lhs), Value::U32(rhs))
                | (BinaryOp::Lt, Value::U32(lhs), Value::I32(rhs)) => {
                    Ok(Value::Bool(lhs.bvslt(&rhs)))
                }
                (BinaryOp::Le, Value::I32(lhs), Value::I32(rhs))
                | (BinaryOp::Le, Value::I32(lhs), Value::U32(rhs))
                | (BinaryOp::Le, Value::U32(lhs), Value::I32(rhs)) => {
                    Ok(Value::Bool(lhs.bvsle(&rhs)))
                }
                (BinaryOp::Gt, Value::I32(lhs), Value::I32(rhs))
                | (BinaryOp::Gt, Value::I32(lhs), Value::U32(rhs))
                | (BinaryOp::Gt, Value::U32(lhs), Value::I32(rhs)) => {
                    Ok(Value::Bool(lhs.bvsgt(&rhs)))
                }
                (BinaryOp::Ge, Value::I32(lhs), Value::I32(rhs))
                | (BinaryOp::Ge, Value::I32(lhs), Value::U32(rhs))
                | (BinaryOp::Ge, Value::U32(lhs), Value::I32(rhs)) => {
                    Ok(Value::Bool(lhs.bvsge(&rhs)))
                }
                (BinaryOp::And, Value::Bool(lhs), Value::Bool(rhs)) => Ok(Value::Bool(&lhs & &rhs)),
                (BinaryOp::Or, Value::Bool(lhs), Value::Bool(rhs)) => Ok(Value::Bool(&lhs | &rhs)),
                _ => Err(format!("operator {:?} has incompatible operands", op)),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axiom_parser::parse;

    #[test]
    fn proves_mul8_shift_candidate() {
        let contract = parse(include_str!("../../../examples/basics/mul8.ax")).unwrap();
        let candidate = Expr::Shl(Box::new(Expr::Arg(0)), Box::new(Expr::Const(3)));

        assert_eq!(
            verify(&contract, &candidate).unwrap(),
            VerificationResult::Verified
        );
    }

    #[test]
    fn returns_counterexample_for_wrong_candidate() {
        let contract = parse(include_str!("../../../examples/basics/mul8.ax")).unwrap();
        let candidate = Expr::Arg(0);

        match verify(&contract, &candidate).unwrap() {
            VerificationResult::Counterexample(values) => assert_eq!(values.len(), 1),
            other => panic!("expected counterexample, got {other:?}"),
        }
    }

    #[test]
    fn detects_unsatisfiable_preconditions_before_proof() {
        let contract = parse(include_str!(
            "../../../examples/errors/unsat_precondition.ax"
        ))
        .unwrap();

        assert_eq!(
            check_preconditions(&contract).unwrap(),
            PreconditionResult::UnsatisfiablePrecondition
        );
        let error = verify(&contract, &Expr::Arg(0)).unwrap_err();
        assert!(error.starts_with("UNSATISFIABLE_PRECONDITION"));
    }

    #[test]
    fn accepts_a_satisfiable_precondition() {
        let contract = parse(
            "contract guarded(x: u32) -> u32 { \
             requires x > 10; \
             ensures result == x; }",
        )
        .unwrap();

        assert_eq!(
            check_preconditions(&contract).unwrap(),
            PreconditionResult::Satisfiable
        );
    }

    #[test]
    fn classifies_contract_validity_and_unsatisfiable_postconditions() {
        let valid = parse(include_str!("../../../examples/basics/mul8.ax")).unwrap();
        assert_eq!(check_contract(&valid).unwrap(), ContractCheck::Valid);

        let impossible = parse(include_str!(
            "../../../examples/errors/unsat_postcondition.ax"
        ))
        .unwrap();
        assert_eq!(
            check_contract(&impossible).unwrap(),
            ContractCheck::UnsatisfiablePostcondition
        );

        let invalid = Contract {
            kind: axiom_parser::DeclarationKind::Contract,
            exported: false,
            name: "invalid".to_string(),
            params: Vec::new(),
            return_type: Type::Text,
            implementation: None,
            requires: Vec::new(),
            ensures: vec![SpecExpr::Bool(true)],
            requires_spans: Vec::new(),
            ensures_spans: Vec::new(),
        };
        assert!(matches!(
            check_contract(&invalid).unwrap(),
            ContractCheck::Invalid(_)
        ));
    }

    #[test]
    fn verifies_fn_implementation_and_accepts_extern_boundary() {
        let function = parse(
            "fn increment(value: u32) -> u32 { \
             return value +% 1; \
             ensures result == value +% 1; }",
        )
        .unwrap();
        assert_eq!(check_contract(&function).unwrap(), ContractCheck::Valid);
        let candidate = implementation_to_ami(&function).unwrap();
        assert_eq!(candidate.eval(&[u32::MAX]), 0);
        assert_eq!(
            verify(&function, &candidate).unwrap(),
            ProofResult::Verified
        );

        let external = parse("extern fn read_count(key: u32) -> u32;").unwrap();
        assert_eq!(check_contract(&external).unwrap(), ContractCheck::Valid);
        assert!(implementation_to_ami(&external).is_err());
    }

    #[test]
    fn rejects_fn_implementation_that_breaks_its_contract() {
        let function = parse(
            "fn broken(value: u32) -> u32 { \
             return value; \
             ensures result == value +% 1; }",
        )
        .unwrap();

        assert!(matches!(
            check_contract(&function).unwrap(),
            ContractCheck::Invalid(_)
        ));
    }

    #[test]
    fn verifies_calls_from_callee_summaries_without_inlining() {
        let callee =
            parse("contract add_one(value: u32) -> u32 { ensures result == value +% 1; }").unwrap();
        let caller_module = axiom_parser::parse_module(
            "contract total(value: u32) -> u32 { ensures result == add_one(value); }",
        )
        .unwrap();
        let caller = &caller_module.contracts[0];
        let candidate = Expr::Add(Box::new(Expr::Arg(0)), Box::new(Expr::Const(1)));
        let callees = HashMap::from([(callee.name.clone(), callee)]);

        let result = verify_compositional(caller, &candidate, &callees).unwrap();
        assert_eq!(result.proof, Some(ProofResult::Verified));
        assert_eq!(result.diagnostic, None);
    }

    #[test]
    fn reports_call_site_precondition_separately() {
        let callee = parse(
            "contract positive(value: u32) -> u32 { \
             requires value > 0; \
             ensures result == value; }",
        )
        .unwrap();
        let caller_module = axiom_parser::parse_module(
            "contract total(value: u32) -> u32 { ensures result == positive(value); }",
        )
        .unwrap();
        let caller = &caller_module.contracts[0];
        let callees = HashMap::from([(callee.name.clone(), callee)]);

        let result = verify_compositional(caller, &Expr::Arg(0), &callees).unwrap();
        assert_eq!(result.proof, None);
        assert!(matches!(
            result.diagnostic,
            Some(Diagnostic::CallPreconditionViolation { callee, span: Some(span) })
                if callee == "positive" && span.line == 1 && span.column > 0
        ));
    }

    #[test]
    fn reports_only_postcondition_models_as_counterexamples() {
        let caller_module = axiom_parser::parse_module(
            "contract total(value: u32) -> u32 { ensures result == value +% 1; }",
        )
        .unwrap();
        let caller = &caller_module.contracts[0];
        let result = verify_compositional(caller, &Expr::Arg(0), &HashMap::new()).unwrap();

        assert!(matches!(result.proof, Some(ProofResult::Counterexample(_))));
        assert_eq!(result.diagnostic, None);
    }

    #[test]
    fn proves_conditional_specification_for_all_inputs() {
        let contract = parse(include_str!("../../../examples/friendly/easy_min.ax")).unwrap();
        let candidate = Expr::Select {
            condition: Box::new(Expr::Lt(Box::new(Expr::Arg(0)), Box::new(Expr::Arg(1)))),
            then_branch: Box::new(Expr::Arg(0)),
            else_branch: Box::new(Expr::Arg(1)),
        };

        assert_eq!(
            verify(&contract, &candidate).unwrap(),
            VerificationResult::Verified
        );
    }

    #[test]
    fn proves_bool_result_contract() {
        let contract =
            parse("program is_ready(value: number) -> bool { gives answer equals value > 0; }")
                .unwrap();
        let candidate = Expr::Gt(Box::new(Expr::Arg(0)), Box::new(Expr::Const(0)));

        assert_eq!(
            verify(&contract, &candidate).unwrap(),
            VerificationResult::Verified
        );
    }

    #[test]
    fn proves_signed_conditional_specification() {
        let contract = parse(include_str!("../../../examples/basics/signed_min.ax")).unwrap();
        let candidate = Expr::Select {
            condition: Box::new(Expr::SLt(Box::new(Expr::Arg(0)), Box::new(Expr::Arg(1)))),
            then_branch: Box::new(Expr::Arg(0)),
            else_branch: Box::new(Expr::Arg(1)),
        };

        assert_eq!(
            verify(&contract, &candidate).unwrap(),
            VerificationResult::Verified
        );
    }

    #[test]
    fn proves_scalar_bitwise_and_subtraction() {
        let contract = parse(
            "contract mask(value: u32, bits: u32) -> u32 { ensures result == (value & bits) ^ (value -% 1); }",
        )
        .unwrap();
        let candidate = Expr::BitXor(
            Box::new(Expr::BitAnd(Box::new(Expr::Arg(0)), Box::new(Expr::Arg(1)))),
            Box::new(Expr::Sub(Box::new(Expr::Arg(0)), Box::new(Expr::Const(1)))),
        );

        assert_eq!(
            verify(&contract, &candidate).unwrap(),
            VerificationResult::Verified
        );
    }
}
