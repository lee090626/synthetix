//! Rust source emitter for verified AMI candidates.
//!
//! The proof covers the AMI expression; this module prints it with the same
//! wrapping semantics as `Expr::try_eval_bits` (u32, or u64 for `wide` rules). Equivalence is checked
//! by `tests/rust_emit.rs`, not proven.

use axiom_ami::Expr;
use axiom_parser::{Contract, Type};

use crate::js::{list_parts, param_groups, settle_sizes};

/// Word types: `(unsigned, signed, shift mask)` for 32- or 64-bit contracts.
fn words(wide: bool) -> (&'static str, &'static str, u32) {
    if wide {
        ("u64", "i64", 63)
    } else {
        ("u32", "i32", 31)
    }
}

fn bin(method: &str, lhs: &Expr, rhs: &Expr, wide: bool) -> String {
    format!("({}).{method}({})", expr(lhs, wide), expr(rhs, wide))
}

fn cmp(op: &str, lhs: &Expr, rhs: &Expr, signed: bool, wide: bool) -> String {
    let (l, r) = (expr(lhs, wide), expr(rhs, wide));
    let (u, i, _) = words(wide);
    if signed {
        format!("{u}::from((({l}) as {i}) {op} (({r}) as {i}))")
    } else {
        format!("{u}::from(({l}) {op} ({r}))")
    }
}

fn checked(method: &str, lhs: &Expr, rhs: &Expr, signed: bool, wide: bool) -> String {
    let (l, r) = (expr(lhs, wide), expr(rhs, wide));
    let (u, i, _) = words(wide);
    if signed {
        format!(
            "((({l}) as {i}).{method}(({r}) as {i}).ok_or(AxiomError(\"division by zero or overflow\"))? as {u})"
        )
    } else {
        format!("({l}).{method}({r}).ok_or(AxiomError(\"division by zero\"))?")
    }
}

/// Prints one AMI expression; every sub-expression is a `u32` (`u64` when wide).
fn expr(e: &Expr, wide: bool) -> String {
    let (u, _, shift) = words(wide);
    let x = |e: &Expr| expr(e, wide);
    match e {
        Expr::Arg(i) => format!("a{i}"),
        Expr::Const(v) => format!("{v}{u}"),
        Expr::Add(l, r) => bin("wrapping_add", l, r, wide),
        Expr::Sub(l, r) => bin("wrapping_sub", l, r, wide),
        Expr::Mul(l, r) => bin("wrapping_mul", l, r, wide),
        Expr::Div(l, r) => checked("checked_div", l, r, false, wide),
        Expr::Rem(l, r) => checked("checked_rem", l, r, false, wide),
        Expr::SDiv(l, r) => checked("checked_div", l, r, true, wide),
        Expr::SRem(l, r) => checked("checked_rem", l, r, true, wide),
        Expr::Shl(l, r) => format!("({}).wrapping_shl((({}) & {shift}) as u32)", x(l), x(r)),
        Expr::BitAnd(l, r) => format!("(({}) & ({}))", x(l), x(r)),
        Expr::BitOr(l, r) => format!("(({}) | ({}))", x(l), x(r)),
        Expr::BitXor(l, r) => format!("(({}) ^ ({}))", x(l), x(r)),
        Expr::Eq(l, r) => cmp("==", l, r, false, wide),
        Expr::Ne(l, r) => cmp("!=", l, r, false, wide),
        Expr::Lt(l, r) => cmp("<", l, r, false, wide),
        Expr::Le(l, r) => cmp("<=", l, r, false, wide),
        Expr::Gt(l, r) => cmp(">", l, r, false, wide),
        Expr::Ge(l, r) => cmp(">=", l, r, false, wide),
        Expr::SLt(l, r) => cmp("<", l, r, true, wide),
        Expr::SLe(l, r) => cmp("<=", l, r, true, wide),
        Expr::SGt(l, r) => cmp(">", l, r, true, wide),
        Expr::SGe(l, r) => cmp(">=", l, r, true, wide),
        Expr::And(l, r) => format!("{u}::from(({}) != 0 && ({}) != 0)", x(l), x(r)),
        Expr::Or(l, r) => format!("{u}::from(({}) != 0 || ({}) != 0)", x(l), x(r)),
        Expr::Select {
            condition,
            then_branch,
            else_branch,
        } => format!(
            "(if ({}) != 0 {{ {} }} else {{ {} }})",
            x(condition),
            x(then_branch),
            x(else_branch)
        ),
        Expr::ArrayGet { elements, index } => {
            let arms: String = elements
                .iter()
                .enumerate()
                .map(|(i, e)| format!("{i} => {}, ", x(e)))
                .collect();
            format!(
                "(match {} {{ {arms}_ => return Err(AxiomError(\"fixed array index is out of bounds\")) }})",
                x(index)
            )
        }
        Expr::Guard { condition, value } => format!(
            "(if ({}) != 0 {{ {} }} else {{ return Err(AxiomError(\"verified runtime guard failed\")) }})",
            x(condition),
            x(value)
        ),
    }
}

fn rs_type(ty: &Type, wide: bool) -> Result<&'static str, String> {
    match ty {
        Type::U32 | Type::I32 if wide => Ok(if *ty == Type::U32 { "u64" } else { "i64" }),
        Type::U32 => Ok("u32"),
        Type::I32 => Ok("i32"),
        Type::Bool => Ok("bool"),
        other => Err(format!(
            "Rust output supports only u32, i32 and bool, found {other:?}"
        )),
    }
}

const KEYWORDS: &[&str] = &[
    "as", "async", "await", "box", "break", "const", "continue", "dyn", "else", "enum", "extern",
    "false", "fn", "for", "gen", "if", "impl", "in", "let", "loop", "match", "mod", "move", "mut",
    "pub", "ref", "return", "static", "struct", "trait", "true", "try", "type", "unsafe", "use",
    "where", "while", "yield", "abstract", "become", "do", "final", "macro", "override", "priv",
    "typeof", "unsized", "virtual",
];

fn ident(name: &str) -> Result<String, String> {
    if matches!(name, "self" | "Self" | "super" | "crate" | "_") || name == "AxiomError" {
        return Err(format!("rule name {name} is not a valid Rust function"));
    }
    Ok(if KEYWORDS.contains(&name) {
        format!("r#{name}")
    } else {
        name.to_string()
    })
}

const HEADER: &str = "// Generated by axiom. Do not edit.\n\
// Source-level proof covers the AMI expression; this Rust is checked by differential tests, not proven.\n\
#![allow(clippy::all, dead_code, unused_parens, unreachable_code, non_snake_case)]\n\n\
/// A rule was called outside its `need` lines, or a checked operation failed.\n\
#[derive(Debug, Clone, Copy, PartialEq, Eq)]\n\
pub struct AxiomError(pub &'static str);\n";

/// Returns one Rust module exporting every given contract.
pub fn emit_module(items: &[(&Contract, &Expr, Option<&Expr>)]) -> Result<String, String> {
    let mut out = HEADER.to_string();
    for (contract, candidate, guard) in items {
        out += "\n";
        out += &emit_function(contract, candidate, *guard)?;
    }
    let contracts: Vec<&Contract> = items.iter().map(|(c, _, _)| *c).collect();
    for contract in &contracts {
        if let Some((base, parts)) = list_parts(contract, &contracts) {
            out += "\n";
            out += &list_wrapper(base, &parts)?;
        }
    }
    for contract in &contracts {
        if let Some(rule) = &contract.list {
            out += "\n";
            out += &any_length_wrapper(contract, rule)?;
        }
        if let Some(rule) = &contract.repeat {
            out += "\n";
            out += &repeat_wrapper(contract, rule)?;
        }
    }
    let sizes = settle_sizes(&contracts);
    if !sizes.is_empty() {
        out += "\n";
        out += &settle_wrapper(&sizes);
    }
    for (choice, uses) in crate::js::choice_types(&contracts)? {
        let ty = |(ty, wide): &(Type, bool)| match (ty, wide) {
            (Type::I32, false) => "i32",
            (Type::I32, true) => "i64",
            (_, false) => "u32",
            (_, true) => "u64",
        };
        // One number type when every input agrees; else `u8`, which `.into()` widens.
        let first = ty(&uses[0]);
        let word = if uses.iter().all(|u| ty(u) == first) {
            first
        } else {
            "u8"
        };
        out += &format!(
            "\n/// Values of `type {}`: each is its place, from 0.\n#[allow(non_snake_case, non_upper_case_globals, dead_code)]\npub mod {} {{\n",
            choice.name,
            ident(&choice.name)?
        );
        for (at, value) in choice.values.iter().enumerate() {
            out += &format!("    pub const {}: {word} = {at};\n", ident(value)?);
        }
        out += "}\n";
    }
    Ok(out)
}

fn emit_function(
    contract: &Contract,
    candidate: &Expr,
    guard: Option<&Expr>,
) -> Result<String, String> {
    let wide = contract.wide;
    let (u, i, _) = words(wide);
    let ret = rs_type(&contract.return_type, wide)?;
    let mut params = Vec::new();
    let mut lowered = String::new();
    for (index, p) in contract.params.iter().enumerate() {
        params.push(format!("p{index}: {}", rs_type(&p.ty, wide)?));
        lowered += &format!("    let a{index} = p{index} as {u};\n");
    }
    let docs: Vec<&str> = contract.params.iter().map(|p| p.name.as_str()).collect();
    let guard = guard
        .map(|g| {
            format!(
                "    if ({}) == 0 {{\n        return Err(AxiomError(\"{}: input violates a requires clause\"));\n    }}\n",
                expr(g, wide),
                contract.name
            )
        })
        .unwrap_or_default();
    let result = expr(candidate, wide);
    let result = match contract.return_type {
        Type::Bool => format!("({result}) != 0"),
        Type::I32 => format!("({result}) as {i}"),
        _ => result,
    };
    Ok(format!(
        "/// Inputs: {}.\n#[allow(unused_variables)]\npub fn {}({}) -> Result<{ret}, AxiomError> {{\n{lowered}{guard}    Ok({result})\n}}\n",
        docs.join(", "),
        ident(&contract.name)?,
        params.join(", "),
    ))
}

fn list_wrapper(base: &str, parts: &[&Contract]) -> Result<String, String> {
    let first = parts[0];
    let mut params = Vec::new();
    let mut args = Vec::new();
    for (i, (_, size, ty)) in param_groups(first).iter().enumerate() {
        let ty = rs_type(ty, first.wide)?;
        if *size > 0 {
            params.push(format!("p{i}: [{ty}; {size}]"));
            args.extend((0..*size).map(|k| format!("p{i}[{k}]")));
        } else {
            params.push(format!("p{i}: {ty}"));
            args.push(format!("p{i}"));
        }
    }
    let calls: Vec<String> = parts
        .iter()
        .map(|c| Ok(format!("{}({})?", ident(&c.name)?, args.join(", "))))
        .collect::<Result<_, String>>()?;
    Ok(format!(
        "pub fn {}({}) -> Result<[{}; {}], AxiomError> {{\n    Ok([{}])\n}}\n",
        ident(base)?,
        params.join(", "),
        rs_type(&first.return_type, first.wide)?,
        parts.len(),
        calls.join(", ")
    ))
}

/// `cart(prices)` for a rule over lists of any length: checks the length,
/// adds each item rule over the items in a loop, then calls the core. The
/// sums cannot overflow below the length limit (`proofs/lean/Lists.lean`).
fn any_length_wrapper(core: &Contract, rule: &axiom_parser::ListRule) -> Result<String, String> {
    let ty = rs_type(&core.params[0].ty, core.wide)?;
    let first = rule.params.iter().position(|(_, list)| *list).unwrap_or(0);
    let mut params = Vec::new();
    let mut checks = String::new();
    for (i, (_, list)) in rule.params.iter().enumerate() {
        if *list {
            params.push(format!("p{i}: &[{ty}]"));
            if i != first {
                checks += &format!(
                    "    if p{i}.len() != p{first}.len() {{\n        return Err(AxiomError(\"{}: lists must have the same length\"));\n    }}\n",
                    rule.name
                );
            }
        } else {
            params.push(format!("p{i}: {ty}"));
        }
    }
    let item_args: Vec<String> = rule
        .params
        .iter()
        .enumerate()
        .map(|(i, (_, list))| {
            if *list {
                format!("p{i}[i]")
            } else {
                format!("p{i}")
            }
        })
        .collect();
    let mut sums = String::new();
    let (mut first_loop, mut second_loop) = (String::new(), String::new());
    for (k, (item, fold)) in rule.items.iter().zip(&rule.folds).enumerate() {
        sums += &format!("    let mut sum{}: {ty} = 0;\n", k + 1);
        // An item taking list totals (`sum(w / sum(w))`) runs in a second loop.
        let mut args = item_args.clone();
        args.extend(rule.uses[k].iter().map(|j| format!("sum{}", j + 1)));
        let call = format!("{}({})?", ident(item)?, args.join(", "));
        let body = if rule.uses[k].is_empty() {
            &mut first_loop
        } else {
            &mut second_loop
        };
        *body += &match fold {
            axiom_parser::Fold::Sum => {
                format!("        sum{0} = sum{0}.wrapping_add({call});\n", k + 1)
            }
            axiom_parser::Fold::Min | axiom_parser::Fold::Max => format!(
                "        let v = {call};\n        if i == 0 || v {} sum{} {{\n            sum{1} = v;\n        }}\n",
                if *fold == axiom_parser::Fold::Min {
                    "<"
                } else {
                    ">"
                },
                k + 1
            ),
        };
    }
    let mut core_args: Vec<String> = if rule.each {
        item_args.clone()
    } else {
        rule.params
            .iter()
            .enumerate()
            .filter(|(_, (_, list))| !*list)
            .map(|(i, _)| format!("p{i}"))
            .collect()
    };
    core_args.push(format!("n as {ty}"));
    if rule.position {
        core_args.push(format!("(i + 1) as {ty}"));
    }
    core_args.extend((1..=rule.items.len()).map(|k| format!("sum{k}")));
    let call = format!("{}({})", ident(&core.name)?, core_args.join(", "));
    let ret = rs_type(&core.return_type, core.wide)?;
    let (ret, tail) = if rule.each {
        (
            format!("Vec<{ret}>"),
            format!("(0..n).map(|i| {call}).collect()"),
        )
    } else {
        (ret.to_string(), call)
    };
    let docs: Vec<&str> = rule.params.iter().map(|(p, _)| p.as_str()).collect();
    Ok(format!(
        "/// Inputs: {}.\npub fn {}({}) -> Result<{ret}, AxiomError> {{\n{checks}    let n = p{first}.len();\n    if {}n > {} {{\n        return Err(AxiomError(\"{}: input violates a requires clause\"));\n    }}\n{sums}    for i in 0..n {{\n{first_loop}    }}\n{}    {tail}\n}}\n",
        docs.join(", "),
        ident(&rule.name)?,
        params.join(", "),
        too_few(rule, "n", "||"),
        rule.max,
        rule.name,
        if second_loop.is_empty() {
            String::new()
        } else {
            format!("    for i in 0..n {{\n{second_loop}    }}\n")
        },
    ))
}

/// `savings(principal, rate, years)` for a `repeat` rule: the start, then the
/// step `years` times. Every promise holds after each step
/// (`proofs/lean/Repeat.lean`).
fn repeat_wrapper(step: &Contract, rule: &axiom_parser::RepeatRule) -> Result<String, String> {
    let inputs = &step.params[..step.params.len() - 1];
    let mut params = Vec::new();
    let mut args = Vec::new();
    for (i, param) in inputs.iter().enumerate() {
        params.push(format!("p{i}: {}", rs_type(&param.ty, step.wide)?));
        args.push(format!("p{i}"));
    }
    let ret = rs_type(&step.return_type, step.wide)?;
    let names: Vec<&str> = inputs.iter().map(|p| p.name.as_str()).collect();
    Ok(format!(
        "/// Inputs: {}.\npub fn {}({}) -> Result<{ret}, AxiomError> {{\n    let mut value = {}({})?;\n    for _ in 0..p{} {{\n        value = {}({}, value)?;\n    }}\n    Ok(value)\n}}\n",
        names.join(", "),
        ident(&rule.name)?,
        params.join(", "),
        ident(&rule.start)?,
        args.join(", "),
        rule.count,
        ident(&step.name)?,
        args.join(", "),
    ))
}

/// `n < least <or> ` when the rule has `need n >= least` (shared by all emitters).
pub(crate) fn too_few(rule: &axiom_parser::ListRule, n: &str, or: &str) -> String {
    if rule.least == 0 {
        String::new()
    } else {
        format!("{n} < {} {or} ", rule.least)
    }
}

/// `settle(owe)`: N from the slice length, N * N transfers, item (i-1)*N+j
/// from person i to person j.
fn settle_wrapper(sizes: &[usize]) -> String {
    let mut arms = String::new();
    for n in sizes {
        let args: Vec<String> = (0..*n).map(|k| format!("owe[{k}]")).collect();
        let calls: Vec<String> = (1..=*n)
            .flat_map(|i| (1..=*n).map(move |j| (i, j)))
            .map(|(i, j)| format!("settle_{n}_{i}_{j}({})?", args.join(", ")))
            .collect();
        arms += &format!("        {n} => vec![{}],\n", calls.join(", "));
    }
    format!(
        "pub fn settle(owe: &[i32]) -> Result<Vec<i32>, AxiomError> {{\n    Ok(match owe.len() {{\n{arms}        _ => return Err(AxiomError(\"settle: unsupported number of people\")),\n    }})\n}}\n"
    )
}
