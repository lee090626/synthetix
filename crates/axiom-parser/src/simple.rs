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

/// Returns true when the first meaningful line is a simple-syntax header:
/// `name(...)`, `from file: names`, or `page Name`.
pub fn is_simple(source: &str) -> bool {
    let Some(line) = source
        .lines()
        .map(|line| strip_comment(line).trim())
        .find(|line| !line.is_empty())
    else {
        return false;
    };
    if line.contains('{') || line.ends_with(';') {
        return false;
    }
    if line.starts_with("from ") || line.starts_with("page ") {
        return true;
    }
    let Some((name, _)) = line.split_once('(') else {
        return false;
    };
    let name = name.trim();
    !name.is_empty()
        && name.chars().all(|c| c.is_alphanumeric() || c == '_')
        && !name.chars().next().is_some_and(|c| c.is_ascii_digit())
}

/// Rejects the old contract syntax in user files. Internal tests and legacy
/// fixtures opt back in with `AXIOM_LEGACY_SYNTAX=1`.
pub fn check_user_syntax(source: &str) -> Result<(), String> {
    if is_simple(source) || std::env::var("AXIOM_LEGACY_SYNTAX").as_deref() == Ok("1") {
        return Ok(());
    }
    Err(
        "this file uses the old syntax, which is no longer supported; start with `name(a, b)`, `from file: name`, or `page Name` (see docs/writing-specs.md)"
            .to_string(),
    )
}

/// Appends the source line an error points at (`line N: ...` or `N:C: ...`).
pub fn show_source_line(error: String, source: &str) -> String {
    let number = error
        .strip_prefix("line ")
        .and_then(|rest| rest.split(':').next())
        .or_else(|| error.split(':').next())
        .and_then(|text| text.trim().parse::<usize>().ok());
    match number.and_then(|n| source.lines().nth(n.wrapping_sub(1)).map(|line| (n, line))) {
        Some((n, line)) => format!("{error}\n  {n} | {}", line.trim_end()),
        None => error,
    }
}

enum Current {
    Rule(Block),
    Page { last_index: usize },
}

impl Current {
    fn finish(self, out: &mut [String]) -> Result<(), String> {
        match self {
            Current::Rule(block) => block.finish(out),
            Current::Page { last_index } => {
                out[last_index].push_str("\n}");
                Ok(())
            }
        }
    }
}

/// Translates simple syntax into contract and page syntax, line for line.
pub fn translate(source: &str) -> Result<String, String> {
    let lines: Vec<&str> = source.lines().collect();
    let mut out: Vec<String> = vec![String::new(); lines.len()];
    let mut current: Option<Current> = None;

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
            if let Some(rest) = trimmed.strip_prefix("from ") {
                out[index] = import(rest, line_no)?;
            } else if let Some(name) = trimmed.strip_prefix("page ") {
                let name = name.trim();
                check_name(name, line_no)?;
                out[index] = format!("page {name} {{");
                current = Some(Current::Page { last_index: index });
            } else {
                out[index] = header(trimmed, line_no)?;
                current = Some(Current::Rule(Block {
                    name_line: line_no,
                    rules: Vec::new(),
                }));
            }
            continue;
        }
        match current.as_mut() {
            Some(Current::Rule(block)) => rule_line(block, &mut out, index, trimmed)?,
            Some(Current::Page { last_index }) => {
                out[index] = page_line(trimmed, line_no)?;
                *last_index = index;
            }
            None => {
                return Err(format!(
                    "line {line_no}: expected `name(a, b)` or `page Name` first"
                ));
            }
        }
    }

    match current {
        Some(block) => block.finish(&mut out)?,
        None if out.iter().all(String::is_empty) => {
            return Err("expected `name(a, b)` or `page Name`".to_string());
        }
        None => {}
    }
    Ok(out.join("\n"))
}

fn rule_line(
    block: &mut Block,
    out: &mut [String],
    index: usize,
    trimmed: &str,
) -> Result<(), String> {
    let line_no = index + 1;
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
    Ok(())
}

fn import(rest: &str, line_no: usize) -> Result<String, String> {
    let (file, names) = rest
        .split_once(':')
        .ok_or_else(|| format!("line {line_no}: expected `from file: name, name`"))?;
    let file = file.trim();
    if file.is_empty() || file.contains('"') {
        return Err(format!("line {line_no}: expected a file name after `from`"));
    }
    let names = names_list(names, line_no)?;
    let path = if file.ends_with(".ax") {
        file.to_string()
    } else {
        format!("{file}.ax")
    };
    let path = if path.starts_with("./") || path.starts_with("../") {
        path
    } else {
        format!("./{path}")
    };
    Ok(format!(
        "import {{ {} }} from \"{path}\";",
        names.join(", ")
    ))
}

fn names_list(text: &str, line_no: usize) -> Result<Vec<String>, String> {
    let names: Vec<String> = text
        .split(',')
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_string)
        .collect();
    if names.is_empty() {
        return Err(format!("line {line_no}: expected at least one name"));
    }
    for name in &names {
        check_name(name, line_no)?;
    }
    Ok(names)
}

fn page_line(trimmed: &str, line_no: usize) -> Result<String, String> {
    let (word, rest) = trimmed.split_once(' ').unwrap_or((trimmed, ""));
    let rest = rest.trim();
    match word {
        "save" => states(rest, true, line_no),
        "ask" => {
            let (name, label) = rest
                .split_once(' ')
                .ok_or_else(|| format!("line {line_no}: expected `ask name \"label\"`"))?;
            check_name(name, line_no)?;
            let label = quoted(label.trim(), line_no)?;
            Ok(format!("    input {name} placeholder {label};"))
        }
        "show" if rest.starts_with('"') => Ok(format!("    show {};", quoted(rest, line_no)?)),
        "show" => Ok(format!("    show {};", expression(rest, line_no)?)),
        "button" => {
            let label_end = rest
                .get(1..)
                .and_then(|tail| tail.find('"'))
                .filter(|_| rest.starts_with('"'))
                .ok_or_else(|| {
                    format!("line {line_no}: expected `button \"label\" -> name = value`")
                })?;
            let label = &rest[..label_end + 2];
            let action = rest[label_end + 2..].trim();
            let assignment = action
                .strip_prefix("->")
                .ok_or_else(|| format!("line {line_no}: expected `->` after the button label"))?;
            let (target, value) = assignment
                .split_once('=')
                .ok_or_else(|| format!("line {line_no}: expected `name = value` after `->`"))?;
            let target = target.trim();
            check_name(target, line_no)?;
            Ok(format!(
                "    button {label} updates {target} with {};",
                expression(value.trim(), line_no)?
            ))
        }
        _ if trimmed.contains('=') => states(trimmed, false, line_no),
        _ => Err(format!(
            "line {line_no}: expected `save`, `ask`, `show`, `button`, or `name = 0`"
        )),
    }
}

fn states(text: &str, persist: bool, line_no: usize) -> Result<String, String> {
    let (names, initial) = text
        .split_once('=')
        .ok_or_else(|| format!("line {line_no}: expected `name = 0`"))?;
    let initial = initial.trim();
    if initial.is_empty() || !initial.chars().all(|c| c.is_ascii_digit()) {
        return Err(format!(
            "line {line_no}: the starting value must be a number"
        ));
    }
    let mut out = String::from("   ");
    for name in names_list(names, line_no)? {
        out.push_str(&format!(" state {name}: number = {initial};"));
        if persist {
            out.push_str(&format!(" persist {name};"));
        }
    }
    Ok(out)
}

fn quoted(text: &str, line_no: usize) -> Result<&str, String> {
    if text.len() >= 2 && text.starts_with('"') && text.ends_with('"') {
        Ok(text)
    } else {
        Err(format!(
            "line {line_no}: put the label in quotes, like \"Price\""
        ))
    }
}

/// Proves that `+`, `-`, and `*` cannot leave `0..=u32::MAX`. Facts come
/// from `need` lines and, inside a rule, from that rule's condition (and the
/// failed conditions of the rules above it).
///
/// `known` holds the answer ranges of rules checked earlier in the file, so
/// `double(x) + 1` can use what `double` returns. The answer range of this
/// rule is returned for the rules after it.
pub fn check_ranges(
    contract: &Contract,
    known: &[(String, u64, u64)],
) -> Result<(u64, u64), String> {
    let mut facts = Facts {
        calls: known.to_vec(),
        bounds: contract
            .params
            .iter()
            .map(|param| (param.name.clone(), 0, u64::from(u32::MAX)))
            .collect(),
    };
    for requirement in &contract.requires {
        facts.learn(requirement, true);
    }
    let mut answer = (0, u64::from(u32::MAX));
    for ensures in &contract.ensures {
        if let SpecExpr::Binary(BinaryOp::Eq, left, right) = ensures
            && matches!(left.as_ref(), SpecExpr::Ident(name) if name == "result")
        {
            answer = range(right, &facts, &contract.name)?;
        } else {
            range(ensures, &facts, &contract.name)?;
        }
    }
    Ok(answer)
}

/// What is known about the inputs: value bounds and answers of earlier rules.
#[derive(Clone)]
struct Facts {
    calls: Vec<(String, u64, u64)>,
    bounds: Vec<(String, u64, u64)>,
}

impl Facts {
    /// Records what `expr` being `holds` implies.
    fn learn(&mut self, expr: &SpecExpr, holds: bool) {
        let SpecExpr::Binary(op, left, right) = expr else {
            return;
        };
        match (op, holds) {
            (BinaryOp::And, true) | (BinaryOp::Or, false) => {
                self.learn(left, holds);
                self.learn(right, holds);
                return;
            }
            (BinaryOp::And, false) | (BinaryOp::Or, true) => return,
            _ => {}
        }
        let op = if holds {
            op.clone()
        } else {
            negate(op.clone())
        };
        match (left.as_ref(), right.as_ref()) {
            (SpecExpr::Ident(name), SpecExpr::Number(n)) => self.bound(name, op, u64::from(*n)),
            (SpecExpr::Number(n), SpecExpr::Ident(name)) => {
                self.bound(name, flip(op), u64::from(*n))
            }
            _ => {}
        }
    }

    fn bound(&mut self, name: &str, op: BinaryOp, number: u64) {
        let Some(entry) = self.bounds.iter_mut().find(|(param, _, _)| param == name) else {
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
    let mut in_quotes = false;
    for (index, c) in line.char_indices() {
        match c {
            '"' => in_quotes = !in_quotes,
            '#' if !in_quotes => return &line[..index],
            _ => {}
        }
    }
    line
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
    let mut depth = 0i32;
    for c in text.chars() {
        depth += match c {
            '(' => 1,
            ')' => -1,
            _ => 0,
        };
        if depth < 0 {
            return Err(format!("line {line_no}: `)` has no matching `(`"));
        }
    }
    if depth > 0 {
        return Err(format!("line {line_no}: a `(` is not closed; add `)`"));
    }
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::new();
    let mut index = 0;
    while index < chars.len() {
        let c = chars[index];
        let next = chars.get(index + 1).copied();
        match c {
            '+' | '*' => {
                out.push(c);
                out.push('%');
            }
            '-' => out.push_str("-!"),
            '!' if next == Some('=') => {
                return Err(format!("line {line_no}: use `is not` to compare"));
            }
            '<' | '>' if next == Some('=') => {
                out.push(c);
                out.push('=');
                index += 1;
            }
            '=' => {
                return Err(format!(
                    "line {line_no}: use `is` to compare (`=` only sets values on pages)"
                ));
            }
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
                    "is" => {
                        let rest: String = chars[index + 1..].iter().collect();
                        let after = rest.trim_start();
                        let skip = rest.len() - after.len();
                        if after.starts_with("not")
                            && !after[3..]
                                .chars()
                                .next()
                                .is_some_and(|c| c.is_alphanumeric() || c == '_')
                        {
                            out.push_str("!=");
                            index += skip + 3;
                        } else {
                            out.push_str("==");
                        }
                    }
                    "not" => {
                        return Err(format!("line {line_no}: write `is not` to compare"));
                    }
                    _ => out.push_str(&word),
                }
            }
            _ => out.push(c),
        }
        index += 1;
    }
    expand_min_max(&out, line_no)
}

/// Rewrites `min(a, b)` and `max(a, b)` into conditionals.
fn expand_min_max(text: &str, line_no: usize) -> Result<String, String> {
    let mut out = String::new();
    let mut rest = text;
    while let Some(position) = find_call(rest) {
        let (before, call) = rest.split_at(position);
        out.push_str(before);
        let comparison = if call.starts_with("min") { "<=" } else { ">=" };
        let open = call.find('(').unwrap_or_default();
        let mut depth = 0;
        let mut close = None;
        let mut comma = None;
        for (index, c) in call.char_indices().skip(open) {
            match c {
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth == 0 {
                        close = Some(index);
                        break;
                    }
                }
                ',' if depth == 1 => {
                    if comma.is_some() {
                        return Err(format!("line {line_no}: min and max take two values"));
                    }
                    comma = Some(index);
                }
                _ => {}
            }
        }
        let (Some(close), Some(comma)) = (close, comma) else {
            return Err(format!(
                "line {line_no}: min and max take two values, like `min(a, b)`"
            ));
        };
        let first = expand_min_max(call[open + 1..comma].trim(), line_no)?;
        let second = expand_min_max(call[comma + 1..close].trim(), line_no)?;
        out.push_str(&format!(
            "(if {first} {comparison} {second} {{ {first} }} else {{ {second} }})"
        ));
        rest = &call[close + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

/// Finds `min(` or `max(` that starts a word.
fn find_call(text: &str) -> Option<usize> {
    let bytes = text.as_bytes();
    (0..text.len()).find(|&index| {
        (text[index..].starts_with("min") || text[index..].starts_with("max"))
            && (index == 0
                || !(bytes[index - 1].is_ascii_alphanumeric() || bytes[index - 1] == b'_'))
            && text[index + 3..].trim_start().starts_with('(')
    })
}

fn negate(op: BinaryOp) -> BinaryOp {
    match op {
        BinaryOp::Lt => BinaryOp::Ge,
        BinaryOp::Le => BinaryOp::Gt,
        BinaryOp::Gt => BinaryOp::Le,
        BinaryOp::Ge => BinaryOp::Lt,
        BinaryOp::Eq => BinaryOp::Ne,
        BinaryOp::Ne => BinaryOp::Eq,
        other => other,
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

fn range(expr: &SpecExpr, facts: &Facts, name: &str) -> Result<(u64, u64), String> {
    const MAX: u64 = u32::MAX as u64;
    Ok(match expr {
        SpecExpr::Number(n) => (u64::from(*n), u64::from(*n)),
        SpecExpr::Ident(ident) => facts
            .bounds
            .iter()
            .find(|(param, _, _)| param == ident)
            .map_or((0, MAX), |(_, low, high)| (*low, *high)),
        SpecExpr::Call(callee, args) => {
            for arg in args {
                range(arg, facts, name)?;
            }
            facts
                .calls
                .iter()
                .find(|(rule, _, _)| rule == callee)
                .map_or((0, MAX), |(_, low, high)| (*low, *high))
        }
        SpecExpr::If(condition, then, otherwise) => {
            range(condition, facts, name)?;
            let mut when_true = facts.clone();
            when_true.learn(condition, true);
            let mut when_false = facts.clone();
            when_false.learn(condition, false);
            let (a_low, a_high) = range(then, &when_true, name)?;
            let (b_low, b_high) = range(otherwise, &when_false, name)?;
            (a_low.min(b_low), a_high.max(b_high))
        }
        SpecExpr::Binary(op, left, right) => {
            let (l_low, l_high) = range(left, facts, name)?;
            let (r_low, r_high) = range(right, facts, name)?;
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
                BinaryOp::Div if r_low == 0 => {
                    return Err(format!(
                        "{name}: `/` may divide by 0; add a `need` line that keeps the right side at least 1"
                    ));
                }
                BinaryOp::Div => (l_low / r_high, l_high / r_low),
                // `-` must not go below 0. When the bounds alone cannot show
                // that, the verifier proves it (or reports an input that breaks it).
                BinaryOp::SubChecked if l_low >= r_high => (l_low - r_high, l_high - r_low),
                BinaryOp::SubChecked => (0, l_high),
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
            translate("f(x)\n  x = 1 -> 1\n  -> 0\n")
                .unwrap_err()
                .contains("use `is`")
        );
        assert!(
            translate("f(x)\n  x != 1 -> 1\n  -> 0\n")
                .unwrap_err()
                .contains("is not")
        );
        let out = translate("f(x, y)\n  x is 1 and y is not 2 -> 1\n  -> 0\n").unwrap();
        assert!(out.contains("if x == 1 && y != 2 { 1 }"), "{out}");
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
        crate::parse_module("f(a, b)\n  need a >= 10\n  need b <= 10\n  -> a - b\n").unwrap();
        crate::parse_module("f(a, b)\n  a > b -> a - b\n  -> b - a\n").unwrap();
        crate::parse_module("f(a, b)\n  need a >= b\n  -> a - b\n").unwrap();
        crate::parse_module("f(a)\n  a > 5 -> a - 5\n  -> 0\n").unwrap();
        crate::parse_module("p(x, r)\n  need x <= 1000000\n  need r <= 100\n  -> x * r / 100\n")
            .unwrap();
        assert!(
            crate::parse_module("q(x, d)\n  -> x / d\n")
                .unwrap_err()
                .contains("divide by 0")
        );
        crate::parse_module("q(x, d)\n  need d >= 1\n  -> x / d\n").unwrap();
        let out = translate("c(t, lo, hi)\n  -> max(lo, min(t, hi))\n").unwrap();
        assert!(
            out.contains("(if lo >= (if t <= hi { t } else { hi }) { lo } else { (if t <= hi { t } else { hi }) })"),
            "{out}"
        );
        assert!(
            translate("c(a)\n  -> min(a)\n")
                .unwrap_err()
                .contains("two values")
        );
        crate::parse_module("c(min, max)\n  -> min + max * 0\n").unwrap();
        crate::parse_module(
            "d(x)\n  need x <= 10\n  -> x + x\n\ng(x)\n  need x <= 5\n  -> d(x) + 1\n",
        )
        .unwrap();
        assert!(
            crate::parse_module("g(x)\n  -> h(x) + 1\n")
                .unwrap_err()
                .contains("may exceed")
        );
    }

    #[test]
    fn translates_pages_and_imports() {
        let source = "from logic: increment\n\npage Counter\n  save count = 0\n  shown, other = 1 # note\n  ask count \"Count #\"\n  show count\n  button \"Add\" -> count = increment(count + 1)\n";
        assert!(is_simple(source));
        let out = translate(source).unwrap();
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines[0], "import { increment } from \"./logic.ax\";");
        assert_eq!(lines[2], "page Counter {");
        assert_eq!(lines[3], "    state count: number = 0; persist count;");
        assert_eq!(
            lines[4],
            "    state shown: number = 1; state other: number = 1;"
        );
        assert_eq!(lines[5], "    input count placeholder \"Count #\";");
        assert_eq!(lines[6], "    show count;");
        assert!(
            translate("page P\n  show a * 2\n")
                .unwrap()
                .contains("show a *% 2;")
        );
        assert!(
            translate("page P\n  show \"Hi\"\n")
                .unwrap()
                .contains("show \"Hi\";")
        );
        assert_eq!(
            lines[7],
            "    button \"Add\" updates count with increment(count +% 1);"
        );
        assert_eq!(lines[8], "}");
        let module = crate::parse_module(source).unwrap();
        assert_eq!(module.pages[0].persisted, vec!["count".to_string()]);
    }

    #[test]
    fn rejects_bad_page_lines() {
        assert!(
            translate("page P\n  ask x\n")
                .unwrap_err()
                .contains("ask name")
        );
        assert!(
            translate("page P\n  button Add -> x = 1\n")
                .unwrap_err()
                .contains("button")
        );
        assert!(
            translate("page P\n  save x = y\n")
                .unwrap_err()
                .contains("must be a number")
        );
        assert!(
            translate("page P\n  hello\n")
                .unwrap_err()
                .contains("expected `save`")
        );
    }

    #[test]
    fn old_syntax_is_not_simple() {
        assert!(!is_simple("import { f } from \"./a.ax\";\npage P {\n}"));
        assert!(!is_simple("page P {\n  state x: number = 0;\n}"));
    }

    #[test]
    fn errors_show_the_source_line() {
        assert!(
            translate("f(x)\n  -> (x\n")
                .unwrap_err()
                .contains("not closed")
        );
        assert!(
            translate("f(x)\n  -> x)\n")
                .unwrap_err()
                .contains("no matching")
        );
        let error = crate::parse_module("f(x)\n  x = 1 -> 2\n  -> 0\n").unwrap_err();
        assert!(error.ends_with("\n  2 |   x = 1 -> 2"), "{error}");
    }
}
