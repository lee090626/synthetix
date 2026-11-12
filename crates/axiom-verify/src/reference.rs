//! Small executable reference semantics for the supported Synthetix subset.
//!
//! This module deliberately does not call the AMI evaluator or Z3. It is the
//! independent comparison point for AMI, verification, and WASM tests.

use std::collections::HashMap;

use axiom_ami::Expr;
use axiom_parser::{BinaryOp, Contract, SpecExpr, Type};

type Numeric = (u32, bool);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    U32(u32),
    I32(i32),
    Bool(bool),
    Array(Vec<u32>),
}

impl Value {
    pub fn bits(&self) -> u32 {
        match self {
            Self::U32(value) => *value,
            Self::I32(value) => *value as u32,
            Self::Bool(value) => u32::from(*value),
            Self::Array(_) => panic!("fixed arrays do not have scalar bits"),
        }
    }
}

/// Evaluates an AMI expression without using `Expr::eval`.
pub fn eval_ami(expr: &Expr, args: &[u32]) -> Result<u32, String> {
    match expr {
        Expr::Arg(index) => args
            .get(*index as usize)
            .copied()
            .ok_or_else(|| format!("candidate references missing argument {index}")),
        Expr::Const(value) => Ok(*value),
        Expr::Add(lhs, rhs) => Ok(eval_ami(lhs, args)?.wrapping_add(eval_ami(rhs, args)?)),
        Expr::Sub(lhs, rhs) => Ok(eval_ami(lhs, args)?.wrapping_sub(eval_ami(rhs, args)?)),
        Expr::Mul(lhs, rhs) => Ok(eval_ami(lhs, args)?.wrapping_mul(eval_ami(rhs, args)?)),
        Expr::Div(lhs, rhs) => eval_ami(lhs, args)?
            .checked_div(eval_ami(rhs, args)?)
            .ok_or_else(|| "division by zero".to_string()),
        Expr::Shl(lhs, rhs) => Ok(eval_ami(lhs, args)?.wrapping_shl(eval_ami(rhs, args)? & 31)),
        Expr::BitAnd(lhs, rhs) => Ok(eval_ami(lhs, args)? & eval_ami(rhs, args)?),
        Expr::BitOr(lhs, rhs) => Ok(eval_ami(lhs, args)? | eval_ami(rhs, args)?),
        Expr::BitXor(lhs, rhs) => Ok(eval_ami(lhs, args)? ^ eval_ami(rhs, args)?),
        Expr::Eq(lhs, rhs) => Ok(u32::from(eval_ami(lhs, args)? == eval_ami(rhs, args)?)),
        Expr::Ne(lhs, rhs) => Ok(u32::from(eval_ami(lhs, args)? != eval_ami(rhs, args)?)),
        Expr::Lt(lhs, rhs) => Ok(u32::from(eval_ami(lhs, args)? < eval_ami(rhs, args)?)),
        Expr::Le(lhs, rhs) => Ok(u32::from(eval_ami(lhs, args)? <= eval_ami(rhs, args)?)),
        Expr::Gt(lhs, rhs) => Ok(u32::from(eval_ami(lhs, args)? > eval_ami(rhs, args)?)),
        Expr::Ge(lhs, rhs) => Ok(u32::from(eval_ami(lhs, args)? >= eval_ami(rhs, args)?)),
        Expr::SLt(lhs, rhs) => Ok(u32::from(
            (eval_ami(lhs, args)? as i32) < (eval_ami(rhs, args)? as i32),
        )),
        Expr::SLe(lhs, rhs) => Ok(u32::from(
            (eval_ami(lhs, args)? as i32) <= (eval_ami(rhs, args)? as i32),
        )),
        Expr::SGt(lhs, rhs) => Ok(u32::from(
            (eval_ami(lhs, args)? as i32) > (eval_ami(rhs, args)? as i32),
        )),
        Expr::SGe(lhs, rhs) => Ok(u32::from(
            (eval_ami(lhs, args)? as i32) >= (eval_ami(rhs, args)? as i32),
        )),
        Expr::And(lhs, rhs) => Ok(u32::from(
            eval_ami(lhs, args)? != 0 && eval_ami(rhs, args)? != 0,
        )),
        Expr::Or(lhs, rhs) => Ok(u32::from(
            eval_ami(lhs, args)? != 0 || eval_ami(rhs, args)? != 0,
        )),
        Expr::Select {
            condition,
            then_branch,
            else_branch,
        } => {
            if eval_ami(condition, args)? != 0 {
                eval_ami(then_branch, args)
            } else {
                eval_ami(else_branch, args)
            }
        }
        Expr::ArrayGet { elements, index } => {
            let index = eval_ami(index, args)? as usize;
            eval_ami(
                elements
                    .get(index)
                    .ok_or_else(|| format!("fixed array index {index} is out of bounds"))?,
                args,
            )
        }
        Expr::Guard { condition, value } => {
            if eval_ami(condition, args)? == 0 {
                Err("verified runtime guard failed".to_string())
            } else {
                eval_ami(value, args)
            }
        }
    }
}

/// Evaluates a scalar specification expression using the documented rules.
pub fn eval_spec(
    expr: &SpecExpr,
    contract: &Contract,
    args: &[u32],
    result: u32,
) -> Result<Value, String> {
    let (_, expected) = axiom_parser::verified_abi_layout(&contract.params)?;
    if args.len() != expected {
        return Err(format!(
            "expected {expected} flattened inputs, got {}",
            args.len()
        ));
    }
    eval_spec_with_locals(expr, contract, args, result, &HashMap::new())
}

/// Checks a concrete input using the contract implication
/// `requires -> ensures`.
pub fn contract_holds(contract: &Contract, candidate: &Expr, args: &[u32]) -> Result<bool, String> {
    let (_, expected) = axiom_parser::verified_abi_layout(&contract.params)?;
    if args.len() != expected {
        return Err(format!(
            "expected {expected} flattened inputs, got {}",
            args.len()
        ));
    }
    let (offsets, _) = axiom_parser::verified_abi_layout(&contract.params)?;
    for (index, param) in contract.params.iter().enumerate() {
        if param.ty == Type::Bool && args[offsets[index]] > 1 {
            return Err(format!("bool argument {} must be 0 or 1", param.name));
        }
    }

    for expr in &contract.requires {
        match eval_spec(expr, contract, args, 0)? {
            Value::Bool(true) => {}
            Value::Bool(false) => return Ok(true),
            _ => return Err("requires clause must be boolean".to_string()),
        }
    }

    let result = eval_ami(candidate, args)?;
    contract
        .ensures
        .iter()
        .map(|expr| eval_spec(expr, contract, args, result))
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .try_fold(true, |all, value| match value {
            Value::Bool(value) => Ok(all && value),
            _ => Err("ensures clause must be boolean".to_string()),
        })
}

fn eval_spec_with_locals(
    expr: &SpecExpr,
    contract: &Contract,
    args: &[u32],
    result: u32,
    locals: &HashMap<String, Value>,
) -> Result<Value, String> {
    match expr {
        SpecExpr::Ident(name) if name == "result" => match contract.return_type {
            Type::U32 => Ok(Value::U32(result)),
            Type::I32 => Ok(Value::I32(result as i32)),
            Type::Bool => Ok(Value::Bool(result != 0)),
            _ => Err("non-scalar contract result cannot be evaluated".to_string()),
        },
        SpecExpr::Ident(name) if locals.contains_key(name) => Ok(locals[name].clone()),
        SpecExpr::Ident(name) => {
            let index = contract
                .params
                .iter()
                .position(|param| param.name == *name)
                .ok_or_else(|| format!("unknown identifier: {name}"))?;
            let (offsets, _) = axiom_parser::verified_abi_layout(&contract.params)?;
            let offset = offsets[index];
            match contract.params[index].ty {
                Type::U32 => Ok(Value::U32(args[offset])),
                Type::I32 => Ok(Value::I32(args[offset] as i32)),
                Type::Bool => Ok(Value::Bool(args[offset] != 0)),
                Type::FixedArray(_, length) => {
                    Ok(Value::Array(args[offset..offset + length].to_vec()))
                }
                _ => Err(format!("non-scalar parameter cannot be evaluated: {name}")),
            }
        }
        SpecExpr::Number(value) => Ok(Value::U32(*value)),
        SpecExpr::Bool(value) => Ok(Value::Bool(*value)),
        SpecExpr::Field(value, field) => {
            match eval_spec_with_locals(value, contract, args, result, locals)? {
                Value::Array(values) if field == "length" => Ok(Value::U32(values.len() as u32)),
                _ => Err("field access is only supported for fixed array length".to_string()),
            }
        }
        SpecExpr::Index(array, index) => {
            let array = eval_spec_with_locals(array, contract, args, result, locals)?;
            let index = eval_spec_with_locals(index, contract, args, result, locals)?;
            let Value::Array(values) = array else {
                return Err("indexed reads require a fixed verified array".to_string());
            };
            let index = checked_array_index(index, values.len())?;
            Ok(Value::U32(values[index]))
        }
        SpecExpr::ArrayUpdate(array, index, value) => {
            let array = eval_spec_with_locals(array, contract, args, result, locals)?;
            let index = eval_spec_with_locals(index, contract, args, result, locals)?;
            let value = eval_spec_with_locals(value, contract, args, result, locals)?;
            let Value::Array(mut values) = array else {
                return Err("array_update requires a fixed verified array".to_string());
            };
            let index = checked_array_index(index, values.len())?;
            let Value::U32(value) = value else {
                return Err("array update value must be u32".to_string());
            };
            values[index] = value;
            Ok(Value::Array(values))
        }
        SpecExpr::Text(_) | SpecExpr::List(_) | SpecExpr::Record { .. } => {
            Err("non-scalar expressions cannot be evaluated as contracts".to_string())
        }
        SpecExpr::Let(name, value, body) => {
            let value = eval_spec_with_locals(value, contract, args, result, locals)?;
            let mut nested = locals.clone();
            nested.insert(name.clone(), value);
            eval_spec_with_locals(body, contract, args, result, &nested)
        }
        SpecExpr::For {
            index,
            start,
            end,
            accumulator,
            initial,
            invariant,
            body,
        } => {
            if end < start || end - start > axiom_parser::MAX_VERIFIED_LOOP_ITERATIONS {
                return Err("bounded loop exceeds the verified iteration limit".to_string());
            }
            let mut current = eval_spec_with_locals(initial, contract, args, result, locals)?;
            for iteration in *start..*end {
                let mut nested = locals.clone();
                nested.insert(index.clone(), Value::U32(iteration));
                nested.insert(accumulator.clone(), current.clone());
                match eval_spec_with_locals(invariant, contract, args, result, &nested)? {
                    Value::Bool(true) => {}
                    Value::Bool(false) => {
                        return Err(format!("loop invariant failed at iteration {iteration}"));
                    }
                    _ => return Err("loop invariant must be boolean".to_string()),
                }
                current = eval_spec_with_locals(body, contract, args, result, &nested)?;
            }
            let mut nested = locals.clone();
            nested.insert(index.clone(), Value::U32(*end));
            nested.insert(accumulator.clone(), current.clone());
            match eval_spec_with_locals(invariant, contract, args, result, &nested)? {
                Value::Bool(true) => Ok(current),
                Value::Bool(false) => Err(format!("loop invariant failed at exit {end}")),
                _ => Err("loop invariant must be boolean".to_string()),
            }
        }
        SpecExpr::If(condition, then_branch, else_branch) => {
            let condition = match eval_spec_with_locals(condition, contract, args, result, locals)?
            {
                Value::Bool(value) => value,
                _ => return Err("if condition must be boolean".to_string()),
            };
            eval_spec_with_locals(
                if condition { then_branch } else { else_branch },
                contract,
                args,
                result,
                locals,
            )
        }
        SpecExpr::Call(name, _) => Err(format!("unresolved contract call: {name}")),
        SpecExpr::Binary(BinaryOp::And, lhs, rhs) => {
            let lhs = eval_spec_with_locals(lhs, contract, args, result, locals)?;
            match lhs {
                Value::Bool(false) => Ok(Value::Bool(false)),
                Value::Bool(true) => eval_spec_with_locals(rhs, contract, args, result, locals),
                _ => Err("operator And requires boolean operands".to_string()),
            }
        }
        SpecExpr::Binary(BinaryOp::Or, lhs, rhs) => {
            let lhs = eval_spec_with_locals(lhs, contract, args, result, locals)?;
            match lhs {
                Value::Bool(true) => Ok(Value::Bool(true)),
                Value::Bool(false) => eval_spec_with_locals(rhs, contract, args, result, locals),
                _ => Err("operator Or requires boolean operands".to_string()),
            }
        }
        SpecExpr::Binary(op, lhs, rhs) => {
            let lhs = eval_spec_with_locals(lhs, contract, args, result, locals)?;
            let rhs = eval_spec_with_locals(rhs, contract, args, result, locals)?;
            eval_binary(op, lhs, rhs)
        }
    }
}

fn eval_binary(op: &BinaryOp, lhs: Value, rhs: Value) -> Result<Value, String> {
    match op {
        BinaryOp::AddWrap => numeric(lhs, rhs, u32::wrapping_add),
        BinaryOp::SubWrap => numeric(lhs, rhs, u32::wrapping_sub),
        BinaryOp::SubChecked => match (lhs, rhs) {
            (Value::U32(lhs), Value::U32(rhs)) => lhs
                .checked_sub(rhs)
                .map(Value::U32)
                .ok_or_else(|| "subtraction below 0".to_string()),
            _ => Err("checked subtraction is only supported for u32".to_string()),
        },
        BinaryOp::MulWrap => numeric(lhs, rhs, u32::wrapping_mul),
        BinaryOp::Div => match (lhs, rhs) {
            (Value::U32(lhs), Value::U32(rhs)) => lhs
                .checked_div(rhs)
                .map(Value::U32)
                .ok_or_else(|| "division by zero".to_string()),
            _ => Err("division is only supported for u32".to_string()),
        },
        BinaryOp::Shl => numeric(lhs, rhs, |lhs, rhs| lhs.wrapping_shl(rhs & 31)),
        BinaryOp::BitAnd => numeric(lhs, rhs, |lhs, rhs| lhs & rhs),
        BinaryOp::BitOr => numeric(lhs, rhs, |lhs, rhs| lhs | rhs),
        BinaryOp::BitXor => numeric(lhs, rhs, |lhs, rhs| lhs ^ rhs),
        BinaryOp::Eq => equality(lhs, rhs, false),
        BinaryOp::Ne => equality(lhs, rhs, true),
        BinaryOp::Lt | BinaryOp::Le | BinaryOp::Gt | BinaryOp::Ge => {
            let (lhs, rhs) = numeric_bits(lhs, rhs)?;
            let result = if lhs.1 || rhs.1 {
                compare_signed(op, lhs.0 as i32, rhs.0 as i32)
            } else {
                compare_unsigned(op, lhs.0, rhs.0)
            };
            Ok(Value::Bool(result))
        }
        BinaryOp::And | BinaryOp::Or => match (lhs, rhs) {
            (Value::Bool(lhs), Value::Bool(rhs)) => Ok(Value::Bool(if *op == BinaryOp::And {
                lhs && rhs
            } else {
                lhs || rhs
            })),
            _ => Err(format!("operator {op:?} requires boolean operands")),
        },
    }
}

fn numeric(
    lhs: Value,
    rhs: Value,
    operation: impl FnOnce(u32, u32) -> u32,
) -> Result<Value, String> {
    let (lhs, rhs) = numeric_bits(lhs, rhs)?;
    let result = operation(lhs.0, rhs.0);
    Ok(if lhs.1 || rhs.1 {
        Value::I32(result as i32)
    } else {
        Value::U32(result)
    })
}

fn numeric_bits(lhs: Value, rhs: Value) -> Result<(Numeric, Numeric), String> {
    match (lhs, rhs) {
        (Value::U32(lhs), Value::U32(rhs)) => Ok(((lhs, false), (rhs, false))),
        (Value::I32(lhs), Value::I32(rhs)) => Ok(((lhs as u32, true), (rhs as u32, true))),
        (Value::I32(lhs), Value::U32(rhs)) => Ok(((lhs as u32, true), (rhs, false))),
        (Value::U32(lhs), Value::I32(rhs)) => Ok(((lhs, false), (rhs as u32, true))),
        _ => Err("operator requires numeric operands".to_string()),
    }
}

fn equality(lhs: Value, rhs: Value, negate: bool) -> Result<Value, String> {
    let equal = match (lhs, rhs) {
        (Value::Bool(lhs), Value::Bool(rhs)) => lhs == rhs,
        (Value::Array(lhs), Value::Array(rhs)) => lhs == rhs,
        (lhs, rhs) => {
            let (lhs, rhs) = numeric_bits(lhs, rhs)?;
            lhs.0 == rhs.0
        }
    };
    Ok(Value::Bool(if negate { !equal } else { equal }))
}

fn checked_array_index(index: Value, length: usize) -> Result<usize, String> {
    let index = match index {
        Value::U32(index) => index as usize,
        Value::I32(index) if index >= 0 => index as usize,
        Value::I32(_) => return Err("fixed array index is negative".to_string()),
        _ => return Err("array index must be u32 or i32".to_string()),
    };
    if index >= length {
        return Err(format!("fixed array index {index} is out of bounds"));
    }
    Ok(index)
}

fn compare_unsigned(op: &BinaryOp, lhs: u32, rhs: u32) -> bool {
    match op {
        BinaryOp::Lt => lhs < rhs,
        BinaryOp::Le => lhs <= rhs,
        BinaryOp::Gt => lhs > rhs,
        BinaryOp::Ge => lhs >= rhs,
        _ => unreachable!("numeric comparison expected"),
    }
}

fn compare_signed(op: &BinaryOp, lhs: i32, rhs: i32) -> bool {
    match op {
        BinaryOp::Lt => lhs < rhs,
        BinaryOp::Le => lhs <= rhs,
        BinaryOp::Gt => lhs > rhs,
        BinaryOp::Ge => lhs >= rhs,
        _ => unreachable!("numeric comparison expected"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axiom_parser::parse;

    #[test]
    fn evaluates_wrapping_and_signed_boundaries() {
        let expression = Expr::Sub(Box::new(Expr::Const(0)), Box::new(Expr::Const(1)));
        assert_eq!(eval_ami(&expression, &[]).unwrap(), u32::MAX);

        let expression = Expr::SLt(Box::new(Expr::Arg(0)), Box::new(Expr::Arg(1)));
        assert_eq!(eval_ami(&expression, &[u32::MAX, 1]).unwrap(), 1);
    }

    #[test]
    fn evaluates_specification_values() {
        let contract = parse("contract f(x: i32) -> i32 { ensures result == x +% 1; }").unwrap();
        let expr = &contract.ensures[0];
        assert_eq!(
            eval_spec(expr, &contract, &[u32::MAX], 1).unwrap(),
            Value::Bool(false)
        );
        assert_eq!(
            eval_spec(expr, &contract, &[u32::MAX], 0).unwrap(),
            Value::Bool(true)
        );
    }
}
