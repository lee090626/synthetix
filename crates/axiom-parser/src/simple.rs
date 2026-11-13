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

use crate::ast::{BinaryOp, Contract, SpecExpr, Type};
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
    Page { last_index: usize, page: PageInfo },
}

/// What a page declares that later lines depend on.
struct PageInfo {
    name: String,
    lists: Vec<(String, usize)>,
    /// Every value: name, whether it can be negative, decimal places.
    values: Vec<(String, bool, u32)>,
}

/// Decimal places declared with `with N decimals`, which the contract syntax
/// cannot express: rule name to places, and (page, value) to places.
#[derive(Debug, Default)]
pub struct Decimals {
    pub rules: Vec<(String, u32)>,
    pub states: Vec<(String, String, u32)>,
    /// Rules whose answers are texts: answer `n` means `labels[n]`.
    pub labels: Vec<(String, Vec<String>)>,
}

impl Current {
    fn finish(self, out: &mut [String], decimals: &mut Decimals) -> Result<(), String> {
        match self {
            Current::Rule(block) => block.finish(out, decimals),
            Current::Page { last_index, .. } => {
                out[last_index].push_str("\n}");
                Ok(())
            }
        }
    }
}

/// Translates simple syntax into contract and page syntax, line for line.
pub fn translate(source: &str) -> Result<String, String> {
    translate_with_decimals(source).map(|(text, _)| text)
}

/// Like [`translate`], also returning the declared decimal places.
pub fn translate_with_decimals(source: &str) -> Result<(String, Decimals), String> {
    let mut decimals = Decimals::default();
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
                block.finish(&mut out, &mut decimals)?;
            }
            if let Some(rest) = trimmed.strip_prefix("from ") {
                out[index] = import(rest, line_no)?;
            } else if let Some(name) = trimmed.strip_prefix("page ") {
                let name = name.trim();
                check_name(name, line_no)?;
                out[index] = format!("page {name} {{");
                current = Some(Current::Page {
                    last_index: index,
                    page: PageInfo {
                        name: name.to_string(),
                        lists: Vec::new(),
                        values: Vec::new(),
                    },
                });
            } else {
                let header = header(trimmed, line_no)?;
                out[index] = header.text;
                if header.decimals > 0 {
                    decimals.rules.push((header.rule.clone(), header.decimals));
                }
                current = Some(Current::Rule(Block {
                    name: header.rule,
                    labels: Vec::new(),
                    name_line: line_no,
                    rules: Vec::new(),
                    names: Vec::new(),
                    signed: header.signed,
                    decimals: header.decimals,
                    lists: header.lists,
                }));
            }
            continue;
        }
        match current.as_mut() {
            Some(Current::Rule(block)) => rule_line(block, &mut out, index, trimmed)?,
            Some(Current::Page { last_index, page }) => {
                out[index] = page_line(trimmed, line_no, page, &mut decimals.states)?;
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
        Some(block) => block.finish(&mut out, &mut decimals)?,
        None if out.iter().all(String::is_empty) => {
            return Err("expected `name(a, b)` or `page Name`".to_string());
        }
        None => {}
    }
    Ok((out.join("\n"), decimals))
}

fn rule_line(
    block: &mut Block,
    out: &mut [String],
    index: usize,
    trimmed: &str,
) -> Result<(), String> {
    let line_no = index + 1;
    let ctx = Ctx {
        signed: block.signed,
        decimals: block.decimals,
        lists: &block.lists,
    };
    let expr = |text: &str| -> Result<String, String> {
        Ok(substitute(&expression(text, line_no, &ctx)?, &block.names))
    };
    if let Some((name, value)) = named_value(trimmed) {
        if !block.rules.is_empty() {
            return Err(format!(
                "line {line_no}: put every `name = value` line before the `->` lines"
            ));
        }
        check_name(name, line_no)?;
        if block.names.iter().any(|(known, _)| known == name) {
            return Err(format!("line {line_no}: `{name}` is already set"));
        }
        let value = format!("({})", expr(value)?);
        block.names.push((name.to_string(), value));
    } else if let Some(condition) = trimmed.strip_prefix("need ") {
        if !block.rules.is_empty() {
            return Err(format!(
                "line {line_no}: put every `need` line before the `->` lines"
            ));
        }
        let condition = each_item(condition, &block.lists, line_no)?;
        out[index] = format!("    requires {};", expr(&condition)?);
    } else if let Some((condition, value)) = trimmed.split_once("->") {
        let condition = condition.trim();
        let condition = if condition.is_empty() {
            None
        } else {
            Some(expr(condition)?)
        };
        let value = value.trim();
        let value = if value.len() >= 2 && value.starts_with('"') && value.ends_with('"') {
            if block.signed || block.decimals > 0 {
                return Err(format!(
                    "line {line_no}: a rule with text answers cannot be negative or have decimals"
                ));
            }
            let label = value[1..value.len() - 1].to_string();
            let position = match block.labels.iter().position(|known| known == &label) {
                Some(position) => position,
                None => {
                    block.labels.push(label);
                    block.labels.len() - 1
                }
            };
            format!("{TEXT_MARK}{position}")
        } else {
            expr(value)?
        };
        block.rules.push(Rule {
            index,
            line_no,
            condition,
            value,
        });
    } else {
        return Err(format!(
            "line {line_no}: expected `need ...`, `name = value`, `condition -> value`, or `-> value`"
        ));
    }
    Ok(())
}

/// Splits `name = value` (a single `=`, not part of `<=` or `>=`).
fn named_value(line: &str) -> Option<(&str, &str)> {
    if line.contains("->") {
        return None;
    }
    let (name, value) = line.split_once('=')?;
    let name = name.trim();
    let simple = !name.is_empty() && name.chars().all(|c| c.is_alphanumeric() || c == '_');
    (simple && !value.starts_with('=')).then_some((name, value.trim()))
}

/// Replaces whole-word uses of earlier names with their values.
fn substitute(text: &str, names: &[(String, String)]) -> String {
    if names.is_empty() {
        return text.to_string();
    }
    let mut out = String::new();
    let mut word = String::new();
    let flush = |word: &mut String, out: &mut String| {
        match names.iter().find(|(name, _)| name == word) {
            Some((_, value)) => out.push_str(value),
            None => out.push_str(word),
        }
        word.clear();
    };
    for c in text.chars() {
        if c.is_alphanumeric() || c == '_' {
            word.push(c);
        } else {
            flush(&mut word, &mut out);
            out.push(c);
        }
    }
    flush(&mut word, &mut out);
    out
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

fn page_line(
    trimmed: &str,
    line_no: usize,
    page: &mut PageInfo,
    decimals: &mut Vec<(String, String, u32)>,
) -> Result<String, String> {
    let (word, rest) = trimmed.split_once(' ').unwrap_or((trimmed, ""));
    let rest = rest.trim();
    let values = page.values.clone();
    let list_names = page.lists.clone();
    let lists = &mut page.lists;
    let mut states =
        |text: &str, persist: bool, line_no: usize, lists: &mut Vec<(String, usize)>| {
            let (text, places) = strip_decimals(text, line_no)?;
            let (line, signed) = states(text, persist, places, line_no, lists)?;
            for state in line.split(" state ").skip(1) {
                let name = state.split(':').next().unwrap_or_default();
                page.values.push((name.to_string(), signed, places));
                if places > 0 {
                    decimals.push((page.name.clone(), name.to_string(), places));
                }
            }
            Ok::<_, String>(line)
        };
    // Values that can be negative are calculated in rules.
    let expression = |text: &str, line_no: usize| {
        let (rewritten, _) = rewrite_lists(text, &list_names, None, line_no)?;
        let used: Vec<&(String, bool, u32)> = words(&rewritten)
            .filter_map(|word| values.iter().find(|(name, _, _)| name == word))
            .collect();
        // Values with fewer places are raised to the most places used.
        let places = used.iter().map(|(_, _, places)| *places).max().unwrap_or(0);
        let arithmetic =
            words(&rewritten).any(|word| word == "mod") || rewritten.contains(['+', '-', '*', '/']);
        if arithmetic && used.iter().any(|(_, signed, _)| *signed) {
            return Err(format!(
                "line {line_no}: calculate with values that can be negative in a rule"
            ));
        }
        let ctx = Ctx {
            signed: false,
            decimals: places,
            lists: &list_names,
        };
        expression(text, line_no, &ctx)
    };
    match word {
        "save" => states(rest, true, line_no, lists),
        "ask" => {
            let (name, label) = rest
                .split_once(' ')
                .ok_or_else(|| format!("line {line_no}: expected `ask name \"label\"`"))?;
            check_name(name, line_no)?;
            let label = quoted(label.trim(), line_no)?;
            if let Some((_, size)) = lists.iter().find(|(list, _)| list == name) {
                let label = &label[1..label.len() - 1];
                return Ok((1..=*size)
                    .map(|item| format!("    input {name}_{item} placeholder \"{label} {item}\";"))
                    .collect::<Vec<_>>()
                    .join(""));
            }
            Ok(format!("    input {name} placeholder {label};"))
        }
        "show" if rest.starts_with('"') => match split_if(rest) {
            Some((text, condition)) => Ok(format!(
                "    when {} show {};",
                expression(condition, line_no)?,
                quoted(text, line_no)?
            )),
            None => Ok(format!("    show {};", quoted(rest, line_no)?)),
        },
        "show" if split_if(rest).is_some() => Err(format!(
            "line {line_no}: only a text in quotes can be shown with `if`, like `show \"Too high\" if total > 100`"
        )),
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
        _ if trimmed.contains('=') => states(trimmed, false, line_no, lists),
        _ => Err(format!(
            "line {line_no}: expected `save`, `ask`, `show`, `button`, or `name = 0`"
        )),
    }
}

/// Translates `a, b = 0` (with an optional `can be negative`) into states;
/// also returns whether they can be negative.
fn states(
    text: &str,
    persist: bool,
    places: u32,
    line_no: usize,
    lists: &mut Vec<(String, usize)>,
) -> Result<(String, bool), String> {
    let (text, signed) = match text.strip_suffix("can be negative") {
        Some(rest) => (rest.trim_end(), true),
        None => (text, false),
    };
    let (names, initial) = text
        .split_once('=')
        .ok_or_else(|| format!("line {line_no}: expected `name = 0`"))?;
    let initial = initial.trim();
    let text_value = initial.len() >= 2 && initial.starts_with('"') && initial.ends_with('"');
    if text_value && (signed || places > 0) {
        return Err(format!(
            "line {line_no}: a text value cannot be negative or have decimals"
        ));
    }
    let initial = if text_value {
        initial.to_string()
    } else if !initial.is_empty() && initial.chars().all(|c| c.is_ascii_digit() || c == '.') {
        scaled_number(initial, places, line_no)?
    } else {
        return Err(format!(
            "line {line_no}: the starting value must be a number or a text in quotes"
        ));
    };
    let ty = if text_value {
        "text"
    } else if signed {
        "i32"
    } else {
        "number"
    };
    let mut out = String::from("   ");
    for name in names
        .split(',')
        .map(str::trim)
        .filter(|name| !name.is_empty())
    {
        let items: Vec<String> = match list_param(name, line_no)? {
            Some((list, size)) => {
                lists.push((list.to_string(), size));
                (1..=size).map(|item| format!("{list}_{item}")).collect()
            }
            None => {
                check_name(name, line_no)?;
                vec![name.to_string()]
            }
        };
        for name in items {
            out.push_str(&format!(" state {name}: {ty} = {initial};"));
            if persist {
                out.push_str(&format!(" persist {name};"));
            }
        }
    }
    if out.trim().is_empty() {
        return Err(format!("line {line_no}: expected at least one name"));
    }
    Ok((out, signed))
}

/// The words (names and keywords) of an expression, outside quotes.
fn words(text: &str) -> impl Iterator<Item = &str> {
    let mut in_quotes = false;
    text.split(move |c: char| {
        if c == '"' {
            in_quotes = !in_quotes;
        }
        in_quotes || !(c.is_alphanumeric() || c == '_')
    })
    .filter(|word| !word.is_empty())
}

/// Splits `"text" if condition` at an `if` outside the quotes.
fn split_if(text: &str) -> Option<(&str, &str)> {
    let mut in_quotes = false;
    for (index, c) in text.char_indices() {
        match c {
            '"' => in_quotes = !in_quotes,
            ' ' if !in_quotes && text[index..].starts_with(" if ") => {
                return Some((text[..index].trim(), text[index + 4..].trim()));
            }
            _ => {}
        }
    }
    None
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

/// Records decimal places on rules and page values, and rescales `*` and `/`
/// in decimal rules: with 2 places, `a * b` is `a * b / 100` and `a / b` is
/// `a * 100 / b`. A call to a rule with other places converts the inputs to
/// its places and the answer back (`/` rounds down when places are dropped).
pub fn apply_decimals(module: &mut crate::ast::Module, decimals: &Decimals) -> Result<(), String> {
    let places_of = |name: &str| {
        decimals
            .rules
            .iter()
            .find(|(rule, _)| rule == name)
            .map_or(0, |(_, places)| *places)
    };
    let local: Vec<String> = module.contracts.iter().map(|c| c.name.clone()).collect();
    let labeled: Vec<String> = decimals
        .labels
        .iter()
        .map(|(rule, _)| rule.clone())
        .collect();
    for contract in &mut module.contracts {
        contract.decimals = places_of(&contract.name);
        contract.labels = decimals
            .labels
            .iter()
            .find(|(rule, _)| rule == &contract.name)
            .map(|(_, labels)| labels.clone())
            .unwrap_or_default();
        let scale = 10u32.pow(contract.decimals);
        for expr in contract
            .requires
            .iter_mut()
            .chain(contract.ensures.iter_mut())
        {
            check_calls(expr, &|callee: &str| {
                if labeled.iter().any(|name| name == callee) {
                    return Err(format!(
                        "{}: {callee} answers with text, which only a page can show",
                        contract.name
                    ));
                }
                Ok(())
            })?;
            if scale > 1 {
                *expr = rescale(expr.clone(), scale);
            }
            *expr = convert_calls(expr.clone(), contract.decimals, &|callee: &str| {
                local
                    .iter()
                    .any(|name| name == callee)
                    .then(|| places_of(callee))
            });
        }
    }
    for page in &mut module.pages {
        for state in &mut page.states {
            state.decimals = decimals
                .states
                .iter()
                .find(|(owner, name, _)| owner == &page.name && name == &state.name)
                .map_or(0, |(_, _, places)| *places);
        }
        // `show price * 2` with 2 places is `price * 200 / 100`; literals were
        // already scaled when the line was translated.
        let places: Vec<(String, u32)> = page
            .states
            .iter()
            .map(|state| (state.name.clone(), state.decimals))
            .collect();
        for element in &mut page.elements {
            let (crate::ast::PageElement::ShowExpr { value: expr }
            | crate::ast::PageElement::WhenShow {
                condition: expr, ..
            }) = element
            else {
                continue;
            };
            let used = idents(expr)
                .into_iter()
                .filter_map(|name| places.iter().find(|(state, _)| state == name))
                .map(|(_, places)| *places)
                .max()
                .unwrap_or(0);
            if used > 0 {
                *expr = lift(rescale(expr.clone(), 10u32.pow(used)), used, &places);
            }
        }
    }
    Ok(())
}

fn idents(expr: &SpecExpr) -> Vec<&str> {
    match expr {
        SpecExpr::Ident(name) => vec![name.as_str()],
        SpecExpr::Binary(_, left, right) => {
            let mut names = idents(left);
            names.extend(idents(right));
            names
        }
        SpecExpr::If(condition, then, otherwise) => {
            let mut names = idents(condition);
            names.extend(idents(then));
            names.extend(idents(otherwise));
            names
        }
        SpecExpr::Call(_, args) => args.iter().flat_map(idents).collect(),
        _ => Vec::new(),
    }
}

fn check_calls(expr: &SpecExpr, check: &dyn Fn(&str) -> Result<(), String>) -> Result<(), String> {
    match expr {
        SpecExpr::Call(callee, args) => {
            check(callee)?;
            for arg in args {
                check_calls(arg, check)?;
            }
        }
        SpecExpr::Binary(_, left, right) => {
            check_calls(left, check)?;
            check_calls(right, check)?;
        }
        SpecExpr::If(condition, then, otherwise) => {
            check_calls(condition, check)?;
            check_calls(then, check)?;
            check_calls(otherwise, check)?;
        }
        _ => {}
    }
    Ok(())
}

/// Moves a value stored with `from` decimal places to `to` places.
fn change_places(expr: SpecExpr, from: u32, to: u32) -> SpecExpr {
    let (op, factor) = match from.cmp(&to) {
        std::cmp::Ordering::Equal => return expr,
        std::cmp::Ordering::Less => (BinaryOp::MulWrap, 10u32.pow(to - from)),
        std::cmp::Ordering::Greater => (BinaryOp::Div, 10u32.pow(from - to)),
    };
    SpecExpr::Binary(op, Box::new(expr), Box::new(SpecExpr::Number(factor)))
}

/// Wraps calls to rules with other places: inputs go to the callee's places,
/// the answer comes back to `places`. Runs after [`rescale`].
fn convert_calls(expr: SpecExpr, places: u32, places_of: &dyn Fn(&str) -> Option<u32>) -> SpecExpr {
    let boxed = |expr: SpecExpr| Box::new(convert_calls(expr, places, places_of));
    match expr {
        SpecExpr::Call(callee, args) => {
            let other = places_of(&callee).unwrap_or(places);
            let args = args
                .into_iter()
                .map(|arg| change_places(convert_calls(arg, places, places_of), places, other))
                .collect();
            change_places(SpecExpr::Call(callee, args), other, places)
        }
        SpecExpr::Binary(op, left, right) => SpecExpr::Binary(op, boxed(*left), boxed(*right)),
        SpecExpr::If(condition, then, otherwise) => {
            SpecExpr::If(boxed(*condition), boxed(*then), boxed(*otherwise))
        }
        other => other,
    }
}

/// Raises page values with fewer places to `places`. Runs after [`rescale`].
fn lift(expr: SpecExpr, places: u32, of: &[(String, u32)]) -> SpecExpr {
    let boxed = |expr: SpecExpr| Box::new(lift(expr, places, of));
    match expr {
        SpecExpr::Ident(name) => {
            let own = of
                .iter()
                .find(|(state, _)| *state == name)
                .map_or(places, |(_, own)| *own);
            change_places(SpecExpr::Ident(name), own, places)
        }
        SpecExpr::Binary(op, left, right) => SpecExpr::Binary(op, boxed(*left), boxed(*right)),
        SpecExpr::If(condition, then, otherwise) => {
            SpecExpr::If(boxed(*condition), boxed(*then), boxed(*otherwise))
        }
        SpecExpr::Call(callee, args) => SpecExpr::Call(
            callee,
            args.into_iter().map(|arg| lift(arg, places, of)).collect(),
        ),
        other => other,
    }
}

fn rescale(expr: SpecExpr, scale: u32) -> SpecExpr {
    let boxed = |expr: SpecExpr| Box::new(rescale(expr, scale));
    match expr {
        SpecExpr::Binary(BinaryOp::MulWrap, left, right) => SpecExpr::Binary(
            BinaryOp::Div,
            Box::new(SpecExpr::Binary(
                BinaryOp::MulWrap,
                boxed(*left),
                boxed(*right),
            )),
            Box::new(SpecExpr::Number(scale)),
        ),
        SpecExpr::Binary(BinaryOp::Div, left, right) => SpecExpr::Binary(
            BinaryOp::Div,
            Box::new(SpecExpr::Binary(
                BinaryOp::MulWrap,
                boxed(*left),
                Box::new(SpecExpr::Number(scale)),
            )),
            boxed(*right),
        ),
        SpecExpr::Binary(op, left, right) => SpecExpr::Binary(op, boxed(*left), boxed(*right)),
        SpecExpr::If(condition, then, otherwise) => {
            SpecExpr::If(boxed(*condition), boxed(*then), boxed(*otherwise))
        }
        SpecExpr::Call(callee, args) => SpecExpr::Call(
            callee,
            args.into_iter().map(|arg| rescale(arg, scale)).collect(),
        ),
        other => other,
    }
}

/// Proves that `+`, `-`, `*`, and `/` stay in range: `0..=u32::MAX`, or the
/// i32 range for `can be negative` rules. Facts come from `need` lines and,
/// inside a rule, from that rule's condition (and the failed conditions of the
/// rules above it).
///
/// `known` holds the answer ranges of rules checked earlier in the file, so
/// `double(x) + 1` can use what `double` returns. The answer range of this
/// rule is returned for the rules after it.
pub fn check_ranges(
    contract: &Contract,
    known: &[(String, i64, i64)],
) -> Result<(i64, i64), String> {
    let limits = if contract.return_type == Type::I32 {
        (i64::from(i32::MIN), i64::from(i32::MAX))
    } else {
        (0, i64::from(u32::MAX))
    };
    let mut facts = Facts {
        limits,
        calls: known.to_vec(),
        bounds: contract
            .params
            .iter()
            .map(|param| (param.name.clone(), limits.0, limits.1))
            .collect(),
    };
    for requirement in &contract.requires {
        facts.learn(requirement, true);
    }
    let mut answer = limits;
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
    limits: (i64, i64),
    calls: Vec<(String, i64, i64)>,
    bounds: Vec<(String, i64, i64)>,
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
            (SpecExpr::Ident(name), number) => {
                if let Some(n) = constant(number) {
                    self.bound(name, op, n);
                }
            }
            (number, SpecExpr::Ident(name)) => {
                if let Some(n) = constant(number) {
                    self.bound(name, flip(op), n);
                }
            }
            _ => {}
        }
    }

    fn bound(&mut self, name: &str, op: BinaryOp, number: i64) {
        let Some(entry) = self.bounds.iter_mut().find(|(param, _, _)| param == name) else {
            return;
        };
        match op {
            BinaryOp::Le => entry.2 = entry.2.min(number),
            BinaryOp::Lt => entry.2 = entry.2.min(number - 1),
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
    name: String,
    /// Text answers in order of first use; empty for number rules.
    labels: Vec<String>,
    name_line: usize,
    rules: Vec<Rule>,
    /// `name = value` lines, already translated, in order.
    names: Vec<(String, String)>,
    /// `can be negative` rules use signed numbers.
    signed: bool,
    /// `with N decimals` rules scale every number by 10^N.
    decimals: u32,
    /// List inputs (`prices[3]`) and their sizes.
    lists: Vec<(String, usize)>,
}

/// What an expression may use: signed arithmetic and list names.
struct Ctx<'a> {
    signed: bool,
    decimals: u32,
    lists: &'a [(String, usize)],
}

struct Rule {
    index: usize,
    line_no: usize,
    condition: Option<String>,
    value: String,
}

/// Marks a translated text answer (its position in `Block::labels`).
const TEXT_MARK: &str = "#text:";

impl Block {
    fn finish(mut self, out: &mut [String], decimals: &mut Decimals) -> Result<(), String> {
        let texts = self
            .rules
            .iter()
            .filter(|rule| rule.value.starts_with(TEXT_MARK))
            .count();
        if texts > 0 && texts < self.rules.len() {
            let rule = self
                .rules
                .iter()
                .find(|rule| !rule.value.starts_with(TEXT_MARK))
                .map_or(self.name_line, |rule| rule.line_no);
            return Err(format!(
                "line {rule}: give every `->` line a text answer in quotes, or none"
            ));
        }
        for rule in &mut self.rules {
            if let Some(position) = rule.value.strip_prefix(TEXT_MARK) {
                rule.value = position.to_string();
            }
        }
        if !self.labels.is_empty() {
            decimals
                .labels
                .push((self.name.clone(), self.labels.clone()));
        }
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

struct Header {
    rule: String,
    text: String,
    signed: bool,
    decimals: u32,
    lists: Vec<(String, usize)>,
}

/// Parses `name(a, b)`, `name(prices[3])`, `name(a, b) can be negative`, and
/// `name(a, b) with 2 decimals`.
fn header(line: &str, line_no: usize) -> Result<Header, String> {
    let (line, decimals) = strip_decimals(line, line_no)?;
    let (line, signed) = match line.strip_suffix("can be negative") {
        Some(rest) => (rest.trim_end(), true),
        None => (line, false),
    };
    let (name, rest) = line
        .split_once('(')
        .ok_or_else(|| format!("line {line_no}: expected a name like `price(a, b)`"))?;
    let params = rest.trim().strip_suffix(')').ok_or_else(|| {
        format!("line {line_no}: expected `)` at the end of the name line (or `) can be negative`)")
    })?;
    let name = name.trim();
    check_name(name, line_no)?;
    let ty = if signed { "i32" } else { "u32" };
    let mut typed = Vec::new();
    let mut lists = Vec::new();
    for param in params.split(',').map(str::trim).filter(|p| !p.is_empty()) {
        match list_param(param, line_no)? {
            Some((list, size)) => {
                for item in 1..=size {
                    typed.push(format!("{list}_{item}: {ty}"));
                }
                lists.push((list.to_string(), size));
            }
            None => {
                check_name(param, line_no)?;
                typed.push(format!("{param}: {ty}"));
            }
        }
    }
    Ok(Header {
        rule: name.to_string(),
        text: format!("export contract {name}({}) -> {ty} {{", typed.join(", ")),
        signed,
        decimals,
        lists,
    })
}

/// The most decimal places a number may have.
const MAX_DECIMALS: u32 = 6;

/// Strips a trailing `with 2 decimals` (or `with 1 decimal`).
fn strip_decimals(line: &str, line_no: usize) -> Result<(&str, u32), String> {
    let Some(rest) = line
        .strip_suffix("decimals")
        .or_else(|| line.strip_suffix("decimal"))
    else {
        return Ok((line, 0));
    };
    let Some((before, places)) = rest.trim_end().rsplit_once(" with ") else {
        return Ok((line, 0));
    };
    let places = places
        .trim()
        .parse::<u32>()
        .ok()
        .filter(|places| (1..=MAX_DECIMALS).contains(places))
        .ok_or_else(|| {
            format!("line {line_no}: write `with 2 decimals` with 1 to {MAX_DECIMALS} places")
        })?;
    Ok((before.trim_end(), places))
}

/// Reads `12.5` as a whole number times 10^decimals (`1250` for 2 places).
fn scaled_number(text: &str, decimals: u32, line_no: usize) -> Result<String, String> {
    let (whole, fraction) = text.split_once('.').unwrap_or((text, ""));
    if fraction.len() > decimals as usize {
        return Err(if decimals == 0 {
            format!("line {line_no}: `{text}` has decimals; add `with 2 decimals` to the name line")
        } else {
            format!("line {line_no}: `{text}` has more than {decimals} decimal places")
        });
    }
    if whole.is_empty() || (text.contains('.') && fraction.is_empty()) {
        return Err(format!("line {line_no}: `{text}` is not a number"));
    }
    let digits = format!("{whole}{fraction:0<width$}", width = decimals as usize);
    let value = digits
        .parse::<u32>()
        .map_err(|_| format!("line {line_no}: `{text}` is too large"))?;
    Ok(value.to_string())
}

/// Reads `prices[3]` as a list of three values.
fn list_param(text: &str, line_no: usize) -> Result<Option<(&str, usize)>, String> {
    let Some((name, size)) = text.split_once('[') else {
        return Ok(None);
    };
    let name = name.trim();
    check_name(name, line_no)?;
    let size = size
        .strip_suffix(']')
        .and_then(|size| size.trim().parse::<usize>().ok())
        .filter(|size| (1..=MAX_LIST).contains(size))
        .ok_or_else(|| {
            format!("line {line_no}: write a list as `{name}[3]` with a size from 1 to {MAX_LIST}")
        })?;
    Ok(Some((name, size)))
}

/// The largest list size; each item becomes one input.
const MAX_LIST: usize = 16;

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
fn expression(text: &str, line_no: usize, ctx: &Ctx) -> Result<String, String> {
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
    let text = rewrite_lists(text, ctx.lists, None, line_no)?.0;
    let text = expand_round(&text, ctx.signed, ctx.decimals, line_no)?;
    let text = expand_negatives(&text, ctx.signed, line_no)?;
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
            '-' if ctx.signed => out.push_str("-%"),
            '-' => out.push_str("-!"),
            '"' => {
                let close = chars[index + 1..]
                    .iter()
                    .position(|&c| c == '"')
                    .ok_or_else(|| format!("line {line_no}: a `\"` is not closed"))?;
                out.extend(&chars[index..=index + 1 + close]);
                index += 1 + close;
            }
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
            c if c.is_ascii_digit() => {
                let start = index;
                while index + 1 < chars.len()
                    && (chars[index + 1].is_ascii_digit() || chars[index + 1] == '.')
                {
                    index += 1;
                }
                let number: String = chars[start..=index].iter().collect();
                out.push_str(&scaled_number(&number, ctx.decimals, line_no)?);
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
                    "mod" => out.push('%'),
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

/// Rewrites a minus sign in front of a value (`-5`, `-x`, `-(a + b)`) into
/// `(0 - value)`. Only rules that can be negative allow it.
fn expand_negatives(text: &str, signed: bool, line_no: usize) -> Result<String, String> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::new();
    let mut index = 0;
    while index < chars.len() {
        let c = chars[index];
        if c != '-' || !starts_value(&out) {
            out.push(c);
            index += 1;
            continue;
        }
        if !signed {
            return Err(format!(
                "line {line_no}: negative numbers need `can be negative` on the name line"
            ));
        }
        let mut start = index + 1;
        while start < chars.len() && chars[start] == ' ' {
            start += 1;
        }
        let mut end = start;
        if end < chars.len() && (chars[end].is_alphanumeric() || chars[end] == '_') {
            while end < chars.len()
                && (chars[end].is_alphanumeric() || chars[end] == '_' || chars[end] == '.')
            {
                end += 1;
            }
        }
        if end < chars.len() && chars[end] == '(' {
            let mut depth = 0;
            while end < chars.len() {
                depth += match chars[end] {
                    '(' => 1,
                    ')' => -1,
                    _ => 0,
                };
                end += 1;
                if depth == 0 {
                    break;
                }
            }
        }
        if end == start {
            return Err(format!("line {line_no}: expected a value after `-`"));
        }
        let operand: String = chars[start..end].iter().collect();
        out.push_str(&format!(
            "(0 - {})",
            expand_negatives(&operand, signed, line_no)?
        ));
        index = end;
    }
    Ok(out)
}

/// True when a `-` here starts a value instead of subtracting.
fn starts_value(before: &str) -> bool {
    let before = before.trim_end();
    let word = before
        .rsplit(|c: char| !(c.is_alphanumeric() || c == '_'))
        .next()
        .unwrap_or_default();
    match before.chars().last() {
        None => true,
        Some('(' | ',' | '+' | '-' | '*' | '/' | '<' | '>' | '=') => true,
        Some(_) => matches!(word, "and" | "or" | "is" | "not" | "mod"),
    }
}

/// Rewrites list uses: `sum(p)`, `min(p)`, `max(p)`, `count(p)`, `p[2]`, and
/// a bare `p` (all items as call arguments, or item `each` when given).
/// Also reports whether a bare list name was seen.
fn rewrite_lists(
    text: &str,
    lists: &[(String, usize)],
    each: Option<usize>,
    line_no: usize,
) -> Result<(String, bool), String> {
    if lists.is_empty() {
        return Ok((text.to_string(), false));
    }
    let size_of = |name: &str| lists.iter().find(|(list, _)| list == name).map(|(_, n)| *n);
    let items =
        |name: &str, n: usize| -> Vec<String> { (1..=n).map(|i| format!("{name}_{i}")).collect() };
    let mut out = String::new();
    let mut bare = false;
    let mut rest = text;
    while let Some(start) = rest.find(|c: char| c.is_alphanumeric() || c == '_') {
        out.push_str(&rest[..start]);
        let after = &rest[start..];
        let end = after
            .find(|c: char| !(c.is_alphanumeric() || c == '_'))
            .unwrap_or(after.len());
        let word = &after[..end];
        let tail = &after[end..];
        // `sum(prices)` and friends take a whole list.
        if matches!(word, "sum" | "min" | "max" | "count")
            && let Some(inner) = tail.trim_start().strip_prefix('(')
            && let Some(close) = inner.find(')')
            && let Some(n) = size_of(inner[..close].trim())
        {
            let list = inner[..close].trim();
            let names = items(list, n);
            let value = match word {
                "sum" => format!("({})", names.join(" + ")),
                "count" => n.to_string(),
                _ => names
                    .iter()
                    .rev()
                    .cloned()
                    .reduce(|acc, name| format!("{word}({name}, {acc})"))
                    .unwrap_or_default(),
            };
            out.push_str(&value);
            rest = &inner[close + 1..];
            continue;
        }
        let Some(n) = size_of(word) else {
            out.push_str(word);
            rest = tail;
            continue;
        };
        if let Some(index) = tail.strip_prefix('[') {
            let close = index
                .find(']')
                .ok_or_else(|| format!("line {line_no}: `[` needs a closing `]`"))?;
            let item = index[..close]
                .trim()
                .parse::<usize>()
                .ok()
                .filter(|item| (1..=n).contains(item))
                .ok_or_else(|| {
                    format!("line {line_no}: `{word}` has items 1 to {n}; write `{word}[1]`")
                })?;
            out.push_str(&format!("{word}_{item}"));
            rest = &index[close + 1..];
            continue;
        }
        bare = true;
        match each {
            Some(item) if item <= n => out.push_str(&format!("{word}_{item}")),
            Some(_) => {
                return Err(format!(
                    "line {line_no}: lists in one `need` line must have the same size"
                ));
            }
            None => out.push_str(&items(word, n).join(", ")),
        }
        rest = tail;
    }
    out.push_str(rest);
    Ok((out, bare))
}

/// `need prices <= 100` holds for every item: one condition per item.
fn each_item(condition: &str, lists: &[(String, usize)], line_no: usize) -> Result<String, String> {
    let (_, bare) = rewrite_lists(condition, lists, Some(0), line_no).unwrap_or_default();
    if !bare {
        return Ok(condition.to_string());
    }
    let size = lists.iter().map(|(_, n)| *n).max().unwrap_or(1);
    let mut parts = Vec::new();
    for item in 1..=size {
        let (part, _) = rewrite_lists(condition, lists, Some(item), line_no)?;
        parts.push(format!("({part})"));
    }
    Ok(parts.join(" and "))
}

/// Rewrites `round(a / b)` into `(a + b / 2) / b` (rounds half up). With
/// decimals or negatives it rounds the last place from twice the quotient,
/// `d = 2 * a / b`: `(d + 1) / 2`, or `(d + sign(d)) / 2` (half away from 0),
/// where 1 is one unit of the last decimal place.
fn expand_round(text: &str, signed: bool, decimals: u32, line_no: usize) -> Result<String, String> {
    let Some(start) = find_word_call(text, "round") else {
        return Ok(text.to_string());
    };
    let open = start + text[start..].find('(').unwrap_or_default();
    let mut depth = 0;
    let mut close = None;
    let mut slash = None;
    for (index, c) in text.char_indices().skip(open) {
        match c {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    close = Some(index);
                    break;
                }
            }
            '/' if depth == 1 => slash = Some(index),
            _ => {}
        }
    }
    let (Some(close), Some(slash)) = (close, slash) else {
        return Err(format!(
            "line {line_no}: round takes a division, like `round(price / 3)`"
        ));
    };
    let top = expand_round(text[open + 1..slash].trim(), signed, decimals, line_no)?;
    let bottom = expand_round(text[slash + 1..close].trim(), signed, decimals, line_no)?;
    let rounded = if !signed && decimals == 0 {
        format!("((({top}) + ({bottom}) / 2) / ({bottom}))")
    } else {
        let unit = if decimals == 0 {
            "1".to_string()
        } else {
            format!("0.{}1", "0".repeat(decimals as usize - 1))
        };
        let twice = format!("(2 * ({top}) / ({bottom}))");
        if signed {
            format!("(({twice} + max(-{unit}, min({unit}, {twice}))) / 2)")
        } else {
            format!("(({twice} + {unit}) / 2)")
        }
    };
    Ok(format!(
        "{}{rounded}{}",
        &text[..start],
        expand_round(&text[close + 1..], signed, decimals, line_no)?
    ))
}

/// Finds `name(` where `name` starts a word.
fn find_word_call(text: &str, name: &str) -> Option<usize> {
    let bytes = text.as_bytes();
    (0..text.len()).find(|&index| {
        text[index..].starts_with(name)
            && (index == 0
                || !(bytes[index - 1].is_ascii_alphanumeric() || bytes[index - 1] == b'_'))
            && text[index + name.len()..].trim_start().starts_with('(')
    })
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

/// The value of a number expression such as `0 - 5` (a written `-5`).
fn constant(expr: &SpecExpr) -> Option<i64> {
    match expr {
        SpecExpr::Number(n) => Some(i64::from(*n)),
        SpecExpr::Binary(BinaryOp::SubWrap | BinaryOp::SubChecked, left, right) => {
            Some(constant(left)? - constant(right)?)
        }
        SpecExpr::Binary(BinaryOp::AddWrap, left, right) => {
            Some(constant(left)? + constant(right)?)
        }
        SpecExpr::Binary(BinaryOp::MulWrap, left, right) => {
            constant(left)?.checked_mul(constant(right)?)
        }
        SpecExpr::Binary(BinaryOp::Div, left, right) => {
            constant(left)?.checked_div(constant(right)?)
        }
        _ => None,
    }
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

fn range(expr: &SpecExpr, facts: &Facts, name: &str) -> Result<(i64, i64), String> {
    let (min, max) = facts.limits;
    let fits = |low: i64, high: i64, op: &str| {
        if low < min || high > max {
            Err(out_of_range(name, op, facts.limits))
        } else {
            Ok((low, high))
        }
    };
    Ok(match expr {
        SpecExpr::Number(n) => (i64::from(*n), i64::from(*n)),
        SpecExpr::Ident(ident) => facts
            .bounds
            .iter()
            .find(|(param, _, _)| param == ident)
            .map_or((min, max), |(_, low, high)| (*low, *high)),
        SpecExpr::Call(callee, args) => {
            for arg in args {
                range(arg, facts, name)?;
            }
            facts
                .calls
                .iter()
                .find(|(rule, _, _)| rule == callee)
                .map_or((min, max), |(_, low, high)| (*low, *high))
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
                BinaryOp::AddWrap => fits(l_low + r_low, l_high + r_high, "+")?,
                BinaryOp::SubWrap => fits(l_low - r_high, l_high - r_low, "-")?,
                BinaryOp::MulWrap => {
                    let corners = [
                        i128::from(l_low) * i128::from(r_low),
                        i128::from(l_low) * i128::from(r_high),
                        i128::from(l_high) * i128::from(r_low),
                        i128::from(l_high) * i128::from(r_high),
                    ];
                    let low = corners.iter().min().copied().unwrap_or_default();
                    let high = corners.iter().max().copied().unwrap_or_default();
                    if low < i128::from(min) || high > i128::from(max) {
                        return Err(out_of_range(name, "*", facts.limits));
                    }
                    (low as i64, high as i64)
                }
                BinaryOp::Div | BinaryOp::Rem if min < 0 && (r_low > 0 || r_high < 0) => {
                    if *op == BinaryOp::Rem {
                        let m = r_low.abs().max(r_high.abs()) - 1;
                        let low = if l_low < 0 { l_low.max(-m) } else { 0 };
                        let high = if l_high > 0 { l_high.min(m) } else { 0 };
                        (low, high)
                    } else {
                        let corners = [
                            l_low / r_low,
                            l_low / r_high,
                            l_high / r_low,
                            l_high / r_high,
                        ];
                        let low = corners.iter().min().copied().unwrap_or_default();
                        let high = corners.iter().max().copied().unwrap_or_default();
                        fits(low, high, "/")?
                    }
                }
                BinaryOp::Div | BinaryOp::Rem if r_low <= 0 => {
                    let op = if *op == BinaryOp::Div { "/" } else { "mod" };
                    return Err(format!(
                        "{name}: `{op}` may divide by 0; add a `need` line that keeps the right side at least 1"
                    ));
                }
                BinaryOp::Div => (l_low / r_high, l_high / r_low),
                BinaryOp::Rem => (0, l_high.min(r_high - 1)),
                // `-` must not go below 0. When the bounds alone cannot show
                // that, the verifier proves it (or reports an input that breaks it).
                BinaryOp::SubChecked if l_low >= r_high => (l_low - r_high, l_high - r_low),
                BinaryOp::SubChecked => (0, l_high),
                _ => (0, 1),
            }
        }
        _ => (min, max),
    })
}

fn out_of_range(name: &str, op: &str, (min, max): (i64, i64)) -> String {
    format!(
        "{name}: `{op}` may go outside {min} to {max}; add `need name <= limit` lines for its inputs"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIT: &str = "# note\nfit_price(price, min, max)\n  need min <= max\n\n  price < min -> min\n  price > max -> max\n  -> price\n";

    #[test]
    fn new_features_reject_misuse() {
        let err = |src: &str| crate::parse_module(src).unwrap_err();
        assert!(err("f(a, b) can be negative\n  -> a / b\n").contains("divide by 0"));
        assert!(err("f(a)\n  -> -5\n").contains("can be negative"));
        assert!(err("f(a)\n  a > 1 -> \"A\"\n  -> 2\n").contains("text answer"));
        assert!(err("f(a) can be negative\n  -> \"A\"\n").contains("text answers cannot"));
        assert!(err("g(a)\n  -> \"A\"\n\nf(a)\n  -> g(a)\n").contains("only a page"));
        assert!(err("page P\n  save a = 0 can be negative\n  show a + 1\n").contains("in a rule"));
        assert!(err("f(a, b) can be negative\n  -> a + b\n").contains("may go outside"));
        assert!(err("f(a, b)\n  -> round(a + b)\n").contains("round takes a division"));
        assert!(err("f(a, b)\n  -> a mod b\n").contains("divide by 0"));
        assert!(err("f(p[3])\n  -> p[4]\n").contains("items 1 to 3"));
        assert!(err("f(p[0])\n  -> 1\n").contains("size from 1"));
        assert!(err("f(a) with 1 decimal\n  -> a * 1.25\n").contains("more than 1"));
        assert!(err("page P\n  save a = 0\n  show a if a > 1\n").contains("only a text"));
    }

    #[test]
    fn number_forms_mix() {
        let out = translate("f(a, b) can be negative\n  need b >= 1\n  -> round(a / b)\n").unwrap();
        assert!(out.contains("2 *% (a) / (b)"), "{out}");
        let out = translate("f(a) with 2 decimals\n  -> round(a / 3) + a mod 2\n").unwrap();
        assert!(
            out.contains("+% 1) / 200") && out.contains("% 200"),
            "{out}"
        );
        let module = crate::parse_module(
            "g(a) with 2 decimals\n  need a <= 10\n  -> a\n\nf(a)\n  need a <= 10\n  -> g(a)\n",
        )
        .unwrap();
        let answer = format!("{:?}", module.contracts[1].ensures);
        assert!(
            answer.contains("Div") && answer.contains("Number(100)"),
            "{answer}"
        );
        let module = crate::parse_module(
            "page P\n  save a = 0 with 2 decimals\n  save b = 0\n  show a + b\n",
        )
        .unwrap();
        let shown = format!("{:?}", module.pages[0].elements);
        assert!(shown.contains("Binary(MulWrap, Ident(\"b\"), Number(100))"), "{shown}");
    }

    #[test]
    fn new_features_translate() {
        let module =
            crate::parse_module("f(p[2]) with 2 decimals\n  need p <= 10\n  -> sum(p) * 1.5\n")
                .unwrap();
        let contract = &module.contracts[0];
        assert_eq!(contract.decimals, 2);
        assert_eq!(contract.params.len(), 2);
        assert_eq!(contract.requires.len(), 1);
        let module =
            crate::parse_module("f(x) can be negative\n  need x >= -10\n  x < -5 -> -5\n  -> -x\n")
                .unwrap();
        assert_eq!(module.contracts[0].requires.len(), 1);
        let module =
            crate::parse_module("g(s)\n  s > 5 -> \"Hi\"\n  s > 2 -> \"Lo\"\n  -> \"Hi\"\n")
                .unwrap();
        assert_eq!(module.contracts[0].labels, ["Hi", "Lo"]);
        let out = translate("f(a, b) can be negative\n  -> a - b\n").unwrap();
        assert!(out.contains("-> i32") && out.contains("a -% b"), "{out}");
        let out = translate("f(a, b)\n  need b >= 1\n  -> round(a / b) + a mod b\n").unwrap();
        assert!(out.contains("(((a) +% (b) / 2) / (b)) +% a % b"), "{out}");
        let out =
            translate("page P\n  save p[2] = 0\n  ask p \"Price\"\n  show \"Big\" if p[1] > 9\n")
                .unwrap();
        assert!(
            out.contains("input p_1 placeholder \"Price 1\";    input p_2"),
            "{out}"
        );
        assert!(out.contains("when p_1 > 9 show \"Big\";"), "{out}");
    }

    #[test]
    fn named_values_are_inlined() {
        let src = "f(total, used)\n  need used <= total\n  need total <= 1000\n  left = total - used\n  twice = left * 2\n  left is 0 -> 0\n  -> twice\n";
        let out = translate(src).unwrap();
        assert!(out.contains("(total -! used) == 0"), "{out}");
        assert!(out.contains("((total -! used) *% 2)"), "{out}");
        crate::parse_module(src).unwrap();
        assert!(
            translate("f(x)\n  a = x\n  a = x\n  -> a\n")
                .unwrap_err()
                .contains("already set")
        );
        assert!(
            translate("f(x)\n  x > 1 -> 1\n  a = x\n  -> a\n")
                .unwrap_err()
                .contains("before")
        );
        assert!(translate("f(x)\n  x <= 1 -> 1\n  -> x\n").is_ok());
    }

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
                .contains("may go outside")
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
                .contains("may go outside")
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
