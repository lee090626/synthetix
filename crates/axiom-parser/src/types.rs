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
    /// Rules whose every answer is a value of one choice.
    pub answers: Vec<(String, ChoiceType)>,
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
    /// Field name and the type it holds (a choice or a record), if any.
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

    /// Every number a record holds, in field order: the dotted path
    /// (`address.zip`) and the choice it holds, if any. Records inside the
    /// record are written out in place.
    fn leaves(&self, record: &str) -> Vec<(String, Option<String>)> {
        let Some(Def::Record(fields)) = self.get(record) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for (field, ty) in fields {
            match ty.as_deref().map(|t| (t, self.get(t))) {
                Some((t, Some(Def::Record(_)))) => {
                    for (path, choice) in self.leaves(t) {
                        out.push((format!("{field}.{path}"), choice));
                    }
                }
                _ => out.push((field.clone(), ty.clone())),
            }
        }
        out
    }

    /// The type at `path` inside `record` (`None` for a number), or `Err`
    /// when the record has no such field.
    fn at(&self, record: &str, path: &str) -> Result<Option<String>, ()> {
        let mut ty = Some(record.to_string());
        for field in path.split('.') {
            let Some(Def::Record(fields)) = ty.as_deref().and_then(|t| self.get(t)) else {
                return Err(());
            };
            ty = fields.iter().find(|(f, _)| f == field).ok_or(())?.1.clone();
        }
        Ok(ty)
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
    let mut answers = Vec::new();

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
        if let Some(choice) = answer_choice(&lines[index..end], &types) {
            answers.push((rule_name(head), types.choice(&choice).unwrap()));
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
        answers,
    }))
}

fn rule_name(head: &str) -> String {
    head.split_once('(')
        .map_or(head, |(n, _)| n)
        .trim()
        .to_string()
}

/// The choice every `->` answer of the rule is a value of, if one is.
fn answer_choice(block: &[String], types: &Types) -> Option<String> {
    let mut found: Option<&str> = None;
    for line in &block[1..] {
        let Some((_, value)) = strip_comment(line).split_once("->") else {
            continue;
        };
        let (choice, _) = types.value(value.trim())?;
        if found.is_some_and(|f| f != choice) {
            return None;
        }
        found = Some(choice);
    }
    found.map(String::from)
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
                    Some(_) => *ty = Some(format!("{t}@{l}")),
                    None => return Err(error(l, format!("there is no type `{t}`"))),
                }
            }
        }
    }
    // A record may hold another record, but not itself.
    for (name, def, _) in &defs {
        let Def::Record(fields) = def else { continue };
        for (_, ty) in fields {
            let Some((t, l)) = ty.as_deref().and_then(|t| t.rsplit_once('@')) else {
                continue;
            };
            let mut path = vec![name.as_str(), t];
            if holds(&defs, t, name, &mut path) {
                return Err(error(
                    l.parse::<usize>().unwrap(),
                    format!("a record cannot hold itself yet ({})", path.join(" holds ")),
                ));
            }
        }
    }
    for (_, def, _) in &mut defs {
        if let Def::Record(fields) = def {
            for (_, ty) in fields.iter_mut() {
                if let Some(t) = ty.as_mut()
                    && let Some((name, _)) = t.rsplit_once('@')
                {
                    *t = name.to_string();
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

/// Whether record `from` holds `target`, directly or through its fields;
/// `path` gets the records on the way.
fn holds<'a>(
    defs: &'a [(String, Def, usize)],
    from: &'a str,
    target: &str,
    path: &mut Vec<&'a str>,
) -> bool {
    let Some((_, Def::Record(fields), _)) = defs.iter().find(|(n, _, _)| n == from) else {
        return false;
    };
    for (_, ty) in fields {
        let Some(t) = ty.as_deref() else { continue };
        let t = t.rsplit_once('@').map_or(t, |(t, _)| t);
        if t == target {
            return true;
        }
        if path.contains(&t) {
            continue;
        }
        path.push(t);
        if holds(defs, t, target, path) {
            return true;
        }
        path.pop();
    }
    false
}

/// The type of each input of each rule (`None` for a number), the choice
/// its answers are, if any, and its decimal places.
type Signature = (String, Vec<Option<String>>, Option<String>, u32);

/// The decimal places a name line declares (`with 2 decimals`).
fn places_of(suffix: &str) -> u32 {
    let Some(rest) = suffix
        .trim_end()
        .strip_suffix("decimals")
        .or_else(|| suffix.trim_end().strip_suffix("decimal"))
    else {
        return 0;
    };
    rest.trim_end()
        .rsplit_once("with ")
        .and_then(|(_, n)| n.trim().parse().ok())
        .unwrap_or(0)
}

/// A choice value's place as a number of a rule with `places` decimals:
/// the rule reads every number as tenths, hundredths..., so place 2 in a
/// rule with 2 decimals is written `0.02` and stays 2 underneath, the same
/// number a rule without decimals takes.
fn place(at: usize, places: u32) -> String {
    if places == 0 {
        return at.to_string();
    }
    let scale = 10usize.pow(places);
    format!(
        "{}.{:0width$}",
        at / scale,
        at % scale,
        width = places as usize
    )
}

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
        let mut end = index + 1;
        while end < lines.len() && !is_top(strip_comment(&lines[end])) {
            end += 1;
        }
        let answer = answer_choice(&lines[index..end], types);
        let places = rest
            .split_once(')')
            .map_or(0, |(_, suffix)| places_of(suffix));
        out.push((name.trim().to_string(), tys, answer, places));
    }
    Ok(out)
}

/// What a name means inside one rule.
enum Local {
    /// A record input: its type and the numbers it holds (dotted path,
    /// written-out input, choice).
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
        // `sizes[n]: Size`: a list of any length holding a type.
        let (param, list) = match param.split_once('[') {
            Some((name, size)) => {
                let size = size.trim_end_matches(']').trim();
                if size.parse::<usize>().is_ok() {
                    return Err(error(
                        first_line,
                        format!(
                            "a list of a type needs a length name, like `{}[n]`",
                            name.trim()
                        ),
                    ));
                }
                (name.trim(), format!("[{size}]"))
            }
            None => (param, String::new()),
        };
        check_name(param, first_line)?;
        if types.value(param).is_some() {
            return Err(error(
                first_line,
                format!("`{param}` is a value of a type; give the input another name"),
            ));
        }
        match types.get(ty) {
            Some(Def::Choice(_)) => {
                written.push(format!("{param}{list}"));
                choices.push((param.to_string(), types.choice(ty).unwrap()));
                locals.push((param.to_string(), Local::Choice(ty.to_string())));
            }
            Some(Def::Record(_)) => {
                let mut flat = Vec::new();
                for (field, choice) in &types.leaves(ty) {
                    let input = format!("{param}__{}", field.replace('.', "__"));
                    written.push(format!("{input}{list}"));
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

    let places = places_of(suffix);
    let rewrite = |text: &str, line: usize| rewrite(text, line, &locals, types, signatures, places);
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
                places,
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
    places: u32,
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
    for (n, (k, at_places, value)) in cases.iter().enumerate() {
        let arm_line = line + 1 + k;
        let value = rewrite(value, arm_line)?;
        let condition = match at_places {
            _ if n == last => "else".to_string(),
            Some(at_places) => at_places
                .iter()
                .map(|at| format!("{input} is {}", place(*at, places)))
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
    places: u32,
) -> Result<String, String> {
    check_calls(text, line, locals, types, signatures, places)?;
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
                // `order.address.zip`: the dotted path after the record.
                let mut path = String::new();
                while chars.get(i) == Some(&'.') && chars.get(i + 1).is_some_and(|c| is_word(*c)) {
                    let from = i + 1;
                    let mut to = from;
                    while to < chars.len() && is_word(chars[to]) {
                        to += 1;
                    }
                    let field: String = chars[from..to].iter().collect();
                    let inner = if path.is_empty() {
                        ty.clone()
                    } else {
                        match types.at(ty, &path) {
                            Ok(Some(t)) if matches!(types.get(&t), Some(Def::Record(_))) => t,
                            _ => break,
                        }
                    };
                    if types.at(&inner, &field).is_err() {
                        let Some(Def::Record(names)) = types.get(&inner) else {
                            break;
                        };
                        let names: Vec<&str> = names.iter().map(|(f, _)| f.as_str()).collect();
                        return Err(error(
                            line,
                            format!("`{inner}` has no field `{field}` ({})", names.join(", ")),
                        ));
                    }
                    if !path.is_empty() {
                        path.push('.');
                    }
                    path += &field;
                    i = to;
                }
                if let Some((_, input, _)) = fields.iter().find(|(f, _, _)| *f == path) {
                    out += input;
                    continue;
                }
                // The whole record, or a record inside it: only as a rule's input.
                let shown = if path.is_empty() {
                    word.clone()
                } else {
                    format!("{word}.{path}")
                };
                let before = chars[..start].iter().rev().find(|c| !c.is_whitespace());
                let next = chars[i..].iter().find(|c| !c.is_whitespace());
                // `count(orders)` of a list of records: the length, from any field.
                let lead: String = chars[..start].iter().collect();
                if path.is_empty() && lead.trim_end().ends_with("count(") && next == Some(&')') {
                    out += &fields[0].1;
                    continue;
                }
                if !matches!(before, Some('(' | ',')) || !matches!(next, Some(')' | ',')) {
                    let field = &fields
                        .iter()
                        .find(|(f, _, _)| path.is_empty() || f.starts_with(&format!("{path}.")))
                        .unwrap()
                        .0;
                    return Err(error(
                        line,
                        format!(
                            "`{shown}` is a record; use one of its fields, like `{word}.{field}`, or pass it to a rule"
                        ),
                    ));
                }
                let inputs: Vec<&str> = fields
                    .iter()
                    .filter(|(f, _, _)| path.is_empty() || f.starts_with(&format!("{path}.")))
                    .map(|(_, n, _)| n.as_str())
                    .collect();
                out += &inputs.join(", ");
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
                    Some((_, at)) => out += &place(at, places),
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

fn arg_type(arg: &str, locals: &[(String, Local)], types: &Types, signatures: &[Signature]) -> Arg {
    let arg = arg.trim();
    if let Some((callee, rest)) = arg.split_once('(')
        && rest.ends_with(')')
        && let Some((_, _, Some(choice), _)) =
            signatures.iter().find(|(n, _, _, _)| n == callee.trim())
    {
        return Arg::Choice(choice.clone());
    }
    if let Some((record, path)) = arg.split_once('.')
        && let Some((_, Local::Record(ty, _))) = locals.iter().find(|(n, _)| n == record)
        && let Ok(Some(inner)) = types.at(ty, path)
    {
        return match types.get(&inner) {
            Some(Def::Record(_)) => Arg::Record(inner),
            _ => Arg::Choice(inner),
        };
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
    places: u32,
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
        let Some((_, params, _, callee_places)) =
            signatures.iter().find(|(n, _, _, _)| *n == callee)
        else {
            // A rule from another file (or `min`, `sum`): its types are not known here.
            // `count(orders)` counts a list of records.
            if callee == "count" {
                continue;
            }
            if let Some(arg) = args
                .iter()
                .find(|a| matches!(arg_type(a, locals, types, signatures), Arg::Record(_)))
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
            let given = arg_type(arg, locals, types, signatures);
            // A call moves numbers to the callee's places; a choice keeps its place.
            if matches!(given, Arg::Choice(_)) && *callee_places != places {
                return Err(error(
                    line,
                    format!(
                        "`{}` is a choice; `{callee}` has other decimal places, so a choice cannot be passed to it yet",
                        arg.trim()
                    ),
                ));
            }
            match (given, param) {
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
    fn a_record_may_hold_a_record() {
        let source = "type Size = small | large\n\ntype Address\n  zone\n  size: Size\n\ntype Order\n  price\n  to: Address\n\nfee(a: Address)\n  need a.zone <= 9\n  match a.size\n    small -> a.zone\n    large -> a.zone + 10\n\ntotal(o: Order)\n  need o.price <= 100\n  need o.to.zone <= 9\n  always -> o.price + fee(o.to)\n";
        let module = parse_module(source).unwrap();
        let total = module.contracts.iter().find(|c| c.name == "total").unwrap();
        let names: Vec<&str> = total.params.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["o__price", "o__to__zone", "o__to__size"]);
        assert_eq!(total.choices[0].0, "o__to__size");
        let e = err(&source.replace("fee(o.to)", "o.to"));
        assert!(
            e.contains("`o.to` is a record") && e.contains("o.to.zone"),
            "{e}"
        );
        let e = err(&source.replace("o.to.zone <= 9", "o.to.street <= 9"));
        assert!(e.contains("`Address` has no field `street`"), "{e}");
        let e = err(&source.replace("fee(o.to)", "fee(o)"));
        assert!(
            e.contains("takes a `Address` as input 1, got a `Order`"),
            "{e}"
        );
    }

    #[test]
    fn an_answer_can_be_a_choice() {
        let source = format!(
            "{TYPES}classify(w)\n  need w <= 100\n  w < 10 -> small\n  else -> large\n\nfee(size: Size)\n  always -> size\n\nship(w)\n  need w <= 100\n  always -> fee(classify(w))\n"
        );
        let module = parse_module(&source).unwrap();
        let classify = &module.contracts[0];
        assert_eq!(classify.answer_choice.as_ref().unwrap().name, "Size");
        assert!(module.contracts[1].answer_choice.is_none());
        let e = err(&format!(
            "{source}\ncost(order: Order)\n  always -> order.price\n\nbad(w)\n  need w <= 100\n  always -> cost(classify(w))\n"
        ));
        assert!(
            e.contains("takes a `Order` as input 1, got a `Size`"),
            "{e}"
        );
        // Mixed with numbers, a value is still its place.
        let mixed =
            parse_module(&format!("{TYPES}f(w)\n  w < 10 -> small\n  else -> 7\n")).unwrap();
        assert!(mixed.contracts[0].answer_choice.is_none());
    }

    #[test]
    fn a_list_can_hold_a_type() {
        let module = parse_module(&format!(
            "{TYPES}boxes(sizes[n]: Size)\n  need n <= 10\n  always -> sum(sizes)\n\npaid(orders[n]: Order)\n  need n <= 10\n  need orders.price <= 100\n  always -> sum(orders.price) + count(orders)\n"
        ))
        .unwrap();
        let item = module
            .contracts
            .iter()
            .find(|c| c.name == "boxes__item1")
            .unwrap();
        assert_eq!(item.choices[0].0, "sizes");
        assert_eq!(item.requires.len(), 1, "an item of `sizes` is at most 2");
        let paid = module
            .contracts
            .iter()
            .find(|c| c.name == "paid__item1")
            .unwrap();
        let names: Vec<&str> = paid.params.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["orders__price", "orders__quantity", "orders__size"]);
        let e = err(&format!("{TYPES}f(sizes[3]: Size)\n  always -> 1\n"));
        assert!(e.contains("needs a length name"), "{e}");
    }

    #[test]
    fn a_record_cannot_hold_itself() {
        let e = err("type A\n  x\n  b: B\n\ntype B\n  a: A\n\nf(v)\n  always -> v\n");
        assert!(
            e.contains("cannot hold itself") && e.contains("A holds B"),
            "{e}"
        );
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
        // In a rule with decimals a choice keeps its place: `big` is `0.01`.
        let module = parse_module(
            "type Size = small | big\nf(s: Size, w) with 2 decimals\n  need w <= 100\n  match s\n    small -> w\n    big -> w * 2\n",
        )
        .unwrap();
        assert_eq!(module.contracts[0].choices[0].0, "s");
        let e = err(
            "type Size = small | big\ng(s: Size)\n  always -> 1\n\nf(s: Size) with 2 decimals\n  always -> g(s)\n",
        );
        assert!(e.contains("other decimal places"), "{e}");
    }
}
