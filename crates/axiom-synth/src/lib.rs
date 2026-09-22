//! Minimal enumerative synthesizer for the first `mul8` vertical slice.

use std::{
    collections::{HashMap, HashSet},
    time::{Duration, Instant},
};

use axiom_ami::Expr;
use axiom_parser::{BinaryOp, Contract, SpecExpr, Type};
use axiom_verify::{VerificationResult, verify};

#[derive(Debug, Clone, Copy)]
pub struct SynthesisOptions {
    pub timeout: Option<Duration>,
    pub max_candidates: usize,
}

impl Default for SynthesisOptions {
    fn default() -> Self {
        Self {
            timeout: None,
            max_candidates: 50_000,
        }
    }
}

/// Finds the first verified candidate in the bounded search space.
pub fn synthesize(contract: &Contract) -> Result<Expr, String> {
    synthesize_with_options(contract, SynthesisOptions::default())
}

/// Finds the cheapest verified candidate within the configured bounded search.
pub fn synthesize_with_options(
    contract: &Contract,
    options: SynthesisOptions,
) -> Result<Expr, String> {
    let started = Instant::now();
    let mut samples = sample_inputs(contract);
    let batches = [
        base_candidates(contract.params.len()),
        depth_one_candidates(contract.params.len()),
        depth_two_candidates(contract.params.len(), options.max_candidates),
    ];
    let mut checked = 0;
    let mut last_counterexample = None;

    if (contract.return_type == Type::Bool
        || contract.ensures.iter().any(contains_conditional_construct))
        && let Some(candidate) = candidate_from_contract(contract)?
        && matches_samples(contract, &candidate, &samples)
    {
        match verify(contract, &candidate)? {
            VerificationResult::Verified => return Ok(candidate),
            VerificationResult::Counterexample(values) => {
                last_counterexample = Some(values);
            }
            VerificationResult::Unknown => {
                return Err("SMT solver returned unknown".to_string());
            }
        }
    }

    for mut candidates in batches {
        candidates.sort_by_key(axiom_wasm::instruction_cost);
        for candidate in candidates {
            if timed_out(started, options.timeout) {
                return Err(timeout_message(options.timeout));
            }
            if !matches_samples(contract, &candidate, &samples) {
                continue;
            }
            checked += 1;
            match verify(contract, &candidate)? {
                VerificationResult::Verified => return Ok(candidate),
                VerificationResult::Counterexample(values) => {
                    last_counterexample = Some(values.clone());
                    if !samples.contains(&values) {
                        samples.push(values);
                    }
                }
                VerificationResult::Unknown => {
                    return Err("SMT solver returned unknown".to_string());
                }
            }
        }
    }

    let counterexample = last_counterexample
        .map(|values| format!("; last counterexample: {values:?}"))
        .unwrap_or_default();
    Err(format!(
        "no verified candidate found in the bounded search space (candidates checked: {checked}{counterexample})"
    ))
}

/// Synthesizes a candidate and lowers the verified result to WebAssembly.
pub fn synthesize_wasm(contract: &Contract) -> Result<(Expr, Vec<u8>), String> {
    synthesize_wasm_with_options(contract, SynthesisOptions::default())
}

/// Synthesizes a candidate and lowers it using explicit search limits.
pub fn synthesize_wasm_with_options(
    contract: &Contract,
    options: SynthesisOptions,
) -> Result<(Expr, Vec<u8>), String> {
    let candidate = synthesize_with_options(contract, options)?;
    let module = axiom_wasm::emit(&candidate, contract.params.len() as u32)?;
    Ok((candidate, module))
}

/// Checks a concrete input against every `requires` clause.
pub fn check_requires(contract: &Contract, args: &[u32]) -> Result<(), String> {
    if args.len() != contract.params.len() {
        return Err(format!(
            "expected {} inputs, got {}",
            contract.params.len(),
            args.len()
        ));
    }

    for expr in &contract.requires {
        match eval_spec(expr, contract, args, 0, &HashMap::new())? {
            Value::Bool(true) => {}
            Value::Bool(false) => return Err("input violates a requires clause".to_string()),
            Value::U32(_) | Value::I32(_) => {
                return Err("requires clause must be boolean".to_string());
            }
        }
    }
    Ok(())
}

fn base_candidates(argument_count: usize) -> Vec<Expr> {
    let mut base = (0..argument_count)
        .map(|index| Expr::Arg(index as u8))
        .collect::<Vec<_>>();
    base.extend((0..=16).map(Expr::Const));
    base
}

fn depth_one_candidates(argument_count: usize) -> Vec<Expr> {
    let base = base_candidates(argument_count);
    combine_candidates(&base, &base)
}

fn depth_two_candidates(argument_count: usize, limit: usize) -> Vec<Expr> {
    // ponytail: cap depth-two enumeration; raise the bound after measuring real workloads.
    let base = base_candidates(argument_count);
    let depth_one = depth_one_candidates(argument_count);
    let mut candidates = combine_candidates(&depth_one, &base);
    candidates.extend(combine_candidates(&base, &depth_one));
    dedupe_and_cap(candidates, limit)
}

fn combine_candidates(lhs: &[Expr], rhs: &[Expr]) -> Vec<Expr> {
    let mut seen = HashSet::new();
    let mut candidates = Vec::new();
    for lhs in lhs {
        for rhs in rhs {
            for candidate in [
                Expr::Shl(Box::new(lhs.clone()), Box::new(rhs.clone())),
                Expr::Mul(Box::new(lhs.clone()), Box::new(rhs.clone())),
                Expr::Add(Box::new(lhs.clone()), Box::new(rhs.clone())),
            ] {
                if seen.insert(candidate.clone()) {
                    candidates.push(candidate);
                }
            }
        }
    }
    candidates
}

fn dedupe_and_cap(candidates: Vec<Expr>, limit: usize) -> Vec<Expr> {
    let mut seen = HashSet::new();
    candidates
        .into_iter()
        .filter(|candidate| seen.insert(candidate.clone()))
        .take(limit)
        .collect()
}

fn timed_out(started: Instant, timeout: Option<Duration>) -> bool {
    timeout.is_some_and(|timeout| started.elapsed() >= timeout)
}

fn timeout_message(timeout: Option<Duration>) -> String {
    match timeout {
        Some(timeout) => format!("synthesis timed out after {} ms", timeout.as_millis()),
        None => "synthesis timed out".to_string(),
    }
}

fn sample_inputs(contract: &Contract) -> Vec<Vec<u32>> {
    const VALUES: [u32; 10] = [
        0,
        1,
        2,
        3,
        7,
        8,
        0x7fff_ffff,
        0x8000_0000,
        u32::MAX - 1,
        u32::MAX,
    ];
    let mut samples = vec![Vec::new()];
    for param in &contract.params {
        let values: &[u32] = if param.ty == Type::Bool {
            &[0, 1]
        } else {
            &VALUES
        };
        samples = samples
            .into_iter()
            .flat_map(|prefix| {
                values.iter().map(move |value| {
                    let mut args = prefix.clone();
                    args.push(*value);
                    args
                })
            })
            .collect();
    }
    samples
}

fn matches_samples(contract: &Contract, candidate: &Expr, samples: &[Vec<u32>]) -> bool {
    samples.iter().all(|args| {
        let result = candidate.eval(args);
        let requires = contract.requires.iter().all(|expr| {
            matches!(
                eval_spec(expr, contract, args, result, &HashMap::new()),
                Ok(Value::Bool(true))
            )
        });
        !requires
            || contract.ensures.iter().all(|expr| {
                matches!(
                    eval_spec(expr, contract, args, result, &HashMap::new()),
                    Ok(Value::Bool(true))
                )
            })
    })
}

#[derive(Clone)]
enum Value {
    U32(u32),
    I32(i32),
    Bool(bool),
}

fn eval_spec(
    expr: &SpecExpr,
    contract: &Contract,
    args: &[u32],
    result: u32,
    locals: &HashMap<String, Value>,
) -> Result<Value, String> {
    match expr {
        SpecExpr::Ident(name) if name == "result" => {
            if contract.return_type == Type::Bool {
                Ok(Value::Bool(result != 0))
            } else if contract.return_type == Type::I32 {
                Ok(Value::I32(result as i32))
            } else {
                Ok(Value::U32(result))
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
                .copied()
                .ok_or_else(|| format!("missing argument: {name}"))?;
            match contract.params[index].ty {
                Type::Bool => Ok(Value::Bool(value != 0)),
                Type::I32 => Ok(Value::I32(value as i32)),
                _ => Ok(Value::U32(value)),
            }
        }
        SpecExpr::Number(value) => Ok(Value::U32(*value)),
        SpecExpr::Bool(value) => Ok(Value::Bool(*value)),
        SpecExpr::Text(_) | SpecExpr::List(_) | SpecExpr::Record { .. } | SpecExpr::Field(_, _) => {
            Err("non-scalar expressions cannot be synthesized".to_string())
        }
        SpecExpr::Let(name, value, body) => {
            let value = eval_spec(value, contract, args, result, locals)?;
            let mut nested = locals.clone();
            nested.insert(name.clone(), value);
            eval_spec(body, contract, args, result, &nested)
        }
        SpecExpr::If(condition, then_branch, else_branch) => {
            let condition = match eval_spec(condition, contract, args, result, locals)? {
                Value::Bool(value) => value,
                Value::U32(_) | Value::I32(_) => {
                    return Err("if condition must be boolean".to_string());
                }
            };
            if condition {
                eval_spec(then_branch, contract, args, result, locals)
            } else {
                eval_spec(else_branch, contract, args, result, locals)
            }
        }
        SpecExpr::Call(name, _) => Err(format!(
            "unresolved contract call: {name}; link the project before synthesis"
        )),
        SpecExpr::Binary(op, lhs, rhs) => {
            let lhs = eval_spec(lhs, contract, args, result, locals)?;
            let rhs = eval_spec(rhs, contract, args, result, locals)?;
            match (op, lhs, rhs) {
                (BinaryOp::AddWrap, Value::U32(lhs), Value::U32(rhs)) => {
                    Ok(Value::U32(lhs.wrapping_add(rhs)))
                }
                (BinaryOp::AddWrap, Value::I32(lhs), Value::I32(rhs)) => {
                    Ok(Value::I32(lhs.wrapping_add(rhs)))
                }
                (BinaryOp::AddWrap, Value::I32(lhs), Value::U32(rhs)) => {
                    Ok(Value::I32(lhs.wrapping_add(rhs as i32)))
                }
                (BinaryOp::AddWrap, Value::U32(lhs), Value::I32(rhs)) => {
                    Ok(Value::I32((lhs as i32).wrapping_add(rhs)))
                }
                (BinaryOp::MulWrap, Value::U32(lhs), Value::U32(rhs)) => {
                    Ok(Value::U32(lhs.wrapping_mul(rhs)))
                }
                (BinaryOp::MulWrap, Value::I32(lhs), Value::I32(rhs)) => {
                    Ok(Value::I32(lhs.wrapping_mul(rhs)))
                }
                (BinaryOp::MulWrap, Value::I32(lhs), Value::U32(rhs)) => {
                    Ok(Value::I32(lhs.wrapping_mul(rhs as i32)))
                }
                (BinaryOp::MulWrap, Value::U32(lhs), Value::I32(rhs)) => {
                    Ok(Value::I32((lhs as i32).wrapping_mul(rhs)))
                }
                (BinaryOp::Shl, Value::U32(lhs), Value::U32(rhs)) => {
                    Ok(Value::U32(lhs.wrapping_shl(rhs & 31)))
                }
                (BinaryOp::Shl, Value::I32(lhs), Value::I32(rhs)) => {
                    Ok(Value::I32(lhs.wrapping_shl((rhs as u32) & 31)))
                }
                (BinaryOp::Shl, Value::I32(lhs), Value::U32(rhs)) => {
                    Ok(Value::I32(lhs.wrapping_shl(rhs & 31)))
                }
                (BinaryOp::Shl, Value::U32(lhs), Value::I32(rhs)) => {
                    Ok(Value::I32((lhs as i32).wrapping_shl((rhs as u32) & 31)))
                }
                (BinaryOp::Eq, Value::U32(lhs), Value::U32(rhs)) => Ok(Value::Bool(lhs == rhs)),
                (BinaryOp::Ne, Value::U32(lhs), Value::U32(rhs)) => Ok(Value::Bool(lhs != rhs)),
                (BinaryOp::Eq, Value::I32(lhs), Value::I32(rhs)) => Ok(Value::Bool(lhs == rhs)),
                (BinaryOp::Eq, Value::I32(lhs), Value::U32(rhs)) => {
                    Ok(Value::Bool(lhs as u32 == rhs))
                }
                (BinaryOp::Eq, Value::U32(lhs), Value::I32(rhs)) => {
                    Ok(Value::Bool(lhs == rhs as u32))
                }
                (BinaryOp::Ne, Value::I32(lhs), Value::I32(rhs)) => Ok(Value::Bool(lhs != rhs)),
                (BinaryOp::Ne, Value::I32(lhs), Value::U32(rhs)) => {
                    Ok(Value::Bool(lhs as u32 != rhs))
                }
                (BinaryOp::Ne, Value::U32(lhs), Value::I32(rhs)) => {
                    Ok(Value::Bool(lhs != rhs as u32))
                }
                (BinaryOp::Eq, Value::Bool(lhs), Value::Bool(rhs)) => Ok(Value::Bool(lhs == rhs)),
                (BinaryOp::Ne, Value::Bool(lhs), Value::Bool(rhs)) => Ok(Value::Bool(lhs != rhs)),
                (BinaryOp::Lt, Value::U32(lhs), Value::U32(rhs)) => Ok(Value::Bool(lhs < rhs)),
                (BinaryOp::Le, Value::U32(lhs), Value::U32(rhs)) => Ok(Value::Bool(lhs <= rhs)),
                (BinaryOp::Gt, Value::U32(lhs), Value::U32(rhs)) => Ok(Value::Bool(lhs > rhs)),
                (BinaryOp::Ge, Value::U32(lhs), Value::U32(rhs)) => Ok(Value::Bool(lhs >= rhs)),
                (BinaryOp::Lt, Value::I32(lhs), Value::I32(rhs)) => Ok(Value::Bool(lhs < rhs)),
                (BinaryOp::Lt, Value::I32(lhs), Value::U32(rhs)) => {
                    Ok(Value::Bool(lhs < rhs as i32))
                }
                (BinaryOp::Lt, Value::U32(lhs), Value::I32(rhs)) => {
                    Ok(Value::Bool((lhs as i32) < rhs))
                }
                (BinaryOp::Le, Value::I32(lhs), Value::I32(rhs)) => Ok(Value::Bool(lhs <= rhs)),
                (BinaryOp::Le, Value::I32(lhs), Value::U32(rhs)) => {
                    Ok(Value::Bool(lhs <= rhs as i32))
                }
                (BinaryOp::Le, Value::U32(lhs), Value::I32(rhs)) => {
                    Ok(Value::Bool((lhs as i32) <= rhs))
                }
                (BinaryOp::Gt, Value::I32(lhs), Value::I32(rhs)) => Ok(Value::Bool(lhs > rhs)),
                (BinaryOp::Gt, Value::I32(lhs), Value::U32(rhs)) => {
                    Ok(Value::Bool(lhs > rhs as i32))
                }
                (BinaryOp::Gt, Value::U32(lhs), Value::I32(rhs)) => {
                    Ok(Value::Bool((lhs as i32) > rhs))
                }
                (BinaryOp::Ge, Value::I32(lhs), Value::I32(rhs)) => Ok(Value::Bool(lhs >= rhs)),
                (BinaryOp::Ge, Value::I32(lhs), Value::U32(rhs)) => {
                    Ok(Value::Bool(lhs >= rhs as i32))
                }
                (BinaryOp::Ge, Value::U32(lhs), Value::I32(rhs)) => {
                    Ok(Value::Bool((lhs as i32) >= rhs))
                }
                (BinaryOp::And, Value::Bool(lhs), Value::Bool(rhs)) => Ok(Value::Bool(lhs && rhs)),
                (BinaryOp::Or, Value::Bool(lhs), Value::Bool(rhs)) => Ok(Value::Bool(lhs || rhs)),
                _ => Err(format!("operator {:?} has incompatible operands", op)),
            }
        }
    }
}

fn contains_conditional_construct(expr: &SpecExpr) -> bool {
    match expr {
        SpecExpr::Let(_, _, _) | SpecExpr::If(_, _, _) => true,
        SpecExpr::Binary(_, lhs, rhs) => {
            contains_conditional_construct(lhs) || contains_conditional_construct(rhs)
        }
        SpecExpr::Ident(_) | SpecExpr::Number(_) | SpecExpr::Bool(_) | SpecExpr::Text(_) => false,
        SpecExpr::List(values) => values.iter().any(contains_conditional_construct),
        SpecExpr::Record { fields, .. } => fields
            .iter()
            .any(|(_, value)| contains_conditional_construct(value)),
        SpecExpr::Field(value, _) => contains_conditional_construct(value),
        SpecExpr::Call(_, args) => args.iter().any(contains_conditional_construct),
    }
}

fn candidate_from_contract(contract: &Contract) -> Result<Option<Expr>, String> {
    for ensure in &contract.ensures {
        let SpecExpr::Binary(BinaryOp::Eq, lhs, rhs) = ensure else {
            continue;
        };

        if matches!(lhs.as_ref(), SpecExpr::Ident(name) if name == "result") {
            return lower_spec_expr(rhs, contract, &HashMap::new()).map(Some);
        }
        if matches!(rhs.as_ref(), SpecExpr::Ident(name) if name == "result") {
            return lower_spec_expr(lhs, contract, &HashMap::new()).map(Some);
        }
    }
    Ok(None)
}

fn lower_spec_expr(
    expr: &SpecExpr,
    contract: &Contract,
    locals: &HashMap<String, Expr>,
) -> Result<Expr, String> {
    match expr {
        SpecExpr::Ident(name) if locals.contains_key(name) => Ok(locals[name].clone()),
        SpecExpr::Ident(name) => contract
            .params
            .iter()
            .position(|param| param.name == *name)
            .map(|index| Expr::Arg(index as u8))
            .ok_or_else(|| format!("unknown identifier: {name}")),
        SpecExpr::Number(value) => Ok(Expr::Const(*value)),
        SpecExpr::Bool(value) => Ok(Expr::Const(u32::from(*value))),
        SpecExpr::Text(_) | SpecExpr::List(_) | SpecExpr::Record { .. } | SpecExpr::Field(_, _) => {
            Err("non-scalar expressions cannot be lowered to WASM".to_string())
        }
        SpecExpr::Let(name, value, body) => {
            let value = lower_spec_expr(value, contract, locals)?;
            let mut nested = locals.clone();
            nested.insert(name.clone(), value);
            lower_spec_expr(body, contract, &nested)
        }
        SpecExpr::If(condition, then_branch, else_branch) => Ok(Expr::Select {
            condition: Box::new(lower_spec_expr(condition, contract, locals)?),
            then_branch: Box::new(lower_spec_expr(then_branch, contract, locals)?),
            else_branch: Box::new(lower_spec_expr(else_branch, contract, locals)?),
        }),
        SpecExpr::Call(name, _) => Err(format!(
            "unresolved contract call: {name}; link the project before synthesis"
        )),
        SpecExpr::Binary(op, lhs, rhs) => {
            let signed = matches!(
                op,
                BinaryOp::Lt | BinaryOp::Le | BinaryOp::Gt | BinaryOp::Ge
            ) && (is_i32_expr(lhs, contract) || is_i32_expr(rhs, contract));
            let lhs = Box::new(lower_spec_expr(lhs, contract, locals)?);
            let rhs = Box::new(lower_spec_expr(rhs, contract, locals)?);
            Ok(match op {
                BinaryOp::AddWrap => Expr::Add(lhs, rhs),
                BinaryOp::MulWrap => Expr::Mul(lhs, rhs),
                BinaryOp::Shl => Expr::Shl(lhs, rhs),
                BinaryOp::Eq => Expr::Eq(lhs, rhs),
                BinaryOp::Ne => Expr::Ne(lhs, rhs),
                BinaryOp::Lt if signed => Expr::SLt(lhs, rhs),
                BinaryOp::Le if signed => Expr::SLe(lhs, rhs),
                BinaryOp::Gt if signed => Expr::SGt(lhs, rhs),
                BinaryOp::Ge if signed => Expr::SGe(lhs, rhs),
                BinaryOp::Lt => Expr::Lt(lhs, rhs),
                BinaryOp::Le => Expr::Le(lhs, rhs),
                BinaryOp::Gt => Expr::Gt(lhs, rhs),
                BinaryOp::Ge => Expr::Ge(lhs, rhs),
                BinaryOp::And => Expr::And(lhs, rhs),
                BinaryOp::Or => Expr::Or(lhs, rhs),
            })
        }
    }
}

fn is_i32_expr(expr: &SpecExpr, contract: &Contract) -> bool {
    let names = contract
        .params
        .iter()
        .map(|param| (param.name.clone(), param.ty.clone()))
        .collect();
    matches!(
        axiom_parser::type_of_expr(expr, &names, Some(&contract.return_type)),
        Ok(Type::I32)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use axiom_parser::parse;

    #[test]
    fn synthesizes_mul8() {
        let contract = parse(include_str!("../../../examples/mul8.ax")).unwrap();
        let candidate = synthesize(&contract).unwrap();

        assert_eq!(
            candidate,
            Expr::Shl(Box::new(Expr::Arg(0)), Box::new(Expr::Const(3)))
        );
    }

    #[test]
    fn completes_mul8_pipeline() {
        let contract = parse(include_str!("../../../examples/mul8.ax")).unwrap();
        let (candidate, module) = synthesize_wasm(&contract).unwrap();

        assert_eq!(
            candidate,
            Expr::Shl(Box::new(Expr::Arg(0)), Box::new(Expr::Const(3)))
        );
        assert_eq!(&module[..8], b"\0asm\x01\0\0\0");
    }

    #[test]
    fn synthesizes_identity_from_base_candidates() {
        let contract = parse("contract identity(x: u32) -> u32 { ensures result == x; }").unwrap();

        assert_eq!(synthesize(&contract).unwrap(), Expr::Arg(0));
    }

    #[test]
    fn synthesizes_a_value_outside_the_depth_one_constant_range() {
        let contract =
            parse("contract mul17(x: u32) -> u32 { ensures result == x *% 17; }").unwrap();

        assert!(synthesize(&contract).is_ok());
    }

    #[test]
    fn rejects_inputs_that_violate_requires() {
        let contract = parse(
            "contract guarded(x: u32) -> u32 { requires x != 0 && x != 1; ensures result == x; }",
        )
        .unwrap();

        assert!(check_requires(&contract, &[2]).is_ok());
        assert!(check_requires(&contract, &[1]).is_err());
    }

    #[test]
    fn reports_a_zero_timeout_before_search() {
        let contract = parse(include_str!("../../../examples/mul8.ax")).unwrap();
        let error = synthesize_with_options(
            &contract,
            SynthesisOptions {
                timeout: Some(Duration::ZERO),
                ..SynthesisOptions::default()
            },
        )
        .unwrap_err();

        assert!(error.contains("synthesis timed out"));
    }

    #[test]
    fn synthesizes_and_runs_conditional_contract() {
        let contract = parse(include_str!("../../../examples/easy_min.ax")).unwrap();
        let (candidate, module) = synthesize_wasm(&contract).unwrap();

        assert_eq!(candidate.eval(&[2, 5]), 2);
        assert_eq!(candidate.eval(&[8, 5]), 5);
        assert_eq!(axiom_wasm::run_with_args(&module, &[2, 5]).unwrap(), 2);
        assert_eq!(axiom_wasm::run_with_args(&module, &[8, 5]).unwrap(), 5);
    }

    #[test]
    fn synthesizes_signed_conditional_contract() {
        let contract = parse(include_str!("../../../examples/signed_min.ax")).unwrap();
        let (candidate, module) = synthesize_wasm(&contract).unwrap();

        assert_eq!(candidate.eval(&[u32::MAX, 1]), u32::MAX);
        assert_eq!(candidate.eval(&[1, u32::MAX]), u32::MAX);
        assert_eq!(
            axiom_wasm::run_with_args(&module, &[u32::MAX, 1]).unwrap(),
            u32::MAX
        );
        assert_eq!(
            axiom_wasm::run_with_args(&module, &[1, u32::MAX]).unwrap(),
            u32::MAX
        );
    }

    #[test]
    fn lowers_let_contract_to_a_verified_candidate() {
        let contract = parse(include_str!("../../../examples/easy_let.ax")).unwrap();
        let candidate = synthesize(&contract).unwrap();

        assert_eq!(candidate.eval(&[21]), 42);
    }

    #[test]
    fn synthesizes_and_runs_bool_contract() {
        let contract =
            parse("program is_ready(value: number) -> bool { gives answer equals value > 0; }")
                .unwrap();
        let (candidate, module) = synthesize_wasm(&contract).unwrap();

        assert_eq!(candidate.eval(&[0]), 0);
        assert_eq!(candidate.eval(&[4]), 1);
        assert_eq!(axiom_wasm::run_with_args(&module, &[0]).unwrap(), 0);
        assert_eq!(axiom_wasm::run_with_args(&module, &[4]).unwrap(), 1);
    }

    #[test]
    fn synthesizes_bool_parameter_contract() {
        let contract =
            parse("program toggle(done: bool) -> bool { gives answer equals (done != true); }")
                .unwrap();
        let (candidate, module) = synthesize_wasm(&contract).unwrap();

        assert_eq!(candidate.eval(&[0]), 1);
        assert_eq!(candidate.eval(&[1]), 0);
        assert_eq!(axiom_wasm::run_with_args(&module, &[0]).unwrap(), 1);
        assert_eq!(axiom_wasm::run_with_args(&module, &[1]).unwrap(), 0);
    }
}
