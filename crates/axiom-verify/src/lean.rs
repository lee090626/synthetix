//! Lean export of a rule's proof (issue #211).
//!
//! `theorem` states, over the same machine words the Z3 proof uses, that
//! whenever the inputs meet the `need` lines, the found answer is defined and
//! keeps every promise. Lean's `bv_decide` closes it: the SAT solver's answer
//! is checked by the Lean kernel, so this proof does not trust Z3 or the Rust
//! verifier, only this translation and Lean.
//!
//! Covered: whole numbers (`u32`, `i32`, `bool`, `with big numbers`),
//! arithmetic, comparisons, `let` and `if`. Anything else (lists, arrays,
//! loops, calls, records) returns [`LeanExport::NotCovered`] with the reason.

use std::collections::HashMap;
use std::fmt::Write as _;

use axiom_ami::Expr;
use axiom_parser::{BinaryOp, Contract, SpecExpr, Type};

/// The outcome of [`export`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LeanExport {
    /// A Lean file whose theorem holds exactly when the rule is proven.
    Source(String),
    /// The rule uses something this export does not translate yet.
    NotCovered(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Unsigned,
    Signed,
    Bool,
}

#[derive(Clone)]
struct Term {
    text: String,
    kind: Kind,
}

struct Ctx<'a> {
    width: u32,
    contract: &'a Contract,
    result: Term,
}

/// Writes the Lean theorem for `contract` answered by `candidate`.
pub fn export(contract: &Contract, candidate: &Expr) -> LeanExport {
    match build(contract, candidate, None) {
        Ok(source) => LeanExport::Source(source),
        Err(reason) => LeanExport::NotCovered(reason),
    }
}

/// Lemmas a `hint` line may name (issue #215): each is proven here, by Lean,
/// in every file that uses it, and states its fact as a `Bool` that holds for
/// every input so `bv_decide` can use it. `(name, arguments, Lean source)`.
pub const LEMMAS: &[(&str, usize, &str)] = &[(
    "mul_div_le",
    3,
    "/-- `x * y / z <= x` when `y <= z` and `x * y` does not wrap. -/
theorem mul_div_le {w : Nat} (x y z : BitVec w) :
    (!(BitVec.ule y z) || x.umulOverflow y || BitVec.ule ((x * y).udiv z) x) = true := by
  rcases Nat.eq_zero_or_pos z.toNat with hz | hz
  · have : z = 0#w := by apply BitVec.eq_of_toNat_eq; simp [hz]
    subst this; simp [BitVec.ule]
  · by_cases hyz : BitVec.ule y z <;> by_cases ho : x.umulOverflow y <;> simp [hyz, ho]
    simp only [BitVec.ule, decide_eq_true_eq] at hyz ⊢
    simp only [BitVec.umulOverflow, decide_eq_true_eq, Nat.not_le] at ho
    rw [BitVec.toNat_udiv, BitVec.toNat_mul, Nat.mod_eq_of_lt ho]
    apply Nat.div_le_of_le_mul
    rw [Nat.mul_comm z.toNat]
    exact Nat.mul_le_mul_left _ hyz
",
)];

/// `bv_decide` with a SAT time limit well above the default 10 seconds, so a
/// proof does not pass on a fast machine and time out on a slower one.
const BV_DECIDE: &str = "bv_decide (config := { timeout := 120 })";

/// The most cases [`export_split`] proves one by one.
pub const MAX_SPLIT_CASES: u64 = 64;

/// Like [`export`], but proves the promises once per value of the input with
/// the fewest values allowed by the `need` lines (at most
/// [`MAX_SPLIT_CASES`]). Each case then divides and multiplies by a fixed
/// number, which `bv_decide` solves where a variable divisor times out.
/// `None` when no input has such a small range.
pub fn export_split(contract: &Contract, candidate: &Expr) -> Option<LeanExport> {
    let (name, low, high) = smallest_range(contract)?;
    Some(match build(contract, candidate, Some((&name, low, high))) {
        Ok(source) => LeanExport::Source(source),
        Err(reason) => LeanExport::NotCovered(reason),
    })
}

/// The unsigned input with the fewest values, from `need` lines comparing it
/// with a number (`x <= 50`, `x >= 1`, also inside `and`).
fn smallest_range(contract: &Contract) -> Option<(String, u64, u64)> {
    fn conjuncts<'a>(expr: &'a SpecExpr, out: &mut Vec<&'a SpecExpr>) {
        match expr {
            SpecExpr::Binary(BinaryOp::And, lhs, rhs) => {
                conjuncts(lhs, out);
                conjuncts(rhs, out);
            }
            other => out.push(other),
        }
    }
    let mut facts = Vec::new();
    for clause in &contract.requires {
        conjuncts(clause, &mut facts);
    }
    let mut best: Option<(String, u64, u64)> = None;
    for param in contract.params.iter().filter(|param| param.ty == Type::U32) {
        let (mut low, mut high) = (0_u64, None::<u64>);
        for fact in &facts {
            let SpecExpr::Binary(op, lhs, rhs) = fact else {
                continue;
            };
            let (op, value) = match (lhs.as_ref(), rhs.as_ref()) {
                (SpecExpr::Ident(n), SpecExpr::Number(v)) if *n == param.name => (op.clone(), *v),
                (SpecExpr::Number(v), SpecExpr::Ident(n)) if *n == param.name => (
                    match op {
                        BinaryOp::Le => BinaryOp::Ge,
                        BinaryOp::Lt => BinaryOp::Gt,
                        BinaryOp::Ge => BinaryOp::Le,
                        BinaryOp::Gt => BinaryOp::Lt,
                        other => other.clone(),
                    },
                    *v,
                ),
                _ => continue,
            };
            match op {
                BinaryOp::Le => high = Some(high.map_or(value, |h| h.min(value))),
                BinaryOp::Lt if value > 0 => {
                    high = Some(high.map_or(value - 1, |h| h.min(value - 1)))
                }
                BinaryOp::Ge => low = low.max(value),
                BinaryOp::Gt => low = low.max(value + 1),
                _ => {}
            }
        }
        let Some(high) = high else { continue };
        if high < low || high - low >= MAX_SPLIT_CASES {
            continue;
        }
        if best.as_ref().is_none_or(|(_, l, h)| high - low < h - l) {
            best = Some((param.name.clone(), low, high));
        }
    }
    best
}

fn build(
    contract: &Contract,
    candidate: &Expr,
    split: Option<(&str, u64, u64)>,
) -> Result<String, String> {
    if contract.ensures.is_empty() {
        return Err("a rule without promises".to_string());
    }
    let width = if contract.wide { 64 } else { 32 };
    let mut names = Vec::new();
    let mut bool_params = Vec::new();
    for param in &contract.params {
        match param.ty {
            Type::U32 | Type::I32 => {}
            Type::Bool => bool_params.push(format!("BitVec.ule {} 1#{width}", ident(&param.name))),
            _ => return Err(format!("input `{}` of type {:?}", param.name, param.ty)),
        }
        names.push(ident(&param.name));
    }
    let answer = candidate_value(candidate, &names, width)?;
    let answer_defined = candidate_defined(candidate, &names, width)?;
    let result = match contract.return_type {
        Type::U32 => Term {
            text: answer,
            kind: Kind::Unsigned,
        },
        Type::I32 => Term {
            text: answer,
            kind: Kind::Signed,
        },
        Type::Bool => Term {
            text: format!("({answer} != 0#{width})"),
            kind: Kind::Bool,
        },
        ref other => return Err(format!("answers of type {other:?}")),
    };
    let ctx = Ctx {
        width,
        contract,
        result,
    };
    let empty = HashMap::new();
    let mut requires = Vec::new();
    let mut requires_defined = Vec::new();
    for clause in &contract.requires {
        requires.push(ctx.boolean(clause, &empty)?);
        requires_defined.push(ctx.defined(clause, &empty)?);
    }
    let mut ensures = Vec::new();
    let mut ensures_defined = Vec::new();
    for clause in &contract.ensures {
        ensures.push(ctx.boolean(clause, &empty)?);
        ensures_defined.push(ctx.defined(clause, &empty)?);
    }

    let inputs = all(&bool_params);
    let need = all(&requires);
    let goal = all(&[answer_defined, all(&ensures_defined), all(&ensures)]);
    let mut out = String::new();
    let _ = writeln!(
        out,
        "-- Generated by `axiom build --lean` from rule `{}`. Do not edit.",
        contract.name
    );
    let _ = writeln!(out, "import Std.Tactic.BVDecide\n");
    let _ = writeln!(out, "set_option linter.unusedVariables false");
    let _ = writeln!(out, "set_option maxHeartbeats 0\n");
    let binders = if names.is_empty() {
        String::new()
    } else {
        format!(" ({} : BitVec {width})", names.join(" "))
    };
    let _ = writeln!(
        out,
        "/-- The `need` lines are defined for every input. -/\ntheorem {}_needs_defined{binders} :\n    (!{inputs} || {}) = true := by\n  {BV_DECIDE}\n",
        ident(&contract.name),
        all(&requires_defined),
    );
    // `hint` lines: each lemma's fact, then its division as a fresh word, so
    // `bv_decide` reasons from the fact instead of the division's circuit.
    let mut lemmas = String::new();
    let mut steps = String::new();
    for (k, hint) in contract.hints.iter().enumerate() {
        let Some((name, arity, text)) = LEMMAS.iter().find(|(name, ..)| *name == hint.lemma) else {
            return Err(format!("hint `{}`: no such lemma", hint.lemma));
        };
        if hint.args.len() != *arity {
            return Err(format!("hint `{name}` takes {arity} values"));
        }
        let args = hint
            .args
            .iter()
            .map(|arg| ctx.value(arg, &empty).map(|term| term.text))
            .collect::<Result<Vec<_>, _>>()?;
        if !lemmas.contains(text) {
            lemmas += text;
            lemmas += "\n";
        }
        let _ = writeln!(steps, "  have h{k} := {name} {}", args.join(" "));
        let _ = writeln!(
            steps,
            "  generalize BitVec.udiv ({} * {}) {} = q{k} at *",
            args[0], args[1], args[2]
        );
    }
    let hypothesis = format!("({inputs} && {need})");
    let proof = match split {
        None => format!("{steps}  {BV_DECIDE}"),
        Some((name, low, high)) => {
            // Either the `need` lines fail, or the input is one of its values.
            let cases = (low..=high)
                .map(|value| format!("{} = {value}#{width}", ident(name)))
                .collect::<Vec<_>>();
            let names = vec!["h"; cases.len() + 1].join(" | ");
            format!(
                "{steps}  rcases (show {} ∨ {hypothesis} = false by {BV_DECIDE}) with {names}\n  all_goals first | (subst h; {BV_DECIDE}) | simp [h]",
                cases.join(" ∨ ")
            )
        }
    };
    out += &lemmas;
    let _ = writeln!(
        out,
        "/-- When the `need` lines hold, the answer is defined and keeps every promise. -/\ntheorem {}_proved{binders} :\n    (!{hypothesis} || {goal}) = true := by\n{proof}",
        ident(&contract.name),
    );
    Ok(out)
}

fn all(parts: &[String]) -> String {
    match parts {
        [] => "true".to_string(),
        [one] => one.clone(),
        _ => format!("({})", parts.join(" && ")),
    }
}

/// Lean names may not clash with keywords; every input gets a prefix.
fn ident(name: &str) -> String {
    format!("x_{name}")
}

fn literal(value: u64, width: u32) -> Result<String, String> {
    if width == 32 && value > u64::from(u32::MAX) {
        return Err(format!("number {value} does not fit 32 bits"));
    }
    Ok(format!("{value}#{width}"))
}

fn candidate_value(expr: &Expr, names: &[String], width: u32) -> Result<String, String> {
    let go = |e: &Expr| candidate_value(e, names, width);
    let bin = |op: &str, a: &Expr, b: &Expr| -> Result<String, String> {
        Ok(format!("({} {op} {})", go(a)?, go(b)?))
    };
    let call = |f: &str, a: &Expr, b: &Expr| -> Result<String, String> {
        Ok(format!("(BitVec.{f} {} {})", go(a)?, go(b)?))
    };
    match expr {
        Expr::Arg(index) => names
            .get(*index as usize)
            .cloned()
            .ok_or_else(|| format!("answer reads missing input {index}")),
        Expr::Const(value) => literal(*value, width),
        Expr::Add(a, b) => bin("+", a, b),
        Expr::Sub(a, b) => bin("-", a, b),
        Expr::Mul(a, b) => bin("*", a, b),
        Expr::Div(a, b) => call("udiv", a, b),
        Expr::Rem(a, b) => call("umod", a, b),
        Expr::SDiv(a, b) => call("sdiv", a, b),
        Expr::SRem(a, b) => call("srem", a, b),
        Expr::Shl(a, b) => Ok(format!(
            "({} <<< ({} &&& {}#{width}))",
            go(a)?,
            go(b)?,
            width - 1
        )),
        Expr::BitAnd(a, b) => bin("&&&", a, b),
        Expr::BitOr(a, b) => bin("|||", a, b),
        Expr::BitXor(a, b) => bin("^^^", a, b),
        Expr::Eq(..)
        | Expr::Ne(..)
        | Expr::Lt(..)
        | Expr::Le(..)
        | Expr::Gt(..)
        | Expr::Ge(..)
        | Expr::SLt(..)
        | Expr::SLe(..)
        | Expr::SGt(..)
        | Expr::SGe(..)
        | Expr::And(..)
        | Expr::Or(..) => Ok(format!(
            "(bif {} then 1#{width} else 0#{width})",
            candidate_bool(expr, names, width)?
        )),
        Expr::Select {
            condition,
            then_branch,
            else_branch,
        } => Ok(format!(
            "(bif {} then {} else {})",
            candidate_bool(condition, names, width)?,
            go(then_branch)?,
            go(else_branch)?
        )),
        Expr::ArrayGet { elements, index } => {
            let index = go(index)?;
            let mut selected = literal(0, width)?;
            for (position, element) in elements.iter().enumerate().rev() {
                selected = format!(
                    "(bif {index} == {position}#{width} then {} else {selected})",
                    go(element)?
                );
            }
            Ok(selected)
        }
        Expr::Guard { value, .. } => go(value),
    }
}

/// A candidate condition as a Lean `Bool`; matches `expr_to_bool`, where any
/// word counts as true when it is not zero.
fn candidate_bool(expr: &Expr, names: &[String], width: u32) -> Result<String, String> {
    let v = |e: &Expr| candidate_value(e, names, width);
    let b = |e: &Expr| candidate_bool(e, names, width);
    Ok(match expr {
        Expr::Eq(x, y) => format!("({} == {})", v(x)?, v(y)?),
        Expr::Ne(x, y) => format!("({} != {})", v(x)?, v(y)?),
        Expr::Lt(x, y) => format!("(BitVec.ult {} {})", v(x)?, v(y)?),
        Expr::Le(x, y) => format!("(BitVec.ule {} {})", v(x)?, v(y)?),
        Expr::Gt(x, y) => format!("(BitVec.ult {} {})", v(y)?, v(x)?),
        Expr::Ge(x, y) => format!("(BitVec.ule {} {})", v(y)?, v(x)?),
        Expr::SLt(x, y) => format!("(BitVec.slt {} {})", v(x)?, v(y)?),
        Expr::SLe(x, y) => format!("(BitVec.sle {} {})", v(x)?, v(y)?),
        Expr::SGt(x, y) => format!("(BitVec.slt {} {})", v(y)?, v(x)?),
        Expr::SGe(x, y) => format!("(BitVec.sle {} {})", v(y)?, v(x)?),
        Expr::And(x, y) => format!("({} && {})", b(x)?, b(y)?),
        Expr::Or(x, y) => format!("({} || {})", b(x)?, b(y)?),
        other => format!("({} != 0#{width})", v(other)?),
    })
}

/// When the candidate is defined: no zero divisor, no `i32::MIN / -1`, every
/// guard and array index holds. Branches only count on their own path.
fn candidate_defined(expr: &Expr, names: &[String], width: u32) -> Result<String, String> {
    let d = |e: &Expr| candidate_defined(e, names, width);
    let v = |e: &Expr| candidate_value(e, names, width);
    let both = |a: &Expr, b: &Expr| -> Result<Vec<String>, String> { Ok(vec![d(a)?, d(b)?]) };
    let min = format!("(1#{width} <<< {}#{width})", width - 1);
    Ok(match expr {
        Expr::Arg(_) | Expr::Const(_) => "true".to_string(),
        Expr::Div(a, b) | Expr::Rem(a, b) => {
            let mut parts = both(a, b)?;
            parts.push(format!("({} != 0#{width})", v(b)?));
            all(&parts)
        }
        Expr::SDiv(a, b) | Expr::SRem(a, b) => {
            let mut parts = both(a, b)?;
            parts.push(format!("({} != 0#{width})", v(b)?));
            parts.push(format!(
                "!({} == {min} && {} == {})",
                v(a)?,
                v(b)?,
                format_args!("(0#{width} - 1#{width})")
            ));
            all(&parts)
        }
        Expr::Add(a, b)
        | Expr::Sub(a, b)
        | Expr::Mul(a, b)
        | Expr::Shl(a, b)
        | Expr::BitAnd(a, b)
        | Expr::BitOr(a, b)
        | Expr::BitXor(a, b)
        | Expr::Eq(a, b)
        | Expr::Ne(a, b)
        | Expr::Lt(a, b)
        | Expr::Le(a, b)
        | Expr::Gt(a, b)
        | Expr::Ge(a, b)
        | Expr::SLt(a, b)
        | Expr::SLe(a, b)
        | Expr::SGt(a, b)
        | Expr::SGe(a, b)
        | Expr::And(a, b)
        | Expr::Or(a, b) => all(&both(a, b)?),
        Expr::Select {
            condition,
            then_branch,
            else_branch,
        } => format!(
            "({} && (bif {} then {} else {}))",
            d(condition)?,
            candidate_bool(condition, names, width)?,
            d(then_branch)?,
            d(else_branch)?
        ),
        Expr::ArrayGet { elements, index } => {
            let mut parts = vec![d(index)?];
            for element in elements {
                parts.push(d(element)?);
            }
            parts.push(format!(
                "(BitVec.ult {} {}#{width})",
                v(index)?,
                elements.len()
            ));
            all(&parts)
        }
        Expr::Guard { condition, value } => all(&[
            d(condition)?,
            candidate_bool(condition, names, width)?,
            d(value)?,
        ]),
    })
}

impl Ctx<'_> {
    fn boolean(&self, expr: &SpecExpr, locals: &HashMap<String, Term>) -> Result<String, String> {
        let term = self.value(expr, locals)?;
        if term.kind != Kind::Bool {
            return Err("a `need` line or promise that is not yes/no".to_string());
        }
        Ok(term.text)
    }

    fn value(&self, expr: &SpecExpr, locals: &HashMap<String, Term>) -> Result<Term, String> {
        let w = self.width;
        match expr {
            SpecExpr::Ident(name) if name == "result" => Ok(self.result.clone()),
            SpecExpr::Ident(name) if locals.contains_key(name) => Ok(locals[name].clone()),
            SpecExpr::Ident(name) => {
                let param = self
                    .contract
                    .params
                    .iter()
                    .find(|param| &param.name == name)
                    .ok_or_else(|| format!("unknown name `{name}`"))?;
                Ok(match param.ty {
                    Type::I32 => Term {
                        text: ident(name),
                        kind: Kind::Signed,
                    },
                    Type::Bool => Term {
                        text: format!("({} != 0#{w})", ident(name)),
                        kind: Kind::Bool,
                    },
                    _ => Term {
                        text: ident(name),
                        kind: Kind::Unsigned,
                    },
                })
            }
            SpecExpr::Number(value) => Ok(Term {
                text: literal(*value, w)?,
                kind: Kind::Unsigned,
            }),
            SpecExpr::Bool(value) => Ok(Term {
                text: value.to_string(),
                kind: Kind::Bool,
            }),
            SpecExpr::Let(name, value, body) => {
                let value = self.value(value, locals)?;
                let mut nested = locals.clone();
                nested.insert(name.clone(), value);
                self.value(body, &nested)
            }
            SpecExpr::If(condition, then_branch, else_branch) => {
                let condition = self.boolean(condition, locals)?;
                let then_branch = self.value(then_branch, locals)?;
                let else_branch = self.value(else_branch, locals)?;
                let kind = match (then_branch.kind, else_branch.kind) {
                    (Kind::Bool, Kind::Bool) => Kind::Bool,
                    (Kind::Unsigned, Kind::Unsigned) => Kind::Unsigned,
                    (Kind::Bool, _) | (_, Kind::Bool) => {
                        return Err("if branches must have the same type".to_string());
                    }
                    _ => Kind::Signed,
                };
                Ok(Term {
                    text: format!(
                        "(bif {condition} then {} else {})",
                        then_branch.text, else_branch.text
                    ),
                    kind,
                })
            }
            SpecExpr::Binary(op, lhs, rhs) => {
                let lhs = self.value(lhs, locals)?;
                let rhs = self.value(rhs, locals)?;
                binary(op, &lhs, &rhs, w)
            }
            SpecExpr::Text(_) => Err("text values".to_string()),
            SpecExpr::List(_) => Err("list values".to_string()),
            SpecExpr::Record { .. } | SpecExpr::Field(..) => Err("records".to_string()),
            SpecExpr::Index(..) | SpecExpr::ArrayUpdate(..) => Err("arrays".to_string()),
            SpecExpr::For { .. } => Err("loops".to_string()),
            SpecExpr::Call(name, _) => Err(format!("calls (`{name}`)")),
        }
    }

    /// Mirrors `spec_definedness`: every part is defined, `-!` stays at or
    /// above zero, divisors are not zero, and an `if` only needs its taken
    /// branch.
    fn defined(&self, expr: &SpecExpr, locals: &HashMap<String, Term>) -> Result<String, String> {
        let w = self.width;
        match expr {
            SpecExpr::Ident(_) | SpecExpr::Number(_) | SpecExpr::Bool(_) => Ok("true".to_string()),
            SpecExpr::Let(name, value, body) => {
                let defined = self.defined(value, locals)?;
                let value = self.value(value, locals)?;
                let mut nested = locals.clone();
                nested.insert(name.clone(), value);
                Ok(all(&[defined, self.defined(body, &nested)?]))
            }
            SpecExpr::If(condition, then_branch, else_branch) => Ok(format!(
                "({} && (bif {} then {} else {}))",
                self.defined(condition, locals)?,
                self.boolean(condition, locals)?,
                self.defined(then_branch, locals)?,
                self.defined(else_branch, locals)?
            )),
            SpecExpr::Binary(op, lhs_expr, rhs_expr) => {
                let mut parts = vec![
                    self.defined(lhs_expr, locals)?,
                    self.defined(rhs_expr, locals)?,
                ];
                let lhs = self.value(lhs_expr, locals)?;
                let rhs = self.value(rhs_expr, locals)?;
                match op {
                    BinaryOp::SubChecked => {
                        parts.push(format!("(BitVec.ule {} {})", rhs.text, lhs.text));
                    }
                    BinaryOp::Div | BinaryOp::Rem => {
                        parts.push(format!("({} != 0#{w})", rhs.text));
                        if lhs.kind == Kind::Signed || rhs.kind == Kind::Signed {
                            parts.push(format!(
                                "!({} == (1#{w} <<< {}#{w}) && {} == (0#{w} - 1#{w}))",
                                lhs.text,
                                w - 1,
                                rhs.text
                            ));
                        }
                    }
                    _ => {}
                }
                Ok(all(&parts))
            }
            other => self.value(other, locals).map(|_| "true".to_string()),
        }
    }
}

fn binary(op: &BinaryOp, lhs: &Term, rhs: &Term, w: u32) -> Result<Term, String> {
    use Kind::*;
    let (a, b) = (&lhs.text, &rhs.text);
    let word = match (lhs.kind, rhs.kind) {
        (Unsigned, Unsigned) => Some(Unsigned),
        (Signed, Signed) | (Signed, Unsigned) | (Unsigned, Signed) => Some(Signed),
        _ => None,
    };
    let term = |text: String, kind: Kind| Ok(Term { text, kind });
    match (op, word) {
        (BinaryOp::And, None) if lhs.kind == Bool && rhs.kind == Bool => {
            term(format!("({a} && {b})"), Bool)
        }
        (BinaryOp::Or, None) if lhs.kind == Bool && rhs.kind == Bool => {
            term(format!("({a} || {b})"), Bool)
        }
        (BinaryOp::Eq, None) if lhs.kind == Bool && rhs.kind == Bool => {
            term(format!("({a} == {b})"), Bool)
        }
        (BinaryOp::Ne, None) if lhs.kind == Bool && rhs.kind == Bool => {
            term(format!("({a} != {b})"), Bool)
        }
        (_, None) => Err(format!("operator {op:?} has incompatible operands")),
        (BinaryOp::SubChecked, Some(Signed)) => {
            Err(format!("operator {op:?} has incompatible operands"))
        }
        (BinaryOp::AddWrap, Some(k)) => term(format!("({a} + {b})"), k),
        (BinaryOp::SubWrap | BinaryOp::SubChecked, Some(k)) => term(format!("({a} - {b})"), k),
        (BinaryOp::MulWrap, Some(k)) => term(format!("({a} * {b})"), k),
        (BinaryOp::Div, Some(Unsigned)) => term(format!("(BitVec.udiv {a} {b})"), Unsigned),
        (BinaryOp::Rem, Some(Unsigned)) => term(format!("(BitVec.umod {a} {b})"), Unsigned),
        (BinaryOp::Div, Some(_)) => term(format!("(BitVec.sdiv {a} {b})"), Signed),
        (BinaryOp::Rem, Some(_)) => term(format!("(BitVec.srem {a} {b})"), Signed),
        (BinaryOp::Shl, Some(k)) => term(format!("({a} <<< ({b} &&& {}#{w}))", w - 1), k),
        (BinaryOp::BitAnd, Some(k)) => term(format!("({a} &&& {b})"), k),
        (BinaryOp::BitOr, Some(k)) => term(format!("({a} ||| {b})"), k),
        (BinaryOp::BitXor, Some(k)) => term(format!("({a} ^^^ {b})"), k),
        (BinaryOp::Eq, Some(_)) => term(format!("({a} == {b})"), Bool),
        (BinaryOp::Ne, Some(_)) => term(format!("({a} != {b})"), Bool),
        (BinaryOp::Lt, Some(Unsigned)) => term(format!("(BitVec.ult {a} {b})"), Bool),
        (BinaryOp::Le, Some(Unsigned)) => term(format!("(BitVec.ule {a} {b})"), Bool),
        (BinaryOp::Gt, Some(Unsigned)) => term(format!("(BitVec.ult {b} {a})"), Bool),
        (BinaryOp::Ge, Some(Unsigned)) => term(format!("(BitVec.ule {b} {a})"), Bool),
        (BinaryOp::Lt, Some(_)) => term(format!("(BitVec.slt {a} {b})"), Bool),
        (BinaryOp::Le, Some(_)) => term(format!("(BitVec.sle {a} {b})"), Bool),
        (BinaryOp::Gt, Some(_)) => term(format!("(BitVec.slt {b} {a})"), Bool),
        (BinaryOp::Ge, Some(_)) => term(format!("(BitVec.sle {b} {a})"), Bool),
        (BinaryOp::And | BinaryOp::Or, Some(_)) => {
            Err(format!("operator {op:?} has incompatible operands"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axiom_parser::parse;

    fn source(text: &str, candidate: Expr) -> String {
        match export(&parse(text).unwrap(), &candidate) {
            LeanExport::Source(source) => source,
            LeanExport::NotCovered(reason) => panic!("not covered: {reason}"),
        }
    }

    #[test]
    fn exports_a_guarded_division() {
        let lean = source(
            "contract half(x: u32, y: u32) -> u32 { requires y > 0; ensures result == x / y; }",
            Expr::Div(Box::new(Expr::Arg(0)), Box::new(Expr::Arg(1))),
        );
        assert!(lean.contains("theorem x_half_proved (x_x x_y : BitVec 32)"));
        assert!(lean.contains("(BitVec.udiv x_x x_y)"));
        assert!(lean.contains("(x_y != 0#32)"));
        assert!(lean.contains("bv_decide"));
    }

    #[test]
    fn splits_on_the_input_with_the_fewest_values() {
        let contract = parse(
            "contract f(t: u32, p: u32) -> u32 { requires t <= 1000 && p >= 1 && p <= 3; ensures result == t / p; }",
        )
        .unwrap();
        let candidate = Expr::Div(Box::new(Expr::Arg(0)), Box::new(Expr::Arg(1)));
        let Some(LeanExport::Source(lean)) = export_split(&contract, &candidate) else {
            panic!("expected a split proof");
        };
        assert!(
            lean.contains("x_p = 1#32 ∨ x_p = 2#32 ∨ x_p = 3#32 ∨"),
            "{lean}"
        );
        assert!(lean.contains("with h | h | h | h"), "{lean}");

        let wide = parse(
            "contract g(t: u32, p: u32) -> u32 { requires p <= 100; ensures result == t / (p +% 1); }",
        )
        .unwrap();
        assert!(export_split(&wide, &Expr::Arg(0)).is_none());
    }

    #[test]
    fn a_hint_states_its_lemma_and_replaces_the_division() {
        let lean = source(
            "f(x, y, z)\n  need x <= 1000\n  need y <= z\n  need z >= 1 and z <= 1000\n  always -> x * y / z\n  promise answer <= x\n  hint mul_div_le(x, y, z)\n",
            Expr::Div(
                Box::new(Expr::Mul(Box::new(Expr::Arg(0)), Box::new(Expr::Arg(1)))),
                Box::new(Expr::Arg(2)),
            ),
        );
        assert!(lean.contains("theorem mul_div_le"), "{lean}");
        assert!(lean.contains("have h0 := mul_div_le x_x x_y x_z"), "{lean}");
        assert!(
            lean.contains("generalize BitVec.udiv (x_x * x_y) x_z = q0 at *"),
            "{lean}"
        );

        let unknown = parse("f(x)\n  always -> x\n  hint nope(x)\n").unwrap();
        assert_eq!(
            export(&unknown, &Expr::Arg(0)),
            LeanExport::NotCovered("hint `nope`: no such lemma".to_string())
        );
    }

    #[test]
    fn reports_what_it_does_not_cover() {
        let contract =
            parse("contract f(xs: [u32; 2]) -> u32 { ensures result == xs[0]; }").unwrap();
        assert!(matches!(
            export(&contract, &Expr::Arg(0)),
            LeanExport::NotCovered(reason) if reason.starts_with("input `xs`")
        ));
    }
}
