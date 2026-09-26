use std::{
    collections::BTreeMap,
    env, fs,
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use axiom_ami::Expr;
use axiom_application::ApplicationValue;
use axiom_parser::{Contract, DeclarationKind, SpecExpr, parse};
use axiom_project::ContractId;
use axiom_synth::{SynthesisOptions, SynthesisResult};
use axiom_verify::ProofResult;
use serde::{Deserialize, Serialize};

const REPEATS: usize = 3;
const DEFAULT_MAX_CANDIDATES: usize = 50_000;
const DEFAULT_MAX_MEMORY_BYTES: usize = 64 * 1024 * 1024;

#[derive(Debug, Clone, Copy, Serialize)]
struct SearchBounds {
    timeout_ms: Option<u128>,
    max_candidates: Option<usize>,
    max_memory_bytes: Option<usize>,
}

#[derive(Debug, Serialize)]
struct Environment {
    commit_sha: String,
    operating_system: String,
    architecture: String,
    working_tree_dirty: bool,
    rustc: String,
    node: Option<String>,
    build_mode: &'static str,
    measured_repeats: usize,
    timing_note: &'static str,
    resource_estimate_note: &'static str,
}

#[derive(Debug, Clone, Serialize)]
struct Measurement {
    name: String,
    category: String,
    source: String,
    contract_count: Option<usize>,
    summary_count: Option<usize>,
    function_call_depth: Option<usize>,
    expression_node_count: Option<usize>,
    parameter_count: Option<usize>,
    candidate_count: Option<usize>,
    generated_candidate_count: Option<usize>,
    rejected_candidate_count: Option<usize>,
    synthesis_iterations: Option<usize>,
    search_batch_count: Option<usize>,
    counterexample_count: Option<usize>,
    initial_sample_count: Option<usize>,
    peak_sample_count: Option<usize>,
    synthesis_time_ms: Option<f64>,
    synthesis_non_solver_time_ms: Option<f64>,
    instrumented_synthesis_time_ms: Option<f64>,
    candidate_generation_time_ms: Option<f64>,
    sample_generation_time_ms: Option<f64>,
    typed_filter_time_ms: Option<f64>,
    resource_estimation_time_ms: Option<f64>,
    candidate_sort_time_ms: Option<f64>,
    sample_evaluation_time_ms: Option<f64>,
    other_search_residual_time_ms: Option<f64>,
    precondition_check_invocation_count: Option<usize>,
    precondition_translation_time_ms: Option<f64>,
    z3_verification_time_ms: Option<f64>,
    solver_invocation_count: Option<usize>,
    project_link_time_ms: Option<f64>,
    summary_verification_time_ms: Option<f64>,
    artifact_generation_time_ms: Option<f64>,
    build_time_ms: Option<f64>,
    runtime_boot_time_ms: Option<f64>,
    runtime_transition_time_ms: Option<f64>,
    runtime_transition_ms_per_action: Option<f64>,
    effect_time_ms: Option<f64>,
    persistence_restore_time_ms: Option<f64>,
    serialization_time_ms: Option<f64>,
    scalar_extraction_time_ms: Option<f64>,
    total_time_ms: Option<f64>,
    peak_resource_estimate_bytes: Option<usize>,
    source_bytes: Option<usize>,
    artifact_bytes: Option<usize>,
    wasm_bytes: Option<usize>,
    cache_hit_count: Option<usize>,
    cache_miss_count: Option<usize>,
    cache_invalidation_count: Option<usize>,
    serialized_bytes: Option<usize>,
    storage_bytes: Option<usize>,
    list_size: Option<usize>,
    record_field_count: Option<usize>,
    text_payload_bytes: Option<usize>,
    scalar_extraction_count: Option<usize>,
    search_bounds: SearchBounds,
    result: String,
    failure_classification: Option<String>,
    resource_status: Option<String>,
    resource_limit_value: Option<usize>,
    details: Option<String>,
}

#[derive(Debug, Serialize)]
struct Baseline {
    format_version: u32,
    generated_at_unix_ms: u128,
    environment: Environment,
    default_search_bounds: SearchBounds,
    outcome_counts: BTreeMap<String, usize>,
    outcomes_not_observed: Vec<String>,
    measurements: Vec<Measurement>,
}

#[derive(Clone)]
struct SynthOutcome {
    measurement: Measurement,
    contract: Option<Contract>,
    candidate: Option<Expr>,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("v6 benchmark failed: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let (json_path, markdown_path, title) = output_paths()?;
    let root = repository_root();
    let default_bounds = SearchBounds {
        timeout_ms: None,
        max_candidates: Some(DEFAULT_MAX_CANDIDATES),
        max_memory_bytes: Some(DEFAULT_MAX_MEMORY_BYTES),
    };
    let mut measurements = Vec::new();
    let mut synth_outputs = BTreeMap::<String, SynthOutcome>::new();

    let synth_cases = [
        ("mul8", "examples/basics/mul8.ax", default_bounds),
        (
            "property_max",
            "examples/properties/property_max.ax",
            default_bounds,
        ),
        (
            "property_clamp",
            "examples/properties/property_clamp.ax",
            default_bounds,
        ),
        (
            "minimum",
            "benchmarks/v6/synthesis/minimum.ax",
            default_bounds,
        ),
        (
            "maximum_3",
            "benchmarks/v6/synthesis/maximum_3.ax",
            default_bounds,
        ),
        (
            "bitwise_combo",
            "benchmarks/v6/synthesis/bitwise_combo.ax",
            default_bounds,
        ),
        ("sum_3", "benchmarks/v6/synthesis/sum_3.ax", default_bounds),
        ("sum_8", "benchmarks/v6/synthesis/sum_8.ax", default_bounds),
        (
            "cegis_hidden_nine",
            "benchmarks/v6/synthesis/cegis_hidden_nine.ax",
            SearchBounds {
                timeout_ms: None,
                max_candidates: Some(DEFAULT_MAX_CANDIDATES),
                max_memory_bytes: Some(256 * 1024 * 1024),
            },
        ),
        (
            "power_5_bounded",
            "benchmarks/v6/synthesis/power_5.ax",
            SearchBounds {
                timeout_ms: Some(2_000),
                max_candidates: Some(1_000),
                max_memory_bytes: Some(DEFAULT_MAX_MEMORY_BYTES),
            },
        ),
    ];

    for (name, relative, bounds) in synth_cases {
        let path = root.join(relative);
        let outcome = run_synthesis_case(name, relative, &path, options(bounds))?;
        measurements.push(outcome.measurement.clone());
        synth_outputs.insert(name.to_string(), outcome);
    }

    let timeout = SearchBounds {
        timeout_ms: Some(0),
        max_candidates: Some(100),
        max_memory_bytes: Some(DEFAULT_MAX_MEMORY_BYTES),
    };
    measurements.push(
        run_synthesis_case(
            "explicit_timeout",
            "examples/basics/mul8.ax",
            &root.join("examples/basics/mul8.ax"),
            options(timeout),
        )?
        .measurement,
    );
    let resource = SearchBounds {
        timeout_ms: None,
        max_candidates: Some(0),
        max_memory_bytes: Some(DEFAULT_MAX_MEMORY_BYTES),
    };
    measurements.push(
        run_synthesis_case(
            "explicit_candidate_resource_limit",
            "examples/basics/mul8.ax",
            &root.join("examples/basics/mul8.ax"),
            options(resource),
        )?
        .measurement,
    );

    for parameter_count in [1, 2, 4, 8, 16] {
        measurements.push(run_sum_verification_case(parameter_count)?);
    }
    for branch_count in [1, 2, 4, 8] {
        measurements.push(run_branch_verification_case(branch_count)?);
    }
    if let Some(maximum) = synth_outputs.get("property_max")
        && let (Some(contract), Some(_candidate)) = (&maximum.contract, &maximum.candidate)
    {
        measurements.push(run_verification_case(
            "counterexample_property_max_arg0",
            "examples/properties/property_max.ax",
            contract,
            &Expr::Arg(0),
            None,
        ));
    }

    for depth in [1, 2, 4, 8] {
        measurements.push(run_compositional_case(depth, &root)?);
    }
    measurements.extend(run_incremental_cache_cases(&root)?);
    measurements.push(run_call_precondition_diagnostic_case()?);
    measurements.push(run_circular_call_case()?);
    measurements.extend(run_application_value_cases());
    measurements.extend(run_static_runtime_cases(&root)?);
    measurements.push(run_v6_release_path_case(&root)?);
    measurements.push(run_array_boundary_case(&root)?);
    measurements.extend(run_unrolled_loop_cases(&root, default_bounds)?);
    measurements.push(run_verified_feature_case(
        "fixed_array_sum4",
        "benchmarks/v6/final/array_sum4.ax",
        &root,
        &[vec![1, 2, 3, 4]],
    )?);
    measurements.push(run_verified_feature_case(
        "bounded_loop_sum4",
        "benchmarks/v6/final/bounded_loop_sum4.ax",
        &root,
        &[vec![1, 2, 3, 4]],
    )?);
    measurements.push(run_verified_feature_case(
        "bounded_recursion_sum_to",
        "benchmarks/v6/final/recursive_sum_to.ax",
        &root,
        &[vec![0], vec![1], vec![16]],
    )?);

    let mut outcome_counts = BTreeMap::new();
    for measurement in &measurements {
        *outcome_counts
            .entry(measurement.result.clone())
            .or_insert(0usize) += 1;
    }
    let outcomes_not_observed = ["NO_CANDIDATE"]
        .into_iter()
        .filter(|status| !outcome_counts.contains_key(*status))
        .map(str::to_string)
        .collect();

    let environment = Environment {
        commit_sha: command_output("git", &["rev-parse", "HEAD"], &root)
            .unwrap_or_else(|| "unknown".to_string()),
        operating_system: env::consts::OS.to_string(),
        architecture: env::consts::ARCH.to_string(),
        working_tree_dirty: command_output("git", &["status", "--porcelain"], &root)
            .is_some_and(|value| !value.trim().is_empty()),
        rustc: command_output("rustc", &["--version"], &root)
            .unwrap_or_else(|| "unknown".to_string()),
        node: command_output("node", &["--version"], &root),
        build_mode: if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        },
        measured_repeats: REPEATS,
        timing_note: "Wall-clock and solver durations are host-dependent. Synthesis, verifier timings, application JSON costs, compositional/release CLI builds, and proof-cache cases are medians of three runs; proof-cache timings come from the debug cargo test profile. Other generated static bundle/runtime rows are single runs and may include cold-start overhead. Generated timestamp is intentionally variable; bounds and counts are recorded per case.",
        resource_estimate_note: "Peak estimate sums the synthesizer's candidate AST estimate and CEGIS sample-vector estimate; it excludes allocator, Z3, process, and browser-engine overhead.",
    };
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| error.to_string())?
        .as_millis();
    let baseline = Baseline {
        format_version: 1,
        generated_at_unix_ms: now,
        environment,
        default_search_bounds: default_bounds,
        outcome_counts,
        outcomes_not_observed,
        measurements,
    };
    write_outputs(&baseline, &json_path, &markdown_path, &title)?;
    println!("wrote {}", json_path.display());
    println!("wrote {}", markdown_path.display());
    println!("measurements: {}", baseline.measurements.len());
    Ok(())
}

fn output_paths() -> Result<(PathBuf, PathBuf, String), String> {
    let mut json = PathBuf::from("docs/v6-benchmark-baseline.json");
    let mut markdown = PathBuf::from("docs/v6-benchmark.md");
    let mut title = "v6.0 Scale Benchmark Baseline".to_string();
    let args = env::args().skip(1).collect::<Vec<_>>();
    let mut index = 0;
    while index < args.len() {
        let value = args
            .get(index + 1)
            .ok_or_else(|| format!("missing output path after {}", args[index]))?;
        match args[index].as_str() {
            "--json" => json = PathBuf::from(value),
            "--markdown" => markdown = PathBuf::from(value),
            "--title" => title = value.clone(),
            flag => return Err(format!("unknown benchmark option: {flag}")),
        }
        index += 2;
    }
    Ok((json, markdown, title))
}

fn repository_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("CLI crate must be nested under crates/")
        .to_path_buf()
}

fn options(bounds: SearchBounds) -> SynthesisOptions {
    SynthesisOptions {
        timeout: bounds
            .timeout_ms
            .map(|value| Duration::from_millis(value as u64)),
        max_candidates: bounds.max_candidates.unwrap_or(DEFAULT_MAX_CANDIDATES),
        max_memory_bytes: bounds.max_memory_bytes.unwrap_or(DEFAULT_MAX_MEMORY_BYTES),
    }
}

fn run_synthesis_case(
    name: &str,
    source_name: &str,
    source_path: &Path,
    search_options: SynthesisOptions,
) -> Result<SynthOutcome, String> {
    let source = fs::read_to_string(source_path)
        .map_err(|error| format!("cannot read {}: {error}", source_path.display()))?;
    let contract = parse(&source).map_err(|error| format!("{}: {error}", source_path.display()))?;
    let bounds = SearchBounds {
        timeout_ms: search_options.timeout.map(|value| value.as_millis()),
        max_candidates: Some(search_options.max_candidates),
        max_memory_bytes: Some(search_options.max_memory_bytes),
    };
    let mut elapsed = Vec::new();
    let mut runs = Vec::new();
    for _ in 0..REPEATS {
        let started = Instant::now();
        let result = axiom_synth::synthesize_result_with_metrics(&contract, search_options);
        elapsed.push(started.elapsed().as_nanos());
        runs.push(result);
    }
    let signatures = runs
        .iter()
        .map(|run| match run {
            Ok((result, metrics)) => format!(
                "{}|{}|{}|{}|{}|{}|{}|{}|{}",
                result.status_name(),
                result.candidates_checked(),
                metrics.candidate_count,
                metrics.generated_candidate_count,
                metrics.rejected_candidate_count,
                metrics.synthesis_iterations,
                metrics.counterexample_count,
                metrics.solver_invocations,
                match result {
                    SynthesisResult::Found { candidate, .. } => format!("{candidate:?}"),
                    _ => String::new(),
                }
            ),
            Err(error) => format!("ERROR|{error}"),
        })
        .collect::<Vec<_>>();
    let deterministic = signatures.windows(2).all(|pair| pair[0] == pair[1]);
    let first = runs.first().cloned().expect("at least one run");
    let mut record = blank_measurement(name, "synthesis", source_name, bounds);
    record.contract_count = Some(1);
    record.expression_node_count = Some(contract_expression_nodes(&contract));
    record.parameter_count = Some(contract.params.len());
    record.source_bytes = Some(source.len());
    let mut candidate = None;
    match first {
        Ok((result, metrics)) => {
            record.result = result.status_name().to_string();
            record.failure_classification = synthesis_failure_class(&result, &metrics);
            if let SynthesisResult::ResourceLimit { limit, .. } = &result {
                record.resource_status = Some("RESOURCE_LIMIT".to_string());
                record.resource_limit_value = Some(*limit);
            }
            record.candidate_count = Some(metrics.candidate_count);
            record.generated_candidate_count = Some(metrics.generated_candidate_count);
            record.rejected_candidate_count = Some(metrics.rejected_candidate_count);
            record.synthesis_iterations = Some(metrics.synthesis_iterations);
            record.search_batch_count = Some(metrics.search_batches);
            record.counterexample_count = Some(metrics.counterexample_count);
            record.initial_sample_count = Some(metrics.initial_sample_count);
            record.peak_sample_count = Some(metrics.peak_sample_count);
            record.solver_invocation_count = Some(metrics.solver_invocations);
            record.precondition_check_invocation_count =
                Some(metrics.precondition_check_invocations);
            record.precondition_translation_time_ms =
                Some(metrics.precondition_translation_time_ns as f64 / 1e6);
            record.z3_verification_time_ms = Some(metrics.solver_time_ns as f64 / 1e6);
            record.peak_resource_estimate_bytes = Some(metrics.peak_resource_estimate_bytes);
            record.details = Some(format!(
                "repeat_deterministic={deterministic}; candidate={}",
                match &result {
                    SynthesisResult::Found { candidate, .. } => format!("{candidate:?}"),
                    _ => String::new(),
                }
            ));
            candidate = match result {
                SynthesisResult::Found {
                    candidate: value, ..
                } => Some(value),
                _ => None,
            };
        }
        Err(error) => {
            record.result = "ERROR".to_string();
            record.failure_classification = Some(classify_error(&error));
            record.details = Some(format!("{error}; repeat_deterministic={deterministic}"));
        }
    }
    // Use the median from each run for the detailed timing counters.
    let metric_runs = runs
        .iter()
        .filter_map(|run| run.as_ref().ok().map(|(_, metrics)| *metrics))
        .collect::<Vec<_>>();
    if !metric_runs.is_empty() {
        record.candidate_generation_time_ms = Some(
            median_u128(
                &metric_runs
                    .iter()
                    .map(|m| m.candidate_generation_time_ns)
                    .collect::<Vec<_>>(),
            ) as f64
                / 1e6,
        );
        record.typed_filter_time_ms = Some(
            median_u128(
                &metric_runs
                    .iter()
                    .map(|m| m.typed_filter_time_ns)
                    .collect::<Vec<_>>(),
            ) as f64
                / 1e6,
        );
        record.resource_estimation_time_ms = Some(
            median_u128(
                &metric_runs
                    .iter()
                    .map(|m| m.resource_estimation_time_ns)
                    .collect::<Vec<_>>(),
            ) as f64
                / 1e6,
        );
        record.candidate_sort_time_ms = Some(
            median_u128(
                &metric_runs
                    .iter()
                    .map(|m| m.candidate_sort_time_ns)
                    .collect::<Vec<_>>(),
            ) as f64
                / 1e6,
        );
        record.sample_evaluation_time_ms = Some(
            median_u128(
                &metric_runs
                    .iter()
                    .map(|m| m.sample_evaluation_time_ns)
                    .collect::<Vec<_>>(),
            ) as f64
                / 1e6,
        );
        record.sample_generation_time_ms = Some(
            median_u128(
                &metric_runs
                    .iter()
                    .map(|m| m.sample_generation_time_ns)
                    .collect::<Vec<_>>(),
            ) as f64
                / 1e6,
        );
        record.z3_verification_time_ms = Some(
            median_u128(
                &metric_runs
                    .iter()
                    .map(|m| m.solver_time_ns)
                    .collect::<Vec<_>>(),
            ) as f64
                / 1e6,
        );
        record.solver_invocation_count = Some(median_usize(
            &metric_runs
                .iter()
                .map(|m| m.solver_invocations)
                .collect::<Vec<_>>(),
        ));
        record.precondition_check_invocation_count = Some(median_usize(
            &metric_runs
                .iter()
                .map(|m| m.precondition_check_invocations)
                .collect::<Vec<_>>(),
        ));
        record.precondition_translation_time_ms = Some(
            median_u128(
                &metric_runs
                    .iter()
                    .map(|m| m.precondition_translation_time_ns)
                    .collect::<Vec<_>>(),
            ) as f64
                / 1e6,
        );
        record.peak_resource_estimate_bytes = Some(
            metric_runs
                .iter()
                .map(|m| m.peak_resource_estimate_bytes)
                .max()
                .unwrap_or(0),
        );
        record.search_batch_count = Some(median_usize(
            &metric_runs
                .iter()
                .map(|m| m.search_batches)
                .collect::<Vec<_>>(),
        ));
        record.initial_sample_count = Some(median_usize(
            &metric_runs
                .iter()
                .map(|m| m.initial_sample_count)
                .collect::<Vec<_>>(),
        ));
        record.peak_sample_count = Some(
            metric_runs
                .iter()
                .map(|m| m.peak_sample_count)
                .max()
                .unwrap_or(0),
        );
        let cex_reused = metric_runs.iter().any(|m| {
            m.counterexample_count > 0
                && m.peak_sample_count > m.initial_sample_count
                && m.synthesis_iterations > 1
        });
        if cex_reused {
            record.details = Some(format!(
                "{}; counterexample_reused_by_later_candidate=true",
                record.details.as_deref().unwrap_or_default()
            ));
        } else if metric_runs
            .iter()
            .any(|m| m.counterexample_count > 0 && m.peak_sample_count > m.initial_sample_count)
        {
            record.details = Some(format!(
                "{}; counterexample_sample_added=true; later_candidate_verification=false",
                record.details.as_deref().unwrap_or_default()
            ));
        }
        record.instrumented_synthesis_time_ms = Some(
            median_u128(
                &metric_runs
                    .iter()
                    .map(|metrics| metrics.total_synthesis_time_ns)
                    .collect::<Vec<_>>(),
            ) as f64
                / 1e6,
        );
        record.other_search_residual_time_ms = Some(
            median_u128(
                &metric_runs
                    .iter()
                    .map(|metrics| metrics.other_search_residual_time_ns)
                    .collect::<Vec<_>>(),
            ) as f64
                / 1e6,
        );
    }
    record.total_time_ms = Some(median_duration_ns(&elapsed) as f64 / 1e6);
    record.synthesis_time_ms = record.total_time_ms;
    record.synthesis_non_solver_time_ms = Some(
        (record.synthesis_time_ms.unwrap_or_default()
            - record.z3_verification_time_ms.unwrap_or_default())
        .max(0.0),
    );
    Ok(SynthOutcome {
        measurement: record,
        contract: Some(contract),
        candidate,
    })
}

fn blank_measurement(
    name: &str,
    category: &str,
    source: &str,
    bounds: SearchBounds,
) -> Measurement {
    Measurement {
        name: name.to_string(),
        category: category.to_string(),
        source: source.to_string(),
        contract_count: None,
        summary_count: None,
        function_call_depth: None,
        expression_node_count: None,
        parameter_count: None,
        candidate_count: None,
        generated_candidate_count: None,
        rejected_candidate_count: None,
        synthesis_iterations: None,
        search_batch_count: None,
        counterexample_count: None,
        initial_sample_count: None,
        peak_sample_count: None,
        synthesis_time_ms: None,
        synthesis_non_solver_time_ms: None,
        instrumented_synthesis_time_ms: None,
        candidate_generation_time_ms: None,
        sample_generation_time_ms: None,
        typed_filter_time_ms: None,
        resource_estimation_time_ms: None,
        candidate_sort_time_ms: None,
        sample_evaluation_time_ms: None,
        other_search_residual_time_ms: None,
        precondition_check_invocation_count: None,
        precondition_translation_time_ms: None,
        z3_verification_time_ms: None,
        solver_invocation_count: None,
        project_link_time_ms: None,
        summary_verification_time_ms: None,
        artifact_generation_time_ms: None,
        build_time_ms: None,
        runtime_boot_time_ms: None,
        runtime_transition_time_ms: None,
        runtime_transition_ms_per_action: None,
        effect_time_ms: None,
        persistence_restore_time_ms: None,
        serialization_time_ms: None,
        scalar_extraction_time_ms: None,
        total_time_ms: None,
        peak_resource_estimate_bytes: None,
        source_bytes: None,
        artifact_bytes: None,
        wasm_bytes: None,
        cache_hit_count: None,
        cache_miss_count: None,
        cache_invalidation_count: None,
        serialized_bytes: None,
        storage_bytes: None,
        list_size: None,
        record_field_count: None,
        text_payload_bytes: None,
        scalar_extraction_count: None,
        search_bounds: bounds,
        result: "NOT_RUN".to_string(),
        failure_classification: None,
        resource_status: None,
        resource_limit_value: None,
        details: None,
    }
}

fn run_sum_verification_case(parameter_count: usize) -> Result<Measurement, String> {
    let names = (0..parameter_count)
        .map(|index| format!("p{index}"))
        .collect::<Vec<_>>();
    let params = names
        .iter()
        .map(|name| format!("{name}: u32"))
        .collect::<Vec<_>>()
        .join(", ");
    let expression = names.join(" +% ");
    let source = format!(
        "contract sum{parameter_count}({params}) -> u32 {{ ensures result == ({expression}); }}"
    );
    let contract = parse(&source).map_err(|error| error.to_string())?;
    let candidate = (1..parameter_count).fold(Expr::Arg(0), |sum, index| {
        Expr::Add(Box::new(sum), Box::new(Expr::Arg(index as u8)))
    });
    Ok(run_verification_case(
        &format!("z3_sum_params_{parameter_count}"),
        &format!("generated:u32 sum with {parameter_count} scalar parameters"),
        &contract,
        &candidate,
        None,
    ))
}

fn run_branch_verification_case(branch_count: usize) -> Result<Measurement, String> {
    let names = (0..branch_count)
        .map(|index| format!("p{index}"))
        .collect::<Vec<_>>();
    let params = names
        .iter()
        .map(|name| format!("{name}: u32"))
        .collect::<Vec<_>>()
        .join(", ");
    let mut spec = "0".to_string();
    let mut candidate = Expr::Const(0);
    for (index, name) in names.iter().enumerate().rev() {
        spec = format!("if {name} > 0 {{ {name} }} else {{ {spec} }}");
        candidate = Expr::Select {
            condition: Box::new(Expr::Gt(
                Box::new(Expr::Arg(index as u8)),
                Box::new(Expr::Const(0)),
            )),
            then_branch: Box::new(Expr::Arg(index as u8)),
            else_branch: Box::new(candidate),
        };
    }
    let source =
        format!("contract choose{branch_count}({params}) -> u32 {{ ensures result == ({spec}); }}");
    let contract = parse(&source).map_err(|error| error.to_string())?;
    Ok(run_verification_case(
        &format!("z3_branch_count_{branch_count}"),
        &format!("generated nested conditional with {branch_count} branches"),
        &contract,
        &candidate,
        None,
    ))
}

fn run_verification_case(
    name: &str,
    source_name: &str,
    contract: &Contract,
    candidate: &Expr,
    bounds: Option<SearchBounds>,
) -> Measurement {
    let no_search_bounds = bounds.unwrap_or(SearchBounds {
        timeout_ms: None,
        max_candidates: None,
        max_memory_bytes: None,
    });
    let mut elapsed = Vec::new();
    let mut run_results = Vec::new();
    for _ in 0..REPEATS {
        let started = Instant::now();
        let result = axiom_verify::verify_with_metrics(contract, candidate);
        elapsed.push(started.elapsed().as_nanos());
        run_results.push(result);
    }
    let signatures = run_results
        .iter()
        .map(|run| match run {
            Ok((ProofResult::Verified, metrics)) => {
                format!("VERIFIED|{}", metrics.solver_invocations)
            }
            Ok((ProofResult::Counterexample(inputs), metrics)) => {
                format!("COUNTEREXAMPLE|{inputs:?}|{}", metrics.solver_invocations)
            }
            Ok((ProofResult::SolverUnknown, metrics)) => {
                format!("SOLVER_UNKNOWN|{}", metrics.solver_invocations)
            }
            Err(error) => format!("ERROR|{error}"),
        })
        .collect::<Vec<_>>();
    let deterministic = signatures.windows(2).all(|pair| pair[0] == pair[1]);
    let solver_runs = run_results
        .iter()
        .filter_map(|run| run.as_ref().ok().map(|(_, metrics)| *metrics))
        .collect::<Vec<_>>();
    let mut record = blank_measurement(name, "z3_verification", source_name, no_search_bounds);
    record.contract_count = Some(1);
    record.function_call_depth = Some(0);
    record.expression_node_count = Some(contract_expression_nodes(contract));
    record.parameter_count = Some(contract.params.len());
    record.source_bytes = Some(source_name.len());
    record.total_time_ms = Some(median_duration_ns(&elapsed) as f64 / 1e6);
    if let Some(Ok((proof, _))) = run_results.first() {
        record.result = proof.status_name().to_string();
        record.failure_classification = match proof {
            ProofResult::Verified => None,
            ProofResult::Counterexample(_) => Some("solver_found_counterexample".to_string()),
            ProofResult::SolverUnknown => Some("solver_unknown".to_string()),
        };
        record.details = Some(format!(
            "candidate={candidate:?}; repeat_deterministic={deterministic}"
        ));
    } else if let Some(Err(error)) = run_results.first() {
        record.result = "ERROR".to_string();
        record.failure_classification = Some(classify_error(error));
        record.details = Some(error.clone());
    }
    if !solver_runs.is_empty() {
        record.solver_invocation_count = Some(median_usize(
            &solver_runs
                .iter()
                .map(|metrics| metrics.solver_invocations)
                .collect::<Vec<_>>(),
        ));
        record.z3_verification_time_ms = Some(
            median_u128(
                &solver_runs
                    .iter()
                    .map(|metrics| metrics.solver_time_ns)
                    .collect::<Vec<_>>(),
            ) as f64
                / 1e6,
        );
    }
    record
}

fn contract_expression_nodes(contract: &Contract) -> usize {
    contract
        .requires
        .iter()
        .chain(&contract.ensures)
        .map(spec_expression_nodes)
        .sum::<usize>()
        + contract
            .implementation
            .as_ref()
            .map(spec_expression_nodes)
            .unwrap_or(0)
}

fn spec_expression_nodes(expr: &SpecExpr) -> usize {
    1 + match expr {
        SpecExpr::Binary(_, lhs, rhs) => spec_expression_nodes(lhs) + spec_expression_nodes(rhs),
        SpecExpr::Let(_, value, body) => spec_expression_nodes(value) + spec_expression_nodes(body),
        SpecExpr::If(condition, then_branch, else_branch) => {
            spec_expression_nodes(condition)
                + spec_expression_nodes(then_branch)
                + spec_expression_nodes(else_branch)
        }
        SpecExpr::Call(_, args) | SpecExpr::List(args) => {
            args.iter().map(spec_expression_nodes).sum()
        }
        SpecExpr::Index(array, index) => {
            spec_expression_nodes(array) + spec_expression_nodes(index)
        }
        SpecExpr::ArrayUpdate(array, index, value) => {
            spec_expression_nodes(array)
                + spec_expression_nodes(index)
                + spec_expression_nodes(value)
        }
        SpecExpr::For {
            initial,
            invariant,
            body,
            ..
        } => {
            spec_expression_nodes(initial)
                + spec_expression_nodes(invariant)
                + spec_expression_nodes(body)
        }
        SpecExpr::Record { fields, .. } => fields
            .iter()
            .map(|(_, value)| spec_expression_nodes(value))
            .sum(),
        SpecExpr::Field(value, _) => spec_expression_nodes(value),
        SpecExpr::Ident(_) | SpecExpr::Number(_) | SpecExpr::Bool(_) | SpecExpr::Text(_) => 0,
    }
}

struct TempDirectory(PathBuf);

impl TempDirectory {
    fn new(label: &str) -> Result<Self, String> {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|error| error.to_string())?
            .as_nanos();
        let path = env::temp_dir().join(format!("synthetix-v6-{label}-{nonce}"));
        fs::create_dir_all(&path).map_err(|error| error.to_string())?;
        Ok(Self(path))
    }
}

impl Drop for TempDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[derive(Default, Clone)]
struct ProjectRun {
    total_ns: u128,
    link_ns: u128,
    summary_ns: u128,
    cli_build_ns: u128,
    candidate_count: usize,
    solver_calls: usize,
    solver_ns: u128,
    wasm_bytes: usize,
    contract_count: usize,
    summary_count: usize,
    expression_nodes: usize,
    source_bytes: usize,
    failure: Option<String>,
}

fn run_verified_feature_case(
    name: &str,
    relative: &str,
    root: &Path,
    wasm_inputs: &[Vec<u32>],
) -> Result<Measurement, String> {
    let source = fs::read_to_string(root.join(relative)).map_err(|error| error.to_string())?;
    let contract = parse(&source).map_err(|error| format!("{relative}: {error}"))?;
    let candidate = axiom_verify::implementation_to_ami(&contract)?;
    let mut record = run_verification_case(name, relative, &contract, &candidate, None);
    if record.result != "VERIFIED" {
        return Err(format!(
            "{relative} feature proof did not verify: {record:?}"
        ));
    }
    let (_, abi_arity) = axiom_parser::verified_abi_layout(&contract.params)?;
    let wasm_arity = u32::try_from(abi_arity).map_err(|_| "WASM argument count overflow")?;
    let wasm = axiom_wasm::emit(&candidate, wasm_arity)?;
    for inputs in wasm_inputs {
        if inputs.len() != abi_arity {
            return Err(format!(
                "{relative} benchmark inputs do not match flattened ABI"
            ));
        }
        let reference = axiom_verify::reference::eval_ami(&candidate, inputs)?;
        let actual = axiom_wasm::run_with_args(&wasm, inputs)?;
        if reference != actual {
            return Err(format!(
                "{relative} reference/WASM mismatch: inputs={inputs:?}, reference={reference}, wasm={actual}"
            ));
        }
    }
    record.category = "verified_feature_proof".to_string();
    record.source_bytes = Some(source.len());
    record.artifact_bytes = Some(wasm.len());
    record.wasm_bytes = Some(wasm.len());
    record.details = Some(format!(
        "{}; fixed implementation candidate; WASM ABI arity={abi_arity}; reference/WASM inputs={}",
        record.details.as_deref().unwrap_or_default(),
        wasm_inputs.len()
    ));
    Ok(record)
}

fn run_compositional_case(depth: usize, _root: &Path) -> Result<Measurement, String> {
    let temp = TempDirectory::new(&format!("call-depth-{depth}"))?;
    write_compositional_project(&temp.0, depth)?;
    let config = temp.0.join("synthetix.toml");
    let mut runs = Vec::new();
    let mut errors = Vec::new();
    for _ in 0..REPEATS {
        match measure_project(&config) {
            Ok(run) => runs.push(run),
            Err(error) => errors.push(error),
        }
    }
    let mut record = blank_measurement(
        &format!("compositional_call_depth_{depth}"),
        "project_compositional",
        &format!("generated single-module summary chain depth={depth}; identity contracts"),
        SearchBounds {
            timeout_ms: None,
            max_candidates: Some(DEFAULT_MAX_CANDIDATES),
            max_memory_bytes: Some(DEFAULT_MAX_MEMORY_BYTES),
        },
    );
    if let Some(first) = runs.first() {
        record.result = first
            .failure
            .as_ref()
            .map_or_else(|| "VERIFIED".to_string(), |_| "FAILED".to_string());
        record.failure_classification = first.failure.as_ref().map(|failure| {
            if failure.contains("unresolved contract call") {
                "compositional_nested_summary_call_limit".to_string()
            } else {
                "project_summary_or_artifact_failure".to_string()
            }
        });
        record.function_call_depth = Some(depth);
        record.contract_count = Some(first.contract_count);
        record.summary_count = Some(first.summary_count);
        record.expression_node_count = Some(first.expression_nodes);
        record.source_bytes = Some(first.source_bytes);
        record.candidate_count = Some(first.candidate_count);
        record.solver_invocation_count = Some(median_usize(
            &runs.iter().map(|run| run.solver_calls).collect::<Vec<_>>(),
        ));
        record.z3_verification_time_ms = Some(
            median_u128(&runs.iter().map(|run| run.solver_ns).collect::<Vec<_>>()) as f64 / 1e6,
        );
        record.project_link_time_ms =
            Some(median_u128(&runs.iter().map(|run| run.link_ns).collect::<Vec<_>>()) as f64 / 1e6);
        record.summary_verification_time_ms = Some(
            median_u128(&runs.iter().map(|run| run.summary_ns).collect::<Vec<_>>()) as f64 / 1e6,
        );
        record.build_time_ms = Some(
            median_u128(&runs.iter().map(|run| run.cli_build_ns).collect::<Vec<_>>()) as f64 / 1e6,
        );
        record.total_time_ms = Some(
            median_u128(&runs.iter().map(|run| run.total_ns).collect::<Vec<_>>()) as f64 / 1e6,
        );
        record.artifact_bytes = Some(first.wasm_bytes);
        record.details = Some(format!(
            "project load -> dependency-first verified-summary API with fixed identity candidates -> release axiom build in a fresh output directory; direct candidate search excluded, production build includes synthesis; contract_count={}; summary_count={}; failure={}",
            first.contract_count,
            first.summary_count,
            first.failure.as_deref().unwrap_or("none")
        ));
    } else {
        record.result = "FAILED".to_string();
        record.failure_classification =
            Some("project_link_or_summary_verification_failure".to_string());
        record.details = Some(errors.join("; "));
    }
    Ok(record)
}

fn write_compositional_project(root: &Path, depth: usize) -> Result<(), String> {
    let mut source = String::new();
    for index in 0..depth {
        if index == 0 {
            source.push_str("contract level0(value: u32) -> u32 { ensures result == value; }\n");
        } else {
            source.push_str(&format!(
                "contract level{index}(value: u32) -> u32 {{ ensures result == level{}(value); }}\n",
                index - 1
            ));
        }
    }
    let last = depth - 1;
    source.push_str(&format!(
        "contract root(value: u32) -> u32 {{ ensures result == level{last}(value); }}\n"
    ));
    fs::write(root.join("entry.ax"), source).map_err(|error| error.to_string())?;
    fs::write(
        root.join("synthetix.toml"),
        "[project]\nentry = \"entry.ax\"\ntarget = \"web\"\n",
    )
    .map_err(|error| error.to_string())
}

fn measure_project(config: &Path) -> Result<ProjectRun, String> {
    let started = Instant::now();
    let link_started = Instant::now();
    let project = axiom_project::load(config)?;
    let link_ns = link_started.elapsed().as_nanos();
    let mut run = ProjectRun {
        link_ns,
        contract_count: project.contracts.len(),
        summary_count: project.verification_order.len(),
        source_bytes: project
            .files
            .iter()
            .filter_map(|path| {
                fs::metadata(path)
                    .ok()
                    .map(|metadata| metadata.len() as usize)
            })
            .sum(),
        ..ProjectRun::default()
    };
    let mut verified = std::collections::HashMap::new();
    for id in &project.verification_order {
        let Some(linked) = project
            .contracts
            .iter()
            .find(|linked| ContractId::from(*linked) == *id)
        else {
            run.failure = Some(format!("missing linked summary {}", id.name));
            break;
        };
        if linked.contract.kind == DeclarationKind::Extern {
            continue;
        }
        let candidate = Expr::Arg(0);
        run.candidate_count += 1;
        run.expression_nodes += contract_expression_nodes(&linked.contract);
        let summary_started = Instant::now();
        let proof = project.verify_compositional_with_verified_summaries_and_metrics(
            linked, &candidate, &verified,
        );
        run.summary_ns += summary_started.elapsed().as_nanos();
        match proof {
            Ok((proof, metrics)) => {
                run.solver_calls += metrics.solver_invocations;
                run.solver_ns += metrics.solver_time_ns;
                match (proof.proof, proof.diagnostic) {
                    (
                        Some(ProofResult::Verified),
                        None | Some(axiom_verify::Diagnostic::AmbiguousCandidates),
                    ) => {
                        verified.insert(id.clone(), linked.contract.clone());
                    }
                    (proof, diagnostic) => {
                        run.failure = Some(format!(
                            "{}: compositional result {proof:?}, diagnostic {diagnostic:?}",
                            linked.contract.name
                        ));
                        break;
                    }
                }
            }
            Err(error) => {
                run.failure = Some(format!("{}: {error}", linked.contract.name));
                break;
            }
        }
    }
    if run.failure.is_none() {
        let cli = release_cli_path()?;
        let project_dir = config.parent().ok_or_else(|| {
            format!(
                "cannot determine project directory for {}",
                config.display()
            )
        })?;
        let output_dir = project_dir.join("dist");
        if output_dir.exists() {
            fs::remove_dir_all(&output_dir).map_err(|error| {
                format!(
                    "cannot clear benchmark output {}: {error}",
                    output_dir.display()
                )
            })?;
        }
        let build_started = Instant::now();
        let build = Command::new(&cli)
            .arg("build")
            .current_dir(project_dir)
            .output()
            .map_err(|error| format!("cannot run {}: {error}", cli.display()))?;
        run.cli_build_ns = build_started.elapsed().as_nanos();
        if !build.status.success() {
            run.failure = Some(format!(
                "release CLI build failed: {}{}",
                String::from_utf8_lossy(&build.stdout),
                String::from_utf8_lossy(&build.stderr)
            ));
        } else {
            let wasm = output_dir.join("app.wasm");
            match fs::read(&wasm) {
                Ok(bytes) if bytes.starts_with(b"\0asm") => run.wasm_bytes = bytes.len(),
                Ok(_) => run.failure = Some("release CLI emitted invalid app.wasm".to_string()),
                Err(error) => run.failure = Some(format!("release CLI omitted app.wasm: {error}")),
            }
        }
    }
    run.total_ns = started.elapsed().as_nanos();
    Ok(run)
}

fn run_call_precondition_diagnostic_case() -> Result<Measurement, String> {
    let temp = TempDirectory::new("call-precondition")?;
    fs::write(
        temp.0.join("positive.ax"),
        "export contract positive(value: u32) -> u32 { requires value > 0; ensures result == value; }\n",
    )
    .map_err(|error| error.to_string())?;
    fs::write(
        temp.0.join("entry.ax"),
        "import { positive } from \"./positive.ax\";\ncontract total(value: u32) -> u32 {\n ensures result == positive(value);\n}\n",
    )
    .map_err(|error| error.to_string())?;
    let config = temp.0.join("synthetix.toml");
    fs::write(
        &config,
        "[project]\nentry = \"entry.ax\"\ntarget = \"web\"\n",
    )
    .map_err(|error| error.to_string())?;
    let project = axiom_project::load(&config)?;
    let linked = project
        .contracts
        .iter()
        .find(|linked| linked.contract.name == "total")
        .ok_or_else(|| "generated caller contract missing".to_string())?;
    let (proof, metrics) = project.verify_compositional_with_metrics(linked, &Expr::Arg(0))?;
    let diagnostic = proof
        .diagnostic
        .clone()
        .ok_or_else(|| format!("expected call-site diagnostic, received {proof:?}"))?;
    let (callee, span) = match diagnostic {
        axiom_verify::Diagnostic::CallPreconditionViolation { callee, span } => (callee, span),
        other => {
            return Err(format!(
                "expected call precondition violation, got {other:?}"
            ));
        }
    };
    let mut record = blank_measurement(
        "call_site_precondition_source_diagnostic",
        "compositional_diagnostic",
        "generated imported positive(value) call without caller requires",
        SearchBounds {
            timeout_ms: None,
            max_candidates: None,
            max_memory_bytes: None,
        },
    );
    record.result = "EXPECTED_CALL_PRECONDITION_DIAGNOSTIC".to_string();
    record.contract_count = Some(2);
    record.function_call_depth = Some(1);
    record.expression_node_count = Some(contract_expression_nodes(&linked.contract));
    record.parameter_count = Some(1);
    record.solver_invocation_count = Some(metrics.solver_invocations);
    record.precondition_check_invocation_count = Some(metrics.precondition_check_invocations);
    record.precondition_translation_time_ms =
        Some(metrics.precondition_translation_time_ns as f64 / 1e6);
    record.z3_verification_time_ms = Some(metrics.solver_time_ns as f64 / 1e6);
    record.details = Some(format!(
        "callee={callee}; source_location=entry.ax:{}:{}; proof_result=none",
        span.map_or(1, |value| value.line),
        span.map_or(1, |value| value.column)
    ));
    Ok(record)
}

fn run_circular_call_case() -> Result<Measurement, String> {
    let temp = TempDirectory::new("circular-call")?;
    fs::write(
        temp.0.join("a.ax"),
        "import { second } from \"./b.ax\";\nexport contract first(value: u32) -> u32 { ensures result == second(value); }\n",
    )
    .map_err(|error| error.to_string())?;
    fs::write(
        temp.0.join("b.ax"),
        "import { first } from \"./a.ax\";\nexport contract second(value: u32) -> u32 { ensures result == first(value); }\n",
    )
    .map_err(|error| error.to_string())?;
    let config = temp.0.join("synthetix.toml");
    fs::write(&config, "[project]\nentry = \"a.ax\"\ntarget = \"web\"\n")
        .map_err(|error| error.to_string())?;
    let started = Instant::now();
    let result = axiom_project::load(&config);
    let mut record = blank_measurement(
        "circular_call_rejection",
        "compositional_diagnostic",
        "generated two-module first <-> second import/call cycle",
        SearchBounds {
            timeout_ms: None,
            max_candidates: None,
            max_memory_bytes: None,
        },
    );
    record.total_time_ms = Some(started.elapsed().as_secs_f64() * 1000.0);
    record.function_call_depth = Some(2);
    match result {
        Err(error) if error.contains("circular") => {
            record.result = "EXPECTED_CIRCULAR_CALL_REJECTION".to_string();
            record.details = Some(format!(
                "expected project cycle rejection; error_kind={}",
                error.split(':').next().unwrap_or("circular error")
            ));
        }
        Err(error) => {
            record.result = "UNEXPECTED_PROJECT_ERROR".to_string();
            record.failure_classification = Some("unexpected_project_link_error".to_string());
            record.details = Some(error);
        }
        Ok(_) => {
            record.result = "CYCLE_ACCEPTED".to_string();
            record.failure_classification = Some("circular_call_was_not_rejected".to_string());
        }
    }
    Ok(record)
}

fn run_application_value_cases() -> Vec<Measurement> {
    [0usize, 10, 100, 1_000, 10_000]
        .into_iter()
        .map(|size| {
            let list = ApplicationValue::List(
                (0..size)
                    .map(|value| ApplicationValue::Integer(value as i128))
                    .collect(),
            );
            let record = ApplicationValue::Record(
                (0..size)
                    .map(|index| {
                        (
                            format!("field_{index:05}"),
                            ApplicationValue::Integer(index as i128),
                        )
                    })
                    .collect::<BTreeMap<_, _>>(),
            );
            let text = ApplicationValue::Text("t".repeat(size));
            let values = ApplicationValue::Record(
                [
                    ("list".to_string(), list),
                    ("record".to_string(), record),
                    ("text".to_string(), text),
                ]
                .into_iter()
                .collect(),
            );
            let mut serialization_times = Vec::new();
            let mut extraction_times = Vec::new();
            let mut byte_len = 0;
            let scalar_extractions = size;
            for _ in 0..REPEATS {
                let serialization_started = Instant::now();
                let json = values.to_json().expect("ApplicationValue serializes");
                let restored = ApplicationValue::from_json(&json).expect("serialized data parses");
                assert_eq!(restored, values);
                serialization_times.push(serialization_started.elapsed().as_nanos());
                byte_len = json.len();

                let extraction_started = Instant::now();
                if let ApplicationValue::Record(fields) = &values {
                    let ApplicationValue::List(items) = &fields["list"] else {
                        unreachable!("benchmark list field")
                    };
                    for item in items.iter().take(scalar_extractions) {
                        let scalar = item.verified_scalar().expect("integer is scalar");
                        assert!(matches!(scalar, ApplicationValue::Integer(_)));
                    }
                }
                extraction_times.push(extraction_started.elapsed().as_nanos());
            }
            let mut record = blank_measurement(
                &format!("application_value_boundary_size_{size}"),
                "application_data_serialization",
                "ApplicationValue tagged JSON list + record + text, measured without WASM lowering",
                SearchBounds {
                    timeout_ms: None,
                    max_candidates: None,
                    max_memory_bytes: None,
                },
            );
            record.list_size = Some(size);
            record.record_field_count = Some(size);
            record.text_payload_bytes = Some(size);
            record.scalar_extraction_count = Some(scalar_extractions);
            record.serialized_bytes = Some(byte_len);
            record.serialization_time_ms = Some(median_duration_ns(&serialization_times) as f64 / 1e6);
            record.scalar_extraction_time_ms = Some(median_duration_ns(&extraction_times) as f64 / 1e6);
            record.total_time_ms = Some(
                record.serialization_time_ms.unwrap_or_default()
                    + record.scalar_extraction_time_ms.unwrap_or_default(),
            );
            record.result = "APPLICATION_DATA_ONLY".to_string();
            record.details = Some(
                "record/list/text remain application values; this row measures JSON and explicit scalar extraction only, not verified collection semantics".to_string(),
            );
            record
        })
        .collect()
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RuntimeResult {
    transitions: usize,
    list_size: usize,
    text_bytes: usize,
    boot_ms: f64,
    transition_ms: f64,
    transition_ms_per_action: f64,
    restore_ms: f64,
    storage_bytes: usize,
    rendered_list_items: usize,
}

fn run_static_runtime_cases(root: &Path) -> Result<Vec<Measurement>, String> {
    let cli = release_cli_path()?;
    if !cli.is_file() {
        return Err(format!(
            "release CLI missing at {}; run cargo build --release -p axiom-cli first",
            cli.display()
        ));
    }
    let list_cases = [(0usize, 0usize), (10, 0), (100, 0), (1_000, 0)];
    let text_cases = [(10usize, 0usize), (10, 1_024), (10, 16_384), (10, 65_536)];
    let mut cases = list_cases.to_vec();
    cases.extend(text_cases);
    let harness = root.join("benchmarks/v6/runtime-harness.cjs");
    let mut records = Vec::new();
    for (list_size, text_bytes) in cases {
        let temp = TempDirectory::new(&format!("static-data-{list_size}-{text_bytes}"))?;
        let template = root.join("benchmarks/v6/application-template");
        for file in ["synthetix.toml", "logic.ax"] {
            fs::copy(template.join(file), temp.0.join(file)).map_err(|error| error.to_string())?;
        }
        let mut main =
            fs::read_to_string(template.join("main.ax")).map_err(|error| error.to_string())?;
        let list = if list_size == 0 {
            "[]".to_string()
        } else {
            format!(
                "[{}]",
                (0..list_size)
                    .map(|value| value.to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        };
        main = main.replace("__LIST__", &list);
        main = main.replace("__TEXT__", &"t".repeat(text_bytes));
        fs::write(temp.0.join("main.ax"), &main).map_err(|error| error.to_string())?;

        let build_started = Instant::now();
        let build = Command::new(&cli)
            .arg("build")
            .current_dir(&temp.0)
            .output()
            .map_err(|error| format!("cannot run {}: {error}", cli.display()))?;
        let build_ms = build_started.elapsed().as_secs_f64() * 1000.0;
        if !build.status.success() {
            return Err(format!(
                "release CLI static build failed: {}",
                String::from_utf8_lossy(&build.stderr)
            ));
        }
        let dist = temp.0.join("dist");
        let transitions = (5_000usize / list_size.max(1)).clamp(2, 50);
        let runtime = Command::new("node")
            .arg(&harness)
            .arg(&dist)
            .arg(transitions.to_string())
            .output()
            .map_err(|error| format!("cannot run Node runtime harness: {error}"))?;
        if !runtime.status.success() {
            return Err(format!(
                "generated static runtime failed: {}",
                String::from_utf8_lossy(&runtime.stderr)
            ));
        }
        let runtime: RuntimeResult = serde_json::from_slice(&runtime.stdout)
            .map_err(|error| format!("runtime harness emitted malformed JSON: {error}"))?;
        if runtime.list_size != list_size
            || runtime.text_bytes != text_bytes
            || runtime.rendered_list_items != list_size
        {
            return Err(format!("static runtime data size mismatch: {runtime:?}"));
        }
        let artifact_bytes = fs::read_dir(&dist)
            .map_err(|error| error.to_string())?
            .filter_map(Result::ok)
            .filter_map(|entry| entry.metadata().ok())
            .map(|metadata| metadata.len() as usize)
            .sum::<usize>();
        let mut record = blank_measurement(
            &format!("static_runtime_list_{list_size}_text_{text_bytes}"),
            "static_bundle_runtime_and_persistence",
            "benchmarks/v6/application-template built by release axiom CLI; generated app.js and app.wasm executed in Node DOM shim",
            SearchBounds {
                timeout_ms: None,
                max_candidates: Some(50_000),
                max_memory_bytes: Some(DEFAULT_MAX_MEMORY_BYTES),
            },
        );
        record.contract_count = Some(1);
        record.function_call_depth = Some(1);
        record.build_time_ms = Some(build_ms);
        record.runtime_boot_time_ms = Some(runtime.boot_ms);
        record.runtime_transition_time_ms = Some(runtime.transition_ms);
        record.runtime_transition_ms_per_action = Some(runtime.transition_ms_per_action);
        record.persistence_restore_time_ms = Some(runtime.restore_ms);
        record.total_time_ms =
            Some(build_ms + runtime.boot_ms + runtime.transition_ms + runtime.restore_ms);
        record.peak_resource_estimate_bytes = None;
        record.source_bytes = Some(main.len());
        record.artifact_bytes = Some(artifact_bytes);
        record.storage_bytes = Some(runtime.storage_bytes);
        record.list_size = Some(list_size);
        record.text_payload_bytes = Some(text_bytes);
        record.synthesis_iterations = None;
        record.result = "VERIFIED_WASM_RUNTIME_AND_PERSISTENCE".to_string();
        record.details = Some(format!(
            "release_build=true; transitions={}; rendered_items={}; persisted state restored; build/search uses CLI default limits; runtime_boot includes generated JS/WASM initialization",
            runtime.transitions, runtime.rendered_list_items
        ));
        records.push(record);
    }
    Ok(records)
}

fn run_v6_release_path_case(root: &Path) -> Result<Measurement, String> {
    let cli = release_cli_path()?;
    if !cli.is_file() {
        return Err(format!(
            "release CLI missing at {}; run cargo build --release -p axiom-cli first",
            cli.display()
        ));
    }
    let smoke = root.join("scripts/v6-release-smoke.cjs");
    let fixture = root.join("examples/v6-release");
    let files = [
        "synthetix.toml",
        "main.ax",
        "arrays.ax",
        "logic.ax",
        "recursive.ax",
        "api.ax",
    ];
    let source_bytes = files
        .iter()
        .map(|file| fs::metadata(fixture.join(file)).map(|metadata| metadata.len() as usize))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?
        .into_iter()
        .sum();
    let mut build_times = Vec::new();
    let mut boot_times = Vec::new();
    let mut transition_times = Vec::new();
    let mut effect_times = Vec::new();
    let mut restore_times = Vec::new();
    let mut total_times = Vec::new();
    let mut artifact_sizes = Vec::new();
    let mut wasm_sizes = Vec::new();
    let mut contract_counts = Vec::new();

    for _ in 0..REPEATS {
        let temp = TempDirectory::new("v6-release-path")?;
        for file in files {
            fs::copy(fixture.join(file), temp.0.join(file)).map_err(|error| error.to_string())?;
        }
        let check = Command::new(&cli)
            .arg("check")
            .current_dir(&temp.0)
            .output()
            .map_err(|error| error.to_string())?;
        if !check.status.success() {
            return Err(format!(
                "v6 release project check failed: {}{}",
                String::from_utf8_lossy(&check.stdout),
                String::from_utf8_lossy(&check.stderr)
            ));
        }
        let build_started = Instant::now();
        let build = Command::new(&cli)
            .arg("build")
            .current_dir(&temp.0)
            .output()
            .map_err(|error| error.to_string())?;
        let build_ms = build_started.elapsed().as_secs_f64() * 1000.0;
        if !build.status.success() {
            return Err(format!(
                "v6 release project build failed: {}{}",
                String::from_utf8_lossy(&build.stdout),
                String::from_utf8_lossy(&build.stderr)
            ));
        }
        let dist = temp.0.join("dist");
        let run = Command::new("node")
            .arg(&smoke)
            .arg(&dist)
            .output()
            .map_err(|error| format!("cannot run v6 release smoke: {error}"))?;
        if !run.status.success() {
            return Err(format!(
                "v6 release smoke failed: {}",
                String::from_utf8_lossy(&run.stderr)
            ));
        }
        let report: serde_json::Value = serde_json::from_slice(&run.stdout)
            .map_err(|error| format!("v6 release smoke returned invalid JSON: {error}"))?;
        if report["status"] != "PASS" {
            return Err(format!("v6 release smoke did not pass: {report}"));
        }
        let get_ms = |key: &str| {
            report[key]
                .as_f64()
                .ok_or_else(|| format!("v6 release smoke omitted {key}"))
        };
        let boot_ms = get_ms("runtime_boot_time_ms")?;
        let transition_ms = get_ms("transition_time_ms")?;
        let effect_ms = get_ms("effect_time_ms")?;
        let restore_ms = get_ms("persistence_restore_time_ms")?;
        build_times.push(build_ms);
        boot_times.push(boot_ms);
        transition_times.push(transition_ms);
        effect_times.push(effect_ms);
        restore_times.push(restore_ms);
        total_times.push(build_ms + boot_ms + transition_ms + effect_ms + restore_ms);
        wasm_sizes.push(
            report["wasm_bytes"]
                .as_u64()
                .and_then(|bytes| usize::try_from(bytes).ok())
                .ok_or_else(|| "v6 release smoke omitted wasm_bytes".to_string())?,
        );
        let artifact_bytes = fs::read_dir(&dist)
            .map_err(|error| error.to_string())?
            .map(|entry| {
                entry
                    .map_err(|error| error.to_string())?
                    .metadata()
                    .map(|metadata| metadata.len() as usize)
                    .map_err(|error| error.to_string())
            })
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .sum();
        artifact_sizes.push(artifact_bytes);
        let proof: serde_json::Value = serde_json::from_slice(
            &fs::read(dist.join("app.proof.json")).map_err(|error| error.to_string())?,
        )
        .map_err(|error| error.to_string())?;
        contract_counts.push(
            proof["contracts"]
                .as_array()
                .ok_or_else(|| "v6 proof artifact omitted contracts".to_string())?
                .len(),
        );
    }

    let bounds = SearchBounds {
        timeout_ms: None,
        max_candidates: Some(DEFAULT_MAX_CANDIDATES),
        max_memory_bytes: Some(DEFAULT_MAX_MEMORY_BYTES),
    };
    let mut record = blank_measurement(
        "v6_release_array_loop_recursion_application",
        "static_release_runtime_and_effect",
        "examples/v6-release built by release CLI; generated bundle served over local HTTP and executed in Node DOM shim",
        bounds,
    );
    record.contract_count = Some(median_usize(&contract_counts));
    record.function_call_depth = Some(16);
    record.build_time_ms = Some(median_f64(build_times));
    record.runtime_boot_time_ms = Some(median_f64(boot_times));
    record.runtime_transition_time_ms = Some(median_f64(transition_times));
    record.runtime_transition_ms_per_action = record.runtime_transition_time_ms;
    record.effect_time_ms = Some(median_f64(effect_times));
    record.persistence_restore_time_ms = Some(median_f64(restore_times));
    record.total_time_ms = Some(median_f64(total_times));
    record.source_bytes = Some(source_bytes);
    record.artifact_bytes = Some(median_usize(&artifact_sizes));
    record.wasm_bytes = Some(median_usize(&wasm_sizes));
    record.result = "VERIFIED_RELEASE_PATH".to_string();
    record.details = Some(format!(
        "repeats={REPEATS}; bounds=timeout:none,max_candidates:{DEFAULT_MAX_CANDIDATES},estimated_memory:{DEFAULT_MAX_MEMORY_BYTES}; fixed array length=4; loop bound=4; recursion max=16; external result type-checked only; static HTTP assets and persisted state passed"
    ));
    Ok(record)
}

fn release_cli_path() -> Result<PathBuf, String> {
    let executable = env::current_exe().map_err(|error| error.to_string())?;
    let release_dir = executable.parent().and_then(Path::parent).ok_or_else(|| {
        format!(
            "cannot locate release directory from {}",
            executable.display()
        )
    })?;
    Ok(release_dir.join(if cfg!(windows) { "axiom.exe" } else { "axiom" }))
}

fn run_array_boundary_case(root: &Path) -> Result<Measurement, String> {
    let relative = "benchmarks/v6/expressiveness/array_boundary.ax";
    let source = fs::read_to_string(root.join(relative)).map_err(|error| error.to_string())?;
    let mut parser = axiom_parser::Parser::new(axiom_parser::Lexer::new(&source))?;
    let contract = parser.parse_contract()?;
    let validation = axiom_parser::validate(&contract);
    let mut record = blank_measurement(
        "verified_array_parameter_boundary",
        "language_expressiveness",
        relative,
        SearchBounds {
            timeout_ms: None,
            max_candidates: None,
            max_memory_bytes: None,
        },
    );
    record.contract_count = Some(1);
    record.parameter_count = Some(contract.params.len());
    record.expression_node_count = Some(contract_expression_nodes(&contract));
    record.source_bytes = Some(source.len());
    record.result = if validation.is_err() {
        "EXPECTED_INVALID".to_string()
    } else {
        "UNEXPECTED_ACCEPTANCE".to_string()
    };
    if let Err(error) = validation {
        record.failure_classification = None;
        record.details = Some(format!(
            "parser/typecheck boundary rejects list parameter: {error}"
        ));
    } else {
        record.failure_classification =
            Some("application_list_crossed_verified_boundary".to_string());
    }
    Ok(record)
}

fn run_unrolled_loop_cases(root: &Path, bounds: SearchBounds) -> Result<Vec<Measurement>, String> {
    let mut measurements = Vec::new();
    for (name, path) in [
        (
            "unrolled_validation_4",
            "benchmarks/v6/expressiveness/validate_4.ax",
        ),
        (
            "unrolled_bounded_search_4",
            "benchmarks/v6/expressiveness/bounded_search_4.ax",
        ),
    ] {
        let outcome = run_synthesis_case(name, path, &root.join(path), options(bounds))?;
        measurements.push(outcome.measurement);
    }
    Ok(measurements)
}

fn run_incremental_cache_cases(root: &Path) -> Result<Vec<Measurement>, String> {
    let names = [
        "five_contract_cold",
        "five_contract_warm",
        "leaf_change",
        "root_change",
        "32_module_cold",
        "32_module_warm",
    ];
    let mut timings = names
        .into_iter()
        .map(|name| (name, Vec::new()))
        .collect::<BTreeMap<_, _>>();

    for _ in 0..REPEATS {
        let output = Command::new("cargo")
            .args([
                "test",
                "-p",
                "axiom-cli",
                "proof_cache_",
                "--",
                "--nocapture",
            ])
            .current_dir(root)
            .output()
            .map_err(|error| format!("cannot run incremental proof-cache tests: {error}"))?;
        let text = format!(
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        if !output.status.success() {
            return Err(format!("incremental proof-cache tests failed: {text}"));
        }
        let five = text
            .lines()
            .find(|line| line.contains("v6.5 cache: 5 contracts;"))
            .ok_or_else(|| "proof-cache tests omitted five-contract timing output".to_string())?;
        let large = text
            .lines()
            .find(|line| line.contains("v6.5 cache large graph:"))
            .ok_or_else(|| "proof-cache tests omitted large-graph timing output".to_string())?;
        for (name, line, key) in [
            ("five_contract_cold", five, "cold"),
            ("five_contract_warm", five, "warm"),
            ("leaf_change", five, "leaf edit"),
            ("root_change", five, "root edit"),
            ("32_module_cold", large, "cold"),
            ("32_module_warm", large, "warm"),
        ] {
            timings
                .get_mut(name)
                .expect("all cache scenarios are initialized")
                .push(cache_elapsed_ms(line, key)?);
        }
    }

    let scenarios = [
        ("five_contract_cold", 5, 0, 5, 0, "CACHE_COLD_VERIFIED"),
        ("five_contract_warm", 5, 5, 0, 0, "CACHE_WARM_HIT"),
        ("leaf_change", 5, 1, 4, 4, "DEPENDENTS_REVERIFIED"),
        ("root_change", 5, 4, 1, 1, "ROOT_REVERIFIED"),
        ("32_module_cold", 32, 0, 32, 0, "CACHE_COLD_VERIFIED"),
        ("32_module_warm", 32, 32, 0, 0, "CACHE_WARM_HIT"),
    ];
    let bounds = SearchBounds {
        timeout_ms: None,
        max_candidates: Some(DEFAULT_MAX_CANDIDATES),
        max_memory_bytes: Some(DEFAULT_MAX_MEMORY_BYTES),
    };
    Ok(scenarios
        .into_iter()
        .map(|(name, modules, hits, misses, invalidations, result)| {
            let mut record = blank_measurement(
                &format!("proof_cache_{name}"),
                "incremental_proof_cache",
                "cargo test -p axiom-cli proof_cache_ -- --nocapture",
                bounds,
            );
            record.contract_count = Some(modules);
            record.total_time_ms = Some(median_f64(
                timings.get(name).expect("all scenarios have timings").clone(),
            ));
            record.cache_hit_count = Some(hits);
            record.cache_miss_count = Some(misses);
            record.cache_invalidation_count = Some(invalidations);
            record.result = result.to_string();
            record.details = Some(format!(
                "debug cargo test profile; median of {REPEATS} test runs; elapsed includes project load and verification/cache lookup, excludes fixture creation and edits; hits={hits}, misses={misses}, invalidations={invalidations}"
            ));
            record
        })
        .collect())
}

fn cache_elapsed_ms(line: &str, key: &str) -> Result<f64, String> {
    let marker = format!("{key}=");
    let elapsed = line
        .split_once(&marker)
        .and_then(|(_, value)| value.split_once("us"))
        .ok_or_else(|| format!("cache timing omitted {key}: {line}"))?
        .0
        .parse::<f64>()
        .map_err(|error| format!("invalid cache timing for {key}: {error}"))?;
    Ok(elapsed / 1000.0)
}

fn command_output(program: &str, args: &[&str], cwd: &Path) -> Option<String> {
    let output = Command::new(program)
        .args(args)
        .current_dir(cwd)
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn write_outputs(
    baseline: &Baseline,
    json_path: &Path,
    markdown_path: &Path,
    title: &str,
) -> Result<(), String> {
    if let Some(parent) = json_path.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    if let Some(parent) = markdown_path.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let json = serde_json::to_vec_pretty(baseline).map_err(|error| error.to_string())?;
    fs::write(json_path, json).map_err(|error| error.to_string())?;
    fs::write(markdown_path, render_markdown(baseline, title)).map_err(|error| error.to_string())
}

fn render_markdown(baseline: &Baseline, title: &str) -> String {
    use std::fmt::Write as _;

    let mut output = String::new();
    let env = &baseline.environment;
    let _ = writeln!(
        output,
        "# {title}\n\nNon-guarantee performance measurements for this Synthetix v6 source and release path.\n\n## Environment\n\n- HEAD: `{}`\n- Working tree dirty: `{}`\n- OS/architecture: `{} / {}`\n- Rust: `{}`\n- Node: `{}`\n- Build mode: `{}`\n- Repeats: {} (median timings)\n- Default search bounds: timeout={} ms, candidates={}, memory={} bytes\n- Timing note: {}\n- Resource estimate: {}\n",
        env.commit_sha,
        env.working_tree_dirty,
        env.operating_system,
        env.architecture,
        env.rustc,
        env.node.as_deref().unwrap_or("not installed"),
        env.build_mode,
        env.measured_repeats,
        baseline
            .default_search_bounds
            .timeout_ms
            .map_or_else(|| "none".to_string(), |v| v.to_string()),
        baseline
            .default_search_bounds
            .max_candidates
            .map_or_else(|| "none".to_string(), |v| v.to_string()),
        baseline
            .default_search_bounds
            .max_memory_bytes
            .map_or_else(|| "none".to_string(), |v| v.to_string()),
        env.timing_note,
        env.resource_estimate_note,
    );
    output.push_str("## Results\n\n| Case | Category | Nodes / params / depth / data | Search bounds | Tested / rejected / generated | Synthesis non-Z3 ms | Link / summary ms | Z3 ms (calls) | Build / boot ms | Transition / effect / restore ms | Total ms | Artifact / WASM bytes | Cache hits / misses / invalidations | Result |\n| --- | --- | ---: | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | --- |\n");
    for record in &baseline.measurements {
        let size = format!(
            "{} / {} / {} / {}",
            number(record.expression_node_count),
            number(record.parameter_count),
            number(record.function_call_depth),
            record
                .list_size
                .map(|value| format!("list={value}"))
                .or_else(|| record
                    .text_payload_bytes
                    .map(|value| format!("text={value}B")))
                .unwrap_or_else(|| "—".to_string()),
        );
        let candidates = match record.candidate_count {
            Some(tested) => format!(
                "{tested} / {} / {}",
                number(record.rejected_candidate_count),
                number(record.generated_candidate_count),
            ),
            None => record
                .generated_candidate_count
                .map(|value| format!("— / — / {value}"))
                .unwrap_or_else(|| "—".to_string()),
        };
        let z3 = match (
            record.z3_verification_time_ms,
            record.solver_invocation_count,
        ) {
            (Some(time), Some(calls)) => format!("{time:.3} ({calls})"),
            (Some(time), None) => format!("{time:.3}"),
            _ => "—".to_string(),
        };
        let bounds = format!(
            "timeout={}, candidates={}, memory={}",
            record
                .search_bounds
                .timeout_ms
                .map_or_else(|| "none".to_string(), |value| value.to_string()),
            number(record.search_bounds.max_candidates),
            number(record.search_bounds.max_memory_bytes),
        );
        let link_summary = format!(
            "{} / {}",
            millis(record.project_link_time_ms),
            millis(record.summary_verification_time_ms),
        );
        let build_runtime = format!(
            "{} / {}",
            millis(record.build_time_ms),
            millis(record.runtime_boot_time_ms),
        );
        let transitions = format!(
            "{} / {} / {}",
            millis(record.runtime_transition_time_ms),
            millis(record.effect_time_ms),
            millis(record.persistence_restore_time_ms),
        );
        let artifacts = format!(
            "{} / {}",
            number(record.artifact_bytes),
            number(record.wasm_bytes),
        );
        let cache = if record.cache_hit_count.is_some()
            || record.cache_miss_count.is_some()
            || record.cache_invalidation_count.is_some()
        {
            format!(
                "{} / {} / {}",
                number(record.cache_hit_count),
                number(record.cache_miss_count),
                number(record.cache_invalidation_count),
            )
        } else {
            "—".to_string()
        };
        let _ = writeln!(
            output,
            "| `{}` | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} |",
            record.name,
            record.category,
            size,
            bounds,
            candidates,
            millis(record.synthesis_non_solver_time_ms),
            link_summary,
            z3,
            build_runtime,
            transitions,
            millis(record.total_time_ms),
            artifacts,
            cache,
            record.result,
        );
    }

    output.push_str("\n## Outcome coverage\n\n");
    for (result, count) in &baseline.outcome_counts {
        let _ = writeln!(output, "- `{result}`: {count}");
    }
    let _ = writeln!(
        output,
        "- Not observed in this bounded corpus: {}",
        if baseline.outcomes_not_observed.is_empty() {
            "none".to_string()
        } else {
            baseline.outcomes_not_observed.join(", ")
        }
    );

    output.push_str(
        "\nCandidate metrics: `candidate_count` is proof attempts; `generated_candidate_count` counts post-canonicalization candidates per batch; `rejected_candidate_count` counts type/sample rejects and counterexamples. An expression emitted in separate batches can be counted more than once.\n\n",
    );
    output.push_str(
        "\n## v6 Feature and Release Evidence\n\n| Case | Result | Scope |\n| --- | --- | --- |\n",
    );
    for record in &baseline.measurements {
        if record.category == "verified_feature_proof"
            || record.category == "static_release_runtime_and_effect"
        {
            let _ = writeln!(
                output,
                "| `{}` | {} | {} |",
                record.name,
                record.result,
                record.details.as_deref().unwrap_or("—"),
            );
        }
    }
    output.push_str("\n## Interpretation\n\n`NO_CANDIDATE` and `RESOURCE_LIMIT` apply only to each recorded grammar and search bound; they do not establish that a program is impossible. Solver and wall-clock timings are local observations, not performance guarantees. The release-path row measures generated static assets served over local HTTP in the Node DOM shim; it is not a real-browser timing claim.\n\n## Case details\n\n");
    for record in &baseline.measurements {
        if record.details.is_some() || record.failure_classification.is_some() {
            let _ = writeln!(
                output,
                "- `{}`: {}{}",
                record.name,
                record.details.as_deref().unwrap_or(""),
                record
                    .failure_classification
                    .as_deref()
                    .map(|classification| format!("; classification={classification}"))
                    .unwrap_or_default(),
            );
        }
    }
    output
}

fn synthesis_failure_class(
    result: &SynthesisResult,
    metrics: &axiom_synth::SynthesisMetrics,
) -> Option<String> {
    match result {
        SynthesisResult::Found { .. } => None,
        SynthesisResult::NoCandidate { .. } => {
            Some("bounded_grammar_exhausted_not_impossibility".to_string())
        }
        SynthesisResult::Timeout { .. } => Some("explicit_search_timeout".to_string()),
        SynthesisResult::ResourceLimit { .. }
            if metrics.search_batches == 0 && metrics.initial_sample_count == 0 =>
        {
            Some("sample_cartesian_product_or_zero_budget_limit".to_string())
        }
        SynthesisResult::ResourceLimit { .. }
            if metrics.generated_candidate_count > 0 && metrics.synthesis_iterations == 0 =>
        {
            Some("candidate_generation_bound_reached_before_verification".to_string())
        }
        SynthesisResult::ResourceLimit { .. } => Some("explicit_search_resource_limit".to_string()),
    }
}

fn classify_error(error: &str) -> String {
    if error.contains("UNSATISFIABLE_PRECONDITION") {
        "unsatisfiable_precondition".to_string()
    } else if error.contains("UNSATISFIABLE_POSTCONDITION") {
        "unsatisfiable_postcondition".to_string()
    } else if error.contains("SOLVER_UNKNOWN") {
        "solver_unknown".to_string()
    } else if error.contains("RESOURCE_LIMIT") {
        "explicit_search_resource_limit".to_string()
    } else if error.contains("TIMEOUT") {
        "explicit_search_timeout".to_string()
    } else {
        "input_or_tool_error".to_string()
    }
}

fn number(value: Option<usize>) -> String {
    value.map_or_else(|| "—".to_string(), |value| value.to_string())
}

fn millis(value: Option<f64>) -> String {
    value.map_or_else(|| "—".to_string(), |value| format!("{value:.3}"))
}

fn median_duration_ns(values: &[u128]) -> u128 {
    median_u128(values)
}

fn median_u128(values: &[u128]) -> u128 {
    let mut sorted = values.to_vec();
    sorted.sort_unstable();
    sorted.get(sorted.len() / 2).copied().unwrap_or_default()
}

fn median_usize(values: &[usize]) -> usize {
    let mut sorted = values.to_vec();
    sorted.sort_unstable();
    sorted.get(sorted.len() / 2).copied().unwrap_or_default()
}

fn median_f64(mut values: Vec<f64>) -> f64 {
    values.sort_by(f64::total_cmp);
    values.get(values.len() / 2).copied().unwrap_or_default()
}
