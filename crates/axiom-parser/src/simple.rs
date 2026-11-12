//! Simple syntax: a small, line-based form that lowers to contract syntax.
//!
//! ```text
//! fit_price(price, min, max)
//!   need min <= max
//!
//!   price < min -> min
//!   price > max -> max
//!   -> price
//! ```
//!
//! Each source line maps to the same output line so parser errors keep their
//! line numbers. Arithmetic must provably stay within `0..=u32::MAX` using the
//! `need` bounds; see [`check_ranges`].

use crate::ast::{BinaryOp, Contract, SpecExpr};
use crate::lexer::{Lexer, Token};

/// Returns true when the first meaningful line looks like `name(...)`.
pub fn is_simple(source: &str) -> bool {
    let Some(line) = source
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.starts_with('#'))
    else {
        return false;
    };
    let Some((name, _)) = line.split_once('(') else {
        return false;
    };
    let name = name.trim();
    !name.is_empty()
        && name.chars().all(|c| c.is_alphanumeric() || c == '_')
        && !name.chars().next().is_some_and(|c| c.is_ascii_digit())
        && !line.contains('{')
}

/// Translates simple syntax into contract syntax, line for line.
pub fn translate(source: &str) -> Result<String, String> {
    let lines: Vec<&str> = source.lines().collect();
    let mut out: Vec<String> = vec![String::new(); lines.len()];
    let mut current: Option<Block> = None;

    for (index, raw) in lines.iter().enumerate() {
        let line_no = index + 1;
        let text = strip_comment(raw);
        let trimmed = text.trim();
        if trimmed.is_empty() {
            continue;
        }
        let indented = text.starts_with(' ') || text.starts_with('\t');
        if !indented {
            if let Some(block) = current.take() {
                block.finish(&mut out)?;
            }
            out[index] = header(trimmed, line_no)?;
            current = Some(Block {
                name_line: line_no,
                rules: Vec::new(),
            });
            continue;
        }
        let block = current
            .as_mut()
            .ok_or_else(|| format!("line {line_no}: expected a name like `price(a, b)` first"))?;
        if let Some(condition) = trimmed.strip_prefix("need ") {
            if !block.rules.is_empty() {
                return Err(format!(
                    "line {line_no}: put every `need` line before the `->` lines"
                ));
            }
            out[index] = format!("    requires {};", expression(condition, line_no)?);
        } else if let Some((condition, value)) = trimmed.split_once("->") {
            let condition = condition.trim();
            let condition = if condition.is_empty() {
                None
            } else {
                Some(expression(condition, line_no)?)
            };
            block.rules.push(Rule {
                index,
                line_no,
                condition,
                value: expression(value.trim(), line_no)?,
            });
        } else {
            return Err(format!(
                "line {line_no}: expected `need ...`, `condition -> value`, or `-> value`"
            ));
        }
    }

    match current {
        Some(block) => block.finish(&mut out)?,
        None => return Err("expected a name like `price(a, b)`".to_string()),
    }
    Ok(out.join("\n"))
}

/// Proves that `+`, `-`, and `*` cannot leave `0..=u32::MAX` given the
/// `need` bounds. Bounds come from `need name <= N` style lines.
pub fn check_ranges(contract: &Contract) -> Result<(), String> {
    let mut bounds: Vec<(String, u64, u64)> = contract
        .params
        .iter()
        .map(|param| (param.name.clone(), 0, u64::from(u32::MAX)))
        .collect();
    for requirement in &contract.requires {
        collect_bounds(requirement, &mut bounds);
    }
    for ensures in &contract.ensures {
        range(ensures, &bounds, &contract.name)?;
    }
    Ok(())
}

struct Block {
    name_line: usize,
    rules: Vec<Rule>,
}

struct Rule {
    index: usize,
    line_no: usize,
    condition: Option<String>,
    value: String,
}

impl Block {
    fn finish(self, out: &mut [String]) -> Result<(), String> {
        let Some(last) = self.rules.last() else {
            return Err(format!(
                "line {}: add at least one `-> value` line",
                self.name_line
            ));
        };
        if last.condition.is_some() {
            return Err(format!(
                "line {}: the last line must be `-> value` (used when no rule matches)",
                last.line_no
            ));
        }
        if let Some(rule) = self.rules[..self.rules.len() - 1]
            .iter()
            .find(|rule| rule.condition.is_none())
        {
            return Err(format!(
                "line {}: `-> value` without a condition must be the last line",
                rule.line_no
            ));
        }
        let mut chain = last.value.clone();
        for rule in self.rules[..self.rules.len() - 1].iter().rev() {
            let condition = rule.condition.as_deref().unwrap_or_default();
            chain = format!("if {condition} {{ {} }} else {{ {chain} }}", rule.value);
        }
        out[last.index] = format!("    ensures result == ({chain});\n}}");
        Ok(())
    }
}

fn strip_comment(line: &str) -> &str {
    line.split_once('#').map_or(line, |(code, _)| code)
}

fn header(line: &str, line_no: usize) -> Result<String, String> {
    let (name, rest) = line
        .split_once('(')
        .ok_or_else(|| format!("line {line_no}: expected a name like `price(a, b)`"))?;
    let params = rest
        .trim()
        .strip_suffix(')')
        .ok_or_else(|| format!("line {line_no}: expected `)` at the end of the name line"))?;
    let name = name.trim();
    check_name(name, line_no)?;
    let mut typed = Vec::new();
    for param in params.split(',').map(str::trim).filter(|p| !p.is_empty()) {
        check_name(param, line_no)?;
        typed.push(format!("{param}: u32"));
    }
    Ok(format!(
        "export contract {name}({}) -> u32 {{",
        typed.join(", ")
    ))
}

fn check_name(name: &str, line_no: usize) -> Result<(), String> {
    match Lexer::new(name).next_token() {
        Ok(Token::Ident(ident)) if ident == name => Ok(()),
        Ok(Token::Ident(_)) | Err(_) => {
            Err(format!("line {line_no}: `{name}` is not a valid name"))
        }
        Ok(_) => Err(format!(
            "line {line_no}: `{name}` is a reserved word; choose another name"
        )),
    }
}

/// Rewrites simple operators to contract operators.
fn expression(text: &str, line_no: usize) -> Result<String, String> {
    if text.is_empty() {
        return Err(format!("line {line_no}: missing value"));
    }
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::new();
    let mut index = 0;
    while index < chars.len() {
        let c = chars[index];
        let next = chars.get(index + 1).copied();
        match c {
            '+' | '-' | '*' => {
                out.push(c);
                out.push('%');
            }
            '<' | '>' | '!' if next == Some('=') => {
                out.push(c);
                out.push('=');
                index += 1;
            }
            '=' if next == Some('=') => {
                return Err(format!("line {line_no}: use `=` instead of `==`"));
            }
            '=' => out.push_str("=="),
            '{' | '}' | ';' | '%' | '&' | '|' | '^' => {
                return Err(format!("line {line_no}: `{c}` is not allowed here"));
            }
            c if c.is_alphabetic() || c == '_' => {
                let start = index;
                while index + 1 < chars.len()
                    && (chars[index + 1].is_alphanumeric() || chars[index + 1] == '_')
                {
                    index += 1;
                }
                let word: String = chars[start..=index].iter().collect();
                match word.as_str() {
                    "and" => out.push_str("&&"),
                    "or" => out.push_str("||"),
                    _ => out.push_str(&word),
                }
            }
            _ => out.push(c),
        }
        index += 1;
    }
    Ok(out)
}

fn collect_bounds(expr: &SpecExpr, bounds: &mut [(String, u64, u64)]) {
    let SpecExpr::Binary(op, left, right) = expr else {
        return;
    };
    if *op == BinaryOp::And {
        collect_bounds(left, bounds);
        collect_bounds(right, bounds);
        return;
    }
    let (name, number, op) = match (left.as_ref(), right.as_ref()) {
        (SpecExpr::Ident(name), SpecExpr::Number(n)) => (name, u64::from(*n), op.clone()),
        (SpecExpr::Number(n), SpecExpr::Ident(name)) => (name, u64::from(*n), flip(op.clone())),
        _ => return,
    };
    let Some(entry) = bounds.iter_mut().find(|(param, _, _)| param == name) else {
        return;
    };
    match op {
        BinaryOp::Le => entry.2 = entry.2.min(number),
        BinaryOp::Lt => entry.2 = entry.2.min(number.saturating_sub(1)),
        BinaryOp::Ge => entry.1 = entry.1.max(number),
        BinaryOp::Gt => entry.1 = entry.1.max(number + 1),
        BinaryOp::Eq => {
            entry.1 = entry.1.max(number);
            entry.2 = entry.2.min(number);
        }
        _ => {}
    }
}

fn flip(op: BinaryOp) -> BinaryOp {
    match op {
        BinaryOp::Lt => BinaryOp::Gt,
        BinaryOp::Le => BinaryOp::Ge,
        BinaryOp::Gt => BinaryOp::Lt,
        BinaryOp::Ge => BinaryOp::Le,
        other => other,
    }
}

fn range(expr: &SpecExpr, bounds: &[(String, u64, u64)], name: &str) -> Result<(u64, u64), String> {
    const MAX: u64 = u32::MAX as u64;
    Ok(match expr {
        SpecExpr::Number(n) => (u64::from(*n), u64::from(*n)),
        SpecExpr::Ident(ident) => bounds
            .iter()
            .find(|(param, _, _)| param == ident)
            .map_or((0, MAX), |(_, low, high)| (*low, *high)),
        SpecExpr::If(condition, then, otherwise) => {
            range(condition, bounds, name)?;
            let (a_low, a_high) = range(then, bounds, name)?;
            let (b_low, b_high) = range(otherwise, bounds, name)?;
            (a_low.min(b_low), a_high.max(b_high))
        }
        SpecExpr::Binary(op, left, right) => {
            let (l_low, l_high) = range(left, bounds, name)?;
            let (r_low, r_high) = range(right, bounds, name)?;
            match op {
                BinaryOp::AddWrap => {
                    let high = l_high + r_high;
                    if high > MAX {
                        return Err(too_big(name, "+"));
                    }
                    (l_low + r_low, high)
                }
                BinaryOp::MulWrap => {
                    let high = l_high.saturating_mul(r_high);
                    if high > MAX {
                        return Err(too_big(name, "*"));
                    }
                    (l_low * r_low, high)
                }
                BinaryOp::SubWrap => {
                    if l_low < r_high {
                        return Err(format!(
                            "{name}: `-` may go below 0; add `need` lines that keep the left side at least as large as the right side"
                        ));
                    }
                    (l_low - r_high, l_high - r_low)
                }
                _ => (0, 1),
            }
        }
        _ => (0, MAX),
    })
}

fn too_big(name: &str, op: &str) -> String {
    format!(
        "{name}: `{op}` may exceed {}; add `need name <= limit` lines for its inputs",
        u32::MAX
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIT: &str = "# note\nfit_price(price, min, max)\n  need min <= max\n\n  price < min -> min\n  price > max -> max\n  -> price\n";

    #[test]
    fn detects_simple_syntax() {
        assert!(is_simple(FIT));
        assert!(!is_simple(
            "contract f(x: u32) -> u32 {\n ensures result == x;\n}"
        ));
        assert!(!is_simple("import { f } from \"./a.ax\";"));
    }

    #[test]
    fn translates_rules_into_one_ensures_line_for_line() {
        let out = translate(FIT).unwrap();
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(
            lines[1],
            "export contract fit_price(price: u32, min: u32, max: u32) -> u32 {"
        );
        assert_eq!(lines[2], "    requires min <= max;");
        assert!(lines[6].contains(
            "ensures result == (if price < min { min } else { if price > max { max } else { price } });"
        ));
        assert_eq!(lines[7], "}");
        crate::parse_module(FIT).unwrap();
    }

    #[test]
    fn rejects_missing_default_and_misplaced_lines() {
        assert!(
            translate("f(x)\n  x > 1 -> 1\n")
                .unwrap_err()
                .contains("last line must be")
        );
        assert!(
            translate("f(x)\n  -> 1\n  -> 2\n")
                .unwrap_err()
                .contains("must be the last line")
        );
        assert!(
            translate("f(x)\n  -> x\n  need x <= 1\n")
                .unwrap_err()
                .contains("before")
        );
        assert!(
            translate("f(x)\n  x == 1 -> 1\n  -> 0\n")
                .unwrap_err()
                .contains("use `=`")
        );
        assert!(
            translate("f(input)\n  -> 1\n")
                .unwrap_err()
                .contains("reserved")
        );
    }

    #[test]
    fn checks_arithmetic_ranges_from_needs() {
        assert!(
            crate::parse_module("f(a, b)\n  -> a * b\n")
                .unwrap_err()
                .contains("may exceed")
        );
        crate::parse_module("f(a, b)\n  need a <= 10000\n  need b <= 100\n  -> a * b\n").unwrap();
        assert!(
            crate::parse_module("f(a, b)\n  -> a - b\n")
                .unwrap_err()
                .contains("below 0")
        );
        crate::parse_module("f(a, b)\n  need a >= 10\n  need b <= 10\n  -> a - b\n").unwrap();
    }
}
