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
    if contract.return_type != Type::U32 {
        return Err("type error: the initial pipeline supports only u32 return values".to_string());
    }
    if contract.ensures.is_empty() {
        return Err("type error: contract must have at least one ensures clause".to_string());
    }

    let mut names = std::collections::HashMap::new();
    for param in &contract.params {
        if param.ty != Type::U32 {
            return Err(format!("type error: parameter {} must be u32", param.name));
        }
        if names.insert(param.name.clone(), param.ty.clone()).is_some() {
            return Err(format!("type error: duplicate parameter: {}", param.name));
        }
    }

    for (index, expr) in contract.requires.iter().enumerate() {
        if spec_type(expr, &names, false)
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
        if spec_type(expr, &names, true)
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

fn type_error(span: Option<Span>, message: impl std::fmt::Display) -> String {
    match span {
        Some(span) => format!("{}:{}: type error: {message}", span.line, span.column),
        None => format!("type error: {message}"),
    }
}

fn spec_type(
    expr: &SpecExpr,
    names: &std::collections::HashMap<String, Type>,
    allow_result: bool,
) -> Result<Type, String> {
    match expr {
        SpecExpr::Ident(name) if name == "result" && allow_result => Ok(Type::U32),
        SpecExpr::Ident(name) if name == "result" => {
            Err("result is only available in ensures clauses".to_string())
        }
        SpecExpr::Ident(name) if names.contains_key(name) => Ok(names[name].clone()),
        SpecExpr::Ident(name) => Err(format!("unknown identifier: {name}")),
        SpecExpr::Number(_) => Ok(Type::U32),
        SpecExpr::Bool(_) => Ok(Type::Bool),
        SpecExpr::Let(name, value, body) => {
            let value_type = spec_type(value, names, allow_result)?;
            let mut nested = names.clone();
            nested.insert(name.clone(), value_type);
            spec_type(body, &nested, allow_result)
        }
        SpecExpr::If(condition, then_branch, else_branch) => {
            if spec_type(condition, names, allow_result)? != Type::Bool {
                return Err("if condition must be boolean".to_string());
            }
            let then_type = spec_type(then_branch, names, allow_result)?;
            let else_type = spec_type(else_branch, names, allow_result)?;
            if then_type != else_type {
                return Err("if branches must have the same type".to_string());
            }
            Ok(then_type)
        }
        SpecExpr::Call(name, _) => Err(format!(
            "unresolved contract call: {name}; use project mode"
        )),
        SpecExpr::Binary(op, lhs, rhs) => {
            let lhs_type = spec_type(lhs, names, allow_result)?;
            let rhs_type = spec_type(rhs, names, allow_result)?;
            match op {
                BinaryOp::AddWrap | BinaryOp::MulWrap | BinaryOp::Shl
                    if lhs_type == Type::U32 && rhs_type == Type::U32 =>
                {
                    Ok(Type::U32)
                }
                BinaryOp::Eq | BinaryOp::Ne if lhs_type == rhs_type => Ok(Type::Bool),
                BinaryOp::Lt | BinaryOp::Le | BinaryOp::Gt | BinaryOp::Ge
                    if lhs_type == Type::U32 && rhs_type == Type::U32 =>
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
}
