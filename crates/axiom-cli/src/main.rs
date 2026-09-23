use std::{
    env,
    fmt::Write as _,
    fs,
    path::{Path, PathBuf},
    process::ExitCode,
    time::Duration,
};

use axiom_application::{
    ApplicationCall, ApplicationValue, BoundaryOperation, VerifiedComputation,
};

const SEMANTICS_VERSION: &str = "v4.1-scalar-baseline";
type SynthesisOutput = (
    axiom_parser::Contract,
    axiom_ami::Expr,
    Vec<u8>,
    Option<String>,
    usize,
);

mod web;

fn main() -> ExitCode {
    match run(env::args().skip(1).collect()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            eprintln!("usage:");
            eprintln!("  axiom dev [--port N] [--no-open] [--once]");
            eprintln!("  axiom check <contract.ax>");
            eprintln!("  axiom check");
            eprintln!(
                "  axiom build <contract.ax> [-o output.wasm] [--report report.json] [--timeout-ms N] [--max-candidates N] [--max-memory-bytes N]"
            );
            eprintln!("  axiom build");
            eprintln!(
                "  axiom run <contract.ax> [--] [<value>...] [-o output.wasm] [--report report.json] [--timeout-ms N] [--max-candidates N] [--max-memory-bytes N]"
            );
            eprintln!("  axiom app-run <contract.ax> [--] [<application-value>...]");
            eprintln!("  axiom <contract.ax> [-o output.wasm]");
            ExitCode::FAILURE
        }
    }
}

fn run(args: Vec<String>) -> Result<(), String> {
    match args.first().map(String::as_str) {
        Some("dev") => dev_command(&args[1..]),
        Some("check") => check_command(&args[1..]),
        Some("build") => build_command(&args[1..]),
        Some("run") => run_command(&args[1..]),
        Some("app-run") => application_run_command(&args[1..]),
        _ => build_command(&args),
    }
}

fn check_command(args: &[String]) -> Result<(), String> {
    if args.is_empty() {
        let config =
            axiom_project::find_config(env::current_dir().map_err(|error| error.to_string())?)?;
        return web::check(&config);
    }
    let input = single_input(args, "check")?;
    let contract = read_contract(input)?;
    let contract_check = axiom_verify::check_contract(&contract)?;
    if !matches!(contract_check, axiom_verify::ContractCheck::Valid) {
        return Err(format_contract_check_error(&contract_check));
    }

    println!("contract: {}", contract.name);
    println!("contract_check: {}", contract_check.status_name());
    println!("status: valid");
    Ok(())
}

fn build_command(args: &[String]) -> Result<(), String> {
    if args.is_empty() {
        let config =
            axiom_project::find_config(env::current_dir().map_err(|error| error.to_string())?)?;
        let root = config
            .parent()
            .ok_or_else(|| "cannot determine project root".to_string())?;
        return web::build(&config, &root.join("dist")).map(|_| ());
    }
    let options = parse_build_args(args)?;
    let (contract, candidate, module, diagnostic, candidates_checked) = match synthesize(
        options.input,
        options.timeout,
        options.max_candidates,
        options.max_memory_bytes,
    ) {
        Ok(result) => result,
        Err(error) => {
            if let Some(report) = &options.report {
                write_failure_report(
                    report,
                    "build",
                    &error,
                    ReportDetails {
                        source: options.input,
                        inputs: None,
                        result: None,
                        diagnostic: None,
                        timeout: options.timeout,
                        max_candidates: options.max_candidates,
                        max_memory_bytes: options.max_memory_bytes,
                        candidates_checked: 0,
                    },
                )?;
            }
            return Err(error);
        }
    };
    fs::write(&options.output, &module)
        .map_err(|error| format!("cannot write {}: {error}", options.output.display()))?;
    if let Some(report) = &options.report {
        write_report(
            report,
            "build",
            &contract,
            &candidate,
            &options.output,
            ReportDetails {
                inputs: None,
                result: None,
                diagnostic: diagnostic.as_deref(),
                source: options.input,
                timeout: options.timeout,
                max_candidates: options.max_candidates,
                max_memory_bytes: options.max_memory_bytes,
                candidates_checked,
            },
        )?;
    }

    println!("contract: {}", contract.name);
    println!("contract_check: VALID");
    println!("synthesis_result: FOUND");
    println!("proof_result: VERIFIED");
    if let Some(diagnostic) = &diagnostic {
        println!("diagnostic: {diagnostic}");
    }
    println!("candidate: {candidate:?}");
    println!("verified: true");
    println!("wrote: {}", options.output.display());
    if let Some(report) = options.report {
        println!("report: {}", report.display());
    }
    Ok(())
}

fn dev_command(args: &[String]) -> Result<(), String> {
    let mut port = 3000_u16;
    let mut open_browser = true;
    let mut once = false;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--port" => {
                index += 1;
                let value = args
                    .get(index)
                    .ok_or_else(|| "missing port after --port".to_string())?;
                port = value
                    .parse()
                    .map_err(|error| format!("invalid port '{value}': {error}"))?;
            }
            "--no-open" => open_browser = false,
            "--once" => once = true,
            flag => return Err(format!("unknown dev option: {flag}")),
        }
        index += 1;
    }

    let config =
        axiom_project::find_config(env::current_dir().map_err(|error| error.to_string())?)?;
    web::dev(&config, port, open_browser, once)
}

fn run_command(args: &[String]) -> Result<(), String> {
    let options = parse_run_args(args)?;
    let contract = read_contract(options.input)?;
    if contract.params.len() != options.values.len() {
        return Err(format!(
            "contract expects {} inputs, got {}",
            contract.params.len(),
            options.values.len()
        ));
    }
    let values = options
        .values
        .iter()
        .zip(&contract.params)
        .map(|(value, param)| parse_input_value(value, &param.ty))
        .collect::<Result<Vec<_>, _>>()?;
    if let Err(error) = axiom_synth::check_requires(&contract, &values) {
        if let Some(report) = &options.report {
            write_failure_report(
                report,
                "run",
                &error,
                ReportDetails {
                    source: options.input,
                    inputs: Some(&values),
                    result: None,
                    diagnostic: None,
                    timeout: options.timeout,
                    max_candidates: options.max_candidates,
                    max_memory_bytes: options.max_memory_bytes,
                    candidates_checked: 0,
                },
            )?;
        }
        return Err(error);
    }
    let (candidate, module, diagnostic, candidates_checked) =
        match axiom_synth::synthesize_wasm_with_metrics(
            &contract,
            axiom_synth::SynthesisOptions {
                timeout: options.timeout,
                max_candidates: options.max_candidates,
                max_memory_bytes: options.max_memory_bytes,
            },
        ) {
            Ok(result) => result,
            Err(error) => {
                if let Some(report) = &options.report {
                    write_failure_report(
                        report,
                        "run",
                        &error,
                        ReportDetails {
                            source: options.input,
                            inputs: Some(&values),
                            result: None,
                            diagnostic: None,
                            timeout: options.timeout,
                            max_candidates: options.max_candidates,
                            max_memory_bytes: options.max_memory_bytes,
                            candidates_checked: 0,
                        },
                    )?;
                }
                return Err(error);
            }
        };
    fs::write(&options.output, &module)
        .map_err(|error| format!("cannot write {}: {error}", options.output.display()))?;
    let result = axiom_wasm::run_with_args(&module, &values)?;
    if let Some(report) = &options.report {
        write_report(
            report,
            "run",
            &contract,
            &candidate,
            &options.output,
            ReportDetails {
                inputs: Some(&values),
                result: Some(result),
                diagnostic: diagnostic
                    .as_ref()
                    .map(axiom_verify::Diagnostic::status_name),
                source: options.input,
                timeout: options.timeout,
                max_candidates: options.max_candidates,
                max_memory_bytes: options.max_memory_bytes,
                candidates_checked,
            },
        )?;
    }

    println!("contract: {}", contract.name);
    println!("contract_check: VALID");
    println!("synthesis_result: FOUND");
    println!("proof_result: VERIFIED");
    if let Some(diagnostic) = &diagnostic {
        println!("diagnostic: {}", diagnostic.status_name());
    }
    println!("candidate: {candidate:?}");
    println!("verified: true");
    if values.len() == 1 {
        println!(
            "input: {}",
            display_result(&contract.params[0].ty, values[0])
        );
    } else {
        let inputs = contract
            .params
            .iter()
            .zip(&values)
            .map(|(param, value)| display_result(&param.ty, *value))
            .collect::<Vec<_>>();
        println!("inputs: {inputs:?}");
    }
    println!("result: {}", display_result(&contract.return_type, result));
    println!("wrote: {}", options.output.display());
    if let Some(report) = options.report {
        println!("report: {}", report.display());
    }
    Ok(())
}

fn application_run_command(args: &[String]) -> Result<(), String> {
    let input = args
        .first()
        .ok_or_else(|| "app-run requires a contract file".to_string())?;
    let values_start = if args.get(1).map(String::as_str) == Some("--") {
        2
    } else {
        1
    };
    let values = args[values_start..]
        .iter()
        .map(|value| ApplicationValue::from_cli_token(value))
        .collect::<Vec<_>>();
    let contract = read_contract(input)?;
    let verified = VerifiedComputation::synthesize(&contract)
        .map_err(|error| format!("cannot prepare verified computation: {error}"))?;
    let call = ApplicationCall::new(verified, &values).map_err(|error| error.to_string())?;
    let result = BoundaryOperation::Application(call)
        .execute()
        .map_err(|error| error.to_string())?;

    println!("contract: {}", contract.name);
    println!("boundary: APPLICATION -> VERIFIED_CORE -> APPLICATION");
    println!("proof_result: VERIFIED");
    println!("result: {result:?}");
    Ok(())
}

fn display_result(ty: &axiom_parser::Type, result: u32) -> String {
    match ty {
        axiom_parser::Type::Bool => (result != 0).to_string(),
        axiom_parser::Type::U32 => result.to_string(),
        axiom_parser::Type::I32 => (result as i32).to_string(),
        _ => result.to_string(),
    }
}

fn parse_input_value(value: &str, ty: &axiom_parser::Type) -> Result<u32, String> {
    match ty {
        axiom_parser::Type::U32 => value
            .parse::<u32>()
            .map_err(|error| format!("invalid u32 input '{value}': {error}")),
        axiom_parser::Type::I32 => value
            .parse::<i32>()
            .map(|value| value as u32)
            .map_err(|error| format!("invalid i32 input '{value}': {error}")),
        axiom_parser::Type::Bool => match value {
            "true" | "1" => Ok(1),
            "false" | "0" => Ok(0),
            _ => Err(format!(
                "invalid bool input '{value}' (use true, false, 1, or 0)"
            )),
        },
        _ => Err(format!(
            "cannot run a contract with {} input type",
            type_name(ty)
        )),
    }
}

fn type_name(ty: &axiom_parser::Type) -> &'static str {
    match ty {
        axiom_parser::Type::U32 => "u32",
        axiom_parser::Type::I32 => "i32",
        axiom_parser::Type::Bool => "bool",
        _ => "non-scalar",
    }
}

fn synthesize(
    input: &str,
    timeout: Option<Duration>,
    max_candidates: usize,
    max_memory_bytes: usize,
) -> Result<SynthesisOutput, String> {
    let contract = read_contract(input)?;
    let (candidate, module, diagnostic, candidates_checked) =
        axiom_synth::synthesize_wasm_with_metrics(
            &contract,
            axiom_synth::SynthesisOptions {
                timeout,
                max_candidates,
                max_memory_bytes,
            },
        )?;
    Ok((
        contract,
        candidate,
        module,
        diagnostic.map(|value| value.status_name().to_string()),
        candidates_checked,
    ))
}

fn read_contract(input: &str) -> Result<axiom_parser::Contract, String> {
    let source =
        fs::read_to_string(input).map_err(|error| format!("cannot read {input}: {error}"))?;
    axiom_parser::parse(&source).map_err(|error| format!("INVALID: {error}"))
}

fn format_contract_check_error(check: &axiom_verify::ContractCheck) -> String {
    match check {
        axiom_verify::ContractCheck::Valid => "VALID".to_string(),
        axiom_verify::ContractCheck::Invalid(error) => format!("INVALID: {error}"),
        axiom_verify::ContractCheck::UnsatisfiablePrecondition => {
            "UNSATISFIABLE_PRECONDITION: requires clauses are contradictory".to_string()
        }
        axiom_verify::ContractCheck::UnsatisfiablePostcondition => {
            "UNSATISFIABLE_POSTCONDITION: ensures clauses have no valid result".to_string()
        }
    }
}

fn write_report(
    path: &Path,
    command: &str,
    contract: &axiom_parser::Contract,
    candidate: &axiom_ami::Expr,
    output: &Path,
    details: ReportDetails<'_>,
) -> Result<(), String> {
    let source_bytes = fs::read(details.source)
        .map_err(|error| format!("cannot read {} for report: {error}", details.source))?;
    let wasm_bytes = fs::read(output)
        .map_err(|error| format!("cannot read {} for report: {error}", output.display()))?;
    let mut artifact = ReportArtifact {
        command: command.to_string(),
        source: details.source.to_string(),
        contract: Some(contract.name.clone()),
        contract_check: Some("VALID".to_string()),
        synthesis_result: Some("FOUND".to_string()),
        proof_result: Some("VERIFIED".to_string()),
        solver_result: Some("UNSAT".to_string()),
        diagnostic: details.diagnostic.map(str::to_string),
        reason: "The contract is valid and the synthesized candidate is proven by Z3.".to_string(),
        next_action: "Run or integrate the emitted WASM artifact.".to_string(),
        candidate: Some(format!("{candidate:?}")),
        verified: true,
        timeout_ms: details.timeout.map(|value| value.as_millis()),
        timed_out: false,
        max_candidates: details.max_candidates,
        max_memory_bytes: details.max_memory_bytes,
        candidate_count: Some(details.candidates_checked),
        resource_status: None,
        resource_limit: None,
        counterexample: None,
        verification_artifact_hash: None,
        wasm_hash: Some(stable_hash(&wasm_bytes)),
        output: Some(output.display().to_string()),
        inputs: details.inputs.map(<[u32]>::to_vec),
        result: details.result,
    };
    write_artifact(path, &mut artifact, &source_bytes)
}

struct ReportDetails<'a> {
    source: &'a str,
    inputs: Option<&'a [u32]>,
    result: Option<u32>,
    diagnostic: Option<&'a str>,
    timeout: Option<Duration>,
    max_candidates: usize,
    max_memory_bytes: usize,
    candidates_checked: usize,
}

fn stable_hash(bytes: &[u8]) -> String {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("fnv1a64:{hash:016x}")
}

fn write_failure_report(
    path: &Path,
    command: &str,
    error: &str,
    details: ReportDetails<'_>,
) -> Result<(), String> {
    let contract_check = error_status(error, "contract");
    let synthesis_result = error_status(error, "synthesis");
    let proof_result = error_status(error, "proof");
    let contract = read_contract(details.source).ok();
    let resource_limited = synthesis_result == "RESOURCE_LIMIT";
    let timed_out = synthesis_result == "TIMEOUT";
    let source_bytes =
        fs::read(details.source).unwrap_or_else(|_| details.source.as_bytes().to_vec());
    let mut artifact = ReportArtifact {
        command: command.to_string(),
        source: details.source.to_string(),
        contract: contract.as_ref().map(|contract| contract.name.clone()),
        contract_check: (contract_check != "null").then_some(contract_check),
        synthesis_result: (synthesis_result != "null").then_some(synthesis_result),
        proof_result: (proof_result != "null").then_some(proof_result.clone()),
        solver_result: solver_result_for_error(error).map(str::to_string),
        diagnostic: error
            .contains("CALL_PRECONDITION_VIOLATION")
            .then(|| "CALL_PRECONDITION_VIOLATION".to_string()),
        reason: error.to_string(),
        next_action: next_action_for_status(error).to_string(),
        candidate: None,
        verified: false,
        timeout_ms: details.timeout.map(|value| value.as_millis()),
        timed_out,
        max_candidates: details.max_candidates,
        max_memory_bytes: details.max_memory_bytes,
        candidate_count: parse_candidate_count(error),
        resource_status: resource_limited.then(|| "RESOURCE_LIMIT".to_string()),
        resource_limit: resource_limited
            .then(|| parse_resource_limit(error).unwrap_or(details.max_candidates)),
        counterexample: parse_counterexample(error),
        verification_artifact_hash: None,
        wasm_hash: None,
        output: None,
        inputs: details.inputs.map(<[u32]>::to_vec),
        result: None,
    };
    write_artifact(path, &mut artifact, &source_bytes)
}

#[derive(Debug)]
struct ReportArtifact {
    command: String,
    source: String,
    contract: Option<String>,
    contract_check: Option<String>,
    synthesis_result: Option<String>,
    proof_result: Option<String>,
    solver_result: Option<String>,
    diagnostic: Option<String>,
    reason: String,
    next_action: String,
    candidate: Option<String>,
    verified: bool,
    timeout_ms: Option<u128>,
    timed_out: bool,
    max_candidates: usize,
    max_memory_bytes: usize,
    candidate_count: Option<usize>,
    resource_status: Option<String>,
    resource_limit: Option<usize>,
    counterexample: Option<Vec<u32>>,
    verification_artifact_hash: Option<String>,
    wasm_hash: Option<String>,
    output: Option<String>,
    inputs: Option<Vec<u32>>,
    result: Option<u32>,
}

fn write_artifact(
    path: &Path,
    artifact: &mut ReportArtifact,
    source_bytes: &[u8],
) -> Result<(), String> {
    let mut integrity_bytes = source_bytes.to_vec();
    integrity_bytes.extend_from_slice(format!("\n{artifact:?}").as_bytes());
    artifact.verification_artifact_hash = Some(stable_hash(&integrity_bytes));
    let contents = format!(
        "{{\n  \"version\": 4,\n  \"command\": {},\n  \"source\": {},\n  \"contract\": {},\n  \"semantics_version\": {},\n  \"contract_check\": {},\n  \"synthesis_result\": {},\n  \"proof_result\": {},\n  \"solver_result\": {},\n  \"diagnostic\": {},\n  \"reason\": {},\n  \"next_action\": {},\n  \"candidate\": {},\n  \"verified\": {},\n  \"synthesis_bounds\": {{\"timeout_ms\": {}, \"max_candidates\": {}, \"max_memory_bytes\": {}}},\n  \"candidate_count\": {},\n  \"candidates_checked\": {},\n  \"timed_out\": {},\n  \"resource_status\": {},\n  \"resource_limit\": {},\n  \"counterexample\": {},\n  \"verification_artifact_hash\": {},\n  \"wasm_hash\": {},\n  \"output\": {},\n  \"inputs\": {},\n  \"result\": {}\n}}\n",
        json_string(&artifact.command),
        json_string(&artifact.source),
        option_string(&artifact.contract),
        json_string(SEMANTICS_VERSION),
        option_string(&artifact.contract_check),
        option_string(&artifact.synthesis_result),
        option_string(&artifact.proof_result),
        option_string(&artifact.solver_result),
        option_string(&artifact.diagnostic),
        json_string(&artifact.reason),
        json_string(&artifact.next_action),
        option_string(&artifact.candidate),
        artifact.verified,
        option_u128(artifact.timeout_ms),
        artifact.max_candidates,
        artifact.max_memory_bytes,
        option_usize(artifact.candidate_count),
        option_usize(artifact.candidate_count),
        artifact.timed_out,
        option_string(&artifact.resource_status),
        option_usize(artifact.resource_limit),
        option_u32_vec(&artifact.counterexample),
        option_string(&artifact.verification_artifact_hash),
        option_string(&artifact.wasm_hash),
        option_string(&artifact.output),
        option_u32_vec(&artifact.inputs),
        artifact
            .result
            .map_or_else(|| "null".to_string(), |value| value.to_string()),
    );
    fs::write(path, contents).map_err(|error| format!("cannot write {}: {error}", path.display()))
}

fn option_string(value: &Option<String>) -> String {
    value
        .as_deref()
        .map(json_string)
        .unwrap_or_else(|| "null".to_string())
}

fn option_u128(value: Option<u128>) -> String {
    value.map_or_else(|| "null".to_string(), |value| value.to_string())
}

fn option_usize(value: Option<usize>) -> String {
    value.map_or_else(|| "null".to_string(), |value| value.to_string())
}

fn option_u32_vec(value: &Option<Vec<u32>>) -> String {
    value
        .as_ref()
        .map(|values| {
            format!(
                "[{}]",
                values
                    .iter()
                    .map(u32::to_string)
                    .collect::<Vec<_>>()
                    .join(",")
            )
        })
        .unwrap_or_else(|| "null".to_string())
}

fn parse_candidate_count(error: &str) -> Option<usize> {
    let remainder = error
        .split_once("candidates checked: ")
        .map(|(_, rest)| rest)
        .or_else(|| error.split_once("after checking ").map(|(_, rest)| rest))
        .or_else(|| error.split_once("candidate limit (").map(|(_, rest)| rest))?;
    remainder
        .split(|character: char| !character.is_ascii_digit())
        .next()?
        .parse()
        .ok()
}

fn parse_counterexample(error: &str) -> Option<Vec<u32>> {
    let marker = "last counterexample: [";
    let start = error.find(marker)? + marker.len();
    let end = error[start..].find(']')? + start;
    error[start..end]
        .split(',')
        .map(|value| value.trim().parse().ok())
        .collect()
}

fn parse_resource_limit(error: &str) -> Option<usize> {
    let (_, bounds) = error.split_once("candidate limit (")?;
    bounds.split_once('/')?.1.split(')').next()?.parse().ok()
}

fn solver_result_for_error(error: &str) -> Option<&'static str> {
    if error.contains("SOLVER_UNKNOWN") {
        Some("UNKNOWN")
    } else if error.contains("COUNTEREXAMPLE") || error.contains("last counterexample:") {
        Some("SAT")
    } else if error.contains("UNSATISFIABLE_PRECONDITION")
        || error.contains("UNSATISFIABLE_POSTCONDITION")
    {
        Some("UNSAT")
    } else {
        None
    }
}

fn error_status(error: &str, kind: &str) -> String {
    let status = [
        "INVALID",
        "UNSATISFIABLE_PRECONDITION",
        "UNSATISFIABLE_POSTCONDITION",
        "NO_CANDIDATE",
        "TIMEOUT",
        "RESOURCE_LIMIT",
        "SOLVER_UNKNOWN",
        "COUNTEREXAMPLE",
    ]
    .into_iter()
    .find(|status| error.contains(status));

    match (kind, status) {
        (
            "contract",
            Some(
                status @ ("INVALID" | "UNSATISFIABLE_PRECONDITION" | "UNSATISFIABLE_POSTCONDITION"),
            ),
        ) => status.to_string(),
        ("synthesis", Some(status @ ("NO_CANDIDATE" | "TIMEOUT" | "RESOURCE_LIMIT"))) => {
            status.to_string()
        }
        ("proof", Some(status @ ("SOLVER_UNKNOWN" | "COUNTEREXAMPLE"))) => status.to_string(),
        ("contract", _) => "VALID".to_string(),
        ("synthesis", _) => "null".to_string(),
        ("proof", _) => "null".to_string(),
        _ => "null".to_string(),
    }
}

fn next_action_for_status(error: &str) -> &'static str {
    if error.contains("INVALID") {
        "Fix the syntax or type error and run the check again."
    } else if error.contains("UNSATISFIABLE_PRECONDITION") {
        "Make the requires clauses jointly satisfiable."
    } else if error.contains("UNSATISFIABLE_POSTCONDITION") {
        "Relax or correct the ensures clauses so a result can exist."
    } else if error.contains("NO_CANDIDATE") {
        "Simplify the contract or stay within the supported synthesis boundary."
    } else if error.contains("TIMEOUT") {
        "Increase the timeout or simplify the contract."
    } else if error.contains("RESOURCE_LIMIT") {
        "Increase the candidate limit or simplify the contract."
    } else if error.contains("SOLVER_UNKNOWN") {
        "Retry or inspect the solver configuration; no proof was accepted."
    } else {
        "Inspect the error and rerun the command."
    }
}

fn json_string(value: &str) -> String {
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
                let _ = write!(result, "\\u{:04x}", character as u32);
            }
            character => result.push(character),
        }
    }
    result.push('"');
    result
}

fn single_input<'a>(args: &'a [String], command: &str) -> Result<&'a str, String> {
    match args {
        [input] => Ok(input),
        [] => Err(format!("missing input file for {command}")),
        _ => Err(format!("expected exactly one input file for {command}")),
    }
}

struct BuildArgs<'a> {
    input: &'a str,
    output: PathBuf,
    report: Option<PathBuf>,
    timeout: Option<Duration>,
    max_candidates: usize,
    max_memory_bytes: usize,
}

fn parse_build_args(args: &[String]) -> Result<BuildArgs<'_>, String> {
    let input = args
        .first()
        .map(String::as_str)
        .ok_or_else(|| "missing input file".to_string())?;
    let mut output = default_output_path(input);
    let mut report = None;
    let mut timeout = None;
    let mut max_candidates = axiom_synth::SynthesisOptions::default().max_candidates;
    let mut max_memory_bytes = axiom_synth::SynthesisOptions::default().max_memory_bytes;
    let mut index = 1;

    while index < args.len() {
        match args[index].as_str() {
            "-o" | "--output" => {
                output = PathBuf::from(required_value(args, &mut index, "output path")?);
            }
            "--report" => {
                report = Some(PathBuf::from(required_value(
                    args,
                    &mut index,
                    "report path",
                )?));
            }
            "--timeout-ms" => {
                timeout = Some(parse_timeout(required_value(
                    args,
                    &mut index,
                    "timeout in milliseconds",
                )?)?);
            }
            "--max-candidates" => {
                max_candidates =
                    parse_limit(required_value(args, &mut index, "maximum candidate count")?)?;
            }
            "--max-memory-bytes" => {
                max_memory_bytes =
                    parse_limit(required_value(args, &mut index, "maximum memory in bytes")?)?;
            }
            flag => return Err(format!("unknown build option: {flag}")),
        }
        index += 1;
    }

    Ok(BuildArgs {
        input,
        output,
        report,
        timeout,
        max_candidates,
        max_memory_bytes,
    })
}

struct RunArgs<'a> {
    input: &'a str,
    values: Vec<String>,
    output: PathBuf,
    report: Option<PathBuf>,
    timeout: Option<Duration>,
    max_candidates: usize,
    max_memory_bytes: usize,
}

fn parse_run_args(args: &[String]) -> Result<RunArgs<'_>, String> {
    if args.is_empty() {
        return Err("run requires an input file".to_string());
    }

    let input = args[0].as_str();
    let mut values = Vec::new();
    let mut output = default_output_path(input);
    let mut report = None;
    let mut timeout = None;
    let mut max_candidates = axiom_synth::SynthesisOptions::default().max_candidates;
    let mut max_memory_bytes = axiom_synth::SynthesisOptions::default().max_memory_bytes;
    let mut index = if args.get(1).map(String::as_str) == Some("--") {
        2
    } else {
        1
    };

    while index < args.len() {
        match args[index].as_str() {
            "-o" | "--output" => {
                output = PathBuf::from(required_value(args, &mut index, "output path")?);
            }
            "--report" => {
                report = Some(PathBuf::from(required_value(
                    args,
                    &mut index,
                    "report path",
                )?));
            }
            "--timeout-ms" => {
                timeout = Some(parse_timeout(required_value(
                    args,
                    &mut index,
                    "timeout in milliseconds",
                )?)?);
            }
            "--max-candidates" => {
                max_candidates =
                    parse_limit(required_value(args, &mut index, "maximum candidate count")?)?;
            }
            "--max-memory-bytes" => {
                max_memory_bytes =
                    parse_limit(required_value(args, &mut index, "maximum memory in bytes")?)?;
            }
            "--" => {}
            value => values.push(value.to_string()),
        }
        index += 1;
    }

    Ok(RunArgs {
        input,
        values,
        output,
        report,
        timeout,
        max_candidates,
        max_memory_bytes,
    })
}

fn required_value<'a>(
    args: &'a [String],
    index: &mut usize,
    description: &str,
) -> Result<&'a str, String> {
    *index += 1;
    args.get(*index)
        .map(String::as_str)
        .ok_or_else(|| format!("missing {description}"))
}

fn parse_timeout(value: &str) -> Result<Duration, String> {
    value
        .parse::<u64>()
        .map(Duration::from_millis)
        .map_err(|error| format!("invalid timeout '{value}': {error}"))
}

fn parse_limit(value: &str) -> Result<usize, String> {
    value
        .parse::<usize>()
        .map_err(|error| format!("invalid candidate limit '{value}': {error}"))
}

fn default_output_path(input: &str) -> PathBuf {
    let path = Path::new(input);
    let stem = match path.file_name().and_then(|value| value.to_str()) {
        Some(".ax") => "out",
        _ => path
            .file_stem()
            .and_then(|value| value.to_str())
            .filter(|value| !value.is_empty())
            .unwrap_or("out"),
    };
    path.with_file_name(format!("{stem}.wasm"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_run_value_with_separator() {
        let args = vec!["mul8.ax".to_string(), "--".to_string(), "5".to_string()];

        assert_eq!(parse_run_args(&args).unwrap().values, vec!["5"]);
    }

    #[test]
    fn parses_multiple_run_values_and_output() {
        let args = vec![
            "add.ax".to_string(),
            "--".to_string(),
            "5".to_string(),
            "7".to_string(),
            "-o".to_string(),
            "add.wasm".to_string(),
        ];

        let parsed = parse_run_args(&args).unwrap();
        assert_eq!(parsed.values, vec!["5", "7"]);
        assert_eq!(parsed.output, PathBuf::from("add.wasm"));
    }

    #[test]
    fn parses_zero_input_run() {
        let args = vec!["zero.ax".to_string(), "--".to_string()];

        assert!(parse_run_args(&args).unwrap().values.is_empty());
    }

    #[test]
    fn parses_signed_i32_input() {
        assert_eq!(
            parse_input_value("-1", &axiom_parser::Type::I32).unwrap(),
            u32::MAX
        );
        assert_eq!(display_result(&axiom_parser::Type::I32, u32::MAX), "-1");
    }

    #[test]
    fn avoids_hidden_ax_output_name() {
        assert_eq!(
            default_output_path("editors/.ax"),
            PathBuf::from("editors/out.wasm")
        );
        assert_eq!(
            default_output_path("examples/basics/mul8.ax"),
            PathBuf::from("examples/basics/mul8.wasm")
        );
    }

    #[test]
    fn parses_build_report_and_timeout() {
        let args = vec![
            "mul8.ax".to_string(),
            "--report".to_string(),
            "build.json".to_string(),
            "--timeout-ms".to_string(),
            "250".to_string(),
            "--max-candidates".to_string(),
            "12".to_string(),
            "--max-memory-bytes".to_string(),
            "4096".to_string(),
        ];

        let parsed = parse_build_args(&args).unwrap();
        assert_eq!(parsed.report, Some(PathBuf::from("build.json")));
        assert_eq!(parsed.timeout, Some(Duration::from_millis(250)));
        assert_eq!(parsed.max_candidates, 12);
        assert_eq!(parsed.max_memory_bytes, 4096);
    }

    #[test]
    fn maps_state_errors_to_report_statuses_and_actions() {
        let error = "RESOURCE_LIMIT: synthesis reached the candidate limit (0/0)";

        assert_eq!(error_status(error, "contract"), "VALID");
        assert_eq!(error_status(error, "synthesis"), "RESOURCE_LIMIT");
        assert_eq!(error_status(error, "proof"), "null");
        assert!(next_action_for_status(error).contains("candidate limit"));
        assert_eq!(parse_candidate_count(error), Some(0));
        assert_eq!(parse_resource_limit(error), Some(0));
        assert_eq!(
            parse_counterexample("NO_CANDIDATE: last counterexample: [7, 4294967295]"),
            Some(vec![7, u32::MAX])
        );
    }

    #[test]
    fn artifact_fingerprint_is_stable_and_content_sensitive() {
        assert_eq!(stable_hash(b"artifact"), stable_hash(b"artifact"));
        assert_ne!(stable_hash(b"artifact"), stable_hash(b"different"));
    }

    #[test]
    fn success_and_failure_reports_share_the_v4_artifact_schema() {
        let root = std::env::temp_dir().join(format!(
            "synthetix-report-schema-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let success_path = root.join("success.json");
        let failure_path = root.join("failure.json");
        let fields = [
            "contract",
            "candidate",
            "semantics_version",
            "synthesis_bounds",
            "candidate_count",
            "timed_out",
            "resource_status",
            "resource_limit",
            "counterexample",
            "solver_result",
            "proof_result",
            "verification_artifact_hash",
            "wasm_hash",
        ];
        let success = ReportArtifact {
            command: "build".to_string(),
            source: "success.ax".to_string(),
            contract: Some("valid".to_string()),
            contract_check: Some("VALID".to_string()),
            synthesis_result: Some("FOUND".to_string()),
            proof_result: Some("VERIFIED".to_string()),
            solver_result: Some("UNSAT".to_string()),
            diagnostic: None,
            reason: "verified".to_string(),
            next_action: "run".to_string(),
            candidate: Some("Arg(0)".to_string()),
            verified: true,
            timeout_ms: None,
            timed_out: false,
            max_candidates: 50000,
            max_memory_bytes: 1024,
            candidate_count: Some(1),
            resource_status: None,
            resource_limit: None,
            counterexample: None,
            verification_artifact_hash: None,
            wasm_hash: Some("fnv1a64:abc".to_string()),
            output: Some("valid.wasm".to_string()),
            inputs: None,
            result: None,
        };
        let mut success = success;
        write_artifact(&success_path, &mut success, b"success source").unwrap();
        let success_json = fs::read_to_string(&success_path).unwrap();

        let mut failure = ReportArtifact {
            command: "build".to_string(),
            source: "failure.ax".to_string(),
            contract: Some("bounded".to_string()),
            contract_check: Some("VALID".to_string()),
            synthesis_result: Some("RESOURCE_LIMIT".to_string()),
            proof_result: None,
            solver_result: None,
            diagnostic: None,
            reason: "resource limit".to_string(),
            next_action: "retry".to_string(),
            candidate: None,
            verified: false,
            timeout_ms: Some(10),
            timed_out: false,
            max_candidates: 100,
            max_memory_bytes: 1024,
            candidate_count: Some(0),
            resource_status: Some("RESOURCE_LIMIT".to_string()),
            resource_limit: Some(100),
            counterexample: None,
            verification_artifact_hash: None,
            wasm_hash: None,
            output: None,
            inputs: None,
            result: None,
        };
        write_artifact(&failure_path, &mut failure, b"failure source").unwrap();
        let failure_json = fs::read_to_string(&failure_path).unwrap();

        for field in fields {
            assert!(success_json.contains(&format!("\"{field}\":")));
            assert!(failure_json.contains(&format!("\"{field}\":")));
        }
        let top_level_fields = |json: &str| {
            json.lines()
                .filter_map(|line| {
                    let line = line.strip_prefix("  \"")?;
                    Some(line.split_once('\"')?.0.to_string())
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(
            top_level_fields(&success_json),
            top_level_fields(&failure_json)
        );
        assert!(success_json.contains("\"verified\": true"));
        assert!(success_json.contains("\"verification_artifact_hash\": \"fnv1a64:"));
        assert!(success_json.contains("\"wasm_hash\": \"fnv1a64:abc\""));
        assert!(failure_json.contains("\"verified\": false"));
        assert!(failure_json.contains("\"verification_artifact_hash\": \"fnv1a64:"));
        assert!(failure_json.contains("\"candidate\": null"));
        assert!(failure_json.contains("\"wasm_hash\": null"));
        assert!(!success_json.contains("proof_hash"));
        assert!(!failure_json.contains("proof_hash"));

        fs::remove_dir_all(root).unwrap();
    }
}
