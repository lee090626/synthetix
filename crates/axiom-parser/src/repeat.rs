//! Repeated steps (`repeat years`, issue #214).
//!
//! ```text
//! savings(principal, rate, years)
//!   need years <= 100
//!   need principal <= 1000000
//!   need rate <= 20
//!   start -> principal
//!   repeat years
//!   value + value * rate / 100 > 100000000 -> 100000000
//!   else -> value + value * rate / 100
//!   promise answer >= principal
//!   promise answer <= 100000000
//! ```
//!
//! The answer is `start`, then the step applied `years` times, where `value`
//! is the answer so far. The rule is split before translation into two
//! ordinary rules, both proven like any rule:
//!
//! - `savings__start(inputs)`: `always -> principal` with every promise;
//! - `savings__step(inputs, value)`: the `->` lines, with every promise as a
//!   `need` on `value` and again as a promise on the answer.
//!
//! So each promise holds at the start and every step keeps it; by induction
//! (`loop_inv` in `proofs/lean/Repeat.lean`) it holds after any number of
//! steps. Emitted code runs the start, then the step in a loop.

use crate::ast::RepeatRule;
use crate::simple::strip_comment;

pub(crate) struct Expanded {
    pub source: String,
    /// For each step rule, its repeat shape.
    pub rules: Vec<(String, RepeatRule)>,
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// `text` with every whole word `from` (outside quotes) replaced by `to`.
fn rename(text: &str, from: &str, to: &str) -> String {
    let mut out = String::new();
    let mut word = String::new();
    let mut quoted = false;
    for c in text.chars().chain(std::iter::once(' ')) {
        if !quoted && is_word(c) {
            word.push(c);
            continue;
        }
        if !word.is_empty() {
            out += if word == from { to } else { &word };
            word.clear();
        }
        if c == '"' {
            quoted = !quoted;
        }
        out.push(c);
    }
    out.pop();
    out
}

pub(crate) fn expand(source: &str) -> Result<Option<Expanded>, String> {
    let mut lines: Vec<String> = source.lines().map(String::from).collect();
    let mut appended = String::new();
    let mut rules = Vec::new();
    let mut index = 0;
    while index < lines.len() {
        let text = strip_comment(&lines[index]).to_string();
        let top = !text.trim().is_empty() && !text.starts_with(' ') && !text.starts_with('\t');
        let mut end = index + 1;
        while end < lines.len() {
            let t = strip_comment(&lines[end]);
            if !t.trim().is_empty() && !t.starts_with(' ') && !t.starts_with('\t') {
                break;
            }
            end += 1;
        }
        let repeats = top
            && lines[index + 1..end]
                .iter()
                .any(|l| strip_comment(l).trim().starts_with("repeat "));
        if !repeats {
            index = end;
            continue;
        }
        let (start, step, rule) = split(&lines[index..end], index + 1)?;
        for (offset, line) in step.into_iter().enumerate() {
            lines[index + offset] = line;
        }
        appended += &start;
        rules.push((format!("{}__step", rule.name), rule));
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

/// The start rule's source, the step rule's lines (same count as `block`)
/// and the shape.
fn split(block: &[String], first_line: usize) -> Result<(String, Vec<String>, RepeatRule), String> {
    let head = strip_comment(&block[0]).trim().to_string();
    let error = |line: usize, message: &str| format!("line {line}: {message}");
    let (name, rest) = head.split_once('(').ok_or_else(|| {
        error(
            first_line,
            "a rule with `repeat` starts with `name(inputs)`",
        )
    })?;
    let (params, suffix) = rest
        .split_once(')')
        .ok_or_else(|| error(first_line, "close the inputs with `)`"))?;
    let name = name.trim().to_string();
    let params: Vec<String> = params
        .split(',')
        .map(|p| p.trim().to_string())
        .filter(|p| !p.is_empty())
        .collect();
    if params.iter().any(|p| p.contains('[')) {
        return Err(error(first_line, "`repeat` does not work with lists yet"));
    }
    if params.iter().any(|p| p == "value") {
        return Err(error(
            first_line,
            "`value` is the answer so far in a `repeat` rule; give the input another name",
        ));
    }
    let suffix = suffix.trim();
    let suffix = if suffix.is_empty() {
        String::new()
    } else {
        format!(" {suffix}")
    };

    let mut needs = Vec::new();
    let mut promises = Vec::new();
    let mut start = None;
    let mut count = None;
    let mut step = vec![String::new(); block.len()];
    for (offset, raw) in block.iter().enumerate().skip(1) {
        let line = first_line + offset;
        let text = strip_comment(raw);
        let trimmed = text.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Some(value) = trimmed.strip_prefix("start ->") {
            if start.replace(value.trim().to_string()).is_some() {
                return Err(error(line, "write one `start -> ...` line"));
            }
        } else if let Some(word) = trimmed.strip_prefix("repeat ") {
            let word = word.trim();
            let Some(at) = params.iter().position(|p| p == word) else {
                return Err(error(
                    line,
                    "write `repeat name` with one of the inputs: how many steps",
                ));
            };
            if count.replace(at).is_some() {
                return Err(error(line, "write one `repeat` line"));
            }
        } else if trimmed.starts_with("need ") {
            needs.push(trimmed.to_string());
            step[offset] = text.to_string();
        } else if let Some(promise) = trimmed.strip_prefix("promise ") {
            promises.push(promise.trim().to_string());
            step[offset] = text.to_string();
        } else {
            if count.is_none() || start.is_none() {
                return Err(error(
                    line,
                    "put `start -> ...` and `repeat name` before the step's `->` lines",
                ));
            }
            step[offset] = text.to_string();
        }
    }
    let (Some(start), Some(count)) = (start, count) else {
        return Err(error(
            first_line,
            "a `repeat` rule needs a `start -> ...` line and a `repeat name` line",
        ));
    };
    let counted = &params[count];
    let max = needs
        .iter()
        .filter_map(|n| {
            let (left, right) = n.strip_prefix("need ")?.split_once("<=")?;
            (left.trim() == counted.as_str())
                .then(|| right.trim().parse::<u64>().ok())
                .flatten()
        })
        .min()
        .ok_or_else(|| {
            error(
                first_line,
                &format!("add `need {counted} <= 1000` (any number) to limit how many steps run"),
            )
        })?;
    if promises.is_empty() {
        return Err(error(
            first_line,
            "a `repeat` rule needs a `promise` about the answer: it must hold at the start and after every step",
        ));
    }

    // Each promise holds for `value` before the step: a `need` of the step.
    let assumed: String = promises
        .iter()
        .map(|p| format!("  need {}\n", rename(p, "answer", "value")))
        .collect();
    let with_value: Vec<&str> = params
        .iter()
        .map(String::as_str)
        .chain(std::iter::once("value"))
        .collect();
    step[0] = format!("{name}__step({}){suffix}", with_value.join(", "));
    if let Some(first_need) = step
        .iter()
        .position(|l| l.trim_start().starts_with("need "))
    {
        step[first_need] = format!("{assumed}{}", step[first_need]);
    } else {
        step[0] += &format!("\n{}", assumed.trim_end());
    }

    let needs: String = needs.iter().map(|n| format!("  {n}\n")).collect();
    let kept: String = promises
        .iter()
        .map(|p| format!("  promise {p}\n"))
        .collect();
    let start_rule = format!(
        "\n{name}__start({}){suffix}\n{needs}  always -> {start}\n{kept}",
        params.join(", ")
    );
    Ok((
        start_rule,
        step,
        RepeatRule {
            name: name.clone(),
            start: format!("{name}__start"),
            count,
            max,
        },
    ))
}

#[cfg(test)]
mod tests {
    use crate::parse_module;

    const SAVINGS: &str = "savings(principal, rate, years)\n  need years <= 100\n  need principal <= 1000000\n  need rate <= 20\n  start -> principal\n  repeat years\n  value + value * rate / 100 > 100000000 -> 100000000\n  else -> value + value * rate / 100\n  promise answer >= principal\n  promise answer <= 100000000\n";

    #[test]
    fn splits_into_a_start_and_a_step() {
        let module = parse_module(SAVINGS).unwrap();
        let names: Vec<&str> = module.contracts.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["savings__step", "savings__start"]);
        let step = &module.contracts[0];
        assert_eq!(step.params.len(), 4);
        let rule = step.repeat.as_ref().unwrap();
        assert_eq!((rule.count, rule.max), (2, 100));
        assert_eq!(rule.start, "savings__start");
        // Two promises assumed of `value`, plus the three `need` lines.
        assert_eq!(step.requires.len(), 5);
    }

    #[test]
    fn a_repeat_rule_needs_a_limit_and_a_promise() {
        let no_limit = SAVINGS.replace("  need years <= 100\n", "");
        assert!(
            parse_module(&no_limit)
                .unwrap_err()
                .contains("need years <= 1000")
        );
        let no_promise = SAVINGS.replace(
            "  promise answer >= principal\n  promise answer <= 100000000\n",
            "",
        );
        assert!(
            parse_module(&no_promise)
                .unwrap_err()
                .contains("needs a `promise`")
        );
    }

    #[test]
    fn renames_whole_words_only() {
        assert_eq!(
            super::rename("answer + answers", "answer", "value"),
            "value + answers"
        );
    }
}
