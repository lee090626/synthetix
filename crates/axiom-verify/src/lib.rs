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
    for clause in &contract.requires {
        let defined = spec_definedness(clause, contract, &args, &result, &HashMap::new())?;
        require_defined_when(
            contract,
            &args,
            &Bool::from_bool(true),
            &defined,
            &[],
            metrics,
            "requires clause",
        )?;
    }
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

    match check_preconditions_inner(contract, metrics) {
        Err(error) => return Ok(ContractCheck::Invalid(error)),
        Ok(PreconditionResult::Satisfiable) => {}
        Ok(PreconditionResult::UnsatisfiablePrecondition) => {
            return Ok(ContractCheck::UnsatisfiablePrecondition);
        }
        Ok(PreconditionResult::SolverUnknown) => {
            return Err("contract precondition satisfiability is unknown".to_string());
        }
    }

    let args = symbolic_args(contract)?;
    let result = BV::new_const("__axiom_result", 32);
    let requires = preconditions_to_bool(contract, &args, &result)?;
    let ensures_defined = contract
        .ensures
        .iter()
        .map(|clause| spec_definedness(clause, contract, &args, &result, &HashMap::new()))
        .collect::<Result<Vec<_>, _>>()?;
    let ensures_defined = Bool::and(&ensures_defined.iter().collect::<Vec<_>>());
    if let Err(error) = require_defined_when(
        contract,
        &args,
        &requires,
        &ensures_defined,
        &[],
        metrics,
        "ensures clause",
    ) {
        return Ok(ContractCheck::Invalid(error));
    }
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
            let proof = match verify_inner(contract, &candidate, metrics) {
                Ok(proof) => proof,
                Err(error) => return Ok(ContractCheck::Invalid(error)),
            };
            match proof {
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

/// Lowers a `fn` return expression into the AMI shared by verification and WASM.
/// Structurally recursive self-calls are statically unrolled to the depth bound.
pub fn implementation_to_ami(contract: &Contract) -> Result<Expr, String> {
    if contract.kind != axiom_parser::DeclarationKind::Function {
        return Err("only fn declarations have a local implementation".to_string());
    }
    axiom_parser::validate(contract)?;
    let implementation = contract
        .implementation
        .as_ref()
        .ok_or_else(|| "fn declaration has no return expression".to_string())?;
    let types = contract
        .params
        .iter()
        .map(|param| (param.name.clone(), param.ty.clone()))
        .collect::<HashMap<_, _>>();
    match lower_implementation_value(implementation, contract, &types, &HashMap::new(), 0)? {
        LoweredImplementation::Scalar {
            expr,
            defined: Expr::Const(1),
        } => Ok(expr),
        LoweredImplementation::Scalar { expr, defined } => Ok(Expr::Guard {
            condition: Box::new(defined),
            value: Box::new(expr),
        }),
        LoweredImplementation::Array { .. } => {
            Err("fn return expression must be scalar".to_string())
        }
    }
}

#[derive(Clone)]
enum LoweredImplementation {
    Scalar { expr: Expr, defined: Expr },
    Array { elements: Vec<Expr>, defined: Expr },
}

fn lower_implementation_value(
    expr: &SpecExpr,
    contract: &Contract,
    types: &HashMap<String, Type>,
    values: &HashMap<String, LoweredImplementation>,
    recursion_depth: u32,
) -> Result<LoweredImplementation, String> {
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
            let (offsets, _) = axiom_parser::verified_abi_layout(&contract.params)?;
            let offset = offsets[index];
            match &contract.params[index].ty {
                Type::FixedArray(_, length) => Ok(LoweredImplementation::Array {
                    elements: (0..*length)
                        .map(|element| {
                            let argument = offset + element;
                            u8::try_from(argument).map(Expr::Arg).map_err(|_| {
                                "verified ABI exceeds 256 flattened inputs".to_string()
                            })
                        })
                        .collect::<Result<Vec<_>, _>>()?,
                    defined: Expr::Const(1),
                }),
                _ => Ok(LoweredImplementation::Scalar {
                    expr: Expr::Arg(
                        u8::try_from(offset)
                            .map_err(|_| "verified ABI exceeds 256 flattened inputs".to_string())?,
                    ),
                    defined: Expr::Const(1),
                }),
            }
        }
        SpecExpr::Number(value) => Ok(LoweredImplementation::Scalar {
            expr: Expr::Const(*value),
            defined: Expr::Const(1),
        }),
        SpecExpr::Bool(value) => Ok(LoweredImplementation::Scalar {
            expr: Expr::Const(u32::from(*value)),
            defined: Expr::Const(1),
        }),
        SpecExpr::Let(name, value, body) => {
            let lowered =
                lower_implementation_value(value, contract, types, values, recursion_depth)?;
            let value_type = axiom_parser::type_of_expr(value, types, None)?;
            let mut nested_types = types.clone();
            nested_types.insert(name.clone(), value_type);
            let mut nested_values = values.clone();
            nested_values.insert(name.clone(), lowered.clone());
            let body = lower_implementation_value(
                body,
                contract,
                &nested_types,
                &nested_values,
                recursion_depth,
            )?;
            Ok(with_defined(body, lowered_defined(&lowered)))
        }
        SpecExpr::For {
            index,
            start,
            end,
            accumulator,
            initial,
            invariant,
            body,
        } => lower_bounded_loop(
            index,
            start,
            end,
            accumulator,
            initial,
            invariant,
            body,
            contract,
            types,
            values,
            recursion_depth,
        ),
        SpecExpr::If(condition, then_branch, else_branch) => {
            let condition =
                lower_implementation_value(condition, contract, types, values, recursion_depth)?;
            let LoweredImplementation::Scalar {
                expr: condition_expr,
                defined: condition_defined,
            } = condition
            else {
                return Err("if condition must be scalar boolean".to_string());
            };
            let then_branch =
                lower_implementation_value(then_branch, contract, types, values, recursion_depth)?;
            let else_branch =
                lower_implementation_value(else_branch, contract, types, values, recursion_depth)?;
            let defined = Expr::And(
                Box::new(condition_defined),
                Box::new(Expr::Select {
                    condition: Box::new(condition_expr.clone()),
                    then_branch: Box::new(lowered_defined(&then_branch)),
                    else_branch: Box::new(lowered_defined(&else_branch)),
                }),
            );
            match (then_branch, else_branch) {
                (
                    LoweredImplementation::Scalar {
                        expr: then_expr, ..
                    },
                    LoweredImplementation::Scalar {
                        expr: else_expr, ..
                    },
                ) => Ok(LoweredImplementation::Scalar {
                    expr: Expr::Select {
                        condition: Box::new(condition_expr),
                        then_branch: Box::new(then_expr),
                        else_branch: Box::new(else_expr),
                    },
                    defined,
                }),
                (
                    LoweredImplementation::Array {
                        elements: then_elements,
                        ..
                    },
                    LoweredImplementation::Array {
                        elements: else_elements,
                        ..
                    },
                ) if then_elements.len() == else_elements.len() => {
                    Ok(LoweredImplementation::Array {
                        elements: then_elements
                            .into_iter()
                            .zip(else_elements)
                            .map(|(then_expr, else_expr)| Expr::Select {
                                condition: Box::new(condition_expr.clone()),
                                then_branch: Box::new(then_expr),
                                else_branch: Box::new(else_expr),
                            })
                            .collect(),
                        defined,
                    })
                }
                _ => Err("if branches must have the same type".to_string()),
            }
        }
        SpecExpr::Field(array, field) if field == "length" => {
            let array =
                lower_implementation_value(array, contract, types, values, recursion_depth)?;
            match array {
                LoweredImplementation::Array { elements, defined } => {
                    Ok(LoweredImplementation::Scalar {
                        expr: Expr::Const(elements.len() as u32),
                        defined,
                    })
                }
                _ => Err("field access is only supported for fixed array length".to_string()),
            }
        }
        SpecExpr::Index(array, index) => {
            let array =
                lower_implementation_value(array, contract, types, values, recursion_depth)?;
            let index =
                lower_implementation_value(index, contract, types, values, recursion_depth)?;
            let (
                LoweredImplementation::Array {
                    elements,
                    defined: array_defined,
                },
                LoweredImplementation::Scalar {
                    expr: index_expr,
                    defined: index_defined,
                },
            ) = (array, index)
            else {
                return Err(
                    "indexed reads require a fixed verified array and scalar index".to_string(),
                );
            };
            let in_bounds = Expr::Lt(
                Box::new(index_expr.clone()),
                Box::new(Expr::Const(elements.len() as u32)),
            );
            Ok(LoweredImplementation::Scalar {
                expr: Expr::ArrayGet {
                    elements,
                    index: Box::new(index_expr),
                },
                defined: and_expr(and_expr(array_defined, index_defined), in_bounds),
            })
        }
        SpecExpr::ArrayUpdate(array, index, value) => {
            let array =
                lower_implementation_value(array, contract, types, values, recursion_depth)?;
            let index =
                lower_implementation_value(index, contract, types, values, recursion_depth)?;
            let value =
                lower_implementation_value(value, contract, types, values, recursion_depth)?;
            let (
                LoweredImplementation::Array {
                    elements,
                    defined: array_defined,
                },
                LoweredImplementation::Scalar {
                    expr: index_expr,
                    defined: index_defined,
                },
                LoweredImplementation::Scalar {
                    expr: value_expr,
                    defined: value_defined,
                },
            ) = (array, index, value)
            else {
                return Err(
                    "array_update requires a fixed array, scalar index, and scalar value"
                        .to_string(),
                );
            };
            let in_bounds = Expr::Lt(
                Box::new(index_expr.clone()),
                Box::new(Expr::Const(elements.len() as u32)),
            );
            Ok(LoweredImplementation::Array {
                elements: elements
                    .into_iter()
                    .enumerate()
                    .map(|(position, element)| Expr::Select {
                        condition: Box::new(Expr::Eq(
                            Box::new(index_expr.clone()),
                            Box::new(Expr::Const(position as u32)),
                        )),
                        then_branch: Box::new(value_expr.clone()),
                        else_branch: Box::new(element),
                    })
                    .collect(),
                defined: and_expr(
                    and_expr(array_defined, index_defined),
                    and_expr(value_defined, in_bounds),
                ),
            })
        }
        SpecExpr::Binary(op, lhs, rhs) => {
            let lhs_type = axiom_parser::type_of_expr(lhs, types, None)?;
            let lhs = lower_implementation_value(lhs, contract, types, values, recursion_depth)?;
            let rhs = lower_implementation_value(rhs, contract, types, values, recursion_depth)?;
            match (lhs, rhs) {
                (
                    LoweredImplementation::Array {
                        elements: lhs,
                        defined: lhs_defined,
                    },
                    LoweredImplementation::Array {
                        elements: rhs,
                        defined: rhs_defined,
                    },
                ) if matches!(op, BinaryOp::Eq | BinaryOp::Ne) && lhs.len() == rhs.len() => {
                    let comparisons = lhs.into_iter().zip(rhs).map(|(lhs, rhs)| match op {
                        BinaryOp::Eq => Expr::Eq(Box::new(lhs), Box::new(rhs)),
                        BinaryOp::Ne => Expr::Ne(Box::new(lhs), Box::new(rhs)),
                        _ => unreachable!("array equality was checked above"),
                    });
                    let mut comparisons = comparisons;
                    let first = comparisons
                        .next()
                        .ok_or_else(|| "fixed arrays cannot be empty".to_string())?;
                    let comparison = comparisons.fold(first, |combined, next| {
                        if *op == BinaryOp::Eq {
                            and_expr(combined, next)
                        } else {
                            or_expr(combined, next)
                        }
                    });
                    Ok(LoweredImplementation::Scalar {
                        expr: comparison,
                        defined: and_expr(lhs_defined, rhs_defined),
                    })
                }
                (
                    LoweredImplementation::Scalar {
                        expr: lhs,
                        defined: lhs_defined,
                    },
                    LoweredImplementation::Scalar {
                        expr: rhs,
                        defined: rhs_defined,
                    },
                ) => {
                    let defined = match op {
                        BinaryOp::And => and_expr(
                            lhs_defined,
                            Expr::Select {
                                condition: Box::new(lhs.clone()),
                                then_branch: Box::new(rhs_defined),
                                else_branch: Box::new(Expr::Const(1)),
                            },
                        ),
                        BinaryOp::Or => and_expr(
                            lhs_defined,
                            Expr::Select {
                                condition: Box::new(lhs.clone()),
                                then_branch: Box::new(Expr::Const(1)),
                                else_branch: Box::new(rhs_defined),
                            },
                        ),
                        _ => and_expr(lhs_defined, rhs_defined),
                    };
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
                        BinaryOp::Lt if lhs_type == Type::I32 => {
                            Expr::SLt(Box::new(lhs), Box::new(rhs))
                        }
                        BinaryOp::Le if lhs_type == Type::I32 => {
                            Expr::SLe(Box::new(lhs), Box::new(rhs))
                        }
                        BinaryOp::Gt if lhs_type == Type::I32 => {
                            Expr::SGt(Box::new(lhs), Box::new(rhs))
                        }
                        BinaryOp::Ge if lhs_type == Type::I32 => {
                            Expr::SGe(Box::new(lhs), Box::new(rhs))
                        }
                        BinaryOp::Lt => Expr::Lt(Box::new(lhs), Box::new(rhs)),
                        BinaryOp::Le => Expr::Le(Box::new(lhs), Box::new(rhs)),
                        BinaryOp::Gt => Expr::Gt(Box::new(lhs), Box::new(rhs)),
                        BinaryOp::Ge => Expr::Ge(Box::new(lhs), Box::new(rhs)),
                        BinaryOp::And => Expr::And(Box::new(lhs.clone()), Box::new(rhs)),
                        BinaryOp::Or => Expr::Or(Box::new(lhs.clone()), Box::new(rhs)),
                    };
                    Ok(LoweredImplementation::Scalar {
                        expr: binary,
                        defined,
                    })
                }
                _ => Err("binary operators require compatible scalar operands".to_string()),
            }
        }
        SpecExpr::Call(name, arguments) if name == &contract.name => {
            if recursion_depth >= axiom_parser::MAX_VERIFIED_RECURSION_DEPTH {
                return Ok(LoweredImplementation::Scalar {
                    expr: Expr::Const(0),
                    defined: Expr::Const(0),
                });
            }
            let [argument] = arguments.as_slice() else {
                return Err("structural recursion requires one argument".to_string());
            };
            let LoweredImplementation::Scalar {
                expr: argument,
                defined: argument_defined,
            } = lower_implementation_value(argument, contract, types, values, recursion_depth)?
            else {
                return Err("structural recursion requires a scalar measure".to_string());
            };
            let param = contract
                .params
                .first()
                .ok_or_else(|| "structural recursion is missing its measure".to_string())?;
            let function_types = contract
                .params
                .iter()
                .map(|param| (param.name.clone(), param.ty.clone()))
                .collect::<HashMap<_, _>>();
            let recursive_values = HashMap::from([(
                param.name.clone(),
                LoweredImplementation::Scalar {
                    expr: argument,
                    defined: Expr::Const(1),
                },
            )]);
            let implementation = contract
                .implementation
                .as_ref()
                .ok_or_else(|| "recursive fn declaration has no implementation".to_string())?;
            let recursive = lower_implementation_value(
                implementation,
                contract,
                &function_types,
                &recursive_values,
                recursion_depth + 1,
            )?;
            Ok(with_defined(recursive, argument_defined))
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

fn lower_bounded_loop(
    index: &String,
    start: &u32,
    end: &u32,
    accumulator: &String,
    initial: &SpecExpr,
    invariant: &SpecExpr,
    body: &SpecExpr,
    contract: &Contract,
    types: &HashMap<String, Type>,
    values: &HashMap<String, LoweredImplementation>,
    recursion_depth: u32,
) -> Result<LoweredImplementation, String> {
    let mut current =
        lower_implementation_value(initial, contract, types, values, recursion_depth)?;
    let accumulator_type = axiom_parser::type_of_expr(initial, types, None)?;
    let mut loop_types = types.clone();
    loop_types.insert(index.clone(), Type::U32);
    loop_types.insert(accumulator.clone(), accumulator_type);
    let mut loop_defined = lowered_defined(&current);

    for iteration in *start..*end {
        let mut loop_values = values.clone();
        loop_values.insert(
            index.clone(),
            LoweredImplementation::Scalar {
                expr: Expr::Const(iteration),
                defined: Expr::Const(1),
            },
        );
        loop_values.insert(accumulator.clone(), current.clone());

        let invariant_value = lower_implementation_value(
            invariant,
            contract,
            &loop_types,
            &loop_values,
            recursion_depth,
        )?;
        let LoweredImplementation::Scalar {
            expr: invariant_expr,
            defined: invariant_defined,
        } = invariant_value
        else {
            return Err("loop invariant must be boolean".to_string());
        };
        loop_defined = and_expr(loop_defined, and_expr(invariant_defined, invariant_expr));

        current =
            lower_implementation_value(body, contract, &loop_types, &loop_values, recursion_depth)?;
        loop_defined = and_expr(loop_defined, lowered_defined(&current));
    }

    let mut final_values = values.clone();
    final_values.insert(
        index.clone(),
        LoweredImplementation::Scalar {
            expr: Expr::Const(*end),
            defined: Expr::Const(1),
        },
    );
    final_values.insert(accumulator.clone(), current.clone());
    let final_invariant = lower_implementation_value(
        invariant,
        contract,
        &loop_types,
        &final_values,
        recursion_depth,
    )?;
    let LoweredImplementation::Scalar {
        expr: final_invariant_expr,
        defined: final_invariant_defined,
    } = final_invariant
    else {
        return Err("loop invariant must be boolean".to_string());
    };
    loop_defined = and_expr(
        loop_defined,
        and_expr(final_invariant_defined, final_invariant_expr),
    );
    Ok(with_defined(current, loop_defined))
}

fn lowered_defined(value: &LoweredImplementation) -> Expr {
    match value {
        LoweredImplementation::Scalar { defined, .. }
        | LoweredImplementation::Array { defined, .. } => defined.clone(),
    }
}

fn with_defined(value: LoweredImplementation, additional: Expr) -> LoweredImplementation {
    match value {
        LoweredImplementation::Scalar { expr, defined } => LoweredImplementation::Scalar {
            expr,
            defined: and_expr(additional, defined),
        },
        LoweredImplementation::Array { elements, defined } => LoweredImplementation::Array {
            elements,
            defined: and_expr(additional, defined),
        },
    }
}

fn and_expr(lhs: Expr, rhs: Expr) -> Expr {
    match (lhs, rhs) {
        (Expr::Const(0), _) | (_, Expr::Const(0)) => Expr::Const(0),
        (Expr::Const(1), rhs) => rhs,
        (lhs, Expr::Const(1)) => lhs,
        (lhs, rhs) => Expr::And(Box::new(lhs), Box::new(rhs)),
    }
}

fn or_expr(lhs: Expr, rhs: Expr) -> Expr {
    match (lhs, rhs) {
        (Expr::Const(1), _) | (_, Expr::Const(1)) => Expr::Const(1),
        (Expr::Const(0), rhs) => rhs,
        (lhs, Expr::Const(0)) => lhs,
        (lhs, rhs) => Expr::Or(Box::new(lhs), Box::new(rhs)),
    }
}

#[derive(Clone)]
enum Value {
    U32(BV),
    I32(BV),
    Bool(Bool),
    Array(Vec<BV>),
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
    let candidate_defined = ami_definedness(candidate, &args)?;
    require_defined_when(
        contract,
        &args,
        &requires,
        &candidate_defined,
        &[],
        metrics,
        "candidate",
    )?;

    if contract.ensures.is_empty() {
        return Err("contract must have at least one ensures clause".to_string());
    }

    let ensures_defined = contract
        .ensures
        .iter()
        .map(|clause| spec_definedness(clause, contract, &args, &candidate_ast, &empty_locals))
        .collect::<Result<Vec<_>, _>>()?;
    let ensures_defined = Bool::and(&ensures_defined.iter().collect::<Vec<_>>());
    require_defined_when(
        contract,
        &args,
        &requires,
        &ensures_defined,
        &[],
        metrics,
        "ensures clause",
    )?;

    let ensures = contract
        .ensures
        .iter()
        .map(
            |expr| match spec_to_value(expr, contract, &args, &candidate_ast, &empty_locals)? {
                Value::Bool(value) => Ok(value),
                Value::U32(_) | Value::I32(_) | Value::Array(_) => {
                    Err("ensures clause must be boolean".to_string())
                }
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
    for clause in &contract.requires {
        let defined = spec_definedness(clause, contract, &args, &result, &empty_locals)?;
        require_defined_when(
            contract,
            &args,
            &true_path,
            &defined,
            &[],
            metrics,
            "requires clause",
        )?;
    }
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
            Value::U32(_) | Value::I32(_) | Value::Array(_) => {
                return Err("requires clause must be boolean".to_string());
            }
        }
    }
    let requires_refs: Vec<&Bool> = requires_values.iter().collect();
    let requires = Bool::and(&requires_refs);
    let candidate_defined = ami_definedness(candidate, &args)?;
    require_defined_when(
        contract,
        &args,
        &requires,
        &candidate_defined,
        &calls.constraints,
        metrics,
        "candidate",
    )?;
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
        let defined = spec_definedness(&prepared, contract, &args, &result, &locals)?;
        require_defined_when(
            contract,
            &args,
            &requires,
            &defined,
            &calls.constraints,
            metrics,
            "ensures clause",
        )?;
        match spec_to_value(&prepared, contract, &args, &result, &locals)? {
            Value::Bool(value) => ensures_values.push(value),
            Value::U32(_) | Value::I32(_) | Value::Array(_) => {
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
            let callee_args = arg_values
                .iter()
                .map(value_as_bv)
                .collect::<Result<Vec<_>, _>>()?;
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
                Value::U32(_) | Value::I32(_) | Value::Array(_) => {
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
        SpecExpr::Index(array, index) => Ok(SpecExpr::Index(
            Box::new(prepare_calls(
                array, contract, args, result, locals, path, callees, calls,
            )?),
            Box::new(prepare_calls(
                index, contract, args, result, locals, path, callees, calls,
            )?),
        )),
        SpecExpr::ArrayUpdate(array, index, value) => Ok(SpecExpr::ArrayUpdate(
            Box::new(prepare_calls(
                array, contract, args, result, locals, path, callees, calls,
            )?),
            Box::new(prepare_calls(
                index, contract, args, result, locals, path, callees, calls,
            )?),
            Box::new(prepare_calls(
                value, contract, args, result, locals, path, callees, calls,
            )?),
        )),
        SpecExpr::For { .. } => {
            Err("bounded loops are only allowed in fn implementation expressions".to_string())
        }
        SpecExpr::Ident(_) | SpecExpr::Number(_) | SpecExpr::Bool(_) | SpecExpr::Text(_) => {
            Ok(expr.clone())
        }
    }
}

fn value_as_bv(value: &Value) -> Result<BV, String> {
    match value {
        Value::U32(value) | Value::I32(value) => Ok(value.clone()),
        Value::Bool(value) => Ok(value.ite(&BV::from_u64(1, 32), &BV::from_u64(0, 32))),
        Value::Array(_) => Err("compositional calls cannot pass fixed arrays yet".to_string()),
    }
}

fn symbolic_args(contract: &Contract) -> Result<Vec<BV>, String> {
    let mut args = Vec::new();
    for param in &contract.params {
        match &param.ty {
            Type::U32 | Type::I32 | Type::Bool => {
                args.push(BV::new_const(param.name.as_str(), 32));
            }
            Type::FixedArray(element, length) if **element == Type::U32 => {
                for index in 0..*length {
                    args.push(BV::new_const(format!("{}_{}", param.name, index), 32));
                }
            }
            _ => return Err(format!("unsupported parameter type: {}", param.name)),
        }
    }
    Ok(args)
}

fn constrain_bool_parameters(solver: &Solver, contract: &Contract, args: &[BV]) {
    let mut offset = 0;
    for param in &contract.params {
        if param.ty == Type::Bool {
            let zero = args[offset].eq(BV::from_u64(0, 32));
            let one = args[offset].eq(BV::from_u64(1, 32));
            solver.assert(Bool::or(&[&zero, &one]));
        }
        offset += match param.ty {
            Type::FixedArray(_, length) => length,
            _ => 1,
        };
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
                Value::U32(_) | Value::I32(_) | Value::Array(_) => {
                    Err("requires clause must be boolean".to_string())
                }
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
                Value::U32(_) | Value::I32(_) | Value::Array(_) => {
                    Err("ensures clause must be boolean".to_string())
                }
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
        Expr::ArrayGet { elements, index } => {
            let index = expr_to_bv(index, args)?;
            let mut selected = BV::from_u64(0, 32);
            for (position, element) in elements.iter().enumerate().rev() {
                let matches = index.eq(BV::from_u64(position as u64, 32));
                selected = matches.ite(&expr_to_bv(element, args)?, &selected);
            }
            Ok(selected)
        }
        Expr::Guard { value, .. } => expr_to_bv(value, args),
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

fn require_defined_when(
    contract: &Contract,
    args: &[BV],
    assumption: &Bool,
    defined: &Bool,
    constraints: &[Bool],
    metrics: &mut Option<&mut VerificationMetrics>,
    subject: &str,
) -> Result<(), String> {
    let solver = Solver::new();
    constrain_bool_parameters(&solver, contract, args);
    for constraint in constraints {
        solver.assert(constraint);
    }
    let violation = Bool::and(&[assumption, &defined.not()]);
    solver.assert(&violation);
    match solver_check(&solver, metrics) {
        SatResult::Unsat => Ok(()),
        SatResult::Sat => Err(format!(
            "{subject} has an unproven bounds, loop-invariant, or recursion-depth guard at {:?}",
            model_inputs(&solver, args)?
        )),
        SatResult::Unknown => Err(format!(
            "{subject} fixed-array bounds check is solver-unknown"
        )),
    }
}

fn spec_definedness(
    expr: &SpecExpr,
    contract: &Contract,
    args: &[BV],
    result: &BV,
    locals: &HashMap<String, Value>,
) -> Result<Bool, String> {
    let and = |values: Vec<Bool>| Bool::and(&values.iter().collect::<Vec<_>>());
    match expr {
        SpecExpr::Index(array, index_expr) => {
            let array_value = spec_to_value(array, contract, args, result, locals)?;
            let Value::Array(elements) = array_value else {
                return Err("indexed reads require a fixed verified array".to_string());
            };
            let index_value = spec_to_value(index_expr, contract, args, result, locals)?;
            let index = match index_value {
                Value::U32(index) | Value::I32(index) => index,
                _ => return Err("array index must be u32 or i32".to_string()),
            };
            Ok(and(vec![
                spec_definedness(array, contract, args, result, locals)?,
                spec_definedness(index_expr, contract, args, result, locals)?,
                index.bvult(BV::from_u64(elements.len() as u64, 32)),
            ]))
        }
        SpecExpr::ArrayUpdate(array, index_expr, value) => {
            let array_value = spec_to_value(array, contract, args, result, locals)?;
            let Value::Array(elements) = array_value else {
                return Err("array_update requires a fixed verified array".to_string());
            };
            let index_value = spec_to_value(index_expr, contract, args, result, locals)?;
            let index = match index_value {
                Value::U32(index) | Value::I32(index) => index,
                _ => return Err("array index must be u32 or i32".to_string()),
            };
            Ok(and(vec![
                spec_definedness(array, contract, args, result, locals)?,
                spec_definedness(index_expr, contract, args, result, locals)?,
                spec_definedness(value, contract, args, result, locals)?,
                index.bvult(BV::from_u64(elements.len() as u64, 32)),
            ]))
        }
        SpecExpr::Let(name, value, body) => {
            let value_result = spec_to_value(value, contract, args, result, locals)?;
            let mut nested = locals.clone();
            nested.insert(name.clone(), value_result);
            Ok(and(vec![
                spec_definedness(value, contract, args, result, locals)?,
                spec_definedness(body, contract, args, result, &nested)?,
            ]))
        }
        SpecExpr::If(condition, then_branch, else_branch) => {
            let condition_value = match spec_to_value(condition, contract, args, result, locals)? {
                Value::Bool(condition) => condition,
                _ => return Err("if condition must be boolean".to_string()),
            };
            let then_defined = spec_definedness(then_branch, contract, args, result, locals)?;
            let else_defined = spec_definedness(else_branch, contract, args, result, locals)?;
            Ok(and(vec![
                spec_definedness(condition, contract, args, result, locals)?,
                condition_value.ite(&then_defined, &else_defined),
            ]))
        }
        SpecExpr::Binary(BinaryOp::And, lhs, rhs) => {
            let Value::Bool(lhs_value) = spec_to_value(lhs, contract, args, result, locals)? else {
                return Err("logical conjunction requires boolean operands".to_string());
            };
            let rhs_defined = spec_definedness(rhs, contract, args, result, locals)?;
            Ok(and(vec![
                spec_definedness(lhs, contract, args, result, locals)?,
                Bool::or(&[&lhs_value.not(), &rhs_defined]),
            ]))
        }
        SpecExpr::Binary(BinaryOp::Or, lhs, rhs) => {
            let Value::Bool(lhs_value) = spec_to_value(lhs, contract, args, result, locals)? else {
                return Err("logical disjunction requires boolean operands".to_string());
            };
            let rhs_defined = spec_definedness(rhs, contract, args, result, locals)?;
            Ok(and(vec![
                spec_definedness(lhs, contract, args, result, locals)?,
                Bool::or(&[&lhs_value, &rhs_defined]),
            ]))
        }
        SpecExpr::Binary(_, lhs, rhs) => Ok(and(vec![
            spec_definedness(lhs, contract, args, result, locals)?,
            spec_definedness(rhs, contract, args, result, locals)?,
        ])),
        SpecExpr::Call(_, values) | SpecExpr::List(values) => Ok(and(values
            .iter()
            .map(|value| spec_definedness(value, contract, args, result, locals))
            .collect::<Result<Vec<_>, _>>()?)),
        SpecExpr::Record { fields, .. } => Ok(and(fields
            .iter()
            .map(|(_, value)| spec_definedness(value, contract, args, result, locals))
            .collect::<Result<Vec<_>, _>>()?)),
        SpecExpr::Field(value, _) => spec_definedness(value, contract, args, result, locals),
        SpecExpr::For { .. } => {
            Err("bounded loops are only allowed in fn implementation expressions".to_string())
        }
        SpecExpr::Ident(_) | SpecExpr::Number(_) | SpecExpr::Bool(_) | SpecExpr::Text(_) => {
            Ok(Bool::from_bool(true))
        }
    }
}

fn ami_definedness(expr: &Expr, args: &[BV]) -> Result<Bool, String> {
    let and = |values: Vec<Bool>| Bool::and(&values.iter().collect::<Vec<_>>());
    match expr {
        Expr::Arg(_) | Expr::Const(_) => Ok(Bool::from_bool(true)),
        Expr::ArrayGet { elements, index } => {
            let index_value = expr_to_bv(index, args)?;
            let index_defined = ami_definedness(index, args)?;
            let mut selected_defined = Vec::with_capacity(elements.len());
            for (position, element) in elements.iter().enumerate() {
                let is_selected = index_value.eq(BV::from_u64(position as u64, 32));
                let element_defined = ami_definedness(element, args)?;
                selected_defined.push(Bool::or(&[&is_selected.not(), &element_defined]));
            }
            Ok(and(vec![
                index_defined,
                index_value.bvult(BV::from_u64(elements.len() as u64, 32)),
                and(selected_defined),
            ]))
        }
        Expr::Guard { condition, value } => {
            let condition_value = expr_to_bv(condition, args)?;
            let value_defined = ami_definedness(value, args)?;
            Ok(and(vec![
                ami_definedness(condition, args)?,
                condition_value.ne(BV::from_u64(0, 32)),
                value_defined,
            ]))
        }
        Expr::Select {
            condition,
            then_branch,
            else_branch,
        } => {
            let condition_value = expr_to_bool(condition, args)?;
            let then_defined = ami_definedness(then_branch, args)?;
            let else_defined = ami_definedness(else_branch, args)?;
            Ok(and(vec![
                ami_definedness(condition, args)?,
                condition_value.ite(&then_defined, &else_defined),
            ]))
        }
        Expr::And(lhs, rhs) | Expr::Or(lhs, rhs) => {
            let lhs_value = expr_to_bool(lhs, args)?;
            let rhs_defined = ami_definedness(rhs, args)?;
            let rhs_required = if matches!(expr, Expr::And(_, _)) {
                lhs_value
            } else {
                lhs_value.not()
            };
            Ok(and(vec![
                ami_definedness(lhs, args)?,
                Bool::or(&[&rhs_required.not(), &rhs_defined]),
            ]))
        }
        Expr::Add(lhs, rhs)
        | Expr::Sub(lhs, rhs)
        | Expr::Mul(lhs, rhs)
        | Expr::Shl(lhs, rhs)
        | Expr::BitAnd(lhs, rhs)
        | Expr::BitOr(lhs, rhs)
        | Expr::BitXor(lhs, rhs)
        | Expr::Eq(lhs, rhs)
        | Expr::Ne(lhs, rhs)
        | Expr::Lt(lhs, rhs)
        | Expr::Le(lhs, rhs)
        | Expr::Gt(lhs, rhs)
        | Expr::Ge(lhs, rhs)
        | Expr::SLt(lhs, rhs)
        | Expr::SLe(lhs, rhs)
        | Expr::SGt(lhs, rhs)
        | Expr::SGe(lhs, rhs) => Ok(and(vec![
            ami_definedness(lhs, args)?,
            ami_definedness(rhs, args)?,
        ])),
    }
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
            let (offsets, _) = axiom_parser::verified_abi_layout(&contract.params)?;
            let offset = offsets[index];
            match contract.params[index].ty {
                Type::Bool => Ok(Value::Bool(args[offset].ne(BV::from_u64(0, 32)))),
                Type::I32 => Ok(Value::I32(args[offset].clone())),
                Type::FixedArray(_, length) => {
                    Ok(Value::Array(args[offset..offset + length].to_vec()))
                }
                _ => Ok(Value::U32(args[offset].clone())),
            }
        }
        SpecExpr::Number(value) => Ok(Value::U32(BV::from_u64(*value as u64, 32))),
        SpecExpr::Bool(value) => Ok(Value::Bool(Bool::from_bool(*value))),
        SpecExpr::Field(value, field) => {
            match spec_to_value(value, contract, args, result, locals)? {
                Value::Array(elements) if field == "length" => {
                    Ok(Value::U32(BV::from_u64(elements.len() as u64, 32)))
                }
                _ => Err("field access is only supported for fixed array length".to_string()),
            }
        }
        SpecExpr::Index(array, index) => {
            let array = spec_to_value(array, contract, args, result, locals)?;
            let index = match spec_to_value(index, contract, args, result, locals)? {
                Value::U32(index) | Value::I32(index) => index,
                _ => return Err("array index must be u32 or i32".to_string()),
            };
            let Value::Array(elements) = array else {
                return Err("indexed reads require a fixed verified array".to_string());
            };
            let mut selected = BV::from_u64(0, 32);
            for (position, element) in elements.iter().enumerate().rev() {
                let matches = index.eq(BV::from_u64(position as u64, 32));
                selected = matches.ite(element, &selected);
            }
            Ok(Value::U32(selected))
        }
        SpecExpr::ArrayUpdate(array, index, value) => {
            let array = spec_to_value(array, contract, args, result, locals)?;
            let index = match spec_to_value(index, contract, args, result, locals)? {
                Value::U32(index) | Value::I32(index) => index,
                _ => return Err("array index must be u32 or i32".to_string()),
            };
            let value = match spec_to_value(value, contract, args, result, locals)? {
                Value::U32(value) => value,
                _ => return Err("array update value must be u32".to_string()),
            };
            let Value::Array(elements) = array else {
                return Err("array_update requires a fixed verified array".to_string());
            };
            Ok(Value::Array(
                elements
                    .iter()
                    .enumerate()
                    .map(|(position, element)| {
                        index
                            .eq(BV::from_u64(position as u64, 32))
                            .ite(&value, element)
                    })
                    .collect(),
            ))
        }
        SpecExpr::Text(_) | SpecExpr::List(_) | SpecExpr::Record { .. } => {
            Err("non-scalar expressions cannot be verified as contracts".to_string())
        }
        SpecExpr::Let(name, value, body) => {
            let value = spec_to_value(value, contract, args, result, locals)?;
            let mut nested = locals.clone();
            nested.insert(name.clone(), value);
            spec_to_value(body, contract, args, result, &nested)
        }
        SpecExpr::For { .. } => {
            Err("bounded loops are only allowed in fn implementation expressions".to_string())
        }
        SpecExpr::If(condition, then_branch, else_branch) => {
            let condition = match spec_to_value(condition, contract, args, result, locals)? {
                Value::Bool(value) => value,
                Value::U32(_) | Value::I32(_) | Value::Array(_) => {
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
                (Value::Array(then_branch), Value::Array(else_branch))
                    if then_branch.len() == else_branch.len() =>
                {
                    Ok(Value::Array(
                        then_branch
                            .iter()
                            .zip(&else_branch)
                            .map(|(then_value, else_value)| condition.ite(then_value, else_value))
                            .collect(),
                    ))
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
                (BinaryOp::Eq, Value::Array(lhs), Value::Array(rhs)) if lhs.len() == rhs.len() => {
                    let equalities = lhs
                        .iter()
                        .zip(&rhs)
                        .map(|(lhs, rhs)| lhs.eq(rhs))
                        .collect::<Vec<_>>();
                    let refs = equalities.iter().collect::<Vec<_>>();
                    Ok(Value::Bool(Bool::and(&refs)))
                }
                (BinaryOp::Ne, Value::Array(lhs), Value::Array(rhs)) if lhs.len() == rhs.len() => {
                    let equalities = lhs
                        .iter()
                        .zip(&rhs)
                        .map(|(lhs, rhs)| lhs.eq(rhs))
                        .collect::<Vec<_>>();
                    let refs = equalities.iter().collect::<Vec<_>>();
                    Ok(Value::Bool(Bool::and(&refs).not()))
                }
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
    fn structural_recursion_is_proved_bounded_and_traps_over_the_depth_limit() {
        let function = parse(
            "fn sum_to(n: u32) -> u32 { \
             requires n <= 16; \
             return if n == 0 { 0 } else { 1 +% sum_to(n -% 1) }; \
             ensures result == n; }",
        )
        .unwrap();
        assert_eq!(check_contract(&function).unwrap(), ContractCheck::Valid);
        let candidate = implementation_to_ami(&function).unwrap();
        assert_eq!(
            verify(&function, &candidate).unwrap(),
            ProofResult::Verified
        );
        let module = axiom_wasm::emit(&candidate, 1).unwrap();

        for n in [0, 1, axiom_parser::MAX_VERIFIED_RECURSION_DEPTH] {
            let args = [n];
            assert_eq!(candidate.try_eval(&args).unwrap(), n);
            assert_eq!(reference::eval_ami(&candidate, &args).unwrap(), n);
            assert_eq!(axiom_wasm::run_with_args(&module, &args).unwrap(), n);
            assert!(reference::contract_holds(&function, &candidate, &args).unwrap());
        }
        assert!(
            candidate
                .try_eval(&[axiom_parser::MAX_VERIFIED_RECURSION_DEPTH + 1])
                .is_err()
        );
        assert!(
            axiom_wasm::run_with_args(&module, &[axiom_parser::MAX_VERIFIED_RECURSION_DEPTH + 1])
                .is_err()
        );

        let unbounded = parse(
            "fn unbounded(n: u32) -> u32 { \
             return if n == 0 { 0 } else { 1 +% unbounded(n -% 1) }; \
             ensures result == n; }",
        )
        .unwrap();
        assert!(matches!(
            check_contract(&unbounded).unwrap(),
            ContractCheck::Invalid(message) if message.contains("recursion-depth")
        ));
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

    #[test]
    fn fixed_array_reads_match_reference_z3_and_wasm_at_boundaries() {
        let contract = parse(
            "fn read(values: [u32; 3], index: u32) -> u32 { \
             requires index < values.length; \
             return values[index]; \
             ensures result == values[index]; }",
        )
        .unwrap();
        let candidate = implementation_to_ami(&contract).unwrap();
        assert_eq!(check_contract(&contract).unwrap(), ContractCheck::Valid);
        assert_eq!(
            verify(&contract, &candidate).unwrap(),
            ProofResult::Verified
        );

        let module = axiom_wasm::emit(&candidate, 4).unwrap();
        for (index, expected) in [(0, 11), (2, 33)] {
            let args = [11, 22, 33, index];
            assert_eq!(candidate.try_eval(&args).unwrap(), expected);
            assert_eq!(reference::eval_ami(&candidate, &args).unwrap(), expected);
            assert_eq!(axiom_wasm::run_with_args(&module, &args).unwrap(), expected);
            assert!(reference::contract_holds(&contract, &candidate, &args).unwrap());
        }

        let out_of_bounds = [11, 22, 33, 3];
        assert!(candidate.try_eval(&out_of_bounds).is_err());
        assert!(axiom_wasm::run_with_args(&module, &out_of_bounds).is_err());
        assert!(reference::contract_holds(&contract, &candidate, &out_of_bounds).unwrap());

        let length_contract = parse(
            "fn length(values: [u32; 3]) -> u32 { return values.length; ensures result == 3; }",
        )
        .unwrap();
        let length_candidate = implementation_to_ami(&length_contract).unwrap();
        assert_eq!(
            verify(&length_contract, &length_candidate).unwrap(),
            ProofResult::Verified
        );
        assert_eq!(length_candidate.try_eval(&[11, 22, 33]).unwrap(), 3);
    }

    #[test]
    fn fixed_array_updates_copy_and_preserve_unchanged_elements() {
        let contract = parse(
            "fn update(values: [u32; 3], index: u32, replacement: u32) -> u32 { \
             requires index < values.length; \
             return array_update(values, index, replacement)[index]; \
             ensures result == replacement && array_update(values, index, replacement)[0] == if index == 0 { replacement } else { values[0] }; }",
        )
        .unwrap();
        let candidate = implementation_to_ami(&contract).unwrap();
        assert_eq!(check_contract(&contract).unwrap(), ContractCheck::Valid);

        let args = [11, 22, 33, 2, 99];
        assert_eq!(candidate.try_eval(&args).unwrap(), 99);
        assert!(reference::contract_holds(&contract, &candidate, &args).unwrap());
        let module = axiom_wasm::emit(&candidate, 5).unwrap();
        assert_eq!(axiom_wasm::run_with_args(&module, &args).unwrap(), 99);

        let update = &contract.ensures[0];
        assert_eq!(
            reference::eval_spec(update, &contract, &args, 99).unwrap(),
            reference::Value::Bool(true)
        );
    }

    #[test]
    fn signed_negative_and_out_of_bounds_indices_are_rejected() {
        let contract = parse(
            "fn read(values: [u32; 2], index: i32) -> u32 { \
             requires index >= 0; \
             requires index < 2; \
             return values[index]; \
             ensures result == values[index]; }",
        )
        .unwrap();
        let candidate = implementation_to_ami(&contract).unwrap();
        assert_eq!(check_contract(&contract).unwrap(), ContractCheck::Valid);
        assert_eq!(
            verify(&contract, &candidate).unwrap(),
            ProofResult::Verified
        );
        assert!(candidate.try_eval(&[7, 8, u32::MAX]).is_err());
        assert!(candidate.try_eval(&[7, 8, 2]).is_err());

        let unsafe_contract = parse(
            "fn unsafe_read(values: [u32; 2], index: u32) -> u32 { \
             return values[index]; \
             ensures result == 0; }",
        )
        .unwrap();
        assert!(matches!(
            check_contract(&unsafe_contract).unwrap(),
            ContractCheck::Invalid(message) if message.contains("unproven")
        ));

        let unsafe_update = parse(
            "fn unsafe_update(values: [u32; 2], index: u32) -> u32 { \
             return array_update(values, index, 0)[0]; \
             ensures result == 0; }",
        )
        .unwrap();
        assert!(matches!(
            check_contract(&unsafe_update).unwrap(),
            ContractCheck::Invalid(message) if message.contains("unproven")
        ));

        let unsafe_requires = parse(
            "contract unsafe_requires(values: [u32; 2], index: u32) -> u32 { \
             requires values[index] == 0; \
             ensures result == 0; }",
        )
        .unwrap();
        assert!(matches!(
            check_contract(&unsafe_requires).unwrap(),
            ContractCheck::Invalid(message) if message.contains("requires clause")
        ));
    }

    #[test]
    fn short_circuit_conditions_guard_fixed_array_accesses() {
        for source in [
            "contract guarded_and(values: [u32; 2], index: u32) -> u32 { \
             requires index < values.length && values[index] == 0; \
             ensures result == 0; }",
            "contract guarded_or(values: [u32; 2], index: u32) -> u32 { \
             requires index >= values.length || values[index] == 0; \
             ensures result == 0; }",
        ] {
            let contract = parse(source).unwrap();
            assert_eq!(
                check_preconditions(&contract).unwrap(),
                PreconditionResult::Satisfiable
            );
            assert_eq!(check_contract(&contract).unwrap(), ContractCheck::Valid);
            for args in [[0, 1, 0], [0, 1, 2], [1, 1, 0]] {
                assert!(reference::contract_holds(&contract, &Expr::Const(0), &args).unwrap());
            }
        }

        let conditional = parse(
            "fn guarded_read(values: [u32; 2], index: u32) -> u32 { \
             return if index < values.length { values[index] } else { 0 }; \
             ensures result == if index < values.length { values[index] } else { 0 }; }",
        )
        .unwrap();
        let candidate = implementation_to_ami(&conditional).unwrap();
        assert_eq!(check_contract(&conditional).unwrap(), ContractCheck::Valid);
        assert_eq!(candidate.try_eval(&[7, 8, 2]).unwrap(), 0);
        assert!(reference::contract_holds(&conditional, &candidate, &[7, 8, 2]).unwrap());

        let guarded_ensures = parse(
            "contract guarded_ensures(values: [u32; 2], index: u32) -> u32 { \
             ensures index >= values.length || result == values[index]; }",
        )
        .unwrap();
        assert_eq!(
            check_contract(&guarded_ensures).unwrap(),
            ContractCheck::Valid
        );
        assert!(reference::contract_holds(&guarded_ensures, &Expr::Const(0), &[7, 8, 2]).unwrap());
    }

    #[test]
    fn fixed_array_equality_matches_reference_z3_and_wasm() {
        let contract = parse(
            "fn equal(left: [u32; 2], right: [u32; 2]) -> bool { \
             return left == right; \
             ensures result == (left == right); }",
        )
        .unwrap();
        let candidate = implementation_to_ami(&contract).unwrap();
        assert_eq!(check_contract(&contract).unwrap(), ContractCheck::Valid);
        assert_eq!(
            verify(&contract, &candidate).unwrap(),
            ProofResult::Verified
        );
        let module = axiom_wasm::emit(&candidate, 4).unwrap();

        for (args, expected) in [([4, 9, 4, 9], 1), ([4, 9, 9, 4], 0)] {
            assert_eq!(candidate.try_eval(&args).unwrap(), expected);
            assert_eq!(reference::eval_ami(&candidate, &args).unwrap(), expected);
            assert_eq!(axiom_wasm::run_with_args(&module, &args).unwrap(), expected);
            assert!(reference::contract_holds(&contract, &candidate, &args).unwrap());
        }
    }

    #[test]
    fn bounded_array_sum_proves_the_loop_and_matches_reference_and_wasm() {
        let function = parse(
            "fn sum(values: [u32; 4]) -> u32 { \
             return for i in 0..4 with acc = 0 invariant acc == acc { acc +% values[i] }; \
             ensures result == values[0] +% values[1] +% values[2] +% values[3]; }",
        )
        .unwrap();
        let candidate = implementation_to_ami(&function).unwrap();
        assert_eq!(check_contract(&function).unwrap(), ContractCheck::Valid);
        assert_eq!(
            verify(&function, &candidate).unwrap(),
            ProofResult::Verified
        );
        let module = axiom_wasm::emit(&candidate, 4).unwrap();

        for (args, expected) in [([u32::MAX, 1, 2, 3], 5), ([3, 4, 5, 6], 18)] {
            assert_eq!(candidate.try_eval(&args).unwrap(), expected);
            assert_eq!(reference::eval_ami(&candidate, &args).unwrap(), expected);
            assert_eq!(
                reference::eval_spec(
                    function.implementation.as_ref().unwrap(),
                    &function,
                    &args,
                    expected
                )
                .unwrap(),
                reference::Value::U32(expected)
            );
            assert_eq!(axiom_wasm::run_with_args(&module, &args).unwrap(), expected);
            assert!(reference::contract_holds(&function, &candidate, &args).unwrap());
        }
    }

    #[test]
    fn bounded_array_loop_search_covers_found_and_not_found() {
        let function = parse(
            "fn contains(values: [u32; 4], needle: u32) -> bool { \
             return for i in 0..4 with acc = false invariant acc == acc { \
               acc || values[i] == needle }; \
             ensures result == (values[0] == needle || values[1] == needle || values[2] == needle || values[3] == needle); }",
        )
        .unwrap();
        let candidate = implementation_to_ami(&function).unwrap();
        assert_eq!(check_contract(&function).unwrap(), ContractCheck::Valid);
        assert_eq!(
            verify(&function, &candidate).unwrap(),
            ProofResult::Verified
        );
        let module = axiom_wasm::emit(&candidate, 5).unwrap();

        for (args, expected) in [([3, 5, 8, 2, 8], 1), ([3, 5, 8, 2, 9], 0)] {
            assert_eq!(candidate.try_eval(&args).unwrap(), expected);
            assert_eq!(reference::eval_ami(&candidate, &args).unwrap(), expected);
            assert_eq!(
                reference::eval_spec(
                    function.implementation.as_ref().unwrap(),
                    &function,
                    &args,
                    expected
                )
                .unwrap(),
                reference::Value::Bool(expected != 0)
            );
            assert_eq!(axiom_wasm::run_with_args(&module, &args).unwrap(), expected);
            assert!(reference::contract_holds(&function, &candidate, &args).unwrap());
        }
    }

    #[test]
    fn bounded_loops_prove_max_min_validation_and_array_updates() {
        let maximum = parse(
            "fn maximum(values: [u32; 4]) -> u32 { \
             return for i in 1..4 with acc = values[0] invariant acc >= values[0] { \
               if acc >= values[i] { acc } else { values[i] } }; \
             ensures result >= values[0]; ensures result >= values[1]; \
             ensures result >= values[2]; ensures result >= values[3]; \
             ensures result == values[0] || result == values[1] || result == values[2] || result == values[3]; }",
        )
        .unwrap();
        let minimum = parse(
            "fn minimum(values: [u32; 4]) -> u32 { \
             return for i in 1..4 with acc = values[0] invariant acc <= values[0] { \
               if acc <= values[i] { acc } else { values[i] } }; \
             ensures result <= values[0]; ensures result <= values[1]; \
             ensures result <= values[2]; ensures result <= values[3]; \
             ensures result == values[0] || result == values[1] || result == values[2] || result == values[3]; }",
        )
        .unwrap();
        for (function, expected) in [(&maximum, 9), (&minimum, 2)] {
            let candidate = implementation_to_ami(function).unwrap();
            assert_eq!(check_contract(function).unwrap(), ContractCheck::Valid);
            assert_eq!(verify(function, &candidate).unwrap(), ProofResult::Verified);
            assert_eq!(candidate.try_eval(&[4, 2, 9, 3]).unwrap(), expected);
            let module = axiom_wasm::emit(&candidate, 4).unwrap();
            assert_eq!(
                axiom_wasm::run_with_args(&module, &[4, 2, 9, 3]).unwrap(),
                expected
            );
        }

        let validation = parse(
            "fn validate(values: [u32; 4]) -> bool { \
             return for i in 0..4 with acc = true invariant acc == true || acc == false { \
               acc && values[i] <= 100 }; \
             ensures result == (values[0] <= 100 && values[1] <= 100 && values[2] <= 100 && values[3] <= 100); }",
        )
        .unwrap();
        let candidate = implementation_to_ami(&validation).unwrap();
        assert_eq!(check_contract(&validation).unwrap(), ContractCheck::Valid);
        assert_eq!(
            verify(&validation, &candidate).unwrap(),
            ProofResult::Verified
        );
        let module = axiom_wasm::emit(&candidate, 4).unwrap();
        assert_eq!(
            axiom_wasm::run_with_args(&module, &[0, 100, 99, 1]).unwrap(),
            1
        );
        assert_eq!(
            axiom_wasm::run_with_args(&module, &[0, 101, 99, 1]).unwrap(),
            0
        );

        let update = parse(
            "fn increment_first(values: [u32; 4]) -> u32 { \
             return (for i in 0..4 with acc = values invariant acc == acc { \
               if i == 0 { array_update(acc, i, acc[i] +% 1) } else { acc } })[0]; \
             ensures result == values[0] +% 1; }",
        )
        .unwrap();
        let candidate = implementation_to_ami(&update).unwrap();
        assert_eq!(check_contract(&update).unwrap(), ContractCheck::Valid);
        assert_eq!(verify(&update, &candidate).unwrap(), ProofResult::Verified);
        assert_eq!(candidate.try_eval(&[7, 8, 9, 10]).unwrap(), 8);
        let module = axiom_wasm::emit(&candidate, 4).unwrap();
        assert_eq!(
            axiom_wasm::run_with_args(&module, &[7, 8, 9, 10]).unwrap(),
            8
        );
    }

    #[test]
    fn bounded_loops_cover_zero_one_and_maximum_iterations() {
        let zero = parse(
            "fn zero(values: [u32; 1]) -> u32 { \
             return for i in 1..1 with acc = 7 invariant acc == 7 { acc +% values[i] }; \
             ensures result == 7; }",
        )
        .unwrap();
        let zero_candidate = implementation_to_ami(&zero).unwrap();
        assert_eq!(check_contract(&zero).unwrap(), ContractCheck::Valid);
        assert_eq!(zero_candidate.try_eval(&[42]).unwrap(), 7);

        let one = parse(
            "fn one(values: [u32; 1]) -> u32 { \
             return for i in 0..1 with acc = 0 invariant acc == acc { acc +% values[i] }; \
             ensures result == values[0]; }",
        )
        .unwrap();
        let one_candidate = implementation_to_ami(&one).unwrap();
        assert_eq!(check_contract(&one).unwrap(), ContractCheck::Valid);
        assert_eq!(one_candidate.try_eval(&[42]).unwrap(), 42);

        let expected = (0..16)
            .map(|index| format!("values[{index}]"))
            .collect::<Vec<_>>()
            .join(" +% ");
        let source = format!(
            "fn sum16(values: [u32; 16]) -> u32 {{ \
             return for i in 0..16 with acc = 0 invariant acc == acc {{ acc +% values[i] }}; \
             ensures result == {expected}; }}"
        );
        let maximum = parse(&source).unwrap();
        let candidate = implementation_to_ami(&maximum).unwrap();
        assert_eq!(check_contract(&maximum).unwrap(), ContractCheck::Valid);
        assert_eq!(verify(&maximum, &candidate).unwrap(), ProofResult::Verified);
        let args: [u32; 16] = std::array::from_fn(|index| index as u32);
        assert_eq!(candidate.try_eval(&args).unwrap(), 120);
    }

    #[test]
    fn bounded_loop_invariant_failure_is_invalid_and_traps_at_runtime() {
        let function = parse(
            "fn broken(values: [u32; 2]) -> u32 { \
             return for i in 0..2 with acc = 0 invariant acc > 0 { acc +% values[i] }; \
             ensures result == values[0] +% values[1]; }",
        )
        .unwrap();
        let candidate = implementation_to_ami(&function).unwrap();
        assert!(matches!(
            check_contract(&function).unwrap(),
            ContractCheck::Invalid(message) if message.contains("loop-invariant")
        ));
        assert!(
            reference::eval_spec(
                function.implementation.as_ref().unwrap(),
                &function,
                &[2, 3],
                0
            )
            .is_err()
        );
        assert!(candidate.try_eval(&[2, 3]).is_err());
        let module = axiom_wasm::emit(&candidate, 2).unwrap();
        assert!(axiom_wasm::run_with_args(&module, &[2, 3]).is_err());
    }
}
