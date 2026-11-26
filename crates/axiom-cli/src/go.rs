//! Go source emitter for verified AMI candidates.
//!
//! The proof covers the AMI expression; this module prints it with the same
//! wrapping semantics as `Expr::try_eval_bits`: every value is a `uint64`, cut
//! to 32 bits after each operation unless the rule is `wide`. Equivalence is
//! checked by `tests/go_emit.rs`, not proven.

use std::collections::HashSet;

use axiom_ami::Expr;
use axiom_parser::{Contract, Type};

use crate::js::{list_parts, param_groups, settle_sizes};

/// Prints one AMI expression; every value is a `uint64` word of `bits` bits.
fn expr(e: &Expr, bits: u32) -> String {
    let x = |e: &Expr| expr(e, bits);
    let mask = |s: String| {
        if bits == 64 {
            format!("({s})")
        } else {
            format!("(({s}) & 0xFFFFFFFF)")
        }
    };
    let call = |f: &str, l: &Expr, r: &Expr| format!("{f}({}, {}, {bits})", x(l), x(r));
    let cmp = |op: &str, l: &Expr, r: &Expr| format!("b2u(({}) {op} ({}))", x(l), x(r));
    let scmp = |op: &str, l: &Expr, r: &Expr| {
        format!("b2u(sx({}, {bits}) {op} sx({}, {bits}))", x(l), x(r))
    };
    match e {
        Expr::Arg(i) => format!("a{i}"),
        // `w(..)` is not a Go constant, so `w(0) - w(1)` wraps at run time
        // instead of failing to compile.
        Expr::Const(v) => format!("w({v})"),
        Expr::Add(l, r) => mask(format!("({}) + ({})", x(l), x(r))),
        Expr::Sub(l, r) => mask(format!("({}) - ({})", x(l), x(r))),
        Expr::Mul(l, r) => mask(format!("({}) * ({})", x(l), x(r))),
        Expr::Div(l, r) => call("div", l, r),
        Expr::Rem(l, r) => call("rem", l, r),
        Expr::SDiv(l, r) => call("sdiv", l, r),
        Expr::SRem(l, r) => call("srem", l, r),
        Expr::Shl(l, r) => mask(format!("({}) << (({}) & {})", x(l), x(r), bits - 1)),
        Expr::BitAnd(l, r) => format!("(({}) & ({}))", x(l), x(r)),
        Expr::BitOr(l, r) => format!("(({}) | ({}))", x(l), x(r)),
        Expr::BitXor(l, r) => format!("(({}) ^ ({}))", x(l), x(r)),
        Expr::Eq(l, r) => cmp("==", l, r),
        Expr::Ne(l, r) => cmp("!=", l, r),
        Expr::Lt(l, r) => cmp("<", l, r),
        Expr::Le(l, r) => cmp("<=", l, r),
        Expr::Gt(l, r) => cmp(">", l, r),
        Expr::Ge(l, r) => cmp(">=", l, r),
        Expr::SLt(l, r) => scmp("<", l, r),
        Expr::SLe(l, r) => scmp("<=", l, r),
        Expr::SGt(l, r) => scmp(">", l, r),
        Expr::SGe(l, r) => scmp(">=", l, r),
        Expr::And(l, r) => format!("b2u(({}) != 0 && ({}) != 0)", x(l), x(r)),
        Expr::Or(l, r) => format!("b2u(({}) != 0 || ({}) != 0)", x(l), x(r)),
        Expr::Select {
            condition,
            then_branch,
            else_branch,
        } => format!(
            "func() uint64 {{ if ({}) != 0 {{ return {} }}; return {} }}()",
            x(condition),
            x(then_branch),
            x(else_branch)
        ),
        Expr::ArrayGet { elements, index } => {
            let arms: String = elements
                .iter()
                .enumerate()
                .map(|(i, e)| format!("case {i}: return {}; ", x(e)))
                .collect();
            format!(
                "func() uint64 {{ switch {} {{ {arms}}}; panic(&AxiomError{{\"fixed array index is out of bounds\"}}) }}()",
                x(index)
            )
        }
        Expr::Guard { condition, value } => format!(
            "func() uint64 {{ if ({}) != 0 {{ return {} }}; panic(&AxiomError{{\"verified runtime guard failed\"}}) }}()",
            x(condition),
            x(value)
        ),
    }
}

fn go_type(ty: &Type, wide: bool) -> Result<&'static str, String> {
    match (ty, wide) {
        (Type::U32, true) => Ok("uint64"),
        (Type::I32, true) => Ok("int64"),
        (Type::U32, false) => Ok("uint32"),
        (Type::I32, false) => Ok("int32"),
        (Type::Bool, _) => Ok("bool"),
        (other, _) => Err(format!(
            "Go output supports only u32, i32 and bool, found {other:?}"
        )),
    }
}

/// `fee_first` -> `FeeFirst`, `settle_4_1_2` -> `Settle_4_1_2`: Go exports
/// names that start with a capital; number parts keep their `_`.
fn exported(name: &str) -> String {
    name.split('_')
        .map(|part| match part.chars().next() {
            Some(first) if first.is_ascii_digit() => format!("_{part}"),
            Some(first) => first.to_ascii_uppercase().to_string() + &part[first.len_utf8()..],
            None => String::new(),
        })
        .collect()
}

const GO_KEYWORDS: &[&str] = &[
    "break",
    "case",
    "chan",
    "const",
    "continue",
    "default",
    "defer",
    "else",
    "fallthrough",
    "for",
    "func",
    "go",
    "goto",
    "if",
    "import",
    "interface",
    "map",
    "package",
    "range",
    "return",
    "select",
    "struct",
    "switch",
    "type",
    "var",
];

/// A Go parameter name: keywords get a trailing `_`.
fn local(name: &str) -> String {
    if GO_KEYWORDS.contains(&name) {
        format!("{name}_")
    } else {
        name.to_string()
    }
}

const HEADER: &str = r#"// Generated by axiom. Do not edit.
// Source-level proof covers the AMI expression; this Go is checked by differential tests, not proven.

package contracts

// AxiomError: a rule was called outside its `need` lines, or a checked operation failed.
type AxiomError struct{ Message string }

func (e *AxiomError) Error() string { return e.Message }

func catch(err *error) {
	if r := recover(); r != nil {
		e, ok := r.(*AxiomError)
		if !ok {
			panic(r)
		}
		*err = e
	}
}

func w(v uint64) uint64 { return v }

func b2u(b bool) uint64 {
	if b {
		return 1
	}
	return 0
}

// sx reads a word of `bits` bits as a signed number.
func sx(v uint64, bits uint) int64 {
	if bits == 32 {
		return int64(int32(uint32(v)))
	}
	return int64(v)
}

func cut(v uint64, bits uint) uint64 {
	if bits == 32 {
		return v & 0xFFFFFFFF
	}
	return v
}

func div(l, r uint64, bits uint) uint64 {
	if r == 0 {
		panic(&AxiomError{"division by zero"})
	}
	return l / r
}

func rem(l, r uint64, bits uint) uint64 {
	if r == 0 {
		panic(&AxiomError{"division by zero"})
	}
	return l % r
}

func signedCheck(a, b int64, bits uint) {
	if b == 0 || (a == -(int64(1)<<(bits-1)) && b == -1) {
		panic(&AxiomError{"division by zero or overflow"})
	}
}

func sdiv(l, r uint64, bits uint) uint64 {
	a, b := sx(l, bits), sx(r, bits)
	signedCheck(a, b, bits)
	return cut(uint64(a/b), bits)
}

func srem(l, r uint64, bits uint) uint64 {
	a, b := sx(l, bits), sx(r, bits)
	signedCheck(a, b, bits)
	return cut(uint64(a%b), bits)
}

var _ = cut
"#;

/// Returns one Go source file (`package contracts`) exporting every given contract.
pub fn emit_module(items: &[(&Contract, &Expr, Option<&Expr>)]) -> Result<String, String> {
    let mut out = HEADER.to_string();
    let mut names = HashSet::new();
    let mut claim = |name: &str| -> Result<String, String> {
        let go = exported(name);
        if go.is_empty() || !names.insert(go.clone()) {
            return Err(format!(
                "rule name {name} gives a duplicate Go function {go}"
            ));
        }
        Ok(go)
    };
    for (contract, candidate, guard) in items {
        out += "\n";
        out += &emit_function(&claim(&contract.name)?, contract, candidate, *guard)?;
    }
    let contracts: Vec<&Contract> = items.iter().map(|(c, _, _)| *c).collect();
    for contract in &contracts {
        if let Some((base, parts)) = list_parts(contract, &contracts) {
            out += "\n";
            out += &list_wrapper(&claim(base)?, &parts)?;
        }
    }
    for contract in &contracts {
        if let Some(rule) = &contract.list {
            out += "\n";
            out += &any_length_wrapper(&claim(&rule.name)?, contract, rule)?;
        }
        if let Some(record) = &contract.record_answer {
            let go_name = claim(&record.name)?;
            let type_name = claim(&record.record)?;
            let names: Vec<String> = contract.params.iter().map(|p| local(&p.name)).collect();
            let mut params = Vec::new();
            for (name, param) in names.iter().zip(&contract.params) {
                params.push(format!("{name} {}", go_type(&param.ty, contract.wide)?));
            }
            let mut fields = String::new();
            let mut sets = String::new();
            for (field, rule) in &record.fields {
                let field_rule = contracts
                    .iter()
                    .find(|c| &c.name == rule)
                    .ok_or_else(|| format!("missing `{rule}`"))?;
                fields += &format!(
                    "\t{} {}\n",
                    exported(field),
                    go_type(&field_rule.return_type, field_rule.wide)?
                );
                sets += &format!(
                    "\tif result.{}, err = {}({}); err != nil {{\n\t\treturn result, err\n\t}}\n",
                    exported(field),
                    exported(rule),
                    names.join(", ")
                );
            }
            out += &format!(
                "\n// {type_name} is `type {}`, as `{}` answers it.\ntype {type_name} struct {{\n{fields}}}\n\n// {go_name} is the proven rule `{}`: each field from its proven rule.\nfunc {go_name}({}) (result {type_name}, err error) {{\n{sets}\treturn result, nil\n}}\n",
                record.record,
                record.name,
                record.name,
                params.join(", "),
            );
        }
        if let Some(rule) = &contract.repeat {
            let go_name = claim(&rule.name)?;
            let inputs = &contract.params[..contract.params.len() - 1];
            let names: Vec<String> = inputs.iter().map(|p| local(&p.name)).collect();
            let mut params = Vec::new();
            for (name, param) in names.iter().zip(inputs) {
                params.push(format!("{name} {}", go_type(&param.ty, contract.wide)?));
            }
            let ret = go_type(&contract.return_type, contract.wide)?;
            let count_ty = go_type(&inputs[rule.count].ty, contract.wide)?;
            let mut step_args = names.clone();
            if rule.counted {
                step_args[rule.count] = "stepPlace".to_string();
            }
            // The start, then the step `count` times (`proofs/lean/Repeat.lean`).
            out += &format!(
                "\n// {go_name} is the proven rule `{}`: the start, then the step {} times.\nfunc {go_name}({}) (result {ret}, err error) {{\n\tif result, err = {}({}); err != nil {{\n\t\treturn result, err\n\t}}\n\tfor stepPlace := {count_ty}(0); stepPlace < {}; stepPlace++ {{\n\t\tif result, err = {}({}, result); err != nil {{\n\t\t\treturn result, err\n\t\t}}\n\t}}\n\treturn result, nil\n}}\n",
                rule.name,
                names[rule.count],
                params.join(", "),
                exported(&rule.start),
                names.join(", "),
                names[rule.count],
                exported(&contract.name),
                step_args.join(", "),
            );
        }
    }
    let sizes = settle_sizes(&contracts);
    if !sizes.is_empty() {
        out += "\n";
        out += &settle_wrapper(&claim("settle")?, &sizes);
    }
    for (choice, _) in crate::js::choice_types(&contracts)? {
        // Untyped constants fit every number type a rule takes.
        out += &format!(
            "\n// Values of `type {}`: each is its place, from 0.\nconst (\n",
            choice.name
        );
        for (at, value) in choice.values.iter().enumerate() {
            out += &format!("\t{} = {at}\n", claim(&format!("{}_{value}", choice.name))?);
        }
        out += ")\n";
    }
    Ok(out)
}

fn emit_function(
    go_name: &str,
    contract: &Contract,
    candidate: &Expr,
    guard: Option<&Expr>,
) -> Result<String, String> {
    let wide = contract.wide;
    let bits = if wide { 64 } else { 32 };
    let ret = go_type(&contract.return_type, wide)?;
    let mut params = Vec::new();
    let mut lowered = String::new();
    for (index, p) in contract.params.iter().enumerate() {
        let name = local(&p.name);
        let ty = go_type(&p.ty, wide)?;
        params.push(format!("{name} {ty}"));
        let word = match (&p.ty, wide) {
            (Type::Bool, _) => format!("b2u({name})"),
            (Type::I32, false) => format!("uint64(uint32({name}))"),
            _ => format!("uint64({name})"),
        };
        lowered += &format!("\ta{index} := {word}\n\t_ = a{index}\n");
    }
    let zero = if contract.return_type == Type::Bool {
        "false"
    } else {
        "0"
    };
    let guard = guard
        .map(|g| {
            format!(
                "\tif ({}) == 0 {{\n\t\treturn {zero}, &AxiomError{{\"{}: input violates a requires clause\"}}\n\t}}\n",
                expr(g, bits),
                contract.name
            )
        })
        .unwrap_or_default();
    let result = expr(candidate, bits);
    let result = match (&contract.return_type, wide) {
        (Type::Bool, _) => format!("({result}) != 0"),
        (Type::I32, false) => format!("int32(uint32({result}))"),
        _ => format!("{ret}({result})"),
    };
    Ok(format!(
        "// {go_name} is the proven rule `{}`.\nfunc {go_name}({}) (result {ret}, err error) {{\n\tdefer catch(&err)\n{lowered}{guard}\treturn {result}, nil\n}}\n",
        contract.name,
        params.join(", "),
    ))
}

fn list_wrapper(go_name: &str, parts: &[&Contract]) -> Result<String, String> {
    let first = parts[0];
    let mut params = Vec::new();
    let mut args = Vec::new();
    for (name, size, ty) in param_groups(first) {
        let name = local(&name);
        let ty = go_type(&ty, first.wide)?;
        if size > 0 {
            params.push(format!("{name} [{size}]{ty}"));
            args.extend((0..size).map(|k| format!("{name}[{k}]")));
        } else {
            params.push(format!("{name} {ty}"));
            args.push(name);
        }
    }
    let ty = go_type(&first.return_type, first.wide)?;
    let n = parts.len();
    let mut body = String::new();
    for (k, part) in parts.iter().enumerate() {
        body += &format!(
            "\tif out[{k}], err = {}({}); err != nil {{\n\t\treturn out, err\n\t}}\n",
            exported(&part.name),
            args.join(", ")
        );
    }
    Ok(format!(
        "func {go_name}({}) (out [{n}]{ty}, err error) {{\n{body}\treturn out, nil\n}}\n",
        params.join(", ")
    ))
}

/// `Cart(prices)` for a rule over lists of any length: checks the length,
/// adds each item rule over the items in a loop, then calls the core. The
/// sums cannot overflow below the length limit (`proofs/lean/Lists.lean`).
fn any_length_wrapper(
    go_name: &str,
    core: &Contract,
    rule: &axiom_parser::ListRule,
) -> Result<String, String> {
    let ty = go_type(&core.params[0].ty, core.wide)?;
    let ret = go_type(&core.return_type, core.wide)?;
    let names: Vec<String> = rule.params.iter().map(|(p, _)| local(p)).collect();
    let first = rule.params.iter().position(|(_, list)| *list).unwrap_or(0);
    let mut params = Vec::new();
    let mut checks = String::new();
    for (i, (_, list)) in rule.params.iter().enumerate() {
        if *list {
            params.push(format!("{} []{ty}", names[i]));
            if i != first {
                checks += &format!(
                    "\tif len({}) != len({}) {{\n\t\treturn result, &AxiomError{{\"{}: lists must have the same length\"}}\n\t}}\n",
                    names[i], names[first], rule.name
                );
            }
        } else {
            params.push(format!("{} {ty}", names[i]));
        }
    }
    let item_args: Vec<String> = rule
        .params
        .iter()
        .zip(&names)
        .map(|((_, list), name)| {
            if *list {
                format!("{name}[_i]")
            } else {
                name.clone()
            }
        })
        .collect();
    let mut sums = String::new();
    let (mut body, mut second_loop) = (String::new(), String::new());
    for (k, (item, fold)) in rule.items.iter().zip(&rule.folds).enumerate() {
        sums += &format!("\tvar sum{} {ty}\n", k + 1);
        // An item taking list totals (`sum(w / sum(w))`) runs in a second loop.
        let mut args = item_args.clone();
        args.extend(rule.uses[k].iter().map(|j| format!("sum{}", j + 1)));
        let step = match fold {
            axiom_parser::Fold::Sum => format!("sum{} += v", k + 1),
            axiom_parser::Fold::Min | axiom_parser::Fold::Max => format!(
                "if _i == 0 || v {} sum{1} {{\n\t\t\t\tsum{1} = v\n\t\t\t}}",
                if *fold == axiom_parser::Fold::Min {
                    "<"
                } else {
                    ">"
                },
                k + 1
            ),
        };
        let target = if rule.uses[k].is_empty() {
            &mut body
        } else {
            &mut second_loop
        };
        *target += &format!(
            "\t\tif v, err := {}({}); err != nil {{\n\t\t\treturn result, err\n\t\t}} else {{\n\t\t\t{step}\n\t\t}}\n",
            exported(item),
            args.join(", "),
        );
    }
    let mut core_args: Vec<String> = if rule.each {
        item_args.clone()
    } else {
        rule.params
            .iter()
            .zip(&names)
            .filter(|((_, list), _)| !*list)
            .map(|(_, name)| name.clone())
            .collect()
    };
    core_args.push(format!("{ty}(n)"));
    if rule.position {
        core_args.push(format!("{ty}(_i + 1)"));
    }
    if !second_loop.is_empty() {
        body += &format!("\t}}\n\tfor _i := 0; _i < n; _i++ {{\n{second_loop}");
    }
    core_args.extend((1..=rule.items.len()).map(|k| format!("sum{k}")));
    let call = format!("{}({})", exported(&core.name), core_args.join(", "));
    let (ret, tail) = if rule.each {
        (
            format!("[]{ret}"),
            format!(
                "\tresult = make([]{ret}, n)\n\tfor _i := 0; _i < n; _i++ {{\n\t\tif result[_i], err = {call}; err != nil {{\n\t\t\treturn nil, err\n\t\t}}\n\t}}\n\treturn result, nil\n"
            ),
        )
    } else {
        (ret.to_string(), format!("\treturn {call}\n"))
    };
    Ok(format!(
        "// {go_name} is the proven rule `{}` over lists of any length.\nfunc {go_name}({}) (result {ret}, err error) {{\n{checks}\tn := len({})\n\tif {}n > {} {{\n\t\treturn result, &AxiomError{{\"{}: input violates a requires clause\"}}\n\t}}\n{sums}\tfor _i := 0; _i < n; _i++ {{\n{body}\t}}\n{tail}}}\n",
        rule.name,
        params.join(", "),
        names[first],
        crate::rs::too_few(rule, "n", "||"),
        rule.max,
        rule.name,
    ))
}

/// `Settle(owe)`: N from the slice length, N * N transfers, item (i-1)*N+j
/// from person i to person j.
fn settle_wrapper(go_name: &str, sizes: &[usize]) -> String {
    let mut cases = String::new();
    for n in sizes {
        let args: Vec<String> = (0..*n).map(|k| format!("owe[{k}]")).collect();
        let mut calls = String::new();
        for i in 1..=*n {
            for j in 1..=*n {
                calls += &format!(
                    "\t\tif v, err := Settle_{n}_{i}_{j}({}); err != nil {{\n\t\t\treturn nil, err\n\t\t}} else {{\n\t\t\tout = append(out, v)\n\t\t}}\n",
                    args.join(", ")
                );
            }
        }
        cases += &format!("\tcase {n}:\n{calls}");
    }
    format!(
        "func {go_name}(owe []int32) (out []int32, err error) {{\n\tswitch len(owe) {{\n{cases}\tdefault:\n\t\treturn nil, &AxiomError{{\"settle: unsupported number of people\"}}\n\t}}\n\treturn out, nil\n}}\n"
    )
}
