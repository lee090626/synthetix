//! Lists of any length (`cart(prices[n])`, issue #182).
//!
//! A rule over `prices[n]` is split before translation into ordinary rules:
//!
//! - one item rule `cart__item1(prices, ...)` per `sum(expr)`, `min(expr)` or
//!   `max(expr)`: `always -> expr` on one item of each list, with the `need`
//!   lines about items;
//! - the core `cart__core(scalars.., n, sum1, ..)` in place of the rule, where
//!   `sum(expr)` is the input `sum1` (`min2`, `max3` for `min`/`max`) and
//!   `count(prices)` is `n`.
//!
//! Both are proven by Z3 like any rule. The core may assume of `sum1` only
//! what `proofs/lean/Lists.lean` proves for every length: with every item in
//! `lo..hi` (the item rule's answer range), `sum1` is between `n * lo` and
//! `n * hi` (`sum_bounds`), and a loop adding the items gives exactly that sum
//! when `max * lo` and `max * hi` fit the number range (`loop_sum_exact`).
//! `min`/`max` of at least one item lie in `lo..hi` (`min_bounds`,
//! `max_bounds`).
//! Emitted code runs that loop, then calls the core.

use crate::ast::{BinaryOp, Fold, ListRule, SpecExpr};
use crate::simple::{check_ranges, strip_comment};

/// The rewritten source (same lines, item rules appended) and, for each
/// core rule, its list shape.
pub(crate) struct Expanded {
    pub source: String,
    pub rules: Vec<(String, ListRule)>,
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

fn is_word_text(text: &str) -> bool {
    !text.is_empty() && text.chars().all(is_word)
}

/// Whether `name` appears as a whole word outside quotes.
fn mentions(text: &str, name: &str) -> bool {
    let mut in_quotes = false;
    text.split(|c: char| {
        if c == '"' {
            in_quotes = !in_quotes;
        }
        in_quotes || !is_word(c)
    })
    .any(|word| word == name)
}

/// Replaces every `f(inner)` call (not part of a longer name, not in quotes)
/// with `with(inner)`.
fn replace_calls(
    text: &str,
    f: &str,
    with: &mut dyn FnMut(&str) -> Result<String, String>,
) -> Result<String, String> {
    // Odd pieces of `split('"')` are quoted text and stay as written.
    let mut out = String::new();
    for (i, piece) in text.split('"').enumerate() {
        if i > 0 {
            out.push('"');
        }
        if i % 2 == 1 {
            out += piece;
        } else {
            out += &replace_unquoted(piece, f, with)?;
        }
    }
    Ok(out)
}

fn replace_unquoted(
    text: &str,
    f: &str,
    with: &mut dyn FnMut(&str) -> Result<String, String>,
) -> Result<String, String> {
    let open = format!("{f}(");
    let mut out = String::new();
    let mut rest = text;
    while let Some(at) = rest.find(&open) {
        let before = &rest[..at];
        if before.chars().next_back().is_some_and(is_word) {
            out += &rest[..at + open.len()];
            rest = &rest[at + open.len()..];
            continue;
        }
        let inner_start = at + open.len();
        let mut depth = 1;
        let mut end = None;
        for (i, c) in rest[inner_start..].char_indices() {
            match c {
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth == 0 {
                        end = Some(inner_start + i);
                        break;
                    }
                }
                _ => {}
            }
        }
        let end = end.ok_or_else(|| format!("missing `)` after `{open}`"))?;
        out += before;
        out += &with(rest[inner_start..end].trim())?;
        rest = &rest[end + 1..];
    }
    out += rest;
    Ok(out)
}

struct Head {
    name: String,
    params: Vec<(String, bool)>,
    length: String,
    /// Text after `)`: `can be negative`, `with big numbers`.
    suffix: String,
    /// Hidden `need`s on list items (`crate::types`: a list of choices).
    item_marks: Vec<String>,
}

/// Reads a name line; `None` unless an input is a list of any length.
fn head(line: &str, line_no: usize) -> Result<Option<Head>, String> {
    let Some((name, rest)) = line.split_once('(') else {
        return Ok(None);
    };
    let Some((params, suffix)) = rest.split_once(')') else {
        return Ok(None);
    };
    let mut out = Vec::new();
    let mut length: Option<String> = None;
    for param in params.split(',').map(str::trim).filter(|p| !p.is_empty()) {
        let Some((list, size)) = param.split_once('[') else {
            out.push((param.to_string(), false));
            continue;
        };
        let size = size.trim_end_matches(']').trim();
        if size.parse::<usize>().is_ok() {
            return Ok(None);
        }
        if length.as_deref().is_some_and(|l| l != size) {
            return Err(format!(
                "line {line_no}: lists in one rule share one length, like `paid[n], share[n]`"
            ));
        }
        length = Some(size.to_string());
        out.push((list.trim().to_string(), true));
    }
    let Some(length) = length else {
        return Ok(None);
    };
    if suffix.contains("decimal") {
        return Err(format!(
            "line {line_no}: a rule with a list of any length cannot have decimals yet"
        ));
    }
    // Hidden `need`s about a list hold for each item; others stay on the name line.
    let mut marks = suffix.split(crate::types::NEED_MARK);
    let mut suffix = marks.next().unwrap_or_default().to_string();
    let mut item_marks = Vec::new();
    for mark in marks {
        if out.iter().any(|(p, list)| *list && mentions(mark, p)) {
            item_marks.push(mark.trim().to_string());
        } else {
            suffix += crate::types::NEED_MARK;
            suffix += mark;
        }
    }
    if out.iter().any(|(p, _)| *p == length) {
        return Err(format!(
            "line {line_no}: `{length}` is the length of the lists; give the input another name"
        ));
    }
    Ok(Some(Head {
        name: name.trim().to_string(),
        params: out,
        length,
        suffix: suffix.trim().to_string(),
        item_marks,
    }))
}

/// Whether `f(inner)` combines a list (`min(prices)`), not two numbers
/// (`min(a, b)`): it uses a list and has no comma outside parentheses.
fn folds_a_list(inner: &str, lists: &[&str]) -> bool {
    let mut depth = 0;
    let top_comma = inner.chars().any(|c| {
        match c {
            '(' => depth += 1,
            ')' => depth -= 1,
            _ => {}
        }
        c == ',' && depth == 0
    });
    !top_comma && lists.iter().any(|l| mentions(inner, l))
}

/// One term of [`linear_terms`]: list (or `None`), coefficient, subtracted.
type Term = (Option<String>, String, bool);

/// An item's answer as `+ - *` of list items, inputs and numbers, with at
/// most one list item per product. Coefficients use only `*`, so they stay
/// defined for unsigned numbers. `None` for a `/`, or two list items
/// multiplied.
fn linear_terms(e: &SpecExpr, lists: &[&str]) -> Option<Vec<Term>> {
    Some(match e {
        SpecExpr::Number(v) => vec![(None, v.to_string(), false)],
        SpecExpr::Ident(x) if lists.contains(&x.as_str()) => {
            vec![(Some(x.clone()), "1".into(), false)]
        }
        SpecExpr::Ident(x) => vec![(None, x.clone(), false)],
        SpecExpr::Binary(BinaryOp::AddWrap, a, b) => {
            [linear_terms(a, lists)?, linear_terms(b, lists)?].concat()
        }
        SpecExpr::Binary(BinaryOp::SubWrap | BinaryOp::SubChecked, a, b) => {
            let mut terms = linear_terms(a, lists)?;
            terms.extend(
                linear_terms(b, lists)?
                    .into_iter()
                    .map(|(l, c, minus)| (l, c, !minus)),
            );
            terms
        }
        SpecExpr::Binary(BinaryOp::MulWrap, a, b) => {
            let (a, b) = (linear_terms(a, lists)?, linear_terms(b, lists)?);
            if a.iter().any(|t| t.0.is_some()) && b.iter().any(|t| t.0.is_some()) {
                return None;
            }
            let mut terms = Vec::new();
            for (la, ca, ma) in &a {
                for (lb, cb, mb) in &b {
                    terms.push((la.clone().or(lb.clone()), format!("{ca} * {cb}"), ma != mb));
                }
            }
            terms
        }
        _ => return None,
    })
}

/// `n * v` written so a negative `v` parses.
fn times(length: &str, v: i128) -> String {
    if v < 0 {
        format!("0 - {length} * {}", -v)
    } else {
        format!("{length} * {v}")
    }
}

pub(crate) fn expand(source: &str) -> Result<Option<Expanded>, String> {
    let mut lines: Vec<String> = source.lines().map(String::from).collect();
    let mut appended = String::new();
    let mut rules = Vec::new();
    let mut index = 0;
    while index < lines.len() {
        let text = strip_comment(&lines[index]).to_string();
        let trimmed = text.trim();
        let top = !trimmed.is_empty() && !text.starts_with(' ') && !text.starts_with('\t');
        let skip = ["from ", "use ", "page "]
            .iter()
            .any(|p| trimmed.starts_with(p));
        let head = if top && !skip {
            head(trimmed, index + 1)?
        } else {
            None
        };
        let Some(head) = head else {
            index += 1;
            continue;
        };
        let start = index;
        let mut end = index + 1;
        while end < lines.len() {
            let t = strip_comment(&lines[end]);
            if !t.trim().is_empty() && !t.starts_with(' ') && !t.starts_with('\t') {
                break;
            }
            end += 1;
        }
        let (item_rules, core_text, rule) = split(&head, &lines[start..end], start + 1)?;
        for (offset, line) in core_text.into_iter().enumerate() {
            lines[start + offset] = line;
        }
        appended += &item_rules;
        rules.push((format!("{}__core", head.name), rule));
        index = end;
    }
    if rules.is_empty() {
        return Ok(None);
    }
    Ok(Some(Expanded {
        source: lines.join("\n") + "\n" + &appended,
        rules,
    }))
}

/// Splits one list rule (its lines, the first at `first_line`) into the item
/// rules' source and the core's lines.
fn split(
    head: &Head,
    block: &[String],
    first_line: usize,
) -> Result<(String, Vec<String>, ListRule), String> {
    let lists: Vec<&str> = head
        .params
        .iter()
        .filter(|(_, list)| *list)
        .map(|(p, _)| p.as_str())
        .collect();
    let scalars: Vec<&str> = head
        .params
        .iter()
        .filter(|(_, list)| !*list)
        .map(|(p, _)| p.as_str())
        .collect();
    let length = head.length.as_str();
    let mut sums: Vec<(Fold, String)> = Vec::new();
    // For each entry of `sums`, the plain list sums its item rule takes.
    let mut deps: Vec<Vec<usize>> = Vec::new();
    let mut item_needs: Vec<String> = head
        .item_marks
        .iter()
        .map(|m| format!("need {m}"))
        .collect();
    let mut core = vec![String::new(); block.len()];
    let mut limit: Option<(usize, u64)> = None;
    // `need n >= k`: the sums' plain ranges start at k items, not 0.
    let least = block
        .iter()
        .filter_map(|raw| {
            let (name, k) = strip_comment(raw)
                .trim()
                .strip_prefix("need ")?
                .split_once(">=")?;
            (name.trim() == length).then(|| k.trim().parse::<u64>().ok())?
        })
        .max()
        .unwrap_or(0);
    // An answer line using a list item outside `sum`/`count` makes the answer
    // a list: the core then runs once per item, with the items as inputs.
    let each = block.iter().skip(1).any(|raw| {
        let text = strip_comment(raw).trim();
        let mut rest = Ok(text.to_string());
        for f in ["min", "max", "sum", "count"] {
            let mut blank = |inner: &str| {
                Ok(
                    if f == "sum" || f == "count" || folds_a_list(inner, &lists) {
                        String::new()
                    } else {
                        format!("{f}({inner})")
                    },
                )
            };
            rest = rest.and_then(|t| replace_calls(&t, f, &mut blank));
        }
        text.contains("->") && rest.is_ok_and(|t| lists.iter().any(|l| mentions(&t, l)))
    });

    // `position`: where the item is in the list, from 1 (issue #207).
    let position = block
        .iter()
        .skip(1)
        .any(|raw| mentions(strip_comment(raw), "position"));
    if position && (!each || head.params.iter().any(|(p, _)| p == "position")) {
        return Err(format!(
            "line {first_line}: `position` is the place of an item (from 1) and only works when the answer is a list"
        ));
    }

    // `sum(answer)` in a promise of a list answer is `sum(E)` of its one
    // `always -> E` line.
    let answers: Vec<&str> = block
        .iter()
        .skip(1)
        .map(|raw| strip_comment(raw).trim())
        .filter(|t| t.contains("->"))
        .collect();
    let signed = head.suffix.contains("can be negative");
    let whole = |text: &str, line_no: usize| -> Result<String, String> {
        // `total * w / sum(w)`: each share rounds down and loses less than 1,
        // so with `total` and every weight at least 0 the shares add up to at
        // most `total` (`share_le`) and, with at least one item, more than
        // `total - n` (`share_ge`). The core proves those conditions.
        if let [line] = answers.as_slice()
            && each
            && let Some((_, e)) = line.split_once("->")
            && let compact = e.split_whitespace().collect::<String>()
            && let Some((top, sum)) = compact.split_once("/sum(")
            && let Some((a, b)) = top.split_once('*')
            && let (total, list) = if lists.contains(&a) { (b, a) } else { (a, b) }
            && sum.strip_suffix(')') == Some(list)
            && lists.contains(&list)
            && is_word_text(total)
            && !lists.contains(&total)
            && text.contains("(answer)")
        {
            let rest = text
                .strip_prefix("promise")
                .map(str::trim_start)
                .and_then(|r| r.strip_prefix("sum(answer)"))
                .map(str::trim_start)
                .unwrap_or_default();
            let bound = if mentions(rest, "answer") {
                None
            } else if let Some(y) = rest.strip_prefix("<=") {
                Some(format!(" and {total} <={y}"))
            } else if let Some(y) = rest.strip_prefix(">=") {
                // Shares are never negative, so a bound of 0 or less always holds.
                if y.trim().parse::<i128>().is_ok_and(|v| v <= 0) {
                    Some(String::new())
                } else {
                    (least >= 1).then(|| format!(" and ({}) + {length} <= {total} + 1", y.trim()))
                }
            } else {
                None
            };
            return match bound {
                Some(bound) => Ok(format!("promise {total} >= 0 and {list} >= 0{bound}")),
                None => Err(format!(
                    "line {line_no}: the shares of `{}` round down, so they add up to at most `{total}` and, with `need {length} >= 1`, at least `{total} - {length} + 1`; write `promise sum(answer) <= ...` or `promise sum(answer) >= ...`",
                    e.trim()
                )),
            };
        }
        // Issue #207: `position <= T - sum(E) -> E + 1`, `else -> E` gives the
        // first `T - sum(E)` items one more, so `sum(answer) is T` holds when
        // `0 <= T - sum(E) <= n` (`alloc_sum`); the core proves that range.
        if let [first, second] = answers.as_slice()
            && each
            && let Some(target) = text
                .strip_prefix("promise")
                .map(str::trim)
                .and_then(|r| r.strip_prefix("sum(answer)"))
                .map(str::trim)
                .and_then(|r| r.strip_prefix("is "))
                .map(str::trim)
            && let Some((cond, plus)) = first.split_once("->")
            && let Some(rest) = cond.trim().strip_prefix("position")
            && let Some(rest) = rest.trim().strip_prefix("<=")
            && let Some((t, sum_e)) = rest.split_once(" - ")
            && t.trim() == target
            && let Some(e) = sum_e
                .trim()
                .strip_prefix("sum(")
                .and_then(|r| r.strip_suffix(')'))
            && let Some(e_plus) = plus.trim().strip_suffix("+ 1")
            && e_plus.trim() == e.trim()
            && second
                .strip_prefix("else")
                .map(|r| r.trim().trim_start_matches("->").trim())
                == Some(e.trim())
        {
            return Ok(format!(
                "promise sum({e}) <= {target} and {target} <= sum({e}) + {length}"
            ));
        }
        let mut text = text.to_string();
        for f in ["sum", "min", "max"] {
            text = replace_calls(&text, f, &mut |inner| {
                if inner != "answer" {
                    return Ok(format!("{f}({inner})"));
                }
                match answers.as_slice() {
                    [line] if each && line.starts_with("always") => {
                        let (_, e) = line.split_once("->").unwrap_or_default();
                        Ok(format!("{f}({})", e.trim()))
                    }
                    _ if !each => Err(format!(
                        "line {line_no}: `{f}(answer)` needs an answer that is a list"
                    )),
                    _ => Err(format!(
                        "line {line_no}: `{f}(answer)` needs one `always -> ...` line"
                    )),
                }
            })?;
        }
        Ok(text)
    };

    for (offset, raw) in block.iter().enumerate().skip(1) {
        let line_no = first_line + offset;
        let text = strip_comment(raw);
        let whole_text;
        let trimmed = if text.trim().starts_with("promise ") && mentions(text, "answer") {
            // A promise only about sums must still name the answer to parse;
            // `answer is answer and` keeps its meaning.
            let read = whole(text.trim(), line_no)?;
            whole_text = if read == text.trim() {
                read
            } else {
                read.replacen("promise ", "promise answer is answer and (", 1) + ")"
            };
            whole_text.as_str()
        } else {
            text.trim()
        };
        if trimmed.is_empty() {
            continue;
        }
        let indent = &text[..text.len() - text.trim_start().len()];
        let mut rewrite = |text: &str| -> Result<String, String> {
            let text = replace_calls(text, "count", &mut |inner| {
                if lists.contains(&inner) {
                    Ok(length.to_string())
                } else {
                    Err(format!(
                        "line {line_no}: write `count(list)` with one of the lists"
                    ))
                }
            })?;
            let mut text = text;
            for fold in [Fold::Min, Fold::Max, Fold::Sum] {
                let f = fold.word();
                text = replace_calls(&text, f, &mut |inner| {
                    if fold != Fold::Sum && !folds_a_list(inner, &lists) {
                        return Ok(format!("{f}({inner})"));
                    }
                    if !lists.iter().any(|l| mentions(inner, l)) {
                        return Err(format!(
                            "line {line_no}: `sum(...)` must use one of the lists, like `sum(prices)`"
                        ));
                    }
                    // `sum(total * w / sum(w))`: the inner `sum(w)` of a plain
                    // list is added up first and given to this item as `sumJ`
                    // (two loops, `single_le_sum`).
                    let mut inner = inner.to_string();
                    let mut uses = Vec::new();
                    if fold == Fold::Sum && !signed && inner.contains("sum(") {
                        let mut found = Vec::new();
                        replace_calls(&inner, "sum", &mut |x| {
                            found.push(x.to_string());
                            Ok(String::new())
                        })?;
                        if found.iter().all(|x| lists.contains(&x.as_str())) {
                            for list in &found {
                                let j = match sums
                                    .iter()
                                    .position(|(g, s)| *g == Fold::Sum && s == list)
                                {
                                    Some(j) => j,
                                    None => {
                                        sums.push((Fold::Sum, list.clone()));
                                        deps.push(Vec::new());
                                        sums.len() - 1
                                    }
                                };
                                if !uses.contains(&j) {
                                    uses.push(j);
                                }
                            }
                            inner = replace_calls(&inner, "sum", &mut |x| {
                                let j = sums.iter().position(|(g, s)| *g == Fold::Sum && s == x);
                                Ok(format!("sum{}", j.unwrap_or(0) + 1))
                            })?;
                        }
                    }
                    let inner = inner.as_str();
                    let nested = sums.iter().enumerate().any(|(k, (g, _))| {
                        !uses.contains(&k) && mentions(inner, &format!("{}{}", g.word(), k + 1))
                    });
                    if nested
                        || mentions(inner, length)
                        || inner.contains("sum(")
                        || inner.contains("count(")
                    {
                        return Err(format!(
                            "line {line_no}: inside `{f}(...)` use list items and inputs, not `{length}` or another sum, min or max"
                        ));
                    }
                    let k = match sums.iter().position(|(g, s)| *g == fold && s == inner) {
                        Some(k) => k,
                        None => {
                            sums.push((fold, inner.to_string()));
                            deps.push(uses);
                            sums.len() - 1
                        }
                    };
                    Ok(format!("{f}{}", k + 1))
                })?;
            }
            Ok(text)
        };
        let rewritten = rewrite(trimmed)?;
        let has_list = lists.iter().any(|l| mentions(&rewritten, l));
        if let Some(condition) = trimmed.strip_prefix("need ") {
            // `least` and the limit read one condition per line.
            if condition.contains(" and ") && mentions(condition, length) {
                return Err(format!(
                    "line {line_no}: split this `need` line: put each condition on `{length}` on its own line"
                ));
            }
            if let Some((name, max)) = condition.split_once("<=")
                && name.trim() == length
                && let Ok(max) = max.trim().parse::<u64>()
            {
                if let Some((first, _)) = limit {
                    return Err(format!(
                        "line {line_no}: `need {length} <= ...` already sets the limit on line {}; keep one",
                        first_line + first
                    ));
                }
                limit = Some((offset, max));
                continue;
            }
            let core_like = rewritten != trimmed || mentions(&rewritten, length);
            if has_list && core_like {
                return Err(format!(
                    "line {line_no}: split this `need` line: one for list items, one for `{length}` or `sum(...)`"
                ));
            }
            if has_list {
                item_needs.push(trimmed.to_string());
                if !each {
                    continue;
                }
            }
            if !core_like {
                item_needs.push(trimmed.to_string());
            }
        } else if has_list && !each {
            return Err(format!(
                "line {line_no}: use a list of any length only inside `sum(...)`, `min(...)`, `max(...)` or `count(...)`"
            ));
        }
        core[offset] = format!("{indent}{rewritten}");
    }
    let Some((limit_offset, max)) = limit else {
        return Err(format!(
            "line {first_line}: add `need {length} <= 1000` (any number) to limit how many items `{}` can have",
            lists[0]
        ));
    };
    if !each && sums.is_empty() && !block.iter().any(|l| l.contains("count(")) {
        return Err(format!(
            "line {first_line}: use the lists with `sum(...)`, `min(...)`, `max(...)` or `count(...)`"
        ));
    }
    let suffix = if head.suffix.is_empty() {
        String::new()
    } else {
        format!(" {}", head.suffix)
    };
    let all: Vec<&str> = head.params.iter().map(|(p, _)| p.as_str()).collect();
    let needs: String = item_needs.iter().map(|n| format!("  {n}\n")).collect();
    let item_rule = |name: &str, inner: &str| {
        format!(
            "\n{name}({}){suffix}\n{needs}  always -> {inner}\n",
            all.join(", ")
        )
    };
    let wide = head.suffix.contains("with big numbers");
    let (low, high) = match (signed, wide) {
        (true, false) => (i128::from(i32::MIN), i128::from(i32::MAX)),
        (false, false) => (0, i128::from(u32::MAX)),
        (true, true) => (i128::from(i64::MIN), i128::from(i64::MAX)),
        (false, true) => (0, i128::from(u64::MAX)),
    };
    // The lowest and highest sum of `max` (or `least`) items in `lo..hi`.
    let total = |lo: i128, hi: i128| {
        let (fewest, most) = (i128::from(least), i128::from(max));
        ((most * lo).min(fewest * lo), (most * hi).max(fewest * hi))
    };

    // `sum(paid - 2 * share)` is `sum(paid) - 2 * sum(share)` (`sum_add`,
    // `sum_mul`, `sum_const`, kept modulo the word size by `sum_emod`): the
    // core may assume it, with `sum(paid)` added as an input when missing
    // and in range.
    let mut linear: Vec<(usize, Vec<Term>)> = Vec::new();
    for (k, (fold, inner)) in sums.iter().enumerate() {
        if *fold != Fold::Sum || lists.contains(&inner.as_str()) {
            continue;
        }
        let Ok(module) = crate::parse_module(&item_rule("probe", inner)) else {
            continue;
        };
        if let Some(SpecExpr::Binary(BinaryOp::Eq, _, answer)) = module.contracts[0].ensures.first()
            && let Some(terms) = linear_terms(answer, &lists)
        {
            linear.push((k, terms));
        }
    }
    for list in &lists {
        let used = linear
            .iter()
            .any(|(_, t)| t.iter().any(|(l, _, _)| l.as_deref() == Some(*list)));
        let present = sums.iter().any(|(f, i)| *f == Fold::Sum && i == list);
        if !used || present {
            continue;
        }
        let fits = crate::parse_module(&item_rule("probe", list))
            .ok()
            .and_then(|m| check_ranges(&m.contracts[0], &[]).ok())
            .is_some_and(|(lo, hi)| {
                let (a, b) = total(lo, hi);
                a >= low && b <= high
            });
        if fits {
            sums.push((Fold::Sum, list.to_string()));
            deps.push(Vec::new());
        }
    }

    let taken: Vec<String> = head.params.iter().map(|(p, _)| p.clone()).collect();
    let results: Vec<String> = sums
        .iter()
        .enumerate()
        .map(|(k, (fold, _))| format!("{}{}", fold.word(), k + 1))
        .collect();
    if let Some(clash) = results.iter().find(|s| taken.contains(s)) {
        return Err(format!(
            "line {first_line}: `{clash}` is used for a list result; give the input another name"
        ));
    }

    // `sum(T * w / sum(w))` with every weight at least 0: each share is at
    // most `T` (the item rule promises it), the shares add up to at most `T`
    // (`share_le`) and, with an item, more than `T - n` (`share_ge`).
    let shares: Vec<Option<String>> = sums
        .iter()
        .zip(&deps)
        .map(|((fold, inner), uses)| {
            let compact: String = inner.split_whitespace().collect();
            let [j] = uses.as_slice() else { return None };
            let (top, by) = compact.split_once('/')?;
            let (a, b) = top.split_once('*')?;
            let list = &sums[*j].1;
            let total = if a == list {
                b
            } else if b == list {
                a
            } else {
                return None;
            };
            (*fold == Fold::Sum && by == format!("sum{}", j + 1) && scalars.contains(&total))
                .then(|| total.to_string())
        })
        .collect();

    // Item rules, then their answer ranges.
    let items: Vec<String> = (1..=sums.len())
        .map(|k| format!("{}__item{k}", head.name))
        .collect();
    let mut item_source = String::new();
    for ((name, (_, inner)), uses) in items.iter().zip(&sums).zip(&deps) {
        if uses.is_empty() {
            item_source += &item_rule(name, inner);
            continue;
        }
        // An item taking `sumJ`: every item is at most the sum of its list
        // (`single_le_sum`, items at least 0), and the sum stays in range.
        let mut params: Vec<String> = all.iter().map(|p| p.to_string()).collect();
        let mut extra = String::new();
        for &j in uses {
            let list = &sums[j].1;
            let (_, hi) = crate::parse_module(&item_rule("probe", list))
                .map_err(|error| format!("line {first_line}: {error}"))
                .and_then(|m| check_ranges(&m.contracts[0], &[]))?;
            let name = format!("sum{}", j + 1);
            extra += &format!(
                "  need {name} >= {list} and {name} <= {}\n",
                hi * i128::from(max)
            );
            params.push(name);
        }
        // The Lean proof of that promise uses `mul_div_le` (issue #215) when
        // the share is written `total * weights / sum(weights)`.
        let promise = match &shares[items.iter().position(|i| i == name).unwrap_or(0)] {
            Some(total) => {
                let compact: String = inner.split_whitespace().collect();
                let hint = match uses.as_slice() {
                    [j] if compact.starts_with(&format!("{total}*")) => {
                        format!("  hint mul_div_le({total}, {}, sum{})\n", sums[*j].1, j + 1)
                    }
                    _ => String::new(),
                };
                format!("  promise answer <= {total}\n{hint}")
            }
            None => String::new(),
        };
        item_source += &format!(
            "\n{name}({}){suffix}\n{needs}{extra}  always -> {inner}\n{promise}",
            params.join(", ")
        );
    }
    let mut bounds = String::new();
    if !sums.is_empty() {
        let module = crate::parse_module(&item_source).map_err(|error| {
            format!("line {first_line}: in an item of `{}`: {error}", head.name)
        })?;
        for ((contract, (fold, inner)), s) in module.contracts.iter().zip(&sums).zip(&results) {
            let (lo, hi) = check_ranges(contract, &[])?;
            if *fold != Fold::Sum {
                // `min_bounds`/`max_bounds`: between the items' own range.
                let f = fold.word();
                if least == 0 {
                    return Err(format!(
                        "line {}: `{f}({inner})` needs at least one item; add `need {length} >= 1`",
                        first_line + limit_offset
                    ));
                }
                bounds += &format!(" and {s} >= {lo} and {s} <= {hi}");
                // `min_le`, `max_ge`, `min_le_max`: min <= every item <= max.
                let le = if *fold == Fold::Min { "<=" } else { ">=" };
                if each {
                    bounds += &format!(" and {s} {le} ({inner})");
                }
                for (other, (g, i)) in results.iter().zip(&sums) {
                    if i == inner && *g != *fold && *g != Fold::Sum {
                        bounds += &format!(" and {s} {le} {other}");
                    }
                }
                continue;
            }
            let k = results.iter().position(|r| r == s).unwrap_or(0);
            let hi = match &shares[k] {
                Some(t) => crate::parse_module(&item_rule("probe", t))
                    .and_then(|m| check_ranges(&m.contracts[0], &[]))
                    .map_or(hi, |(_, t_hi)| hi.min(t_hi)),
                None => hi,
            };
            let (total_lo, total_hi) = total(lo, hi);
            if total_lo < low || total_hi > high {
                let reach = if total_hi > high { total_hi } else { total_lo };
                return Err(format!(
                    "line {}: `sum({inner})` can reach {reach} with {max} items, past the number range; lower the limit, limit the items with `need`, or add `with big numbers`",
                    first_line + limit_offset
                ));
            }
            bounds += &format!(
                " and {s} >= {} and {s} <= {} and {s} >= {} and {s} <= {}",
                total_lo,
                total_hi,
                times(length, lo),
                times(length, hi)
            );
        }
    }
    // Subtracted terms go to the left: `sum1 + sum3 is sum2`.
    'facts: for (k, terms) in &linear {
        let mut sides = [vec![results[*k].clone()], Vec::new()];
        for (list, c, minus) in terms {
            let term = match list {
                None => format!("{length} * {c}"),
                Some(list) => {
                    let Some(at) = sums.iter().position(|(f, i)| *f == Fold::Sum && i == list)
                    else {
                        continue 'facts;
                    };
                    format!("{c} * {}", results[at])
                }
            };
            sides[usize::from(!minus)].push(term);
        }
        if sides[1].is_empty() {
            sides[1].push("0".into());
        }
        bounds += &format!(" and {} is {}", sides[0].join(" + "), sides[1].join(" + "));
    }
    for (k, share) in shares.iter().enumerate() {
        let Some(total) = share else { continue };
        bounds += &format!(" and {} <= {total}", results[k]);
        if least >= 1 {
            bounds += &format!(" and {total} + 1 <= {} + {length}", results[k]);
        }
    }
    let indent = {
        let raw = &block[limit_offset];
        raw[..raw.len() - raw.trim_start().len()].to_string()
    };
    core[limit_offset] = format!("{indent}need {length} >= 0 and {length} <= {max}{bounds}");
    let mut core_params: Vec<String> = if each { &all } else { &scalars }
        .iter()
        .map(|s| s.to_string())
        .collect();
    core_params.push(length.to_string());
    if position {
        core_params.push("position".to_string());
        core[limit_offset] += &format!(" and position >= 1 and position <= {length}");
    }
    core_params.extend(results);
    // A core run once per item takes the items, and their hidden `need`s.
    let marks: String = if each {
        head.item_marks
            .iter()
            .map(|m| format!("{}{m}", crate::types::NEED_MARK))
            .collect()
    } else {
        String::new()
    };
    core[0] = format!(
        "{}__core({}){suffix}{marks}",
        head.name,
        core_params.join(", ")
    );
    Ok((
        item_source,
        core,
        ListRule {
            name: head.name.clone(),
            params: head.params.clone(),
            items,
            folds: sums.iter().map(|(fold, _)| *fold).collect(),
            max,
            least,
            each,
            uses: deps,
            position,
        },
    ))
}

#[cfg(test)]
mod tests {
    use crate::{Fold, parse_module};

    fn error(source: &str) -> String {
        parse_module(source).unwrap_err()
    }

    #[test]
    fn calls_inside_quoted_text_stay_as_written() {
        let out = super::replace_calls("\"sum(prices) is big\" + sum(prices)", "sum", &mut |_| {
            Ok("S".into())
        });
        assert_eq!(out.unwrap(), "\"sum(prices) is big\" + S");
    }

    #[test]
    fn a_second_length_limit_is_an_error() {
        parse_module(
            "cart(prices[n])\n  need n <= 3\n  need prices <= 100\n  always -> sum(prices)\n",
        )
        .unwrap();
        let error = parse_module("cart(prices[n])\n  need n <= 3\n  need n <= 9\n  need prices <= 100\n  always -> sum(prices)\n").unwrap_err();
        assert!(error.contains("keep one"), "{error}");
    }

    #[test]
    fn a_share_sum_at_least_zero_reports_the_division_not_the_rounding() {
        // Without `need n >= 1` the division is the problem; "sum >= 0" is not.
        assert!(
            error("share(weights[n], total)\n  need n <= 50\n  need weights <= 1000\n  need total <= 1000000\n  always -> total * weights / sum(weights)\n  promise sum(answer) >= 0\n")
                .contains("may divide by 0")
        );
    }

    #[test]
    fn position_needs_a_list_answer() {
        assert!(
            error(
                "f(a[n])\n  need n <= 5\n  need a <= 10\n  position <= 2 -> sum(a)\n  else -> 0\n"
            )
            .contains("only works when the answer is a list")
        );
    }

    #[test]
    fn an_exact_total_needs_the_rest_given_out_one_each() {
        let rule = "split(weights[n], total)\n  need n <= 50\n  need n >= 1\n  need weights >= 1 and weights <= 1000\n  need total <= 1000000\n";
        let core = parse_module(&format!("{rule}  position <= total - sum(total * weights / sum(weights)) -> total * weights / sum(weights) + 1\n  else -> total * weights / sum(weights)\n  promise sum(answer) is total\n")).unwrap();
        let list = core.contracts.iter().find_map(|c| c.list.clone()).unwrap();
        assert!(list.position);
        assert_eq!(list.uses, vec![vec![], vec![0]]);
        // Two of each extra unit is not the rule `alloc_sum` proves.
        assert!(
            error(&format!("{rule}  position <= total - sum(total * weights / sum(weights)) -> total * weights / sum(weights) + 2\n  else -> total * weights / sum(weights)\n  promise sum(answer) is total\n"))
                .contains("needs one `always -> ...` line")
        );
    }

    #[test]
    fn two_conditions_on_one_length_line_must_be_split() {
        assert!(
            error("f(a[n])\n  need n >= 1 and n <= 5\n  always -> sum(a)\n")
                .contains("on its own line")
        );
    }

    #[test]
    fn splits_a_rule_into_item_rules_and_a_core() {
        let module = parse_module(
            "cart(prices[n], fee)\n  need n <= 1000\n  need prices <= 100000\n  need fee <= 10\n  always -> sum(prices) + count(prices) * fee\n",
        )
        .unwrap();
        let core = module
            .contracts
            .iter()
            .find(|c| c.name == "cart__core")
            .unwrap();
        let names: Vec<&str> = core.params.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["fee", "n", "sum1"]);
        let rule = core.list.as_ref().unwrap();
        assert_eq!(rule.items, ["cart__item1"]);
        assert_eq!(rule.max, 1000);
        let item = module
            .contracts
            .iter()
            .find(|c| c.name == "cart__item1")
            .unwrap();
        assert_eq!(item.params.len(), 2);
    }

    #[test]
    fn an_answer_using_items_is_a_list() {
        let module = parse_module(
            "cut(prices[n], rate)\n  need n <= 10\n  need prices <= 100\n  need rate <= 100\n  always -> prices * rate / 100 + sum(prices)\n",
        )
        .unwrap();
        let core = module
            .contracts
            .iter()
            .find(|c| c.name == "cut__core")
            .unwrap();
        let names: Vec<&str> = core.params.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["prices", "rate", "n", "sum1"]);
        assert!(core.list.as_ref().unwrap().each);
    }

    #[test]
    fn sum_of_a_list_answer_is_the_sum_of_its_line() {
        let module = parse_module(
            "gap(paid[n], share[n]) can be negative\n  need n <= 10\n  need paid >= 0 and paid <= 100\n  need share >= 0 and share <= 100\n  always -> paid - share\n  promise sum(answer) is sum(paid) - sum(share)\n",
        )
        .unwrap();
        let rule = module
            .contracts
            .iter()
            .find(|c| c.name == "gap__core")
            .unwrap()
            .list
            .clone()
            .unwrap();
        assert!(rule.each);
        assert_eq!(rule.folds.len(), 3);
        assert!(
            error("cart(prices[n])\n  need n <= 10\n  prices > 5 -> 1\n  else -> 0\n  promise sum(answer) <= n\n")
                .contains("one `always")
        );
        assert!(
            error("cart(prices[n])\n  need n <= 10\n  always -> sum(prices)\n  promise sum(answer) <= 5\n")
                .contains("answer that is a list")
        );
    }

    #[test]
    fn shares_add_up_to_at_most_the_total() {
        let share = "share(weights[n], total)\n  need n <= 50\n  need n >= 1\n  need weights >= 1 and weights <= 1000\n  need total <= 1000000\n  always -> total * weights / sum(weights)\n";
        parse_module(&format!("{share}  promise sum(answer) <= total\n")).unwrap();
        parse_module(&format!("{share}  promise sum(answer) >= total - n + 1\n")).unwrap();
        // Spacing and the order of `*` do not matter.
        let tight = share.replace(
            "total * weights / sum(weights)",
            "weights*total/sum(weights)",
        );
        parse_module(&format!("{tight}  promise sum(answer)<=total\n")).unwrap();
        assert!(
            error(&format!("{share}  promise sum(answer) is total\n")).contains("at most `total`")
        );
    }

    #[test]
    fn min_and_max_of_a_list_are_core_inputs() {
        let module = parse_module(
            "spread(prices[n], cap)\n  need n <= 10\n  need n >= 1\n  need prices <= 100\n  need cap <= 100\n  always -> max(prices) - min(prices) + min(cap, 5)\n",
        )
        .unwrap();
        let core = module
            .contracts
            .iter()
            .find(|c| c.name == "spread__core")
            .unwrap();
        let names: Vec<&str> = core.params.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["cap", "n", "min1", "max2"]);
        let rule = core.list.as_ref().unwrap();
        assert_eq!(rule.folds, [Fold::Min, Fold::Max]);
        assert!(
            error("f(a[n])\n  need n <= 10\n  always -> max(a)\n")
                .contains("needs at least one item; add `need n >= 1`")
        );
    }

    #[test]
    fn a_sum_of_plus_minus_times_relates_to_the_lists_sums() {
        let module = parse_module(
            "net(prices[n], off[n], fee)\n  need n <= 100\n  need prices <= 1000\n  need off <= prices\n  need fee <= 10\n  always -> sum(2 * prices - off + fee)\n",
        )
        .unwrap();
        let core = module
            .contracts
            .iter()
            .find(|c| c.name == "net__core")
            .unwrap();
        let names: Vec<&str> = core.params.iter().map(|p| p.name.as_str()).collect();
        // `sum(prices)` and `sum(off)` are added for the relation.
        assert_eq!(names, ["fee", "n", "sum1", "sum2", "sum3"]);
        let parsed = parse_module(
            "f(sum1, sum2, sum3, n, fee)\n  need sum1 + 1 * sum3 is 2 * 1 * sum2 + n * fee\n  always -> 0\n",
        )
        .unwrap();
        let fact = &parsed.contracts[0].requires[0];
        assert!(format!("{:?}", core.requires).contains(&format!("{fact:?}")));
        // `/` is not `+ - *`: no relation, no added sums.
        let module = parse_module(
            "half(prices[n])\n  need n <= 10\n  need prices <= 100\n  always -> sum(prices / 2)\n",
        )
        .unwrap();
        let core = module
            .contracts
            .iter()
            .find(|c| c.name == "half__core")
            .unwrap();
        assert_eq!(core.params.len(), 2);
    }

    #[test]
    fn explains_what_a_list_rule_needs() {
        assert!(
            error("cart(prices[n])\n  need prices <= 100\n  always -> sum(prices)\n")
                .contains("add `need n <= 1000`")
        );
        assert!(
            error("cart(prices[n])\n  need n <= 100000\n  need prices <= 100000\n  always -> sum(prices)\n")
                .contains("can reach 10000000000 with 100000 items")
        );
        assert!(
            error("cart(prices[n])\n  need n <= 10\n  always -> sum(prices)\n  promise answer <= prices\n")
                .contains("only inside `sum(...)`, `min(...)`, `max(...)` or `count(...)`")
        );
        assert!(
            error("f(a[n], b[m])\n  need n <= 10\n  always -> sum(a)\n")
                .contains("share one length")
        );
        assert!(
            error("f(a[n])\n  need n <= 10\n  need a <= 5 and n >= 1\n  always -> sum(a)\n")
                .contains("split this `need` line")
        );
    }
}
