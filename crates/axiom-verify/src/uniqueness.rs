//! Output uniqueness of declarative u32 contracts; does not prove totality.

use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

use axiom_parser::{Contract, DeclarationKind, SpecExpr, Type};
use z3::{
    Params, SatResult, Solver,
    ast::{BV, Bool},
};

use super::{ensures_to_bool, model_inputs, preconditions_to_bool, spec_definedness};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResultUniqueness {
    AtMostOne,
    NonUnique {
        inputs: Vec<u32>,
        first: u32,
        second: u32,
    },
    EmptyDomain,
    Unknown {
        reason: String,
    },
}

/// Checks for two distinct outputs at the same valid input within a shared
/// solver-time budget. AT_MOST_ONE says nothing about output existence.
pub fn analyze_result_uniqueness(
    contract: &Contract,
    timeout_ms: u32,
) -> Result<ResultUniqueness, String> {
    let started = Instant::now();
    axiom_parser::validate(contract)?;
    if contract.kind != DeclarationKind::Contract || contract.return_type != Type::U32 {
        return Err("result analysis supports contract declarations returning u32 only".into());
    }
    for param in &contract.params {
        if param.name == "result" || !matches!(&param.ty, Type::U32 | Type::FixedArray(_, _)) {
            return Err(
                "result analysis requires u32/fixed-u32-array inputs without a result parameter"
                    .into(),
            );
        }
    }
    for expr in &contract.requires {
        validate_expression(expr, false)?;
    }
    for expr in &contract.ensures {
        validate_expression(expr, true)?;
    }
    let (_, count) = axiom_parser::verified_abi_layout(&contract.params)?;
    // Fresh symbols prevent user names and flattened array names from aliasing.
    let args: Vec<BV> = (0..count).map(|_| BV::fresh_const("input", 32)).collect();
    let first = BV::fresh_const("output", 32);
    let second = BV::fresh_const("output", 32);
    let locals = HashMap::new();
    let solver = Solver::new();
    let requires_defined = contract
        .requires
        .iter()
        .map(|expr| spec_definedness(expr, contract, &args, &first, &locals))
        .collect::<Result<Vec<_>, _>>()?;
    solver.assert(Bool::and(&requires_defined).not());
    match check(&solver, started, timeout_ms) {
        Ok(SatResult::Sat) => {
            return Err("requires clause has an unproven array bounds guard".into());
        }
        Ok(SatResult::Unsat) => {}
        Ok(SatResult::Unknown) => unreachable!(),
        Err(reason) => return Ok(ResultUniqueness::Unknown { reason }),
    }
    solver.reset();
    let requires = preconditions_to_bool(contract, &args, &first)?;
    solver.assert(&requires);
    match check(&solver, started, timeout_ms) {
        Ok(SatResult::Unsat) => return Ok(ResultUniqueness::EmptyDomain),
        Ok(SatResult::Sat) => {}
        Ok(SatResult::Unknown) => unreachable!(),
        Err(reason) => return Ok(ResultUniqueness::Unknown { reason }),
    }
    let mut ensures_defined = Vec::new();
    for output in [&first, &second] {
        for expr in &contract.ensures {
            ensures_defined.push(spec_definedness(expr, contract, &args, output, &locals)?);
        }
    }
    solver.push();
    solver.assert(Bool::and(&ensures_defined).not());
    match check(&solver, started, timeout_ms) {
        Ok(SatResult::Sat) => {
            return Err("ensures clause has an unproven array bounds guard".into());
        }
        Ok(SatResult::Unsat) => {}
        Ok(SatResult::Unknown) => unreachable!(),
        Err(reason) => return Ok(ResultUniqueness::Unknown { reason }),
    }
    solver.pop(1);
    solver.assert(ensures_to_bool(contract, &args, &first)?);
    solver.assert(ensures_to_bool(contract, &args, &second)?);
    solver.assert(first.ne(&second));
    match check(&solver, started, timeout_ms) {
        Ok(SatResult::Unsat) => Ok(ResultUniqueness::AtMostOne),
        Ok(SatResult::Sat) => {
            let values = model_inputs(&solver, &[first, second])?;
            Ok(ResultUniqueness::NonUnique {
                inputs: model_inputs(&solver, &args)?,
                first: values[0],
                second: values[1],
            })
        }
        Ok(SatResult::Unknown) => unreachable!(),
        Err(reason) => Ok(ResultUniqueness::Unknown { reason }),
    }
}

fn check(solver: &Solver, started: Instant, timeout_ms: u32) -> Result<SatResult, String> {
    let remaining = Duration::from_millis(u64::from(timeout_ms)).checked_sub(started.elapsed());
    let Some(remaining) = remaining.filter(|value| !value.is_zero()) else {
        return Err("result analysis time budget exhausted".into());
    };
    let mut params = Params::new();
    params.set_u32("timeout", remaining.as_millis().max(1) as u32);
    solver.set_params(&params);
    match solver.check() {
        SatResult::Unknown => Err(solver
            .get_reason_unknown()
            .unwrap_or_else(|| "solver unknown".into())),
        result => Ok(result),
    }
}

fn validate_expression(expr: &SpecExpr, allow_result: bool) -> Result<(), String> {
    match expr {
        SpecExpr::Ident(name) if name == "result" && !allow_result =>
            Err("result analysis requires input-only requires clauses".into()),
        SpecExpr::Ident(_) | SpecExpr::Number(_) | SpecExpr::Bool(_) => Ok(()),
        SpecExpr::Binary(_, lhs, rhs) | SpecExpr::Index(lhs, rhs) => {
            validate_expression(lhs, allow_result)?;
            validate_expression(rhs, allow_result)
        },
        SpecExpr::Field(value, _) => validate_expression(value, allow_result),
        SpecExpr::Let(name, value, body) => {
            if name == "result" { return Err("result analysis does not support shadowing result".into()); }
            validate_expression(value, allow_result)?;
            validate_expression(body, allow_result)
        },
        SpecExpr::If(a, b, c) | SpecExpr::ArrayUpdate(a, b, c) => {
            validate_expression(a, allow_result)?;
            validate_expression(b, allow_result)?;
            validate_expression(c, allow_result)
        },
        _ => Err("result analysis supports local integer and fixed-array expressions only; calls are unsupported".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ProofResult, reference, verify};
    use axiom_parser::parse;

    #[test]
    fn non_unique_witness_replays_against_the_original_specification() {
        for source in [
            include_str!("../../../tests/fixtures/first-product/underspecified.ax"),
            "contract bounded(values: [u32; 2], values_0: u32) -> u32 { ensures result <= values[0]; ensures result <= values_0; }",
        ] {
            let contract = parse(source).unwrap();
            let ResultUniqueness::NonUnique {
                inputs,
                first,
                second,
            } = analyze_result_uniqueness(&contract, 10_000).unwrap()
            else {
                panic!("expected two outputs for {source}");
            };
            assert_ne!(first, second);
            for result in [first, second] {
                for clause in contract.requires.iter().chain(&contract.ensures) {
                    assert_eq!(
                        reference::eval_spec(clause, &contract, &inputs, result).unwrap(),
                        reference::Value::Bool(true)
                    );
                }
            }
        }
    }

    #[test]
    fn complete_product_policies_have_at_most_one_result() {
        for source in [
            include_str!("../../../tests/fixtures/first-product/capped.ax"),
            include_str!("../../../tests/fixtures/first-product/floor_cap.ax"),
            include_str!("../../../tests/fixtures/first-product/aggregate.ax"),
            "contract identity(__axiom_result: u32) -> u32 { ensures result == __axiom_result; }",
        ] {
            assert_eq!(
                analyze_result_uniqueness(&parse(source).unwrap(), 10_000).unwrap(),
                ResultUniqueness::AtMostOne
            );
        }
    }

    #[test]
    fn array_and_scalar_input_symbols_remain_independent() {
        let contract = parse("contract distinct(values: [u32; 2], values_0: u32) -> u32 { requires values[0] != values_0; ensures result == values[0]; }").unwrap();
        assert_eq!(
            analyze_result_uniqueness(&contract, 10_000).unwrap(),
            ResultUniqueness::AtMostOne
        );
    }

    #[test]
    fn at_most_one_does_not_prove_output_existence() {
        let contract = parse("contract partial(x: u32) -> u32 { requires x <= 10; ensures x == 5 && result == 100; }").unwrap();
        assert_eq!(
            analyze_result_uniqueness(&contract, 10_000).unwrap(),
            ResultUniqueness::AtMostOne
        );
        assert!(matches!(
            verify(&contract, &axiom_ami::Expr::Const(100)).unwrap(),
            ProofResult::Counterexample(_)
        ));
        assert_eq!(
            reference::eval_spec(&contract.ensures[0], &contract, &[3], 100).unwrap(),
            reference::Value::Bool(false)
        );
        let impossible = parse(include_str!(
            "../../../tests/fixtures/first-product/impossible_result.ax"
        ))
        .unwrap();
        assert_eq!(
            analyze_result_uniqueness(&impossible, 10_000).unwrap(),
            ResultUniqueness::AtMostOne
        );
    }

    #[test]
    fn empty_domain_is_not_reported_as_unique() {
        let contract = parse(include_str!(
            "../../../tests/fixtures/first-product/empty_domain.ax"
        ))
        .unwrap();
        assert_eq!(
            analyze_result_uniqueness(&contract, 10_000).unwrap(),
            ResultUniqueness::EmptyDomain
        );
    }

    #[test]
    fn exhausted_budget_is_inconclusive() {
        let contract = parse("contract identity(x: u32) -> u32 { ensures result == x; }").unwrap();
        assert!(matches!(
            analyze_result_uniqueness(&contract, 0).unwrap(),
            ResultUniqueness::Unknown { .. }
        ));
    }

    #[test]
    fn unsupported_contracts_and_output_dependent_preconditions_are_rejected() {
        for source in [
            "contract signed(x: i32) -> u32 { ensures result == 0; }",
            "contract boolean(x: u32) -> bool { ensures result == true; }",
            "fn identity(x: u32) -> u32 { return x; ensures result == x; }",
            "contract shadow(result: u32) -> u32 { ensures result == 0; }",
        ] {
            assert!(
                analyze_result_uniqueness(&parse(source).unwrap(), 10_000).is_err(),
                "{source}"
            );
        }
        let mut contract =
            parse("contract identity(x: u32) -> u32 { ensures result == x; }").unwrap();
        contract.requires.push(SpecExpr::Binary(
            axiom_parser::BinaryOp::Eq,
            Box::new(SpecExpr::Ident("result".into())),
            Box::new(SpecExpr::Number(0)),
        ));
        assert!(analyze_result_uniqueness(&contract, 10_000).is_err());
    }

    #[test]
    fn array_bounds_must_be_defined_under_the_input_contract() {
        for source in [
            "contract unsafe_post(values: [u32; 2], i: u32) -> u32 { ensures result == values[i]; }",
            "contract unsafe_pre(values: [u32; 2], i: u32) -> u32 { requires values[i] == 0; ensures result == 0; }",
        ] {
            let error = analyze_result_uniqueness(&parse(source).unwrap(), 10_000).unwrap_err();
            assert!(error.contains("bounds guard"), "{error}");
        }
        let contract = parse("contract safe(values: [u32; 2], i: u32) -> u32 { requires i < values.length; ensures result == values[i]; }").unwrap();
        assert_eq!(
            analyze_result_uniqueness(&contract, 10_000).unwrap(),
            ResultUniqueness::AtMostOne
        );
    }
}
