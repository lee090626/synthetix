//! Project loading and module linking for Synthetix v2.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use axiom_ami::Expr;
use axiom_parser::{
    Component, Contract, Import, Module, Page, PageElement, RecordDef, Reducer, Span, SpecExpr,
    Type,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectConfig {
    pub root: PathBuf,
    pub entry: PathBuf,
    pub target: String,
    pub storage_schema_version: u32,
}

#[derive(Debug, Clone)]
pub struct LinkedContract {
    pub source: PathBuf,
    pub contract: Contract,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ContractId {
    pub source: PathBuf,
    pub name: String,
}

impl From<&LinkedContract> for ContractId {
    fn from(linked: &LinkedContract) -> Self {
        Self {
            source: linked.source.clone(),
            name: linked.contract.name.clone(),
        }
    }
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
    /// Imported contract identities keyed by the importing module and local name.
    pub verification_callee_ids: HashMap<PathBuf, HashMap<String, ContractId>>,
    /// Dependency-first order for verifying summaries.
    pub verification_order: Vec<ContractId>,
    /// Exact source bytes used to parse each module, for sound incremental proof keys.
    pub source_contents: HashMap<PathBuf, Arc<[u8]>>,
    pub pages: Vec<LinkedPage>,
    pub files: Vec<PathBuf>,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct RetainedLoadStats {
    pub modules_parsed: usize,
    pub modules_reused: usize,
    pub contracts_relinked: usize,
    pub contracts_reused: usize,
    pub pages_relinked: usize,
    pub pages_reused: usize,
    pub affected_modules: Vec<PathBuf>,
}

#[derive(Default, Clone)]
pub struct RetainedProjectLoader {
    state: Option<RetainedState>,
    watch_paths: Vec<PathBuf>,
}

#[derive(Clone)]
struct RetainedState {
    config: ProjectConfig,
    modules: HashMap<PathBuf, ModuleInfo>,
    project: Project,
}

impl RetainedProjectLoader {
    /// Paths observed during the most recent load attempt, including unresolved imports.
    pub fn watch_paths(&self) -> &[PathBuf] {
        &self.watch_paths
    }

    /// Loads a fresh project graph while retaining byte-identical parse and link products.
    /// The ordinary `load` function remains the cold, stateless reference path.
    pub fn load(
        &mut self,
        config_path: impl AsRef<Path>,
    ) -> Result<(Project, RetainedLoadStats), String> {
        self.watch_paths.clear();
        push_watch_path(&mut self.watch_paths, config_path.as_ref().to_path_buf());
        let config = parse_config(config_path.as_ref())?;
        let previous = self.state.as_ref();
        let mut modules = Vec::new();
        let mut visiting = Vec::new();
        let mut stats = RetainedLoadStats::default();
        visit_module_retained(
            &config.entry,
            &mut modules,
            &mut visiting,
            previous.map(|state| &state.modules),
            &mut stats,
            &mut self.watch_paths,
        )?;

        let current_modules = modules
            .iter()
            .cloned()
            .map(|module| (module.path.clone(), module))
            .collect::<HashMap<_, _>>();
        let mut affected = affected_modules(previous, &config, &current_modules);
        let global_relink = previous.is_none_or(|state| {
            global_declarations(&state.modules) != global_declarations(&current_modules)
        });
        if global_relink {
            affected.extend(current_modules.keys().cloned());
            if let Some(state) = previous {
                affected.extend(state.modules.keys().cloned());
            }
        }
        stats.affected_modules = affected.iter().cloned().collect();
        stats.affected_modules.sort();

        let project = link_project_retained(
            config.clone(),
            modules,
            previous.map(|state| &state.project),
            &affected,
            &mut stats,
        )?;
        self.state = Some(RetainedState {
            config,
            modules: current_modules,
            project: project.clone(),
        });
        Ok((project, stats))
    }
}

impl Project {
    /// Verifies a linked contract against the project's closed contract summaries.
    pub fn verify_compositional(
        &self,
        linked: &LinkedContract,
        candidate: &Expr,
    ) -> Result<axiom_verify::CompositionalProof, String> {
        let summaries = self.all_linked_summaries();
        self.verify_compositional_with_verified_summaries(linked, candidate, &summaries)
    }

    pub fn verify_compositional_with_metrics(
        &self,
        linked: &LinkedContract,
        candidate: &Expr,
    ) -> Result<
        (
            axiom_verify::CompositionalProof,
            axiom_verify::VerificationMetrics,
        ),
        String,
    > {
        let summaries = self.all_linked_summaries();
        self.verify_compositional_with_verified_summaries_and_metrics(linked, candidate, &summaries)
    }

    /// Verifies a source summary using only dependency summaries already accepted by the caller.
    pub fn verify_compositional_with_verified_summaries(
        &self,
        linked: &LinkedContract,
        candidate: &Expr,
        verified_summaries: &HashMap<ContractId, Contract>,
    ) -> Result<axiom_verify::CompositionalProof, String> {
        let summary = self.source_summary(linked)?;
        let callees = self.verified_callees(linked, verified_summaries)?;
        axiom_verify::verify_compositional(&summary.contract, candidate, &callees)
    }

    pub fn verify_compositional_with_verified_summaries_and_metrics(
        &self,
        linked: &LinkedContract,
        candidate: &Expr,
        verified_summaries: &HashMap<ContractId, Contract>,
    ) -> Result<
        (
            axiom_verify::CompositionalProof,
            axiom_verify::VerificationMetrics,
        ),
        String,
    > {
        let summary = self.source_summary(linked)?;
        let callees = self.verified_callees(linked, verified_summaries)?;
        axiom_verify::verify_compositional_with_metrics(&summary.contract, candidate, &callees)
    }

    fn source_summary(&self, linked: &LinkedContract) -> Result<&LinkedContract, String> {
        self.verification_summaries
            .iter()
            .find(|item| item.source == linked.source && item.contract.name == linked.contract.name)
            .ok_or_else(|| format!("missing verification summary: {}", linked.contract.name))
    }

    pub fn verification_summary(&self, id: &ContractId) -> Option<&LinkedContract> {
        self.verification_summaries
            .iter()
            .find(|linked| ContractId::from(*linked) == *id)
    }

    pub fn verification_dependencies(
        &self,
        linked: &LinkedContract,
    ) -> Result<Vec<(String, ContractId)>, String> {
        let summary = self.source_summary(linked)?;
        let mut dependencies = Vec::new();
        for clause in summary
            .contract
            .requires
            .iter()
            .chain(&summary.contract.ensures)
        {
            collect_contract_dependencies(clause, None, &mut dependencies);
        }
        let mut names = dependencies
            .into_iter()
            .map(|dependency| dependency.call_name)
            .collect::<Vec<_>>();
        names.sort();
        names.dedup();

        let identities = self.verification_callee_ids.get(&linked.source);
        names
            .into_iter()
            .map(|name| {
                let id = identities
                    .and_then(|identities| identities.get(&name))
                    .cloned()
                    .ok_or_else(|| {
                        format!("unknown callee `{name}` in {}", linked.contract.name)
                    })?;
                Ok((name, id))
            })
            .collect()
    }

    fn all_linked_summaries(&self) -> HashMap<ContractId, Contract> {
        self.contracts
            .iter()
            .map(|linked| (ContractId::from(linked), linked.contract.clone()))
            .collect()
    }

    fn verified_callees(
        &self,
        linked: &LinkedContract,
        verified_summaries: &HashMap<ContractId, Contract>,
    ) -> Result<HashMap<String, Contract>, String> {
        let signatures = self.verification_callees.get(&linked.source);
        let mut callees = HashMap::new();
        for (name, id) in self.verification_dependencies(linked)? {
            if let Some(contract) = verified_summaries.get(&id) {
                callees.insert(name, contract.clone());
                continue;
            }
            if let Some(signature) = signatures
                .and_then(|signatures| signatures.get(&name))
                .filter(|contract| contract.kind == axiom_parser::DeclarationKind::Extern)
            {
                callees.insert(name, signature.clone());
                continue;
            }
            return Err(format!(
                "unverified callee summary `{name}` required by `{}`",
                linked.contract.name
            ));
        }
        Ok(callees)
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
    source_contents: Arc<[u8]>,
}

#[derive(Debug, Clone)]
struct ContractDependency {
    call_name: String,
    span: Option<Span>,
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
    let mut storage_schema_version = 1;

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
            "storage_schema_version" => {
                storage_schema_version = value.parse().map_err(|_| {
                    format!(
                        "{}:{}: storage_schema_version must be a positive integer",
                        path.display(),
                        index + 1
                    )
                })?;
                if storage_schema_version == 0 {
                    return Err(format!(
                        "{}:{}: storage_schema_version must be a positive integer",
                        path.display(),
                        index + 1
                    ));
                }
            }
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
        storage_schema_version,
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
    axiom_parser::simple::check_user_syntax(&source)
        .map_err(|error| format!("{}: {error}", path.display()))?;
    let module = axiom_parser::parse_module(&source)
        .map_err(|error| format!("{}: {error}", path.display()))?;
    let source_contents = Arc::<[u8]>::from(source.as_bytes());
    let imports = module.imports.clone();
    for import in &imports {
        let imported = resolve_import(&path, import)?;
        visit_module(&imported, modules, visiting)?;
    }
    visiting.pop();
    modules.push(ModuleInfo {
        path,
        module,
        source_contents,
    });
    Ok(())
}

fn visit_module_retained(
    path: &Path,
    modules: &mut Vec<ModuleInfo>,
    visiting: &mut Vec<PathBuf>,
    previous: Option<&HashMap<PathBuf, ModuleInfo>>,
    stats: &mut RetainedLoadStats,
    watch_paths: &mut Vec<PathBuf>,
) -> Result<(), String> {
    push_watch_path(watch_paths, path.to_path_buf());
    let path = canonical(path)?;
    push_watch_path(watch_paths, path.clone());
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
    let source_bytes = source.as_bytes();
    let cached = previous
        .and_then(|cache| cache.get(&path))
        .filter(|module| module.source_contents.as_ref() == source_bytes);
    let (module, source_contents) = if let Some(cached) = cached {
        stats.modules_reused += 1;
        (cached.module.clone(), Arc::clone(&cached.source_contents))
    } else {
        stats.modules_parsed += 1;
        axiom_parser::simple::check_user_syntax(&source)
            .map_err(|error| format!("{}: {error}", path.display()))?;
        let module = axiom_parser::parse_module(&source)
            .map_err(|error| format!("{}: {error}", path.display()))?;
        (module, Arc::<[u8]>::from(source_bytes))
    };
    for import in &module.imports {
        push_watch_path(watch_paths, import_candidate_path(&path, import)?);
        let imported = resolve_import(&path, import)?;
        visit_module_retained(&imported, modules, visiting, previous, stats, watch_paths)?;
    }
    visiting.pop();
    modules.push(ModuleInfo {
        path,
        module,
        source_contents,
    });
    Ok(())
}

fn push_watch_path(paths: &mut Vec<PathBuf>, path: PathBuf) {
    if !paths.contains(&path) {
        paths.push(path);
    }
}

fn affected_modules(
    previous: Option<&RetainedState>,
    config: &ProjectConfig,
    current: &HashMap<PathBuf, ModuleInfo>,
) -> HashSet<PathBuf> {
    let Some(previous) = previous else {
        return current.keys().cloned().collect();
    };
    let old = &previous.modules;
    let mut affected = HashSet::new();
    for (path, module) in current {
        if old
            .get(path)
            .is_none_or(|cached| cached.source_contents.as_ref() != module.source_contents.as_ref())
        {
            affected.insert(path.clone());
        }
    }
    affected.extend(
        old.keys()
            .filter(|path| !current.contains_key(*path))
            .cloned(),
    );
    if previous.config.entry != config.entry {
        if let Ok(path) = canonical(&previous.config.entry) {
            affected.insert(path);
        }
        if let Ok(path) = canonical(&config.entry) {
            affected.insert(path);
        }
    }

    let mut reverse = HashMap::<PathBuf, HashSet<PathBuf>>::new();
    for modules in [old, current] {
        for module in modules.values() {
            for import in &module.module.imports {
                if let Ok(target) = resolve_import(&module.path, import) {
                    reverse
                        .entry(target)
                        .or_default()
                        .insert(module.path.clone());
                }
            }
        }
    }
    let mut pending = affected.iter().cloned().collect::<Vec<_>>();
    while let Some(path) = pending.pop() {
        if let Some(importers) = reverse.get(&path) {
            for importer in importers {
                if affected.insert(importer.clone()) {
                    pending.push(importer.clone());
                }
            }
        }
    }
    affected
}

fn global_declarations(modules: &HashMap<PathBuf, ModuleInfo>) -> Vec<String> {
    let mut declarations = modules
        .values()
        .flat_map(|module| {
            module
                .module
                .records
                .iter()
                .map(|record| format!("record:{record:?}"))
                .chain(
                    module
                        .module
                        .components
                        .iter()
                        .map(|component| format!("component:{component:?}")),
                )
        })
        .collect::<Vec<_>>();
    declarations.sort();
    declarations
}

fn resolve_import(source: &Path, import: &Import) -> Result<PathBuf, String> {
    let path = import_candidate_path(source, import)?;
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

fn import_candidate_path(source: &Path, import: &Import) -> Result<PathBuf, String> {
    let mut path = source
        .parent()
        .ok_or_else(|| format!("cannot determine parent of {}", source.display()))?
        .join(&import.path);
    if path.extension().is_none() {
        path.set_extension("ax");
    }
    Ok(path)
}

fn canonical(path: &Path) -> Result<PathBuf, String> {
    fs::canonicalize(path).map_err(|error| format!("cannot resolve {}: {error}", path.display()))
}

fn link_project(config: ProjectConfig, modules: Vec<ModuleInfo>) -> Result<Project, String> {
    let mut stats = RetainedLoadStats::default();
    link_project_retained(config, modules, None, &HashSet::new(), &mut stats)
}

fn link_project_retained(
    config: ProjectConfig,
    modules: Vec<ModuleInfo>,
    previous: Option<&Project>,
    affected: &HashSet<PathBuf>,
    stats: &mut RetainedLoadStats,
) -> Result<Project, String> {
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

    let verification_order = verification_order(&definitions, &scopes)?;
    let verification_callee_ids = scopes
        .iter()
        .map(|(source, scope)| {
            let ids = scope
                .iter()
                .map(|(name, key)| {
                    (
                        name.clone(),
                        ContractId {
                            source: key.0.clone(),
                            name: key.1.clone(),
                        },
                    )
                })
                .collect();
            (source.clone(), ids)
        })
        .collect();

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
    let previous_contracts = previous
        .map(|project| {
            project
                .contracts
                .iter()
                .map(|linked| (ContractId::from(linked), linked))
                .collect::<HashMap<_, _>>()
        })
        .unwrap_or_default();
    for module in &modules {
        for contract in &module.module.contracts {
            let key = (module.path.clone(), contract.name.clone());
            let id = ContractId {
                source: module.path.clone(),
                name: contract.name.clone(),
            };
            verification_summaries.push(LinkedContract {
                source: module.path.clone(),
                contract: contract.clone(),
            });
            let linked = if !affected.contains(&module.path)
                && let Some(previous) = previous_contracts.get(&id)
            {
                stats.contracts_reused += 1;
                previous.contract.clone()
            } else {
                stats.contracts_relinked += 1;
                let linked =
                    expand_contract(&key, &definitions, &scopes, &mut cache, &mut Vec::new())
                        .map_err(|error| format!("{}: {error}", module.path.display()))?;
                axiom_parser::validate(&linked)
                    .map_err(|error| format!("{}: {error}", module.path.display()))?;
                linked
            };
            contracts.push(LinkedContract {
                source: module.path.clone(),
                contract: linked,
            });
        }
    }

    let mut pages = Vec::new();
    for module in &modules {
        if !affected.contains(&module.path)
            && let Some(previous) = previous
        {
            let reused = previous
                .pages
                .iter()
                .filter(|page| page.source == module.path)
                .cloned()
                .collect::<Vec<_>>();
            stats.pages_reused += reused.len();
            pages.extend(reused);
            continue;
        }
        for page in &module.module.pages {
            stats.pages_relinked += 1;
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

    let source_contents = modules
        .iter()
        .map(|module| (module.path.clone(), Arc::clone(&module.source_contents)))
        .collect();
    let files = modules.iter().map(|module| module.path.clone()).collect();

    Ok(Project {
        config,
        contracts,
        verification_summaries,
        verification_callees,
        verification_callee_ids,
        verification_order,
        source_contents,
        pages,
        files,
    })
}

fn verification_order(
    definitions: &HashMap<ContractKey, Definition>,
    scopes: &HashMap<PathBuf, HashMap<String, ContractKey>>,
) -> Result<Vec<ContractId>, String> {
    fn visit(
        key: &ContractKey,
        definitions: &HashMap<ContractKey, Definition>,
        scopes: &HashMap<PathBuf, HashMap<String, ContractKey>>,
        visiting: &mut Vec<ContractKey>,
        visited: &mut HashSet<ContractKey>,
        ordered: &mut Vec<ContractId>,
    ) -> Result<(), String> {
        if visited.contains(key) {
            return Ok(());
        }
        let definition = definitions
            .get(key)
            .ok_or_else(|| format!("contract not found: {}", key.1))?;
        visiting.push(key.clone());
        let mut dependencies = Vec::new();
        for (clauses, spans) in [
            (
                &definition.contract.requires,
                &definition.contract.requires_spans,
            ),
            (
                &definition.contract.ensures,
                &definition.contract.ensures_spans,
            ),
        ] {
            for (index, clause) in clauses.iter().enumerate() {
                collect_contract_dependencies(clause, spans.get(index).copied(), &mut dependencies);
            }
        }
        let scope = scopes
            .get(&key.0)
            .ok_or_else(|| format!("missing module scope: {}", key.0.display()))?;
        for dependency in dependencies {
            let Some(target) = scope.get(&dependency.call_name) else {
                let (line, column) = dependency
                    .span
                    .map(|span| (span.line, span.column))
                    .unwrap_or((1, 1));
                return Err(format!(
                    "{}:{line}:{column}: unknown contract call `{}` in `{}`",
                    key.0.display(),
                    dependency.call_name,
                    key.1
                ));
            };
            if let Some(cycle_start) = visiting.iter().position(|item| item == target) {
                let chain = visiting[cycle_start..]
                    .iter()
                    .map(|item| item.1.as_str())
                    .chain(std::iter::once(target.1.as_str()))
                    .collect::<Vec<_>>()
                    .join(" -> ");
                let (line, column) = dependency
                    .span
                    .map(|span| (span.line, span.column))
                    .unwrap_or((1, 1));
                return Err(format!(
                    "{}:{line}:{column}: circular contract call: {chain} (caller={}, callee={}, call_chain={chain})",
                    key.0.display(),
                    key.1,
                    dependency.call_name
                ));
            }
            if let Some(callee) = definitions.get(target)
                && callee.contract.decimals != definition.contract.decimals
            {
                return Err(format!(
                    "{}: {} has {} decimal places but uses {}, which has {}; give both the same `with N decimals`",
                    key.0.display(),
                    key.1,
                    definition.contract.decimals,
                    dependency.call_name,
                    callee.contract.decimals
                ));
            }
            visit(target, definitions, scopes, visiting, visited, ordered)?;
        }
        visiting.pop();
        visited.insert(key.clone());
        ordered.push(ContractId {
            source: key.0.clone(),
            name: key.1.clone(),
        });
        Ok(())
    }

    let mut roots = definitions.keys().cloned().collect::<Vec<_>>();
    roots.sort();
    let mut visiting = Vec::new();
    let mut visited = HashSet::new();
    let mut ordered = Vec::with_capacity(roots.len());
    for key in roots {
        visit(
            &key,
            definitions,
            scopes,
            &mut visiting,
            &mut visited,
            &mut ordered,
        )?;
    }
    Ok(ordered)
}

fn collect_contract_dependencies(
    expr: &SpecExpr,
    span: Option<Span>,
    dependencies: &mut Vec<ContractDependency>,
) {
    match expr {
        SpecExpr::Call(name, args) => {
            // The target is resolved against the caller's scope during graph traversal.
            dependencies.push(ContractDependency {
                call_name: name.clone(),
                span,
            });
            for arg in args {
                collect_contract_dependencies(arg, span, dependencies);
            }
        }
        SpecExpr::Binary(_, lhs, rhs) => {
            collect_contract_dependencies(lhs, span, dependencies);
            collect_contract_dependencies(rhs, span, dependencies);
        }
        SpecExpr::If(condition, then_branch, else_branch) => {
            collect_contract_dependencies(condition, span, dependencies);
            collect_contract_dependencies(then_branch, span, dependencies);
            collect_contract_dependencies(else_branch, span, dependencies);
        }
        SpecExpr::Let(_, value, body) => {
            collect_contract_dependencies(value, span, dependencies);
            collect_contract_dependencies(body, span, dependencies);
        }
        SpecExpr::List(values) => {
            for value in values {
                collect_contract_dependencies(value, span, dependencies);
            }
        }
        SpecExpr::Record { fields, .. } => {
            for (_, value) in fields {
                collect_contract_dependencies(value, span, dependencies);
            }
        }
        SpecExpr::Field(value, _) => collect_contract_dependencies(value, span, dependencies),
        SpecExpr::Index(array, index) => {
            collect_contract_dependencies(array, span, dependencies);
            collect_contract_dependencies(index, span, dependencies);
        }
        SpecExpr::ArrayUpdate(array, index, value) => {
            collect_contract_dependencies(array, span, dependencies);
            collect_contract_dependencies(index, span, dependencies);
            collect_contract_dependencies(value, span, dependencies);
        }
        SpecExpr::For {
            initial,
            invariant,
            body,
            ..
        } => {
            collect_contract_dependencies(initial, span, dependencies);
            collect_contract_dependencies(invariant, span, dependencies);
            collect_contract_dependencies(body, span, dependencies);
        }
        SpecExpr::Ident(_) | SpecExpr::Number(_) | SpecExpr::Bool(_) | SpecExpr::Text(_) => {}
    }
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
        SpecExpr::Index(array, index) => Ok(SpecExpr::Index(
            Box::new(expand_expr(
                array,
                scope,
                definitions,
                scopes,
                cache,
                stack,
            )?),
            Box::new(expand_expr(
                index,
                scope,
                definitions,
                scopes,
                cache,
                stack,
            )?),
        )),
        SpecExpr::ArrayUpdate(array, index, value) => Ok(SpecExpr::ArrayUpdate(
            Box::new(expand_expr(
                array,
                scope,
                definitions,
                scopes,
                cache,
                stack,
            )?),
            Box::new(expand_expr(
                index,
                scope,
                definitions,
                scopes,
                cache,
                stack,
            )?),
            Box::new(expand_expr(
                value,
                scope,
                definitions,
                scopes,
                cache,
                stack,
            )?),
        )),
        SpecExpr::For {
            index,
            start,
            end,
            accumulator,
            initial,
            invariant,
            body,
        } => Ok(SpecExpr::For {
            index: index.clone(),
            start: *start,
            end: *end,
            accumulator: accumulator.clone(),
            initial: Box::new(expand_expr(
                initial,
                scope,
                definitions,
                scopes,
                cache,
                stack,
            )?),
            invariant: Box::new(expand_expr(
                invariant,
                scope,
                definitions,
                scopes,
                cache,
                stack,
            )?),
            body: Box::new(expand_expr(body, scope, definitions, scopes, cache, stack)?),
        }),
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
        SpecExpr::Index(array, index) => SpecExpr::Index(
            Box::new(substitute(array, values)),
            Box::new(substitute(index, values)),
        ),
        SpecExpr::ArrayUpdate(array, index, value) => SpecExpr::ArrayUpdate(
            Box::new(substitute(array, values)),
            Box::new(substitute(index, values)),
            Box::new(substitute(value, values)),
        ),
        SpecExpr::For {
            index,
            start,
            end,
            accumulator,
            initial,
            invariant,
            body,
        } => {
            let mut nested = values.clone();
            nested.remove(index);
            nested.remove(accumulator);
            SpecExpr::For {
                index: index.clone(),
                start: *start,
                end: *end,
                accumulator: accumulator.clone(),
                initial: Box::new(substitute(initial, values)),
                invariant: Box::new(substitute(invariant, &nested)),
                body: Box::new(substitute(body, &nested)),
            }
        }
        SpecExpr::List(items) => {
            SpecExpr::List(items.iter().map(|item| substitute(item, values)).collect())
        }
        SpecExpr::Record { name, fields } => SpecExpr::Record {
            name: name.clone(),
            fields: fields
                .iter()
                .map(|(field, value)| (field.clone(), substitute(value, values)))
                .collect(),
        },
        SpecExpr::Text(_) | SpecExpr::Field(_, _) => expr.clone(),
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
        if matches!(state.ty, Type::FixedArray(_, _)) {
            return Err(format!(
                "{}: verified fixed arrays cannot be used as application state",
                source.display()
            ));
        }
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
        if !types_compatible_expr(&initial_type, &state.ty, &state.initial) {
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
    validate_decimals(page, scope, definitions, source)?;
    Ok(())
}

/// Values with decimals are stored times 10^places, so they may only meet
/// values and rules with the same places, and are shown on their own.
fn validate_decimals(
    page: &Page,
    scope: Option<&HashMap<String, ContractKey>>,
    definitions: &HashMap<ContractKey, Definition>,
    source: &Path,
) -> Result<(), String> {
    let places: HashMap<&str, u32> = page
        .states
        .iter()
        .map(|state| (state.name.as_str(), state.decimals))
        .collect();
    let of_state = |name: &str| places.get(name).copied().unwrap_or(0);
    let uses_decimals = |expr: &SpecExpr| {
        fn walk(expr: &SpecExpr, of_state: &dyn Fn(&str) -> u32) -> bool {
            match expr {
                SpecExpr::Ident(name) => of_state(name) > 0,
                SpecExpr::Binary(_, left, right) => walk(left, of_state) || walk(right, of_state),
                SpecExpr::If(condition, then, otherwise) => {
                    walk(condition, of_state) || walk(then, of_state) || walk(otherwise, of_state)
                }
                SpecExpr::Call(_, args) => args.iter().any(|arg| walk(arg, of_state)),
                _ => false,
            }
        }
        walk(expr, &of_state)
    };
    for element in &page.elements {
        match element {
            PageElement::ShowExpr { value: expr }
            | PageElement::WhenShow {
                condition: expr, ..
            } if uses_decimals(expr) => {
                return Err(format!(
                    "{}: a value with decimals can only be shown on its own, like `show price`",
                    source.display()
                ));
            }
            PageElement::Button {
                state,
                function,
                args,
                ..
            } => {
                let rule_places = scope
                    .and_then(|scope| scope.get(function))
                    .and_then(|key| definitions.get(key))
                    .map_or(0, |definition| definition.contract.decimals);
                let mismatch = |what: &str, found: u32| {
                    format!(
                        "{}: {what} has {found} decimal places but {function} has {rule_places}; use the same `with N decimals`",
                        source.display()
                    )
                };
                if of_state(state) != rule_places {
                    return Err(mismatch(state, of_state(state)));
                }
                for arg in args {
                    match arg {
                        SpecExpr::Ident(name) if of_state(name) != rule_places => {
                            return Err(mismatch(name, of_state(name)));
                        }
                        SpecExpr::Ident(_) => {}
                        _ if rule_places > 0 => {
                            return Err(format!(
                                "{}: {function} has decimals; pass saved values by name",
                                source.display()
                            ));
                        }
                        other if uses_decimals(other) => {
                            return Err(format!(
                                "{}: {function} has no decimals but gets a value with decimals",
                                source.display()
                            ));
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
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
        SpecExpr::Index(_, _) | SpecExpr::ArrayUpdate(_, _, _) => Err(format!(
            "{}: verified fixed arrays cannot be used in application expressions",
            source.display()
        )),
        SpecExpr::For { .. } => Err(format!(
            "{}: bounded verified loops cannot be used in application expressions",
            source.display()
        )),
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
                | axiom_parser::BinaryOp::SubChecked
                | axiom_parser::BinaryOp::MulWrap
                | axiom_parser::BinaryOp::Div
                | axiom_parser::BinaryOp::Rem
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
        (Type::FixedArray(_, _), _) | (_, Type::FixedArray(_, _)) => actual == expected,
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
        // A whole number may go into a `can be negative` rule; the runtime
        // rejects values above i32::MAX before the call.
        let widens = actual_type == Type::U32 && expected_type == &Type::I32;
        if !widens && !types_compatible_expr(&actual_type, expected_type, arg) {
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
                if state.contains('.') {
                    return Err(format!(
                        "{}: input requires a top-level state: {state}",
                        source.display(),
                    ));
                }
                let input_type = path_type(state, states, records, locals, source)?;
                if !matches!(input_type, Type::Text | Type::U32 | Type::I32 | Type::Bool) {
                    return Err(format!(
                        "{}: input state must be text or a scalar: {state}",
                        source.display(),
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

    fn assert_same_project(left: &Project, right: &Project) {
        assert_eq!(left.config, right.config);
        assert_eq!(left.files, right.files);
        assert_eq!(left.verification_order, right.verification_order);
        assert_eq!(left.verification_callee_ids, right.verification_callee_ids);
        assert_eq!(left.verification_callees, right.verification_callees);
        assert_eq!(
            left.contracts
                .iter()
                .map(|linked| (&linked.source, &linked.contract))
                .collect::<Vec<_>>(),
            right
                .contracts
                .iter()
                .map(|linked| (&linked.source, &linked.contract))
                .collect::<Vec<_>>()
        );
        assert_eq!(
            left.verification_summaries
                .iter()
                .map(|linked| (&linked.source, &linked.contract))
                .collect::<Vec<_>>(),
            right
                .verification_summaries
                .iter()
                .map(|linked| (&linked.source, &linked.contract))
                .collect::<Vec<_>>()
        );
        assert_eq!(
            left.pages
                .iter()
                .map(|page| (&page.source, &page.page))
                .collect::<Vec<_>>(),
            right
                .pages
                .iter()
                .map(|page| (&page.source, &page.page))
                .collect::<Vec<_>>()
        );
        let source_bytes = |project: &Project| {
            let mut values = project
                .source_contents
                .iter()
                .map(|(path, bytes)| (path.clone(), bytes.to_vec()))
                .collect::<Vec<_>>();
            values.sort_by(|left, right| left.0.cmp(&right.0));
            values
        };
        assert_eq!(source_bytes(left), source_bytes(right));
    }

    #[test]
    fn retained_loader_matches_cold_load_across_edit_classes_and_recovery() {
        let root = std::env::temp_dir().join(format!(
            "synthetix-retained-loader-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        fs::write(
            root.join("leaf.ax"),
            "export contract leaf(value: u32) -> u32 { requires value > 0; ensures result == value; }",
        )
        .unwrap();
        fs::write(
            root.join("middle.ax"),
            "import { leaf } from \"./leaf.ax\";\nexport contract middle(value: u32) -> u32 { requires value > 0; ensures result == leaf(value); }",
        )
        .unwrap();
        fs::write(
            root.join("unrelated.ax"),
            "export contract unrelated(value: u32) -> u32 { ensures result == value; }",
        )
        .unwrap();
        fs::write(
            root.join("entry.ax"),
            "import { middle } from \"./middle.ax\";\nimport { unrelated } from \"./unrelated.ax\";\ncontract root(value: u32) -> u32 { requires value > 0; ensures result == middle(value); }",
        )
        .unwrap();
        let config = root.join("synthetix.toml");
        fs::write(
            &config,
            "[project]\nentry = \"entry.ax\"\ntarget = \"web\"\n",
        )
        .unwrap();

        let mut loader = RetainedProjectLoader::default();
        let (first, first_stats) = loader.load(&config).unwrap();
        assert_same_project(&first, &load(&config).unwrap());
        assert_eq!(first_stats.modules_parsed, 4);
        assert_eq!(first_stats.contracts_relinked, 4);
        let (noop, noop_stats) = loader.load(&config).unwrap();
        assert_same_project(&noop, &load(&config).unwrap());
        assert_eq!(noop_stats.modules_parsed, 0);
        assert_eq!(noop_stats.modules_reused, 4);
        assert_eq!(noop_stats.contracts_relinked, 0);
        assert_eq!(noop_stats.contracts_reused, 4);
        assert!(noop_stats.affected_modules.is_empty());

        fs::write(
            &config,
            "[project]\nentry = \"entry.ax\"\ntarget = \"web\"\nstorage_schema_version = 2\n",
        )
        .unwrap();
        let (manifest_edit, manifest_stats) = loader.load(&config).unwrap();
        assert_same_project(&manifest_edit, &load(&config).unwrap());
        assert_eq!(manifest_stats.modules_parsed, 0);
        assert_eq!(manifest_stats.contracts_relinked, 0);
        assert_eq!(manifest_stats.contracts_reused, 4);
        assert!(manifest_stats.affected_modules.is_empty());

        let leaf_path = root.join("leaf.ax");
        let leaf_source = fs::read_to_string(&leaf_path).unwrap();
        fs::write(
            &leaf_path,
            leaf_source.replace("result == value", "result == value +% 0"),
        )
        .unwrap();
        let (leaf_edit, leaf_stats) = loader.load(&config).unwrap();
        assert_same_project(&leaf_edit, &load(&config).unwrap());
        assert_eq!(leaf_stats.modules_parsed, 1);
        assert_eq!(leaf_stats.modules_reused, 3);
        assert_eq!(leaf_stats.contracts_relinked, 3);
        assert_eq!(leaf_stats.contracts_reused, 1);

        let entry_path = root.join("entry.ax");
        let entry_source = fs::read_to_string(&entry_path).unwrap();
        fs::write(
            &entry_path,
            entry_source.replace("result == middle(value)", "result == middle(value) +% 0"),
        )
        .unwrap();
        let (root_edit, root_stats) = loader.load(&config).unwrap();
        assert_same_project(&root_edit, &load(&config).unwrap());
        assert_eq!(root_stats.modules_parsed, 1);
        assert_eq!(root_stats.modules_reused, 3);
        assert_eq!(root_stats.contracts_relinked, 1);
        assert_eq!(root_stats.contracts_reused, 3);

        let entry_source = fs::read_to_string(&entry_path).unwrap();
        fs::write(
            &entry_path,
            entry_source
                .lines()
                .filter(|line| !line.contains("unrelated"))
                .collect::<Vec<_>>()
                .join("\n"),
        )
        .unwrap();
        let (import_edit, import_stats) = loader.load(&config).unwrap();
        assert_same_project(&import_edit, &load(&config).unwrap());
        assert_eq!(import_stats.modules_parsed, 1);
        assert_eq!(import_stats.modules_reused, 2);
        assert_eq!(import_stats.contracts_relinked, 1);
        assert_eq!(import_stats.contracts_reused, 2);
        assert_eq!(import_edit.files.len(), 3);
        assert!(
            !import_edit
                .files
                .contains(&canonical(&root.join("unrelated.ax")).unwrap())
        );

        let middle_path = root.join("middle.ax");
        let middle_source = fs::read_to_string(&middle_path).unwrap();
        fs::rename(&leaf_path, root.join("renamed.ax")).unwrap();
        fs::write(
            &middle_path,
            middle_source.replace("./leaf.ax", "./renamed.ax"),
        )
        .unwrap();
        let (renamed, rename_stats) = loader.load(&config).unwrap();
        assert_same_project(&renamed, &load(&config).unwrap());
        assert_eq!(rename_stats.modules_parsed, 2);
        assert_eq!(rename_stats.modules_reused, 1);
        assert_eq!(rename_stats.contracts_relinked, 3);
        assert_eq!(rename_stats.contracts_reused, 0);
        assert!(
            renamed
                .files
                .contains(&canonical(&root.join("renamed.ax")).unwrap())
        );
        assert!(!renamed.files.iter().any(|path| path.ends_with("leaf.ax")));

        let broken_source = middle_source.replace("./leaf.ax", "./missing.ax");
        fs::write(&middle_path, &broken_source).unwrap();
        let retained_error = loader.load(&config).unwrap_err();
        let cold_error = load(&config).unwrap_err();
        assert_eq!(retained_error, cold_error);
        fs::write(
            &middle_path,
            middle_source.replace("./leaf.ax", "./renamed.ax"),
        )
        .unwrap();
        let (recovered, recovered_stats) = loader.load(&config).unwrap();
        assert_same_project(&recovered, &load(&config).unwrap());
        assert_eq!(recovered_stats.modules_parsed, 0);
        assert_eq!(recovered_stats.modules_reused, 3);
        assert_eq!(recovered_stats.contracts_relinked, 0);
        assert_eq!(recovered_stats.contracts_reused, 3);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn retained_loader_relinks_all_modules_when_global_records_change() {
        let root = std::env::temp_dir().join(format!(
            "synthetix-retained-global-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        fs::write(
            root.join("dep.ax"),
            "export contract leaf(value: u32) -> u32 { ensures result == value; }",
        )
        .unwrap();
        fs::write(
            root.join("entry.ax"),
            "import { leaf } from \"./dep.ax\";\ncontract root(value: u32) -> u32 { ensures result == leaf(value); }",
        )
        .unwrap();
        let config = root.join("synthetix.toml");
        fs::write(&config, "[project]\nentry = \"entry.ax\"\n").unwrap();
        let mut loader = RetainedProjectLoader::default();
        loader.load(&config).unwrap();
        fs::write(
            root.join("dep.ax"),
            "record Shared { value: u32; }\nexport contract leaf(value: u32) -> u32 { ensures result == value; }",
        )
        .unwrap();
        let (project, stats) = loader.load(&config).unwrap();
        assert_same_project(&project, &load(&config).unwrap());
        assert_eq!(stats.modules_parsed, 1);
        assert_eq!(stats.contracts_relinked, 2);
        assert_eq!(stats.contracts_reused, 0);
        assert_eq!(stats.affected_modules.len(), 2);
        fs::remove_dir_all(root).unwrap();
    }

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
        assert!(error.contains(&format!("{}:2:", entry.display())));
        assert!(error.contains("circular contract call: first"));
        assert!(error.contains("caller=second"));
        assert!(error.contains("callee=first"));
        assert!(error.contains("call_chain=first -> second -> first"));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rejects_missing_contract_call_with_source_location() {
        let root = std::env::temp_dir().join(format!(
            "synthetix-project-missing-call-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let entry = root.join("entry.ax");
        fs::write(
            &entry,
            "contract caller(value: u32) -> u32 {\n ensures result == missing(value);\n}\n",
        )
        .unwrap();
        fs::write(
            root.join("synthetix.toml"),
            "[project]\nentry = \"entry.ax\"\ntarget = \"web\"\n",
        )
        .unwrap();

        let error = load(root.join("synthetix.toml")).unwrap_err();
        assert!(error.contains(&format!("{}:2:", entry.display())));
        assert!(error.contains("unknown contract call `missing` in `caller`"));
        assert!(!error.contains("COUNTEREXAMPLE"));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn verification_order_is_deterministic_and_dependency_first() {
        let root = std::env::temp_dir().join(format!(
            "synthetix-project-order-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        fs::write(
            root.join("entry.ax"),
            "contract root(value: u32) -> u32 { requires value > 0; ensures result == middle(value); }\n\
             contract middle(value: u32) -> u32 { requires value > 0; ensures result == leaf(value); }\n\
             contract leaf(value: u32) -> u32 { requires value > 0; ensures result == value; }\n",
        )
        .unwrap();
        fs::write(
            root.join("synthetix.toml"),
            "[project]\nentry = \"entry.ax\"\ntarget = \"web\"\n",
        )
        .unwrap();

        let first = load(root.join("synthetix.toml")).unwrap();
        let second = load(root.join("synthetix.toml")).unwrap();
        assert_eq!(first.verification_order, second.verification_order);
        let positions = first
            .verification_order
            .iter()
            .enumerate()
            .map(|(index, id)| (id.name.as_str(), index))
            .collect::<HashMap<_, _>>();
        assert!(positions["leaf"] < positions["middle"]);
        assert!(positions["middle"] < positions["root"]);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn reports_deep_postcondition_counterexample_after_verified_summaries() {
        let root = std::env::temp_dir().join(format!(
            "synthetix-project-deep-counterexample-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        fs::write(
            root.join("entry.ax"),
            "contract level0(value: u32) -> u32 { requires value > 0; ensures result == value; }\n\
             contract level1(value: u32) -> u32 { requires value > 0; ensures result == level0(value); }\n\
             contract level2(value: u32) -> u32 { requires value > 0; ensures result == level1(value); }\n\
             contract root(value: u32) -> u32 { requires value > 0; ensures result == level2(value); }\n",
        )
        .unwrap();
        fs::write(
            root.join("synthetix.toml"),
            "[project]\nentry = \"entry.ax\"\ntarget = \"web\"\n",
        )
        .unwrap();

        let project = load(root.join("synthetix.toml")).unwrap();
        let mut verified = HashMap::new();
        let root_id = ContractId {
            source: fs::canonicalize(root.join("entry.ax")).unwrap(),
            name: "root".to_string(),
        };
        for id in &project.verification_order {
            if *id == root_id {
                break;
            }
            let linked = project
                .contracts
                .iter()
                .find(|linked| ContractId::from(*linked) == *id)
                .unwrap();
            let proof = project
                .verify_compositional_with_verified_summaries(linked, &Expr::Arg(0), &verified)
                .unwrap();
            assert_eq!(proof.proof, Some(axiom_verify::ProofResult::Verified));
            verified.insert(id.clone(), linked.contract.clone());
        }
        let linked_root = project
            .contracts
            .iter()
            .find(|linked| ContractId::from(*linked) == root_id)
            .unwrap();
        let proof = project
            .verify_compositional_with_verified_summaries(linked_root, &Expr::Const(0), &verified)
            .unwrap();
        assert!(matches!(
            proof.proof,
            Some(axiom_verify::ProofResult::Counterexample(_))
        ));
        assert_eq!(proof.diagnostic, None);

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
    fn keeps_verified_fixed_arrays_out_of_application_state() {
        let root = std::env::temp_dir().join(format!(
            "synthetix-project-verified-array-boundary-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        fs::write(
            root.join("entry.ax"),
            "page Boundary { state values: [u32; 2] = []; }",
        )
        .unwrap();
        fs::write(
            root.join("synthetix.toml"),
            "[project]\nentry = \"entry.ax\"\ntarget = \"web\"\n",
        )
        .unwrap();

        let error = load(root.join("synthetix.toml")).unwrap_err();
        assert!(error.contains("verified fixed arrays cannot be used as application state"));

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
