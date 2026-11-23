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

use crate::ast::{Fold, ListRule};
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

/// Replaces every `f(inner)` call (not part of a longer name) with `with(inner)`.
fn replace_calls(
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
    let mut item_needs: Vec<String> = Vec::new();
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

    for (offset, raw) in block.iter().enumerate().skip(1) {
        let line_no = first_line + offset;
        let text = strip_comment(raw);
        let trimmed = text.trim();
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
                    let nested = sums
                        .iter()
                        .enumerate()
                        .any(|(k, (g, _))| mentions(inner, &format!("{}{}", g.word(), k + 1)));
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
            if let Some((name, max)) = condition.split_once("<=")
                && name.trim() == length
                && let Ok(max) = max.trim().parse::<u64>()
            {
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

    // Item rules, then their answer ranges.
    let suffix = if head.suffix.is_empty() {
        String::new()
    } else {
        format!(" {}", head.suffix)
    };
    let all: Vec<&str> = head.params.iter().map(|(p, _)| p.as_str()).collect();
    let needs: String = item_needs.iter().map(|n| format!("  {n}\n")).collect();
    let items: Vec<String> = (1..=sums.len())
        .map(|k| format!("{}__item{k}", head.name))
        .collect();
    let mut item_source = String::new();
    for (name, (_, inner)) in items.iter().zip(&sums) {
        item_source += &format!(
            "\n{name}({}){suffix}\n{needs}  always -> {inner}\n",
            all.join(", ")
        );
    }
    let signed = head.suffix.contains("can be negative");
    let wide = head.suffix.contains("with big numbers");
    let (low, high) = match (signed, wide) {
        (true, false) => (i128::from(i32::MIN), i128::from(i32::MAX)),
        (false, false) => (0, i128::from(u32::MAX)),
        (true, true) => (i128::from(i64::MIN), i128::from(i64::MAX)),
        (false, true) => (0, i128::from(u64::MAX)),
    };
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
            let (fewest, most) = (i128::from(least), i128::from(max));
            let (total_lo, total_hi) = ((most * lo).min(fewest * lo), (most * hi).max(fewest * hi));
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
    core_params.extend(results);
    core[0] = format!("{}__core({}){suffix}", head.name, core_params.join(", "));
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
