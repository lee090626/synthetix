use std::collections::HashMap;
use std::fs;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use axiom_ami::Expr;
use axiom_parser::{
    BinaryOp, DeclarationKind, PageElement, Reducer, SpecExpr, Type, VERIFIED_SEMANTICS_VERSION,
};
use axiom_project::{ContractId, LinkedPage, Project};

#[derive(Debug, Clone)]
struct CompiledFunction {
    name: String,
    source: PathBuf,
    params: u32,
    candidate: Expr,
    requires: Option<Expr>,
    requires_export: Option<String>,
}

#[derive(Debug, Clone)]
pub struct BuildResult {
    pub files: Vec<PathBuf>,
    pub verification_cache: VerificationCacheStats,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct VerificationCacheStats {
    pub hits: usize,
    pub misses: usize,
    pub invalidations: usize,
    pub elapsed_micros: u128,
}

#[derive(Debug, Default)]
struct VerificationCache {
    entries: HashMap<ContractId, CachedVerifiedCandidate>,
    stats: VerificationCacheStats,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ProofCacheKey {
    id: ContractId,
    source: Arc<[u8]>,
    semantics_version: String,
    synthesis_options: ProofCacheOptions,
    dependencies: Vec<(ContractId, Arc<ProofCacheKey>)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ProofCacheOptions {
    timeout: Option<Duration>,
    max_candidates: usize,
    max_memory_bytes: usize,
}

#[derive(Debug, Clone)]
struct CachedVerifiedCandidate {
    key: Arc<ProofCacheKey>,
    candidate: Expr,
}

#[derive(Debug, Default)]
struct DevState {
    version: u64,
    error: Option<String>,
}

pub fn build(config_path: &Path, output_dir: &Path) -> Result<BuildResult, String> {
    let mut cache = VerificationCache::default();
    build_with_cache(config_path, output_dir, &mut cache, false)
}

fn build_with_cache(
    config_path: &Path,
    output_dir: &Path,
    cache: &mut VerificationCache,
    dev_runtime: bool,
) -> Result<BuildResult, String> {
    let project = axiom_project::load(config_path)?;
    build_project(&project, output_dir, cache, dev_runtime)
}

fn build_project(
    project: &Project,
    output_dir: &Path,
    cache: &mut VerificationCache,
    dev_runtime: bool,
) -> Result<BuildResult, String> {
    let compiled = compile_contracts_with_cache(
        project,
        cache,
        axiom_synth::SynthesisOptions::default(),
        VERIFIED_SEMANTICS_VERSION,
    )?;
    if compiled.is_empty() {
        return Err("web project must contain at least one contract".to_string());
    }

    let mut exports = Vec::with_capacity(compiled.len() * 2);
    for function in &compiled {
        exports.push((function.name.clone(), &function.candidate, function.params));
        if let (Some(requires), Some(name)) = (&function.requires, &function.requires_export) {
            exports.push((name.clone(), requires, function.params));
        }
    }
    let functions = exports
        .iter()
        .map(|(name, expression, params)| (name.as_str(), *expression, *params))
        .collect::<Vec<_>>();
    let wasm = axiom_wasm::emit_functions(&functions)?;

    fs::create_dir_all(output_dir)
        .map_err(|error| format!("cannot create {}: {error}", output_dir.display()))?;
    write_file(&output_dir.join("app.wasm"), &wasm)?;
    write_file(
        &output_dir.join("index.html"),
        page_html(project).as_bytes(),
    )?;
    write_file(
        &output_dir.join("app.config.js"),
        b"window.synthetixConfig = window.synthetixConfig || {};\n",
    )?;
    write_file(
        &output_dir.join("app.js"),
        page_javascript(project, dev_runtime).as_bytes(),
    )?;
    write_file(&output_dir.join("app.css"), DEFAULT_CSS.as_bytes())?;
    write_file(
        &output_dir.join("app.proof.json"),
        proof_json(project, &compiled).as_bytes(),
    )?;

    println!("target: web");
    println!("contract_check: VALID");
    println!("synthesis_result: FOUND");
    println!("proof_result: VERIFIED");
    println!("verified: {} contract(s)", compiled.len());
    println!("wrote: {}", output_dir.display());
    Ok(BuildResult {
        files: project.files.clone(),
        verification_cache: cache.stats,
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
    println!("contract_check: VALID");
    println!("synthesis_result: FOUND");
    println!("proof_result: VERIFIED");
    println!("verified: true");
    Ok(())
}

pub fn dev(config_path: &Path, port: u16, open_browser: bool, once: bool) -> Result<(), String> {
    let root = config_path
        .parent()
        .ok_or_else(|| "cannot determine project root".to_string())?;
    let output_dir = root.join(".synthetix").join("dev");
    let mut cache = VerificationCache::default();
    let initial = build_with_cache(config_path, &output_dir, &mut cache, true)?;
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
        watch_project(
            watcher_config,
            watcher_output,
            watcher_state,
            watched_files,
            cache,
        )
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
    let mut cache = VerificationCache::default();
    compile_contracts_with_cache(
        project,
        &mut cache,
        axiom_synth::SynthesisOptions::default(),
        VERIFIED_SEMANTICS_VERSION,
    )
}

fn compile_contracts_with_cache(
    project: &Project,
    cache: &mut VerificationCache,
    options: axiom_synth::SynthesisOptions,
    semantics_version: &str,
) -> Result<Vec<CompiledFunction>, String> {
    cache.stats = VerificationCacheStats::default();
    let started = Instant::now();
    let result = compile_contracts_inner(project, cache, options, semantics_version);
    cache.stats.elapsed_micros = started.elapsed().as_micros();
    result
}

fn compile_contracts_inner(
    project: &Project,
    cache: &mut VerificationCache,
    options: axiom_synth::SynthesisOptions,
    semantics_version: &str,
) -> Result<Vec<CompiledFunction>, String> {
    let mut names = HashMap::new();
    let mut export_names = std::collections::HashSet::new();
    let mut extern_names = std::collections::HashSet::new();
    for linked in &project.verification_summaries {
        if linked.contract.kind == DeclarationKind::Extern
            && !extern_names.insert(linked.contract.name.clone())
        {
            return Err(format!("duplicate web extern {}", linked.contract.name));
        }
    }
    for linked in &project.contracts {
        if linked.contract.kind == DeclarationKind::Extern {
            continue;
        }
        let name = linked.contract.name.clone();
        if let Some(previous) = names.insert(name.clone(), linked.source.clone()) {
            return Err(format!(
                "duplicate web function {name}: {} and {}",
                previous.display(),
                linked.source.display()
            ));
        }
        if !export_names.insert(name.clone()) {
            return Err(format!("duplicate web export {name}"));
        }
    }

    let mut verified_summaries = HashMap::new();
    let mut verified_cache_keys = HashMap::new();
    let mut compiled_by_id = HashMap::new();
    let linked_by_id = project
        .contracts
        .iter()
        .map(|linked| (ContractId::from(linked), linked))
        .collect::<HashMap<_, _>>();
    for id in &project.verification_order {
        let linked = linked_by_id
            .get(id)
            .copied()
            .ok_or_else(|| format!("missing linked contract {}", id.name))?;
        if linked.contract.kind == DeclarationKind::Extern {
            let key = proof_cache_key(
                project,
                linked,
                options,
                semantics_version,
                &verified_cache_keys,
            )?;
            verified_cache_keys.insert(id.clone(), key);
            continue;
        }
        let name = linked.contract.name.clone();
        let key = proof_cache_key(
            project,
            linked,
            options,
            semantics_version,
            &verified_cache_keys,
        )?;
        let contract_id = ContractId::from(linked);
        let cached = cache
            .entries
            .get(&contract_id)
            .filter(|entry| entry.key == key)
            .cloned();
        let (candidate, cache_hit) = if let Some(entry) = cached {
            cache.stats.hits += 1;
            (entry.candidate, true)
        } else {
            cache.stats.misses += 1;
            if cache.entries.remove(&contract_id).is_some() {
                cache.stats.invalidations += 1;
            }
            let candidate = axiom_synth::synthesize_with_options(&linked.contract, options)
                .map_err(|error| format!("{}:{}: {error}", linked.source.display(), name))?;
            let composition = project
                .verify_compositional_with_verified_summaries(
                    linked,
                    &candidate,
                    &verified_summaries,
                )
                .map_err(|error| format!("{}:{}: {error}", linked.source.display(), name))?;
            match (composition.proof, composition.diagnostic) {
                (
                    Some(axiom_verify::ProofResult::Verified),
                    None | Some(axiom_verify::Diagnostic::AmbiguousCandidates),
                ) => {}
                (
                    None,
                    Some(axiom_verify::Diagnostic::CallPreconditionViolation { callee, span }),
                ) => {
                    let (line, column) =
                        span.map(|span| (span.line, span.column)).unwrap_or((1, 1));
                    return Err(format!(
                        "{}:{line}:{column}: call-site precondition for {callee} in {name} is not established",
                        linked.source.display(),
                    ));
                }
                (Some(axiom_verify::ProofResult::Counterexample(values)), _) => {
                    return Err(format!(
                        "{}:{}: COUNTEREXAMPLE: compositional postcondition fails for {values:?}",
                        linked.source.display(),
                        linked.contract.name
                    ));
                }
                (Some(axiom_verify::ProofResult::SolverUnknown), _) => {
                    return Err(format!(
                        "{}:{}: SOLVER_UNKNOWN: compositional verification returned unknown",
                        linked.source.display(),
                        linked.contract.name
                    ));
                }
                _ => {
                    return Err(format!(
                        "{}:{}: compositional verification did not produce a proof",
                        linked.source.display(),
                        linked.contract.name
                    ));
                }
            }
            cache.entries.insert(
                contract_id.clone(),
                CachedVerifiedCandidate {
                    key: key.clone(),
                    candidate: candidate.clone(),
                },
            );
            (candidate, false)
        };
        let requires = axiom_synth::lower_requires(&linked.contract)
            .map_err(|error| format!("{}:{}: {error}", linked.source.display(), name))?;
        let requires_export = requires.as_ref().map(|_| requires_export_name(&name));
        if let Some(requires_export) = &requires_export
            && !export_names.insert(requires_export.clone())
        {
            return Err(format!(
                "{}:{}: reserved or duplicate WASM export {requires_export}",
                linked.source.display(),
                name
            ));
        }
        verified_cache_keys.insert(contract_id.clone(), key);
        verified_summaries.insert(contract_id.clone(), linked.contract.clone());
        let (_, abi_arity) = axiom_parser::verified_abi_layout(&linked.contract.params)
            .map_err(|error| format!("{}:{}: {error}", linked.source.display(), name))?;
        let params = u32::try_from(abi_arity).map_err(|_| {
            format!(
                "{}:{}: WASM parameter count overflow",
                linked.source.display(),
                name
            )
        })?;
        if cache_hit {
            println!("verified: {name} (cache hit)");
        } else {
            println!("verified: {name}");
        }
        compiled_by_id.insert(
            contract_id,
            CompiledFunction {
                name,
                source: linked.source.clone(),
                params,
                candidate,
                requires,
                requires_export,
            },
        );
    }
    Ok(project
        .contracts
        .iter()
        .filter_map(|linked| compiled_by_id.remove(&ContractId::from(linked)))
        .collect())
}

fn proof_cache_key(
    project: &Project,
    linked: &axiom_project::LinkedContract,
    options: axiom_synth::SynthesisOptions,
    semantics_version: &str,
    verified_keys: &HashMap<ContractId, Arc<ProofCacheKey>>,
) -> Result<Arc<ProofCacheKey>, String> {
    let id = ContractId::from(linked);
    let source = project
        .source_contents
        .get(&linked.source)
        .cloned()
        .ok_or_else(|| {
            format!(
                "missing source snapshot for proof cache: {}",
                linked.source.display()
            )
        })?;
    let mut dependencies = project
        .verification_dependencies(linked)?
        .into_iter()
        .map(|(name, dependency)| {
            let key = verified_keys.get(&dependency).ok_or_else(|| {
                format!(
                    "missing verified cache identity for callee `{name}` in `{}`",
                    linked.contract.name
                )
            })?;
            Ok((dependency, Arc::clone(key)))
        })
        .collect::<Result<Vec<_>, String>>()?;
    dependencies.sort_by(|lhs, rhs| lhs.0.cmp(&rhs.0));
    Ok(Arc::new(ProofCacheKey {
        id,
        source,
        semantics_version: semantics_version.to_string(),
        synthesis_options: ProofCacheOptions {
            timeout: options.timeout,
            max_candidates: options.max_candidates,
            max_memory_bytes: options.max_memory_bytes,
        },
        dependencies,
    }))
}

fn page_html(project: &Project) -> String {
    let title = project
        .pages
        .first()
        .map(|page| page.page.name.as_str())
        .unwrap_or("Synthetix Web App");
    format!(
        "<!doctype html>\n<html lang=\"ko\">\n<head>\n  <meta charset=\"utf-8\">\n  <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n  <title>{}</title>\n  <link rel=\"stylesheet\" href=\"./app.css\">\n</head>\n<body>\n  <main id=\"app\"><p>Loading…</p></main>\n  <div id=\"synthetix-error\" hidden></div>\n  <script src=\"./app.config.js\"></script>\n  <script type=\"module\" src=\"./app.js\"></script>\n</body>\n</html>\n",
        escape_html(title)
    )
}

fn page_javascript(project: &Project, dev_runtime: bool) -> String {
    let page = project
        .pages
        .first()
        .map(|page| {
            page_json(
                page,
                &project.config.root,
                project.config.storage_schema_version,
            )
        })
        .unwrap_or_else(|| generic_page_json(project));
    let contracts = contracts_json(project);
    let externs = externs_json(project);
    let mut script = r#"const __page = __PAGE_JSON__;
const __contracts = __CONTRACTS__;
const __externs = __EXTERNS__;

let __exports = null;
let __state = {};
let __lastError = null;
let __lastTransition = null;
let __lastEffect = null;
let __effectAdapter = null;
let __nextEffectId = 0;
let __latestEffectId = 0;
const __effectRuns = new Map();
const __effectRetries = new Map();
let __storageMigration = typeof window.synthetixStorageMigration === 'function'
  ? window.synthetixStorageMigration
  : null;
const __storagePrefix = `synthetix:${__page.name}:`;
const __storageKey = `${__storagePrefix}state`;

function getPath(scope, path) {
  return path.split('.').reduce((value, part) => value?.[part], scope);
}

function setPath(scope, path, value) {
  const parts = path.split('.');
  if (parts.length === 1) {
    scope[parts[0]] = value;
    return;
  }
  const owner = getPath(scope, parts.slice(0, -1).join('.'));
  if (!owner) throw new Error(`Missing state path: ${path}`);
  owner[parts.at(-1)] = value;
}

function evaluate(expression, scope = __state) {
  switch (expression.kind) {
    case 'number': return expression.value >>> 0;
    case 'i32': return expression.value | 0;
    case 'bool': return expression.value;
    case 'text': return expression.value;
    case 'ident': return scope[expression.name];
    case 'field': return evaluate(expression.object, scope)?.[expression.name];
    case 'list': return expression.values.map((value) => evaluate(value, scope));
    case 'record': return Object.fromEntries(
      Object.entries(expression.fields).map(([name, value]) => [name, evaluate(value, scope)]),
    );
    case 'let': {
      const nested = { ...scope, [expression.name]: evaluate(expression.value, scope) };
      return evaluate(expression.body, nested);
    }
    case 'if': return evaluate(expression.condition, scope)
      ? evaluate(expression.then, scope)
      : evaluate(expression.else, scope);
    case 'binary': return binary(expression.op, evaluate(expression.left, scope), evaluate(expression.right, scope));
    default: throw new Error(`Unknown expression kind: ${expression.kind}`);
  }
}

function binary(op, left, right) {
  switch (op) {
    case 'add': return (left + right) >>> 0;
    case 'mul': return Math.imul(left, right) >>> 0;
    case 'shl': return (left << (right & 31)) >>> 0;
    case 'eq': return left === right;
    case 'ne': return left !== right;
    case 'lt': return left < right;
    case 'le': return left <= right;
    case 'gt': return left > right;
    case 'ge': return left >= right;
    case 'and': return left && right;
    case 'or': return left || right;
    default: throw new Error(`Unknown operator: ${op}`);
  }
}

function display(value) {
  if (value === undefined) return '';
  if (typeof value === 'boolean') return value ? 'true' : 'false';
  if (typeof value === 'object') return JSON.stringify(value);
  return String(value);
}

function applicationError(error, functionName = null, sourceOverride = null) {
  if (error?.category === 'APPLICATION_RUNTIME_ERROR' || error?.category === 'APPLICATION_TRANSITION_FAILURE') return error;
  const contract = functionName ? __contracts[functionName] : null;
  const source = sourceOverride ?? contract?.source ?? __page.source;
  const message = error instanceof Error ? error.message : String(error);
  const location = functionName ? `${source}:${functionName}` : source;
  const wrapped = new Error(`${location}: application runtime error: ${message}`);
  wrapped.category = 'APPLICATION_RUNTIME_ERROR';
  wrapped.source = source;
  wrapped.contract = functionName;
  return wrapped;
}

function reportApplicationError(error, functionName = null, sourceOverride = null) {
  __lastError = applicationError(error, functionName, sourceOverride);
  const node = document.querySelector('#synthetix-error');
  node.hidden = false;
  node.dataset.category = __lastError.category;
  node.dataset.source = __lastError.source;
  node.textContent = __lastError.message;
  return __lastError;
}

function clearApplicationError() {
  __lastError = null;
  const node = document.querySelector('#synthetix-error');
  node.hidden = true;
  node.textContent = '';
  delete node.dataset.category;
  delete node.dataset.source;
}

function runApplicationAction(action) {
  try {
    const result = action();
    clearApplicationError();
    return result;
  } catch (error) {
    return reportApplicationError(error);
  }
}

function lowerArgument(value, type, index) {
  if (type === 'bool') {
    if (typeof value !== 'boolean') throw new TypeError(`argument ${index}: expected bool`);
    return value ? 1 : 0;
  }
  if (type === 'u32') {
    if (typeof value !== 'number' || !Number.isInteger(value) || value < 0 || value > 0xffffffff)
      throw new TypeError(`argument ${index}: expected u32`);
    return value;
  }
  if (type === 'i32') {
    if (typeof value !== 'number' || !Number.isInteger(value) || value < -0x80000000 || value > 0x7fffffff)
      throw new TypeError(`argument ${index}: expected i32`);
    return value;
  }
  throw new TypeError(`argument ${index}: unsupported verified type ${type}`);
}

function liftResult(value, type) {
  if (type === 'bool') {
    if (value !== 0 && value !== 1) throw new TypeError(`verified WASM returned invalid bool ${value}`);
    return value === 1;
  }
  if (type === 'u32') return value >>> 0;
  if (type === 'i32') return value | 0;
  throw new TypeError(`unsupported verified result type ${type}`);
}

function isApplicationValue(value, seen = new Set()) {
  if (value === null || typeof value === 'string' || typeof value === 'boolean') return true;
  if (typeof value === 'number') return Number.isSafeInteger(value);
  if (typeof value !== 'object' || seen.has(value)) return false;
  if (!Array.isArray(value)) {
    const prototype = Object.getPrototypeOf(value);
    if (Object.prototype.toString.call(value) !== '[object Object]'
      || (prototype !== null && prototype.constructor?.name !== 'Object')) return false;
  }
  seen.add(value);
  const children = Array.isArray(value) ? value : Object.values(value);
  for (const child of children) {
    if (!isApplicationValue(child, seen)) return false;
  }
  seen.delete(value);
  return true;
}

function validateEffectResponse(value, type = null) {
  if (type === 'bool' && typeof value === 'boolean') return value;
  if (type === 'u32' && typeof value === 'number' && Number.isInteger(value) && value >= 0 && value <= 0xffffffff) return value;
  if (type === 'i32' && typeof value === 'number' && Number.isInteger(value) && value >= -0x80000000 && value <= 0x7fffffff) return value;
  if (type !== null && type !== undefined) throw new TypeError(`external response does not match ${type}`);
  if (!isApplicationValue(value)) throw new TypeError('external response is not a supported application value');
  return value;
}

function callContract(name, args) {
  try {
    const contract = __contracts[name];
    if (!contract) throw new Error(`Missing verified contract: ${name}`);
    if (!Array.isArray(args) || args.length !== contract.params.length)
      throw new TypeError(`${name}: expected ${contract.params.length} arguments`);
    const fn = __exports?.[name];
    if (typeof fn !== 'function') throw new Error(`Missing WASM export: ${name}`);
    const lowered = args.map((value, index) => lowerArgument(value, contract.params[index], index));
    if (contract.requires) {
      const check = __exports?.[contract.requires];
      if (typeof check !== 'function') throw new Error(`Missing WASM precondition check: ${name}`);
      if (check(...lowered) !== 1) throw new Error(`${name}: input violates a requires clause`);
    }
    return liftResult(fn(...lowered), contract.result);
  } catch (error) {
    throw applicationError(error, name);
  }
}

function parseInputValue(input, type) {
  if (type === 'text') return input.value;
  if (type === 'bool') return input.checked;
  if (type !== 'u32' && type !== 'i32') throw new TypeError(`input does not support ${type}`);
  if (input.value.trim() === '') throw new TypeError(`expected ${type} input`);
  const value = Number(input.value);
  const minimum = type === 'i32' ? -0x80000000 : 0;
  const maximum = type === 'i32' ? 0x7fffffff : 0xffffffff;
  if (!Number.isInteger(value) || value < minimum || value > maximum)
    throw new TypeError(`input is outside ${type}`);
  return value;
}

function storageAdapter() {
  const adapter = window.synthetixStorageAdapter ?? localStorage;
  if (!adapter || typeof adapter.getItem !== 'function' || typeof adapter.setItem !== 'function')
    throw new TypeError('storage adapter must implement getItem and setItem');
  return adapter;
}

function validatePersistedValues(values) {
  if (!values || typeof values !== 'object' || Array.isArray(values))
    throw new TypeError('persisted state must be an object');
  for (const [name, value] of Object.entries(values)) {
    const state = __page.states.find((candidate) => candidate.name === name && candidate.persisted);
    if (!state) throw new TypeError(`unknown persisted state: ${name}`);
    const valid = state.type === 'bool' ? typeof value === 'boolean'
      : state.type === 'u32' ? Number.isInteger(value) && value >= 0 && value <= 0xffffffff
      : state.type === 'i32' ? Number.isInteger(value) && value >= -0x80000000 && value <= 0x7fffffff
      : state.type === 'text' ? typeof value === 'string'
      : state.type === 'list' ? Array.isArray(value)
      : state.type === 'record' ? value !== null && typeof value === 'object' && !Array.isArray(value)
      : false;
    if (!valid) throw new TypeError(`persisted state ${name} does not match ${state.type}`);
  }
  return values;
}

function saveState(state = __state) {
  const persisted = __page.states.filter((candidate) => candidate.persisted);
  if (persisted.length === 0) return;
  const values = Object.fromEntries(persisted.map((candidate) => [candidate.name, state[candidate.name]]));
  validatePersistedValues(values);
  storageAdapter().setItem(__storageKey, JSON.stringify({
    version: __page.storageSchemaVersion,
    values,
  }));
}

async function loadPersistedState(baseState = __state) {
  const persisted = __page.states.filter((candidate) => candidate.persisted);
  if (persisted.length === 0) return baseState;
  const adapter = storageAdapter();
  const stored = adapter.getItem(__storageKey);
  let values;
  let version;
  let legacy = false;
  if (stored === null) {
    values = {};
    for (const state of persisted) {
      const saved = adapter.getItem(__storagePrefix + state.name);
      if (saved !== null) {
        values[state.name] = JSON.parse(saved);
        legacy = true;
      }
    }
    if (!legacy) return baseState;
    version = 0;
  } else {
    const envelope = JSON.parse(stored);
    if (!envelope || typeof envelope !== 'object' || Array.isArray(envelope)
      || !Number.isSafeInteger(envelope.version) || envelope.version < 0
      || !Object.prototype.hasOwnProperty.call(envelope, 'values'))
      throw new TypeError('persisted state envelope is malformed');
    ({ version, values } = envelope);
  }

  let migrated = false;
  if (version !== __page.storageSchemaVersion) {
    if (legacy && version === 0 && !__storageMigration) {
      migrated = true;
    } else if (__storageMigration) {
      values = await __storageMigration({
        page: __page.name,
        fromVersion: version,
        toVersion: __page.storageSchemaVersion,
        values: structuredClone(values),
      });
      migrated = true;
    } else {
      throw new Error(`incompatible storage schema ${version}; expected ${__page.storageSchemaVersion}`);
    }
  }

  validatePersistedValues(values);
  const nextState = structuredClone(baseState);
  Object.assign(nextState, values);
  if (migrated) saveState(nextState);
  return nextState;
}

function callVerified(element, scope, target) {
  setPath(scope, target, callContract(element.function, element.args.map((arg) => evaluate(arg, scope))));
  saveState();
  render();
}

function applyReducerTransition(element) {
  const nextState = structuredClone(__state);
  let updateIndex = 0;
  let functionName = null;
  try {
    for (; updateIndex < element.updates.length; updateIndex++) {
      const update = element.updates[updateIndex];
      functionName = update.function;
      const value = callContract(update.function, update.args.map((arg) => evaluate(arg, nextState)));
      setPath(nextState, update.state, value);
    }
    functionName = null;
    saveState(nextState);
  } catch (error) {
    const cause = applicationError(error, functionName);
    const failure = new Error(`${cause.message}; transition ${element.event} failed at update ${updateIndex + 1}`);
    failure.category = 'APPLICATION_TRANSITION_FAILURE';
    failure.source = cause.source;
    failure.contract = cause.contract;
    failure.event = element.event;
    failure.failedUpdate = updateIndex;
    failure.cause = cause;
    __lastTransition = {
      event: element.event,
      status: 'FAILED',
      updatesApplied: 0,
      failedUpdate: updateIndex,
      state: structuredClone(__state),
      error: failure,
    };
    reportApplicationError(failure);
    return __lastTransition;
  }

  for (const key of Object.keys(__state)) delete __state[key];
  Object.assign(__state, nextState);
  __lastTransition = {
    event: element.event,
    status: 'APPLIED',
    updatesApplied: element.updates.length,
    state: structuredClone(__state),
  };
  try {
    render();
    clearApplicationError();
  } catch (error) {
    reportApplicationError(error);
  }
  return __lastTransition;
}

function renderElement(element, root, scope) {
  if (element.kind === 'show') {
    const output = document.createElement('output');
    output.dataset.state = element.state;
    output.textContent = display(getPath(scope, element.state));
    root.append(output);
    return;
  }
  if (element.kind === 'show-expr') {
    const output = document.createElement('output');
    output.textContent = display(evaluate(element.value, scope));
    root.append(output);
    return;
  }
  if (element.kind === 'text') {
    const text = document.createElement('p');
    text.textContent = element.text;
    root.append(text);
    return;
  }
  if (element.kind === 'when-show') {
    if (evaluate(element.condition, scope)) {
      const message = document.createElement('p');
      message.textContent = element.text;
      root.append(message);
    }
    return;
  }
  if (element.kind === 'input') {
    const state = __page.states.find((candidate) => candidate.name === element.state);
    const input = document.createElement('input');
    input.type = state?.type === 'bool' ? 'checkbox' : state?.type === 'u32' || state?.type === 'i32' ? 'number' : 'text';
    input.step = input.type === 'number' ? '1' : undefined;
    if (input.type === 'checkbox') input.checked = getPath(__state, element.state) ?? false;
    else input.value = getPath(__state, element.state) ?? '';
    input.placeholder = element.placeholder;
    input.addEventListener('change', () => {
      try {
        setPath(__state, element.state, parseInputValue(input, state?.type ?? 'unsupported'));
        saveState();
        clearApplicationError();
      } catch (error) {
        reportApplicationError(error);
      }
    });
    root.append(input);
    return;
  }
  if (element.kind === 'list') {
    const list = document.createElement('ul');
    for (const item of getPath(scope, element.state) ?? []) {
      const row = document.createElement('li');
      const rowScope = { ...scope, [element.item]: item };
      for (const child of element.elements) renderElement(child, row, rowScope);
      list.append(row);
    }
    root.append(list);
    return;
  }
  if (element.kind === 'button') {
    const button = document.createElement('button');
    button.type = 'button';
    button.textContent = element.label;
    button.addEventListener('click', () => {
      try { callVerified(element, scope, element.state); clearApplicationError(); }
      catch (error) { reportApplicationError(error, element.function); }
    });
    root.append(button);
    return;
  }
  if (element.kind === 'add') {
    const button = document.createElement('button');
    button.type = 'button';
    button.textContent = element.label;
    button.addEventListener('click', () => {
      try {
        const list = getPath(__state, element.state);
        if (!Array.isArray(list)) throw new Error(`State is not a list: ${element.state}`);
        list.push(evaluate(element.value, __state));
        saveState();
        render();
        clearApplicationError();
      } catch (error) { reportApplicationError(error); }
    });
    root.append(button);
    return;
  }
  if (element.kind === 'reducer') {
    const button = document.createElement('button');
    button.type = 'button';
    button.textContent = element.label;
    button.addEventListener('click', () => applyReducerTransition(element));
    root.append(button);
  }
}

function render() {
  const root = document.querySelector('#app');
  root.replaceChildren();
  const title = document.createElement('h1');
  title.textContent = __page.name;
  root.append(title);
  for (const element of __page.elements) renderElement(element, root, __state);
}

async function boot() {
  const runtimeConfig = window.synthetixConfig ?? {};
  if (typeof runtimeConfig !== 'object' || runtimeConfig === null
    || Array.isArray(runtimeConfig) || !isApplicationValue(runtimeConfig))
    throw new TypeError('deployment config must be a JSON object');
  const bytes = await fetch('./app.wasm').then((response) => response.arrayBuffer());
  const { instance } = await WebAssembly.instantiate(bytes, {});
  __exports = instance.exports;
  for (const state of __page.states) __state[state.name] = evaluate(state.initial);
  try { Object.assign(__state, await loadPersistedState(__state)); }
  catch (error) { reportApplicationError(error); }
  window.synthetix = {
    state: __state,
    config: Object.freeze({ ...runtimeConfig }),
    call: callContract,
    storageSchemaVersion: __page.storageSchemaVersion,
    registerStorageMigration(migration) {
      if (typeof migration !== 'function') throw reportApplicationError(new TypeError('storage migration must be a function'));
      __storageMigration = migration;
    },
    async restore() {
      try {
        const restored = await loadPersistedState(__state);
        for (const key of Object.keys(__state)) delete __state[key];
        Object.assign(__state, restored);
        render();
        clearApplicationError();
        return __state;
      } catch (error) {
        throw reportApplicationError(error);
      }
    },
    registerEffectAdapter(adapter) {
      if (typeof adapter !== 'function') throw reportApplicationError(new TypeError('effect adapter must be a function'));
      __effectAdapter = adapter;
    },
    cancelEffect(target = null) {
      let cancelled = false;
      for (const run of __effectRuns.values()) {
        if (run.status !== 'LOADING'
          || (target !== null && run.targetState !== target && run.name !== target)) continue;
        run.status = 'CANCELLED';
        run.controller.abort();
        if (__latestEffectId === run.id)
          __lastEffect = { name: run.name, status: 'CANCELLED', requestId: run.id, targetState: run.targetState };
        cancelled = true;
      }
      return cancelled;
    },
    retryEffect(target = null) {
      const latest = __lastEffect;
      const key = target === null
        ? latest?.targetState !== null && latest?.targetState !== undefined
          ? `state:${latest.targetState}`
          : `effect:${latest?.name}`
        : `state:${target}`;
      let retry = __effectRetries.get(key);
      if (!retry && target !== null)
        retry = [...__effectRetries.values()].find((candidate) => candidate.request.name === target);
      if (!retry) throw reportApplicationError(new Error('No failed effect request to retry'));
      return window.synthetix.requestEffect(retry.request, retry.targetState);
    },
    async requestExtern(name, args, targetState = null) {
      let requestId = null;
      try {
        const declaration = __externs[name];
        if (!declaration) throw new Error(`Missing extern declaration: ${name}`);
        if (!Array.isArray(args) || args.length !== declaration.params.length)
          throw new TypeError(`${name}: expected ${declaration.params.length} arguments`);
        args.forEach((value, index) => lowerArgument(value, declaration.params[index], index));
        if (targetState !== null) {
          const state = __page.states.find((candidate) => candidate.name === targetState);
          if (!state || state.type !== declaration.result)
            throw new TypeError(`${name}: target state must have type ${declaration.result}`);
        }
        const pending = window.synthetix.requestEffect({
          name,
          source: declaration.source,
          inputs: args,
          effect: declaration.effect,
          verificationStatus: declaration.verification_status,
          trustBoundary: declaration.trust_boundary,
          responseType: declaration.result,
        }, targetState);
        requestId = __lastEffect?.requestId ?? null;
        return await pending;
      } catch (error) {
        const failure = reportApplicationError(error, null, __externs[name]?.source);
        if (requestId === null) __lastEffect = { name, status: 'ERROR', error: failure };
        else if (__lastEffect?.requestId === requestId)
          __lastEffect = { ...__lastEffect, name, status: 'ERROR', error: failure };
        throw failure;
      }
    },
    async requestEffect(request, targetState = null) {
      const name = typeof request?.name === 'string' ? request.name : null;
      const key = targetState === null ? `effect:${name}` : `state:${targetState}`;
      const previous = __effectRuns.get(key);
      if (previous?.status === 'LOADING') {
        previous.status = 'STALE';
        previous.controller.abort();
      }
      const controller = new AbortController();
      const run = { id: ++__nextEffectId, key, name, targetState, controller, status: 'LOADING' };
      __effectRuns.set(key, run);
      __effectRetries.delete(key);
      __latestEffectId = run.id;
      __lastEffect = { name, status: 'LOADING', requestId: run.id, targetState };
      let retryRequest = null;
      try {
        if (!request || typeof request.name !== 'string' || !Array.isArray(request.inputs))
          throw new TypeError('effect request must contain a name and inputs');
        if (!request.inputs.every((value) => isApplicationValue(value)))
          throw new TypeError('effect request contains an unsupported input value');
        if (!__effectAdapter) throw new Error(`No effect adapter registered for ${request.name}`);
        retryRequest = { ...request, inputs: JSON.parse(JSON.stringify(request.inputs)) };
        const signal = controller.signal;
        let abortListener;
        const aborted = new Promise((_, reject) => {
          abortListener = () => {
            const error = new Error('effect request cancelled');
            error.name = 'AbortError';
            reject(error);
          };
          signal.addEventListener('abort', abortListener, { once: true });
          if (signal.aborted) abortListener();
        });
        let response;
        try {
          response = await Promise.race([
            Promise.resolve().then(() => __effectAdapter(request, { signal, requestId: run.id })),
            aborted,
          ]);
        } finally {
          signal.removeEventListener('abort', abortListener);
        }
        if (signal.aborted || __effectRuns.get(key) !== run) return undefined;
        const result = validateEffectResponse(response, request.responseType ?? null);
        if (targetState !== null) {
          setPath(__state, targetState, result);
          saveState();
          render();
        }
        run.status = 'SUCCEEDED';
        if (__latestEffectId === run.id) {
          __lastEffect = { name, status: 'SUCCEEDED', response: result, requestId: run.id, targetState };
          clearApplicationError();
        }
        return result;
      } catch (error) {
        if (__effectRuns.get(key) !== run) return undefined;
        if (controller.signal.aborted || error?.name === 'AbortError') {
          run.status = 'CANCELLED';
          if (__latestEffectId === run.id)
            __lastEffect = { name, status: 'CANCELLED', requestId: run.id, targetState };
          return undefined;
        }
        const failure = reportApplicationError(error, null, request?.source);
        run.status = 'ERROR';
        if (retryRequest) __effectRetries.set(key, { request: retryRequest, targetState });
        if (__latestEffectId === run.id)
          __lastEffect = { name, status: 'ERROR', error: failure, requestId: run.id, targetState };
        throw failure;
      }
    },
    get lastError() { return __lastError; },
    get lastTransition() { return __lastTransition; },
    get lastEffect() { return __lastEffect; },
  };
  render();
}

boot().catch((error) => {
  const node = document.querySelector('#synthetix-error');
  reportApplicationError(error);
});

"#
    .to_string();
    if dev_runtime {
        script.push_str(
            r#"
// Local development uses polling instead of a WebSocket so the dev server stays dependency-free.
let __devVersion = null;
setInterval(async () => {
  try {
    const response = await fetch('/__synthetix/version', { cache: 'no-store' });
    if (!response.ok) return;
    const next = await response.text();
    if (__devVersion === null) __devVersion = next;
    else if (next !== __devVersion) location.reload();
    const errorResponse = await fetch('/__synthetix/error', { cache: 'no-store' });
    const errorNode = document.querySelector('#synthetix-error');
    const error = await errorResponse.text();
    errorNode.hidden = !error;
    errorNode.textContent = error;
  } catch (_) {}
}, 500);
"#,
        );
    }
    script = script
        .replace("__PAGE_JSON__", &page)
        .replace("__CONTRACTS__", &contracts)
        .replace("__EXTERNS__", &externs);
    script
}

fn page_json(linked: &LinkedPage, root: &Path, storage_schema_version: u32) -> String {
    let page = &linked.page;
    let states = page
        .states
        .iter()
        .map(|state| {
            format!(
                "{{\"name\":{},\"type\":{},\"initial\":{},\"persisted\":{}}}",
                json_string(&state.name),
                json_string(state_type_name(&state.ty)),
                expr_json(&state.initial).unwrap_or_else(|error| {
                    format!("{{\"kind\":\"error\",\"message\":{}}}", json_string(&error))
                }),
                page.persisted.iter().any(|name| name == &state.name)
            )
        })
        .collect::<Vec<_>>();
    let elements = page
        .elements
        .iter()
        .map(|element| element_json(element, &page.reducers))
        .collect::<Vec<_>>();
    let reducers = page
        .reducers
        .iter()
        .map(|reducer| {
            let updates = reducer
                .updates
                .iter()
                .map(|update| {
                    format!(
                        "{{\"state\":{},\"function\":{},\"args\":[{}]}}",
                        json_string(&update.state),
                        json_string(&update.function),
                        update
                            .args
                            .iter()
                            .map(|arg| expr_json(arg).unwrap_or_else(|error| error_json(&error)))
                            .collect::<Vec<_>>()
                            .join(",")
                    )
                })
                .collect::<Vec<_>>();
            format!(
                "{{\"name\":{},\"updates\":[{}]}}",
                json_string(&reducer.name),
                updates.join(",")
            )
        })
        .collect::<Vec<_>>();
    format!(
        "{{\"name\":{},\"source\":{},\"storageSchemaVersion\":{},\"states\":[{}],\"reducers\":[{}],\"elements\":[{}]}}",
        json_string(&page.name),
        json_string(&source_label(root, &linked.source)),
        storage_schema_version,
        states.join(","),
        reducers.join(","),
        elements.join(",")
    )
}

fn element_json(element: &PageElement, reducers: &[Reducer]) -> String {
    match element {
        PageElement::Show { state } => {
            format!("{{\"kind\":\"show\",\"state\":{}}}", json_string(state))
        }
        PageElement::ShowExpr { value } => format!(
            "{{\"kind\":\"show-expr\",\"value\":{}}}",
            expr_json(value).unwrap_or_else(|error| error_json(&error))
        ),
        PageElement::ShowText { text } => {
            format!("{{\"kind\":\"text\",\"text\":{}}}", json_string(text))
        }
        PageElement::WhenShow { condition, text } => format!(
            "{{\"kind\":\"when-show\",\"condition\":{},\"text\":{}}}",
            expr_json(condition).unwrap_or_else(|error| error_json(&error)),
            json_string(text)
        ),
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
                .map(|arg| expr_json(arg).unwrap_or_else(|error| error_json(&error)))
                .collect::<Vec<_>>()
                .join(",")
        ),
        PageElement::RunReducer { label, reducer } => {
            let updates = reducers
                .iter()
                .find(|candidate| candidate.name == *reducer)
                .map(|candidate| {
                    candidate
                        .updates
                        .iter()
                        .map(|update| {
                            format!(
                                "{{\"state\":{},\"function\":{},\"args\":[{}]}}",
                                json_string(&update.state),
                                json_string(&update.function),
                                update
                                    .args
                                    .iter()
                                    .map(|arg| expr_json(arg)
                                        .unwrap_or_else(|error| error_json(&error)))
                                    .collect::<Vec<_>>()
                                    .join(",")
                            )
                        })
                        .collect::<Vec<_>>()
                        .join(",")
                })
                .unwrap_or_default();
            format!(
                "{{\"kind\":\"reducer\",\"label\":{},\"event\":{},\"updates\":[{}]}}",
                json_string(label),
                json_string(reducer),
                updates
            )
        }
        PageElement::Input { state, placeholder } => format!(
            "{{\"kind\":\"input\",\"state\":{},\"placeholder\":{}}}",
            json_string(state),
            json_string(placeholder)
        ),
        PageElement::List {
            state,
            item,
            elements,
        } => format!(
            "{{\"kind\":\"list\",\"state\":{},\"item\":{},\"elements\":[{}]}}",
            json_string(state),
            json_string(item),
            elements
                .iter()
                .map(|element| element_json(element, reducers))
                .collect::<Vec<_>>()
                .join(",")
        ),
        PageElement::Add {
            label,
            state,
            value,
        } => format!(
            "{{\"kind\":\"add\",\"label\":{},\"state\":{},\"value\":{}}}",
            json_string(label),
            json_string(state),
            expr_json(value).unwrap_or_else(|error| error_json(&error))
        ),
        PageElement::Use { name } => {
            format!("{{\"kind\":\"text\",\"text\":{}}}", json_string(name))
        }
    }
}

fn error_json(error: &str) -> String {
    format!("{{\"kind\":\"error\",\"message\":{}}}", json_string(error))
}

fn generic_page_json(project: &Project) -> String {
    let contract = &project.contracts[0].contract;
    let mut states = contract
        .params
        .iter()
        .map(|param| {
            format!(
                "{{\"name\":{},\"type\":{},\"initial\":{}}}",
                json_string(&param.name),
                json_string(state_type_name(&param.ty)),
                initial_json(&param.ty)
            )
        })
        .collect::<Vec<_>>();
    states.push(format!(
        "{{\"name\":\"result\",\"type\":{},\"initial\":{}}}",
        json_string(state_type_name(&contract.return_type)),
        initial_json(&contract.return_type)
    ));
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
    let mut elements = contract
        .params
        .iter()
        .map(|param| {
            format!(
                "{{\"kind\":\"input\",\"state\":{},\"placeholder\":{}}}",
                json_string(&param.name),
                json_string(&param.name)
            )
        })
        .collect::<Vec<_>>();
    elements.push("{\"kind\":\"show\",\"state\":\"result\"}".to_string());
    elements.push(format!(
        "{{\"kind\":\"button\",\"label\":\"실행\",\"state\":\"result\",\"function\":{},\"args\":[{}]}}",
        json_string(&contract.name),
        args.join(",")
    ));
    format!(
        "{{\"name\":{},\"source\":{},\"storageSchemaVersion\":{},\"states\":[{}],\"elements\":[{}]}}",
        json_string(&contract.name),
        json_string(&source_label(
            &project.config.root,
            &project.contracts[0].source
        )),
        project.config.storage_schema_version,
        states.join(","),
        elements.join(",")
    )
}

fn contracts_json(project: &Project) -> String {
    let contracts = project
        .contracts
        .iter()
        .filter(|linked| linked.contract.kind != DeclarationKind::Extern)
        .map(|linked| {
            let params = linked
                .contract
                .params
                .iter()
                .map(|param| json_string(state_type_name(&param.ty)))
                .collect::<Vec<_>>()
                .join(",");
            let requires = if linked.contract.requires.is_empty() {
                "null".to_string()
            } else {
                json_string(&requires_export_name(&linked.contract.name))
            };
            format!(
                "{}:{{\"source\":{},\"params\":[{}],\"result\":{},\"requires\":{}}}",
                json_string(&linked.contract.name),
                json_string(&source_label(&project.config.root, &linked.source)),
                params,
                json_string(state_type_name(&linked.contract.return_type)),
                requires
            )
        })
        .collect::<Vec<_>>()
        .join(",");
    format!("{{{contracts}}}")
}

fn externs_json(project: &Project) -> String {
    let externs = project
        .verification_summaries
        .iter()
        .filter(|linked| linked.contract.kind == DeclarationKind::Extern)
        .map(|linked| {
            let params = linked
                .contract
                .params
                .iter()
                .map(|parameter| json_string(state_type_name(&parameter.ty)))
                .collect::<Vec<_>>()
                .join(",");
            format!(
                "{}:{{\"source\":{},\"params\":[{}],\"result\":{},\"effect\":\"HOST_EFFECT\",\"verification_status\":\"UNVERIFIED\",\"implementation_verified\":false,\"trust_boundary\":\"EXTERNAL_IMPLEMENTATION\",\"response_validation\":\"DECLARED_TYPE_ONLY\",\"assumptions\":[\"a host adapter is registered when this effect is called\",\"the external behavior is suitable for the application; Axiom does not prove it\"],\"audit_metadata\":{{\"status\":\"NOT_PROVIDED\",\"scope\":\"external implementation\"}}}}",
                json_string(&linked.contract.name),
                json_string(&source_label(&project.config.root, &linked.source)),
                params,
                json_string(state_type_name(&linked.contract.return_type))
            )
        })
        .collect::<Vec<_>>()
        .join(",");
    format!("{{{externs}}}")
}

fn requires_export_name(contract: &str) -> String {
    format!("__axiom_requires_{contract}")
}

fn state_type_name(ty: &Type) -> &'static str {
    match ty {
        Type::U32 => "u32",
        Type::I32 => "i32",
        Type::Bool => "bool",
        Type::FixedArray(_, _) => "verified_array",
        Type::Text => "text",
        Type::Record(_) => "record",
        Type::List(_) => "list",
        Type::Unknown => "application",
    }
}

fn source_label(root: &Path, source: &Path) -> String {
    let root = fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let source = fs::canonicalize(source).unwrap_or_else(|_| source.to_path_buf());
    let relative = source.strip_prefix(root).unwrap_or(&source);
    relative.to_string_lossy().replace('\\', "/")
}

fn initial_json(ty: &axiom_parser::Type) -> &'static str {
    match ty {
        axiom_parser::Type::Bool => "{\"kind\":\"bool\",\"value\":false}",
        axiom_parser::Type::FixedArray(_, _) => "{\"kind\":\"null\"}",
        axiom_parser::Type::U32 => "{\"kind\":\"number\",\"value\":0}",
        axiom_parser::Type::I32 => "{\"kind\":\"i32\",\"value\":0}",
        axiom_parser::Type::Text => "{\"kind\":\"text\",\"value\":\"\"}",
        axiom_parser::Type::List(_) => "{\"kind\":\"list\",\"values\":[]}",
        axiom_parser::Type::Record(_) => "{\"kind\":\"record\",\"fields\":{}}",
        axiom_parser::Type::Unknown => "{\"kind\":\"null\"}",
    }
}

fn expr_json(expr: &SpecExpr) -> Result<String, String> {
    match expr {
        SpecExpr::Ident(name) => Ok(format!(
            "{{\"kind\":\"ident\",\"name\":{}}}",
            json_string(name)
        )),
        SpecExpr::Number(value) => Ok(format!("{{\"kind\":\"number\",\"value\":{value}}}")),
        SpecExpr::Bool(value) => Ok(format!("{{\"kind\":\"bool\",\"value\":{value}}}")),
        SpecExpr::Text(value) => Ok(format!(
            "{{\"kind\":\"text\",\"value\":{}}}",
            json_string(value)
        )),
        SpecExpr::List(values) => Ok(format!(
            "{{\"kind\":\"list\",\"values\":[{}]}}",
            values
                .iter()
                .map(expr_json)
                .collect::<Result<Vec<_>, _>>()?
                .join(",")
        )),
        SpecExpr::Record { name, fields } => Ok(format!(
            "{{\"kind\":\"record\",\"name\":{},\"fields\":{{{}}}}}",
            json_string(name),
            fields
                .iter()
                .map(|(field, value)| {
                    Ok(format!("{}:{}", json_string(field), expr_json(value)?))
                })
                .collect::<Result<Vec<_>, String>>()?
                .join(",")
        )),
        SpecExpr::Field(value, field) => Ok(format!(
            "{{\"kind\":\"field\",\"object\":{},\"name\":{}}}",
            expr_json(value)?,
            json_string(field)
        )),
        SpecExpr::Index(_, _) | SpecExpr::ArrayUpdate(_, _, _) | SpecExpr::For { .. } => {
            Err("verified fixed arrays are outside the application expression boundary".to_string())
        }
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
        BinaryOp::SubWrap => "sub",
        BinaryOp::MulWrap => "mul",
        BinaryOp::Shl => "shl",
        BinaryOp::BitAnd => "bit_and",
        BinaryOp::BitOr => "bit_or",
        BinaryOp::BitXor => "bit_xor",
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
            let source = source_label(&project.config.root, &function.source);
            format!(
                "{{\"name\":{},\"source\":{},\"contract_check\":\"VALID\",\"synthesis_result\":\"FOUND\",\"proof_result\":\"VERIFIED\",\"diagnostic\":null,\"verified\":true,\"candidate\":{}}}",
                json_string(&function.name),
                json_string(&source),
                json_string(&format!("{:?}", function.candidate))
            )
        })
        .collect::<Vec<_>>();
    let external_boundaries = externs_json(project);
    format!(
        "{{\n  \"version\": 4,\n  \"target\": \"web\",\n  \"verification\": {{\"contract_check\": \"VALID\", \"synthesis_result\": \"FOUND\", \"proof_result\": \"VERIFIED\", \"diagnostic\": null, \"verified\": true, \"scope\": \"WASM_CONTRACTS_ONLY\"}},\n  \"verification_scope\": {{\"contracts\": \"WASM_CONTRACTS_ONLY\", \"extern_implementations\": \"UNVERIFIED\", \"extern_response_validation\": \"DECLARED_TYPE_ONLY\"}},\n  \"contracts\": [{}],\n  \"external_boundaries\": {}\n}}\n",
        contracts.join(","),
        external_boundaries
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
    mut cache: VerificationCache,
) {
    let mut snapshot = file_snapshot_for(&watched_files);
    loop {
        thread::sleep(Duration::from_millis(300));
        rebuild_if_changed(
            &config_path,
            &output_dir,
            &state,
            &mut watched_files,
            &mut snapshot,
            &mut cache,
        );
    }
}

fn rebuild_if_changed(
    config_path: &Path,
    output_dir: &Path,
    state: &Arc<Mutex<DevState>>,
    watched_files: &mut Vec<PathBuf>,
    snapshot: &mut HashMap<PathBuf, Option<Vec<u8>>>,
    cache: &mut VerificationCache,
) -> bool {
    let current = file_snapshot_for(watched_files);
    if current == *snapshot {
        return false;
    }
    *snapshot = current;
    let project = match axiom_project::load(config_path) {
        Ok(project) => project,
        Err(error) => {
            let mut guard = state.lock().expect("dev state lock poisoned");
            guard.error = Some(error.clone());
            eprintln!("build error: {error}");
            return true;
        }
    };
    *watched_files = project.files.clone();
    watched_files.push(config_path.to_path_buf());
    let mut guard = state.lock().expect("dev state lock poisoned");
    match build_project(&project, output_dir, cache, true) {
        Ok(result) => {
            guard.version = guard.version.saturating_add(1);
            guard.error = None;
            println!("rebuilt successfully (version {})", guard.version);
            println!(
                "proof cache: {} hit(s), {} miss(es), {} invalidation(s), {}us",
                result.verification_cache.hits,
                result.verification_cache.misses,
                result.verification_cache.invalidations,
                result.verification_cache.elapsed_micros,
            );
        }
        Err(error) => {
            guard.error = Some(error.clone());
            eprintln!("build error: {error}");
        }
    }
    true
}

fn file_snapshot_for(files: &[PathBuf]) -> HashMap<PathBuf, Option<Vec<u8>>> {
    // ponytail: exact polling costs O(total source bytes); use native file events if that becomes measurable.
    files
        .iter()
        .map(|path| (path.clone(), fs::read(path).ok()))
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
input { min-width: 16rem; border: 1px solid #4b5563; border-radius: .5rem; padding: .7rem; color: #f9fafb; background: #111827; }
ul { margin: 0; padding: 0; display: grid; gap: .5rem; list-style: none; }
li { display: flex; align-items: center; gap: .5rem; padding: .5rem; border-radius: .5rem; background: #111827; }
p { margin: 0; color: #d1d5db; }
output { font-size: 3rem; font-variant-numeric: tabular-nums; }
button { border: 0; border-radius: .5rem; padding: .7rem 1rem; color: white; background: #2563eb; cursor: pointer; }
button:hover { background: #1d4ed8; }
#synthetix-error { position: fixed; inset: 1rem; padding: 1rem; white-space: pre-wrap; color: #fecaca; background: #450a0a; border: 1px solid #ef4444; border-radius: .5rem; }
"#;

#[cfg(test)]
mod tests {
    use super::*;
    use axiom_parser::{Page, State, Type};
    use axiom_project::{LinkedContract, ProjectConfig};

    fn temp_dir(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "synthetix-{label}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    fn write_import_graph(root: &Path) -> PathBuf {
        fs::create_dir_all(root).unwrap();
        fs::write(
            root.join("c.ax"),
            "export contract c(x: u32) -> u32 { ensures result == x +% 1; }\n",
        )
        .unwrap();
        fs::write(
            root.join("b.ax"),
            "import { c } from \"./c.ax\";\nexport contract b(x: u32) -> u32 { ensures result == c(x); }\n",
        )
        .unwrap();
        fs::write(
            root.join("a.ax"),
            "import { b } from \"./b.ax\";\nexport contract a(x: u32) -> u32 { ensures result == b(x); }\n",
        )
        .unwrap();
        fs::write(
            root.join("independent.ax"),
            "export contract independent(x: u32) -> u32 { ensures result == x; }\n",
        )
        .unwrap();
        fs::write(
            root.join("entry.ax"),
            "import { a } from \"./a.ax\";\nimport { independent } from \"./independent.ax\";\ncontract root(x: u32) -> u32 { ensures result == a(x); }\n",
        )
        .unwrap();
        let config = root.join("synthetix.toml");
        fs::write(
            &config,
            "[project]\nentry = \"entry.ax\"\ntarget = \"web\"\n",
        )
        .unwrap();
        config
    }

    fn compile_cached(
        config: &Path,
        cache: &mut VerificationCache,
        options: axiom_synth::SynthesisOptions,
        semantics_version: &str,
    ) -> Result<Vec<CompiledFunction>, String> {
        let project = axiom_project::load(config)?;
        compile_contracts_with_cache(&project, cache, options, semantics_version)
    }

    #[test]
    fn proof_cache_reuses_warm_proofs_and_invalidates_only_dependents() {
        let root = temp_dir("proof-cache");
        let config = write_import_graph(&root);
        let mut cache = VerificationCache::default();
        let options = axiom_synth::SynthesisOptions::default();

        let cold_start = Instant::now();
        assert_eq!(
            compile_cached(&config, &mut cache, options, VERIFIED_SEMANTICS_VERSION)
                .unwrap()
                .len(),
            5
        );
        let cold_elapsed_us = cold_start.elapsed().as_micros();
        assert_eq!(cache.stats.misses, 5);
        assert_eq!(cache.stats.hits, 0);

        let started = Instant::now();
        compile_cached(&config, &mut cache, options, VERIFIED_SEMANTICS_VERSION).unwrap();
        let warm_elapsed_us = started.elapsed().as_micros();
        assert_eq!(cache.stats.hits, 5);
        assert_eq!(cache.stats.misses, 0);

        fs::write(
            root.join("c.ax"),
            "export contract c(x: u32) -> u32 { ensures result == x +% 2; }\n",
        )
        .unwrap();
        let leaf_start = Instant::now();
        compile_cached(&config, &mut cache, options, VERIFIED_SEMANTICS_VERSION).unwrap();
        let leaf_elapsed_us = leaf_start.elapsed().as_micros();
        assert_eq!(cache.stats.hits, 1);
        assert_eq!(cache.stats.misses, 4);
        assert_eq!(cache.stats.invalidations, 4);

        fs::write(
            root.join("entry.ax"),
            "import { a } from \"./a.ax\";\nimport { independent } from \"./independent.ax\";\n// root-only edit\ncontract root(x: u32) -> u32 { ensures result == a(x); }\n",
        )
        .unwrap();
        let root_start = Instant::now();
        compile_cached(&config, &mut cache, options, VERIFIED_SEMANTICS_VERSION).unwrap();
        let root_elapsed_us = root_start.elapsed().as_micros();
        assert_eq!(cache.stats.hits, 4);
        assert_eq!(cache.stats.misses, 1);
        assert_eq!(cache.stats.invalidations, 1);

        fs::write(
            root.join("independent.ax"),
            "export contract independent(x: u32) -> u32 { ensures result == x +% 3; }\n",
        )
        .unwrap();
        compile_cached(&config, &mut cache, options, VERIFIED_SEMANTICS_VERSION).unwrap();
        assert_eq!(cache.stats.hits, 4);
        assert_eq!(cache.stats.misses, 1);
        assert_eq!(cache.stats.invalidations, 1);

        compile_cached(&config, &mut cache, options, "test-semantics-next").unwrap();
        assert_eq!(cache.stats.hits, 0);
        assert_eq!(cache.stats.misses, 5);
        assert_eq!(cache.stats.invalidations, 5);

        let limited_options = axiom_synth::SynthesisOptions {
            max_candidates: options.max_candidates - 1,
            ..options
        };
        compile_cached(&config, &mut cache, limited_options, "test-semantics-next").unwrap();
        assert_eq!(cache.stats.hits, 0);
        assert_eq!(cache.stats.misses, 5);
        assert_eq!(cache.stats.invalidations, 5);

        println!(
            "v6.5 cache: 5 contracts; cold={cold_elapsed_us}us; warm={warm_elapsed_us}us (5 hits); leaf edit={leaf_elapsed_us}us (4 invalidations + 1 unrelated hit); root edit={root_elapsed_us}us (1 invalidation); unrelated edit (1 invalidation)"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn proof_cache_keeps_independent_entry_across_remove_readd_and_source_edit() {
        fn wasm_bytes(compiled: &[CompiledFunction]) -> Vec<u8> {
            let mut exports = Vec::with_capacity(compiled.len() * 2);
            for function in compiled {
                exports.push((function.name.as_str(), &function.candidate, function.params));
                if let (Some(requires), Some(name)) =
                    (&function.requires, &function.requires_export)
                {
                    exports.push((name.as_str(), requires, function.params));
                }
            }
            axiom_wasm::emit_functions(&exports).unwrap()
        }

        fn assert_clean_equivalent(
            config: &Path,
            cached: &[CompiledFunction],
            options: axiom_synth::SynthesisOptions,
        ) {
            let mut clean_cache = VerificationCache::default();
            let clean = compile_cached(
                config,
                &mut clean_cache,
                options,
                VERIFIED_SEMANTICS_VERSION,
            )
            .unwrap();
            assert_eq!(
                cached
                    .iter()
                    .map(|function| (&function.name, &function.candidate))
                    .collect::<Vec<_>>(),
                clean
                    .iter()
                    .map(|function| (&function.name, &function.candidate))
                    .collect::<Vec<_>>()
            );
            assert_eq!(wasm_bytes(cached), wasm_bytes(&clean));
        }

        let root = temp_dir("proof-cache-remove-readd");
        let config = write_import_graph(&root);
        let entry = root.join("entry.ax");
        let independent = root.join("independent.ax");
        let independent_source =
            "export contract independent(x: u32) -> u32 { ensures result == x; }\n";
        let options = axiom_synth::SynthesisOptions::default();
        let mut cache = VerificationCache::default();

        compile_cached(&config, &mut cache, options, VERIFIED_SEMANTICS_VERSION).unwrap();
        assert_eq!(cache.entries.len(), 5);

        fs::write(
            &entry,
            "import { a } from \"./a.ax\";\ncontract root(x: u32) -> u32 { ensures result == a(x); }\n",
        )
        .unwrap();
        fs::remove_file(&independent).unwrap();
        let absent =
            compile_cached(&config, &mut cache, options, VERIFIED_SEMANTICS_VERSION).unwrap();
        assert_eq!(absent.len(), 4);
        assert_eq!(cache.stats.hits, 3);
        assert_eq!(cache.stats.misses, 1);
        assert_eq!(cache.stats.invalidations, 1);
        assert_eq!(cache.entries.len(), 5);
        assert_clean_equivalent(&config, &absent, options);

        fs::write(&independent, independent_source).unwrap();
        fs::write(
            &entry,
            "import { a } from \"./a.ax\";\nimport { independent } from \"./independent.ax\";\ncontract root(x: u32) -> u32 { ensures result == a(x); }\n",
        )
        .unwrap();
        let readded =
            compile_cached(&config, &mut cache, options, VERIFIED_SEMANTICS_VERSION).unwrap();
        assert_eq!(cache.stats.hits, 4);
        assert_eq!(cache.stats.misses, 1);
        assert_eq!(cache.stats.invalidations, 1);
        assert_eq!(cache.entries.len(), 5);
        assert_clean_equivalent(&config, &readded, options);

        fs::write(
            &independent,
            "export contract independent(x: u32) -> u32 { ensures result == x +% 3; }\n",
        )
        .unwrap();
        let edited =
            compile_cached(&config, &mut cache, options, VERIFIED_SEMANTICS_VERSION).unwrap();
        assert_eq!(cache.stats.hits, 4);
        assert_eq!(cache.stats.misses, 1);
        assert_eq!(cache.stats.invalidations, 1);
        assert_eq!(cache.entries.len(), 5);
        assert_clean_equivalent(&config, &edited, options);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn proof_cache_does_not_store_failed_or_partial_verification() {
        let root = temp_dir("proof-cache-failure");
        fs::create_dir_all(&root).unwrap();
        let source = root.join("main.ax");
        fs::write(
            root.join("positive.ax"),
            "export contract positive(x: u32) -> u32 { requires x > 0; ensures result == x; }\n",
        )
        .unwrap();
        fs::write(
            &source,
            "import { positive } from \"./positive.ax\";\ncontract value(x: u32) -> u32 { requires x > 0; ensures result == positive(x); }\n",
        )
        .unwrap();
        let config = root.join("synthetix.toml");
        fs::write(
            &config,
            "[project]\nentry = \"main.ax\"\ntarget = \"web\"\n",
        )
        .unwrap();
        let mut cache = VerificationCache::default();
        let options = axiom_synth::SynthesisOptions::default();
        compile_cached(&config, &mut cache, options, VERIFIED_SEMANTICS_VERSION).unwrap();
        assert_eq!(cache.entries.len(), 2);

        fs::write(
            &source,
            "import { positive } from \"./positive.ax\";\ncontract value(x: u32) -> u32 { ensures result == positive(x); }\n",
        )
        .unwrap();
        assert!(compile_cached(&config, &mut cache, options, VERIFIED_SEMANTICS_VERSION).is_err());
        let root_id = ContractId {
            source: fs::canonicalize(&source).unwrap(),
            name: "value".to_string(),
        };
        assert!(!cache.entries.contains_key(&root_id));
        assert_eq!(cache.stats.hits, 1);
        assert_eq!(cache.stats.misses, 1);
        assert_eq!(cache.stats.invalidations, 1);

        assert!(compile_cached(&config, &mut cache, options, VERIFIED_SEMANTICS_VERSION).is_err());
        assert_eq!(cache.stats.hits, 1);
        assert_eq!(cache.stats.misses, 1);
        assert_eq!(cache.stats.invalidations, 0);
        assert!(!cache.entries.contains_key(&root_id));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn proof_cache_handles_large_import_graph_cold_and_warm() {
        const MODULES: usize = 32;
        let root = temp_dir("proof-cache-large");
        fs::create_dir_all(&root).unwrap();
        for index in 0..MODULES {
            let contract = if index == 0 {
                "export contract c0(x: u32) -> u32 { ensures result == x; }\n".to_string()
            } else {
                format!(
                    "import {{ c{} }} from \"./module{:02}.ax\";\nexport contract c{index}(x: u32) -> u32 {{ ensures result == c{}(x); }}\n",
                    index - 1,
                    index - 1,
                    index - 1,
                )
            };
            fs::write(root.join(format!("module{index:02}.ax")), contract).unwrap();
        }
        let config = root.join("synthetix.toml");
        fs::write(
            &config,
            format!(
                "[project]\nentry = \"module{:02}.ax\"\ntarget = \"web\"\n",
                MODULES - 1
            ),
        )
        .unwrap();

        let mut cache = VerificationCache::default();
        let options = axiom_synth::SynthesisOptions::default();
        let cold_start = Instant::now();
        compile_cached(&config, &mut cache, options, VERIFIED_SEMANTICS_VERSION).unwrap();
        let cold_elapsed_us = cold_start.elapsed().as_micros();
        assert_eq!(cache.stats.misses, MODULES);
        assert_eq!(cache.stats.hits, 0);

        let warm_start = Instant::now();
        compile_cached(&config, &mut cache, options, VERIFIED_SEMANTICS_VERSION).unwrap();
        let warm_elapsed_us = warm_start.elapsed().as_micros();
        assert_eq!(cache.stats.hits, MODULES);
        assert_eq!(cache.stats.misses, 0);
        println!(
            "v6.5 cache large graph: modules={MODULES}; cold={cold_elapsed_us}us; warm={warm_elapsed_us}us; warm_hits={}",
            cache.stats.hits
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn generated_runtime_routes_verified_calls_to_wasm() {
        let contract = axiom_parser::parse(
            "contract add(x: u32, y: u32) -> u32 { ensures result == x +% y; }",
        )
        .unwrap();
        let project = Project {
            config: ProjectConfig {
                root: PathBuf::from("/tmp/synthetix"),
                entry: PathBuf::from("/tmp/synthetix/main.ax"),
                target: "web".to_string(),
                storage_schema_version: 1,
            },
            contracts: vec![LinkedContract {
                source: PathBuf::from("/tmp/synthetix/main.ax"),
                contract,
            }],
            verification_summaries: Vec::new(),
            verification_callees: HashMap::new(),
            verification_callee_ids: HashMap::new(),
            verification_order: Vec::new(),
            source_contents: HashMap::new(),
            pages: Vec::new(),
            files: Vec::new(),
        };

        let javascript = page_javascript(&project, false);
        assert!(javascript.contains("WebAssembly.instantiate(bytes, {})"));
        assert!(javascript.contains("callContract(element.function"));
        assert!(javascript.contains("requestEffect(request, targetState"));
        assert!(javascript.contains("fetch('./app.wasm')"));
        assert!(!javascript.contains("/__synthetix/version"));
        assert!(!javascript.contains("/__synthetix/error"));
        assert!(!javascript.contains("setInterval("));
    }

    #[test]
    fn production_build_verifies_nested_contract_chains_at_depth_1_2_4_8() {
        let root = std::env::temp_dir().join(format!(
            "synthetix-web-summary-depth-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        for depth in [1, 2, 4, 8] {
            let project_dir = root.join(format!("depth-{depth}"));
            fs::create_dir_all(&project_dir).unwrap();
            let mut source = String::new();
            for index in 0..depth {
                let value = if index == 0 {
                    "value".to_string()
                } else {
                    format!("level{}(value)", index - 1)
                };
                source.push_str(&format!(
                    "contract level{index}(value: u32) -> u32 {{ requires value > 0; ensures result == {value}; }}\n"
                ));
            }
            source.push_str(&format!(
                "contract root(value: u32) -> u32 {{ requires value > 0; ensures result == level{}(value); }}\n",
                depth - 1
            ));
            fs::write(project_dir.join("entry.ax"), source).unwrap();
            let config = project_dir.join("synthetix.toml");
            fs::write(
                &config,
                "[project]\nentry = \"entry.ax\"\ntarget = \"web\"\n",
            )
            .unwrap();

            let output = project_dir.join("dist");
            build(&config, &output).unwrap();
            let wasm = fs::read(output.join("app.wasm")).unwrap();
            assert!(
                wasm.starts_with(b"\0asm"),
                "depth {depth} did not emit WASM"
            );
            assert!(fs::metadata(output.join("app.proof.json")).unwrap().len() > 0);
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn production_build_reports_middle_callee_precondition_without_wasm() {
        let root = std::env::temp_dir().join(format!(
            "synthetix-web-middle-precondition-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        fs::write(
            root.join("entry.ax"),
            "contract positive(value: u32) -> u32 { requires value > 0; ensures result == value; }\n\
             contract middle(value: u32) -> u32 { requires value >= 0; ensures result == positive(value); }\n\
             contract root(value: u32) -> u32 { requires value > 0; ensures result == middle(value); }\n",
        )
        .unwrap();
        let config = root.join("synthetix.toml");
        fs::write(
            &config,
            "[project]\nentry = \"entry.ax\"\ntarget = \"web\"\n",
        )
        .unwrap();

        let output = root.join("dist");
        let error = build(&config, &output).unwrap_err();
        assert!(error.contains("entry.ax:2:"), "{error}");
        assert!(error.contains("call-site precondition for positive in middle"));
        assert!(!error.contains("COUNTEREXAMPLE"));
        assert!(!output.join("app.wasm").exists());

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn dev_rebuild_routes_propagate_live_reload_version_and_source_errors() {
        fn get(address: std::net::SocketAddr, path: &str) -> String {
            use std::io::{Read, Write};

            let mut stream = TcpStream::connect(address).unwrap();
            stream
                .write_all(format!("GET {path} HTTP/1.1\r\nHost: localhost\r\n\r\n").as_bytes())
                .unwrap();
            let mut response = String::new();
            stream.read_to_string(&mut response).unwrap();
            response
        }

        let root = std::env::temp_dir().join(format!(
            "synthetix-dev-live-reload-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let source = root.join("main.ax");
        let config = root.join("synthetix.toml");
        let output_dir = root.join(".synthetix/dev");
        std::fs::write(
            &source,
            "contract value(x: u32) -> u32 { ensures result == x; }\n",
        )
        .unwrap();
        std::fs::write(
            &config,
            "[project]\nentry = \"main.ax\"\ntarget = \"web\"\n",
        )
        .unwrap();
        let mut cache = VerificationCache::default();
        build_with_cache(&config, &output_dir, &mut cache, true).unwrap();
        let dev_javascript = std::fs::read_to_string(output_dir.join("app.js")).unwrap();
        assert!(dev_javascript.contains("/__synthetix/version"));
        assert!(dev_javascript.contains("/__synthetix/error"));
        assert!(dev_javascript.contains("}, 500);"));
        let project = axiom_project::load(&config).unwrap();
        let mut watched_files = project.files;
        watched_files.push(config.clone());
        let mut snapshot = file_snapshot_for(&watched_files);
        assert_eq!(cache.stats.misses, 1);
        let state = Arc::new(Mutex::new(DevState {
            version: 1,
            error: None,
        }));
        assert!(!rebuild_if_changed(
            &config,
            &output_dir,
            &state,
            &mut watched_files,
            &mut snapshot,
            &mut cache,
        ));

        std::fs::write(
            &source,
            "contract value(x: u32) -> u32 { ensures result == x +% 1; }\n",
        )
        .unwrap();
        assert!(rebuild_if_changed(
            &config,
            &output_dir,
            &state,
            &mut watched_files,
            &mut snapshot,
            &mut cache,
        ));
        assert_eq!(state.lock().unwrap().version, 2);
        assert_eq!(cache.stats.misses, 1);
        assert_eq!(cache.stats.invalidations, 1);

        std::fs::write(&source, "contract value(x: u32) -> u32 {\n").unwrap();
        assert!(rebuild_if_changed(
            &config,
            &output_dir,
            &state,
            &mut watched_files,
            &mut snapshot,
            &mut cache,
        ));
        assert_eq!(state.lock().unwrap().version, 2);
        assert!(
            state
                .lock()
                .unwrap()
                .error
                .as_deref()
                .unwrap()
                .contains("main.ax")
        );

        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let address = listener.local_addr().unwrap();
        let server_state = Arc::clone(&state);
        let server_output = output_dir.clone();
        let server = thread::spawn(move || {
            for _ in 0..6 {
                let (stream, _) = listener.accept().unwrap();
                handle_request(stream, &server_output, &server_state);
            }
        });
        let failed_version = get(address, "/__synthetix/version");
        assert!(failed_version.starts_with("HTTP/1.1 200 OK"));
        assert_eq!(failed_version.split("\r\n\r\n").nth(1), Some("2"));
        let failed_error = get(address, "/__synthetix/error");
        assert!(failed_error.contains("main.ax"));

        std::fs::write(
            &source,
            "contract value(x: u32) -> u32 { ensures result == x +% 2; }\n",
        )
        .unwrap();
        assert!(rebuild_if_changed(
            &config,
            &output_dir,
            &state,
            &mut watched_files,
            &mut snapshot,
            &mut cache,
        ));
        assert_eq!(state.lock().unwrap().version, 3);
        assert!(state.lock().unwrap().error.is_none());
        assert_eq!(cache.stats.misses, 1);
        assert_eq!(cache.stats.invalidations, 1);
        let recovered_version = get(address, "/__synthetix/version");
        assert_eq!(recovered_version.split("\r\n\r\n").nth(1), Some("3"));
        let recovered_error = get(address, "/__synthetix/error");
        assert_eq!(recovered_error.split("\r\n\r\n").nth(1), Some(""));
        let bundle = get(address, "/app.js");
        assert!(bundle.contains("200 OK"));
        assert!(bundle.contains("WebAssembly.instantiate"));
        let proof_map = get(address, "/app.proof.json");
        assert!(proof_map.contains("\"source\":\"main.ax\""));
        server.join().unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn static_bundle_is_reproducible_and_uses_public_environment_config() {
        let root = std::env::temp_dir().join(format!(
            "synthetix-static-bundle-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join("main.ax"),
            "contract increment(x: u32) -> u32 { ensures result == x +% 1; }\n",
        )
        .unwrap();
        let config = root.join("synthetix.toml");
        std::fs::write(
            &config,
            "[project]\nentry = \"main.ax\"\ntarget = \"web\"\n",
        )
        .unwrap();
        let first = root.join("first");
        let second = root.join("second");
        build(&config, &first).unwrap();
        build(&config, &second).unwrap();

        let files = [
            "index.html",
            "app.config.js",
            "app.js",
            "app.wasm",
            "app.css",
            "app.proof.json",
        ];
        for file in files {
            assert_eq!(
                std::fs::read(first.join(file)).unwrap(),
                std::fs::read(second.join(file)).unwrap(),
                "static bundle output differs for {file}"
            );
        }
        let html = std::fs::read_to_string(first.join("index.html")).unwrap();
        assert!(
            html.find("./app.config.js").unwrap() < html.find("./app.js").unwrap(),
            "deployment config must load before the runtime"
        );
        std::fs::write(
            first.join("app.config.js"),
            "window.synthetixConfig = Object.freeze({ environment: 'staging', apiBaseUrl: 'https://api.example.invalid' });\n",
        )
        .unwrap();
        let script = r#"
const fs = require('node:fs');
const vm = require('node:vm');
const bytes = fs.readFileSync(process.env.AXIOM_TEST_WASM);
class Element {
  constructor() { this.children = []; this.dataset = {}; this.hidden = true; this.textContent = ''; }
  append(...items) { this.children.push(...items); }
  replaceChildren() { this.children = []; }
}
const root = new Element();
const error = new Element();
global.window = {};
global.location = { reload() {} };
global.localStorage = { getItem() { return null; }, setItem() {} };
global.document = {
  querySelector(selector) { return selector === '#app' ? root : error; },
  createElement() { return new Element(); },
};
global.setInterval = () => 0;
global.fetch = async (url) => url === './app.wasm'
  ? { ok: true, arrayBuffer: async () => bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength) }
  : { ok: false, text: async () => '' };
vm.runInThisContext(fs.readFileSync(process.env.AXIOM_TEST_CONFIG, 'utf8'));
vm.runInThisContext(fs.readFileSync(process.env.AXIOM_TEST_JS, 'utf8'));
(async () => {
  for (let attempt = 0; attempt < 100 && !window.synthetix; attempt++)
    await new Promise((resolve) => setTimeout(resolve, 0));
  if (!window.synthetix) throw new Error('generated bundle did not start');
  window.synthetix.registerEffectAdapter(async () => window.synthetix.config.apiBaseUrl);
  const baseUrl = await window.synthetix.requestEffect({ name: 'config', inputs: [] });
  process.stdout.write([window.synthetix.config.environment, window.synthetix.config.apiBaseUrl, baseUrl].join('|'));
})().catch((error) => { console.error(error); process.exitCode = 1; });
"#;
        let result = std::process::Command::new("node")
            .arg("-e")
            .arg(script)
            .env("AXIOM_TEST_CONFIG", first.join("app.config.js"))
            .env("AXIOM_TEST_JS", first.join("app.js"))
            .env("AXIOM_TEST_WASM", first.join("app.wasm"))
            .output()
            .expect("Node.js is required to execute the generated static bundle");
        assert!(
            result.status.success(),
            "static bundle runtime failed: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert_eq!(
            String::from_utf8(result.stdout).unwrap(),
            "staging|https://api.example.invalid|https://api.example.invalid"
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn project_build_path_verifies_imported_requires_from_summaries() {
        let root = std::env::temp_dir().join(format!(
            "synthetix-web-composition-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join("positive.ax"),
            "export contract positive(value: u32) -> u32 {\n requires value > 0;\n ensures result == value;\n}\n",
        )
        .unwrap();
        let entry = root.join("entry.ax");
        std::fs::write(
            &entry,
            "import { positive } from \"./positive.ax\";\ncontract total(value: u32) -> u32 {\n requires value > 0;\n ensures result == positive(value);\n}\n",
        )
        .unwrap();
        let config = root.join("synthetix.toml");
        std::fs::write(
            &config,
            "[project]\nentry = \"entry.ax\"\ntarget = \"web\"\n",
        )
        .unwrap();

        let output = root.join("dist");
        build(&config, &output).unwrap();
        assert!(output.join("app.wasm").is_file());

        std::fs::write(
            &entry,
            "import { positive } from \"./positive.ax\";\ncontract total(value: u32) -> u32 {\n ensures result == positive(value);\n}\n",
        )
        .unwrap();
        let failed_output = root.join("failed-dist");
        let error = build(&config, &failed_output).unwrap_err();
        assert!(error.contains(&format!("{}:3:", entry.display())));
        assert!(error.contains("call-site precondition for positive"));
        assert!(!failed_output.exists());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn generated_browser_runtime_instantiates_wasm_and_matches_scalar_backends() {
        let definitions = [
            (
                "mul8",
                "contract mul8(x: u32) -> u32 { ensures result == x *% 8; }",
                Expr::Mul(Box::new(Expr::Arg(0)), Box::new(Expr::Const(8))),
            ),
            (
                "add_wrap",
                "contract add_wrap(x: u32, y: u32) -> u32 { ensures result == x +% y; }",
                Expr::Add(Box::new(Expr::Arg(0)), Box::new(Expr::Arg(1))),
            ),
            (
                "mul_wrap",
                "contract mul_wrap(x: u32, y: u32) -> u32 { ensures result == x *% y; }",
                Expr::Mul(Box::new(Expr::Arg(0)), Box::new(Expr::Arg(1))),
            ),
            (
                "signed_lt",
                "contract signed_lt(x: i32, y: i32) -> bool { ensures result == (x < y); }",
                Expr::SLt(Box::new(Expr::Arg(0)), Box::new(Expr::Arg(1))),
            ),
            (
                "unsigned_lt",
                "contract unsigned_lt(x: u32, y: u32) -> bool { ensures result == (x < y); }",
                Expr::Lt(Box::new(Expr::Arg(0)), Box::new(Expr::Arg(1))),
            ),
            (
                "bool_identity",
                "contract bool_identity(x: bool) -> bool { ensures result == x; }",
                Expr::Arg(0),
            ),
        ];
        let mut contracts = Vec::new();
        let mut exports = Vec::new();
        let mut candidates = HashMap::new();
        for (name, source, candidate) in &definitions {
            let contract = axiom_parser::parse(source).unwrap();
            exports.push((*name, candidate, contract.params.len() as u32));
            candidates.insert(*name, candidate.clone());
            contracts.push(LinkedContract {
                source: PathBuf::from("/tmp/synthetix/main.ax"),
                contract,
            });
        }

        let buttons: Vec<PageElement> = definitions
            .iter()
            .map(|(name, _, _)| PageElement::Button {
                label: (*name).to_string(),
                state: "result".to_string(),
                function: (*name).to_string(),
                args: vec![
                    SpecExpr::Ident("x".to_string()),
                    SpecExpr::Ident("y".to_string()),
                ]
                .into_iter()
                .take(if matches!(*name, "bool_identity" | "mul8") {
                    1
                } else {
                    2
                })
                .collect(),
            })
            .collect();
        let mut elements = vec![
            PageElement::Input {
                state: "x".to_string(),
                placeholder: "x".to_string(),
            },
            PageElement::Show {
                state: "result".to_string(),
            },
        ];
        elements.extend(buttons);
        let page = Page {
            name: "Conformance".to_string(),
            states: vec![
                State {
                    name: "x".to_string(),
                    ty: Type::U32,
                    initial: SpecExpr::Number(0),
                },
                State {
                    name: "y".to_string(),
                    ty: Type::U32,
                    initial: SpecExpr::Number(0),
                },
                State {
                    name: "result".to_string(),
                    ty: Type::U32,
                    initial: SpecExpr::Number(0),
                },
            ],
            persisted: Vec::new(),
            reducers: Vec::new(),
            elements,
        };
        let contracts_for_project = contracts.clone();
        let project = Project {
            config: ProjectConfig {
                root: PathBuf::from("/tmp/synthetix"),
                entry: PathBuf::from("/tmp/synthetix/main.ax"),
                target: "web".to_string(),
                storage_schema_version: 1,
            },
            contracts,
            verification_summaries: contracts_for_project,
            verification_callees: HashMap::new(),
            verification_callee_ids: HashMap::new(),
            verification_order: Vec::new(),
            source_contents: HashMap::new(),
            pages: vec![LinkedPage {
                source: PathBuf::from("/tmp/synthetix/main.ax"),
                page,
            }],
            files: Vec::new(),
        };
        let javascript = page_javascript(&project, true);
        let wasm = axiom_wasm::emit_functions(&exports).unwrap();
        let cases = [
            ("mul8", [5, 0]),
            ("add_wrap", [u32::MAX, 1]),
            ("add_wrap", [0x7fff_ffff, 1]),
            ("mul_wrap", [u32::MAX, 2]),
            ("signed_lt", [u32::MAX, 1]),
            ("signed_lt", [0x7fff_ffff, 0x8000_0000]),
            ("unsigned_lt", [u32::MAX, 1]),
            ("unsigned_lt", [0, 1]),
            ("bool_identity", [0, 0]),
            ("bool_identity", [1, 0]),
        ];

        for (name, inputs) in cases {
            let candidate = &candidates[name];
            let linked = project
                .verification_summaries
                .iter()
                .find(|linked| linked.contract.name == name)
                .unwrap();
            let values = &inputs[..linked.contract.params.len()];
            let expected = axiom_verify::reference::eval_ami(candidate, values).unwrap();
            assert_eq!(candidate.eval(values), expected);
            let point = point_contract(&linked.contract, &inputs, expected);
            assert_eq!(
                axiom_verify::verify(&point, candidate).unwrap(),
                axiom_verify::ProofResult::Verified,
                "Z3 point proof failed for {name}({inputs:?})"
            );
            let single_wasm =
                axiom_wasm::emit(candidate, linked.contract.params.len() as u32).unwrap();
            assert_eq!(
                axiom_wasm::run_with_args(&single_wasm, values).unwrap(),
                expected
            );
        }

        let root = std::env::temp_dir().join(format!(
            "synthetix-browser-wasm-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let javascript_path = root.join("app.js");
        let wasm_path = root.join("app.wasm");
        std::fs::write(&javascript_path, javascript).unwrap();
        std::fs::write(&wasm_path, wasm).unwrap();
        let cases_js = cases
            .iter()
            .map(|(name, inputs)| format!("{{name:'{name}',inputs:[{},{}]}}", inputs[0], inputs[1]))
            .collect::<Vec<_>>()
            .join(",");
        let script = format!(
            r#"
const fs = require('node:fs');
const vm = require('node:vm');
const bytes = fs.readFileSync(process.env.AXIOM_TEST_WASM);
const cases = [{cases_js}];
class Element {{
  constructor(tagName) {{ this.tagName = tagName; this.children = []; this.listeners = {{}}; this.dataset = {{}}; this.value = ''; this.checked = false; this.hidden = true; this.textContent = ''; }}
  append(...items) {{ this.children.push(...items); }}
  replaceChildren() {{ this.children = []; }}
  addEventListener(name, callback) {{ this.listeners[name] = callback; }}
  dispatch(name) {{ this.listeners[name]?.({{ target: this }}); }}
  click() {{ this.listeners.click?.(); }}
}}
const root = new Element('main');
const error = new Element('div');
let reloadCount = 0;
let devVersion = '1';
let devError = '';
let poll;
global.window = {{}};
global.location = {{ reload() {{ reloadCount++; }} }};
global.localStorage = {{ getItem() {{ return null; }}, setItem() {{}} }};
global.document = {{
  querySelector(selector) {{ return selector === '#app' ? root : error; }},
  createElement(tagName) {{ return new Element(tagName); }},
}};
global.setInterval = (callback) => {{ poll = callback; return 0; }};
global.fetch = async (url) => {{
  if (url === './app.wasm') return {{ ok: true, arrayBuffer: async () => bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength) }};
  if (url === '/__synthetix/version') return {{ ok: true, text: async () => devVersion }};
  if (url === '/__synthetix/error') return {{ ok: true, text: async () => devError }};
  return {{ ok: false, text: async () => '' }};
}};
vm.runInThisContext(fs.readFileSync(process.env.AXIOM_TEST_JS, 'utf8'));
(async () => {{
  for (let attempt = 0; attempt < 100 && !window.synthetix; attempt++)
    await new Promise((resolve) => setTimeout(resolve, 0));
  if (!window.synthetix) throw new Error('generated runtime did not instantiate WASM');
  const output = [];
  for (const item of cases) {{
    window.synthetix.state.x = item.name === 'bool_identity'
      ? item.inputs[0] !== 0
      : item.name === 'signed_lt' ? item.inputs[0] | 0 : item.inputs[0];
    window.synthetix.state.y = item.name === 'signed_lt' ? item.inputs[1] | 0 : item.inputs[1];
    const button = root.children.find((node) => node.tagName === 'button' && node.textContent === item.name);
    if (!button) throw new Error(`missing generated button: ${{item.name}}`);
    button.click();
    output.push(window.synthetix.state.result >>> 0);
  }}
  const input = root.children.find((node) => node.tagName === 'input');
  window.synthetix.state.x = 7;
  input.value = '4294967296';
  input.dispatch('change');
  const invalid = window.synthetix.lastError;
  const invalidLine = [invalid?.category, invalid?.source, window.synthetix.state.x, String(error.textContent.includes('COUNTEREXAMPLE'))].join('|');

  input.value = '5';
  input.dispatch('change');
  const mul8 = root.children.find((node) => node.tagName === 'button' && node.textContent === 'mul8');
  mul8.click();
  const outputNode = root.children.find((node) => node.tagName === 'output');
  const flowLine = [window.synthetix.state.x, window.synthetix.state.result, outputNode.textContent].join('|');

  window.synthetix.registerEffectAdapter(async (request) => {{
    if (request.name === 'double') return request.inputs[0] * 2;
    throw new Error('adapter unavailable');
  }});
  await window.synthetix.requestEffect({{ name: 'double', inputs: [3] }}, 'result');
  const effectOutput = root.children.find((node) => node.tagName === 'output');
  const effectLine = [window.synthetix.state.result, effectOutput.textContent].join('|');

  window.synthetix.state.x = '5';
  const invalidCall = root.children.find((node) => node.tagName === 'button' && node.textContent === 'mul8');
  invalidCall.click();
  const callError = window.synthetix.lastError;
  const callErrorLine = [callError?.category, callError?.source, callError?.contract, String(error.textContent.includes('COUNTEREXAMPLE'))].join('|');

  const effectFailure = await window.synthetix.requestEffect({{ name: 'offline', inputs: [] }}, 'result').catch((failure) => failure);
  const effectFailureLine = [effectFailure?.category, effectFailure?.source, window.synthetix.state.result, String(error.textContent.includes('COUNTEREXAMPLE'))].join('|');
  devError = 'main.ax:3:1: rebuild failed';
  await poll();
  const reloadErrorLine = [reloadCount, String(error.hidden), error.textContent].join('|');
  devVersion = '2';
  devError = '';
  await poll();
  const reloadLine = [reloadCount, String(error.hidden), error.textContent].join('|');
  process.stdout.write([output.join(','), invalidLine, flowLine, effectLine, callErrorLine,
    effectFailureLine, reloadErrorLine, reloadLine].join('\n'));
}})().catch((error) => {{ console.error(error); process.exitCode = 1; }});
"#
        );
        let result = std::process::Command::new("node")
            .arg("-e")
            .arg(script)
            .env("AXIOM_TEST_JS", &javascript_path)
            .env("AXIOM_TEST_WASM", &wasm_path)
            .output()
            .expect("Node.js is required to execute the generated browser runtime");
        assert!(
            result.status.success(),
            "browser runtime failed: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        let output = String::from_utf8(result.stdout).unwrap();
        let mut lines = output.lines();
        let browser_results = lines
            .next()
            .unwrap()
            .split(',')
            .map(|value| value.parse::<u32>().unwrap())
            .collect::<Vec<_>>();
        let expected_results = cases
            .iter()
            .map(|(name, inputs)| candidates[name].eval(inputs))
            .collect::<Vec<_>>();
        assert_eq!(browser_results, expected_results);
        assert_eq!(
            lines.next(),
            Some("APPLICATION_RUNTIME_ERROR|main.ax|7|false")
        );
        assert_eq!(lines.next(), Some("5|40|40"));
        assert_eq!(lines.next(), Some("6|6"));
        assert_eq!(
            lines.next(),
            Some("APPLICATION_RUNTIME_ERROR|main.ax|mul8|false")
        );
        assert_eq!(
            lines.next(),
            Some("APPLICATION_RUNTIME_ERROR|main.ax|6|false")
        );
        assert_eq!(lines.next(), Some("0|false|main.ax:3:1: rebuild failed"));
        assert_eq!(lines.next(), Some("1|true|"));
        assert!(lines.next().is_none());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn built_web_app_runs_input_state_verified_wasm_and_effect_runtime() {
        let root = std::env::temp_dir().join(format!(
            "synthetix-web-runtime-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join("main.ax"),
            "contract mul8(x: u32) -> u32 { ensures result == x *% 8; }\n\
             contract positive(x: u32) -> u32 { requires x > 0; ensures result == x; }\n\
             contract bool_identity(value: bool) -> bool { ensures result == value; }\n\
             page Counter {\n\
               state value: number = 0;\n\
               state result: number = 0;\n\
               state guarded_result: number = 0;\n\
               state ready: bool = false;\n\
               state ready_result: bool = false;\n\
               input value placeholder \"number\";\n\
               input ready placeholder \"ready\";\n\
               show result;\n\
               show guarded_result;\n\
               show ready_result;\n\
               button \"run\" updates result with mul8(value);\n\
               button \"guard\" updates guarded_result with positive(value);\n\
               button \"check\" updates ready_result with bool_identity(ready);\n\
             }\n",
        )
        .unwrap();
        let config = root.join("synthetix.toml");
        std::fs::write(
            &config,
            "[project]\nentry = \"main.ax\"\ntarget = \"web\"\n",
        )
        .unwrap();
        let output = root.join("dist");
        build(&config, &output).unwrap();

        let script = r#"
const fs = require('node:fs');
const vm = require('node:vm');
const bytes = fs.readFileSync(process.env.AXIOM_TEST_WASM);
class Element {
  constructor(tagName) { this.tagName = tagName; this.children = []; this.listeners = {}; this.dataset = {}; this.value = ''; this.checked = false; this.hidden = true; this.textContent = ''; }
  append(...items) { this.children.push(...items); }
  replaceChildren() { this.children = []; }
  addEventListener(name, callback) { this.listeners[name] = callback; }
  dispatch(name) { this.listeners[name]?.({ target: this }); }
  click() { this.listeners.click?.(); }
}
const root = new Element('main');
const error = new Element('div');
global.window = {};
global.location = { reload() {} };
global.localStorage = { getItem() { return null; }, setItem() {} };
global.document = {
  querySelector(selector) { return selector === '#app' ? root : error; },
  createElement(tagName) { return new Element(tagName); },
};
global.setInterval = () => 0;
global.fetch = async (url) => url === './app.wasm'
  ? { ok: true, arrayBuffer: async () => bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength) }
  : { ok: false, text: async () => '' };
vm.runInThisContext(fs.readFileSync(process.env.AXIOM_TEST_JS, 'utf8'));
(async () => {
  for (let attempt = 0; attempt < 100 && !window.synthetix; attempt++)
    await new Promise((resolve) => setTimeout(resolve, 0));
  if (!window.synthetix) throw new Error('generated runtime did not instantiate verified WASM');
  const input = root.children.find((node) => node.tagName === 'input');
  if (input.type !== 'number') throw new Error(`unexpected input type: ${input.type}`);
  input.value = '5';
  input.dispatch('change');
  const run = root.children.find((node) => node.tagName === 'button' && node.textContent === 'run');
  run.click();
  const rendered = root.children.find((node) => node.tagName === 'output');
  const flow = [window.synthetix.state.value, window.synthetix.state.result, rendered.textContent].join('|');

  const invalidInput = root.children.find((node) => node.tagName === 'input');
  invalidInput.value = '4294967296';
  invalidInput.dispatch('change');
  const invalid = [window.synthetix.lastError?.category, window.synthetix.lastError?.source, window.synthetix.state.value, String(error.textContent.includes('COUNTEREXAMPLE'))].join('|');

  const guardedInput = root.children.find((node) => node.tagName === 'input' && node.type === 'number');
  guardedInput.value = '0';
  guardedInput.dispatch('change');
  root.children.find((node) => node.tagName === 'button' && node.textContent === 'guard').click();
  const precondition = [window.synthetix.lastError?.category, window.synthetix.lastError?.source, window.synthetix.lastError?.contract, window.synthetix.state.guarded_result, String(error.textContent.includes('COUNTEREXAMPLE'))].join('|');

  window.synthetix.registerEffectAdapter(async (request) => request.inputs[0] * 2);
  await window.synthetix.requestEffect({ name: 'double', inputs: [3] }, 'result');
  const effectRendered = root.children.find((node) => node.tagName === 'output');
  const effect = [window.synthetix.state.result, effectRendered.textContent].join('|');

  const checkbox = root.children.find((node) => node.tagName === 'input' && node.type === 'checkbox');
  checkbox.checked = true;
  checkbox.dispatch('change');
  root.children.find((node) => node.tagName === 'button' && node.textContent === 'check').click();
  const booleanOutput = root.children.find((node) => node.tagName === 'output' && node.dataset.state === 'ready_result');
  const booleanFlow = [window.synthetix.state.ready, window.synthetix.state.ready_result, booleanOutput.textContent].join('|');

  window.synthetix.registerEffectAdapter(async () => { throw new Error('offline'); });
  const failure = await window.synthetix.requestEffect({ name: 'offline', inputs: [] }, 'result').catch((error) => error);
  const failedEffect = [failure?.category, failure?.source, window.synthetix.state.result, String(error.textContent.includes('COUNTEREXAMPLE'))].join('|');
  process.stdout.write([flow, invalid, precondition, effect, booleanFlow, failedEffect].join('\n'));
})().catch((error) => { console.error(error); process.exitCode = 1; });
"#;
        let result = std::process::Command::new("node")
            .arg("-e")
            .arg(script)
            .env("AXIOM_TEST_JS", output.join("app.js"))
            .env("AXIOM_TEST_WASM", output.join("app.wasm"))
            .output()
            .expect("Node.js is required to execute the generated browser runtime");
        assert!(
            result.status.success(),
            "web runtime failed: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        let stdout = String::from_utf8(result.stdout).unwrap();
        let mut lines = stdout.lines();
        assert_eq!(lines.next(), Some("5|40|40"));
        assert_eq!(
            lines.next(),
            Some("APPLICATION_RUNTIME_ERROR|main.ax|5|false")
        );
        assert_eq!(
            lines.next(),
            Some("APPLICATION_RUNTIME_ERROR|main.ax|positive|0|false")
        );
        assert_eq!(lines.next(), Some("6|6"));
        assert_eq!(lines.next(), Some("true|true|true"));
        assert_eq!(
            lines.next(),
            Some("APPLICATION_RUNTIME_ERROR|main.ax|6|false")
        );
        assert!(lines.next().is_none());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn todo_example_keeps_application_data_outside_verified_scalars() {
        let config = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../examples/v3-todo-v31/synthetix.toml");
        let root = std::env::temp_dir().join(format!(
            "synthetix-web-data-boundary-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let output = root.join("dist");
        build(&config, &output).unwrap();

        let script = r#"
const fs = require('node:fs');
const vm = require('node:vm');
const bytes = fs.readFileSync(process.env.AXIOM_TEST_WASM);
const storage = new Map();
class Element {
  constructor(tagName) { this.tagName = tagName; this.children = []; this.listeners = {}; this.dataset = {}; this.value = ''; this.checked = false; this.textContent = ''; }
  append(...items) { this.children.push(...items); }
  replaceChildren() { this.children = []; }
  addEventListener(name, callback) { this.listeners[name] = callback; }
  dispatch(name) { this.listeners[name]?.({ target: this }); }
  click() { this.listeners.click?.(); }
}
const root = new Element('main');
const error = new Element('div');
global.window = {};
global.location = { reload() {} };
global.localStorage = {
  getItem(key) { return storage.get(key) ?? null; },
  setItem(key, value) { storage.set(key, String(value)); },
};
global.document = {
  querySelector(selector) { return selector === '#app' ? root : error; },
  createElement(tagName) { return new Element(tagName); },
};
global.setInterval = () => 0;
global.fetch = async (url) => url === './app.wasm'
  ? { ok: true, arrayBuffer: async () => bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength) }
  : { ok: false, text: async () => '' };
vm.runInThisContext(fs.readFileSync(process.env.AXIOM_TEST_JS, 'utf8'));
(async () => {
  for (let attempt = 0; attempt < 100 && !window.synthetix; attempt++)
    await new Promise((resolve) => setTimeout(resolve, 0));
  if (!window.synthetix) throw new Error('Todo runtime did not instantiate verified WASM');
  const input = root.children.find((node) => node.tagName === 'input');
  if (input.type !== 'text') throw new Error(`unexpected text state input type: ${input.type}`);
  input.value = 'Task';
  input.dispatch('change');
  root.children.find((node) => node.tagName === 'button' && node.textContent === '추가').click();

  let row = root.children.find((node) => node.tagName === 'ul')?.children[0];
  const initial = [window.synthetix.state.draft, window.synthetix.state.todos.length,
    window.synthetix.state.todos[0]?.title, String(window.synthetix.state.todos[0]?.done),
    row?.children.find((node) => node.tagName === 'output')?.textContent].join('|');
  row?.children.find((node) => node.tagName === 'button' && node.textContent === '완료 전환').click();

  row = root.children.find((node) => node.tagName === 'ul')?.children[0];
  const saved = JSON.parse(storage.get('synthetix:Todo:state')).values.todos;
  const completed = [window.synthetix.state.todos[0]?.done,
    row?.children.find((node) => node.tagName === 'output')?.textContent,
    row?.children.find((node) => node.tagName === 'p')?.textContent,
    saved[0]?.done, window.synthetix.lastError?.category ?? 'none'].join('|');
  process.stdout.write([initial, completed].join('\n'));
})().catch((error) => { console.error(error); process.exitCode = 1; });
"#;
        let result = std::process::Command::new("node")
            .arg("-e")
            .arg(script)
            .env("AXIOM_TEST_JS", output.join("app.js"))
            .env("AXIOM_TEST_WASM", output.join("app.wasm"))
            .output()
            .expect("Node.js is required to execute the generated browser runtime");
        assert!(
            result.status.success(),
            "Todo data boundary runtime failed: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        let stdout = String::from_utf8(result.stdout).unwrap();
        let mut lines = stdout.lines();
        assert_eq!(lines.next(), Some("Task|1|Task|false|Task"));
        assert_eq!(lines.next(), Some("true|Task|완료됨|true|none"));
        assert!(lines.next().is_none());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn persisted_state_is_versioned_validated_and_migratable() {
        let root = std::env::temp_dir().join(format!(
            "synthetix-web-persistence-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let example = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/v3-todo-v31");
        std::fs::copy(example.join("main.ax"), root.join("main.ax")).unwrap();
        std::fs::copy(example.join("logic.ax"), root.join("logic.ax")).unwrap();
        let config = root.join("synthetix.toml");
        std::fs::write(
            &config,
            "[project]\nentry = \"main.ax\"\ntarget = \"web\"\nstorage_schema_version = 2\n",
        )
        .unwrap();
        let output = root.join("dist");
        build(&config, &output).unwrap();

        let script = r#"
const fs = require('node:fs');
const vm = require('node:vm');
const bytes = fs.readFileSync(process.env.AXIOM_TEST_WASM);
const source = fs.readFileSync(process.env.AXIOM_TEST_JS, 'utf8');
const key = 'synthetix:Todo:state';
class Element {
  constructor(tagName) { this.tagName = tagName; this.children = []; this.listeners = {}; this.dataset = {}; this.value = ''; this.checked = false; this.hidden = true; this.textContent = ''; }
  append(...items) { this.children.push(...items); }
  replaceChildren() { this.children = []; }
  addEventListener(name, callback) { this.listeners[name] = callback; }
  dispatch(name) { this.listeners[name]?.({ target: this }); }
  click() { this.listeners.click?.(); }
}
function makeStorage(seed = {}, fail = {}) {
  const values = new Map(Object.entries(seed));
  return {
    values,
    getItem(name) { if (fail.read) throw new Error('storage unavailable'); return values.get(name) ?? null; },
    setItem(name, value) { if (fail.write) throw new Error('storage unavailable'); values.set(name, String(value)); },
  };
}
async function launch(storage, migration = null) {
  const root = new Element('main');
  const error = new Element('div');
  const window = { synthetixStorageAdapter: storage };
  if (migration) window.synthetixStorageMigration = migration;
  const context = {
    window,
    location: { reload() {} },
    document: {
      querySelector(selector) { return selector === '#app' ? root : error; },
      createElement(tagName) { return new Element(tagName); },
    },
    setInterval: () => 0,
    setTimeout,
    structuredClone,
    fetch: async (url) => url === './app.wasm'
      ? { ok: true, arrayBuffer: async () => bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength) }
      : { ok: false, text: async () => '' },
  };
  vm.runInNewContext(`(function() { ${source}\n })()`, context);
  for (let attempt = 0; attempt < 100 && !window.synthetix?.state; attempt++)
    await new Promise((resolve) => setTimeout(resolve, 0));
  if (!window.synthetix?.state) throw new Error('runtime did not finish booting');
  return { window, root, error };
}
(async () => {
  const storage = makeStorage();
  const first = await launch(storage);
  if (first.window.synthetix.storageSchemaVersion !== 2) throw new Error('schema version was not embedded');
  const input = first.root.children.find((node) => node.tagName === 'input');
  input.value = 'Task';
  input.dispatch('change');
  first.root.children.find((node) => node.tagName === 'button' && node.textContent === '추가').click();
  first.root.children.find((node) => node.tagName === 'button' && node.textContent === '검증된 상태 갱신').click();
  const envelope = JSON.parse(storage.values.get(key));
  const persistence = [envelope.version, Object.keys(envelope.values).sort().join(','),
    String('count' in envelope.values || 'ready' in envelope.values)].join('|');

  const restored = await launch(storage);
  const restore = [restored.window.synthetix.state.draft, restored.window.synthetix.state.todos[0]?.title,
    restored.window.synthetix.state.count, restored.window.synthetix.state.ready].join('|');

  const malformed = await launch(makeStorage({ [key]: '{' }));
  const malformedResult = [malformed.window.synthetix.lastError?.category,
    malformed.window.synthetix.state.todos.length].join('|');
  const badShape = await launch(makeStorage({ [key]: JSON.stringify({ version: 2, values: { todos: 3 } }) }));
  const badShapeResult = [badShape.window.synthetix.lastError?.category,
    badShape.window.synthetix.state.todos.length].join('|');

  const incompatibleStorage = makeStorage({ [key]: JSON.stringify({ version: 99, values: { draft: 'old', todos: [] } }) });
  const incompatible = await launch(incompatibleStorage);
  const incompatibleResult = [incompatible.window.synthetix.lastError?.category,
    incompatible.window.synthetix.state.draft, JSON.parse(incompatibleStorage.values.get(key)).version].join('|');

  let migrations = 0;
  const migrationStorage = makeStorage({ [key]: JSON.stringify({
    version: 1, values: { title: 'Legacy', todos: [{ title: 'old', done: false }] },
  }) });
  const migrated = await launch(migrationStorage, async (request) => {
    if (request.fromVersion !== 1 || request.toVersion !== 2) throw new Error('wrong migration versions');
    migrations++;
    return { draft: request.values.title, todos: request.values.todos };
  });
  const migrationResult = [migrated.window.synthetix.state.draft,
    migrated.window.synthetix.state.todos[0]?.title,
    JSON.parse(migrationStorage.values.get(key)).version, migrations].join('|');

  const retryStorage = makeStorage({ [key]: JSON.stringify({ version: 1, values: { draft: 'Retry' } }) });
  const retry = await launch(retryStorage);
  retry.window.synthetix.registerStorageMigration(({ values }) => ({ draft: `${values.draft} migrated` }));
  await retry.window.synthetix.restore();
  const retryResult = [retry.window.synthetix.state.draft,
    JSON.parse(retryStorage.values.get(key)).version,
    retry.window.synthetix.lastError?.category ?? 'none'].join('|');

  const failedMigrationStorage = makeStorage({ [key]: JSON.stringify({ version: 1, values: { draft: 'old' } }) });
  const failedMigration = await launch(failedMigrationStorage, async () => { throw new Error('migration failed'); });
  const migrationFailureResult = [failedMigration.window.synthetix.lastError?.category,
    failedMigration.window.synthetix.state.draft,
    JSON.parse(failedMigrationStorage.values.get(key)).version].join('|');

  const legacyStorage = makeStorage({
    'synthetix:Todo:draft': JSON.stringify('Legacy'),
    'synthetix:Todo:todos': JSON.stringify([{ title: 'Legacy todo', done: false }]),
  });
  const legacy = await launch(legacyStorage);
  const legacyResult = [legacy.window.synthetix.state.draft,
    legacy.window.synthetix.state.todos[0]?.title,
    JSON.parse(legacyStorage.values.get(key)).version].join('|');

  const unavailable = await launch(makeStorage({}, { read: true }));
  const readFailure = [unavailable.window.synthetix.lastError?.category,
    unavailable.window.synthetix.state.todos.length].join('|');
  const writeFailure = await launch(makeStorage({}, { write: true }));
  const writeInput = writeFailure.root.children.find((node) => node.tagName === 'input');
  writeInput.value = 'Not saved';
  writeInput.dispatch('change');
  const writeResult = [writeFailure.window.synthetix.lastError?.category,
    String(writeFailure.error.textContent.includes('COUNTEREXAMPLE'))].join('|');

  process.stdout.write([persistence, restore, malformedResult, badShapeResult,
    incompatibleResult, migrationResult, retryResult, migrationFailureResult,
    legacyResult, readFailure, writeResult].join('\n'));
})().catch((error) => { console.error(error); process.exitCode = 1; });
"#;
        let result = std::process::Command::new("node")
            .arg("-e")
            .arg(script)
            .env("AXIOM_TEST_JS", output.join("app.js"))
            .env("AXIOM_TEST_WASM", output.join("app.wasm"))
            .output()
            .expect("Node.js is required to execute the generated browser runtime");
        assert!(
            result.status.success(),
            "persistence runtime failed: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        let stdout = String::from_utf8(result.stdout).unwrap();
        let mut lines = stdout.lines();
        assert_eq!(lines.next(), Some("2|draft,todos|false"));
        assert_eq!(lines.next(), Some("Task|Task|0|false"));
        assert_eq!(lines.next(), Some("APPLICATION_RUNTIME_ERROR|0"));
        assert_eq!(lines.next(), Some("APPLICATION_RUNTIME_ERROR|0"));
        assert_eq!(lines.next(), Some("APPLICATION_RUNTIME_ERROR||99"));
        assert_eq!(lines.next(), Some("Legacy|old|2|1"));
        assert_eq!(lines.next(), Some("Retry migrated|2|none"));
        assert_eq!(lines.next(), Some("APPLICATION_RUNTIME_ERROR||1"));
        assert_eq!(lines.next(), Some("Legacy|Legacy todo|2"));
        assert_eq!(lines.next(), Some("APPLICATION_RUNTIME_ERROR|0"));
        assert_eq!(lines.next(), Some("APPLICATION_RUNTIME_ERROR|false"));
        assert!(lines.next().is_none());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn extern_requests_use_typed_adapters_outside_verified_wasm() {
        let root = std::env::temp_dir().join(format!(
            "synthetix-web-extern-adapter-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join("api.ax"),
            "export extern fn lookup(key: u32) -> u32;\n\
             export extern fn signed_value(key: u32) -> i32;\n\
             export extern fn readiness(key: u32) -> bool;\n",
        )
        .unwrap();
        std::fs::write(
            root.join("main.ax"),
            "import { lookup, signed_value, readiness } from \"./api.ax\";\n\
             contract double(value: u32) -> u32 { ensures result == value *% 2; }\n\
             contract signed_identity(value: i32) -> i32 { ensures result == value; }\n\
             contract bool_identity(value: bool) -> bool { ensures result == value; }\n\
             page Counter {\n\
               state count: number = 0;\n\
               state ready: bool = false;\n\
               show count;\n\
               show ready;\n\
             }\n",
        )
        .unwrap();
        let config = root.join("synthetix.toml");
        std::fs::write(
            &config,
            "[project]\nentry = \"main.ax\"\ntarget = \"web\"\n",
        )
        .unwrap();
        let output = root.join("dist");
        build(&config, &output).unwrap();

        let script = r#"
const fs = require('node:fs');
const vm = require('node:vm');
const bytes = fs.readFileSync(process.env.AXIOM_TEST_WASM);
const wasmExports = WebAssembly.Module.exports(new WebAssembly.Module(bytes)).map((entry) => entry.name);
class Element {
  constructor(tagName) { this.tagName = tagName; this.children = []; this.listeners = {}; this.dataset = {}; this.hidden = true; this.textContent = ''; }
  append(...items) { this.children.push(...items); }
  replaceChildren() { this.children = []; }
  addEventListener(name, callback) { this.listeners[name] = callback; }
}
const root = new Element('main');
const error = new Element('div');
global.window = {};
global.location = { reload() {} };
global.localStorage = { getItem() { return null; }, setItem() {} };
global.document = {
  querySelector(selector) { return selector === '#app' ? root : error; },
  createElement(tagName) { return new Element(tagName); },
};
global.setInterval = () => 0;
global.fetch = async (url) => url === './app.wasm'
  ? { ok: true, arrayBuffer: async () => bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength) }
  : { ok: false, text: async () => '' };
vm.runInThisContext(fs.readFileSync(process.env.AXIOM_TEST_JS, 'utf8'));
(async () => {
  for (let attempt = 0; attempt < 100 && !window.synthetix?.state; attempt++)
    await new Promise((resolve) => setTimeout(resolve, 0));
  if (!window.synthetix?.state) throw new Error('runtime did not instantiate verified WASM');
  const missingAdapter = await window.synthetix.requestExtern('lookup', [7]).catch((failure) => failure);
  const missingAdapterResult = [missingAdapter.category, missingAdapter.source,
    window.synthetix.lastEffect.status, String(error.textContent.includes('COUNTEREXAMPLE'))].join('|');
  let invalidInputAdapterCalled = false;
  window.synthetix.registerEffectAdapter(async () => {
    invalidInputAdapterCalled = true;
    return 1;
  });
  const invalidInput = await window.synthetix.requestExtern('lookup', [true]).catch((failure) => failure);
  const invalidInputResult = [invalidInput.category, invalidInput.source,
    window.synthetix.lastEffect.status, String(error.textContent.includes('COUNTEREXAMPLE')),
    String(invalidInputAdapterCalled)].join('|');
  let loadingSeen = false;
  let calls = 0;
  window.synthetix.registerEffectAdapter(async (request) => {
    calls++;
    loadingSeen ||= window.synthetix.lastEffect.status === 'LOADING';
    if (request.source !== 'api.ax') throw new Error('extern source metadata missing');
    await new Promise((resolve) => setTimeout(resolve, 0));
    if (request.name === 'lookup' && request.responseType === 'u32' && request.inputs[0] === 7) return 5;
    if (request.name === 'signed_value' && request.responseType === 'i32') return -1;
    if (request.name === 'readiness' && request.responseType === 'bool') return true;
    throw new Error('unexpected external request');
  });
  const count = await window.synthetix.requestExtern('lookup', [7], 'count');
  const signed = await window.synthetix.requestExtern('signed_value', [8]);
  const ready = await window.synthetix.requestExtern('readiness', [9], 'ready');
  const output = root.children.filter((node) => node.tagName === 'output').map((node) => node.textContent).join(',');
  const verified = [window.synthetix.call('double', [count]),
    window.synthetix.call('signed_identity', [signed]),
    window.synthetix.call('bool_identity', [ready])].join('|');
  const success = [count, window.synthetix.state.count, signed, ready,
    window.synthetix.state.ready, output, verified, window.synthetix.lastEffect.status,
    String(loadingSeen), String(!wasmExports.includes('lookup')), calls].join('|');

  window.synthetix.registerEffectAdapter(async () => '5');
  const invalid = await window.synthetix.requestExtern('lookup', [1], 'count').catch((failure) => failure);
  const invalidResult = [invalid.category, invalid.source, window.synthetix.lastEffect.status,
    window.synthetix.state.count, String(error.textContent.includes('COUNTEREXAMPLE'))].join('|');

  window.synthetix.registerEffectAdapter(async () => { throw new Error('offline'); });
  const network = await window.synthetix.requestExtern('lookup', [1]).catch((failure) => failure);
  const networkResult = [network.category, network.source, window.synthetix.lastEffect.status,
    String(error.textContent.includes('COUNTEREXAMPLE'))].join('|');

  window.synthetix.registerEffectAdapter(async () => 1n);
  const unsupported = await window.synthetix.requestEffect({ name: 'unsupported', inputs: [] }).catch((failure) => failure);
  const unsupportedResult = [unsupported.category, window.synthetix.lastEffect.status,
    String(error.textContent.includes('COUNTEREXAMPLE'))].join('|');

  let staleResolve;
  let staleSignal;
  let staleRequestId;
  let latestRequestId;
  window.synthetix.registerEffectAdapter(async (request, { signal, requestId }) => {
    if (request.inputs[0] === 10) {
      staleSignal = signal;
      staleRequestId = requestId;
      return new Promise((resolve) => { staleResolve = resolve; });
    }
    latestRequestId = requestId;
    return request.inputs[0];
  });
  const staleRequest = window.synthetix.requestExtern('lookup', [10], 'count');
  for (let attempt = 0; attempt < 100 && !staleResolve; attempt++) await Promise.resolve();
  const latestValue = await window.synthetix.requestExtern('lookup', [20], 'count');
  const staleValue = await staleRequest;
  staleResolve(10);
  await new Promise((resolve) => setTimeout(resolve, 0));
  const staleResult = [latestValue, String(staleValue === undefined), window.synthetix.state.count,
    window.synthetix.lastEffect.response, window.synthetix.lastEffect.status,
    String(staleSignal.aborted), String(Number.isInteger(staleRequestId) && latestRequestId > staleRequestId)].join('|');

  let cancelResolve;
  let cancelSignal;
  window.synthetix.registerEffectAdapter(async (_request, { signal }) => {
    cancelSignal = signal;
    return new Promise((resolve) => { cancelResolve = resolve; });
  });
  const cancelRequest = window.synthetix.requestExtern('lookup', [30], 'count');
  for (let attempt = 0; attempt < 100 && !cancelResolve; attempt++) await Promise.resolve();
  const didCancel = window.synthetix.cancelEffect('count');
  const cancelValue = await cancelRequest;
  cancelResolve(30);
  await new Promise((resolve) => setTimeout(resolve, 0));
  const cancelResult = [String(didCancel), String(cancelValue === undefined), String(cancelSignal.aborted),
    window.synthetix.state.count, window.synthetix.lastEffect.status].join('|');

  let retryCalls = 0;
  window.synthetix.registerEffectAdapter(async (request) => {
    retryCalls++;
    if (retryCalls === 1) throw new Error('temporary failure');
    return request.inputs[0];
  });
  const retryFailure = await window.synthetix.requestExtern('lookup', [40], 'count').catch((failure) => failure);
  const retryStatus = window.synthetix.lastEffect.status;
  const retriedValue = await window.synthetix.retryEffect('count');
  const retryResult = [retryFailure.category, retryStatus, retriedValue,
    window.synthetix.state.count, retryCalls, window.synthetix.lastEffect.status].join('|');

  window.synthetix.state.ready = false;
  window.synthetix.registerEffectAdapter(async (request) => {
    if (request.name === 'lookup') return 50;
    throw new Error('readiness endpoint unavailable');
  });
  const partial = await Promise.allSettled([
    window.synthetix.requestExtern('lookup', [50], 'count'),
    window.synthetix.requestExtern('readiness', [1], 'ready'),
  ]);
  const partialError = partial[1].reason;
  const partialResult = [partial[0].status, partial[1].status, window.synthetix.state.count,
    window.synthetix.state.ready, partialError.category, window.synthetix.lastEffect.status,
    String(error.textContent.includes('COUNTEREXAMPLE'))].join('|');
  process.stdout.write([missingAdapterResult, invalidInputResult, success, invalidResult, networkResult, unsupportedResult,
    staleResult, cancelResult, retryResult, partialResult].join('\n'));
})().catch((error) => { console.error(error); process.exitCode = 1; });
"#;
        let result = std::process::Command::new("node")
            .arg("-e")
            .arg(script)
            .env("AXIOM_TEST_JS", output.join("app.js"))
            .env("AXIOM_TEST_WASM", output.join("app.wasm"))
            .output()
            .expect("Node.js is required to execute the generated browser runtime");
        assert!(
            result.status.success(),
            "extern adapter runtime failed: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        let stdout = String::from_utf8(result.stdout).unwrap();
        let mut lines = stdout.lines();
        assert_eq!(
            lines.next(),
            Some("APPLICATION_RUNTIME_ERROR|api.ax|ERROR|false")
        );
        assert_eq!(
            lines.next(),
            Some("APPLICATION_RUNTIME_ERROR|api.ax|ERROR|false|false")
        );
        assert_eq!(
            lines.next(),
            Some("5|5|-1|true|true|5,true|10|-1|true|SUCCEEDED|true|true|3")
        );
        assert_eq!(
            lines.next(),
            Some("APPLICATION_RUNTIME_ERROR|api.ax|ERROR|5|false")
        );
        assert_eq!(
            lines.next(),
            Some("APPLICATION_RUNTIME_ERROR|api.ax|ERROR|false")
        );
        assert_eq!(lines.next(), Some("APPLICATION_RUNTIME_ERROR|ERROR|false"));
        assert_eq!(lines.next(), Some("20|true|20|20|SUCCEEDED|true|true"));
        assert_eq!(lines.next(), Some("true|true|true|20|CANCELLED"));
        assert_eq!(
            lines.next(),
            Some("APPLICATION_RUNTIME_ERROR|ERROR|40|40|2|SUCCEEDED")
        );
        assert_eq!(
            lines.next(),
            Some("fulfilled|rejected|50|false|APPLICATION_RUNTIME_ERROR|ERROR|false")
        );
        assert!(lines.next().is_none());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn reducer_transitions_are_ordered_deterministic_and_atomic() {
        let root = std::env::temp_dir().join(format!(
            "synthetix-web-transition-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join("main.ax"),
            "contract increment(value: u32) -> u32 { ensures result == value +% 1; }\n\
             contract double(value: u32) -> u32 { ensures result == value *% 2; }\n\
             contract positive(value: u32) -> u32 { requires value > 10; ensures result == value; }\n\
             page Counter {\n\
               state value: number = 1;\n\
               state doubled: number = 0;\n\
             reducer ordered {\n\
               updates value with increment(value);\n\
               updates doubled with double(value);\n\
             }\n\
             reducer step {\n\
               updates value with increment(value);\n\
             }\n\
             reducer failing {\n\
                 updates value with increment(value);\n\
                 updates doubled with positive(value);\n\
               }\n\
               show value;\n\
               show doubled;\n\
               button \"ordered\" runs ordered;\n\
               button \"step\" runs step;\n\
               button \"fail\" runs failing;\n\
             }\n",
        )
        .unwrap();
        let config = root.join("synthetix.toml");
        std::fs::write(
            &config,
            "[project]\nentry = \"main.ax\"\ntarget = \"web\"\n",
        )
        .unwrap();
        let output = root.join("dist");
        build(&config, &output).unwrap();

        let script = r#"
const fs = require('node:fs');
const vm = require('node:vm');
const bytes = fs.readFileSync(process.env.AXIOM_TEST_WASM);
class Element {
  constructor(tagName) { this.tagName = tagName; this.children = []; this.listeners = {}; this.dataset = {}; this.textContent = ''; }
  append(...items) { this.children.push(...items); }
  replaceChildren() { this.children = []; }
  addEventListener(name, callback) { this.listeners[name] = callback; }
  click() { this.listeners.click?.(); }
}
const root = new Element('main');
const error = new Element('div');
global.window = {};
global.location = { reload() {} };
global.localStorage = { getItem() { return null; }, setItem() {} };
global.document = {
  querySelector(selector) { return selector === '#app' ? root : error; },
  createElement(tagName) { return new Element(tagName); },
};
global.setInterval = () => 0;
global.fetch = async (url) => url === './app.wasm'
  ? { ok: true, arrayBuffer: async () => bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength) }
  : { ok: false, text: async () => '' };
vm.runInThisContext(fs.readFileSync(process.env.AXIOM_TEST_JS, 'utf8'));
(async () => {
  for (let attempt = 0; attempt < 100 && !window.synthetix; attempt++)
    await new Promise((resolve) => setTimeout(resolve, 0));
  if (!window.synthetix) throw new Error('generated runtime did not instantiate verified WASM');
  if (window.synthetix.state.value !== 1 || window.synthetix.state.doubled !== 0)
    throw new Error('unexpected initial state');
  const click = (name) => root.children.find((node) => node.tagName === 'button' && node.textContent === name).click();
  click('ordered');
  const first = JSON.stringify(window.synthetix.lastTransition);
  const rendered = root.children.filter((node) => node.tagName === 'output').map((node) => node.textContent);
  window.synthetix.state.value = 1;
  window.synthetix.state.doubled = 0;
  click('ordered');
  const deterministic = first === JSON.stringify(window.synthetix.lastTransition);
  const success = [window.synthetix.state.value, window.synthetix.state.doubled, rendered[0], rendered[1], String(deterministic), window.synthetix.lastTransition.event, window.synthetix.lastTransition.status, window.synthetix.lastTransition.updatesApplied].join('|');

  window.synthetix.state.value = 1;
  window.synthetix.state.doubled = 0;
  click('ordered');
  click('step');
  const firstSequence = [window.synthetix.state.value, window.synthetix.state.doubled].join('|');
  window.synthetix.state.value = 1;
  window.synthetix.state.doubled = 0;
  click('step');
  click('ordered');
  const reversedSequence = [window.synthetix.state.value, window.synthetix.state.doubled].join('|');

  window.synthetix.state.value = 1;
  window.synthetix.state.doubled = 0;
  click('fail');
  const transition = window.synthetix.lastTransition;
  const failure = [window.synthetix.state.value, window.synthetix.state.doubled, transition.status, transition.updatesApplied, transition.failedUpdate, transition.error.category, transition.error.source, transition.error.contract, String(error.textContent.includes('COUNTEREXAMPLE'))].join('|');
  process.stdout.write([success, [firstSequence, reversedSequence].join('|'), failure].join('\n'));
})().catch((error) => { console.error(error); process.exitCode = 1; });
"#;
        let result = std::process::Command::new("node")
            .arg("-e")
            .arg(script)
            .env("AXIOM_TEST_JS", output.join("app.js"))
            .env("AXIOM_TEST_WASM", output.join("app.wasm"))
            .output()
            .expect("Node.js is required to execute the generated browser runtime");
        assert!(
            result.status.success(),
            "web transition runtime failed: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        let stdout = String::from_utf8(result.stdout).unwrap();
        let mut lines = stdout.lines();
        assert_eq!(lines.next(), Some("2|4|2|4|true|ordered|APPLIED|2"));
        assert_eq!(lines.next(), Some("3|4|3|6"));
        assert_eq!(
            lines.next(),
            Some("1|0|FAILED|0|1|APPLICATION_TRANSITION_FAILURE|main.ax|positive|false")
        );
        assert!(lines.next().is_none());
        std::fs::remove_dir_all(root).unwrap();
    }

    fn point_contract(
        contract: &axiom_parser::Contract,
        inputs: &[u32; 2],
        expected: u32,
    ) -> axiom_parser::Contract {
        let mut point = contract.clone();
        point.requires = contract
            .params
            .iter()
            .enumerate()
            .map(|(index, param)| {
                let value = match &param.ty {
                    Type::Bool => SpecExpr::Bool(inputs[index] != 0),
                    _ => SpecExpr::Number(inputs[index]),
                };
                SpecExpr::Binary(
                    BinaryOp::Eq,
                    Box::new(SpecExpr::Ident(param.name.clone())),
                    Box::new(value),
                )
            })
            .collect();
        let expected = if contract.return_type == Type::Bool {
            SpecExpr::Bool(expected != 0)
        } else {
            SpecExpr::Number(expected)
        };
        point.ensures = vec![SpecExpr::Binary(
            BinaryOp::Eq,
            Box::new(SpecExpr::Ident("result".to_string())),
            Box::new(expected),
        )];
        point
    }
}
