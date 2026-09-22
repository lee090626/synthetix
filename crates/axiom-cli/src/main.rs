use std::{
    env,
    fmt::Write as _,
    fs,
    path::{Path, PathBuf},
    process::ExitCode,
    time::Duration,
};

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
                "  axiom build <contract.ax> [-o output.wasm] [--report report.json] [--timeout-ms N] [--max-candidates N]"
            );
            eprintln!("  axiom build");
            eprintln!(
                "  axiom run <contract.ax> [--] [<value>...] [-o output.wasm] [--report report.json] [--timeout-ms N] [--max-candidates N]"
            );
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

    println!("contract: {}", contract.name);
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
    let (contract, candidate, module) =
        synthesize(options.input, options.timeout, options.max_candidates)?;
    fs::write(&options.output, &module)
        .map_err(|error| format!("cannot write {}: {error}", options.output.display()))?;
    if let Some(report) = &options.report {
        write_report(
            report,
            "build",
            &contract,
            &candidate,
            &options.output,
            None,
            None,
        )?;
    }

    println!("contract: {}", contract.name);
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
    axiom_synth::check_requires(&contract, &values)?;
    let (candidate, module) = axiom_synth::synthesize_wasm_with_options(
        &contract,
        axiom_synth::SynthesisOptions {
            timeout: options.timeout,
            max_candidates: options.max_candidates,
        },
    )?;
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
            Some(&values),
            Some(result),
        )?;
    }

    println!("contract: {}", contract.name);
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
) -> Result<(axiom_parser::Contract, axiom_ami::Expr, Vec<u8>), String> {
    let contract = read_contract(input)?;
    let (candidate, module) = axiom_synth::synthesize_wasm_with_options(
        &contract,
        axiom_synth::SynthesisOptions {
            timeout,
            max_candidates,
        },
    )?;
    Ok((contract, candidate, module))
}

fn read_contract(input: &str) -> Result<axiom_parser::Contract, String> {
    let source =
        fs::read_to_string(input).map_err(|error| format!("cannot read {input}: {error}"))?;
    axiom_parser::parse(&source)
}

fn write_report(
    path: &Path,
    command: &str,
    contract: &axiom_parser::Contract,
    candidate: &axiom_ami::Expr,
    output: &Path,
    inputs: Option<&[u32]>,
    result: Option<u32>,
) -> Result<(), String> {
    let input_json = inputs
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
        .unwrap_or_else(|| "null".to_string());
    let result_json = result
        .map(|value| value.to_string())
        .unwrap_or_else(|| "null".to_string());
    let contents = format!(
        "{{\n  \"version\": 1,\n  \"command\": {},\n  \"contract\": {},\n  \"candidate\": {},\n  \"verified\": true,\n  \"output\": {},\n  \"inputs\": {},\n  \"result\": {}\n}}\n",
        json_string(command),
        json_string(&contract.name),
        json_string(&format!("{candidate:?}")),
        json_string(&output.display().to_string()),
        input_json,
        result_json,
    );
    fs::write(path, contents).map_err(|error| format!("cannot write {}: {error}", path.display()))
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
    })
}

struct RunArgs<'a> {
    input: &'a str,
    values: Vec<String>,
    output: PathBuf,
    report: Option<PathBuf>,
    timeout: Option<Duration>,
    max_candidates: usize,
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
            default_output_path("examples/mul8.ax"),
            PathBuf::from("examples/mul8.wasm")
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
        ];

        let parsed = parse_build_args(&args).unwrap();
        assert_eq!(parsed.report, Some(PathBuf::from("build.json")));
        assert_eq!(parsed.timeout, Some(Duration::from_millis(250)));
        assert_eq!(parsed.max_candidates, 12);
    }
}
