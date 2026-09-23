//! Project loading and module linking for Synthetix v2.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use axiom_ami::Expr;
use axiom_parser::{
    Component, Contract, Import, Module, Page, PageElement, RecordDef, Reducer, SpecExpr, Type,
};

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
    /// Original contract summaries retained for compositional verification.
    pub verification_summaries: Vec<LinkedContract>,
    /// Imported signatures keyed by the importing module and local name.
    pub verification_callees: HashMap<PathBuf, HashMap<String, Contract>>,
    pub pages: Vec<LinkedPage>,
    pub files: Vec<PathBuf>,
}

impl Project {
    /// Verifies a linked contract against imported signature summaries.
    pub fn verify_compositional(
        &self,
        linked: &LinkedContract,
        candidate: &Expr,
    ) -> Result<axiom_verify::CompositionalProof, String> {
        let summary = self
            .verification_summaries
            .iter()
            .find(|item| item.source == linked.source && item.contract.name == linked.contract.name)
            .ok_or_else(|| format!("missing verification summary: {}", linked.contract.name))?;
        let callees = self
            .verification_callees
            .get(&linked.source)
            .cloned()
            .unwrap_or_default();
        axiom_verify::verify_compositional(&summary.contract, candidate, &callees)
    }
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
    let mut records = HashMap::<String, RecordDef>::new();
    let mut components = HashMap::<String, Component>::new();
    for module in &modules {
        module_paths.insert(module.path.clone());
        for record in &module.module.records {
            if records
                .insert(record.name.clone(), record.clone())
                .is_some()
            {
                return Err(format!(
                    "{}: duplicate record: {}",
                    module.path.display(),
                    record.name
                ));
            }
        }
        for component in &module.module.components {
            if components
                .insert(component.name.clone(), component.clone())
                .is_some()
            {
                return Err(format!(
                    "{}: duplicate component: {}",
                    module.path.display(),
                    component.name
                ));
            }
        }
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
    let verification_callees = scopes
        .iter()
        .map(|(source, scope)| {
            let callees = scope
                .iter()
                .filter_map(|(name, key)| {
                    definitions
                        .get(key)
                        .map(|definition| (name.clone(), definition.contract.clone()))
                })
                .collect();
            (source.clone(), callees)
        })
        .collect();
    let mut contracts = Vec::new();
    let mut verification_summaries = Vec::new();
    for module in &modules {
        for contract in &module.module.contracts {
            let key = (module.path.clone(), contract.name.clone());
            verification_summaries.push(LinkedContract {
                source: module.path.clone(),
                contract: contract.clone(),
            });
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
            let mut page = page.clone();
            expand_components(&mut page, &components, &module.path)?;
            validate_page(
                &page,
                &module.path,
                scopes.get(&module.path),
                &definitions,
                &records,
            )?;
            pages.push(LinkedPage {
                source: module.path.clone(),
                page,
            });
        }
    }

    Ok(Project {
        config,
        contracts,
        verification_summaries,
        verification_callees,
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
        SpecExpr::Ident(_) | SpecExpr::Number(_) | SpecExpr::Bool(_) | SpecExpr::Text(_) => {
            Ok(expr.clone())
        }
        SpecExpr::List(values) => Ok(SpecExpr::List(
            values
                .iter()
                .map(|value| expand_expr(value, scope, definitions, scopes, cache, stack))
                .collect::<Result<_, _>>()?,
        )),
        SpecExpr::Record { name, fields } => Ok(SpecExpr::Record {
            name: name.clone(),
            fields: fields
                .iter()
                .map(|(field, value)| {
                    Ok((
                        field.clone(),
                        expand_expr(value, scope, definitions, scopes, cache, stack)?,
                    ))
                })
                .collect::<Result<_, String>>()?,
        }),
        SpecExpr::Field(value, field) => Ok(SpecExpr::Field(
            Box::new(expand_expr(
                value,
                scope,
                definitions,
                scopes,
                cache,
                stack,
            )?),
            field.clone(),
        )),
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
        SpecExpr::Text(_) | SpecExpr::List(_) | SpecExpr::Record { .. } | SpecExpr::Field(_, _) => {
            expr.clone()
        }
    }
}

fn expand_components(
    page: &mut Page,
    components: &HashMap<String, Component>,
    source: &Path,
) -> Result<(), String> {
    page.elements = expand_elements(&page.elements, components, &mut Vec::new(), source)?;
    Ok(())
}

fn expand_elements(
    elements: &[PageElement],
    components: &HashMap<String, Component>,
    stack: &mut Vec<String>,
    source: &Path,
) -> Result<Vec<PageElement>, String> {
    let mut expanded = Vec::new();
    for element in elements {
        match element {
            PageElement::Use { name } => {
                if stack.contains(name) {
                    return Err(format!(
                        "{}: circular component use: {}",
                        source.display(),
                        name
                    ));
                }
                let component = components
                    .get(name)
                    .ok_or_else(|| format!("{}: unknown component: {name}", source.display()))?;
                stack.push(name.clone());
                expanded.extend(expand_elements(
                    &component.elements,
                    components,
                    stack,
                    source,
                )?);
                stack.pop();
            }
            PageElement::List {
                state,
                item,
                elements,
            } => expanded.push(PageElement::List {
                state: state.clone(),
                item: item.clone(),
                elements: expand_elements(elements, components, stack, source)?,
            }),
            other => expanded.push(other.clone()),
        }
    }
    Ok(expanded)
}

fn validate_page(
    page: &Page,
    source: &Path,
    scope: Option<&HashMap<String, ContractKey>>,
    definitions: &HashMap<ContractKey, Definition>,
    records: &HashMap<String, RecordDef>,
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
        let initial_type =
            page_expr_type(&state.initial, &states, records, &HashMap::new(), source)?;
        if !types_compatible(&initial_type, &state.ty) {
            return Err(format!(
                "{}: state {} initial value has type {:?}, expected {:?}",
                source.display(),
                state.name,
                initial_type,
                state.ty
            ));
        }
    }

    for name in &page.persisted {
        if !states.contains_key(name) {
            return Err(format!(
                "{}: persist references unknown state: {name}",
                source.display()
            ));
        }
    }

    let mut reducer_names = HashSet::new();
    for reducer in &page.reducers {
        if !reducer_names.insert(reducer.name.as_str()) {
            return Err(format!(
                "{}: duplicate reducer: {}",
                source.display(),
                reducer.name
            ));
        }
        for update in &reducer.updates {
            validate_call(
                &update.state,
                &update.function,
                &update.args,
                &states,
                records,
                &HashMap::new(),
                scope,
                definitions,
                source,
            )?;
        }
    }

    validate_elements(
        &page.elements,
        &states,
        records,
        &HashMap::new(),
        scope,
        definitions,
        &page.reducers,
        source,
    )?;
    Ok(())
}

fn page_expr_type(
    expr: &SpecExpr,
    states: &HashMap<String, axiom_parser::Type>,
    records: &HashMap<String, RecordDef>,
    locals: &HashMap<String, Type>,
    source: &Path,
) -> Result<axiom_parser::Type, String> {
    match expr {
        SpecExpr::Ident(name) => locals
            .get(name)
            .or_else(|| states.get(name))
            .cloned()
            .ok_or_else(|| {
                format!(
                    "{}: page expression references unknown state: {name}",
                    source.display()
                )
            }),
        SpecExpr::Number(_) => Ok(Type::U32),
        SpecExpr::Bool(_) => Ok(Type::Bool),
        SpecExpr::Text(_) => Ok(Type::Text),
        SpecExpr::List(values) => {
            let Some(first) = values.first() else {
                return Ok(Type::List(Box::new(Type::Unknown)));
            };
            let item = page_expr_type(first, states, records, locals, source)?;
            for value in &values[1..] {
                let value_type = page_expr_type(value, states, records, locals, source)?;
                if !types_compatible(&value_type, &item) {
                    return Err(format!(
                        "{}: list values have incompatible types",
                        source.display()
                    ));
                }
            }
            Ok(Type::List(Box::new(item)))
        }
        SpecExpr::Record { name, fields } => {
            let definition = records
                .get(name)
                .ok_or_else(|| format!("{}: unknown record: {name}", source.display()))?;
            if fields.len() != definition.fields.len() {
                return Err(format!(
                    "{}: record {} expects {} fields, got {}",
                    source.display(),
                    name,
                    definition.fields.len(),
                    fields.len()
                ));
            }
            for field in &definition.fields {
                let Some((_, value)) = fields.iter().find(|(name, _)| name == &field.name) else {
                    return Err(format!(
                        "{}: record {} is missing field {}",
                        source.display(),
                        name,
                        field.name
                    ));
                };
                let actual = page_expr_type(value, states, records, locals, source)?;
                if !types_compatible(&actual, &field.ty) {
                    return Err(format!(
                        "{}: field {} has type {:?}, expected {:?}",
                        source.display(),
                        field.name,
                        actual,
                        field.ty
                    ));
                }
            }
            Ok(Type::Record(name.clone()))
        }
        SpecExpr::Field(value, field) => {
            let Type::Record(record_name) = page_expr_type(value, states, records, locals, source)?
            else {
                return Err(format!("{}: only records have fields", source.display()));
            };
            let definition = records
                .get(&record_name)
                .ok_or_else(|| format!("{}: unknown record: {record_name}", source.display()))?;
            definition
                .fields
                .iter()
                .find(|candidate| candidate.name == *field)
                .map(|candidate| candidate.ty.clone())
                .ok_or_else(|| {
                    format!(
                        "{}: unknown field {}.{}",
                        source.display(),
                        record_name,
                        field
                    )
                })
        }
        SpecExpr::Let(name, value, body) => {
            let value_type = page_expr_type(value, states, records, locals, source)?;
            let mut nested = locals.clone();
            nested.insert(name.clone(), value_type);
            page_expr_type(body, states, records, &nested, source)
        }
        SpecExpr::If(condition, then_branch, else_branch) => {
            if page_expr_type(condition, states, records, locals, source)? != Type::Bool {
                return Err(format!(
                    "{}: if condition must be boolean",
                    source.display()
                ));
            }
            let then_type = page_expr_type(then_branch, states, records, locals, source)?;
            let else_type = page_expr_type(else_branch, states, records, locals, source)?;
            if !types_compatible(&then_type, &else_type) {
                return Err(format!(
                    "{}: if branches must have the same type",
                    source.display()
                ));
            }
            Ok(then_type)
        }
        SpecExpr::Binary(op, lhs, rhs) => {
            let lhs_type = page_expr_type(lhs, states, records, locals, source)?;
            let rhs_type = page_expr_type(rhs, states, records, locals, source)?;
            match op {
                axiom_parser::BinaryOp::Eq | axiom_parser::BinaryOp::Ne
                    if types_compatible(&lhs_type, &rhs_type)
                        || page_numeric_type(lhs, &lhs_type, rhs, &rhs_type).is_some() =>
                {
                    Ok(Type::Bool)
                }
                axiom_parser::BinaryOp::And | axiom_parser::BinaryOp::Or
                    if lhs_type == Type::Bool && rhs_type == Type::Bool =>
                {
                    Ok(Type::Bool)
                }
                axiom_parser::BinaryOp::AddWrap
                | axiom_parser::BinaryOp::SubWrap
                | axiom_parser::BinaryOp::MulWrap
                | axiom_parser::BinaryOp::Shl
                | axiom_parser::BinaryOp::BitAnd
                | axiom_parser::BinaryOp::BitOr
                | axiom_parser::BinaryOp::BitXor
                    if let Some(ty) = page_numeric_type(lhs, &lhs_type, rhs, &rhs_type) =>
                {
                    Ok(ty)
                }
                axiom_parser::BinaryOp::Lt
                | axiom_parser::BinaryOp::Le
                | axiom_parser::BinaryOp::Gt
                | axiom_parser::BinaryOp::Ge
                    if page_numeric_type(lhs, &lhs_type, rhs, &rhs_type).is_some() =>
                {
                    Ok(Type::Bool)
                }
                _ => Err(format!(
                    "{}: invalid page expression types",
                    source.display()
                )),
            }
        }
        SpecExpr::Call(name, _) => Err(format!(
            "{}: page expressions cannot call contract {name}",
            source.display()
        )),
    }
}

fn page_numeric_type(
    lhs: &SpecExpr,
    lhs_type: &Type,
    rhs: &SpecExpr,
    rhs_type: &Type,
) -> Option<Type> {
    match (lhs_type, rhs_type) {
        (Type::U32, Type::U32) => Some(Type::U32),
        (Type::I32, Type::I32) => Some(Type::I32),
        (Type::I32, Type::U32) if matches!(rhs, SpecExpr::Number(_)) => Some(Type::I32),
        (Type::U32, Type::I32) if matches!(lhs, SpecExpr::Number(_)) => Some(Type::I32),
        _ => None,
    }
}

fn types_compatible(actual: &Type, expected: &Type) -> bool {
    if actual == &Type::Unknown || expected == &Type::Unknown {
        return true;
    }
    match (actual, expected) {
        (Type::List(actual), Type::List(expected)) => types_compatible(actual, expected),
        _ => actual == expected,
    }
}

fn types_compatible_expr(actual: &Type, expected: &Type, expr: &SpecExpr) -> bool {
    types_compatible(actual, expected)
        || (expected == &Type::I32 && actual == &Type::U32 && matches!(expr, SpecExpr::Number(_)))
}

fn path_type(
    path: &str,
    states: &HashMap<String, Type>,
    records: &HashMap<String, RecordDef>,
    locals: &HashMap<String, Type>,
    source: &Path,
) -> Result<Type, String> {
    let mut parts = path.split('.');
    let root = parts.next().unwrap_or_default();
    let mut current = locals
        .get(root)
        .or_else(|| states.get(root))
        .cloned()
        .ok_or_else(|| format!("{}: unknown state or item: {root}", source.display()))?;
    for field in parts {
        let Type::Record(record_name) = current else {
            return Err(format!(
                "{}: {} has no field {}",
                source.display(),
                root,
                field
            ));
        };
        let definition = records
            .get(&record_name)
            .ok_or_else(|| format!("{}: unknown record: {record_name}", source.display()))?;
        current = definition
            .fields
            .iter()
            .find(|candidate| candidate.name == field)
            .map(|candidate| candidate.ty.clone())
            .ok_or_else(|| {
                format!(
                    "{}: unknown field {}.{}",
                    source.display(),
                    record_name,
                    field
                )
            })?;
    }
    Ok(current)
}

#[allow(clippy::too_many_arguments)]
fn validate_call(
    state: &str,
    function: &str,
    args: &[SpecExpr],
    states: &HashMap<String, Type>,
    records: &HashMap<String, RecordDef>,
    locals: &HashMap<String, Type>,
    scope: Option<&HashMap<String, ContractKey>>,
    definitions: &HashMap<ContractKey, Definition>,
    source: &Path,
) -> Result<(), String> {
    let state_type = path_type(state, states, records, locals, source)?;
    let Some(scope) = scope else {
        return Err(format!("{}: missing module scope", source.display()));
    };
    let Some(key) = scope.get(function) else {
        return Err(format!(
            "{}: page calls unknown contract: {function}",
            source.display()
        ));
    };
    let definition = definitions
        .get(key)
        .ok_or_else(|| format!("{}: contract not found: {function}", source.display()))?;
    if definition.contract.params.len() != args.len() {
        return Err(format!(
            "{}: page call {} expects {} arguments, got {}",
            source.display(),
            function,
            definition.contract.params.len(),
            args.len()
        ));
    }
    if !types_compatible(&definition.contract.return_type, &state_type) {
        return Err(format!(
            "{}: target {} and contract {} have different types",
            source.display(),
            state,
            function
        ));
    }
    for (index, arg) in args.iter().enumerate() {
        let actual_type = page_expr_type(arg, states, records, locals, source)?;
        let expected_type = &definition.contract.params[index].ty;
        if !types_compatible_expr(&actual_type, expected_type, arg) {
            return Err(format!(
                "{}: argument {} of {} has type {:?}, expected {:?}",
                source.display(),
                index + 1,
                function,
                actual_type,
                expected_type
            ));
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn validate_elements(
    elements: &[PageElement],
    states: &HashMap<String, Type>,
    records: &HashMap<String, RecordDef>,
    locals: &HashMap<String, Type>,
    scope: Option<&HashMap<String, ContractKey>>,
    definitions: &HashMap<ContractKey, Definition>,
    reducers: &[Reducer],
    source: &Path,
) -> Result<(), String> {
    for element in elements {
        match element {
            PageElement::Show { state } => {
                path_type(state, states, records, locals, source)?;
            }
            PageElement::ShowExpr { value } => {
                page_expr_type(value, states, records, locals, source)?;
            }
            PageElement::ShowText { .. } => {}
            PageElement::WhenShow { condition, .. } => {
                if page_expr_type(condition, states, records, locals, source)? != Type::Bool {
                    return Err(format!(
                        "{}: conditional show requires a bool expression",
                        source.display()
                    ));
                }
            }
            PageElement::Input { state, .. } => {
                if path_type(state, states, records, locals, source)? != Type::Text {
                    return Err(format!(
                        "{}: input state must be text: {state}",
                        source.display()
                    ));
                }
            }
            PageElement::Button {
                state,
                function,
                args,
                ..
            } => {
                validate_call(
                    state,
                    function,
                    args,
                    states,
                    records,
                    locals,
                    scope,
                    definitions,
                    source,
                )?;
            }
            PageElement::RunReducer { reducer, .. } => {
                if !reducers.iter().any(|candidate| candidate.name == *reducer) {
                    return Err(format!("{}: unknown reducer: {reducer}", source.display()));
                }
            }
            PageElement::Add { state, value, .. } => {
                let Type::List(item_type) = path_type(state, states, records, locals, source)?
                else {
                    return Err(format!(
                        "{}: adds target must be a list: {state}",
                        source.display()
                    ));
                };
                let value_type = page_expr_type(value, states, records, locals, source)?;
                if !types_compatible(&value_type, &item_type) {
                    return Err(format!(
                        "{}: list item has type {:?}, expected {:?}",
                        source.display(),
                        value_type,
                        item_type
                    ));
                }
            }
            PageElement::List {
                state,
                item,
                elements,
            } => {
                let Type::List(item_type) = path_type(state, states, records, locals, source)?
                else {
                    return Err(format!(
                        "{}: list element needs a list state: {state}",
                        source.display()
                    ));
                };
                let mut nested = locals.clone();
                nested.insert(item.clone(), *item_type);
                validate_elements(
                    elements,
                    states,
                    records,
                    &nested,
                    scope,
                    definitions,
                    reducers,
                    source,
                )?;
            }
            PageElement::Use { name } => {
                return Err(format!(
                    "{}: unresolved component: {name}",
                    source.display()
                ));
            }
        }
    }
    Ok(())
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
        let summary = project
            .verification_summaries
            .iter()
            .find(|item| item.contract.name == "total")
            .unwrap();
        assert!(matches!(
            &summary.contract.ensures[0],
            SpecExpr::Binary(axiom_parser::BinaryOp::Eq, _, rhs)
                if matches!(rhs.as_ref(), SpecExpr::Call(name, _) if name == "add")
        ));
        assert_eq!(project.files.len(), 2);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rejects_circular_contract_calls_with_source_path() {
        let root = std::env::temp_dir().join(format!(
            "synthetix-project-circular-call-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let entry = root.join("entry.ax");
        fs::write(
            &entry,
            "contract first(value: u32) -> u32 { ensures result == second(value); }\n\
             contract second(value: u32) -> u32 { ensures result == first(value); }",
        )
        .unwrap();
        fs::write(
            root.join("synthetix.toml"),
            "[project]\nentry = \"entry.ax\"\ntarget = \"web\"\n",
        )
        .unwrap();

        let error = load(root.join("synthetix.toml")).unwrap_err();
        assert!(error.contains(&entry.display().to_string()));
        assert!(error.contains("circular contract call: first"));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn composes_imported_requires_and_ensures_through_project_api() {
        let root = std::env::temp_dir().join(format!(
            "synthetix-project-compositional-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        fs::write(
            root.join("positive.ax"),
            "export contract positive(value: u32) -> u32 {\n requires value > 0;\n ensures result == value;\n}\n",
        )
        .unwrap();
        fs::write(
            root.join("entry.ax"),
            "import { positive } from \"./positive.ax\";\ncontract total(value: u32) -> u32 {\n requires value > 0;\n ensures result == positive(value);\n}\n",
        )
        .unwrap();
        fs::write(
            root.join("synthetix.toml"),
            "[project]\nentry = \"entry.ax\"\ntarget = \"web\"\n",
        )
        .unwrap();

        let project = load(root.join("synthetix.toml")).unwrap();
        let linked = project
            .contracts
            .iter()
            .find(|item| item.contract.name == "total")
            .unwrap();
        let proof = project.verify_compositional(linked, &Expr::Arg(0)).unwrap();
        assert_eq!(proof.proof, Some(axiom_verify::ProofResult::Verified));
        assert_eq!(proof.diagnostic, None);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn project_call_precondition_diagnostic_keeps_source_location() {
        let root = std::env::temp_dir().join(format!(
            "synthetix-project-call-location-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        fs::write(
            root.join("positive.ax"),
            "export contract positive(value: u32) -> u32 { requires value > 0; ensures result == value; }",
        )
        .unwrap();
        fs::write(
            root.join("entry.ax"),
            "import { positive } from \"./positive.ax\";\ncontract total(value: u32) -> u32 {\n ensures result == positive(value);\n}\n",
        )
        .unwrap();
        fs::write(
            root.join("synthetix.toml"),
            "[project]\nentry = \"entry.ax\"\ntarget = \"web\"\n",
        )
        .unwrap();

        let project = load(root.join("synthetix.toml")).unwrap();
        let linked = project
            .contracts
            .iter()
            .find(|item| item.contract.name == "total")
            .unwrap();
        let proof = project.verify_compositional(linked, &Expr::Arg(0)).unwrap();
        assert_eq!(proof.proof, None);
        assert!(matches!(
            proof.diagnostic,
            Some(axiom_verify::Diagnostic::CallPreconditionViolation {
                callee,
                span: Some(span),
            }) if callee == "positive" && span.line == 3
        ));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn validates_v31_page_data_and_component_usage() {
        let root = std::env::temp_dir().join(format!(
            "synthetix-project-v31-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        fs::write(
            root.join("logic.ax"),
            "export program toggle(done: bool) -> bool { gives answer equals (done != true); }",
        )
        .unwrap();
        fs::write(
            root.join("entry.ax"),
            r#"
            import { toggle } from "./logic.ax";
            record Todo { title: text; done: bool; }
            component Help { show "도움말"; }
            page Todo {
                state todos: list<Todo> = [];
                state draft: text = "";
                persist todos;
                input draft placeholder "할 일";
                use Help;
                button "추가" adds todos with Todo { title: draft, done: false };
                list todos as todo {
                    show todo.title;
                    button "완료" updates todo.done with toggle(todo.done);
                }
            }
            "#,
        )
        .unwrap();
        fs::write(
            root.join("synthetix.toml"),
            "[project]\nentry = \"entry.ax\"\ntarget = \"web\"\n",
        )
        .unwrap();

        let project = load(root.join("synthetix.toml")).unwrap();
        let page = &project.pages[0].page;
        assert!(
            page.elements
                .iter()
                .all(|element| { !matches!(element, PageElement::Use { .. }) })
        );
        assert!(
            page.elements
                .iter()
                .any(|element| matches!(element, PageElement::List { .. }))
        );

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn links_fn_and_extern_without_treating_extern_as_synthesizable() {
        let root = std::env::temp_dir().join(format!(
            "synthetix-project-v45-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        fs::write(
            root.join("logic.ax"),
            "export fn increment(value: u32) -> u32 { return value +% 1; ensures result == value +% 1; }\nexport extern fn read_count(key: u32) -> u32;",
        )
        .unwrap();
        fs::write(
            root.join("entry.ax"),
            "import { increment, read_count } from \"./logic.ax\"; contract total(value: u32) -> u32 { ensures result == value; }",
        )
        .unwrap();
        fs::write(
            root.join("synthetix.toml"),
            "[project]\nentry = \"entry.ax\"\ntarget = \"web\"\n",
        )
        .unwrap();

        let project = load(root.join("synthetix.toml")).unwrap();
        assert!(
            project
                .contracts
                .iter()
                .any(|item| { item.contract.kind == axiom_parser::DeclarationKind::Function })
        );
        assert!(
            project
                .contracts
                .iter()
                .any(|item| { item.contract.kind == axiom_parser::DeclarationKind::Extern })
        );

        fs::remove_dir_all(root).unwrap();
    }
}
