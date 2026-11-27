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

use crate::ast::{ChoiceType, RecordAnswer};
use crate::simple::{check_name, strip_comment};
use std::hash::Hash;

use crate::{AnswerRange, FromReader, ImportCache};

pub(crate) struct Expanded {
    pub source: String,
    /// For each rule, its choice inputs (after records are written out).
    pub choices: Vec<(String, String, ChoiceType)>,
    /// Rules whose every answer is a value of one choice.
    pub answers: Vec<(String, ChoiceType)>,
    /// Rules that answer a record, written out as one rule per field.
    pub records: Vec<RecordAnswer>,
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

/// The values after `=`: split at `|`, except inside quoted texts.
fn split_values(text: &str) -> Vec<String> {
    let mut values = vec![String::new()];
    let mut quoted = false;
    for c in text.chars() {
        match c {
            '"' => {
                quoted = !quoted;
                values.last_mut().unwrap().push(c);
            }
            '|' if !quoted => values.push(String::new()),
            _ => values.last_mut().unwrap().push(c),
        }
    }
    values.into_iter().map(|v| v.trim().to_string()).collect()
}

/// A text value of a choice: `"A"`, written in quotes.
fn is_text(value: &str) -> bool {
    value.len() >= 2 && value.starts_with('"') && value.ends_with('"')
}

/// The text a choice value shows: `A` for `"A"`, the name itself otherwise.
pub fn value_text(value: &str) -> &str {
    if is_text(value) {
        &value[1..value.len() - 1]
    } else {
        value
    }
}

/// The name generated code gives a choice value: a name stays as written,
/// a text is kept when it is ASCII, else every other character is spelled
/// in hex (`T_uD569_`), so each text gets its own name.
pub fn value_ident(value: &str) -> String {
    if !is_text(value) {
        return value.to_string();
    }
    let text = value_text(value);
    let plain = text
        .chars()
        .enumerate()
        .all(|(i, c)| c == '_' || c.is_ascii_alphabetic() || (i > 0 && c.is_ascii_digit()));
    if plain && !text.is_empty() {
        return text.to_string();
    }
    let mut out = String::from("T");
    for c in text.chars() {
        if c.is_ascii_alphanumeric() || c == '_' {
            out.push(c);
        } else {
            out += &format!("_u{:X}_", c as u32);
        }
    }
    out
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
    let mut records = Vec::new();
    let mut appended: Vec<String> = Vec::new();

    let mut index = 0;
    while index < lines.len() {
        let text = strip_comment(&lines[index]).to_string();
        let mut end = index + 1;
        while end < lines.len() && !is_top(strip_comment(&lines[end])) {
            end += 1;
        }
        let head = text.trim();
        if head.starts_with(FROM_MARK) {
            for line in &mut lines[index..end] {
                line.clear();
            }
            index = end;
            continue;
        }
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
        if let Some((record, blocks)) = record_answer(&lines[index..end], index + 1, &types)? {
            let name = rule_name(head);
            let mut fields = Vec::new();
            for (k, (field, block)) in blocks.into_iter().enumerate() {
                if let Some(choice) = answer_choice(&block, &types) {
                    answers.push((rule_name(&block[0]), types.choice(&choice).unwrap()));
                }
                let rule = rule_block(&block, index + 1, &types, &signatures)?;
                for (param, choice) in rule.choices {
                    choices.push((rule.name.clone(), param, choice));
                }
                fields.push((field, rule.name));
                if k == 0 {
                    for (offset, line) in rule.lines.into_iter().enumerate() {
                        lines[index + offset] = line;
                    }
                } else {
                    appended.push(rule.lines.join("\n"));
                }
            }
            records.push(RecordAnswer {
                name,
                record,
                fields,
            });
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
    let mut source = lines.join("\n");
    for rule in appended {
        source += "\n\n";
        source += &rule;
    }
    Ok(Some(Expanded {
        source,
        choices,
        answers,
        records,
    }))
}

/// `name(a) -> Record`: the text between `)` and `->`, and the record.
fn answer_record(head: &str) -> Option<(&str, &str)> {
    let (_, suffix) = head.split_once(')')?;
    let (before, record) = suffix.rsplit_once("->")?;
    Some((before.trim(), record.trim()))
}

/// Blocks of one field of a rule answering a record.
type FieldBlocks = Vec<(String, Vec<String>)>;

/// For a rule that answers a record (`quote(price) -> Bill`), one block per
/// field, as long as the rule: the name line becomes `quote__total(price)`,
/// the lines above the fields are shared, and the lines of other fields are
/// blank, so errors keep the user's line numbers. A field's `promise`
/// lines go under it, where `answer` is that field.
fn record_answer(
    block: &[String],
    first_line: usize,
    types: &Types,
) -> Result<Option<(String, FieldBlocks)>, String> {
    let head = strip_comment(&block[0]).trim().to_string();
    let Some((suffix, record)) = answer_record(&head) else {
        return Ok(None);
    };
    let fields = match types.get(record) {
        Some(Def::Record(fields)) => fields.clone(),
        Some(Def::Choice(_)) => {
            return Err(error(
                first_line,
                format!(
                    "`{record}` is a choice: answer its values and drop `-> {record}` from the name line"
                ),
            ));
        }
        None => return Err(error(first_line, format!("there is no type `{record}`"))),
    };
    if let Some((field, _)) = fields.iter().find(|(_, t)| {
        t.as_deref()
            .is_some_and(|t| matches!(types.get(t), Some(Def::Record(_))))
    }) {
        return Err(error(
            first_line,
            format!("`{record}` holds a record (`{field}`); a rule cannot answer it yet"),
        ));
    }
    let (name, rest) = head.split_once('(').unwrap();
    let params = rest.split_once(')').unwrap().0;
    let gap = if suffix.is_empty() { "" } else { " " };

    // Each field's line and the end of its lines.
    let mut places: Vec<(String, usize, usize)> = Vec::new();
    let mut offset = 1;
    while offset < block.len() {
        let text = strip_comment(&block[offset]);
        let Some(field) = text.trim().strip_suffix(':').map(str::trim) else {
            offset += 1;
            continue;
        };
        if !fields.iter().any(|(f, _)| f == field) {
            let names: Vec<&str> = fields.iter().map(|(f, _)| f.as_str()).collect();
            return Err(error(
                first_line + offset,
                format!("`{record}` has no field `{field}` ({})", names.join(", ")),
            ));
        }
        if places.iter().any(|(f, _, _)| f == field) {
            return Err(error(
                first_line + offset,
                format!("`{field}:` is written twice"),
            ));
        }
        let at = indent(text);
        let mut end = offset + 1;
        while end < block.len() {
            let t = strip_comment(&block[end]);
            if !t.trim().is_empty() && indent(t) <= at {
                break;
            }
            end += 1;
        }
        places.push((field.to_string(), offset, end));
        offset = end;
    }
    let missing: Vec<String> = fields
        .iter()
        .filter(|(f, _)| !places.iter().any(|(p, _, _)| p == f))
        .map(|(f, _)| format!("`{f}:`"))
        .collect();
    if !missing.is_empty() {
        return Err(error(
            first_line,
            format!(
                "a rule answering `{record}` gives each field its lines below a line like `{}:`; add {}",
                fields[0].0,
                missing.join(", ")
            ),
        ));
    }
    let inside = |offset: usize| {
        places
            .iter()
            .any(|(_, at, end)| (*at..*end).contains(&offset))
    };
    let mut out = Vec::new();
    for (field, at, end) in &places {
        let mut lines = vec![String::new(); block.len()];
        lines[0] = format!("{name}__{field}({params}){gap}{suffix}");
        for offset in 1..block.len() {
            let trimmed = strip_comment(&block[offset]).trim();
            if (*at + 1..*end).contains(&offset) {
                lines[offset] = block[offset].clone();
            } else if inside(offset) || trimmed.is_empty() {
                continue;
            } else if trimmed.starts_with("promise ") {
                return Err(error(
                    first_line + offset,
                    format!(
                        "put a promise under its field, after its `->` lines; there `answer` is that field, like `{}`",
                        fields[0].0
                    ),
                ));
            } else if trimmed.contains("->") {
                return Err(error(
                    first_line + offset,
                    format!("put each answer under its field, like `{}:`", fields[0].0),
                ));
            } else {
                lines[offset] = block[offset].clone();
            }
        }
        out.push((field.clone(), lines));
    }
    // In the order the record declares its fields.
    out.sort_by_key(|(f, _)| fields.iter().position(|(n, _)| n == f));
    Ok(Some((record.to_string(), out)))
}

/// Marks the name line of a rule another file declares (`from rules:
/// total`), added by [`with_imported_types`] so calls to it are checked.
const FROM_MARK: &str = "__from ";

/// Adds to `source` the types its `from file: ...` lines name, and the
/// name lines of the rules they name. `base` is where `source` is, and
/// `read` gives a file's source and place from the path written after
/// `from`. The added lines go after the last line, so line numbers stay the
/// user's. Types a named rule takes come too. Also returns the answer ranges
/// of the named rules, so calls to them are range-checked.
pub fn with_imported_types<B: Clone + PartialEq + Eq + Hash>(
    source: &str,
    base: &B,
    read: &FromReader<'_, B>,
    cache: &mut ImportCache<B>,
) -> Result<(String, Vec<AnswerRange>), String> {
    imported(source, base, read, &mut Vec::new(), cache)
}

/// `chain` holds the files whose `from` lines are being read: a file that is
/// already in it closes a circle.
fn imported<B: Clone + PartialEq + Eq + Hash>(
    source: &str,
    base: &B,
    read: &FromReader<'_, B>,
    chain: &mut Vec<B>,
    cache: &mut ImportCache<B>,
) -> Result<(String, Vec<AnswerRange>), String> {
    chain.push(base.clone());
    let mut lines: Vec<String> = source.lines().map(String::from).collect();
    let mut added: Vec<String> = Vec::new();
    let mut added_types: Vec<String> = Vec::new();
    let mut known: Vec<AnswerRange> = Vec::new();
    for line in &mut lines {
        let text = strip_comment(line).to_string();
        let Some(rest) = text.strip_prefix("from ") else {
            continue;
        };
        let Some((file, names)) = rest.split_once(':') else {
            continue;
        };
        // The other file, expanded once per load for each file that names it.
        let key = (base.clone(), file.trim().to_string());
        let (other, ranges) = match cache.0.get(&key) {
            Some(hit) => hit.clone(),
            None => {
                // The other file's own `from` lines resolve from its own place.
                let (other_source, other_base) = read(base, file.trim())?;
                if chain.contains(&other_base) {
                    return Err("`from` lines import each other in a circle".to_string());
                }
                let (other, other_known) =
                    imported(&other_source, &other_base, read, chain, cache)?;
                let (_, ranges) = crate::parse_module_known(&other, &other_known)
                    .map_err(|error| format!("{}: {error}", file.trim()))?;
                cache.0.insert(key, (other.clone(), ranges.clone()));
                (other, ranges)
            }
        };
        let items = import_items(names);
        for (name, local) in &items {
            // A rule answering a record is one rule per field (`quote__goods`).
            for (rule, low, high) in ranges.iter().filter(|(rule, _, _)| {
                rule == name
                    || rule
                        .strip_prefix(name.as_str())
                        .is_some_and(|rest| rest.starts_with("__"))
            }) {
                let field = &rule[name.len()..];
                known.push((format!("{local}{field}"), *low, *high));
            }
        }
        let mut other_lines: Vec<String> = other.lines().map(String::from).collect();
        // Each `type` block of the other file, as written, with the types it names.
        let mut blocks: Vec<(String, String, Vec<String>)> = Vec::new();
        let mut at = 0;
        while at < other_lines.len() {
            let head = strip_comment(&other_lines[at]).to_string();
            let mut end = at + 1;
            while end < other_lines.len() && !is_top(strip_comment(&other_lines[end])) {
                end += 1;
            }
            if let Some(decl) = head.strip_prefix("type ") {
                let name = decl
                    .split('=')
                    .next()
                    .unwrap_or_default()
                    .trim()
                    .to_string();
                let uses = other_lines[at + 1..end]
                    .iter()
                    .filter_map(|l| Some(strip_comment(l).split_once(':')?.1.trim().to_string()))
                    .collect();
                blocks.push((name, other_lines[at..end].join("\n"), uses));
            }
            at = end;
        }
        if blocks.is_empty() {
            continue;
        }
        // Types brought in under another name (`Order as Purchase`) are renamed
        // wherever this file's added text mentions them.
        let type_renames: Vec<(String, String)> = items
            .iter()
            .filter(|(name, local)| name != local && blocks.iter().any(|(n, _, _)| n == name))
            .cloned()
            .collect();
        let types = read_types(&mut other_lines)?;
        let signatures = signatures(&other_lines, &types)?;
        let mut need: Vec<String> = Vec::new();
        let mut rules: Vec<(String, String)> = Vec::new();
        for (name, local) in &items {
            if blocks.iter().any(|(n, _, _)| n == name) {
                need.push(name.to_string());
                continue;
            }
            rules.push((name.clone(), local.clone()));
            if let Some((_, params, answer, _)) = signatures.iter().find(|(n, _, _, _)| n == name) {
                need.extend(params.iter().flatten().cloned());
                need.extend(answer.iter().cloned());
            }
        }
        // The types the named ones hold, too.
        let mut k = 0;
        while k < need.len() {
            if let Some((_, _, uses)) = blocks.iter().find(|(n, _, _)| *n == need[k]) {
                for used in uses {
                    if !need.contains(used) {
                        need.push(used.clone());
                    }
                }
            }
            k += 1;
        }
        for (name, block, _) in &blocks {
            if need.contains(name) && !added_types.contains(name) {
                added_types.push(name.clone());
                added.push(rename_words(block, &type_renames));
            }
        }
        for (rule, local) in &rules {
            let Some((_, params, answer, places)) =
                signatures.iter().find(|(n, _, _, _)| n == rule)
            else {
                continue;
            };
            if params.iter().all(Option::is_none) && answer.is_none() {
                continue;
            }
            let params: Vec<String> = params
                .iter()
                .enumerate()
                .map(|(k, t)| match t {
                    Some(t) => format!("v{k}: {}", rename_words(t, &type_renames)),
                    None => format!("v{k}"),
                })
                .collect();
            let decimals = if *places > 0 {
                format!(" with {places} decimals")
            } else {
                String::new()
            };
            let mut sig = format!("{FROM_MARK}{local}({}){decimals}", params.join(", "));
            match answer
                .as_deref()
                .and_then(|a| types.get(a).map(|def| (a, def)))
            {
                Some((_, Def::Choice(values))) => sig += &format!("\n  -> {}", values[0]),
                // `quote(price) -> Bill`: the caller reads its fields as `quote(price).goods`.
                Some((record, Def::Record(_))) => {
                    sig += &format!(" -> {}", rename_words(record, &type_renames));
                }
                _ => {}
            }
            added.push(sig);
        }
        let file = file.trim();
        let rules: Vec<String> = rules
            .iter()
            .map(|(name, local)| {
                if name == local {
                    name.clone()
                } else {
                    format!("{name} as {local}")
                }
            })
            .collect();
        let rules = rules.join(", ");
        *line = if rules.is_empty() {
            String::new()
        } else {
            format!("from {file}: {rules}")
        };
    }
    chain.pop();
    if added.is_empty() {
        return Ok((source.to_string(), known));
    }
    Ok((
        format!("{}\n\n{}\n", lines.join("\n"), added.join("\n\n")),
        known,
    ))
}

/// The names a `from` line brings in, each with the name it has here:
/// `fee` is (`fee`, `fee`), `fee as price` is (`fee`, `price`).
fn import_items(names: &str) -> Vec<(String, String)> {
    names
        .split(',')
        .map(|item| item.split_whitespace().collect::<Vec<_>>())
        .filter(|words| !words.is_empty())
        .map(|words| match words.as_slice() {
            [name, "as", local] => ((*name).to_string(), (*local).to_string()),
            [name, ..] => ((*name).to_string(), (*name).to_string()),
            [] => unreachable!("empty items are filtered out"),
        })
        .collect()
}

/// `text` with each word that `renames` names replaced, outside quotes.
fn rename_words(text: &str, renames: &[(String, String)]) -> String {
    if renames.is_empty() {
        return text.to_string();
    }
    let mut out = String::new();
    let mut word = String::new();
    let mut quoted = false;
    for c in text.chars() {
        if c.is_alphanumeric() || c == '_' {
            word.push(c);
            continue;
        }
        push_word(&mut out, &mut word, quoted, renames);
        if c == '"' {
            quoted = !quoted;
        }
        out.push(c);
    }
    push_word(&mut out, &mut word, quoted, renames);
    out
}

/// Appends `word` to `out`, renamed unless it is inside quotes.
fn push_word(out: &mut String, word: &mut String, quoted: bool, renames: &[(String, String)]) {
    match renames.iter().find(|(from, _)| from == word) {
        Some((_, to)) if !quoted => out.push_str(to),
        _ => out.push_str(word),
    }
    word.clear();
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
            let values: Vec<String> = split_values(values);
            if values.iter().any(String::is_empty) {
                return Err(error(
                    line_no,
                    "write a choice as `type Size = small | medium | large`",
                ));
            }
            let texts = values.iter().filter(|v| is_text(v)).count();
            if texts != 0 && texts != values.len() {
                return Err(error(
                    line_no,
                    "a choice holds either names or quoted texts, not both",
                ));
            }
            for value in &values {
                if is_text(value) {
                    if value[1..value.len() - 1].trim().is_empty() {
                        return Err(error(line_no, "a text value cannot be empty"));
                    }
                } else {
                    check_name(value, line_no)?;
                }
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
        let head = head.strip_prefix(FROM_MARK).unwrap_or(head);
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
        let answer = match answer_record(head) {
            Some((_, record)) => Some(record.to_string()),
            None => answer_choice(&lines[index..end], types),
        };
        let places = rest.split_once(')').map_or(0, |(_, suffix)| {
            places_of(suffix.split("->").next().unwrap_or_default())
        });
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
        if c == '"' && !quoted {
            // A quoted text that is a value of a choice is its place.
            let close = chars[i + 1..].iter().position(|c| *c == '"');
            if let Some(close) = close {
                let text: String = chars[i..=i + 1 + close].iter().collect();
                if let Some((_, at)) = types.value(&text) {
                    out += &place(at, places);
                    i += close + 2;
                    continue;
                }
            }
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
            None if chars.get(i) == Some(&'(')
                && record_rule(&word, types, signatures).is_some() =>
            {
                // `quote(a).total` is `quote__total(a)`; `fee(quote(a))` passes each field.
                let (record, fields) = record_rule(&word, types, signatures).unwrap();
                let mut depth = 0;
                let mut close = i;
                for (k, c) in chars.iter().enumerate().skip(i) {
                    match c {
                        '(' => depth += 1,
                        ')' => {
                            depth -= 1;
                            if depth == 0 {
                                close = k;
                                break;
                            }
                        }
                        _ => {}
                    }
                }
                let inner: String = chars[i + 1..close].iter().collect();
                let args = rewrite(&inner, line, locals, types, signatures, places)?;
                i = close + 1;
                if chars.get(i) == Some(&'.') {
                    let from = i + 1;
                    let mut to = from;
                    while to < chars.len() && is_word(chars[to]) {
                        to += 1;
                    }
                    let field: String = chars[from..to].iter().collect();
                    if !fields.contains(&field) {
                        return Err(error(
                            line,
                            format!("`{record}` has no field `{field}` ({})", fields.join(", ")),
                        ));
                    }
                    out += &format!("{word}__{field}({args})");
                    i = to;
                    continue;
                }
                let before = chars[..start].iter().rev().find(|c| !c.is_whitespace());
                let next = chars[i..].iter().find(|c| !c.is_whitespace());
                if !matches!(before, Some('(' | ',')) || !matches!(next, Some(')' | ',')) {
                    return Err(error(
                        line,
                        format!(
                            "`{word}` answers a `{record}`; use one of its fields, like `{word}(...).{}`, or pass it to a rule",
                            fields[0]
                        ),
                    ));
                }
                let calls: Vec<String> = fields
                    .iter()
                    .map(|f| format!("{word}__{f}({args})"))
                    .collect();
                out += &calls.join(", ");
            }
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

/// The record a rule of this file answers, and its fields.
fn record_rule(
    name: &str,
    types: &Types,
    signatures: &[Signature],
) -> Option<(String, Vec<String>)> {
    let (_, _, answer, _) = signatures.iter().find(|(n, _, _, _)| n == name)?;
    let record = answer.as_deref()?;
    let Some(Def::Record(fields)) = types.get(record) else {
        return None;
    };
    Some((
        record.to_string(),
        fields.iter().map(|(f, _)| f.clone()).collect(),
    ))
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
        && let Some((_, _, Some(answer), _)) =
            signatures.iter().find(|(n, _, _, _)| n == callee.trim())
    {
        if rest.ends_with(')') {
            return match types.get(answer) {
                Some(Def::Record(_)) => Arg::Record(answer.clone()),
                _ => Arg::Choice(answer.clone()),
            };
        }
        // `quote(a).size`: a field of the answer.
        if let Some((_, field)) = rest.rsplit_once(").")
            && let Ok(ty) = types.at(answer, field.trim())
        {
            return match ty {
                Some(t) if matches!(types.get(&t), Some(Def::Choice(_))) => Arg::Choice(t),
                _ => Arg::Number,
            };
        }
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
    use crate::ImportCache;
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
    fn types_come_from_other_files() {
        let shop = format!(
            "{TYPES}fee(order: Order)\n  need order.price <= 100\n  always -> order.price\n\nsize_of(w)\n  w < 5 -> small\n  else -> large\n"
        );
        let read = |_: &String, file: &str| {
            assert_eq!(file, "shop");
            Ok((shop.clone(), file.to_string()))
        };
        let main = "from shop: Order, fee, size_of\n\nf(o: Order)\n  need o.price <= 100\n  always -> fee(o)\n";
        let module = crate::parse_module_with(
            main,
            &"main".to_string(),
            &read,
            &mut ImportCache::default(),
        )
        .unwrap();
        let f = module.contracts.iter().find(|c| c.name == "f").unwrap();
        let names: Vec<&str> = f.params.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["o__price", "o__quantity", "o__size"]);
        assert_eq!(
            module.imports[0].names.len(),
            2,
            "types are not imported as rules"
        );
        // `Size` comes with `Order`, which holds it.
        assert_eq!(f.choices[0].1.name, "Size");
        let e = crate::parse_module_with(
            "from shop: Order, fee, size_of\n\ng(w)\n  always -> fee(size_of(w))\n",
            &"g".to_string(),
            &read,
            &mut ImportCache::default(),
        )
        .unwrap_err();
        assert!(
            e.contains("takes a `Order` as input 1, got a `Size`"),
            "{e}"
        );
    }

    #[test]
    fn from_lines_resolve_from_the_file_that_writes_them() {
        // `lib/tax` names `money` next to itself, so `main` reaches `Order`
        // through it, and the `from` line in `lib/tax` finds `lib/money`.
        let files = [
            (
                "lib/money",
                "type Size = small | large\ntype Order\n  price\n  size: Size\n",
            ),
            (
                "lib/tax",
                "from money: Order\n\nvat(o: Order)\n  need o.price <= 100\n  always -> o.price\n",
            ),
        ];
        let read = |base: &String, file: &str| -> Result<(String, String), String> {
            let dir = base.rsplit_once('/').map_or("", |(dir, _)| dir);
            let key = if dir.is_empty() {
                file.to_string()
            } else {
                format!("{dir}/{file}")
            };
            let text = files
                .iter()
                .find(|(name, _)| *name == key)
                .map(|(_, text)| text.to_string())
                .ok_or_else(|| format!("no file {key}"))?;
            Ok((text, key))
        };
        let main =
            "from lib/tax: vat, Order\n\nf(o: Order)\n  need o.price <= 100\n  always -> vat(o)\n";
        let module = crate::parse_module_with(
            main,
            &"main".to_string(),
            &read,
            &mut ImportCache::default(),
        )
        .unwrap();
        let f = module.contracts.iter().find(|c| c.name == "f").unwrap();
        assert_eq!(f.choices[0].1.name, "Size");
    }

    #[test]
    fn answer_ranges_of_an_imported_rule_bound_the_callers_arithmetic() {
        // As in one file: `fee` answers 0 to 10000 here, so `fee(p) + p`
        // with `p` up to 10000 stays in range; unbounded, it does not.
        let caller = "from money: fee\n\nvat(p)\n  need p <= 10000\n  always -> fee(p) + p\n";
        let bounded = "fee(price)\n  need price <= 10000\n  always -> price\n";
        let unbounded = "fee(price)\n  always -> price\n";
        let ok = crate::parse_module_with(
            caller,
            &"vat".to_string(),
            &|_: &String, file: &str| {
                assert_eq!(file, "money");
                Ok((bounded.to_string(), file.to_string()))
            },
            &mut ImportCache::default(),
        );
        assert!(ok.is_ok(), "{ok:?}");
        let e = crate::parse_module_with(
            caller,
            &"vat".to_string(),
            &|_: &String, file: &str| {
                assert_eq!(file, "money");
                Ok((unbounded.to_string(), file.to_string()))
            },
            &mut ImportCache::default(),
        )
        .unwrap_err();
        assert!(e.contains("may go outside"), "{e}");
    }

    #[test]
    fn a_rule_can_answer_a_record() {
        let rule = "type Bill\n  goods\n  tax\n\nquote(p) -> Bill\n  need p <= 100\n  goods:\n    always -> p\n  tax:\n    always -> p / 10\n";
        let module = parse_module(&format!(
            "{rule}\nboth(p)\n  need p <= 100\n  always -> quote(p).goods + quote(p).tax\n"
        ))
        .unwrap();
        let names: Vec<&str> = module.contracts.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["quote__goods", "both", "quote__tax"]);
        let record = module.contracts[0].record_answer.as_ref().unwrap();
        assert_eq!(
            record.fields[1],
            ("tax".to_string(), "quote__tax".to_string())
        );
        let e = err(&rule.replace("  tax:\n    always -> p / 10\n", ""));
        assert!(e.contains("add `tax:`"), "{e}");
        let e = err(&rule.replace("  goods:", "  promise answer.goods <= 100\n  goods:"));
        assert!(e.contains("put a promise under its field"), "{e}");
        let e = err(&format!(
            "{rule}\nbad(p)\n  need p <= 100\n  always -> quote(p)\n"
        ));
        assert!(e.contains("answers a `Bill`"), "{e}");
        let e = err(&format!(
            "{rule}\nbad(p)\n  need p <= 100\n  always -> quote(p).fee\n"
        ));
        assert!(e.contains("no field `fee`"), "{e}");
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
