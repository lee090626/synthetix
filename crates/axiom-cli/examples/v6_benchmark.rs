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
    synthesis_iterations: Option<usize>,
    search_batch_count: Option<usize>,
    counterexample_count: Option<usize>,
    initial_sample_count: Option<usize>,
    peak_sample_count: Option<usize>,
    synthesis_time_ms: Option<f64>,
    synthesis_non_solver_time_ms: Option<f64>,
    candidate_generation_time_ms: Option<f64>,
    sample_generation_time_ms: Option<f64>,
    z3_verification_time_ms: Option<f64>,
    solver_invocation_count: Option<usize>,
    project_link_time_ms: Option<f64>,
    summary_verification_time_ms: Option<f64>,
    artifact_generation_time_ms: Option<f64>,
    build_time_ms: Option<f64>,
    runtime_boot_time_ms: Option<f64>,
    runtime_transition_time_ms: Option<f64>,
    runtime_transition_ms_per_action: Option<f64>,
    persistence_restore_time_ms: Option<f64>,
    serialization_time_ms: Option<f64>,
    scalar_extraction_time_ms: Option<f64>,
    total_time_ms: Option<f64>,
    peak_resource_estimate_bytes: Option<usize>,
    source_bytes: Option<usize>,
    artifact_bytes: Option<usize>,
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
    let (json_path, markdown_path) = output_paths()?;
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
    measurements.push(run_call_precondition_diagnostic_case()?);
    measurements.push(run_circular_call_case()?);
    measurements.push(run_structural_recursion_case(&root)?);

    measurements.extend(run_application_value_cases());
    measurements.extend(run_static_runtime_cases(&root)?);
    measurements.push(run_array_boundary_case(&root)?);
    measurements.extend(run_unrolled_loop_cases(&root, default_bounds)?);

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
        timing_note: "Wall-clock and solver durations are host-dependent. Synthesis, verifier timings, application JSON costs, and compositional release CLI builds are medians of three runs; generated static bundle/runtime rows are single runs, so their first row may include cold-start overhead. Generated timestamp is intentionally variable; bounds and counts are recorded per case.",
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
    write_outputs(&baseline, &json_path, &markdown_path)?;
    println!("wrote {}", json_path.display());
    println!("wrote {}", markdown_path.display());
    println!("measurements: {}", baseline.measurements.len());
    Ok(())
}

fn output_paths() -> Result<(PathBuf, PathBuf), String> {
    let mut json = PathBuf::from("docs/v6-benchmark-baseline.json");
    let mut markdown = PathBuf::from("docs/v6-benchmark.md");
    let args = env::args().skip(1).collect::<Vec<_>>();
    let mut index = 0;
    while index < args.len() {
        let value = args
            .get(index + 1)
            .ok_or_else(|| format!("missing output path after {}", args[index]))?;
        match args[index].as_str() {
            "--json" => json = PathBuf::from(value),
            "--markdown" => markdown = PathBuf::from(value),
            flag => return Err(format!("unknown benchmark option: {flag}")),
        }
        index += 2;
    }
    Ok((json, markdown))
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
                "{}|{}|{}|{}|{}|{}|{}",
                result.status_name(),
                result.candidates_checked(),
                metrics.generated_candidate_count,
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
            record.synthesis_iterations = Some(metrics.synthesis_iterations);
            record.search_batch_count = Some(metrics.search_batches);
            record.counterexample_count = Some(metrics.counterexample_count);
            record.initial_sample_count = Some(metrics.initial_sample_count);
            record.peak_sample_count = Some(metrics.peak_sample_count);
            record.solver_invocation_count = Some(metrics.solver_invocations);
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
        synthesis_iterations: None,
        search_batch_count: None,
        counterexample_count: None,
        initial_sample_count: None,
        peak_sample_count: None,
        synthesis_time_ms: None,
        synthesis_non_solver_time_ms: None,
        candidate_generation_time_ms: None,
        sample_generation_time_ms: None,
        z3_verification_time_ms: None,
        solver_invocation_count: None,
        project_link_time_ms: None,
        summary_verification_time_ms: None,
        artifact_generation_time_ms: None,
        build_time_ms: None,
        runtime_boot_time_ms: None,
        runtime_transition_time_ms: None,
        runtime_transition_ms_per_action: None,
        persistence_restore_time_ms: None,
        serialization_time_ms: None,
        scalar_extraction_time_ms: None,
        total_time_ms: None,
        peak_resource_estimate_bytes: None,
        source_bytes: None,
        artifact_bytes: None,
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

fn run_structural_recursion_case(root: &Path) -> Result<Measurement, String> {
    let relative = "benchmarks/v6/expressiveness/recursive_sum.ax";
    let temp = TempDirectory::new("structural-recursion")?;
    let source = fs::read_to_string(root.join(relative)).map_err(|error| error.to_string())?;
    let mut parser = axiom_parser::Parser::new(axiom_parser::Lexer::new(&source))?;
    let contract = parser.parse_contract()?;
    fs::write(temp.0.join("entry.ax"), &source).map_err(|error| error.to_string())?;
    let config = temp.0.join("synthetix.toml");
    fs::write(
        &config,
        "[project]\nentry = \"entry.ax\"\ntarget = \"web\"\n",
    )
    .map_err(|error| error.to_string())?;
    let started = Instant::now();
    let result = axiom_project::load(&config);
    let mut record = blank_measurement(
        "structural_recursion_boundary",
        "language_expressiveness",
        relative,
        SearchBounds {
            timeout_ms: None,
            max_candidates: None,
            max_memory_bytes: None,
        },
    );
    record.contract_count = Some(1);
    record.function_call_depth = Some(1);
    record.expression_node_count = Some(contract_expression_nodes(&contract));
    record.parameter_count = Some(contract.params.len());
    record.source_bytes = Some(source.len());
    record.total_time_ms = Some(started.elapsed().as_secs_f64() * 1000.0);
    match result {
        Err(error) if error.contains("circular contract call") => {
            record.result = "EXPECTED_RECURSION_REJECTION".to_string();
            record.failure_classification = Some("recursion_not_supported".to_string());
            record.details = Some(
                "structurally decreasing sum_to(n -% 1) parses, but project linking rejects all recursive contract cycles before termination analysis".to_string(),
            );
        }
        Err(error) => {
            record.result = "UNEXPECTED_RECURSION_ERROR".to_string();
            record.failure_classification = Some("unexpected_recursion_boundary_error".to_string());
            record.details = Some(error);
        }
        Ok(_) => {
            record.result = "RECURSION_ACCEPTED_WITHOUT_TERMINATION_PROOF".to_string();
            record.failure_classification =
                Some("recursive_call_accepted_without_termination_proof".to_string());
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
) -> Result<(), String> {
    if let Some(parent) = json_path.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    if let Some(parent) = markdown_path.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let json = serde_json::to_vec_pretty(baseline).map_err(|error| error.to_string())?;
    fs::write(json_path, json).map_err(|error| error.to_string())?;
    fs::write(markdown_path, render_markdown(baseline)).map_err(|error| error.to_string())
}

fn render_markdown(baseline: &Baseline) -> String {
    use std::fmt::Write as _;

    let mut output = String::new();
    let env = &baseline.environment;
    let _ = writeln!(
        output,
        "# v6.0 Scale Benchmark Baseline\n\nNon-guarantee measurements for the current scalar core and application boundary. Generated by `scripts/run-v6-benchmarks.sh`.\n\n## Environment\n\n- HEAD: `{}`\n- Working tree dirty: `{}` (benchmark harness/instrumentation are included in this run)\n- OS/architecture: `{} / {}`\n- Rust: `{}`\n- Node: `{}`\n- Build mode: `{}`\n- Repeats: {} (median timings)\n- Default search bounds: timeout={} ms, candidates={}, memory={} bytes\n- Timing note: {}\n- Resource estimate: {}\n",
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
    output.push_str(
        "## Results\n\n| Case | Category | Size (nodes / params / depth / data) | Candidates | Synthesis non-Z3 ms | Z3 ms (checks) | Total ms | Result |\n| --- | --- | ---: | ---: | ---: | ---: | ---: | --- |\n",
    );
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
        let candidates = record
            .candidate_count
            .map(|value| value.to_string())
            .or_else(|| {
                record
                    .generated_candidate_count
                    .map(|value| format!("{value} gen"))
            })
            .unwrap_or_else(|| "—".to_string());
        let z3 = match (
            record.z3_verification_time_ms,
            record.solver_invocation_count,
        ) {
            (Some(time), Some(calls)) => format!("{time:.3} ({calls})"),
            (Some(time), None) => format!("{time:.3}"),
            _ => "—".to_string(),
        };
        let _ = writeln!(
            output,
            "| `{}` | {} | {} | {} | {} | {} | {} | {} |",
            record.name,
            record.category,
            size,
            candidates,
            millis(record.synthesis_non_solver_time_ms),
            z3,
            millis(record.total_time_ms),
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
        "\n## Expressiveness evidence\n\n| Use case | Measured result | Boundary shown |\n| --- | --- | --- |\n",
    );
    let outcome = |name: &str| {
        baseline
            .measurements
            .iter()
            .find(|record| record.name == name)
            .map(|record| record.result.as_str())
            .unwrap_or("MISSING")
    };
    let _ = writeln!(
        output,
        "| N-value sum | `sum_3={}`; `sum_8={}` | Three scalar inputs synthesize; eight inputs exceed the sample Cartesian-product limit before allocation. |",
        outcome("sum_3"),
        outcome("sum_8")
    );
    let _ = writeln!(
        output,
        "| max/min | `property_max={}`, `minimum={}`, `maximum_3={}` | Two-input max/min synthesize; three-input max reaches the candidate-generation bound. |",
        outcome("property_max"),
        outcome("minimum"),
        outcome("maximum_3")
    );
    let _ = writeln!(
        output,
        "| Repeated validation | `unrolled_validation_4={}` | Four checks are expressible as a 17-node scalar predicate and synthesize without a loop. |",
        outcome("unrolled_validation_4")
    );
    let _ = writeln!(
        output,
        "| Bounded search | `unrolled_bounded_search_4={}` | Five scalar parameters exceed the default sample limit; a loop bound/invariant would avoid hand-unrolling but needs an explicit memory model. |",
        outcome("unrolled_bounded_search_4")
    );
    let _ = writeln!(
        output,
        "| List transform / aggregation | `verified_array_parameter_boundary={}` | The representative `sum_list` signature is rejected at the list parameter before verified body synthesis. Application list display/add is measured separately and does not establish verified transform semantics. |",
        outcome("verified_array_parameter_boundary")
    );
    let _ = writeln!(
        output,
        "| Structural recursion | `structural_recursion_boundary={}` | A decreasing `sum_to` call parses, then the project linker rejects the recursive cycle before termination proof. |",
        outcome("structural_recursion_boundary")
    );
    output.push_str(
        "\nIndexed array reads/writes, fixed-array bounds checks, and element-wise collection validation are not separate executable cases: the current proof boundary rejects list parameters before those operations can be expressed. That is a type-boundary observation, not evidence that every such operation would be costly after arrays exist.\n",
    );

    output.push_str("\n## Bottleneck and feature assessment\n\n");
    output.push_str(
        "- **Synthesis:** `candidate_count` is the number of candidates sent through verification, including the source-shaped seed; generated candidates and search-batch iterations are separate. `NO_CANDIDATE` and `RESOURCE_LIMIT` describe only the configured grammar/budget, never proof of implementation impossibility. A generation-bound row may stop before any candidate reaches Z3. `synthesis_non_solver_time_ms` subtracts instrumented solver time from synthesis wall time.\n- **Z3:** direct verifier rows report exact `Solver::check()` invocation count and summed time. Search rows include contract satisfiability, CEGIS precondition checks, and candidate proof calls.\n- **Compositional depth:** each generated chain is verified dependency-first with a fixed identity candidate through the verified-summary API at every contract, then built by the release CLI in a fresh temporary `dist` directory. Rows record contract count, summary count, solver time, and end-to-end CLI build time. Direct API timing isolates summary verification; the CLI build also performs production synthesis and artifact emission. Depths 1/2/4/8 are measured after the nested-summary fix. Circular-call rejection and a source-located failed call precondition are separate cases.\n- **Data boundary:** `application_data_serialization` measures tagged JSON plus explicit scalar extraction while scaling record fields, list items, and text together. `static_bundle_runtime_and_persistence` builds with the release CLI and executes generated JS/WASM in Node with a small DOM/storage shim; transition cost includes verified WASM calls, persistence serialization, and list rendering. It is not a real-browser timing claim.\n- **Loop cases:** sum3/sum8, min/max/clamp, validation4, and bounded-search4 measure current source/unrolled encodings. Sum8 and bounded-search4 exceed the 50,000-sample limit before allocating the Cartesian product. A verified list transform is inferred to hit the same tested list-parameter boundary; generated application lists are not verified collections.\n- **Array/collection:** `verified_array_parameter_boundary` records the actual typecheck rejection for a list aggregation parameter. A separate transform body is not benchmarked because its list parameter would be rejected before verified body synthesis. Static application list serialization/rendering costs are reported separately.\n- **Recursion:** the `sum_to(n -% 1)` source-shaped case parses but project loading rejects its recursive contract cycle before any termination analysis. The cross-module cycle case is also rejected. No tree-walk or divide-and-conquer workload was observed, so recursion is not the selected priority.\n\n");
    output.push_str(
        "## Priority recommendation\n\nThe former nested-summary blocker is exercised at depths 1/2/4/8 with both dependency-first verification and the release CLI build. The next measured pressure remains bounded search: three-parameter max and bitwise synthesis reach the 50,000 candidate-generation bound, while eight-parameter sum exceeds the 50,000 sample bound. Fixed arrays remain a separate language boundary: list parameters are rejected by verified contracts, while static runtime data handling is measured independently. These timings are local evidence, not guarantees.\n\n## Case details\n\n");
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
