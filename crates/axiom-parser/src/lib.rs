//! Axiom Contract DSL Parser

pub mod ast;
pub mod lexer;
pub mod parser;

pub use ast::*;
pub use lexer::Lexer;
pub use parser::Parser;

pub fn parse(input: &str) -> Result<Contract, String> {
    let lexer = Lexer::new(input);
    let mut parser = Parser::new(lexer)?;
    let contract = parser.parse_contract()?;
    validate(&contract)?;
    Ok(contract)
}

pub fn parse_module(input: &str) -> Result<Module, String> {
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

    let mut names = std::collections::HashMap::new();
    for param in &contract.params {
        if !is_scalar(&param.ty) {
            return Err(format!(
                "type error: contract parameter {} must be number or bool",
                param.name
            ));
        }
        if names.insert(param.name.clone(), param.ty.clone()).is_some() {
            return Err(format!("type error: duplicate parameter: {}", param.name));
        }
    }

    if let Some(implementation) = &contract.implementation {
        let implementation_type = type_of_expr(implementation, &names, None)?;
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
        SpecExpr::Record { fields, .. } => fields.iter().any(|(_, value)| mentions_result(value)),
        SpecExpr::Field(value, _) => mentions_result(value),
        SpecExpr::Number(_) | SpecExpr::Bool(_) | SpecExpr::Text(_) => false,
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
        SpecExpr::Field(_, _) => Err("field access requires a page record context".to_string()),
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
}
