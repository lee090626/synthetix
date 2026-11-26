//! JavaScript/TypeScript-declaration emitter for verified AMI candidates.
//!
//! The proof covers the AMI expression; this module only prints it with u32
//! wrapping semantics. Equivalence with the AMI evaluator is checked by the
//! differential test in `tests/js_emit.rs`, not proven.

use axiom_ami::Expr;
use axiom_parser::{Contract, Type};

fn bin(op: &str, lhs: &Expr, rhs: &Expr) -> String {
    format!("(({} {op} {}) >>> 0)", expr(lhs), expr(rhs))
}

fn cmp(op: &str, lhs: &Expr, rhs: &Expr, signed: bool) -> String {
    let (l, r) = (expr(lhs), expr(rhs));
    if signed {
        format!("(({l} | 0) {op} ({r} | 0) ? 1 : 0)")
    } else {
        format!("({l} {op} {r} ? 1 : 0)")
    }
}

/// Prints one AMI expression; every sub-expression evaluates to a u32 number.
pub fn expr(e: &Expr) -> String {
    match e {
        Expr::Arg(i) => format!("a{i}"),
        Expr::Const(v) => v.to_string(),
        Expr::Add(l, r) => bin("+", l, r),
        Expr::Sub(l, r) => bin("-", l, r),
        Expr::Mul(l, r) => format!("(Math.imul({}, {}) >>> 0)", expr(l), expr(r)),
        Expr::Div(l, r) => format!("(Math.floor({} / {}) >>> 0)", expr(l), expr(r)),
        Expr::Rem(l, r) => format!("(({} % {}) >>> 0)", expr(l), expr(r)),
        Expr::SDiv(l, r) => format!("(Math.trunc(({} | 0) / ({} | 0)) >>> 0)", expr(l), expr(r)),
        Expr::SRem(l, r) => format!("((({} | 0) % ({} | 0)) >>> 0)", expr(l), expr(r)),
        Expr::Shl(l, r) => format!("(({} << ({} & 31)) >>> 0)", expr(l), expr(r)),
        Expr::BitAnd(l, r) => bin("&", l, r),
        Expr::BitOr(l, r) => bin("|", l, r),
        Expr::BitXor(l, r) => bin("^", l, r),
        Expr::Eq(l, r) => cmp("===", l, r, false),
        Expr::Ne(l, r) => cmp("!==", l, r, false),
        Expr::Lt(l, r) => cmp("<", l, r, false),
        Expr::Le(l, r) => cmp("<=", l, r, false),
        Expr::Gt(l, r) => cmp(">", l, r, false),
        Expr::Ge(l, r) => cmp(">=", l, r, false),
        Expr::SLt(l, r) => cmp("<", l, r, true),
        Expr::SLe(l, r) => cmp("<=", l, r, true),
        Expr::SGt(l, r) => cmp(">", l, r, true),
        Expr::SGe(l, r) => cmp(">=", l, r, true),
        Expr::And(l, r) => format!("({} !== 0 && {} !== 0 ? 1 : 0)", expr(l), expr(r)),
        Expr::Or(l, r) => format!("({} !== 0 || {} !== 0 ? 1 : 0)", expr(l), expr(r)),
        Expr::Select {
            condition,
            then_branch,
            else_branch,
        } => format!(
            "({} !== 0 ? {} : {})",
            expr(condition),
            expr(then_branch),
            expr(else_branch)
        ),
        Expr::ArrayGet { elements, index } => {
            let items: Vec<String> = elements
                .iter()
                .map(|e| format!("() => {}", expr(e)))
                .collect();
            format!("$at({}, [{}])", expr(index), items.join(", "))
        }
        Expr::Guard { condition, value } => format!(
            "({} !== 0 ? {} : $trap(\"verified runtime guard failed\"))",
            expr(condition),
            expr(value)
        ),
    }
}

/// Prints one AMI expression of a `wide` contract; every sub-expression is a
/// bigint in `0..2^64`.
pub fn expr64(e: &Expr) -> String {
    let wrap = |op: &str, l: &Expr, r: &Expr| {
        format!("BigInt.asUintN(64, {} {op} {})", expr64(l), expr64(r))
    };
    let cmp = |op: &str, l: &Expr, r: &Expr, signed: bool| {
        if signed {
            format!(
                "(BigInt.asIntN(64, {}) {op} BigInt.asIntN(64, {}) ? 1n : 0n)",
                expr64(l),
                expr64(r)
            )
        } else {
            format!("({} {op} {} ? 1n : 0n)", expr64(l), expr64(r))
        }
    };
    match e {
        Expr::Arg(i) => format!("a{i}"),
        Expr::Const(v) => format!("{v}n"),
        Expr::Add(l, r) => wrap("+", l, r),
        Expr::Sub(l, r) => wrap("-", l, r),
        Expr::Mul(l, r) => wrap("*", l, r),
        Expr::Div(l, r) => format!("$div64({}, {}, false)", expr64(l), expr64(r)),
        Expr::Rem(l, r) => format!("$rem64({}, {}, false)", expr64(l), expr64(r)),
        Expr::SDiv(l, r) => format!("$div64({}, {}, true)", expr64(l), expr64(r)),
        Expr::SRem(l, r) => format!("$rem64({}, {}, true)", expr64(l), expr64(r)),
        Expr::Shl(l, r) => format!("BigInt.asUintN(64, {} << ({} & 63n))", expr64(l), expr64(r)),
        Expr::BitAnd(l, r) => format!("({} & {})", expr64(l), expr64(r)),
        Expr::BitOr(l, r) => format!("({} | {})", expr64(l), expr64(r)),
        Expr::BitXor(l, r) => format!("({} ^ {})", expr64(l), expr64(r)),
        Expr::Eq(l, r) => cmp("===", l, r, false),
        Expr::Ne(l, r) => cmp("!==", l, r, false),
        Expr::Lt(l, r) => cmp("<", l, r, false),
        Expr::Le(l, r) => cmp("<=", l, r, false),
        Expr::Gt(l, r) => cmp(">", l, r, false),
        Expr::Ge(l, r) => cmp(">=", l, r, false),
        Expr::SLt(l, r) => cmp("<", l, r, true),
        Expr::SLe(l, r) => cmp("<=", l, r, true),
        Expr::SGt(l, r) => cmp(">", l, r, true),
        Expr::SGe(l, r) => cmp(">=", l, r, true),
        Expr::And(l, r) => format!("({} !== 0n && {} !== 0n ? 1n : 0n)", expr64(l), expr64(r)),
        Expr::Or(l, r) => format!("({} !== 0n || {} !== 0n ? 1n : 0n)", expr64(l), expr64(r)),
        Expr::Select {
            condition,
            then_branch,
            else_branch,
        } => format!(
            "({} !== 0n ? {} : {})",
            expr64(condition),
            expr64(then_branch),
            expr64(else_branch)
        ),
        Expr::ArrayGet { elements, index } => {
            let items: Vec<String> = elements
                .iter()
                .map(|e| format!("() => {}", expr64(e)))
                .collect();
            format!("$at({}, [{}])", expr64(index), items.join(", "))
        }
        Expr::Guard { condition, value } => format!(
            "({} !== 0n ? {} : $trap(\"verified runtime guard failed\"))",
            expr64(condition),
            expr64(value)
        ),
    }
}

fn ts_type(ty: &Type, wide: bool) -> Result<&'static str, String> {
    match ty {
        Type::U32 | Type::I32 if wide => Ok("bigint"),
        Type::U32 | Type::I32 => Ok("number"),
        Type::Bool => Ok("boolean"),
        other => Err(format!(
            "JS output supports only u32, i32 and bool, found {other:?}"
        )),
    }
}

const RESERVED: &[&str] = &[
    "break",
    "case",
    "catch",
    "class",
    "const",
    "continue",
    "debugger",
    "default",
    "delete",
    "do",
    "else",
    "enum",
    "export",
    "extends",
    "false",
    "finally",
    "for",
    "function",
    "if",
    "import",
    "in",
    "instanceof",
    "new",
    "null",
    "return",
    "super",
    "switch",
    "this",
    "throw",
    "true",
    "try",
    "typeof",
    "var",
    "void",
    "while",
    "with",
    "yield",
    "let",
    "static",
    "await",
    "implements",
    "interface",
    "package",
    "private",
    "protected",
    "public",
    "arguments",
    "eval",
    "Number",
    "Math",
    "BigInt",
    "Array",
    "TypeError",
    "RangeError",
];

fn check_for(name: &str, ty: &Type, var: &str, wide: bool) -> Result<String, String> {
    Ok(match ty {
        Type::U32 if wide => format!(
            "  if (typeof {var} !== \"bigint\" || {var} < 0n || {var} > 0xffffffffffffffffn) throw new TypeError(\"{name}: expected u64 bigint\");\n"
        ),
        Type::I32 if wide => format!(
            "  if (typeof {var} !== \"bigint\" || {var} < -0x8000000000000000n || {var} > 0x7fffffffffffffffn) throw new TypeError(\"{name}: expected i64 bigint\");\n"
        ),
        Type::U32 => format!(
            "  if (!Number.isInteger({var}) || {var} < 0 || {var} > 0xffffffff) throw new TypeError(\"{name}: expected u32\");\n"
        ),
        Type::I32 => format!(
            "  if (!Number.isInteger({var}) || {var} < -0x80000000 || {var} > 0x7fffffff) throw new TypeError(\"{name}: expected i32\");\n"
        ),
        Type::Bool => format!(
            "  if (typeof {var} !== \"boolean\") throw new TypeError(\"{name}: expected bool\");\n"
        ),
        other => return Err(format!("unsupported JS input type {other:?}")),
    })
}

pub const PROOF_NOTE: &str = "Source-level proof covers the AMI expression; this JS is checked by differential tests, not proven.";

const HEADER: &str = "// Generated by axiom. Do not edit.\n";

/// Returns `(module.js, module.d.ts)` for a verified contract.
pub fn emit_module(
    contract: &Contract,
    candidate: &Expr,
    guard: Option<&Expr>,
    proof_note: &str,
) -> Result<(String, String), String> {
    emit_modules(&[(contract, candidate, guard)], proof_note)
}

/// Returns one ES module (and its declarations) exporting every given contract.
pub fn emit_modules(
    items: &[(&Contract, &Expr, Option<&Expr>)],
    proof_note: &str,
) -> Result<(String, String), String> {
    let mut js = format!(
        "{HEADER}// {proof_note}\n\
         const $trap = (message) => {{ throw new RangeError(message); }};\n\
         const $at = (index, items) => (index < items.length ? items[Number(index)]() : $trap(\"fixed array index is out of bounds\"));\n\
         const $div64 = (l, r, s) => {{ const [a, b] = s ? [BigInt.asIntN(64, l), BigInt.asIntN(64, r)] : [l, r]; if (b === 0n || (s && a === -(2n ** 63n) && b === -1n)) $trap(\"division by zero or overflow\"); return BigInt.asUintN(64, a / b); }};\n\
         const $rem64 = (l, r, s) => {{ const [a, b] = s ? [BigInt.asIntN(64, l), BigInt.asIntN(64, r)] : [l, r]; if (b === 0n || (s && a === -(2n ** 63n) && b === -1n)) $trap(\"division by zero or overflow\"); return BigInt.asUintN(64, a % b); }};\n"
    );
    let mut dts = HEADER.to_string();
    for (contract, candidate, guard) in items {
        let (function, declaration) = emit_function(contract, candidate, *guard)?;
        js += "\n";
        js += &function;
        dts += &declaration;
    }
    let contracts: Vec<&Contract> = items.iter().map(|(c, _, _)| *c).collect();
    for contract in &contracts {
        if let Some((function, declaration)) = list_wrapper(contract, &contracts)? {
            js += "\n";
            js += &function;
            dts += &declaration;
        }
    }
    for contract in &contracts {
        if let Some(rule) = &contract.list {
            let (function, declaration) = any_length_wrapper(contract, rule)?;
            js += "\n";
            js += &function;
            dts += &declaration;
        }
        if let Some(record) = &contract.record_answer {
            let (function, declaration) = record_wrapper(contract, record, &contracts)?;
            js += "\n";
            js += &function;
            dts += &declaration;
        }
        if let Some(rule) = &contract.repeat {
            let (function, declaration) = repeat_wrapper(contract, rule)?;
            js += "\n";
            js += &function;
            dts += &declaration;
        }
    }
    if let Some((function, declaration)) = settle_wrapper(&contracts) {
        js += "\n";
        js += &function;
        dts += &declaration;
    }
    for (choice, uses) in choice_types(&contracts)? {
        // Rules with big numbers take `bigint`; the values match when only they use it.
        let big = if uses.iter().all(|(_, wide)| *wide) {
            "n"
        } else {
            ""
        };
        let values: Vec<String> = choice
            .values
            .iter()
            .enumerate()
            .map(|(at, value)| format!("{value}: {at}{big}"))
            .collect();
        js += &format!(
            "\n/** Values of `type {}`: each is its place, from 0. */\nexport const {} = Object.freeze({{ {} }});\n",
            choice.name,
            choice.name,
            values.join(", ")
        );
        let types: Vec<String> = values.iter().map(|v| format!("readonly {v}")).collect();
        dts += &format!(
            "export declare const {}: {{ {} }};\n",
            choice.name,
            types.join("; ")
        );
    }
    Ok((js, dts))
}

/// A choice type and the number type and width of each input that holds it.
type ChoiceUses<'a> = (
    &'a axiom_parser::ChoiceType,
    Vec<(axiom_parser::Type, bool)>,
);

/// Every choice type the rules take (`type Size = small | large`, issue
/// #226), once, with the number type and width of each input that holds it.
pub(crate) fn choice_types<'a>(contracts: &[&'a Contract]) -> Result<Vec<ChoiceUses<'a>>, String> {
    let mut out: Vec<ChoiceUses> = Vec::new();
    for contract in contracts {
        let answer = contract
            .answer_choice
            .iter()
            .map(|c| (c, contract.return_type.clone()));
        let inputs = contract.choices.iter().map(|(input, choice)| {
            let ty = contract
                .params
                .iter()
                .find(|p| &p.name == input)
                .map_or(axiom_parser::Type::U32, |p| p.ty.clone());
            (choice, ty)
        });
        for (choice, ty) in inputs.chain(answer) {
            match out.iter_mut().find(|(c, _)| c.name == choice.name) {
                Some((known, _)) if known.values != choice.values => {
                    return Err(format!(
                        "two files declare `type {}` with different values",
                        choice.name
                    ));
                }
                Some((_, uses)) => uses.push((ty, contract.wide)),
                None => out.push((choice, vec![(ty, contract.wide)])),
            }
        }
    }
    Ok(out)
}

/// Group sizes with generated `settle_N_i_j`, unless a rule is named `settle`.
pub(crate) fn settle_sizes(all: &[&Contract]) -> Vec<usize> {
    if all.iter().any(|c| c.name == "settle") {
        return Vec::new();
    }
    (2..=axiom_parser::simple::SETTLE_MAX)
        .filter(|n| all.iter().any(|c| c.name == format!("settle_{n}_1_1")))
        .collect()
}

/// For `f_1`, the rule `f` was a list answer split into `f_1..f_N` (same
/// inputs), and no rule is named `f`: returns `f` and the parts.
pub(crate) fn list_parts<'a, 'f>(
    first: &'f Contract,
    all: &[&'a Contract],
) -> Option<(&'f str, Vec<&'a Contract>)> {
    let base = first.name.strip_suffix("_1")?;
    if base.starts_with("settle_") {
        return None;
    }
    let parts: Vec<&Contract> = (1..)
        .map_while(|i| {
            let name = format!("{base}_{i}");
            all.iter().find(|c| c.name == name).copied()
        })
        .collect();
    let taken = all.iter().any(|c| c.name == base);
    let same = parts
        .iter()
        .all(|c| c.params == first.params && c.return_type == first.return_type);
    (parts.len() >= 2 && !taken && same).then_some((base, parts))
}

/// `settle(owe)` over every generated `settle_N_i_j`: picks N from the array
/// length and returns the N * N transfers, item (i-1)*N+j from i to j.
fn settle_wrapper(all: &[&Contract]) -> Option<(String, String)> {
    let sizes = settle_sizes(all);
    if sizes.is_empty() {
        return None;
    }
    let mut cases = String::new();
    for n in &sizes {
        let calls: Vec<String> = (1..=*n)
            .flat_map(|i| (1..=*n).map(move |j| format!("settle_{n}_{i}_{j}(...owe)")))
            .collect();
        cases += &format!("  if (owe.length === {n}) return [{}];\n", calls.join(", "));
    }
    let list: Vec<String> = sizes.iter().map(usize::to_string).collect();
    Some((
        format!(
            "export function settle(owe) {{\n  if (!Array.isArray(owe)) throw new TypeError(\"settle: owe must be an array\");\n{cases}  throw new TypeError(\"settle: owe must have {} items\");\n}}\n",
            list.join(" or ")
        ),
        "export function settle(owe: number[]): number[];\n".to_string(),
    ))
}

/// `savings(principal, rate, years)` for a `repeat` rule: the start, then the
/// step `years` times. Every promise holds after each step
/// (`proofs/lean/Repeat.lean`).
fn repeat_wrapper(
    step: &Contract,
    rule: &axiom_parser::RepeatRule,
) -> Result<(String, String), String> {
    let inputs = &step.params[..step.params.len() - 1];
    let args: Vec<String> = (0..inputs.len()).map(|i| format!("p{i}")).collect();
    let mut typed = Vec::new();
    for (i, param) in inputs.iter().enumerate() {
        typed.push(format!("p{i}: {}", ts_type(&param.ty, step.wide)?));
    }
    let ret = ts_type(&step.return_type, step.wide)?;
    let zero = if step.wide { "0n" } else { "0" };
    let one = if step.wide { "1n" } else { "1" };
    Ok((
        format!(
            "export function {}({}) {{\n  let value = {}({});\n  for (let i = {zero}; i < p{}; i += {one}) value = {}({}, value);\n  return value;\n}}\n",
            rule.name,
            args.join(", "),
            rule.start,
            args.join(", "),
            rule.count,
            step.name,
            args.join(", "),
        ),
        format!(
            "export function {}({}): {ret};\n",
            rule.name,
            typed.join(", ")
        ),
    ))
}

/// `quote(price, qty)` for a rule answering a record: an object with each
/// field's proven rule.
fn record_wrapper(
    first: &Contract,
    record: &axiom_parser::RecordAnswer,
    all: &[&Contract],
) -> Result<(String, String), String> {
    let args: Vec<String> = (0..first.params.len()).map(|i| format!("p{i}")).collect();
    let mut typed = Vec::new();
    for (i, param) in first.params.iter().enumerate() {
        typed.push(format!("p{i}: {}", ts_type(&param.ty, first.wide)?));
    }
    let mut values = Vec::new();
    let mut types = Vec::new();
    for (field, rule) in &record.fields {
        let contract = all
            .iter()
            .find(|c| &c.name == rule)
            .ok_or_else(|| format!("missing `{rule}`"))?;
        values.push(format!("{field}: {rule}({})", args.join(", ")));
        types.push(format!(
            "{field}: {}",
            ts_type(&contract.return_type, contract.wide)?
        ));
    }
    Ok((
        format!(
            "/** `{}`: a `{}`, each field from its proven rule. */\nexport function {}({}) {{\n  return {{ {} }};\n}}\n",
            record.name,
            record.record,
            record.name,
            args.join(", "),
            values.join(", ")
        ),
        format!(
            "export function {}({}): {{ {} }};\n",
            record.name,
            typed.join(", "),
            types.join("; ")
        ),
    ))
}

/// `cart(prices)` for a rule over lists of any length: checks the length,
/// adds each item rule over the items in a loop, then calls the core. The
/// sums cannot overflow below the length limit (`proofs/lean/Lists.lean`).
fn any_length_wrapper(
    core: &Contract,
    rule: &axiom_parser::ListRule,
) -> Result<(String, String), String> {
    let wide = core.wide;
    let item_ty = ts_type(&core.params[0].ty, wide)?;
    let names: Vec<String> = (0..rule.params.len()).map(|i| format!("p{i}")).collect();
    let first = rule.params.iter().position(|(_, list)| *list).unwrap_or(0);
    let mut checks = String::new();
    let mut dts = Vec::new();
    for (i, (name, list)) in rule.params.iter().enumerate() {
        let doc = if RESERVED.contains(&name.as_str()) {
            names[i].clone()
        } else {
            name.clone()
        };
        if *list {
            checks += &format!(
                "  if (!Array.isArray(p{i}) || p{i}.length !== p{first}.length) throw new TypeError(\"{}: {doc} must be an array as long as the other lists\");\n",
                rule.name
            );
            dts.push(format!("{doc}: {item_ty}[]"));
        } else {
            dts.push(format!("{doc}: {item_ty}"));
        }
    }
    let zero = if wide { "0n" } else { "0" };
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
    let (mut loop_body, mut second_loop) = (String::new(), String::new());
    let mut sums = String::new();
    for (k, (item, fold)) in rule.items.iter().zip(&rule.folds).enumerate() {
        sums += &format!("  let sum{} = {zero};\n", k + 1);
        // An item taking list totals (`sum(w / sum(w))`) runs in a second loop.
        let mut args = item_args.clone();
        args.extend(rule.uses[k].iter().map(|j| format!("sum{}", j + 1)));
        let call = format!("{item}({})", args.join(", "));
        let body = if rule.uses[k].is_empty() {
            &mut loop_body
        } else {
            &mut second_loop
        };
        *body += &match fold {
            axiom_parser::Fold::Sum => format!("    sum{} += {call};\n", k + 1),
            axiom_parser::Fold::Min | axiom_parser::Fold::Max => format!(
                "    {{ const v = {call}; if (i === 0 || v {} sum{1}) sum{1} = v; }}\n",
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
    core_args.push(if wide { "BigInt(n)".into() } else { "n".into() });
    if rule.position {
        core_args.push(if wide {
            "BigInt(i + 1)".into()
        } else {
            "i + 1".into()
        });
    }
    if !second_loop.is_empty() {
        loop_body += &format!("  }}\n  for (let i = 0; i < n; i++) {{\n{second_loop}");
    }
    core_args.extend((1..=rule.items.len()).map(|k| format!("sum{k}")));
    let call = format!("{}({})", core.name, core_args.join(", "));
    let ret = ts_type(&core.return_type, wide)?;
    let (ret, tail) = if rule.each {
        (
            format!("{ret}[]"),
            format!("Array.from({{ length: n }}, (_, i) => {call})"),
        )
    } else {
        (ret.to_string(), call)
    };
    let name = &rule.name;
    Ok((
        format!(
            "export function {name}({}) {{\n{checks}  const n = p{first}.length;\n  if ({few}n > {max}) throw new RangeError(\"{name}: input violates a requires clause\");\n{sums}  for (let i = 0; i < n; i++) {{\n{loop_body}  }}\n  return {tail};\n}}\n",
            names.join(", "),
            max = rule.max,
            few = crate::rs::too_few(rule, "n", "||"),
        ),
        format!("export function {name}({}): {ret};\n", dts.join(", ")),
    ))
}

/// Splits `x_1..x_k` (k >= 2, same type) runs into one list parameter.
pub(crate) fn param_groups(contract: &Contract) -> Vec<(String, usize, Type)> {
    let mut groups: Vec<(String, usize, Type)> = Vec::new();
    for p in &contract.params {
        let split = p
            .name
            .rsplit_once('_')
            .and_then(|(base, n)| n.parse::<usize>().ok().map(|n| (base.to_string(), n)));
        if let (Some((base, n)), Some(last)) = (&split, groups.last_mut())
            && last.0 == *base
            && last.1 + 1 == *n
            && last.2 == p.ty
        {
            last.1 += 1;
            continue;
        }
        match split {
            Some((base, 1)) => groups.push((base, 1, p.ty.clone())),
            _ => groups.push((p.name.clone(), 0, p.ty.clone())),
        }
    }
    // A run of one is an ordinary scalar named `x_1`.
    for g in &mut groups {
        if g.1 == 1 {
            g.0 += "_1";
            g.1 = 0;
        }
    }
    groups
}

/// For a list-answer rule split into `f_1..f_N`, emits `f(...)` taking and
/// returning arrays; it only calls the verified `f_i` exports.
fn list_wrapper(first: &Contract, all: &[&Contract]) -> Result<Option<(String, String)>, String> {
    let Some((base, parts)) = list_parts(first, all) else {
        return Ok(None);
    };
    if RESERVED.contains(&base) || base.starts_with('$') {
        return Ok(None);
    }
    let groups = param_groups(first);
    let mut params = Vec::new();
    let mut checks = String::new();
    let mut args = Vec::new();
    let mut dts = Vec::new();
    for (i, (name, size, ty)) in groups.iter().enumerate() {
        let var = format!("p{i}");
        let doc = if RESERVED.contains(&name.as_str()) {
            var.clone()
        } else {
            name.clone()
        };
        let ty = ts_type(ty, first.wide)?;
        if *size > 0 {
            checks += &format!(
                "  if (!Array.isArray({var}) || {var}.length !== {size}) throw new TypeError(\"{base}: {doc} must be an array of {size}\");\n"
            );
            args.push(format!("...{var}"));
            dts.push(format!("{doc}: {ty}[]"));
        } else {
            args.push(var.clone());
            dts.push(format!("{doc}: {ty}"));
        }
        params.push(var);
    }
    let calls: Vec<String> = parts
        .iter()
        .map(|c| format!("{}({})", c.name, args.join(", ")))
        .collect();
    Ok(Some((
        format!(
            "export function {base}({}) {{\n{checks}  return [{}];\n}}\n",
            params.join(", "),
            calls.join(", ")
        ),
        format!(
            "export function {base}({}): {}[];\n",
            dts.join(", "),
            ts_type(&first.return_type, first.wide)?
        ),
    )))
}

fn emit_function(
    contract: &Contract,
    candidate: &Expr,
    guard: Option<&Expr>,
) -> Result<(String, String), String> {
    let wide = contract.wide;
    let print = if wide { expr64 } else { expr };
    let ret = ts_type(&contract.return_type, wide)?;
    if RESERVED.contains(&contract.name.as_str()) || contract.name.starts_with('$') {
        return Err(format!(
            "contract name {} is not a valid JS export",
            contract.name
        ));
    }
    let mut checks = String::new();
    let mut lowered = String::new();
    let mut names = Vec::new();
    let mut dts_params = Vec::new();
    for (i, p) in contract.params.iter().enumerate() {
        let ty = ts_type(&p.ty, wide)?;
        let var = format!("p{i}");
        checks += &check_for(&contract.name, &p.ty, &var, wide)?;
        lowered += &match (&p.ty, wide) {
            (Type::Bool, false) => format!("  const a{i} = {var} ? 1 : 0;\n"),
            (Type::Bool, true) => format!("  const a{i} = {var} ? 1n : 0n;\n"),
            (_, false) => format!("  const a{i} = {var} >>> 0;\n"),
            (_, true) => format!("  const a{i} = BigInt.asUintN(64, {var});\n"),
        };
        names.push(var);
        let doc_name = if RESERVED.contains(&p.name.as_str()) {
            format!("p{i}")
        } else {
            p.name.clone()
        };
        dts_params.push(format!("{doc_name}: {ty}"));
    }
    let guard_js = guard
        .map(|g| {
            format!(
                "  if (!{}) throw new RangeError(\"{}: input violates a requires clause\");\n",
                print(g),
                contract.name
            )
        })
        .unwrap_or_default();
    let result = print(candidate);
    let result = match (&contract.return_type, wide) {
        (Type::Bool, false) => format!("{result} !== 0"),
        (Type::Bool, true) => format!("{result} !== 0n"),
        (Type::I32, false) => format!("({result}) | 0"),
        (Type::I32, true) => format!("BigInt.asIntN(64, {result})"),
        _ => result,
    };
    Ok((
        format!(
            "export function {name}({params}) {{\n{checks}{lowered}{guard_js}  return {result};\n}}\n",
            name = contract.name,
            params = names.join(", "),
        ),
        format!(
            "export function {}({}): {ret};\n",
            contract.name,
            dts_params.join(", ")
        ),
    ))
}

#[cfg(test)]
mod tests {
    #[test]
    fn settle_wrapper_takes_every_size_up_to_the_parser_limit() {
        let module = axiom_parser::parse_module("use settle for 21\n").unwrap();
        let all: Vec<&axiom_parser::Contract> = module.contracts.iter().collect();
        assert_eq!(super::settle_sizes(&all), [21]);
    }

    #[test]
    fn js_globals_the_emitted_code_uses_are_reserved() {
        for name in [
            "Number",
            "Math",
            "BigInt",
            "Array",
            "TypeError",
            "RangeError",
        ] {
            assert!(super::RESERVED.contains(&name), "{name}");
        }
    }
}
