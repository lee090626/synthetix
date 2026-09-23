use std::time::Instant;

use axiom_parser::parse;
use axiom_synth::{SynthesisOptions, SynthesisResult, synthesize_result_with_options};

fn main() {
    let cases = [
        ("mul8", include_str!("../../../examples/basics/mul8.ax")),
        (
            "property_max",
            include_str!("../../../examples/properties/property_max.ax"),
        ),
        (
            "property_clamp",
            include_str!("../../../examples/properties/property_clamp.ax"),
        ),
    ];
    let options = SynthesisOptions::default();

    println!(
        "{{\"format_version\":1,\"purpose\":\"non-guarantee synthesis baseline\",\"profile\":\"release\",\"os\":{},\"arch\":{},\"benchmarks\":[",
        json(std::env::consts::OS),
        json(std::env::consts::ARCH),
    );
    for (index, (name, source)) in cases.iter().enumerate() {
        let contract = parse(source).expect("baseline fixture must parse");
        let started = Instant::now();
        let result = synthesize_result_with_options(&contract, options)
            .expect("baseline fixture must complete without a solver error");
        let elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;
        let (candidate_count, candidate) = match &result {
            SynthesisResult::Found {
                candidate,
                candidates_checked,
                ..
            } => (Some(*candidates_checked), Some(format!("{candidate:?}"))),
            SynthesisResult::NoCandidate {
                candidates_checked, ..
            }
            | SynthesisResult::Timeout { candidates_checked }
            | SynthesisResult::ResourceLimit {
                candidates_checked, ..
            } => (Some(*candidates_checked), None),
        };
        let separator = if index + 1 == cases.len() { "" } else { "," };
        println!(
            "{{\"benchmark\":{},\"status\":{},\"candidate_count\":{},\"elapsed_ms\":{elapsed_ms:.3},\"candidate\":{},\"search_bounds\":{{\"timeout_ms\":null,\"max_candidates\":{},\"max_memory_bytes\":{}}}}}{separator}",
            json(name),
            json(result.status_name()),
            candidate_count.map_or_else(|| "null".to_string(), |count| count.to_string()),
            candidate.map_or_else(|| "null".to_string(), |candidate| json(&candidate)),
            options.max_candidates,
            options.max_memory_bytes,
        );
    }
    println!("]}}");
}

fn json(value: &str) -> String {
    let mut result = String::with_capacity(value.len() + 2);
    result.push('"');
    for character in value.chars() {
        match character {
            '"' => result.push_str("\\\""),
            '\\' => result.push_str("\\\\"),
            '\n' => result.push_str("\\n"),
            '\r' => result.push_str("\\r"),
            '\t' => result.push_str("\\t"),
            character if character.is_control() => {
                result.push_str(&format!("\\u{:04x}", character as u32));
            }
            character => result.push(character),
        }
    }
    result.push('"');
    result
}
