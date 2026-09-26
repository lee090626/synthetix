use std::{env, fs, path::PathBuf};

use axiom_parser::parse;
use axiom_synth::{SynthesisMetrics, SynthesisOptions, SynthesisResult};
use serde::Serialize;

#[derive(Serialize)]
struct Report<'a> {
    scenario: &'a str,
    status: &'a str,
    classification: &'a str,
    timeout_ms: Option<u64>,
    max_candidates: Option<usize>,
    estimated_memory_limit_bytes: Option<usize>,
    candidate_count: Option<usize>,
    generated_candidate_count: Option<usize>,
    rejected_candidate_count: Option<usize>,
    synthesis_iterations: Option<usize>,
    search_batches: Option<usize>,
    peak_resource_estimate_bytes: Option<usize>,
    detail: String,
}

fn main() {
    if let Err(error) = run() {
        eprintln!(
            "{{\"status\":\"ERROR\",\"error\":{}}}",
            serde_json::to_string(&error).unwrap()
        );
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let mut args = env::args().skip(1);
    let scenario = args.next().ok_or("usage: v7_memory_case <scenario>")?;
    if args.next().is_some() {
        return Err("expected exactly one scenario".into());
    }
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|path| path.parent())
        .ok_or("cannot locate repository root")?
        .to_path_buf();
    let (relative, inputs) = match scenario.as_str() {
        "cegis_hidden_nine" => return run_cegis(&root, &scenario),
        "fixed_array_sum4" => ("benchmarks/v6/final/array_sum4.ax", vec![vec![1, 2, 3, 4]]),
        "bounded_loop_sum4" => (
            "benchmarks/v6/final/bounded_loop_sum4.ax",
            vec![vec![1, 2, 3, 4]],
        ),
        "bounded_recursion_sum_to" => (
            "benchmarks/v6/final/recursive_sum_to.ax",
            vec![vec![0], vec![1], vec![16]],
        ),
        _ => return Err(format!("unknown scenario: {scenario}")),
    };
    let source = fs::read_to_string(root.join(relative))
        .map_err(|error| format!("cannot read {relative}: {error}"))?;
    let contract = parse(&source).map_err(|error| format!("{relative}: {error}"))?;
    let candidate = axiom_verify::implementation_to_ami(&contract)?;
    let proof = axiom_verify::verify(&contract, &candidate)?;
    if proof != axiom_verify::ProofResult::Verified {
        return emit(
            &scenario,
            proof.status_name(),
            proof.status_name(),
            None,
            None,
            None,
            format!("{proof:?}"),
        );
    }
    let (_, abi_arity) = axiom_parser::verified_abi_layout(&contract.params)?;
    let wasm = axiom_wasm::emit(
        &candidate,
        u32::try_from(abi_arity).map_err(|_| "WASM argument count overflow")?,
    )?;
    let mut outputs = Vec::new();
    for input in inputs {
        if input.len() != abi_arity {
            return Err(format!("{relative}: input does not match flattened ABI"));
        }
        let reference = axiom_verify::reference::eval_ami(&candidate, &input)?;
        let actual = axiom_wasm::run_with_args(&wasm, &input)?;
        if reference != actual {
            return Err(format!(
                "{relative}: reference/WASM mismatch for {input:?}: {reference} != {actual}"
            ));
        }
        outputs.push(serde_json::json!({"inputs": input, "reference": reference, "wasm": actual}));
    }
    emit(
        &scenario,
        "VERIFIED",
        "verified_feature_proof",
        None,
        None,
        None,
        serde_json::to_string(
            &serde_json::json!({"source": relative, "wasm_bytes": wasm.len(), "outputs": outputs}),
        )
        .unwrap(),
    )
}

fn run_cegis(root: &std::path::Path, scenario: &str) -> Result<(), String> {
    let relative = "benchmarks/v6/synthesis/cegis_hidden_nine.ax";
    let source = fs::read_to_string(root.join(relative))
        .map_err(|error| format!("cannot read {relative}: {error}"))?;
    let contract = parse(&source).map_err(|error| format!("{relative}: {error}"))?;
    let limit = 268_435_456;
    let (result, metrics) = axiom_synth::synthesize_result_with_metrics(
        &contract,
        SynthesisOptions {
            timeout: None,
            max_candidates: 50_000,
            max_memory_bytes: limit,
        },
    )?;
    let classification = match &result {
        SynthesisResult::ResourceLimit { .. }
            if metrics.search_batches == 0 && metrics.initial_sample_count == 0 =>
        {
            "sample_cartesian_product_or_zero_budget_limit"
        }
        SynthesisResult::ResourceLimit { .. }
            if metrics.generated_candidate_count > 0 && metrics.synthesis_iterations == 0 =>
        {
            "candidate_generation_bound_reached_before_verification"
        }
        SynthesisResult::ResourceLimit { .. } => "explicit_search_resource_limit",
        SynthesisResult::Timeout { .. } => "explicit_search_timeout",
        SynthesisResult::NoCandidate { .. } => "bounded_grammar_exhausted_not_impossibility",
        SynthesisResult::Found { .. } => "none",
    };
    let detail = match &result {
        SynthesisResult::ResourceLimit {
            candidates_checked,
            limit,
        } => format!("candidates_checked={candidates_checked}; limit={limit}"),
        SynthesisResult::Timeout { candidates_checked } => {
            format!("candidates_checked={candidates_checked}")
        }
        SynthesisResult::NoCandidate {
            candidates_checked, ..
        } => format!("candidates_checked={candidates_checked}"),
        SynthesisResult::Found {
            candidate,
            candidates_checked,
            ..
        } => format!("candidates_checked={candidates_checked}; candidate={candidate:?}"),
    };
    emit_with_metrics(
        scenario,
        result.status_name(),
        classification,
        None,
        Some(50_000),
        Some(limit),
        &metrics,
        detail,
    )
}

fn emit(
    scenario: &str,
    status: &str,
    classification: &str,
    timeout_ms: Option<u64>,
    max_candidates: Option<usize>,
    limit: Option<usize>,
    detail: String,
) -> Result<(), String> {
    emit_with_metrics(
        scenario,
        status,
        classification,
        timeout_ms,
        max_candidates,
        limit,
        &SynthesisMetrics::default(),
        detail,
    )
}

#[allow(clippy::too_many_arguments)]
fn emit_with_metrics(
    scenario: &str,
    status: &str,
    classification: &str,
    timeout_ms: Option<u64>,
    max_candidates: Option<usize>,
    limit: Option<usize>,
    metrics: &SynthesisMetrics,
    detail: String,
) -> Result<(), String> {
    let report = Report {
        scenario,
        status,
        classification,
        timeout_ms,
        max_candidates,
        estimated_memory_limit_bytes: limit,
        candidate_count: (metrics.candidate_count > 0).then_some(metrics.candidate_count),
        generated_candidate_count: (metrics.generated_candidate_count > 0)
            .then_some(metrics.generated_candidate_count),
        rejected_candidate_count: (metrics.rejected_candidate_count > 0)
            .then_some(metrics.rejected_candidate_count),
        synthesis_iterations: (metrics.synthesis_iterations > 0)
            .then_some(metrics.synthesis_iterations),
        search_batches: (metrics.search_batches > 0).then_some(metrics.search_batches),
        peak_resource_estimate_bytes: (metrics.peak_resource_estimate_bytes > 0)
            .then_some(metrics.peak_resource_estimate_bytes),
        detail,
    };
    println!(
        "{}",
        serde_json::to_string(&report).map_err(|error| error.to_string())?
    );
    Ok(())
}
