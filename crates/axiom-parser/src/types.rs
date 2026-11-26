//! Types (issue #226): choices, records and `match`.
//!
//! ```text
//! type Size = small | medium | large
//!
//! type Order
//!   price
//!   quantity
//!   size: Size
//!
//! shipping(size: Size, weight)
//!   need weight <= 100000
//!   match size
//!     small -> 3000
//!     medium -> 5000
//!     large -> 5000 + weight / 1000 * 100
//!
//! total(order: Order, weight)
//!   need order.price <= 10000000
//!   need order.quantity <= 100
//!   need weight <= 100000
//!   always -> order.price * order.quantity + shipping(order.size, weight)
//! ```
//!
//! Before translation every type is written out with plain numbers, line for
//! line, so a rule with types is proven like any rule:
//!
//! - a choice is its place, from 0 (`large` is 2), and a choice input gets a
//!   hidden `need size <= 2` on its name line;
//! - a record input is one input per field (`order.price` is `order__price`),
//!   and a record passed to another rule is its fields in order;
//! - `match size` becomes `size is 0 -> ...` lines; the last case becomes
//!   `else`, so a `match` must name every value of the choice (or end with
//!   `else`).

use crate::ast::ChoiceType;
use crate::simple::{check_name, strip_comment};

pub(crate) struct Expanded {
    pub source: String,
    /// For each rule, its choice inputs (after records are written out).
    pub choices: Vec<(String, String, ChoiceType)>,
}

/// Marks a `need` the name line adds (`name(a) __need a <= 2`); read by
/// `simple::header`. Users cannot write it: names cannot start with `_`.
pub(crate) const NEED_MARK: &str = " __need ";

/// Words rule lines give a meaning to; a choice value cannot be one.
const RESERVED: &[&str] = &[
    "need", "promise", "hint", "always", "else", "start", "repeat", "match", "answer", "value",
    "is", "not", "and", "or", "mod", "min", "max", "sum", "count", "round", "up", "position",
    "true", "false", "yes", "no",
];

#[derive(Clone)]
enum Def {
    Choice(Vec<String>),
    /// Field name and the choice type it holds, if any.
    Record(Vec<(String, Option<String>)>),
}

struct Types {
    defs: Vec<(String, Def)>,
}

impl Types {
    fn get(&self, name: &str) -> Option<&Def> {
        self.defs.iter().find(|(n, _)| n == name).map(|(_, d)| d)
    }

    fn choice(&self, name: &str) -> Option<ChoiceType> {
        match self.get(name)? {
            Def::Choice(values) => Some(ChoiceType {
                name: name.to_string(),
                values: values.clone(),
            }),
            Def::Record(_) => None,
        }
    }

    /// The choice that has `value`, and its place.
    fn value(&self, value: &str) -> Option<(&str, usize)> {
        self.defs.iter().find_map(|(name, def)| match def {
            Def::Choice(values) => values
                .iter()
                .position(|v| v == value)
                .map(|at| (name.as_str(), at)),
            Def::Record(_) => None,
        })
    }
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

fn error(line: usize, message: impl std::fmt::Display) -> String {
    format!("line {line}: {message}")
}

fn indent(text: &str) -> usize {
    text.len() - text.trim_start().len()
}

/// Whether the line opens a top-level block (unindented, not blank).
fn is_top(text: &str) -> bool {
    !text.trim().is_empty() && !text.starts_with(' ') && !text.starts_with('\t')
}

pub(crate) fn expand(source: &str) -> Result<Option<Expanded>, String> {
    let mut lines: Vec<String> = source.lines().map(String::from).collect();
    let has_types = lines
        .iter()
        .any(|l| is_top(strip_comment(l)) && strip_comment(l).trim_start().starts_with("type "));
    let has_match = lines
        .iter()
        .any(|l| strip_comment(l).trim().starts_with("match "));
    if !has_types && !has_match {
        return Ok(None);
    }
    let types = read_types(&mut lines)?;
    let signatures = signatures(&lines, &types)?;
    let mut choices = Vec::new();

    let mut index = 0;
    while index < lines.len() {
        let text = strip_comment(&lines[index]).to_string();
        let mut end = index + 1;
        while end < lines.len() && !is_top(strip_comment(&lines[end])) {
            end += 1;
        }
        let head = text.trim();
        let is_rule = is_top(&text)
            && !head.starts_with("page ")
            && !head.starts_with("from ")
            && !head.starts_with("use ");
        if !is_rule {
            if !is_top(&text) || head.starts_with("page ") {
                // Pages keep plain numbers; a `match` belongs in a rule.
                for (offset, line) in lines[index..end].iter().enumerate() {
                    if strip_comment(line).trim().starts_with("match ") {
                        return Err(error(index + offset + 1, "`match` works only in a rule"));
                    }
                }
            }
            index = end;
            continue;
        }
        let rule = rule_block(&lines[index..end], index + 1, &types, &signatures)?;
        for (offset, line) in rule.lines.into_iter().enumerate() {
            lines[index + offset] = line;
        }
        for (param, choice) in rule.choices {
            choices.push((rule.name.clone(), param, choice));
        }
        index = end;
    }
    Ok(Some(Expanded {
        source: lines.join("\n"),
        choices,
    }))
}

/// Reads every `type` block and blanks its lines.
fn read_types(lines: &mut [String]) -> Result<Types, String> {
    let mut defs: Vec<(String, Def, usize)> = Vec::new();
    let mut index = 0;
    while index < lines.len() {
        let text = strip_comment(&lines[index]).to_string();
        let Some(rest) = text.strip_prefix("type ") else {
            index += 1;
            continue;
        };
        let line_no = index + 1;
        lines[index].clear();
        let (name, def) = if let Some((name, values)) = rest.split_once('=') {
            let values: Vec<String> = values.split('|').map(|v| v.trim().to_string()).collect();
            if values.iter().any(String::is_empty) {
                return Err(error(
                    line_no,
                    "write a choice as `type Size = small | medium | large`",
                ));
            }
            for value in &values {
                check_name(value, line_no)?;
                if RESERVED.contains(&value.as_str()) {
                    return Err(error(
                        line_no,
                        format!("`{value}` is a word rules use; choose another value name"),
                    ));
                }
                if values.iter().filter(|v| *v == value).count() > 1 {
                    return Err(error(line_no, format!("`{value}` is written twice")));
                }
            }
            index += 1;
            if index < lines.len()
                && !is_top(strip_comment(&lines[index]))
                && !strip_comment(&lines[index]).trim().is_empty()
            {
                return Err(error(
                    index + 1,
                    "a choice has no lines below it; put its values after `=`",
                ));
            }
            (name.trim().to_string(), Def::Choice(values))
        } else {
            let mut fields: Vec<(String, Option<String>)> = Vec::new();
            index += 1;
            while index < lines.len() && !is_top(strip_comment(&lines[index])) {
                let field_line = index + 1;
                let field = strip_comment(&lines[index]).trim().to_string();
                lines[index].clear();
                index += 1;
                if field.is_empty() {
                    continue;
                }
                let (field, ty) = match field.split_once(':') {
                    Some((f, t)) => (f.trim().to_string(), Some(t.trim().to_string())),
                    None => (field, None),
                };
                check_name(&field, field_line)?;
                if fields.iter().any(|(f, _)| *f == field) {
                    return Err(error(field_line, format!("`{field}` is written twice")));
                }
                if field.contains("__") {
                    return Err(error(field_line, format!("`{field}` cannot contain `__`")));
                }
                fields.push((
                    field,
                    ty.map(|t| (t, field_line)).map(|(t, l)| format!("{t}@{l}")),
                ));
            }
            if fields.is_empty() {
                return Err(error(
                    line_no,
                    "add the record's fields on the lines below, one name per line",
                ));
            }
            (rest.trim().to_string(), Def::Record(fields))
        };
        check_name(&name, line_no)?;
        if defs.iter().any(|(n, _, _)| *n == name) {
            return Err(error(line_no, format!("type `{name}` is declared twice")));
        }
        defs.push((name, def, line_no));
    }

    // Field types may name a choice declared later.
    let names: Vec<(String, bool)> = defs
        .iter()
        .map(|(n, d, _)| (n.clone(), matches!(d, Def::Choice(_))))
        .collect();
    for (_, def, _) in &mut defs {
        if let Def::Record(fields) = def {
            for (_, ty) in fields.iter_mut() {
                let Some(written) = ty.take() else { continue };
                let (t, l) = written.rsplit_once('@').unwrap();
                let l: usize = l.parse().unwrap();
                match names.iter().find(|(n, _)| n == t) {
                    Some((_, true)) => *ty = Some(t.to_string()),
                    Some((_, false)) => {
                        return Err(error(l, format!("a field cannot be a record (`{t}`) yet")));
                    }
                    None => return Err(error(l, format!("there is no type `{t}`"))),
                }
            }
        }
    }
    // A value names one thing in the file.
    let mut seen: Vec<(&str, &str)> = Vec::new();
    for (name, def, line_no) in &defs {
        if let Def::Choice(values) = def {
            for value in values {
                if let Some((_, other)) = seen.iter().find(|(v, _)| v == value) {
                    return Err(error(
                        *line_no,
                        format!("`{value}` is already a value of `{other}`"),
                    ));
                }
                if defs.iter().any(|(n, _, _)| n == value) {
                    return Err(error(*line_no, format!("`{value}` is also a type name")));
                }
                seen.push((value, name));
            }
        }
    }
    Ok(Types {
        defs: defs.into_iter().map(|(n, d, _)| (n, d)).collect(),
    })
}

/// The type of each input of each rule: `None` for a number.
type Signature = (String, Vec<Option<String>>);

fn signatures(lines: &[String], types: &Types) -> Result<Vec<Signature>, String> {
    let mut out = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        let text = strip_comment(line);
        let head = text.trim();
        if !is_top(text)
            || ["page ", "from ", "use "]
                .iter()
                .any(|p| head.starts_with(p))
        {
            continue;
        }
        let Some((name, rest)) = head.split_once('(') else {
            continue;
        };
        let Some((params, _)) = rest.split_once(')') else {
            continue;
        };
        let mut tys = Vec::new();
        for param in params.split(',').map(str::trim).filter(|p| !p.is_empty()) {
            tys.push(match param.split_once(':') {
                Some((_, ty)) => {
                    let ty = ty.trim();
                    if types.get(ty).is_none() {
                        return Err(error(index + 1, format!("there is no type `{ty}`")));
                    }
                    Some(ty.to_string())
                }
                None => None,
            });
        }
        out.push((name.trim().to_string(), tys));
    }
    Ok(out)
}

/// What a name means inside one rule.
enum Local {
    /// A record input: its type and fields (name, written-out input, choice).
    Record(String, Vec<(String, String, Option<String>)>),
    /// A choice input (or a record field written out) and its type.
    Choice(String),
}

struct RuleOut {
    name: String,
    lines: Vec<String>,
    choices: Vec<(String, ChoiceType)>,
}

fn rule_block(
    block: &[String],
    first_line: usize,
    types: &Types,
    signatures: &[Signature],
) -> Result<RuleOut, String> {
    let head = strip_comment(&block[0]).trim().to_string();
    let (name, rest) = head
        .split_once('(')
        .ok_or_else(|| error(first_line, "expected a name like `price(a, b)`"))?;
    let (params, suffix) = rest
        .split_once(')')
        .ok_or_else(|| error(first_line, "expected `)` at the end of the name line"))?;
    let name = name.trim().to_string();
    let suffix = suffix.trim();

    let mut locals: Vec<(String, Local)> = Vec::new();
    let mut written = Vec::new();
    let mut choices = Vec::new();
    for param in params.split(',').map(str::trim).filter(|p| !p.is_empty()) {
        let Some((param, ty)) = param.split_once(':') else {
            if types.value(param).is_some() {
                return Err(error(
                    first_line,
                    format!("`{param}` is a value of a type; give the input another name"),
                ));
            }
            written.push(param.to_string());
            continue;
        };
        let (param, ty) = (param.trim(), ty.trim());
        if param.contains('[') {
            return Err(error(first_line, "a list cannot hold a type yet"));
        }
        check_name(param, first_line)?;
        if types.value(param).is_some() {
            return Err(error(
                first_line,
                format!("`{param}` is a value of a type; give the input another name"),
            ));
        }
        match types.get(ty) {
            Some(Def::Choice(_)) => {
                written.push(param.to_string());
                choices.push((param.to_string(), types.choice(ty).unwrap()));
                locals.push((param.to_string(), Local::Choice(ty.to_string())));
            }
            Some(Def::Record(fields)) => {
                let mut flat = Vec::new();
                for (field, choice) in fields {
                    let input = format!("{param}__{field}");
                    written.push(input.clone());
                    if let Some(choice) = choice {
                        choices.push((input.clone(), types.choice(choice).unwrap()));
                    }
                    flat.push((field.clone(), input, choice.clone()));
                }
                locals.push((param.to_string(), Local::Record(ty.to_string(), flat)));
            }
            None => return Err(error(first_line, format!("there is no type `{ty}`"))),
        }
    }
    if !choices.is_empty() && suffix.contains("decimal") {
        return Err(error(
            first_line,
            "a rule with a choice input cannot have decimals yet",
        ));
    }
    let signed = suffix.contains("can be negative");
    let mut needs = String::new();
    for (input, choice) in &choices {
        if signed {
            needs += &format!("{NEED_MARK}{input} >= 0");
        }
        needs += &format!("{NEED_MARK}{input} <= {}", choice.values.len() - 1);
    }
    let gap = if suffix.is_empty() { "" } else { " " };
    let mut out = vec![String::new(); block.len()];
    out[0] = format!("{name}({}){gap}{suffix}{needs}", written.join(", "));

    let rewrite = |text: &str, line: usize| rewrite(text, line, &locals, types, signatures);
    let mut offset = 1;
    let mut matched = false;
    while offset < block.len() {
        let line = first_line + offset;
        let text = strip_comment(&block[offset]).to_string();
        let trimmed = text.trim();
        if trimmed.is_empty() {
            offset += 1;
            continue;
        }
        if let Some(subject) = trimmed.strip_prefix("match ") {
            if matched {
                return Err(error(line, "write one `match` in a rule"));
            }
            matched = true;
            let at = indent(&text);
            let mut end = offset + 1;
            while end < block.len() {
                let t = strip_comment(&block[end]);
                if !t.trim().is_empty() && indent(t) <= at {
                    break;
                }
                end += 1;
            }
            let arms = match_arms(
                subject.trim(),
                &block[offset + 1..end],
                line,
                &locals,
                types,
                &rewrite,
            )?;
            for (k, arm) in arms.into_iter().enumerate() {
                out[offset + 1 + k] = arm;
            }
            offset = end;
            continue;
        }
        if matched && trimmed.contains("->") {
            return Err(error(
                line,
                "a `match` gives the last answers; put other `->` lines above it",
            ));
        }
        if let Some((left, _)) = trimmed.split_once('=')
            && !trimmed.contains("->")
            && !left.ends_with(['<', '>', '!'])
        {
            let left = left.trim();
            if types.value(left).is_some() {
                return Err(error(
                    line,
                    format!("`{left}` is a value of a type; choose another name"),
                ));
            }
            if locals.iter().any(|(n, _)| n == left) {
                return Err(error(line, format!("`{left}` is already an input")));
            }
        }
        let start = &text[..indent(&text)];
        out[offset] = format!("{start}{}", rewrite(trimmed, line)?);
        offset += 1;
    }
    Ok(RuleOut {
        name,
        lines: out,
        choices,
    })
}

/// The `match` cases as `->` lines, one per source line (blank where none).
fn match_arms(
    subject: &str,
    arms: &[String],
    line: usize,
    locals: &[(String, Local)],
    types: &Types,
    rewrite: &dyn Fn(&str, usize) -> Result<String, String>,
) -> Result<Vec<String>, String> {
    let found = match subject.split_once('.') {
        None => locals.iter().find_map(|(n, l)| match l {
            Local::Choice(ty) if n == subject => Some((subject.to_string(), ty.clone())),
            _ => None,
        }),
        Some((record, field)) => locals.iter().find_map(|(n, l)| match l {
            Local::Record(_, fields) if n == record.trim() => fields
                .iter()
                .find(|(f, _, _)| f == field.trim())
                .and_then(|(_, input, choice)| Some((input.clone(), choice.clone()?))),
            _ => None,
        }),
    };
    let (input, choice) = found.ok_or_else(|| {
        error(
            line,
            format!("`match {subject}`: `{subject}` is not an input that holds a choice"),
        )
    })?;
    let values = match types.get(&choice) {
        Some(Def::Choice(values)) => values.clone(),
        _ => unreachable!("choice inputs have choice types"),
    };

    let mut out = vec![String::new(); arms.len()];
    let mut covered: Vec<&str> = Vec::new();
    let mut cases: Vec<(usize, Option<Vec<usize>>, String)> = Vec::new();
    for (k, raw) in arms.iter().enumerate() {
        let arm_line = line + 1 + k;
        let text = strip_comment(raw).trim();
        if text.is_empty() {
            continue;
        }
        let Some((pattern, value)) = text.split_once("->") else {
            return Err(error(
                arm_line,
                format!("write a case as `{} -> value`", values[0]),
            ));
        };
        if cases.last().is_some_and(|(_, p, _)| p.is_none()) {
            return Err(error(arm_line, "`else` must be the last case"));
        }
        let pattern = pattern.trim();
        let places = if pattern == "else" {
            None
        } else {
            let mut places = Vec::new();
            for word in pattern
                .split(" or ")
                .flat_map(|p| p.split(','))
                .map(str::trim)
            {
                let Some(at) = values.iter().position(|v| v == word) else {
                    return Err(error(
                        arm_line,
                        format!(
                            "`{word}` is not a value of `{choice}` ({})",
                            values.join(", ")
                        ),
                    ));
                };
                if covered.contains(&word) {
                    return Err(error(arm_line, format!("`{word}` already has a case")));
                }
                covered.push(&values[at]);
                places.push(at);
            }
            Some(places)
        };
        cases.push((k, places, value.trim().to_string()));
    }
    let missing: Vec<&str> = values
        .iter()
        .map(String::as_str)
        .filter(|v| !covered.contains(v))
        .collect();
    match cases.last() {
        None => {
            return Err(error(
                line,
                format!("add a case per value of `{choice}` below `match`"),
            ));
        }
        Some((_, None, _)) if missing.is_empty() => {
            return Err(error(
                line + 1 + cases.last().unwrap().0,
                "every value already has a case; remove `else`",
            ));
        }
        Some((_, Some(_), _)) if !missing.is_empty() => {
            return Err(error(
                line,
                format!(
                    "`match {subject}` has no case for {}; add them or end with `else -> value`",
                    missing
                        .iter()
                        .map(|m| format!("`{m}`"))
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            ));
        }
        _ => {}
    }
    let last = cases.len() - 1;
    for (n, (k, places, value)) in cases.iter().enumerate() {
        let arm_line = line + 1 + k;
        let value = rewrite(value, arm_line)?;
        let condition = match places {
            _ if n == last => "else".to_string(),
            Some(places) => places
                .iter()
                .map(|at| format!("{input} is {at}"))
                .collect::<Vec<_>>()
                .join(" or "),
            None => unreachable!("`else` is last"),
        };
        let start = &arms[*k][..indent(&arms[*k])];
        out[*k] = format!("{start}{condition} -> {value}");
    }
    Ok(out)
}

/// One rule line with fields written out, values as numbers, and records
/// passed to rules as their fields; checks the types of rule calls.
fn rewrite(
    text: &str,
    line: usize,
    locals: &[(String, Local)],
    types: &Types,
    signatures: &[Signature],
) -> Result<String, String> {
    check_calls(text, line, locals, types, signatures)?;
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    let mut quoted = false;
    while i < chars.len() {
        let c = chars[i];
        if c == '"' {
            quoted = !quoted;
        }
        if quoted || !is_word(c) {
            out.push(c);
            i += 1;
            continue;
        }
        let start = i;
        while i < chars.len() && is_word(chars[i]) {
            i += 1;
        }
        let word: String = chars[start..i].iter().collect();
        let after_number = start > 0 && chars[start - 1] == '.';
        if word.starts_with(|c: char| c.is_ascii_digit()) || after_number {
            out += &word;
            continue;
        }
        match locals.iter().find(|(n, _)| *n == word).map(|(_, l)| l) {
            Some(Local::Record(ty, fields)) => {
                if chars.get(i) == Some(&'.') {
                    let from = i + 1;
                    let mut to = from;
                    while to < chars.len() && is_word(chars[to]) {
                        to += 1;
                    }
                    let field: String = chars[from..to].iter().collect();
                    let Some((_, input, _)) = fields.iter().find(|(f, _, _)| *f == field) else {
                        let names: Vec<&str> = fields.iter().map(|(f, _, _)| f.as_str()).collect();
                        return Err(error(
                            line,
                            format!("`{ty}` has no field `{field}` ({})", names.join(", ")),
                        ));
                    };
                    out += input;
                    i = to;
                } else {
                    let before = chars[..start].iter().rev().find(|c| !c.is_whitespace());
                    let next = chars[i..].iter().find(|c| !c.is_whitespace());
                    if !matches!(before, Some('(' | ',')) || !matches!(next, Some(')' | ',')) {
                        let field = &fields[0].0;
                        return Err(error(
                            line,
                            format!(
                                "`{word}` is a record; use one of its fields, like `{word}.{field}`, or pass it to a rule"
                            ),
                        ));
                    }
                    let inputs: Vec<&str> = fields.iter().map(|(_, n, _)| n.as_str()).collect();
                    out += &inputs.join(", ");
                }
            }
            Some(Local::Choice(_)) => out += &word,
            None => {
                if chars.get(i) == Some(&'.')
                    && chars
                        .get(i + 1)
                        .is_some_and(|c| is_word(*c) && !c.is_ascii_digit())
                {
                    return Err(error(line, format!("`{word}` is not a record input")));
                }
                match types.value(&word) {
                    Some((_, at)) => out += &at.to_string(),
                    None => out += &word,
                }
            }
        }
    }
    Ok(out)
}

/// What an argument is, as far as types go.
enum Arg {
    Record(String),
    Choice(String),
    Number,
}

fn arg_type(arg: &str, locals: &[(String, Local)], types: &Types) -> Arg {
    let arg = arg.trim();
    if let Some((record, field)) = arg.split_once('.')
        && let Some((_, Local::Record(_, fields))) = locals.iter().find(|(n, _)| n == record)
        && let Some((_, _, Some(choice))) = fields.iter().find(|(f, _, _)| f == field)
    {
        return Arg::Choice(choice.clone());
    }
    match locals.iter().find(|(n, _)| n == arg).map(|(_, l)| l) {
        Some(Local::Record(ty, _)) => Arg::Record(ty.clone()),
        Some(Local::Choice(ty)) => Arg::Choice(ty.clone()),
        None => match types.value(arg) {
            Some((choice, _)) => Arg::Choice(choice.to_string()),
            None => Arg::Number,
        },
    }
}

/// Calls to rules of this file must pass a record where one is taken, and
/// a value of the right choice where a choice is taken.
fn check_calls(
    text: &str,
    line: usize,
    locals: &[(String, Local)],
    types: &Types,
    signatures: &[Signature],
) -> Result<(), String> {
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if !is_word(chars[i]) || (i > 0 && (is_word(chars[i - 1]) || chars[i - 1] == '.')) {
            i += 1;
            continue;
        }
        let start = i;
        while i < chars.len() && is_word(chars[i]) {
            i += 1;
        }
        if chars.get(i) != Some(&'(') {
            continue;
        }
        let callee: String = chars[start..i].iter().collect();
        // Split the arguments at top-level commas.
        let mut depth = 0;
        let mut args = vec![String::new()];
        for &c in &chars[i + 1..] {
            match c {
                '(' => depth += 1,
                ')' if depth == 0 => break,
                ')' => depth -= 1,
                ',' if depth == 0 => {
                    args.push(String::new());
                    continue;
                }
                _ => {}
            }
            args.last_mut().unwrap().push(c);
        }
        if args.len() == 1 && args[0].trim().is_empty() {
            args.clear();
        }
        let Some((_, params)) = signatures.iter().find(|(n, _)| *n == callee) else {
            // A rule from another file (or `min`, `sum`): its types are not known here.
            if let Some(arg) = args
                .iter()
                .find(|a| matches!(arg_type(a, locals, types), Arg::Record(_)))
            {
                return Err(error(
                    line,
                    format!(
                        "`{}` is a record; `{callee}` is not a rule of this file, so pass its fields one by one",
                        arg.trim()
                    ),
                ));
            }
            continue;
        };
        if args.len() != params.len() {
            return Err(error(
                line,
                format!(
                    "`{callee}` takes {} inputs, got {}",
                    params.len(),
                    args.len()
                ),
            ));
        }
        for (n, (arg, param)) in args.iter().zip(params).enumerate() {
            let place = n + 1;
            match (arg_type(arg, locals, types), param) {
                (Arg::Record(given), Some(want)) if given == *want => {}
                (Arg::Choice(given), Some(want)) if given == *want => {}
                (Arg::Number, Some(want)) if matches!(types.get(want), Some(Def::Choice(_))) => {}
                (Arg::Number | Arg::Choice(_), None) => {}
                (given, want) => {
                    let given = match given {
                        Arg::Record(t) | Arg::Choice(t) => format!("a `{t}`"),
                        Arg::Number => "a number".to_string(),
                    };
                    let want = match want {
                        Some(t) => format!("a `{t}`"),
                        None => "a number".to_string(),
                    };
                    return Err(error(
                        line,
                        format!("`{callee}` takes {want} as input {place}, got {given}"),
                    ));
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::parse_module;

    const TYPES: &str =
        "type Size = small | medium | large\n\ntype Order\n  price\n  quantity\n  size: Size\n\n";

    fn err(source: &str) -> String {
        parse_module(source).unwrap_err()
    }

    #[test]
    fn a_choice_is_its_place_and_its_input_is_bounded() {
        let module = parse_module(&format!(
            "{TYPES}fee(size: Size)\n  match size\n    small -> 1\n    medium -> 2\n    large -> 3\n"
        ))
        .unwrap();
        let fee = &module.contracts[0];
        assert_eq!(fee.choices[0].0, "size");
        assert_eq!(fee.choices[0].1.values, ["small", "medium", "large"]);
        assert_eq!(fee.requires.len(), 1);
    }

    #[test]
    fn a_record_is_one_input_per_field() {
        let module = parse_module(&format!(
            "{TYPES}cost(order: Order)\n  need order.price <= 100\n  need order.quantity <= 10\n  always -> order.price * order.quantity\n\ntotal(order: Order)\n  need order.price <= 100\n  need order.quantity <= 10\n  order.size is large -> cost(order) + 5\n  else -> cost(order)\n"
        ))
        .unwrap();
        let names: Vec<&str> = module.contracts[0]
            .params
            .iter()
            .map(|p| p.name.as_str())
            .collect();
        assert_eq!(names, ["order__price", "order__quantity", "order__size"]);
        assert_eq!(module.contracts[1].choices[0].0, "order__size");
    }

    #[test]
    fn errors_keep_the_users_line() {
        let e = err(&format!(
            "{TYPES}fee(size: Size)\n  match size\n    small -> 1\n    large -> 3\n"
        ));
        assert!(e.starts_with("line 9:") && e.contains("`medium`"), "{e}");
        let e = err(&format!(
            "{TYPES}fee(size: Size)\n  match size\n    small -> 1\n    huge -> 3\n"
        ));
        assert!(
            e.starts_with("line 11:") && e.contains("not a value of `Size`"),
            "{e}"
        );
        let e = err(&format!("{TYPES}fee(o: Order)\n  always -> o.weight\n"));
        assert!(
            e.starts_with("line 9:") && e.contains("no field `weight`"),
            "{e}"
        );
        let e = err(&format!("{TYPES}fee(o: Order)\n  always -> o + 1\n"));
        assert!(e.contains("is a record"), "{e}");
        let e = err(&format!("{TYPES}fee(o: Thing)\n  always -> 1\n"));
        assert!(e.contains("no type `Thing`"), "{e}");
    }

    #[test]
    fn calls_check_types() {
        let base = format!(
            "{TYPES}cost(order: Order)\n  need order.price <= 100\n  need order.quantity <= 10\n  always -> order.price * order.quantity\n\nfee(size: Size)\n  always -> size\n\n"
        );
        let e = err(&format!(
            "{base}bad(a)\n  need a <= 2\n  always -> cost(a)\n"
        ));
        assert!(
            e.contains("`cost` takes a `Order` as input 1, got a number"),
            "{e}"
        );
        let e = err(&format!("{base}bad(o: Order)\n  always -> fee(o)\n"));
        assert!(
            e.contains("takes a `Size` as input 1, got a `Order`"),
            "{e}"
        );
        parse_module(&format!(
            "{base}good(o: Order)\n  always -> fee(o.size) + fee(large)\n"
        ))
        .unwrap();
        let e = err(&format!("{base}bad(o: Order)\n  always -> min(o)\n"));
        assert!(e.contains("pass its fields one by one"), "{e}");
    }

    #[test]
    fn match_cases_cover_once() {
        let rule = |cases: &str| format!("{TYPES}fee(size: Size)\n  match size\n{cases}");
        assert!(
            err(&rule("    small -> 1\n    small -> 2\n    else -> 3\n"))
                .contains("already has a case")
        );
        assert!(
            err(&rule("    small, medium, large -> 1\n    else -> 3\n")).contains("remove `else`")
        );
        assert!(err(&rule("    else -> 3\n    small -> 1\n")).contains("must be the last case"));
        parse_module(&rule("    small or medium -> 1\n    else -> 3\n")).unwrap();
    }

    #[test]
    fn names_do_not_clash() {
        assert!(err("type Size = small | small\nf(a)\n  always -> a\n").contains("twice"));
        assert!(
            err("type A = x | y\ntype B = y | z\nf(a)\n  always -> a\n")
                .contains("already a value of `A`")
        );
        assert!(
            err("type Size = small | big\nf(small)\n  always -> small\n")
                .contains("give the input another name")
        );
        assert!(
            err("type Size = small | big\nf(a)\n  big = a\n  always -> big\n")
                .contains("choose another name")
        );
        assert!(
            err("type Size = small | big\nf(s: Size) with 2 decimals\n  always -> s\n")
                .contains("decimals")
        );
    }
}
