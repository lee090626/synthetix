//! A rule that calls itself once less (issue #225, M2).
//!
//! ```text
//! triangle(n)
//!   need n <= 1000
//!   n is 0 -> 0
//!   else -> n + triangle(n - 1)
//!   promise answer * 2 is n * (n + 1)
//!   promise answer <= 1000000
//! ```
//!
//! The first `->` line answers for 0; the other lines may call the rule with
//! the same inputs and `n - 1` for `n`. The rule is split before translation
//! into two ordinary rules, both proven like any rule:
//!
//! - `triangle__start(n)`: `always -> 0`, every promise with `n` as 0;
//! - `triangle__step(n__prev, value)`: the other lines for `n = n__prev + 1`,
//!   with the call as `value` and every promise for `n__prev` as a `need` on
//!   `value`.
//!
//! So each promise holds for 0, and for `n` whenever it holds for `n - 1`; by
//! induction (`rec_inv` in `proofs/lean/Repeat.lean`) it holds for every `n`.
//! Emitted code runs the start, then the step for `n__prev` = 0, 1, ... n - 1:
//! the `repeat` loop, with the step's place as the counted input.

use crate::ast::RepeatRule;
use crate::repeat::rename;
use crate::simple::strip_comment;

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Byte ranges of each `name(...)` call in `text` (outside quotes), with
/// the text between the parentheses.
fn calls(text: &str, name: &str) -> Vec<(usize, usize, String)> {
    let mut found = Vec::new();
    let mut quoted = false;
    let mut previous: Option<char> = None;
    for (at, c) in text.char_indices() {
        if c == '"' {
            quoted = !quoted;
        }
        let starts = !quoted
            && !previous.is_some_and(is_word)
            && text[at..].starts_with(name)
            && text[at + name.len()..].starts_with('(');
        previous = Some(c);
        if !starts {
            continue;
        }
        let open = at + name.len();
        let mut depth = 0;
        for (offset, d) in text[open..].char_indices() {
            match d {
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth == 0 {
                        let end = open + offset + 1;
                        found.push((at, end, text[open + 1..end - 1].to_string()));
                        break;
                    }
                }
                _ => {}
            }
        }
    }
    found
}

/// Whether a top-level rule block calls itself.
pub(crate) fn recurses(block: &[String]) -> bool {
    let head = strip_comment(&block[0]);
    let Some((name, _)) = head.split_once('(') else {
        return false;
    };
    let name = name.trim();
    !name.is_empty()
        && block[1..]
            .iter()
            .any(|l| !calls(strip_comment(l), name).is_empty())
}

fn squash(text: &str) -> String {
    text.split_whitespace().collect()
}

/// The start rule's source, the step rule's lines (same count as `block`)
/// and the shape.
pub(crate) fn split(
    block: &[String],
    first_line: usize,
) -> Result<(String, Vec<String>, RepeatRule), String> {
    let error = |line: usize, message: &str| format!("line {line}: {message}");
    let head = strip_comment(&block[0]).trim().to_string();
    let (name, rest) = head
        .split_once('(')
        .ok_or_else(|| error(first_line, "a rule starts with `name(inputs)`"))?;
    let (params, suffix) = rest
        .split_once(')')
        .ok_or_else(|| error(first_line, "close the inputs with `)`"))?;
    let name = name.trim().to_string();
    let params: Vec<String> = params
        .split(',')
        .map(|p| p.trim().to_string())
        .filter(|p| !p.is_empty())
        .collect();
    if !suffix.trim().is_empty() {
        return Err(error(
            first_line,
            &format!(
                "a rule that calls itself does not take `{}` yet",
                suffix.trim()
            ),
        ));
    }
    if params.iter().any(|p| p.contains('[')) {
        return Err(error(
            first_line,
            "a rule that calls itself does not work with lists yet",
        ));
    }
    if params.iter().any(|p| p == "value") {
        return Err(error(
            first_line,
            "`value` is the answer of the call in a rule that calls itself; give the input another name",
        ));
    }

    // The lines, sorted.
    let mut needs = Vec::new();
    let mut promises = Vec::new();
    let mut answers = Vec::new();
    for (offset, raw) in block.iter().enumerate().skip(1) {
        let line = first_line + offset;
        let trimmed = strip_comment(raw).trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Some(need) = trimmed.strip_prefix("need ") {
            needs.push((offset, need.trim().to_string()));
        } else if let Some(promise) = trimmed.strip_prefix("promise ") {
            promises.push(promise.trim().to_string());
        } else if trimmed.contains("->") {
            answers.push((offset, trimmed.to_string()));
        } else {
            return Err(error(
                line,
                "a rule that calls itself has only `need`, `->` and `promise` lines yet; write the named value into the `->` lines",
            ));
        }
    }
    for (offset, need) in &needs {
        if !calls(need, &name).is_empty() {
            return Err(error(
                first_line + offset,
                &format!("call `{name}` only in `->` lines"),
            ));
        }
    }
    if promises.iter().any(|p| !calls(p, &name).is_empty()) {
        return Err(error(
            first_line,
            &format!("call `{name}` only in `->` lines"),
        ));
    }

    // The first `->` line answers for 0: `n is 0 -> ...`.
    let Some(((base_offset, base_line), steps)) = answers.split_first() else {
        return Err(error(first_line, "write the `->` lines"));
    };
    let base_error = || {
        error(
            first_line + base_offset,
            &format!(
                "a rule that calls itself starts with the answer for 0: `n is 0 -> ...`, where `n` is the input that goes down by 1 in each call to `{name}`"
            ),
        )
    };
    let (condition, base) = base_line.split_once("->").ok_or_else(base_error)?;
    let measure = condition
        .trim()
        .strip_suffix(" is 0")
        .map(str::trim)
        .ok_or_else(base_error)?;
    let Some(count) = params.iter().position(|p| p == measure) else {
        return Err(base_error());
    };
    if !calls(base, &name).is_empty() {
        return Err(error(
            first_line + base_offset,
            &format!("the answer for 0 cannot call `{name}`"),
        ));
    }
    if steps.is_empty() {
        return Err(error(
            first_line + base_offset,
            &format!("add `else -> ...` with a call to `{name}`"),
        ));
    }

    // Every call takes the same inputs, with `n - 1` for `n`.
    let expected: Vec<String> = params
        .iter()
        .map(|p| {
            if p == measure {
                squash(&format!("{measure}-1"))
            } else {
                p.clone()
            }
        })
        .collect();
    let mut called = false;
    for (offset, text) in steps {
        for (_, _, args) in calls(text, &name) {
            let args: Vec<String> = args.split(',').map(squash).collect();
            if args != expected {
                return Err(error(
                    first_line + offset,
                    &format!(
                        "call `{name}({})`: the same inputs, with `{measure} - 1` for `{measure}`",
                        params
                            .iter()
                            .map(|p| if p == measure {
                                format!("{measure} - 1")
                            } else {
                                p.clone()
                            })
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                ));
            }
            called = true;
        }
    }
    if !called {
        return Err(error(
            first_line,
            &format!("a rule that calls itself calls `{name}` in a `->` line"),
        ));
    }

    // Lines about `n` limit it from above only: each call keeps them.
    let mut max = None;
    for (offset, need) in &needs {
        if rename(need, measure, "") == *need {
            continue;
        }
        let limit = need
            .split_once("<=")
            .filter(|(left, _)| left.trim() == measure)
            .and_then(|(_, right)| right.trim().parse::<u64>().ok());
        let Some(limit) = limit else {
            return Err(error(
                first_line + offset,
                &format!(
                    "a line about `{measure}` is `need {measure} <= limit` only: each call to `{name}` takes `{measure} - 1`, which must meet it too"
                ),
            ));
        };
        max = Some(max.map_or(limit, |m: u64| m.min(limit)));
    }
    let Some(max) = max else {
        return Err(error(
            first_line,
            &format!("add `need {measure} <= 1000` (any number) to limit how many calls run"),
        ));
    };
    if max == 0 {
        return Err(error(
            first_line,
            &format!("`need {measure} <= 0` leaves only the answer for 0: drop the call"),
        ));
    }
    if promises.is_empty() {
        return Err(error(
            first_line,
            "a rule that calls itself needs a `promise` about the answer: it must hold for 0 and for `n` whenever it holds for `n - 1`",
        ));
    }

    // The step, for `n = prev + 1`; the call's answer is `value`.
    let prev = format!("{measure}__prev");
    let now = format!("({prev} + 1)");
    let step_params: Vec<String> = params
        .iter()
        .map(|p| {
            if p == measure {
                prev.clone()
            } else {
                p.clone()
            }
        })
        .chain(std::iter::once("value".to_string()))
        .collect();
    let mut step = vec![String::new(); block.len()];
    step[0] = format!("{name}__step({})", step_params.join(", "));
    let assumed: Vec<String> = promises
        .iter()
        .map(|p| {
            format!(
                "  need {}",
                rename(&rename(p, "answer", "value"), measure, &prev)
            )
        })
        .collect();
    step[0] += &format!("\n{}", assumed.join("\n"));
    for (offset, need) in &needs {
        step[*offset] = if rename(need, measure, "") == *need {
            format!("  need {need}")
        } else {
            format!("  need {prev} <= {}", max - 1)
        };
    }
    let single_else = steps.len() == 1;
    for (offset, text) in steps {
        let mut text = text.clone();
        for (start, end, _) in calls(&text, &name).into_iter().rev() {
            text.replace_range(start..end, "value");
        }
        let mut text = rename(&text, measure, &now);
        if single_else && let Some(rest) = text.strip_prefix("else ->") {
            text = format!("always ->{rest}");
        }
        step[*offset] = format!("  {text}");
    }
    let promised: String = promises
        .iter()
        .map(|p| format!("\n  promise {}", rename(p, measure, &now)))
        .collect();
    let last = steps.last().map_or(0, |(offset, _)| *offset);
    step[last] += &promised;

    let start_needs: String = needs.iter().map(|(_, n)| format!("  need {n}\n")).collect();
    let start_promises: String = promises
        .iter()
        .map(|p| format!("  promise {}\n", rename(p, measure, "0")))
        .collect();
    let start_rule = format!(
        "\n{name}__start({})\n{start_needs}  always -> {}\n{start_promises}",
        params.join(", "),
        rename(base.trim(), measure, "0"),
    );
    Ok((
        start_rule,
        step,
        RepeatRule {
            name: name.clone(),
            start: format!("{name}__start"),
            count,
            max,
            counted: true,
        },
    ))
}

#[cfg(test)]
mod tests {
    use crate::parse_module;

    const TRIANGLE: &str = "triangle(n)\n  need n <= 1000\n  n is 0 -> 0\n  else -> n + triangle(n - 1)\n  promise answer * 2 is n * (n + 1)\n  promise answer <= 1000000\n";

    #[test]
    fn splits_into_a_start_and_a_step() {
        let module = parse_module(TRIANGLE).unwrap();
        let names: Vec<&str> = module.contracts.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["triangle__step", "triangle__start"]);
        let step = &module.contracts[0];
        let params: Vec<&str> = step.params.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(params, ["n__prev", "value"]);
        let rule = step.repeat.as_ref().unwrap();
        assert_eq!((rule.count, rule.max, rule.counted), (0, 1000, true));
        assert_eq!(rule.start, "triangle__start");
    }

    #[test]
    fn a_call_takes_the_same_inputs_and_one_less() {
        let other = TRIANGLE.replace("triangle(n - 1)", "triangle(n - 2)");
        assert!(
            parse_module(&other)
                .unwrap_err()
                .contains("`n - 1` for `n`")
        );
        let no_base = TRIANGLE.replace("  n is 0 -> 0\n", "");
        assert!(
            parse_module(&no_base)
                .unwrap_err()
                .contains("the answer for 0")
        );
    }

    #[test]
    fn lines_about_the_input_are_limits_only() {
        let lower = TRIANGLE.replace("  need n <= 1000\n", "  need n <= 1000\n  need n >= 3\n");
        assert!(
            parse_module(&lower)
                .unwrap_err()
                .contains("`need n <= limit` only")
        );
        let none = TRIANGLE.replace("  need n <= 1000\n", "");
        assert!(parse_module(&none).unwrap_err().contains("need n <= 1000"));
        let no_promise = TRIANGLE.replace(
            "  promise answer * 2 is n * (n + 1)\n  promise answer <= 1000000\n",
            "",
        );
        assert!(
            parse_module(&no_promise)
                .unwrap_err()
                .contains("needs a `promise`")
        );
    }
}
