//! Axiom Contract DSL Parser

pub mod ast;
pub mod lexer;
pub mod parser;
pub mod simple;

pub use ast::*;
pub use lexer::Lexer;
pub use parser::Parser;

pub fn parse(input: &str) -> Result<Contract, String> {
    if simple::is_simple(input) {
        let module = parse_module(input)?;
        let [contract] = <[Contract; 1]>::try_from(module.contracts)
            .map_err(|_| "expected exactly one rule in this file".to_string())?;
        validate(&contract)?;
        return Ok(contract);
    }
    let lexer = Lexer::new(input);
    let mut parser = Parser::new(lexer)?;
    let contract = parser.parse_contract()?;
    validate(&contract)?;
    Ok(contract)
}

pub fn parse_module(input: &str) -> Result<Module, String> {
    if simple::is_simple(input) {
        let with_line = |error: String| simple::show_source_line(error, input);
        let translated = simple::translate(input).map_err(with_line)?;
        let mut parser = Parser::new(Lexer::new(&translated)).map_err(with_line)?;
        let module = parser.parse_module().map_err(with_line)?;
        let mut known = Vec::new();
        for contract in &module.contracts {
            let (low, high) = simple::check_ranges(contract, &known)?;
            known.push((contract.name.clone(), low, high));
        }
        return Ok(module);
    }
    let lexer = Lexer::new(input);
    let mut parser = Parser::new(lexer)?;
    parser.parse_module()
}

/// Validates the subset supported by the first end-to-end pipeline.
pub fn validate(contract: &Contract) -> Result<(), String> {
    if !is_scalar(&contract.return_type) {
        return Err("type error: contracts can only return number or bool".to_string());
    }

    match contract.kind {
        DeclarationKind::Extern => {
            if contract.implementation.is_some()
                || !contract.requires.is_empty()
                || !contract.ensures.is_empty()
            {
                return Err("type error: extern declarations cannot contain an implementation or contract clauses".to_string());
            }
            if contract
                .params
                .iter()
                .any(|param| matches!(param.ty, Type::FixedArray(_, _)))
            {
                return Err(
                    "type error: extern declarations cannot accept verified arrays".to_string(),
                );
            }
            return Ok(());
        }
        DeclarationKind::Contract if contract.implementation.is_some() => {
            return Err(
                "type error: contract declarations cannot contain a return expression".to_string(),
            );
        }
        DeclarationKind::Function if contract.implementation.is_none() => {
            return Err("type error: fn declarations require one return expression".to_string());
        }
        DeclarationKind::Contract | DeclarationKind::Function => {}
    }

    if contract.ensures.is_empty() {
        return Err("type error: contract must have at least one ensures clause".to_string());
    }

    if contract
        .requires
        .iter()
        .chain(&contract.ensures)
        .any(contains_for)
    {
        return Err(
            "type error: bounded loops are only allowed in fn implementation expressions"
                .to_string(),
        );
    }

    let mut names = std::collections::HashMap::new();
    for param in &contract.params {
        if !is_scalar(&param.ty) && !is_supported_array(&param.ty) {
            return Err(format!(
                "type error: contract parameter {} must be number, bool, or a supported fixed array",
                param.name
            ));
        }
        if names.insert(param.name.clone(), param.ty.clone()).is_some() {
            return Err(format!("type error: duplicate parameter: {}", param.name));
        }
    }

    if let Some(implementation) = &contract.implementation {
        let has_self_call = count_self_calls(implementation, &contract.name) > 0;
        let typed_implementation = if has_self_call {
            validate_structural_recursion(contract, implementation)?;
            replace_self_calls(implementation, &contract.name)
        } else {
            implementation.clone()
        };
        let implementation_type = type_of_expr(&typed_implementation, &names, None)?;
        if implementation_type != contract.return_type {
            return Err(format!(
                "type error: fn return expression has type {:?}, expected {:?}",
                implementation_type, contract.return_type
            ));
        }
    }

    for (index, expr) in contract.requires.iter().enumerate() {
        if type_of_expr(expr, &names, None)
            .map_err(|error| type_error(contract.requires_spans.get(index).copied(), error))?
            != Type::Bool
        {
            return Err(type_error(
                contract.requires_spans.get(index).copied(),
                "requires clause must be boolean",
            ));
        }
    }
    for (index, expr) in contract.ensures.iter().enumerate() {
        if !mentions_result(expr) {
            return Err(type_error(
                contract.ensures_spans.get(index).copied(),
                "ensures clause must constrain result",
            ));
        }
        if type_of_expr(expr, &names, Some(&contract.return_type))
            .map_err(|error| type_error(contract.ensures_spans.get(index).copied(), error))?
            != Type::Bool
        {
            return Err(type_error(
                contract.ensures_spans.get(index).copied(),
                "ensures clause must be boolean",
            ));
        }
    }
    Ok(())
}

fn is_scalar(ty: &Type) -> bool {
    matches!(ty, Type::U32 | Type::I32 | Type::Bool)
}

fn is_supported_array(ty: &Type) -> bool {
    matches!(
        ty,
        Type::FixedArray(element, length)
            if **element == Type::U32 && (1..=MAX_VERIFIED_ARRAY_LEN).contains(length)
    )
}

fn count_self_calls(expr: &SpecExpr, function: &str) -> usize {
    match expr {
        SpecExpr::Call(name, args) => {
            usize::from(name == function)
                + args
                    .iter()
                    .map(|argument| count_self_calls(argument, function))
                    .sum::<usize>()
        }
        SpecExpr::Binary(_, lhs, rhs) => {
            count_self_calls(lhs, function) + count_self_calls(rhs, function)
        }
        SpecExpr::Let(_, value, body) => {
            count_self_calls(value, function) + count_self_calls(body, function)
        }
        SpecExpr::If(condition, then_branch, else_branch) => {
            count_self_calls(condition, function)
                + count_self_calls(then_branch, function)
                + count_self_calls(else_branch, function)
        }
        SpecExpr::List(values) => values
            .iter()
            .map(|value| count_self_calls(value, function))
            .sum(),
        SpecExpr::Record { fields, .. } => fields
            .iter()
            .map(|(_, value)| count_self_calls(value, function))
            .sum(),
        SpecExpr::Field(value, _) => count_self_calls(value, function),
        SpecExpr::Index(array, index) => {
            count_self_calls(array, function) + count_self_calls(index, function)
        }
        SpecExpr::ArrayUpdate(array, index, value) => {
            count_self_calls(array, function)
                + count_self_calls(index, function)
                + count_self_calls(value, function)
        }
        SpecExpr::For {
            initial,
            invariant,
            body,
            ..
        } => {
            count_self_calls(initial, function)
                + count_self_calls(invariant, function)
                + count_self_calls(body, function)
        }
        SpecExpr::Ident(_) | SpecExpr::Number(_) | SpecExpr::Bool(_) | SpecExpr::Text(_) => 0,
    }
}

fn is_zero_test(expr: &SpecExpr, measure: &str) -> bool {
    let SpecExpr::Binary(BinaryOp::Eq, lhs, rhs) = expr else {
        return false;
    };
    matches!((lhs.as_ref(), rhs.as_ref()),
        (SpecExpr::Ident(name), SpecExpr::Number(0)) if name == measure
    ) || matches!((lhs.as_ref(), rhs.as_ref()),
        (SpecExpr::Number(0), SpecExpr::Ident(name)) if name == measure
    )
}

fn is_decrement(expr: &SpecExpr, measure: &str) -> bool {
    matches!(expr,
        SpecExpr::Binary(BinaryOp::SubWrap, lhs, rhs)
            if matches!(lhs.as_ref(), SpecExpr::Ident(name) if name == measure)
                && matches!(rhs.as_ref(), SpecExpr::Number(1))
    )
}

fn validate_structural_recursion(
    contract: &Contract,
    implementation: &SpecExpr,
) -> Result<(), String> {
    if contract.kind != DeclarationKind::Function
        || contract.return_type != Type::U32
        || contract.params.len() != 1
        || contract.params[0].ty != Type::U32
    {
        return Err(
            "type error: structural recursion currently requires one u32 parameter and a u32 result"
                .to_string(),
        );
    }
    if contains_for(implementation) {
        return Err("type error: recursion cannot be combined with bounded loops".to_string());
    }
    let measure = &contract.params[0].name;
    if contains_measure_shadow(implementation, measure) {
        return Err(format!(
            "type error: recursive measure {measure} cannot be shadowed"
        ));
    }
    if count_self_calls(implementation, &contract.name) != 1 {
        return Err(
            "type error: structural recursion requires exactly one recursive call".to_string(),
        );
    }
    validate_recursive_branch(implementation, &contract.name, measure, false)
}

fn contains_measure_shadow(expr: &SpecExpr, measure: &str) -> bool {
    match expr {
        SpecExpr::Let(name, value, body) => {
            name == measure
                || contains_measure_shadow(value, measure)
                || contains_measure_shadow(body, measure)
        }
        SpecExpr::Binary(_, lhs, rhs) => {
            contains_measure_shadow(lhs, measure) || contains_measure_shadow(rhs, measure)
        }
        SpecExpr::If(condition, then_branch, else_branch) => {
            contains_measure_shadow(condition, measure)
                || contains_measure_shadow(then_branch, measure)
                || contains_measure_shadow(else_branch, measure)
        }
        SpecExpr::Call(_, args) | SpecExpr::List(args) => args
            .iter()
            .any(|value| contains_measure_shadow(value, measure)),
        SpecExpr::Record { fields, .. } => fields
            .iter()
            .any(|(_, value)| contains_measure_shadow(value, measure)),
        SpecExpr::Field(value, _) => contains_measure_shadow(value, measure),
        SpecExpr::Index(array, index) => {
            contains_measure_shadow(array, measure) || contains_measure_shadow(index, measure)
        }
        SpecExpr::ArrayUpdate(array, index, value) => {
            contains_measure_shadow(array, measure)
                || contains_measure_shadow(index, measure)
                || contains_measure_shadow(value, measure)
        }
        SpecExpr::For {
            index,
            accumulator,
            initial,
            invariant,
            body,
            ..
        } => {
            index == measure
                || accumulator == measure
                || contains_measure_shadow(initial, measure)
                || contains_measure_shadow(invariant, measure)
                || contains_measure_shadow(body, measure)
        }
        SpecExpr::Ident(_) | SpecExpr::Number(_) | SpecExpr::Bool(_) | SpecExpr::Text(_) => false,
    }
}

fn validate_recursive_branch(
    expr: &SpecExpr,
    function: &str,
    measure: &str,
    under_decreasing_branch: bool,
) -> Result<(), String> {
    match expr {
        SpecExpr::Call(name, args) if name == function => {
            if !under_decreasing_branch || args.len() != 1 || !is_decrement(&args[0], measure) {
                return Err(format!(
                    "type error: {function} must recurse once as {measure} -% 1 only in the nonzero branch of {measure} == 0"
                ));
            }
            Ok(())
        }
        SpecExpr::Call(_, args) | SpecExpr::List(args) => {
            for value in args {
                validate_recursive_branch(value, function, measure, under_decreasing_branch)?;
            }
            Ok(())
        }
        SpecExpr::Binary(_, lhs, rhs) => {
            validate_recursive_branch(lhs, function, measure, under_decreasing_branch)?;
            validate_recursive_branch(rhs, function, measure, under_decreasing_branch)
        }
        SpecExpr::Let(_, value, body) => {
            validate_recursive_branch(value, function, measure, under_decreasing_branch)?;
            validate_recursive_branch(body, function, measure, under_decreasing_branch)
        }
        SpecExpr::If(condition, then_branch, else_branch) if is_zero_test(condition, measure) => {
            validate_recursive_branch(condition, function, measure, false)?;
            validate_recursive_branch(then_branch, function, measure, false)?;
            validate_recursive_branch(else_branch, function, measure, true)
        }
        SpecExpr::If(condition, then_branch, else_branch) => {
            validate_recursive_branch(condition, function, measure, under_decreasing_branch)?;
            validate_recursive_branch(then_branch, function, measure, under_decreasing_branch)?;
            validate_recursive_branch(else_branch, function, measure, under_decreasing_branch)
        }
        SpecExpr::Record { fields, .. } => {
            for (_, value) in fields {
                validate_recursive_branch(value, function, measure, under_decreasing_branch)?;
            }
            Ok(())
        }
        SpecExpr::Field(value, _) => {
            validate_recursive_branch(value, function, measure, under_decreasing_branch)
        }
        SpecExpr::Index(array, index) => {
            validate_recursive_branch(array, function, measure, under_decreasing_branch)?;
            validate_recursive_branch(index, function, measure, under_decreasing_branch)
        }
        SpecExpr::ArrayUpdate(array, index, value) => {
            validate_recursive_branch(array, function, measure, under_decreasing_branch)?;
            validate_recursive_branch(index, function, measure, under_decreasing_branch)?;
            validate_recursive_branch(value, function, measure, under_decreasing_branch)
        }
        SpecExpr::For { .. } => {
            Err("type error: recursion cannot be combined with bounded loops".to_string())
        }
        SpecExpr::Ident(_) | SpecExpr::Number(_) | SpecExpr::Bool(_) | SpecExpr::Text(_) => Ok(()),
    }
}

fn replace_self_calls(expr: &SpecExpr, function: &str) -> SpecExpr {
    match expr {
        SpecExpr::Call(name, _) if name == function => SpecExpr::Number(0),
        SpecExpr::Call(name, args) => SpecExpr::Call(
            name.clone(),
            args.iter()
                .map(|argument| replace_self_calls(argument, function))
                .collect(),
        ),
        SpecExpr::Binary(op, lhs, rhs) => SpecExpr::Binary(
            op.clone(),
            Box::new(replace_self_calls(lhs, function)),
            Box::new(replace_self_calls(rhs, function)),
        ),
        SpecExpr::Let(name, value, body) => SpecExpr::Let(
            name.clone(),
            Box::new(replace_self_calls(value, function)),
            Box::new(replace_self_calls(body, function)),
        ),
        SpecExpr::If(condition, then_branch, else_branch) => SpecExpr::If(
            Box::new(replace_self_calls(condition, function)),
            Box::new(replace_self_calls(then_branch, function)),
            Box::new(replace_self_calls(else_branch, function)),
        ),
        SpecExpr::List(values) => SpecExpr::List(
            values
                .iter()
                .map(|value| replace_self_calls(value, function))
                .collect(),
        ),
        SpecExpr::Record { name, fields } => SpecExpr::Record {
            name: name.clone(),
            fields: fields
                .iter()
                .map(|(field, value)| (field.clone(), replace_self_calls(value, function)))
                .collect(),
        },
        SpecExpr::Field(value, field) => {
            SpecExpr::Field(Box::new(replace_self_calls(value, function)), field.clone())
        }
        SpecExpr::Index(array, index) => SpecExpr::Index(
            Box::new(replace_self_calls(array, function)),
            Box::new(replace_self_calls(index, function)),
        ),
        SpecExpr::ArrayUpdate(array, index, value) => SpecExpr::ArrayUpdate(
            Box::new(replace_self_calls(array, function)),
            Box::new(replace_self_calls(index, function)),
            Box::new(replace_self_calls(value, function)),
        ),
        SpecExpr::For {
            index,
            start,
            end,
            accumulator,
            initial,
            invariant,
            body,
        } => SpecExpr::For {
            index: index.clone(),
            start: *start,
            end: *end,
            accumulator: accumulator.clone(),
            initial: Box::new(replace_self_calls(initial, function)),
            invariant: Box::new(replace_self_calls(invariant, function)),
            body: Box::new(replace_self_calls(body, function)),
        },
        SpecExpr::Ident(_) | SpecExpr::Number(_) | SpecExpr::Bool(_) | SpecExpr::Text(_) => {
            expr.clone()
        }
    }
}

/// Returns the flattened core/WASM argument offsets for a scalar or fixed-array signature.
pub fn verified_abi_layout(params: &[Param]) -> Result<(Vec<usize>, usize), String> {
    let mut offsets = Vec::with_capacity(params.len());
    let mut arity = 0usize;
    for param in params {
        offsets.push(arity);
        let width = match &param.ty {
            Type::U32 | Type::I32 | Type::Bool => 1,
            Type::FixedArray(element, length)
                if **element == Type::U32 && (1..=MAX_VERIFIED_ARRAY_LEN).contains(length) =>
            {
                *length
            }
            _ => return Err(format!("unsupported verified input type: {}", param.name)),
        };
        arity = arity
            .checked_add(width)
            .ok_or_else(|| "verified ABI argument count overflow".to_string())?;
    }
    Ok((offsets, arity))
}

fn numeric_type(lhs: &SpecExpr, lhs_type: &Type, rhs: &SpecExpr, rhs_type: &Type) -> Option<Type> {
    match (lhs_type, rhs_type) {
        (Type::U32, Type::U32) => Some(Type::U32),
        (Type::I32, Type::I32) => Some(Type::I32),
        (Type::I32, Type::U32) if matches!(rhs, SpecExpr::Number(_)) => Some(Type::I32),
        (Type::U32, Type::I32) if matches!(lhs, SpecExpr::Number(_)) => Some(Type::I32),
        _ => None,
    }
}

fn type_error(span: Option<Span>, message: impl std::fmt::Display) -> String {
    match span {
        Some(span) => format!("{}:{}: type error: {message}", span.line, span.column),
        None => format!("type error: {message}"),
    }
}

fn mentions_result(expr: &SpecExpr) -> bool {
    match expr {
        SpecExpr::Ident(name) => name == "result",
        SpecExpr::Binary(_, lhs, rhs) => mentions_result(lhs) || mentions_result(rhs),
        SpecExpr::Let(_, value, body) => mentions_result(value) || mentions_result(body),
        SpecExpr::If(condition, then_branch, else_branch) => {
            mentions_result(condition)
                || mentions_result(then_branch)
                || mentions_result(else_branch)
        }
        SpecExpr::Call(_, args) | SpecExpr::List(args) => args.iter().any(mentions_result),
        SpecExpr::Index(array, index) => mentions_result(array) || mentions_result(index),
        SpecExpr::ArrayUpdate(array, index, value) => {
            mentions_result(array) || mentions_result(index) || mentions_result(value)
        }
        SpecExpr::For {
            initial,
            invariant,
            body,
            ..
        } => mentions_result(initial) || mentions_result(invariant) || mentions_result(body),
        SpecExpr::Record { fields, .. } => fields.iter().any(|(_, value)| mentions_result(value)),
        SpecExpr::Field(value, _) => mentions_result(value),
        SpecExpr::Number(_) | SpecExpr::Bool(_) | SpecExpr::Text(_) => false,
    }
}

fn contains_for(expr: &SpecExpr) -> bool {
    match expr {
        SpecExpr::For { .. } => true,
        SpecExpr::Binary(_, lhs, rhs) => contains_for(lhs) || contains_for(rhs),
        SpecExpr::Let(_, value, body) => contains_for(value) || contains_for(body),
        SpecExpr::If(condition, then_branch, else_branch) => {
            contains_for(condition) || contains_for(then_branch) || contains_for(else_branch)
        }
        SpecExpr::Call(_, args) | SpecExpr::List(args) => args.iter().any(contains_for),
        SpecExpr::Index(array, index) => contains_for(array) || contains_for(index),
        SpecExpr::ArrayUpdate(array, index, value) => {
            contains_for(array) || contains_for(index) || contains_for(value)
        }
        SpecExpr::Record { fields, .. } => fields.iter().any(|(_, value)| contains_for(value)),
        SpecExpr::Field(value, _) => contains_for(value),
        SpecExpr::Ident(_) | SpecExpr::Number(_) | SpecExpr::Bool(_) | SpecExpr::Text(_) => false,
    }
}

pub fn type_of_expr(
    expr: &SpecExpr,
    names: &std::collections::HashMap<String, Type>,
    result_type: Option<&Type>,
) -> Result<Type, String> {
    match expr {
        SpecExpr::Ident(name) if name == "result" => result_type
            .cloned()
            .ok_or_else(|| "result is only available in ensures clauses".to_string()),
        SpecExpr::Ident(name) if names.contains_key(name) => Ok(names[name].clone()),
        SpecExpr::Ident(name) => Err(format!("unknown identifier: {name}")),
        SpecExpr::Number(_) => Ok(Type::U32),
        SpecExpr::Bool(_) => Ok(Type::Bool),
        SpecExpr::Text(_) => Ok(Type::Text),
        SpecExpr::List(values) => {
            let Some(first) = values.first() else {
                return Ok(Type::List(Box::new(Type::Unknown)));
            };
            let item_type = type_of_expr(first, names, result_type)?;
            for value in &values[1..] {
                if type_of_expr(value, names, result_type)? != item_type {
                    return Err("list values must have the same type".to_string());
                }
            }
            Ok(Type::List(Box::new(item_type)))
        }
        SpecExpr::Record { name, .. } => Ok(Type::Record(name.clone())),
        SpecExpr::Field(value, field) => match type_of_expr(value, names, result_type)? {
            Type::FixedArray(_, _) if field == "length" => Ok(Type::U32),
            _ => Err("field access requires a page record context".to_string()),
        },
        SpecExpr::Index(array, index) => {
            let array_type = type_of_expr(array, names, result_type)?;
            let index_type = type_of_expr(index, names, result_type)?;
            match array_type {
                Type::FixedArray(element, _) if matches!(index_type, Type::U32 | Type::I32) => {
                    Ok(*element)
                }
                Type::FixedArray(_, _) => Err("array index must be u32 or i32".to_string()),
                _ => Err("indexed reads require a fixed verified array".to_string()),
            }
        }
        SpecExpr::ArrayUpdate(array, index, value) => {
            let array_type = type_of_expr(array, names, result_type)?;
            let index_type = type_of_expr(index, names, result_type)?;
            let value_type = type_of_expr(value, names, result_type)?;
            match &array_type {
                Type::FixedArray(element, _)
                    if matches!(index_type, Type::U32 | Type::I32) && value_type == **element =>
                {
                    Ok(array_type)
                }
                Type::FixedArray(_, _) if !matches!(index_type, Type::U32 | Type::I32) => {
                    Err("array index must be u32 or i32".to_string())
                }
                Type::FixedArray(_, _) => Err("array update value has the wrong type".to_string()),
                _ => Err("array_update requires a fixed verified array".to_string()),
            }
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
            if end < start || end - start > MAX_VERIFIED_LOOP_ITERATIONS {
                return Err("bounded loop exceeds the verified iteration limit".to_string());
            }
            if index == accumulator {
                return Err("loop index and accumulator must have different names".to_string());
            }
            if [initial.as_ref(), invariant.as_ref(), body.as_ref()]
                .iter()
                .any(|expr| contains_for(expr))
            {
                return Err("nested bounded loops are not supported".to_string());
            }
            let initial_type = type_of_expr(initial, names, result_type)?;
            let mut nested = names.clone();
            nested.insert(index.clone(), Type::U32);
            nested.insert(accumulator.clone(), initial_type.clone());
            if type_of_expr(invariant, &nested, result_type)? != Type::Bool {
                return Err("loop invariant must be boolean".to_string());
            }
            let body_type = type_of_expr(body, &nested, result_type)?;
            if body_type != initial_type {
                return Err("loop body must return the accumulator type".to_string());
            }
            Ok(initial_type)
        }
        SpecExpr::Let(name, value, body) => {
            let value_type = type_of_expr(value, names, result_type)?;
            let mut nested = names.clone();
            nested.insert(name.clone(), value_type);
            type_of_expr(body, &nested, result_type)
        }
        SpecExpr::If(condition, then_branch, else_branch) => {
            if type_of_expr(condition, names, result_type)? != Type::Bool {
                return Err("if condition must be boolean".to_string());
            }
            let then_type = type_of_expr(then_branch, names, result_type)?;
            let else_type = type_of_expr(else_branch, names, result_type)?;
            if then_type != else_type {
                return Err("if branches must have the same type".to_string());
            }
            Ok(then_type)
        }
        SpecExpr::Call(name, _) => Err(format!(
            "unresolved contract call: {name}; use project mode"
        )),
        SpecExpr::Binary(op, lhs, rhs) => {
            let lhs_type = type_of_expr(lhs, names, result_type)?;
            let rhs_type = type_of_expr(rhs, names, result_type)?;
            match op {
                BinaryOp::AddWrap
                | BinaryOp::SubWrap
                | BinaryOp::MulWrap
                | BinaryOp::Shl
                | BinaryOp::BitAnd
                | BinaryOp::BitOr
                | BinaryOp::BitXor
                    if numeric_type(lhs, &lhs_type, rhs, &rhs_type).is_some() =>
                {
                    Ok(numeric_type(lhs, &lhs_type, rhs, &rhs_type).unwrap())
                }
                BinaryOp::Eq | BinaryOp::Ne
                    if lhs_type == rhs_type
                        || numeric_type(lhs, &lhs_type, rhs, &rhs_type).is_some() =>
                {
                    Ok(Type::Bool)
                }
                BinaryOp::Lt | BinaryOp::Le | BinaryOp::Gt | BinaryOp::Ge
                    if numeric_type(lhs, &lhs_type, rhs, &rhs_type).is_some() =>
                {
                    Ok(Type::Bool)
                }
                BinaryOp::And | BinaryOp::Or
                    if lhs_type == Type::Bool && rhs_type == Type::Bool =>
                {
                    Ok(Type::Bool)
                }
                _ => Err(format!("operator {:?} has incompatible operands", op)),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_mul8_contract() {
        let src = r#"
        // Axiom specification: Multiply by 8 with modulo 2^32 wrap-around
        contract mul8(x: u32) -> u32 {
            ensures result == x *% 8;
        }
        "#;

        let contract = parse(src).expect("Failed to parse mul8 contract");
        assert_eq!(contract.name, "mul8");
        assert_eq!(contract.params.len(), 1);
        assert_eq!(contract.params[0].name, "x");
        assert_eq!(contract.params[0].ty, Type::U32);
        assert_eq!(contract.return_type, Type::U32);
        assert_eq!(contract.ensures.len(), 1);

        // Check ensures: result == (x *% 8)
        match &contract.ensures[0] {
            SpecExpr::Binary(BinaryOp::Eq, lhs, rhs) => {
                assert_eq!(**lhs, SpecExpr::Ident("result".to_string()));
                match &**rhs {
                    SpecExpr::Binary(BinaryOp::MulWrap, x_id, eight) => {
                        assert_eq!(**x_id, SpecExpr::Ident("x".to_string()));
                        assert_eq!(**eight, SpecExpr::Number(8));
                    }
                    other => panic!("Expected x *% 8, found {:?}", other),
                }
            }
            other => panic!("Expected result == ..., found {:?}", other),
        }
    }

    #[test]
    fn parses_function_and_extern_declarations() {
        let function = parse(
            "fn increment(value: u32) -> u32 { \
             return value +% 1; \
             ensures result == value +% 1; }",
        )
        .unwrap();
        assert_eq!(function.kind, DeclarationKind::Function);
        assert!(function.implementation.is_some());

        let external = parse("extern fn read_count(key: u32) -> u32;").unwrap();
        assert_eq!(external.kind, DeclarationKind::Extern);
        assert!(external.implementation.is_none());
        assert!(external.ensures.is_empty());
    }

    #[test]
    fn rejects_function_without_a_return_expression() {
        let error = parse("fn broken(value: u32) -> u32 { ensures result == value; }").unwrap_err();
        assert!(error.contains("fn declarations require one return expression"));
    }

    #[test]
    fn rejects_tokens_after_contract() {
        let error = parse("contract f() -> u32 { ensures result == 0; } extra").unwrap_err();
        assert!(error.contains("Unexpected token after contract"));
    }

    #[test]
    fn rejects_unknown_identifiers() {
        let error = parse("contract f(x: u32) -> u32 { ensures result == y; }").unwrap_err();
        assert!(error.contains("unknown identifier: y"));
    }

    #[test]
    fn rejects_ensures_that_do_not_constrain_result() {
        let error = parse("contract f(x: u32) -> u32 { ensures x == x; }").unwrap_err();
        assert!(error.contains("ensures clause must constrain result"));
    }

    #[test]
    fn parses_multiple_parameters_with_commas() {
        let contract =
            parse("contract add(x: u32, y: u32) -> u32 { ensures result == x +% y; }").unwrap();

        assert_eq!(contract.params.len(), 2);
        assert_eq!(contract.params[1].name, "y");
    }

    #[test]
    fn parses_compound_boolean_requires() {
        let contract = parse(
            "contract guarded(x: u32) -> u32 { requires x != 0 && x != 1; ensures result == x; }",
        )
        .unwrap();

        assert_eq!(contract.requires.len(), 1);
    }

    #[test]
    fn reports_parser_line_and_column() {
        let error = parse("contract bad(x: u32) -> u32 {\n    ensures result == ;\n}").unwrap_err();

        assert!(error.starts_with("2:"), "unexpected error: {error}");
    }

    #[test]
    fn reports_type_errors_at_clause_positions() {
        let error =
            parse("contract bad(x: u32) -> u32 {\n    ensures result +% x;\n}").unwrap_err();

        assert!(error.starts_with("2:5: type error:"));
    }

    #[test]
    fn parses_the_easy_word_aliases() {
        let contract = parse(
            "program mul8(x: number) -> number { when x is_not 0; gives answer equals x times 8; }",
        )
        .unwrap();

        assert_eq!(contract.name, "mul8");
        assert_eq!(contract.params[0].ty, Type::U32);
        assert_eq!(
            contract.requires,
            vec![SpecExpr::Binary(
                BinaryOp::Ne,
                Box::new(SpecExpr::Ident("x".to_string())),
                Box::new(SpecExpr::Number(0)),
            )]
        );
        assert_eq!(
            contract.ensures,
            vec![SpecExpr::Binary(
                BinaryOp::Eq,
                Box::new(SpecExpr::Ident("result".to_string())),
                Box::new(SpecExpr::Binary(
                    BinaryOp::MulWrap,
                    Box::new(SpecExpr::Ident("x".to_string())),
                    Box::new(SpecExpr::Number(8)),
                )),
            )]
        );
    }

    #[test]
    fn parses_comparisons_and_conditional_values() {
        let contract = parse(
            "program minimum(a: number, b: number) -> number { \
             gives answer equals if a < b { a } else { b }; }",
        )
        .unwrap();

        assert!(matches!(
            &contract.ensures[0],
            SpecExpr::Binary(BinaryOp::Eq, _, rhs)
                if matches!(rhs.as_ref(), SpecExpr::If(_, _, _))
        ));
    }

    #[test]
    fn parses_and_types_let_values() {
        let contract = parse(
            "program double(x: number) -> number { \
             gives answer equals let value = x plus x in value; }",
        )
        .unwrap();

        assert!(matches!(
            &contract.ensures[0],
            SpecExpr::Binary(BinaryOp::Eq, _, rhs)
                if matches!(rhs.as_ref(), SpecExpr::Let(_, _, _))
        ));
    }

    #[test]
    fn parses_modules_and_pages() {
        let module = parse_module(
            r#"
            import { increment } from "./logic.ax";
            page Counter {
                state count: number = 0;
                show count;
                button "더하기" updates count with increment(count);
            }
            "#,
        )
        .unwrap();

        assert_eq!(module.imports[0].names, vec!["increment"]);
        assert_eq!(module.imports[0].path, "./logic.ax");
        assert_eq!(module.pages[0].name, "Counter");
        assert_eq!(module.pages[0].states[0].name, "count");
        assert!(matches!(
            module.pages[0].elements[1],
            PageElement::Button { .. }
        ));
    }

    #[test]
    fn parses_bool_contracts_and_conditional_page_text() {
        let contract =
            parse("program is_ready(value: number) -> bool { gives answer equals value > 0; }")
                .unwrap();

        assert_eq!(contract.return_type, Type::Bool);
        let module = parse_module(
            r#"
            page Status {
                state ready: bool = false;
                when ready show "준비됨";
            }
            "#,
        )
        .unwrap();

        assert!(matches!(
            module.pages[0].elements[0],
            PageElement::WhenShow { .. }
        ));
    }

    #[test]
    fn parses_v31_records_lists_inputs_reducers_and_components() {
        let module = parse_module(
            r#"
            record Todo { title: text; done: bool; }
            component Help { show "도움말"; }
            page Todo {
                state todos: list<Todo> = [];
                state draft: text = "";
                persist todos;
                input draft placeholder "할 일";
                use Help;
                button "추가" adds todos with Todo { title: draft, done: false };
                list todos as todo {
                    show todo.title;
                    when todo.done show "완료";
                }
                reducer refresh { updates todos with toggle(todos); }
                button "갱신" runs refresh;
            }
            "#,
        )
        .unwrap();

        assert_eq!(module.records[0].name, "Todo");
        assert_eq!(module.components[0].name, "Help");
        assert_eq!(module.pages[0].persisted, vec!["todos"]);
        assert!(
            module.pages[0]
                .elements
                .iter()
                .any(|element| matches!(element, PageElement::List { .. }))
        );
        assert_eq!(module.pages[0].reducers[0].name, "refresh");
    }

    #[test]
    fn parses_signed_i32_contracts() {
        let contract = parse(
            "contract minimum(first: i32, second: i32) -> i32 { ensures result == if first < second { first } else { second }; }",
        )
        .unwrap();

        assert_eq!(contract.params[0].ty, Type::I32);
        assert_eq!(contract.return_type, Type::I32);
    }

    #[test]
    fn parses_scalar_bitwise_and_subtraction_operators() {
        let contract = parse(
            "contract mask(value: u32, bits: u32) -> u32 { ensures result == (value & bits) ^ (value -% 1); }",
        )
        .unwrap();

        assert!(matches!(
            &contract.ensures[0],
            SpecExpr::Binary(BinaryOp::Eq, _, rhs)
                if matches!(rhs.as_ref(), SpecExpr::Binary(BinaryOp::BitXor, _, _))
        ));
    }

    #[test]
    fn parses_bounded_verified_arrays_and_flattened_abi() {
        let contract = parse(
            "fn read(values: [u32; 3], index: u32) -> u32 { \
             requires index < values.length; \
             return array_update(values, index, 9)[index]; \
             ensures result == values[index]; }",
        )
        .unwrap();

        assert_eq!(
            contract.params[0].ty,
            Type::FixedArray(Box::new(Type::U32), 3)
        );
        assert_ne!(contract.params[0].ty, Type::List(Box::new(Type::U32)));
        assert_eq!(
            verified_abi_layout(&contract.params).unwrap(),
            (vec![0, 3], 4)
        );
        assert!(matches!(
            contract.implementation,
            Some(SpecExpr::Index(_, _))
        ));
    }

    #[test]
    fn rejects_unbounded_or_unsupported_verified_array_types() {
        for source in [
            "contract f(values: [u32; 0]) -> u32 { ensures result == 0; }",
            "contract f(values: [u32; 17]) -> u32 { ensures result == 0; }",
            "contract f(values: [i32; 2]) -> u32 { ensures result == 0; }",
        ] {
            assert!(
                parse(source).is_err(),
                "accepted unsupported array: {source}"
            );
        }
        assert!(
            parse("extern fn read(values: [u32; 2]) -> u32;").is_err(),
            "extern ABI must remain outside the verified array boundary"
        );
    }

    #[test]
    fn parses_statically_bounded_for_accumulators() {
        let function = parse(
            "fn sum(values: [u32; 4]) -> u32 { \
             return for i in 0..4 with acc = 0 invariant acc == acc { acc +% values[i] }; \
             ensures result == values[0] +% values[1] +% values[2] +% values[3]; }",
        )
        .unwrap();

        let Some(SpecExpr::For {
            index,
            start,
            end,
            accumulator,
            ..
        }) = function.implementation.as_ref()
        else {
            panic!("expected a bounded loop implementation");
        };
        assert_eq!(index, "i");
        assert_eq!((*start, *end), (0, 4));
        assert_eq!(accumulator, "acc");
    }

    #[test]
    fn rejects_unbounded_nested_or_ill_typed_loops() {
        for source in [
            "fn f(values: [u32; 2]) -> u32 { return for i in 0..values.length with acc = 0 invariant true { acc }; ensures result == 0; }",
            "fn f(values: [u32; 2]) -> u32 { return for i in 0..17 with acc = 0 invariant true { acc }; ensures result == 0; }",
            "fn f(values: [u32; 2]) -> u32 { return for i in 3..2 with acc = 0 invariant true { acc }; ensures result == 0; }",
            "fn f(values: [u32; 2]) -> u32 { return for i in 0..1 with acc = 0 invariant 0 { acc }; ensures result == 0; }",
            "fn f(values: [u32; 2]) -> u32 { return for i in 0..1 with acc = 0 invariant true { true }; ensures result == 0; }",
            "fn f(values: [u32; 2]) -> u32 { return for i in 0..1 with acc = 0 invariant true { for j in 0..1 with inner = 0 invariant true { inner } }; ensures result == 0; }",
            "contract f(values: [u32; 2]) -> u32 { requires for i in 0..1 with acc = 0 invariant true { acc == 0 }; ensures result == 0; }",
        ] {
            assert!(
                parse(source).is_err(),
                "accepted unsupported loop: {source}"
            );
        }
    }

    #[test]
    fn accepts_single_measure_decreasing_structural_recursion() {
        let function = parse(
            "fn sum_to(n: u32) -> u32 { \
             requires n <= 16; \
             return if n == 0 { 0 } else { 1 +% sum_to(n -% 1) }; \
             ensures result == n; }",
        )
        .unwrap();
        assert_eq!(function.name, "sum_to");
    }

    #[test]
    fn rejects_non_decreasing_unbounded_or_unsupported_recursion() {
        for source in [
            "fn f(n: u32) -> u32 { requires n <= 16; return if n == 0 { 0 } else { f(n) }; ensures result == n; }",
            "fn f(n: u32) -> u32 { requires n <= 16; return f(n -% 1); ensures result == n; }",
            "fn f(n: u32) -> u32 { requires n <= 16; return if n > 0 { f(n -% 1) } else { 0 }; ensures result == n; }",
            "fn f(n: u32) -> u32 { requires n <= 16; return if n == 0 { 0 } else { f(n -% 1) +% f(n -% 1) }; ensures result == n; }",
            "fn f(n: u32) -> u32 { requires n <= 16; return let n = 0 in if n == 0 { 0 } else { f(n -% 1) }; ensures result == 0; }",
            "fn f(n: i32) -> u32 { requires n <= 16; return if n == 0 { 0 } else { f(n -% 1) }; ensures result == 0; }",
            "fn even(n: u32) -> u32 { return odd(n); ensures result == 0; }",
            "contract f(n: u32) -> u32 { ensures result == f(n -% 1); }",
        ] {
            assert!(
                parse(source).is_err(),
                "accepted unsupported recursion: {source}"
            );
        }
    }
}
