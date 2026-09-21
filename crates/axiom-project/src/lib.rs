//! Project loading and module linking for Synthetix v2.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use axiom_parser::{Contract, Import, Module, Page, PageElement, SpecExpr};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectConfig {
    pub root: PathBuf,
    pub entry: PathBuf,
    pub target: String,
}

#[derive(Debug, Clone)]
pub struct LinkedContract {
    pub source: PathBuf,
    pub contract: Contract,
}

#[derive(Debug, Clone)]
pub struct LinkedPage {
    pub source: PathBuf,
    pub page: Page,
}

#[derive(Debug, Clone)]
pub struct Project {
    pub config: ProjectConfig,
    pub contracts: Vec<LinkedContract>,
    pub pages: Vec<LinkedPage>,
    pub files: Vec<PathBuf>,
}

type ContractKey = (PathBuf, String);

#[derive(Debug, Clone)]
struct Definition {
    contract: Contract,
}

#[derive(Debug, Clone)]
struct ModuleInfo {
    path: PathBuf,
    module: Module,
}

/// Loads a project from `synthetix.toml` and links every imported module.
pub fn load(config_path: impl AsRef<Path>) -> Result<Project, String> {
    let config_path = config_path.as_ref();
    let config = parse_config(config_path)?;
    let mut modules = Vec::new();
    let mut visiting = Vec::new();
    visit_module(&config.entry, &mut modules, &mut visiting)?;
    link_project(config, modules)
}

/// Finds `synthetix.toml` in the supplied directory or one of its parents.
pub fn find_config(start: impl AsRef<Path>) -> Result<PathBuf, String> {
    let mut current = start.as_ref().to_path_buf();
    if current.is_file() {
        current = current
            .parent()
            .ok_or_else(|| "cannot determine project directory".to_string())?
            .to_path_buf();
    }

    loop {
        let candidate = current.join("synthetix.toml");
        if candidate.is_file() {
            return Ok(candidate);
        }
        if !current.pop() {
            break;
        }
    }

    Err(format!(
        "cannot find synthetix.toml from {}",
        start.as_ref().display()
    ))
}

fn parse_config(path: &Path) -> Result<ProjectConfig, String> {
    let contents = fs::read_to_string(path)
        .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    let root = path
        .parent()
        .ok_or_else(|| format!("cannot determine root for {}", path.display()))?
        .to_path_buf();
    let mut in_project = false;
    let mut entry = None;
    let mut target = None;

    for (index, raw_line) in contents.lines().enumerate() {
        let line = raw_line.split('#').next().unwrap_or_default().trim();
        if line.is_empty() {
            continue;
        }
        if line.starts_with('[') {
            in_project = line == "[project]";
            continue;
        }
        if !in_project {
            continue;
        }

        let Some((key, value)) = line.split_once('=') else {
            return Err(format!(
                "{}:{}: expected key = value",
                path.display(),
                index + 1
            ));
        };
        let value = value.trim().trim_matches('"').trim_matches('\'');
        match key.trim() {
            "entry" => entry = Some(value.to_string()),
            "target" => target = Some(value.to_string()),
            other => {
                return Err(format!(
                    "{}:{}: unknown project setting: {other}",
                    path.display(),
                    index + 1
                ));
            }
        }
    }

    let entry = entry.ok_or_else(|| {
        format!(
            "{}: [project] entry is required; example: entry = \"main.ax\"",
            path.display()
        )
    })?;
    let target = target.unwrap_or_else(|| "web".to_string());
    if target != "web" {
        return Err(format!(
            "unsupported target '{target}'; v2 currently supports only web"
        ));
    }

    let entry = root.join(entry);
    Ok(ProjectConfig {
        root,
        entry,
        target,
    })
}

fn visit_module(
    path: &Path,
    modules: &mut Vec<ModuleInfo>,
    visiting: &mut Vec<PathBuf>,
) -> Result<(), String> {
    let path = canonical(path)?;
    if modules.iter().any(|module| module.path == path) {
        return Ok(());
    }
    if visiting.contains(&path) {
        let cycle = visiting
            .iter()
            .chain(std::iter::once(&path))
            .map(|item| item.display().to_string())
            .collect::<Vec<_>>()
            .join(" -> ");
        return Err(format!("circular import: {cycle}"));
    }

    visiting.push(path.clone());
    let source = fs::read_to_string(&path)
        .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    let module = axiom_parser::parse_module(&source)
        .map_err(|error| format!("{}: {error}", path.display()))?;
    let imports = module.imports.clone();
    for import in &imports {
        let imported = resolve_import(&path, import)?;
        visit_module(&imported, modules, visiting)?;
    }
    visiting.pop();
    modules.push(ModuleInfo { path, module });
    Ok(())
}

fn resolve_import(source: &Path, import: &Import) -> Result<PathBuf, String> {
    let mut path = source
        .parent()
        .ok_or_else(|| format!("cannot determine parent of {}", source.display()))?
        .join(&import.path);
    if path.extension().is_none() {
        path.set_extension("ax");
    }
    if !path.is_file() {
        return Err(format!(
            "{}:{}: imported file not found: {}",
            source.display(),
            import.span.line,
            path.display()
        ));
    }
    canonical(&path)
}

fn canonical(path: &Path) -> Result<PathBuf, String> {
    fs::canonicalize(path).map_err(|error| format!("cannot resolve {}: {error}", path.display()))
}

fn link_project(config: ProjectConfig, modules: Vec<ModuleInfo>) -> Result<Project, String> {
    let mut definitions = HashMap::<ContractKey, Definition>::new();
    let mut module_paths = HashSet::new();
    for module in &modules {
        module_paths.insert(module.path.clone());
        for contract in &module.module.contracts {
            let key = (module.path.clone(), contract.name.clone());
            if definitions
                .insert(
                    key.clone(),
                    Definition {
                        contract: contract.clone(),
                    },
                )
                .is_some()
            {
                return Err(format!(
                    "{}: duplicate contract: {}",
                    module.path.display(),
                    contract.name
                ));
            }
        }
    }

    let mut scopes = HashMap::<PathBuf, HashMap<String, ContractKey>>::new();
    for module in &modules {
        let scope = scopes.entry(module.path.clone()).or_default();
        for contract in &module.module.contracts {
            scope.insert(
                contract.name.clone(),
                (module.path.clone(), contract.name.clone()),
            );
        }
        for import in &module.module.imports {
            let target = resolve_import(&module.path, import)?;
            if !module_paths.contains(&target) {
                return Err(format!(
                    "{}: imported module was not loaded",
                    target.display()
                ));
            }
            for name in &import.names {
                let key = (target.clone(), name.clone());
                let Some(definition) = definitions.get(&key) else {
                    return Err(format!(
                        "{}:{}: imported contract not found: {name}",
                        module.path.display(),
                        import.span.line
                    ));
                };
                if !definition.contract.exported {
                    return Err(format!(
                        "{}:{}: contract {name} is not exported",
                        module.path.display(),
                        import.span.line
                    ));
                }
                if scope.insert(name.clone(), key).is_some() {
                    return Err(format!(
                        "{}:{}: duplicate imported name: {name}",
                        module.path.display(),
                        import.span.line
                    ));
                }
            }
        }
    }

    let mut cache = HashMap::new();
    let mut contracts = Vec::new();
    for module in &modules {
        for contract in &module.module.contracts {
            let key = (module.path.clone(), contract.name.clone());
            let linked = expand_contract(&key, &definitions, &scopes, &mut cache, &mut Vec::new())
                .map_err(|error| format!("{}: {error}", module.path.display()))?;
            axiom_parser::validate(&linked)
                .map_err(|error| format!("{}: {error}", module.path.display()))?;
            contracts.push(LinkedContract {
                source: module.path.clone(),
                contract: linked,
            });
        }
    }

    let mut pages = Vec::new();
    for module in &modules {
        for page in &module.module.pages {
            validate_page(page, &module.path, scopes.get(&module.path), &definitions)?;
            pages.push(LinkedPage {
                source: module.path.clone(),
                page: page.clone(),
            });
        }
    }

    Ok(Project {
        config,
        contracts,
        pages,
        files: modules.into_iter().map(|module| module.path).collect(),
    })
}

fn expand_contract(
    key: &ContractKey,
    definitions: &HashMap<ContractKey, Definition>,
    scopes: &HashMap<PathBuf, HashMap<String, ContractKey>>,
    cache: &mut HashMap<ContractKey, Contract>,
    stack: &mut Vec<ContractKey>,
) -> Result<Contract, String> {
    if let Some(contract) = cache.get(key) {
        return Ok(contract.clone());
    }
    if stack.contains(key) {
        return Err(format!("circular contract call: {}", key.1));
    }
    let Some(definition) = definitions.get(key) else {
        return Err(format!("contract not found: {}", key.1));
    };
    let scope = scopes
        .get(&key.0)
        .ok_or_else(|| format!("missing module scope: {}", key.0.display()))?;
    stack.push(key.clone());
    let mut linked = definition.contract.clone();
    linked.requires = linked
        .requires
        .iter()
        .map(|expr| expand_expr(expr, scope, definitions, scopes, cache, stack))
        .collect::<Result<_, _>>()?;
    linked.ensures = linked
        .ensures
        .iter()
        .map(|expr| expand_expr(expr, scope, definitions, scopes, cache, stack))
        .collect::<Result<_, _>>()?;
    stack.pop();
    cache.insert(key.clone(), linked.clone());
    Ok(linked)
}

fn expand_expr(
    expr: &SpecExpr,
    scope: &HashMap<String, ContractKey>,
    definitions: &HashMap<ContractKey, Definition>,
    scopes: &HashMap<PathBuf, HashMap<String, ContractKey>>,
    cache: &mut HashMap<ContractKey, Contract>,
    stack: &mut Vec<ContractKey>,
) -> Result<SpecExpr, String> {
    match expr {
        SpecExpr::Ident(_) | SpecExpr::Number(_) | SpecExpr::Bool(_) => Ok(expr.clone()),
        SpecExpr::Let(name, value, body) => Ok(SpecExpr::Let(
            name.clone(),
            Box::new(expand_expr(
                value,
                scope,
                definitions,
                scopes,
                cache,
                stack,
            )?),
            Box::new(expand_expr(body, scope, definitions, scopes, cache, stack)?),
        )),
        SpecExpr::If(condition, then_branch, else_branch) => Ok(SpecExpr::If(
            Box::new(expand_expr(
                condition,
                scope,
                definitions,
                scopes,
                cache,
                stack,
            )?),
            Box::new(expand_expr(
                then_branch,
                scope,
                definitions,
                scopes,
                cache,
                stack,
            )?),
            Box::new(expand_expr(
                else_branch,
                scope,
                definitions,
                scopes,
                cache,
                stack,
            )?),
        )),
        SpecExpr::Binary(op, lhs, rhs) => Ok(SpecExpr::Binary(
            op.clone(),
            Box::new(expand_expr(lhs, scope, definitions, scopes, cache, stack)?),
            Box::new(expand_expr(rhs, scope, definitions, scopes, cache, stack)?),
        )),
        SpecExpr::Call(name, args) => {
            let Some(target) = scope.get(name) else {
                return Err(format!("unknown imported contract: {name}"));
            };
            let Some(definition) = definitions.get(target) else {
                return Err(format!("contract not found: {name}"));
            };
            if !definition.contract.requires.is_empty() {
                return Err(format!(
                    "contract calls with requires are not supported yet: {name}"
                ));
            }
            if definition.contract.params.len() != args.len() {
                return Err(format!(
                    "contract {name} expects {} arguments, got {}",
                    definition.contract.params.len(),
                    args.len()
                ));
            }
            let args = args
                .iter()
                .map(|arg| expand_expr(arg, scope, definitions, scopes, cache, stack))
                .collect::<Result<Vec<_>, _>>()?;
            let callee = expand_contract(target, definitions, scopes, cache, stack)?;
            let body = result_body(&callee)?;
            let substitutions = callee
                .params
                .iter()
                .map(|param| param.name.clone())
                .zip(args)
                .collect::<HashMap<_, _>>();
            Ok(substitute(body, &substitutions))
        }
    }
}

fn result_body(contract: &Contract) -> Result<&SpecExpr, String> {
    if contract.ensures.len() != 1 {
        return Err(format!(
            "contract {} must have exactly one ensures equality when called",
            contract.name
        ));
    }
    let SpecExpr::Binary(axiom_parser::BinaryOp::Eq, lhs, rhs) = &contract.ensures[0] else {
        return Err(format!(
            "contract {} must return a result equality when called",
            contract.name
        ));
    };
    if matches!(lhs.as_ref(), SpecExpr::Ident(name) if name == "result") {
        Ok(rhs)
    } else if matches!(rhs.as_ref(), SpecExpr::Ident(name) if name == "result") {
        Ok(lhs)
    } else {
        Err(format!(
            "contract {} must compare result when called",
            contract.name
        ))
    }
}

fn substitute(expr: &SpecExpr, values: &HashMap<String, SpecExpr>) -> SpecExpr {
    match expr {
        SpecExpr::Ident(name) => values
            .get(name)
            .cloned()
            .unwrap_or_else(|| SpecExpr::Ident(name.clone())),
        SpecExpr::Number(_) | SpecExpr::Bool(_) => expr.clone(),
        SpecExpr::Binary(op, lhs, rhs) => SpecExpr::Binary(
            op.clone(),
            Box::new(substitute(lhs, values)),
            Box::new(substitute(rhs, values)),
        ),
        SpecExpr::Let(name, value, body) => SpecExpr::Let(
            name.clone(),
            Box::new(substitute(value, values)),
            Box::new(substitute(body, values)),
        ),
        SpecExpr::If(condition, then_branch, else_branch) => SpecExpr::If(
            Box::new(substitute(condition, values)),
            Box::new(substitute(then_branch, values)),
            Box::new(substitute(else_branch, values)),
        ),
        SpecExpr::Call(name, args) => SpecExpr::Call(
            name.clone(),
            args.iter().map(|arg| substitute(arg, values)).collect(),
        ),
    }
}

fn validate_page(
    page: &Page,
    source: &Path,
    scope: Option<&HashMap<String, ContractKey>>,
    definitions: &HashMap<ContractKey, Definition>,
) -> Result<(), String> {
    let mut states = HashMap::new();
    for state in &page.states {
        if states
            .insert(state.name.clone(), state.ty.clone())
            .is_some()
        {
            return Err(format!(
                "{}: duplicate page state: {}",
                source.display(),
                state.name
            ));
        }
    }
    for state in &page.states {
        validate_page_expr(&state.initial, &states, source)?;
    }

    for element in &page.elements {
        match element {
            PageElement::Show { state } => {
                if !states.contains_key(state) {
                    return Err(format!(
                        "{}: page {} shows unknown state: {state}",
                        source.display(),
                        page.name
                    ));
                }
            }
            PageElement::Button {
                state,
                function,
                args,
                ..
            } => {
                let Some(state_type) = states.get(state) else {
                    return Err(format!(
                        "{}: button updates unknown state: {state}",
                        source.display()
                    ));
                };
                let Some(scope) = scope else {
                    return Err(format!("{}: missing module scope", source.display()));
                };
                let Some(key) = scope.get(function) else {
                    return Err(format!(
                        "{}: page calls unknown contract: {function}",
                        source.display()
                    ));
                };
                let definition = definitions.get(key).ok_or_else(|| {
                    format!("{}: contract not found: {function}", source.display())
                })?;
                if definition.contract.params.len() != args.len() {
                    return Err(format!(
                        "{}: page call {} expects {} arguments, got {}",
                        source.display(),
                        function,
                        definition.contract.params.len(),
                        args.len()
                    ));
                }
                if &definition.contract.return_type != state_type {
                    return Err(format!(
                        "{}: page state {} and contract {} have different types",
                        source.display(),
                        state,
                        function
                    ));
                }
                for arg in args {
                    validate_page_expr(arg, &states, source)?;
                }
            }
        }
    }
    Ok(())
}

fn validate_page_expr(
    expr: &SpecExpr,
    states: &HashMap<String, axiom_parser::Type>,
    source: &Path,
) -> Result<(), String> {
    match expr {
        SpecExpr::Ident(name) if !states.contains_key(name) => Err(format!(
            "{}: page expression references unknown state: {name}",
            source.display()
        )),
        SpecExpr::Ident(_) | SpecExpr::Number(_) | SpecExpr::Bool(_) => Ok(()),
        SpecExpr::Let(_, value, body) => {
            validate_page_expr(value, states, source)?;
            validate_page_expr(body, states, source)
        }
        SpecExpr::If(condition, then_branch, else_branch) => {
            validate_page_expr(condition, states, source)?;
            validate_page_expr(then_branch, states, source)?;
            validate_page_expr(else_branch, states, source)
        }
        SpecExpr::Binary(_, lhs, rhs) => {
            validate_page_expr(lhs, states, source)?;
            validate_page_expr(rhs, states, source)
        }
        SpecExpr::Call(name, _) => Err(format!(
            "{}: page expressions cannot call contract {name}",
            source.display()
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn links_imported_contracts_without_a_folder_convention() {
        let root = std::env::temp_dir().join(format!(
            "synthetix-project-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        fs::write(
            root.join("math.ax"),
            "export contract add(x: u32, y: u32) -> u32 { ensures result == x +% y; }",
        )
        .unwrap();
        fs::write(
            root.join("entry.ax"),
            "import { add } from \"./math.ax\"; contract total(x: u32, y: u32) -> u32 { ensures result == add(x, y); }",
        )
        .unwrap();
        fs::write(
            root.join("synthetix.toml"),
            "[project]\nentry = \"entry.ax\"\ntarget = \"web\"\n",
        )
        .unwrap();

        let project = load(root.join("synthetix.toml")).unwrap();
        let total = project
            .contracts
            .iter()
            .find(|item| item.contract.name == "total")
            .unwrap();
        let SpecExpr::Binary(axiom_parser::BinaryOp::Eq, _, rhs) = &total.contract.ensures[0]
        else {
            panic!("expected result equality")
        };
        assert!(matches!(
            rhs.as_ref(),
            SpecExpr::Binary(axiom_parser::BinaryOp::AddWrap, _, _)
        ));
        assert_eq!(project.files.len(), 2);

        fs::remove_dir_all(root).unwrap();
    }
}
