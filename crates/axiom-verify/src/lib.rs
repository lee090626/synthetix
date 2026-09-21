//! SMT verification for the initial u32 contract subset.

use std::collections::HashMap;

use axiom_ami::Expr;
use axiom_parser::{BinaryOp, Contract, SpecExpr, Type};
use z3::{
    SatResult, Solver,
    ast::{BV, Bool},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VerificationResult {
    Verified,
    Counterexample(Vec<u32>),
    Unknown,
}

#[derive(Clone)]
enum Value {
    U32(BV),
    Bool(Bool),
}

/// Proves that `candidate` satisfies every `ensures` clause under `requires`.
pub fn verify(contract: &Contract, candidate: &Expr) -> Result<VerificationResult, String> {
    if !matches!(contract.return_type, Type::U32 | Type::Bool) {
        return Err("unsupported contract return type".to_string());
    }

    let args: Vec<BV> = contract
        .params
        .iter()
        .map(|param| {
            if !matches!(param.ty, Type::U32 | Type::Bool) {
                return Err(format!("unsupported parameter type: {}", param.name));
            }
            Ok(BV::new_const(param.name.as_str(), 32))
        })
        .collect::<Result<_, String>>()?;

    let candidate_ast = expr_to_bv(candidate, &args)?;
    let empty_locals = HashMap::new();
    let requires = if contract.requires.is_empty() {
        Bool::from_bool(true)
    } else {
        let clauses = contract
            .requires
            .iter()
            .map(|expr| {
                match spec_to_value(expr, contract, &args, &candidate_ast, &empty_locals)? {
                    Value::Bool(value) => Ok(value),
                    Value::U32(_) => Err("requires clause must be boolean".to_string()),
                }
            })
            .collect::<Result<Vec<_>, String>>()?;
        let refs: Vec<&Bool> = clauses.iter().collect();
        Bool::and(&refs)
    };

    if contract.ensures.is_empty() {
        return Err("contract must have at least one ensures clause".to_string());
    }

    let ensures = contract
        .ensures
        .iter()
        .map(
            |expr| match spec_to_value(expr, contract, &args, &candidate_ast, &empty_locals)? {
                Value::Bool(value) => Ok(value),
                Value::U32(_) => Err("ensures clause must be boolean".to_string()),
            },
        )
        .collect::<Result<Vec<_>, String>>()?;
    let refs: Vec<&Bool> = ensures.iter().collect();
    let violation = requires & Bool::and(&refs).not();

    let solver = Solver::new();
    for (index, param) in contract.params.iter().enumerate() {
        if param.ty == Type::Bool {
            let zero = args[index].eq(BV::from_u64(0, 32));
            let one = args[index].eq(BV::from_u64(1, 32));
            solver.assert(Bool::or(&[&zero, &one]));
        }
    }
    solver.assert(&violation);
    match solver.check() {
        SatResult::Unsat => Ok(VerificationResult::Verified),
        SatResult::Unknown => Ok(VerificationResult::Unknown),
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
            Ok(VerificationResult::Counterexample(values))
        }
    }
}

fn expr_to_bv(expr: &Expr, args: &[BV]) -> Result<BV, String> {
    match expr {
        Expr::Arg(index) => args
            .get(*index as usize)
            .cloned()
            .ok_or_else(|| format!("candidate references missing argument {}", index)),
        Expr::Const(value) => Ok(BV::from_u64(*value as u64, 32)),
        Expr::Add(lhs, rhs) => Ok(&expr_to_bv(lhs, args)? + &expr_to_bv(rhs, args)?),
        Expr::Mul(lhs, rhs) => Ok(&expr_to_bv(lhs, args)? * &expr_to_bv(rhs, args)?),
        Expr::Shl(lhs, rhs) => {
            let lhs = expr_to_bv(lhs, args)?;
            let rhs = expr_to_bv(rhs, args)?;
            Ok(lhs.bvshl(rhs.bvand(BV::from_u64(31, 32))))
        }
        Expr::Eq(_, _)
        | Expr::Ne(_, _)
        | Expr::Lt(_, _)
        | Expr::Le(_, _)
        | Expr::Gt(_, _)
        | Expr::Ge(_, _)
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
            if contract.params[index].ty == Type::Bool {
                Ok(Value::Bool(value.ne(BV::from_u64(0, 32))))
            } else {
                Ok(Value::U32(value))
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
                Value::U32(_) => return Err("if condition must be boolean".to_string()),
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
                (BinaryOp::MulWrap, Value::U32(lhs), Value::U32(rhs)) => {
                    Ok(Value::U32(&lhs * &rhs))
                }
                (BinaryOp::Shl, Value::U32(lhs), Value::U32(rhs)) => {
                    Ok(Value::U32(lhs.bvshl(rhs.bvand(BV::from_u64(31, 32)))))
                }
                (BinaryOp::Eq, Value::U32(lhs), Value::U32(rhs)) => Ok(Value::Bool(lhs.eq(rhs))),
                (BinaryOp::Ne, Value::U32(lhs), Value::U32(rhs)) => Ok(Value::Bool(lhs.ne(rhs))),
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
        let contract = parse(include_str!("../../../examples/mul8.ax")).unwrap();
        let candidate = Expr::Shl(Box::new(Expr::Arg(0)), Box::new(Expr::Const(3)));

        assert_eq!(
            verify(&contract, &candidate).unwrap(),
            VerificationResult::Verified
        );
    }

    #[test]
    fn returns_counterexample_for_wrong_candidate() {
        let contract = parse(include_str!("../../../examples/mul8.ax")).unwrap();
        let candidate = Expr::Arg(0);

        match verify(&contract, &candidate).unwrap() {
            VerificationResult::Counterexample(values) => assert_eq!(values.len(), 1),
            other => panic!("expected counterexample, got {other:?}"),
        }
    }

    #[test]
    fn proves_conditional_specification_for_all_inputs() {
        let contract = parse(include_str!("../../../examples/easy_min.ax")).unwrap();
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
}
