use std::collections::HashMap;
use std::fs;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, UNIX_EPOCH};

use axiom_ami::Expr;
use axiom_parser::{BinaryOp, PageElement, SpecExpr};
use axiom_project::{LinkedPage, Project};

#[derive(Debug, Clone)]
struct CompiledFunction {
    name: String,
    source: PathBuf,
    params: u32,
    candidate: Expr,
}

#[derive(Debug, Clone)]
pub struct BuildResult {
    pub files: Vec<PathBuf>,
}

#[derive(Debug, Default)]
struct DevState {
    version: u64,
    error: Option<String>,
}

pub fn build(config_path: &Path, output_dir: &Path) -> Result<BuildResult, String> {
    let project = axiom_project::load(config_path)?;
    let compiled = compile_contracts(&project)?;
    if compiled.is_empty() {
        return Err("web project must contain at least one contract".to_string());
    }

    let functions = compiled
        .iter()
        .map(|function| (function.name.as_str(), &function.candidate, function.params))
        .collect::<Vec<_>>();
    let wasm = axiom_wasm::emit_functions(&functions)?;

    fs::create_dir_all(output_dir)
        .map_err(|error| format!("cannot create {}: {error}", output_dir.display()))?;
    write_file(&output_dir.join("app.wasm"), &wasm)?;
    write_file(
        &output_dir.join("index.html"),
        page_html(&project).as_bytes(),
    )?;
    write_file(
        &output_dir.join("app.js"),
        page_javascript(&project).as_bytes(),
    )?;
    write_file(&output_dir.join("app.css"), DEFAULT_CSS.as_bytes())?;
    write_file(
        &output_dir.join("app.proof.json"),
        proof_json(&project, &compiled).as_bytes(),
    )?;

    println!("target: web");
    println!("verified: {} contract(s)", compiled.len());
    println!("wrote: {}", output_dir.display());
    Ok(BuildResult {
        files: project.files,
    })
}

pub fn check(config_path: &Path) -> Result<(), String> {
    let project = axiom_project::load(config_path)?;
    let compiled = compile_contracts(&project)?;
    println!("project: {}", config_path.display());
    println!("target: {}", project.config.target);
    println!("files: {}", project.files.len());
    println!("contracts: {}", compiled.len());
    println!("pages: {}", project.pages.len());
    println!("verified: true");
    Ok(())
}

pub fn dev(config_path: &Path, port: u16, open_browser: bool, once: bool) -> Result<(), String> {
    let root = config_path
        .parent()
        .ok_or_else(|| "cannot determine project root".to_string())?;
    let output_dir = root.join(".synthetix").join("dev");
    let initial = build(config_path, &output_dir)?;
    if once {
        return Ok(());
    }

    let listener = TcpListener::bind(("127.0.0.1", port))
        .map_err(|error| format!("cannot start localhost server on port {port}: {error}"))?;
    let address = format!("http://127.0.0.1:{port}/");
    println!("dev server: {address}");
    println!("watching: {} file(s)", initial.files.len());

    let state = Arc::new(Mutex::new(DevState {
        version: 1,
        error: None,
    }));
    let watcher_state = Arc::clone(&state);
    let watcher_config = config_path.to_path_buf();
    let watcher_output = output_dir.clone();
    let mut watched_files = initial.files;
    watched_files.push(config_path.to_path_buf());
    thread::spawn(move || {
        watch_project(watcher_config, watcher_output, watcher_state, watched_files)
    });

    if open_browser {
        open_url(&address);
    }

    for stream in listener.incoming() {
        match stream {
            Ok(stream) => handle_request(stream, &output_dir, &state),
            Err(error) => eprintln!("dev server connection error: {error}"),
        }
    }
    Ok(())
}

fn compile_contracts(project: &Project) -> Result<Vec<CompiledFunction>, String> {
    let mut names = HashMap::new();
    let mut compiled = Vec::new();
    for linked in &project.contracts {
        let name = linked.contract.name.clone();
        if let Some(previous) = names.insert(name.clone(), linked.source.clone()) {
            return Err(format!(
                "duplicate web function {name}: {} and {}",
                previous.display(),
                linked.source.display()
            ));
        }
        let candidate = axiom_synth::synthesize(&linked.contract)
            .map_err(|error| format!("{}:{}: {error}", linked.source.display(), name))?;
        println!("verified: {name}");
        compiled.push(CompiledFunction {
            name,
            source: linked.source.clone(),
            params: linked.contract.params.len() as u32,
            candidate,
        });
    }
    Ok(compiled)
}

fn page_html(project: &Project) -> String {
    let title = project
        .pages
        .first()
        .map(|page| page.page.name.as_str())
        .unwrap_or("Synthetix Web App");
    format!(
        "<!doctype html>\n<html lang=\"ko\">\n<head>\n  <meta charset=\"utf-8\">\n  <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n  <title>{}</title>\n  <link rel=\"stylesheet\" href=\"./app.css\">\n</head>\n<body>\n  <main id=\"app\"><p>Loading…</p></main>\n  <div id=\"synthetix-error\" hidden></div>\n  <script type=\"module\" src=\"./app.js\"></script>\n</body>\n</html>\n",
        escape_html(title)
    )
}

fn page_javascript(project: &Project) -> String {
    let page = project
        .pages
        .first()
        .map(page_json)
        .unwrap_or_else(|| generic_page_json(project));
    format!(
        "const __page = {page};\n\nlet __exports = null;\nlet __state = {{}};\n\nfunction evaluate(expression, scope = __state) {{\n  switch (expression.kind) {{\n    case 'number': return expression.value >>> 0;\n    case 'bool': return expression.value;\n    case 'ident': return scope[expression.name];\n    case 'let': {{\n      const nested = {{ ...scope, [expression.name]: evaluate(expression.value, scope) }};\n      return evaluate(expression.body, nested);\n    }}\n    case 'if': return evaluate(expression.condition, scope)\n      ? evaluate(expression.then, scope)\n      : evaluate(expression.else, scope);\n    case 'binary': return binary(expression.op, evaluate(expression.left, scope), evaluate(expression.right, scope));\n    default: throw new Error(`Unknown expression kind: ${{expression.kind}}`);\n  }}\n}}\n\nfunction binary(op, left, right) {{\n  switch (op) {{\n    case 'add': return (left + right) >>> 0;\n    case 'mul': return Math.imul(left, right) >>> 0;\n    case 'shl': return (left << (right & 31)) >>> 0;\n    case 'eq': return left === right;\n    case 'ne': return left !== right;\n    case 'lt': return left < right;\n    case 'le': return left <= right;\n    case 'gt': return left > right;\n    case 'ge': return left >= right;\n    case 'and': return left && right;\n    case 'or': return left || right;\n    default: throw new Error(`Unknown operator: ${{op}}`);\n  }}\n}}\n\nfunction render() {{\n  const root = document.querySelector('#app');\n  root.replaceChildren();\n  const title = document.createElement('h1');\n  title.textContent = __page.name;\n  root.append(title);\n  for (const element of __page.elements) {{\n    if (element.kind === 'show') {{\n      const output = document.createElement('output');\n      output.dataset.state = element.state;\n      output.textContent = String(__state[element.state]);\n      root.append(output);\n      continue;\n    }}\n    const button = document.createElement('button');\n    button.type = 'button';\n    button.textContent = element.label;\n    button.addEventListener('click', () => {{\n      const fn = __exports[element.function];\n      if (typeof fn !== 'function') throw new Error(`Missing WASM export: ${{element.function}}`);\n      const args = element.args.map((arg) => evaluate(arg));\n      __state[element.state] = fn(...args);\n      render();\n    }});\n    root.append(button);\n  }}\n}}\n\nasync function boot() {{\n  const bytes = await fetch('./app.wasm').then((response) => response.arrayBuffer());\n  const {{ instance }} = await WebAssembly.instantiate(bytes, {{}});\n  __exports = instance.exports;\n  for (const state of __page.states) __state[state.name] = evaluate(state.initial);\n  window.synthetix = {{ exports: __exports, state: __state }};\n  render();\n}}\n\nboot().catch((error) => {{\n  const node = document.querySelector('#synthetix-error');\n  node.hidden = false;\n  node.textContent = error.message;\n}});\n\n// Local development uses polling instead of a WebSocket so the dev server stays dependency-free.\nlet __devVersion = null;\nsetInterval(async () => {{\n  try {{\n    const response = await fetch('/__synthetix/version', {{ cache: 'no-store' }});\n    if (!response.ok) return;\n    const next = await response.text();\n    if (__devVersion === null) __devVersion = next;\n    else if (next !== __devVersion) location.reload();\n    const errorResponse = await fetch('/__synthetix/error', {{ cache: 'no-store' }});\n    const errorNode = document.querySelector('#synthetix-error');\n    const error = await errorResponse.text();\n    errorNode.hidden = !error;\n    if (error) errorNode.textContent = error;\n  }} catch (_) {{}}\n}}, 500);\n",
        page = page
    )
}

fn page_json(linked: &LinkedPage) -> String {
    let page = &linked.page;
    let states = page
        .states
        .iter()
        .map(|state| {
            format!(
                "{{\"name\":{},\"initial\":{}}}",
                json_string(&state.name),
                expr_json(&state.initial).unwrap_or_else(|error| {
                    format!("{{\"kind\":\"error\",\"message\":{}}}", json_string(&error))
                })
            )
        })
        .collect::<Vec<_>>();
    let elements = page
        .elements
        .iter()
        .map(|element| match element {
            PageElement::Show { state } => {
                format!("{{\"kind\":\"show\",\"state\":{}}}", json_string(state))
            }
            PageElement::Button {
                label,
                state,
                function,
                args,
            } => format!(
                "{{\"kind\":\"button\",\"label\":{},\"state\":{},\"function\":{},\"args\":[{}]}}",
                json_string(label),
                json_string(state),
                json_string(function),
                args.iter()
                    .map(|arg| expr_json(arg).unwrap_or_else(|error| {
                        format!("{{\"kind\":\"error\",\"message\":{}}}", json_string(&error))
                    }))
                    .collect::<Vec<_>>()
                    .join(",")
            ),
        })
        .collect::<Vec<_>>();
    format!(
        "{{\"name\":{},\"states\":[{}],\"elements\":[{}]}}",
        json_string(&page.name),
        states.join(","),
        elements.join(",")
    )
}

fn generic_page_json(project: &Project) -> String {
    let contract = &project.contracts[0].contract;
    let mut states = contract
        .params
        .iter()
        .map(|param| {
            format!(
                "{{\"name\":{},\"initial\":{{\"kind\":\"number\",\"value\":0}}}}",
                json_string(&param.name)
            )
        })
        .collect::<Vec<_>>();
    states.push("{\"name\":\"result\",\"initial\":{\"kind\":\"number\",\"value\":0}}".to_string());
    let args = contract
        .params
        .iter()
        .map(|param| {
            format!(
                "{{\"kind\":\"ident\",\"name\":{}}}",
                json_string(&param.name)
            )
        })
        .collect::<Vec<_>>();
    format!(
        "{{\"name\":{},\"states\":[{}],\"elements\":[{{\"kind\":\"show\",\"state\":\"result\"}},{{\"kind\":\"button\",\"label\":\"실행\",\"state\":\"result\",\"function\":{},\"args\":[{}]}}]}}",
        json_string(&contract.name),
        states.join(","),
        json_string(&contract.name),
        args.join(",")
    )
}

fn expr_json(expr: &SpecExpr) -> Result<String, String> {
    match expr {
        SpecExpr::Ident(name) => Ok(format!(
            "{{\"kind\":\"ident\",\"name\":{}}}",
            json_string(name)
        )),
        SpecExpr::Number(value) => Ok(format!("{{\"kind\":\"number\",\"value\":{value}}}")),
        SpecExpr::Bool(value) => Ok(format!("{{\"kind\":\"bool\",\"value\":{value}}}")),
        SpecExpr::Let(name, value, body) => Ok(format!(
            "{{\"kind\":\"let\",\"name\":{},\"value\":{},\"body\":{}}}",
            json_string(name),
            expr_json(value)?,
            expr_json(body)?
        )),
        SpecExpr::If(condition, then_branch, else_branch) => Ok(format!(
            "{{\"kind\":\"if\",\"condition\":{},\"then\":{},\"else\":{}}}",
            expr_json(condition)?,
            expr_json(then_branch)?,
            expr_json(else_branch)?
        )),
        SpecExpr::Binary(op, lhs, rhs) => Ok(format!(
            "{{\"kind\":\"binary\",\"op\":{},\"left\":{},\"right\":{}}}",
            json_string(binary_name(op)),
            expr_json(lhs)?,
            expr_json(rhs)?
        )),
        SpecExpr::Call(name, _) => Err(format!("page expressions cannot call contract {name}")),
    }
}

fn binary_name(op: &BinaryOp) -> &'static str {
    match op {
        BinaryOp::AddWrap => "add",
        BinaryOp::MulWrap => "mul",
        BinaryOp::Shl => "shl",
        BinaryOp::Eq => "eq",
        BinaryOp::Ne => "ne",
        BinaryOp::Lt => "lt",
        BinaryOp::Le => "le",
        BinaryOp::Gt => "gt",
        BinaryOp::Ge => "ge",
        BinaryOp::And => "and",
        BinaryOp::Or => "or",
    }
}

fn proof_json(project: &Project, compiled: &[CompiledFunction]) -> String {
    let contracts = compiled
        .iter()
        .map(|function| {
            let source = function
                .source
                .strip_prefix(&project.config.root)
                .unwrap_or(&function.source)
                .display()
                .to_string();
            format!(
                "{{\"name\":{},\"source\":{},\"verified\":true,\"candidate\":{}}}",
                json_string(&function.name),
                json_string(&source),
                json_string(&format!("{:?}", function.candidate))
            )
        })
        .collect::<Vec<_>>();
    format!(
        "{{\n  \"version\": 2,\n  \"target\": \"web\",\n  \"verified\": true,\n  \"contracts\": [{}]\n}}\n",
        contracts.join(",")
    )
}

fn write_file(path: &Path, contents: &[u8]) -> Result<(), String> {
    fs::write(path, contents).map_err(|error| format!("cannot write {}: {error}", path.display()))
}

fn watch_project(
    config_path: PathBuf,
    output_dir: PathBuf,
    state: Arc<Mutex<DevState>>,
    mut watched_files: Vec<PathBuf>,
) {
    let mut snapshot = file_snapshot_for(&watched_files);
    loop {
        thread::sleep(Duration::from_millis(300));
        if let Ok(project) = axiom_project::load(&config_path) {
            watched_files = project.files;
            watched_files.push(config_path.clone());
        }
        let current = file_snapshot_for(&watched_files);
        if current == snapshot {
            continue;
        }
        snapshot = current;
        let mut guard = state.lock().expect("dev state lock poisoned");
        match build(&config_path, &output_dir) {
            Ok(_) => {
                guard.version = guard.version.saturating_add(1);
                guard.error = None;
                println!("rebuilt successfully (version {})", guard.version);
            }
            Err(error) => {
                guard.error = Some(error.clone());
                eprintln!("build error: {error}");
            }
        }
    }
}

fn file_snapshot_for(files: &[PathBuf]) -> HashMap<PathBuf, (u64, u64)> {
    files
        .iter()
        .filter_map(|path| {
            let metadata = fs::metadata(path).ok()?;
            let modified = metadata
                .modified()
                .ok()?
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos() as u64;
            Some((path.clone(), (modified, metadata.len())))
        })
        .collect()
}

fn handle_request(mut stream: TcpStream, output_dir: &Path, state: &Arc<Mutex<DevState>>) {
    let mut request = [0_u8; 8192];
    let Ok(size) = stream.read(&mut request) else {
        return;
    };
    let request = String::from_utf8_lossy(&request[..size]);
    let Some(path) = request
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
    else {
        return;
    };

    if path == "/__synthetix/version" {
        let version = state.lock().map(|state| state.version).unwrap_or_default();
        respond(
            &mut stream,
            "text/plain; charset=utf-8",
            &version.to_string(),
        );
        return;
    }
    if path == "/__synthetix/error" {
        let error = state
            .lock()
            .ok()
            .and_then(|state| state.error.clone())
            .unwrap_or_default();
        respond(&mut stream, "text/plain; charset=utf-8", &error);
        return;
    }

    let relative = path.trim_start_matches('/');
    let relative = if relative.is_empty() {
        "index.html"
    } else {
        relative
    };
    if relative.contains("..") {
        respond_status(&mut stream, 400, "Bad Request");
        return;
    }
    let file = output_dir.join(relative);
    match fs::read(&file) {
        Ok(contents) => respond_bytes(&mut stream, content_type(&file), &contents),
        Err(_) => respond_status(&mut stream, 404, "Not Found"),
    }
}

fn respond(stream: &mut TcpStream, content_type: &str, contents: &str) {
    respond_bytes(stream, content_type, contents.as_bytes());
}

fn respond_bytes(stream: &mut TcpStream, content_type: &str, contents: &[u8]) {
    let header = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
        contents.len()
    );
    let _ = stream.write_all(header.as_bytes());
    let _ = stream.write_all(contents);
}

fn respond_status(stream: &mut TcpStream, status: u16, message: &str) {
    let body = message.as_bytes();
    let header = format!(
        "HTTP/1.1 {status} {message}\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(header.as_bytes());
    let _ = stream.write_all(body);
}

fn content_type(path: &Path) -> &'static str {
    match path.extension().and_then(|extension| extension.to_str()) {
        Some("html") => "text/html; charset=utf-8",
        Some("js") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("wasm") => "application/wasm",
        Some("json") => "application/json; charset=utf-8",
        _ => "application/octet-stream",
    }
}

fn open_url(url: &str) {
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open").arg(url).spawn();
    #[cfg(target_os = "windows")]
    let _ = std::process::Command::new("cmd")
        .args(["/C", "start", "", url])
        .spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let _ = std::process::Command::new("xdg-open").arg(url).spawn();
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
                result.push_str(&format!("\\u{:04x}", character as u32));
            }
            character => result.push(character),
        }
    }
    result.push('"');
    result
}

fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

const DEFAULT_CSS: &str = r#":root { color-scheme: dark; font-family: system-ui, sans-serif; }
body { margin: 0; min-height: 100vh; display: grid; place-items: center; background: #111827; color: #f9fafb; }
main { min-width: 18rem; padding: 2rem; display: grid; gap: 1rem; border: 1px solid #374151; border-radius: 1rem; background: #1f2937; }
h1 { margin: 0; font-size: 1.5rem; }
output { font-size: 3rem; font-variant-numeric: tabular-nums; }
button { border: 0; border-radius: .5rem; padding: .7rem 1rem; color: white; background: #2563eb; cursor: pointer; }
button:hover { background: #1d4ed8; }
#synthetix-error { position: fixed; inset: 1rem; padding: 1rem; white-space: pre-wrap; color: #fecaca; background: #450a0a; border: 1px solid #ef4444; border-radius: .5rem; }
"#;
