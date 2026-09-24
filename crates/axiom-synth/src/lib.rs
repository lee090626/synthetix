//! Minimal enumerative synthesizer for the first `mul8` vertical slice.

use std::{
    collections::{HashMap, HashSet},
    time::{Duration, Instant},
};

use axiom_ami::Expr;
use axiom_parser::{BinaryOp, Contract, DeclarationKind, SpecExpr, Type};
use axiom_verify::{
    ContractCheck, PreconditionResult, ProofResult, check_contract, check_preconditions, verify,
};

#[derive(Debug, Clone, Copy)]
pub struct SynthesisOptions {
    pub timeout: Option<Duration>,
    pub max_candidates: usize,
    pub max_memory_bytes: usize,
}

impl Default for SynthesisOptions {
    fn default() -> Self {
        Self {
            timeout: None,
            max_candidates: 50_000,
            max_memory_bytes: 64 * 1024 * 1024,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SynthesisResult {
    Found {
        candidate: Expr,
        diagnostic: Option<axiom_verify::Diagnostic>,
        candidates_checked: usize,
    },
    NoCandidate {
        candidates_checked: usize,
        last_counterexample: Option<Vec<u32>>,
    },
    Timeout {
        candidates_checked: usize,
    },
    ResourceLimit {
        candidates_checked: usize,
        limit: usize,
    },
}

impl SynthesisResult {
    pub fn status_name(&self) -> &'static str {
        match self {
            Self::Found { .. } => "FOUND",
            Self::NoCandidate { .. } => "NO_CANDIDATE",
            Self::Timeout { .. } => "TIMEOUT",
            Self::ResourceLimit { .. } => "RESOURCE_LIMIT",
        }
    }

    pub fn candidates_checked(&self) -> usize {
        match self {
            Self::Found {
                candidates_checked, ..
            }
            | Self::NoCandidate {
                candidates_checked, ..
            }
            | Self::Timeout { candidates_checked }
            | Self::ResourceLimit {
                candidates_checked, ..
            } => *candidates_checked,
        }
    }
}

/// Optional search and solver counters for benchmark runs. Normal synthesis
/// APIs keep their existing uninstrumented path.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SynthesisMetrics {
    pub candidate_count: usize,
    pub generated_candidate_count: usize,
    pub synthesis_iterations: usize,
    pub search_batches: usize,
    pub counterexample_count: usize,
    pub initial_sample_count: usize,
    pub peak_sample_count: usize,
    pub candidate_generation_time_ns: u128,
    pub sample_generation_time_ns: u128,
    pub solver_invocations: usize,
    pub solver_time_ns: u128,
    pub peak_resource_estimate_bytes: usize,
}

impl SynthesisMetrics {
    fn merge_verification(&mut self, metrics: axiom_verify::VerificationMetrics) {
        self.solver_invocations += metrics.solver_invocations;
        self.solver_time_ns += metrics.solver_time_ns;
    }
}

/// Finds the first verified candidate in the bounded search space.
pub fn synthesize(contract: &Contract) -> Result<Expr, String> {
    synthesize_with_options(contract, SynthesisOptions::default())
}

/// Finds a candidate while preserving the bounded-search outcome.
pub fn synthesize_result_with_options(
    contract: &Contract,
    options: SynthesisOptions,
) -> Result<SynthesisResult, String> {
    synthesize_result_inner(contract, options, &mut None)
}

/// Runs the same bounded search as `synthesize_result_with_options` and also
/// collects counters for benchmark reporting.
pub fn synthesize_result_with_metrics(
    contract: &Contract,
    options: SynthesisOptions,
) -> Result<(SynthesisResult, SynthesisMetrics), String> {
    let mut metrics = SynthesisMetrics::default();
    let result = synthesize_result_inner(contract, options, &mut Some(&mut metrics))?;
    metrics.candidate_count = metrics.synthesis_iterations;
    Ok((result, metrics))
}

fn synthesize_result_inner(
    contract: &Contract,
    options: SynthesisOptions,
    metrics: &mut Option<&mut SynthesisMetrics>,
) -> Result<SynthesisResult, String> {
    if contract.kind == DeclarationKind::Extern {
        return Err(
            "EXTERN_OUTSIDE_VERIFICATION_BOUNDARY: extern declarations require a runtime adapter"
                .to_string(),
        );
    }
    let contract_check = if let Some(metrics) = metrics.as_deref_mut() {
        let (result, solver_metrics) = axiom_verify::check_contract_with_metrics(contract)?;
        metrics.merge_verification(solver_metrics);
        result
    } else {
        check_contract(contract)?
    };
    match contract_check {
        ContractCheck::Valid => {}
        ContractCheck::Invalid(error) => return Err(format!("INVALID: {error}")),
        ContractCheck::UnsatisfiablePrecondition => {
            return Err(
                "UNSATISFIABLE_PRECONDITION: requires clauses are contradictory".to_string(),
            )
        }
        ContractCheck::UnsatisfiablePostcondition => {
            return Err(
                "UNSATISFIABLE_POSTCONDITION: ensures clauses have no valid result; specification may be contradictory"
                    .to_string(),
            )
        }
    }
    if contract.kind == DeclarationKind::Function {
        let candidate = axiom_verify::implementation_to_ami(contract)?;
        return Ok(SynthesisResult::Found {
            candidate,
            diagnostic: None,
            candidates_checked: 0,
        });
    }
    search_with_options(contract, options, metrics)
}

/// Finds the cheapest verified candidate within the configured bounded search.
pub fn synthesize_with_options(
    contract: &Contract,
    options: SynthesisOptions,
) -> Result<Expr, String> {
    match synthesize_result_with_options(contract, options)? {
        SynthesisResult::Found { candidate, .. } => Ok(candidate),
        result => Err(format_synthesis_result(&result)),
    }
}

fn search_with_options(
    contract: &Contract,
    options: SynthesisOptions,
    metrics: &mut Option<&mut SynthesisMetrics>,
) -> Result<SynthesisResult, String> {
    let preconditions = if let Some(metrics) = metrics.as_deref_mut() {
        let (result, solver_metrics) = axiom_verify::check_preconditions_with_metrics(contract)?;
        metrics.merge_verification(solver_metrics);
        result
    } else {
        check_preconditions(contract)?
    };
    match preconditions {
        PreconditionResult::Satisfiable => {}
        PreconditionResult::UnsatisfiablePrecondition => {
            return Err(
                "UNSATISFIABLE_PRECONDITION: requires clauses are contradictory".to_string(),
            );
        }
        PreconditionResult::SolverUnknown => {
            return Err("SOLVER_UNKNOWN: precondition satisfiability is unknown".to_string());
        }
    }

    let started = Instant::now();
    let mut checked = 0;
    let bytes_per_sample = std::mem::size_of::<Vec<u32>>()
        .saturating_add(
            contract
                .params
                .len()
                .saturating_mul(std::mem::size_of::<u32>()),
        )
        .max(1);
    let sample_limit = options
        .max_candidates
        .min((options.max_memory_bytes / 4) / bytes_per_sample);
    let sample_started = metrics.as_ref().is_some().then(Instant::now);
    let mut cegis = match CegisState::new(contract, sample_limit, started, options.timeout) {
        Ok(state) => state,
        Err(SampleError::Timeout) => {
            return Ok(SynthesisResult::Timeout {
                candidates_checked: 0,
            });
        }
        Err(SampleError::ResourceLimit(limit)) => {
            return Ok(SynthesisResult::ResourceLimit {
                candidates_checked: 0,
                limit,
            });
        }
    };
    if let (Some(metrics), Some(sample_started)) = (metrics.as_deref_mut(), sample_started) {
        metrics.sample_generation_time_ns += sample_started.elapsed().as_nanos();
        metrics.initial_sample_count = cegis.samples.len();
        metrics.peak_sample_count = cegis.samples.len();
        metrics.peak_resource_estimate_bytes = cegis.samples.len().saturating_mul(bytes_per_sample);
    }
    let mut first_verified = None;
    let generation_limit = options
        .max_candidates
        .min(options.max_memory_bytes / (std::mem::size_of::<Expr>().saturating_mul(32).max(1)));

    if (contract.return_type == Type::Bool
        || contract.ensures.iter().any(contains_conditional_construct))
        && let Some(candidate) = candidate_from_contract(contract)?
        && cegis.matches(contract, &candidate)
    {
        if let Some(metrics) = metrics.as_deref_mut() {
            metrics.synthesis_iterations += 1;
        }
        match verify_for_search(contract, &candidate, metrics)? {
            ProofResult::Verified => first_verified = Some(candidate),
            ProofResult::Counterexample(values) => {
                if let Some(metrics) = metrics.as_deref_mut() {
                    metrics.counterexample_count += 1;
                    metrics.peak_sample_count =
                        metrics.peak_sample_count.max(cegis.samples.len() + 1);
                }
                if !cegis.observe(values) {
                    return Ok(SynthesisResult::ResourceLimit {
                        candidates_checked: checked,
                        limit: sample_limit,
                    });
                }
            }
            ProofResult::SolverUnknown => {
                return Err("SOLVER_UNKNOWN: SMT solver returned unknown".to_string());
            }
        }
    }

    let mut generation_truncated = generation_limit == 0;
    for batch_index in 0..6 {
        let generation_started = metrics.as_ref().is_some().then(Instant::now);
        let candidates = match batch_index {
            0 => base_candidates(
                contract.params.len(),
                generation_limit,
                started,
                options.timeout,
            ),
            1 => depth_one_candidates(
                contract.params.len(),
                generation_limit,
                started,
                options.timeout,
            ),
            2 => conditional_candidates(
                contract.params.len(),
                generation_limit,
                started,
                options.timeout,
            ),
            3 => constant_conditional_candidates(
                contract.params.len(),
                generation_limit,
                started,
                options.timeout,
            ),
            4 => {
                let conditional = conditional_candidates(
                    contract.params.len(),
                    generation_limit,
                    started,
                    options.timeout,
                );
                nested_conditional_candidates(
                    contract.params.len(),
                    &conditional,
                    generation_limit,
                    started,
                    options.timeout,
                )
            }
            5 => depth_two_candidates(
                contract.params.len(),
                generation_limit,
                started,
                options.timeout,
            ),
            _ => unreachable!("fixed synthesis batch index"),
        };
        if let (Some(metrics), Some(generation_started)) =
            (metrics.as_deref_mut(), generation_started)
        {
            metrics.search_batches += 1;
            metrics.generated_candidate_count += candidates.len();
            metrics.candidate_generation_time_ns += generation_started.elapsed().as_nanos();
        }
        generation_truncated |= generation_limit > 0 && candidates.len() >= generation_limit;
        if timed_out(started, options.timeout) {
            if let Some(candidate) = first_verified.clone() {
                return Ok(SynthesisResult::Found {
                    candidate,
                    diagnostic: None,
                    candidates_checked: checked,
                });
            }
            return Ok(SynthesisResult::Timeout {
                candidates_checked: checked,
            });
        }
        let mut candidates = typed_candidates(candidates, contract);
        let estimated_memory = candidates
            .iter()
            .map(estimated_candidate_bytes)
            .sum::<usize>();
        if let Some(metrics) = metrics.as_deref_mut() {
            let sample_bytes = cegis.samples.len().saturating_mul(bytes_per_sample);
            metrics.peak_resource_estimate_bytes = metrics
                .peak_resource_estimate_bytes
                .max(sample_bytes.saturating_add(estimated_memory));
        }
        if estimated_memory > options.max_memory_bytes {
            return Ok(SynthesisResult::ResourceLimit {
                candidates_checked: checked,
                limit: options.max_memory_bytes,
            });
        }
        // Stable generation order is deterministic and preserves the existing
        // preference for cheaper source-shaped candidates on equal-cost ties.
        candidates.sort_by_key(axiom_wasm::instruction_cost);
        for candidate in candidates {
            if timed_out(started, options.timeout) {
                if let Some(candidate) = &first_verified {
                    return Ok(SynthesisResult::Found {
                        candidate: candidate.clone(),
                        diagnostic: None,
                        candidates_checked: checked,
                    });
                }
                return Ok(SynthesisResult::Timeout {
                    candidates_checked: checked,
                });
            }
            if checked >= options.max_candidates {
                if let Some(candidate) = &first_verified {
                    return Ok(SynthesisResult::Found {
                        candidate: candidate.clone(),
                        diagnostic: None,
                        candidates_checked: checked,
                    });
                }
                return Ok(SynthesisResult::ResourceLimit {
                    candidates_checked: checked,
                    limit: options.max_candidates,
                });
            }
            if !cegis.matches(contract, &candidate) {
                continue;
            }
            checked += 1;
            if let Some(metrics) = metrics.as_deref_mut() {
                metrics.synthesis_iterations += 1;
            }
            match verify_for_search(contract, &candidate, metrics)? {
                ProofResult::Verified => {
                    if let Some(first) = &first_verified {
                        if first != &candidate {
                            return Ok(SynthesisResult::Found {
                                candidate: first.clone(),
                                diagnostic: Some(axiom_verify::Diagnostic::AmbiguousCandidates),
                                candidates_checked: checked,
                            });
                        }
                    } else {
                        first_verified = Some(candidate);
                    }
                }
                ProofResult::Counterexample(values) => {
                    if let Some(metrics) = metrics.as_deref_mut() {
                        metrics.counterexample_count += 1;
                        metrics.peak_sample_count =
                            metrics.peak_sample_count.max(cegis.samples.len() + 1);
                    }
                    if !cegis.observe(values) {
                        return Ok(SynthesisResult::ResourceLimit {
                            candidates_checked: checked,
                            limit: sample_limit,
                        });
                    }
                }
                ProofResult::SolverUnknown => {
                    return Err("SOLVER_UNKNOWN: SMT solver returned unknown".to_string());
                }
            }
        }
    }

    if let Some(candidate) = first_verified {
        return Ok(SynthesisResult::Found {
            candidate,
            diagnostic: None,
            candidates_checked: checked,
        });
    }

    if generation_truncated {
        return Ok(SynthesisResult::ResourceLimit {
            candidates_checked: checked,
            limit: generation_limit,
        });
    }

    Ok(SynthesisResult::NoCandidate {
        candidates_checked: checked,
        last_counterexample: cegis.last_counterexample,
    })
}

fn verify_for_search(
    contract: &Contract,
    candidate: &Expr,
    metrics: &mut Option<&mut SynthesisMetrics>,
) -> Result<ProofResult, String> {
    if let Some(metrics) = metrics.as_deref_mut() {
        let (result, solver_metrics) = axiom_verify::verify_with_metrics(contract, candidate)?;
        metrics.merge_verification(solver_metrics);
        Ok(result)
    } else {
        verify(contract, candidate)
    }
}

fn format_synthesis_result(result: &SynthesisResult) -> String {
    match result {
        SynthesisResult::Found { .. } => "FOUND: verified candidate".to_string(),
        SynthesisResult::NoCandidate {
            candidates_checked,
            last_counterexample,
        } => {
            let counterexample = last_counterexample
                .as_ref()
                .map(|values| format!("; last counterexample: {values:?}"))
                .unwrap_or_default();
            format!(
                "NO_CANDIDATE: no verified candidate found in the bounded search space (candidates checked: {candidates_checked}{counterexample}); the specification may be outside the current synthesis boundary"
            )
        }
        SynthesisResult::Timeout { candidates_checked } => {
            format!("TIMEOUT: synthesis timed out after checking {candidates_checked} candidates")
        }
        SynthesisResult::ResourceLimit {
            candidates_checked,
            limit,
        } => format!(
            "RESOURCE_LIMIT: synthesis reached the candidate limit ({candidates_checked}/{limit})"
        ),
    }
}

struct CegisState {
    samples: Vec<Vec<u32>>,
    last_counterexample: Option<Vec<u32>>,
    max_samples: usize,
}

impl CegisState {
    fn new(
        contract: &Contract,
        max_samples: usize,
        started: Instant,
        timeout: Option<Duration>,
    ) -> Result<Self, SampleError> {
        Ok(Self {
            samples: sample_inputs(contract, max_samples, started, timeout)?,
            last_counterexample: None,
            max_samples,
        })
    }

    fn matches(&self, contract: &Contract, candidate: &Expr) -> bool {
        matches_samples(contract, candidate, &self.samples)
    }

    fn observe(&mut self, counterexample: Vec<u32>) -> bool {
        self.last_counterexample = Some(counterexample.clone());
        if !self.samples.contains(&counterexample) {
            if self.samples.len() >= self.max_samples {
                return false;
            }
            self.samples.push(counterexample);
        }
        true
    }
}

#[derive(Debug)]
enum SampleError {
    Timeout,
    ResourceLimit(usize),
}

/// Synthesizes a candidate and lowers the verified result to WebAssembly.
pub fn synthesize_wasm(contract: &Contract) -> Result<(Expr, Vec<u8>), String> {
    synthesize_wasm_with_options(contract, SynthesisOptions::default())
}

/// Synthesizes a candidate and lowers it using explicit search limits.
pub fn synthesize_wasm_with_options(
    contract: &Contract,
    options: SynthesisOptions,
) -> Result<(Expr, Vec<u8>), String> {
    let (candidate, module, _) = synthesize_wasm_with_diagnostic(contract, options)?;
    Ok((candidate, module))
}

/// Synthesizes and lowers a candidate while preserving search diagnostics.
pub fn synthesize_wasm_with_diagnostic(
    contract: &Contract,
    options: SynthesisOptions,
) -> Result<(Expr, Vec<u8>, Option<axiom_verify::Diagnostic>), String> {
    let (candidate, module, diagnostic, _) = synthesize_wasm_with_metrics(contract, options)?;
    Ok((candidate, module, diagnostic))
}

/// Synthesizes and lowers a candidate with the bounded-search count.
pub fn synthesize_wasm_with_metrics(
    contract: &Contract,
    options: SynthesisOptions,
) -> Result<(Expr, Vec<u8>, Option<axiom_verify::Diagnostic>, usize), String> {
    let result = synthesize_result_with_options(contract, options)?;
    let (candidate, diagnostic, candidates_checked) = match result {
        SynthesisResult::Found {
            candidate,
            diagnostic,
            candidates_checked,
        } => (candidate, diagnostic, candidates_checked),
        result => return Err(format_synthesis_result(&result)),
    };
    let module = axiom_wasm::emit(&candidate, contract.params.len() as u32)?;
    Ok((candidate, module, diagnostic, candidates_checked))
}

/// Lowers the contract precondition to a scalar AMI predicate for runtime checks.
/// An empty `requires` clause list means every input satisfies the precondition.
pub fn lower_requires(contract: &Contract) -> Result<Option<Expr>, String> {
    contract
        .requires
        .iter()
        .map(|condition| lower_spec_expr(condition, contract, &HashMap::new()))
        .try_fold(None, |combined, condition| {
            Ok(Some(match combined {
                Some(previous) => Expr::And(Box::new(previous), Box::new(condition?)),
                None => condition?,
            }))
        })
}

/// Checks a concrete input against every `requires` clause.
pub fn check_requires(contract: &Contract, args: &[u32]) -> Result<(), String> {
    if args.len() != contract.params.len() {
        return Err(format!(
            "expected {} inputs, got {}",
            contract.params.len(),
            args.len()
        ));
    }

    for expr in &contract.requires {
        match eval_spec(expr, contract, args, 0, &HashMap::new())? {
            Value::Bool(true) => {}
            Value::Bool(false) => return Err("input violates a requires clause".to_string()),
            Value::U32(_) | Value::I32(_) => {
                return Err("requires clause must be boolean".to_string());
            }
        }
    }
    Ok(())
}

fn argument_candidates(
    argument_count: usize,
    limit: usize,
    started: Instant,
    timeout: Option<Duration>,
) -> Vec<Expr> {
    let mut args = Vec::new();
    for index in 0..argument_count.min(limit) {
        if timed_out(started, timeout) {
            break;
        }
        args.push(Expr::Arg(index as u8));
    }
    args
}

fn base_candidates(
    argument_count: usize,
    limit: usize,
    started: Instant,
    timeout: Option<Duration>,
) -> Vec<Expr> {
    let mut base = argument_candidates(argument_count, limit, started, timeout);
    for value in 0..=16 {
        if base.len() == limit || timed_out(started, timeout) {
            break;
        }
        base.push(Expr::Const(value));
    }
    base
}

fn typed_candidates(candidates: Vec<Expr>, contract: &Contract) -> Vec<Expr> {
    let mut seen = HashSet::new();
    candidates
        .into_iter()
        .map(canonicalize)
        .filter(|candidate| {
            candidate_type(candidate, contract) == Some(contract.return_type.clone())
        })
        .filter(|candidate| seen.insert(candidate.clone()))
        .collect()
}

fn candidate_type(expr: &Expr, contract: &Contract) -> Option<Type> {
    match expr {
        Expr::Arg(index) => contract
            .params
            .get(*index as usize)
            .map(|param| param.ty.clone()),
        Expr::Const(_) => Some(Type::U32),
        Expr::Add(lhs, rhs)
        | Expr::Sub(lhs, rhs)
        | Expr::Mul(lhs, rhs)
        | Expr::Shl(lhs, rhs)
        | Expr::BitAnd(lhs, rhs)
        | Expr::BitOr(lhs, rhs)
        | Expr::BitXor(lhs, rhs) => {
            numeric_candidate_type(candidate_type(lhs, contract), candidate_type(rhs, contract))
        }
        Expr::Eq(lhs, rhs) | Expr::Ne(lhs, rhs) => {
            let lhs_type = candidate_type(lhs, contract)?;
            let rhs_type = candidate_type(rhs, contract)?;
            if lhs_type == rhs_type
                || numeric_candidate_type(Some(lhs_type), Some(rhs_type)).is_some()
            {
                Some(Type::Bool)
            } else {
                None
            }
        }
        Expr::Lt(lhs, rhs)
        | Expr::Le(lhs, rhs)
        | Expr::Gt(lhs, rhs)
        | Expr::Ge(lhs, rhs)
        | Expr::SLt(lhs, rhs)
        | Expr::SLe(lhs, rhs)
        | Expr::SGt(lhs, rhs)
        | Expr::SGe(lhs, rhs) => {
            numeric_candidate_type(candidate_type(lhs, contract), candidate_type(rhs, contract))
                .map(|_| Type::Bool)
        }
        Expr::And(lhs, rhs) | Expr::Or(lhs, rhs)
            if candidate_type(lhs, contract) == Some(Type::Bool)
                && candidate_type(rhs, contract) == Some(Type::Bool) =>
        {
            Some(Type::Bool)
        }
        Expr::And(_, _) | Expr::Or(_, _) => None,
        Expr::Select {
            condition,
            then_branch,
            else_branch,
        } if candidate_type(condition, contract) == Some(Type::Bool) => {
            let then_type = candidate_type(then_branch, contract)?;
            let else_type = candidate_type(else_branch, contract)?;
            (then_type == else_type).then_some(then_type)
        }
        Expr::Select { .. } => None,
    }
}

fn numeric_candidate_type(lhs: Option<Type>, rhs: Option<Type>) -> Option<Type> {
    match (lhs?, rhs?) {
        (Type::U32, Type::U32) => Some(Type::U32),
        (Type::I32, Type::I32) | (Type::I32, Type::U32) | (Type::U32, Type::I32) => Some(Type::I32),
        _ => None,
    }
}

fn canonicalize(expr: Expr) -> Expr {
    match expr {
        Expr::Add(lhs, rhs) => {
            let (lhs, rhs) = ordered(*lhs, *rhs);
            Expr::Add(Box::new(lhs), Box::new(rhs))
        }
        Expr::Mul(lhs, rhs) => {
            let (lhs, rhs) = ordered(*lhs, *rhs);
            Expr::Mul(Box::new(lhs), Box::new(rhs))
        }
        Expr::BitAnd(lhs, rhs) => {
            let (lhs, rhs) = ordered(*lhs, *rhs);
            Expr::BitAnd(Box::new(lhs), Box::new(rhs))
        }
        Expr::BitOr(lhs, rhs) => {
            let (lhs, rhs) = ordered(*lhs, *rhs);
            Expr::BitOr(Box::new(lhs), Box::new(rhs))
        }
        Expr::BitXor(lhs, rhs) => {
            let (lhs, rhs) = ordered(*lhs, *rhs);
            Expr::BitXor(Box::new(lhs), Box::new(rhs))
        }
        Expr::Eq(lhs, rhs) => {
            let (lhs, rhs) = ordered(*lhs, *rhs);
            Expr::Eq(Box::new(lhs), Box::new(rhs))
        }
        Expr::Ne(lhs, rhs) => {
            let (lhs, rhs) = ordered(*lhs, *rhs);
            Expr::Ne(Box::new(lhs), Box::new(rhs))
        }
        Expr::And(lhs, rhs) => {
            let (lhs, rhs) = ordered(*lhs, *rhs);
            Expr::And(Box::new(lhs), Box::new(rhs))
        }
        Expr::Or(lhs, rhs) => {
            let (lhs, rhs) = ordered(*lhs, *rhs);
            Expr::Or(Box::new(lhs), Box::new(rhs))
        }
        expr => expr,
    }
}

fn ordered(lhs: Expr, rhs: Expr) -> (Expr, Expr) {
    if rhs < lhs { (rhs, lhs) } else { (lhs, rhs) }
}

fn estimated_candidate_bytes(expr: &Expr) -> usize {
    let own = std::mem::size_of::<Expr>();
    match expr {
        Expr::Arg(_) | Expr::Const(_) => own,
        Expr::Add(lhs, rhs)
        | Expr::Sub(lhs, rhs)
        | Expr::Mul(lhs, rhs)
        | Expr::Shl(lhs, rhs)
        | Expr::BitAnd(lhs, rhs)
        | Expr::BitOr(lhs, rhs)
        | Expr::BitXor(lhs, rhs)
        | Expr::Eq(lhs, rhs)
        | Expr::Ne(lhs, rhs)
        | Expr::Lt(lhs, rhs)
        | Expr::Le(lhs, rhs)
        | Expr::Gt(lhs, rhs)
        | Expr::Ge(lhs, rhs)
        | Expr::SLt(lhs, rhs)
        | Expr::SLe(lhs, rhs)
        | Expr::SGt(lhs, rhs)
        | Expr::SGe(lhs, rhs)
        | Expr::And(lhs, rhs)
        | Expr::Or(lhs, rhs) => {
            own + estimated_candidate_bytes(lhs) + estimated_candidate_bytes(rhs)
        }
        Expr::Select {
            condition,
            then_branch,
            else_branch,
        } => {
            own + estimated_candidate_bytes(condition)
                + estimated_candidate_bytes(then_branch)
                + estimated_candidate_bytes(else_branch)
        }
    }
}

fn depth_one_candidates(
    argument_count: usize,
    limit: usize,
    started: Instant,
    timeout: Option<Duration>,
) -> Vec<Expr> {
    let base = base_candidates(argument_count, limit, started, timeout);
    combine_candidates(&base, &base, limit, started, timeout)
}

fn depth_two_candidates(
    argument_count: usize,
    limit: usize,
    started: Instant,
    timeout: Option<Duration>,
) -> Vec<Expr> {
    // ponytail: cap depth-two enumeration; raise the bound after measuring real workloads.
    let base = base_candidates(argument_count, limit, started, timeout);
    let depth_one = depth_one_candidates(argument_count, limit, started, timeout);
    let mut candidates = combine_candidates(&depth_one, &base, limit, started, timeout);
    if candidates.len() < limit && !timed_out(started, timeout) {
        candidates.extend(combine_candidates(
            &base,
            &depth_one,
            limit - candidates.len(),
            started,
            timeout,
        ));
    }
    dedupe_and_cap(candidates, limit)
}

fn conditional_candidates(
    argument_count: usize,
    limit: usize,
    started: Instant,
    timeout: Option<Duration>,
) -> Vec<Expr> {
    let args = argument_candidates(argument_count, limit, started, timeout);
    let conditions = comparison_candidates(&args, limit, started, timeout);
    let mut candidates = Vec::new();

    for condition in &conditions {
        for then_branch in &args {
            for else_branch in &args {
                if timed_out(started, timeout) {
                    return dedupe_and_cap(candidates, limit);
                }
                candidates.push(Expr::Select {
                    condition: Box::new(condition.clone()),
                    then_branch: Box::new(then_branch.clone()),
                    else_branch: Box::new(else_branch.clone()),
                });
                if candidates.len() == limit {
                    return dedupe_and_cap(candidates, limit);
                }
            }
        }
    }

    dedupe_and_cap(candidates, limit)
}

fn constant_conditional_candidates(
    argument_count: usize,
    limit: usize,
    started: Instant,
    timeout: Option<Duration>,
) -> Vec<Expr> {
    // ponytail: constants stay in the small 0..=16 seed set; widen after benchmarks.
    if limit == 0 {
        return Vec::new();
    }

    let args = argument_candidates(argument_count, limit, started, timeout);
    let constants = (0..=16).map(Expr::Const).collect::<Vec<_>>();
    let mut values = args.clone();
    values.extend(constants.iter().cloned());
    let mut conditions = Vec::new();

    for arg in &args {
        for constant in &constants {
            if timed_out(started, timeout) {
                return Vec::new();
            }
            conditions.extend([
                Expr::Gt(Box::new(arg.clone()), Box::new(constant.clone())),
                Expr::Ge(Box::new(arg.clone()), Box::new(constant.clone())),
                Expr::Lt(Box::new(arg.clone()), Box::new(constant.clone())),
                Expr::Le(Box::new(arg.clone()), Box::new(constant.clone())),
                Expr::Gt(Box::new(constant.clone()), Box::new(arg.clone())),
                Expr::Ge(Box::new(constant.clone()), Box::new(arg.clone())),
                Expr::Lt(Box::new(constant.clone()), Box::new(arg.clone())),
                Expr::Le(Box::new(constant.clone()), Box::new(arg.clone())),
            ]);
            if conditions.len() >= limit {
                break;
            }
        }
        if conditions.len() >= limit {
            break;
        }
    }

    let mut candidates = Vec::new();
    for condition in &conditions {
        for then_branch in &values {
            for else_branch in &values {
                if timed_out(started, timeout) {
                    return dedupe_and_cap(candidates, limit);
                }
                candidates.push(Expr::Select {
                    condition: Box::new(condition.clone()),
                    then_branch: Box::new(then_branch.clone()),
                    else_branch: Box::new(else_branch.clone()),
                });
                if candidates.len() == limit {
                    return candidates;
                }
            }
        }
    }

    dedupe_and_cap(candidates, limit)
}

fn nested_conditional_candidates(
    argument_count: usize,
    inner_candidates: &[Expr],
    limit: usize,
    started: Instant,
    timeout: Option<Duration>,
) -> Vec<Expr> {
    // ponytail: one nested select and parameter-only branches; add deeper trees after benchmarks.
    let args = argument_candidates(argument_count, limit, started, timeout);
    let conditions = comparison_candidates(&args, limit, started, timeout);
    let mut candidates = Vec::new();

    for condition in &conditions {
        for branch in &args {
            for inner in inner_candidates {
                if timed_out(started, timeout) {
                    return dedupe_and_cap(candidates, limit);
                }
                candidates.push(Expr::Select {
                    condition: Box::new(condition.clone()),
                    then_branch: Box::new(branch.clone()),
                    else_branch: Box::new(inner.clone()),
                });
                if candidates.len() == limit {
                    return dedupe_and_cap(candidates, limit);
                }
            }
        }
    }

    dedupe_and_cap(candidates, limit)
}

fn comparison_candidates(
    args: &[Expr],
    limit: usize,
    started: Instant,
    timeout: Option<Duration>,
) -> Vec<Expr> {
    let mut candidates = Vec::new();
    'pairs: for lhs in args {
        for rhs in args {
            if timed_out(started, timeout) {
                break 'pairs;
            }
            candidates.extend([
                Expr::Gt(Box::new(lhs.clone()), Box::new(rhs.clone())),
                Expr::Ge(Box::new(lhs.clone()), Box::new(rhs.clone())),
                Expr::Lt(Box::new(lhs.clone()), Box::new(rhs.clone())),
                Expr::Le(Box::new(lhs.clone()), Box::new(rhs.clone())),
            ]);
            if candidates.len() >= limit {
                candidates.truncate(limit);
                break 'pairs;
            }
        }
    }
    candidates
}

fn combine_candidates(
    lhs: &[Expr],
    rhs: &[Expr],
    limit: usize,
    started: Instant,
    timeout: Option<Duration>,
) -> Vec<Expr> {
    let mut seen = HashSet::new();
    let mut candidates = Vec::new();
    'pairs: for lhs in lhs {
        for rhs in rhs {
            if timed_out(started, timeout) {
                break 'pairs;
            }
            for candidate in [
                Expr::Shl(Box::new(lhs.clone()), Box::new(rhs.clone())),
                Expr::Mul(Box::new(lhs.clone()), Box::new(rhs.clone())),
                Expr::Add(Box::new(lhs.clone()), Box::new(rhs.clone())),
                Expr::Sub(Box::new(lhs.clone()), Box::new(rhs.clone())),
                Expr::BitAnd(Box::new(lhs.clone()), Box::new(rhs.clone())),
                Expr::BitOr(Box::new(lhs.clone()), Box::new(rhs.clone())),
                Expr::BitXor(Box::new(lhs.clone()), Box::new(rhs.clone())),
            ] {
                if seen.insert(candidate.clone()) {
                    candidates.push(candidate);
                    if candidates.len() == limit {
                        break 'pairs;
                    }
                }
            }
        }
    }
    candidates
}

fn dedupe_and_cap(candidates: Vec<Expr>, limit: usize) -> Vec<Expr> {
    let mut seen = HashSet::new();
    candidates
        .into_iter()
        .map(canonicalize)
        .filter(|candidate| seen.insert(candidate.clone()))
        .take(limit)
        .collect()
}

fn timed_out(started: Instant, timeout: Option<Duration>) -> bool {
    timeout.is_some_and(|timeout| started.elapsed() >= timeout)
}

fn sample_inputs(
    contract: &Contract,
    max_samples: usize,
    started: Instant,
    timeout: Option<Duration>,
) -> Result<Vec<Vec<u32>>, SampleError> {
    const VALUES: [u32; 10] = [
        0,
        1,
        2,
        3,
        7,
        8,
        0x7fff_ffff,
        0x8000_0000,
        u32::MAX - 1,
        u32::MAX,
    ];
    if timed_out(started, timeout) {
        return Err(SampleError::Timeout);
    }
    if max_samples == 0 {
        return Err(SampleError::ResourceLimit(0));
    }
    let mut samples = vec![Vec::new()];
    for param in &contract.params {
        if timed_out(started, timeout) {
            return Err(SampleError::Timeout);
        }
        let values: &[u32] = if param.ty == Type::Bool {
            &[0, 1]
        } else {
            &VALUES
        };
        let Some(next_len) = samples.len().checked_mul(values.len()) else {
            return Err(SampleError::ResourceLimit(max_samples));
        };
        if next_len > max_samples {
            return Err(SampleError::ResourceLimit(max_samples));
        }
        let mut next = Vec::with_capacity(next_len);
        for prefix in &samples {
            for value in values {
                if timed_out(started, timeout) {
                    return Err(SampleError::Timeout);
                }
                let mut args = prefix.clone();
                args.push(*value);
                next.push(args);
            }
        }
        samples = next;
    }
    Ok(samples)
}

fn matches_samples(contract: &Contract, candidate: &Expr, samples: &[Vec<u32>]) -> bool {
    samples.iter().all(|args| {
        let result = candidate.eval(args);
        let requires = contract.requires.iter().all(|expr| {
            matches!(
                eval_spec(expr, contract, args, result, &HashMap::new()),
                Ok(Value::Bool(true))
            )
        });
        !requires
            || contract.ensures.iter().all(|expr| {
                matches!(
                    eval_spec(expr, contract, args, result, &HashMap::new()),
                    Ok(Value::Bool(true))
                )
            })
    })
}

#[derive(Clone)]
enum Value {
    U32(u32),
    I32(i32),
    Bool(bool),
}

fn eval_spec(
    expr: &SpecExpr,
    contract: &Contract,
    args: &[u32],
    result: u32,
    locals: &HashMap<String, Value>,
) -> Result<Value, String> {
    match expr {
        SpecExpr::Ident(name) if name == "result" => {
            if contract.return_type == Type::Bool {
                Ok(Value::Bool(result != 0))
            } else if contract.return_type == Type::I32 {
                Ok(Value::I32(result as i32))
            } else {
                Ok(Value::U32(result))
            }
        }
        SpecExpr::Ident(name) if locals.contains_key(name) => Ok(locals[name].clone()),
        SpecExpr::Ident(name) => {
            let index = contract
                .params
                .iter()
                .position(|param| param.name == *name)
                .ok_or_else(|| format!("unknown identifier: {name}"))?;
            let value = args
                .get(index)
                .copied()
                .ok_or_else(|| format!("missing argument: {name}"))?;
            match contract.params[index].ty {
                Type::Bool => Ok(Value::Bool(value != 0)),
                Type::I32 => Ok(Value::I32(value as i32)),
                _ => Ok(Value::U32(value)),
            }
        }
        SpecExpr::Number(value) => Ok(Value::U32(*value)),
        SpecExpr::Bool(value) => Ok(Value::Bool(*value)),
        SpecExpr::Text(_) | SpecExpr::List(_) | SpecExpr::Record { .. } | SpecExpr::Field(_, _) => {
            Err("non-scalar expressions cannot be synthesized".to_string())
        }
        SpecExpr::Let(name, value, body) => {
            let value = eval_spec(value, contract, args, result, locals)?;
            let mut nested = locals.clone();
            nested.insert(name.clone(), value);
            eval_spec(body, contract, args, result, &nested)
        }
        SpecExpr::If(condition, then_branch, else_branch) => {
            let condition = match eval_spec(condition, contract, args, result, locals)? {
                Value::Bool(value) => value,
                Value::U32(_) | Value::I32(_) => {
                    return Err("if condition must be boolean".to_string());
                }
            };
            if condition {
                eval_spec(then_branch, contract, args, result, locals)
            } else {
                eval_spec(else_branch, contract, args, result, locals)
            }
        }
        SpecExpr::Call(name, _) => Err(format!(
            "unresolved contract call: {name}; link the project before synthesis"
        )),
        SpecExpr::Binary(op, lhs, rhs) => {
            let lhs = eval_spec(lhs, contract, args, result, locals)?;
            let rhs = eval_spec(rhs, contract, args, result, locals)?;
            match (op, lhs, rhs) {
                (BinaryOp::AddWrap, Value::U32(lhs), Value::U32(rhs)) => {
                    Ok(Value::U32(lhs.wrapping_add(rhs)))
                }
                (BinaryOp::AddWrap, Value::I32(lhs), Value::I32(rhs)) => {
                    Ok(Value::I32(lhs.wrapping_add(rhs)))
                }
                (BinaryOp::AddWrap, Value::I32(lhs), Value::U32(rhs)) => {
                    Ok(Value::I32(lhs.wrapping_add(rhs as i32)))
                }
                (BinaryOp::AddWrap, Value::U32(lhs), Value::I32(rhs)) => {
                    Ok(Value::I32((lhs as i32).wrapping_add(rhs)))
                }
                (BinaryOp::SubWrap, Value::U32(lhs), Value::U32(rhs)) => {
                    Ok(Value::U32(lhs.wrapping_sub(rhs)))
                }
                (BinaryOp::SubWrap, Value::I32(lhs), Value::I32(rhs)) => {
                    Ok(Value::I32(lhs.wrapping_sub(rhs)))
                }
                (BinaryOp::SubWrap, Value::I32(lhs), Value::U32(rhs)) => {
                    Ok(Value::I32(lhs.wrapping_sub(rhs as i32)))
                }
                (BinaryOp::SubWrap, Value::U32(lhs), Value::I32(rhs)) => {
                    Ok(Value::I32((lhs as i32).wrapping_sub(rhs)))
                }
                (BinaryOp::MulWrap, Value::U32(lhs), Value::U32(rhs)) => {
                    Ok(Value::U32(lhs.wrapping_mul(rhs)))
                }
                (BinaryOp::MulWrap, Value::I32(lhs), Value::I32(rhs)) => {
                    Ok(Value::I32(lhs.wrapping_mul(rhs)))
                }
                (BinaryOp::MulWrap, Value::I32(lhs), Value::U32(rhs)) => {
                    Ok(Value::I32(lhs.wrapping_mul(rhs as i32)))
                }
                (BinaryOp::MulWrap, Value::U32(lhs), Value::I32(rhs)) => {
                    Ok(Value::I32((lhs as i32).wrapping_mul(rhs)))
                }
                (BinaryOp::Shl, Value::U32(lhs), Value::U32(rhs)) => {
                    Ok(Value::U32(lhs.wrapping_shl(rhs & 31)))
                }
                (BinaryOp::Shl, Value::I32(lhs), Value::I32(rhs)) => {
                    Ok(Value::I32(lhs.wrapping_shl((rhs as u32) & 31)))
                }
                (BinaryOp::Shl, Value::I32(lhs), Value::U32(rhs)) => {
                    Ok(Value::I32(lhs.wrapping_shl(rhs & 31)))
                }
                (BinaryOp::Shl, Value::U32(lhs), Value::I32(rhs)) => {
                    Ok(Value::I32((lhs as i32).wrapping_shl((rhs as u32) & 31)))
                }
                (BinaryOp::BitAnd, Value::U32(lhs), Value::U32(rhs)) => Ok(Value::U32(lhs & rhs)),
                (BinaryOp::BitAnd, Value::I32(lhs), Value::I32(rhs)) => Ok(Value::I32(lhs & rhs)),
                (BinaryOp::BitAnd, Value::I32(lhs), Value::U32(rhs)) => {
                    Ok(Value::I32(lhs & rhs as i32))
                }
                (BinaryOp::BitAnd, Value::U32(lhs), Value::I32(rhs)) => {
                    Ok(Value::I32(lhs as i32 & rhs))
                }
                (BinaryOp::BitOr, Value::U32(lhs), Value::U32(rhs)) => Ok(Value::U32(lhs | rhs)),
                (BinaryOp::BitOr, Value::I32(lhs), Value::I32(rhs)) => Ok(Value::I32(lhs | rhs)),
                (BinaryOp::BitOr, Value::I32(lhs), Value::U32(rhs)) => {
                    Ok(Value::I32(lhs | rhs as i32))
                }
                (BinaryOp::BitOr, Value::U32(lhs), Value::I32(rhs)) => {
                    Ok(Value::I32(lhs as i32 | rhs))
                }
                (BinaryOp::BitXor, Value::U32(lhs), Value::U32(rhs)) => Ok(Value::U32(lhs ^ rhs)),
                (BinaryOp::BitXor, Value::I32(lhs), Value::I32(rhs)) => Ok(Value::I32(lhs ^ rhs)),
                (BinaryOp::BitXor, Value::I32(lhs), Value::U32(rhs)) => {
                    Ok(Value::I32(lhs ^ rhs as i32))
                }
                (BinaryOp::BitXor, Value::U32(lhs), Value::I32(rhs)) => {
                    Ok(Value::I32(lhs as i32 ^ rhs))
                }
                (BinaryOp::Eq, Value::U32(lhs), Value::U32(rhs)) => Ok(Value::Bool(lhs == rhs)),
                (BinaryOp::Ne, Value::U32(lhs), Value::U32(rhs)) => Ok(Value::Bool(lhs != rhs)),
                (BinaryOp::Eq, Value::I32(lhs), Value::I32(rhs)) => Ok(Value::Bool(lhs == rhs)),
                (BinaryOp::Eq, Value::I32(lhs), Value::U32(rhs)) => {
                    Ok(Value::Bool(lhs as u32 == rhs))
                }
                (BinaryOp::Eq, Value::U32(lhs), Value::I32(rhs)) => {
                    Ok(Value::Bool(lhs == rhs as u32))
                }
                (BinaryOp::Ne, Value::I32(lhs), Value::I32(rhs)) => Ok(Value::Bool(lhs != rhs)),
                (BinaryOp::Ne, Value::I32(lhs), Value::U32(rhs)) => {
                    Ok(Value::Bool(lhs as u32 != rhs))
                }
                (BinaryOp::Ne, Value::U32(lhs), Value::I32(rhs)) => {
                    Ok(Value::Bool(lhs != rhs as u32))
                }
                (BinaryOp::Eq, Value::Bool(lhs), Value::Bool(rhs)) => Ok(Value::Bool(lhs == rhs)),
                (BinaryOp::Ne, Value::Bool(lhs), Value::Bool(rhs)) => Ok(Value::Bool(lhs != rhs)),
                (BinaryOp::Lt, Value::U32(lhs), Value::U32(rhs)) => Ok(Value::Bool(lhs < rhs)),
                (BinaryOp::Le, Value::U32(lhs), Value::U32(rhs)) => Ok(Value::Bool(lhs <= rhs)),
                (BinaryOp::Gt, Value::U32(lhs), Value::U32(rhs)) => Ok(Value::Bool(lhs > rhs)),
                (BinaryOp::Ge, Value::U32(lhs), Value::U32(rhs)) => Ok(Value::Bool(lhs >= rhs)),
                (BinaryOp::Lt, Value::I32(lhs), Value::I32(rhs)) => Ok(Value::Bool(lhs < rhs)),
                (BinaryOp::Lt, Value::I32(lhs), Value::U32(rhs)) => {
                    Ok(Value::Bool(lhs < rhs as i32))
                }
                (BinaryOp::Lt, Value::U32(lhs), Value::I32(rhs)) => {
                    Ok(Value::Bool((lhs as i32) < rhs))
                }
                (BinaryOp::Le, Value::I32(lhs), Value::I32(rhs)) => Ok(Value::Bool(lhs <= rhs)),
                (BinaryOp::Le, Value::I32(lhs), Value::U32(rhs)) => {
                    Ok(Value::Bool(lhs <= rhs as i32))
                }
                (BinaryOp::Le, Value::U32(lhs), Value::I32(rhs)) => {
                    Ok(Value::Bool((lhs as i32) <= rhs))
                }
                (BinaryOp::Gt, Value::I32(lhs), Value::I32(rhs)) => Ok(Value::Bool(lhs > rhs)),
                (BinaryOp::Gt, Value::I32(lhs), Value::U32(rhs)) => {
                    Ok(Value::Bool(lhs > rhs as i32))
                }
                (BinaryOp::Gt, Value::U32(lhs), Value::I32(rhs)) => {
                    Ok(Value::Bool((lhs as i32) > rhs))
                }
                (BinaryOp::Ge, Value::I32(lhs), Value::I32(rhs)) => Ok(Value::Bool(lhs >= rhs)),
                (BinaryOp::Ge, Value::I32(lhs), Value::U32(rhs)) => {
                    Ok(Value::Bool(lhs >= rhs as i32))
                }
                (BinaryOp::Ge, Value::U32(lhs), Value::I32(rhs)) => {
                    Ok(Value::Bool((lhs as i32) >= rhs))
                }
                (BinaryOp::And, Value::Bool(lhs), Value::Bool(rhs)) => Ok(Value::Bool(lhs && rhs)),
                (BinaryOp::Or, Value::Bool(lhs), Value::Bool(rhs)) => Ok(Value::Bool(lhs || rhs)),
                _ => Err(format!("operator {:?} has incompatible operands", op)),
            }
        }
    }
}

fn contains_conditional_construct(expr: &SpecExpr) -> bool {
    match expr {
        SpecExpr::Let(_, _, _) | SpecExpr::If(_, _, _) => true,
        SpecExpr::Binary(_, lhs, rhs) => {
            contains_conditional_construct(lhs) || contains_conditional_construct(rhs)
        }
        SpecExpr::Ident(_) | SpecExpr::Number(_) | SpecExpr::Bool(_) | SpecExpr::Text(_) => false,
        SpecExpr::List(values) => values.iter().any(contains_conditional_construct),
        SpecExpr::Record { fields, .. } => fields
            .iter()
            .any(|(_, value)| contains_conditional_construct(value)),
        SpecExpr::Field(value, _) => contains_conditional_construct(value),
        SpecExpr::Call(_, args) => args.iter().any(contains_conditional_construct),
    }
}

fn candidate_from_contract(contract: &Contract) -> Result<Option<Expr>, String> {
    for ensure in &contract.ensures {
        let SpecExpr::Binary(BinaryOp::Eq, lhs, rhs) = ensure else {
            continue;
        };

        if matches!(lhs.as_ref(), SpecExpr::Ident(name) if name == "result") {
            return lower_spec_expr(rhs, contract, &HashMap::new()).map(Some);
        }
        if matches!(rhs.as_ref(), SpecExpr::Ident(name) if name == "result") {
            return lower_spec_expr(lhs, contract, &HashMap::new()).map(Some);
        }
    }
    Ok(None)
}

fn lower_spec_expr(
    expr: &SpecExpr,
    contract: &Contract,
    locals: &HashMap<String, Expr>,
) -> Result<Expr, String> {
    match expr {
        SpecExpr::Ident(name) if locals.contains_key(name) => Ok(locals[name].clone()),
        SpecExpr::Ident(name) => contract
            .params
            .iter()
            .position(|param| param.name == *name)
            .map(|index| Expr::Arg(index as u8))
            .ok_or_else(|| format!("unknown identifier: {name}")),
        SpecExpr::Number(value) => Ok(Expr::Const(*value)),
        SpecExpr::Bool(value) => Ok(Expr::Const(u32::from(*value))),
        SpecExpr::Text(_) | SpecExpr::List(_) | SpecExpr::Record { .. } | SpecExpr::Field(_, _) => {
            Err("non-scalar expressions cannot be lowered to WASM".to_string())
        }
        SpecExpr::Let(name, value, body) => {
            let value = lower_spec_expr(value, contract, locals)?;
            let mut nested = locals.clone();
            nested.insert(name.clone(), value);
            lower_spec_expr(body, contract, &nested)
        }
        SpecExpr::If(condition, then_branch, else_branch) => Ok(Expr::Select {
            condition: Box::new(lower_spec_expr(condition, contract, locals)?),
            then_branch: Box::new(lower_spec_expr(then_branch, contract, locals)?),
            else_branch: Box::new(lower_spec_expr(else_branch, contract, locals)?),
        }),
        SpecExpr::Call(name, _) => Err(format!(
            "unresolved contract call: {name}; link the project before synthesis"
        )),
        SpecExpr::Binary(op, lhs, rhs) => {
            let signed = matches!(
                op,
                BinaryOp::Lt | BinaryOp::Le | BinaryOp::Gt | BinaryOp::Ge
            ) && (is_i32_expr(lhs, contract) || is_i32_expr(rhs, contract));
            let lhs = Box::new(lower_spec_expr(lhs, contract, locals)?);
            let rhs = Box::new(lower_spec_expr(rhs, contract, locals)?);
            Ok(match op {
                BinaryOp::AddWrap => Expr::Add(lhs, rhs),
                BinaryOp::SubWrap => Expr::Sub(lhs, rhs),
                BinaryOp::MulWrap => Expr::Mul(lhs, rhs),
                BinaryOp::Shl => Expr::Shl(lhs, rhs),
                BinaryOp::BitAnd => Expr::BitAnd(lhs, rhs),
                BinaryOp::BitOr => Expr::BitOr(lhs, rhs),
                BinaryOp::BitXor => Expr::BitXor(lhs, rhs),
                BinaryOp::Eq => Expr::Eq(lhs, rhs),
                BinaryOp::Ne => Expr::Ne(lhs, rhs),
                BinaryOp::Lt if signed => Expr::SLt(lhs, rhs),
                BinaryOp::Le if signed => Expr::SLe(lhs, rhs),
                BinaryOp::Gt if signed => Expr::SGt(lhs, rhs),
                BinaryOp::Ge if signed => Expr::SGe(lhs, rhs),
                BinaryOp::Lt => Expr::Lt(lhs, rhs),
                BinaryOp::Le => Expr::Le(lhs, rhs),
                BinaryOp::Gt => Expr::Gt(lhs, rhs),
                BinaryOp::Ge => Expr::Ge(lhs, rhs),
                BinaryOp::And => Expr::And(lhs, rhs),
                BinaryOp::Or => Expr::Or(lhs, rhs),
            })
        }
    }
}

fn is_i32_expr(expr: &SpecExpr, contract: &Contract) -> bool {
    let names = contract
        .params
        .iter()
        .map(|param| (param.name.clone(), param.ty.clone()))
        .collect();
    matches!(
        axiom_parser::type_of_expr(expr, &names, Some(&contract.return_type)),
        Ok(Type::I32)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use axiom_parser::parse;

    #[test]
    fn synthesizes_mul8() {
        let contract = parse(include_str!("../../../examples/basics/mul8.ax")).unwrap();
        let candidate = synthesize(&contract).unwrap();

        assert_eq!(
            candidate,
            Expr::Shl(Box::new(Expr::Arg(0)), Box::new(Expr::Const(3)))
        );
    }

    #[test]
    fn completes_mul8_pipeline() {
        let contract = parse(include_str!("../../../examples/basics/mul8.ax")).unwrap();
        let (candidate, module) = synthesize_wasm(&contract).unwrap();

        assert_eq!(
            candidate,
            Expr::Shl(Box::new(Expr::Arg(0)), Box::new(Expr::Const(3)))
        );
        assert_eq!(&module[..8], b"\0asm\x01\0\0\0");
    }

    #[test]
    fn synthesizes_identity_from_base_candidates() {
        let contract = parse("contract identity(x: u32) -> u32 { ensures result == x; }").unwrap();

        assert_eq!(synthesize(&contract).unwrap(), Expr::Arg(0));
    }

    #[test]
    fn synthesizes_max_from_postconditions_without_an_implementation_expression() {
        let contract = parse(
            "contract maximum(first: u32, second: u32) -> u32 { \
             ensures result >= first; \
             ensures result >= second; \
             ensures result == first || result == second; }",
        )
        .unwrap();

        assert_eq!(
            synthesize(&contract).unwrap(),
            Expr::Select {
                condition: Box::new(Expr::Gt(Box::new(Expr::Arg(0)), Box::new(Expr::Arg(1)),)),
                then_branch: Box::new(Expr::Arg(0)),
                else_branch: Box::new(Expr::Arg(1)),
            }
        );
    }

    #[test]
    fn synthesizes_clamp_from_postconditions_without_an_implementation_expression() {
        let contract = parse(
            "contract clamp(value: u32, lower: u32, upper: u32) -> u32 { \
             requires lower <= upper; \
             ensures value < lower || value > upper || result == value; \
             ensures value >= lower || result == lower; \
             ensures value <= upper || result == upper; }",
        )
        .unwrap();

        assert_eq!(
            synthesize(&contract).unwrap(),
            Expr::Select {
                condition: Box::new(Expr::Lt(Box::new(Expr::Arg(0)), Box::new(Expr::Arg(1)),)),
                then_branch: Box::new(Expr::Arg(1)),
                else_branch: Box::new(Expr::Select {
                    condition: Box::new(Expr::Gt(Box::new(Expr::Arg(0)), Box::new(Expr::Arg(2)),)),
                    then_branch: Box::new(Expr::Arg(2)),
                    else_branch: Box::new(Expr::Arg(0)),
                }),
            }
        );
    }

    #[test]
    fn synthesizes_a_constant_branch_from_postconditions() {
        let contract = parse(
            "contract at_least_ten(value: u32) -> u32 { \
             ensures value >= 10 || result == 10; \
             ensures value < 10 || result == value; }",
        )
        .unwrap();

        let candidate = synthesize(&contract).unwrap();

        assert_eq!(candidate.eval(&[5]), 10);
        assert_eq!(candidate.eval(&[10]), 10);
        assert_eq!(candidate.eval(&[15]), 15);
    }

    #[test]
    fn reports_when_postconditions_are_contradictory() {
        let contract = parse(include_str!(
            "../../../examples/errors/unsat_postcondition.ax"
        ))
        .unwrap();

        let error = synthesize(&contract).unwrap_err();
        assert!(error.contains("may be contradictory"));
    }

    #[test]
    fn rejects_unsatisfiable_preconditions_before_search() {
        let contract = parse(include_str!(
            "../../../examples/errors/unsat_precondition.ax"
        ))
        .unwrap();

        let error = synthesize(&contract).unwrap_err();
        assert!(error.starts_with("UNSATISFIABLE_PRECONDITION"));
    }

    #[test]
    fn exposes_timeout_and_resource_limit_as_search_states() {
        let contract = parse(include_str!("../../../examples/basics/mul8.ax")).unwrap();

        assert!(matches!(
            synthesize_result_with_options(
                &contract,
                SynthesisOptions {
                    timeout: Some(Duration::ZERO),
                    ..SynthesisOptions::default()
                }
            )
            .unwrap(),
            SynthesisResult::Timeout { .. }
        ));
        assert!(matches!(
            synthesize_result_with_options(
                &contract,
                SynthesisOptions {
                    max_candidates: 0,
                    ..SynthesisOptions::default()
                }
            )
            .unwrap(),
            SynthesisResult::ResourceLimit { .. }
        ));
        assert!(matches!(
            synthesize_result_with_options(
                &contract,
                SynthesisOptions {
                    max_memory_bytes: 0,
                    ..SynthesisOptions::default()
                }
            )
            .unwrap(),
            SynthesisResult::ResourceLimit { .. }
        ));
        assert!(matches!(
            synthesize_result_with_options(&contract, SynthesisOptions::default()).unwrap(),
            SynthesisResult::Found { .. }
        ));
    }

    #[test]
    fn bounds_cartesian_sample_generation_before_allocating_the_product() {
        let contract = parse(
            "contract many(a: u32, b: u32, c: u32, d: u32, e: u32, f: u32) -> u32 { ensures result == a; }",
        )
        .unwrap();

        assert!(matches!(
            synthesize_result_with_options(
                &contract,
                SynthesisOptions {
                    max_candidates: 100,
                    max_memory_bytes: 1024 * 1024,
                    ..SynthesisOptions::default()
                }
            )
            .unwrap(),
            SynthesisResult::ResourceLimit {
                candidates_checked: 0,
                limit: 100,
            }
        ));
    }

    #[test]
    fn reuses_a_counterexample_in_the_next_candidate_filter() {
        let contract =
            parse("contract identity(value: u32) -> u32 { ensures result == value; }").unwrap();
        let mut cegis = CegisState::new(&contract, 100, Instant::now(), None).unwrap();
        let candidate = Expr::Select {
            condition: Box::new(Expr::Eq(Box::new(Expr::Arg(0)), Box::new(Expr::Const(123)))),
            then_branch: Box::new(Expr::Const(0)),
            else_branch: Box::new(Expr::Arg(0)),
        };

        assert!(cegis.matches(&contract, &candidate));
        assert!(cegis.observe(vec![123]));
        assert!(!cegis.matches(&contract, &candidate));
    }

    #[test]
    fn search_result_and_candidate_order_are_deterministic() {
        let contract = parse(include_str!("../../../examples/basics/mul8.ax")).unwrap();
        let first_order = depth_one_candidates(contract.params.len(), 1000, Instant::now(), None);
        let second_order = depth_one_candidates(contract.params.len(), 1000, Instant::now(), None);
        assert_eq!(first_order, second_order);

        let first = synthesize_result_with_options(&contract, SynthesisOptions::default()).unwrap();
        let second =
            synthesize_result_with_options(&contract, SynthesisOptions::default()).unwrap();
        assert_eq!(first, second);
    }

    #[test]
    fn candidate_batches_enforce_count_and_elapsed_time_budgets() {
        let limit = 31;
        let started = Instant::now();
        assert!(base_candidates(1000, limit, started, None).len() <= limit);
        assert!(conditional_candidates(1000, limit, started, None).len() <= limit);
        assert!(depth_two_candidates(1000, limit, started, None).len() <= limit);

        let expired = Instant::now() - Duration::from_secs(1);
        assert!(depth_two_candidates(1000, limit, expired, Some(Duration::ZERO)).is_empty());
    }

    #[test]
    fn synthesizes_a_value_outside_the_depth_one_constant_range() {
        let contract =
            parse("contract mul17(x: u32) -> u32 { ensures result == x *% 17; }").unwrap();

        assert!(synthesize(&contract).is_ok());
    }

    #[test]
    fn rejects_inputs_that_violate_requires() {
        let contract = parse(
            "contract guarded(x: u32) -> u32 { requires x != 0 && x != 1; ensures result == x; }",
        )
        .unwrap();

        assert!(check_requires(&contract, &[2]).is_ok());
        assert!(check_requires(&contract, &[1]).is_err());
    }

    #[test]
    fn lowered_requires_matches_the_reference_runtime_check() {
        let contract = parse(
            "contract bounded(x: u32) -> u32 { requires x > 0; requires x < 10; ensures result == x; }",
        )
        .unwrap();
        let predicate = lower_requires(&contract).unwrap().unwrap();
        for value in [0, 1, 9, 10, u32::MAX] {
            assert_eq!(
                predicate.eval(&[value]) != 0,
                check_requires(&contract, &[value]).is_ok(),
                "requires lowering differs at {value}"
            );
        }
        assert_eq!(
            lower_requires(
                &parse("contract identity(x: u32) -> u32 { ensures result == x; }").unwrap()
            )
            .unwrap(),
            None
        );
    }

    #[test]
    fn reports_a_zero_timeout_before_search() {
        let contract = parse(include_str!("../../../examples/basics/mul8.ax")).unwrap();
        let error = synthesize_with_options(
            &contract,
            SynthesisOptions {
                timeout: Some(Duration::ZERO),
                ..SynthesisOptions::default()
            },
        )
        .unwrap_err();

        assert!(error.contains("synthesis timed out"));
    }

    #[test]
    fn synthesizes_and_runs_conditional_contract() {
        let contract = parse(include_str!("../../../examples/friendly/easy_min.ax")).unwrap();
        let (candidate, module) = synthesize_wasm(&contract).unwrap();

        assert_eq!(candidate.eval(&[2, 5]), 2);
        assert_eq!(candidate.eval(&[8, 5]), 5);
        assert_eq!(axiom_wasm::run_with_args(&module, &[2, 5]).unwrap(), 2);
        assert_eq!(axiom_wasm::run_with_args(&module, &[8, 5]).unwrap(), 5);
    }

    #[test]
    fn synthesizes_signed_conditional_contract() {
        let contract = parse(include_str!("../../../examples/basics/signed_min.ax")).unwrap();
        let (candidate, module) = synthesize_wasm(&contract).unwrap();

        assert_eq!(candidate.eval(&[u32::MAX, 1]), u32::MAX);
        assert_eq!(candidate.eval(&[1, u32::MAX]), u32::MAX);
        assert_eq!(
            axiom_wasm::run_with_args(&module, &[u32::MAX, 1]).unwrap(),
            u32::MAX
        );
        assert_eq!(
            axiom_wasm::run_with_args(&module, &[1, u32::MAX]).unwrap(),
            u32::MAX
        );
    }

    #[test]
    fn lowers_let_contract_to_a_verified_candidate() {
        let contract = parse(include_str!("../../../examples/friendly/easy_let.ax")).unwrap();
        let candidate = synthesize(&contract).unwrap();

        assert_eq!(candidate.eval(&[21]), 42);
    }

    #[test]
    fn synthesizes_and_runs_bool_contract() {
        let contract =
            parse("program is_ready(value: number) -> bool { gives answer equals value > 0; }")
                .unwrap();
        let (candidate, module) = synthesize_wasm(&contract).unwrap();

        assert_eq!(candidate.eval(&[0]), 0);
        assert_eq!(candidate.eval(&[4]), 1);
        assert_eq!(axiom_wasm::run_with_args(&module, &[0]).unwrap(), 0);
        assert_eq!(axiom_wasm::run_with_args(&module, &[4]).unwrap(), 1);
    }

    #[test]
    fn synthesizes_bool_parameter_contract() {
        let contract =
            parse("program toggle(done: bool) -> bool { gives answer equals (done != true); }")
                .unwrap();
        let (candidate, module) = synthesize_wasm(&contract).unwrap();

        assert_eq!(candidate.eval(&[0]), 1);
        assert_eq!(candidate.eval(&[1]), 0);
        assert_eq!(axiom_wasm::run_with_args(&module, &[0]).unwrap(), 1);
        assert_eq!(axiom_wasm::run_with_args(&module, &[1]).unwrap(), 0);
    }

    #[test]
    fn typed_kernel_rejects_candidates_with_wrong_result_type() {
        let contract =
            parse("contract ready(value: bool) -> bool { ensures result == value; }").unwrap();
        let candidates = vec![
            Expr::Arg(0),
            Expr::Const(1),
            Expr::Add(Box::new(Expr::Arg(0)), Box::new(Expr::Const(1))),
        ];

        assert_eq!(candidate_type(&Expr::Arg(0), &contract), Some(Type::Bool));
        assert_eq!(typed_candidates(candidates, &contract), vec![Expr::Arg(0)]);
    }

    #[test]
    fn canonicalization_removes_commutative_duplicates() {
        let left = Expr::Add(Box::new(Expr::Const(1)), Box::new(Expr::Arg(0)));
        let right = Expr::Add(Box::new(Expr::Arg(0)), Box::new(Expr::Const(1)));

        assert_eq!(canonicalize(left), canonicalize(right));
    }

    #[test]
    fn reports_ambiguity_without_changing_the_found_state() {
        let contract =
            parse("contract identity(value: u32) -> u32 { ensures result == value; }").unwrap();

        let result =
            synthesize_result_with_options(&contract, SynthesisOptions::default()).unwrap();
        assert!(matches!(
            result,
            SynthesisResult::Found {
                diagnostic: Some(axiom_verify::Diagnostic::AmbiguousCandidates),
                ..
            }
        ));
    }

    #[test]
    fn synthesizes_property_and_constant_branches() {
        let contract = parse(include_str!(
            "../../../examples/properties/property_at_least_ten.ax"
        ))
        .unwrap();
        let (candidate, module) = synthesize_wasm(&contract).unwrap();

        for (input, expected) in [(3, 10), (10, 10), (11, 11)] {
            assert_eq!(candidate.eval(&[input]), expected);
            assert_eq!(
                axiom_wasm::run_with_args(&module, &[input]).unwrap(),
                expected
            );
        }
    }

    #[test]
    fn synthesizes_bit_mask_and_wrapping_subtraction_properties() {
        let mask = parse(include_str!("../../../examples/basics/bitwise.ax")).unwrap();
        let (mask_candidate, mask_module) = synthesize_wasm(&mask).unwrap();
        assert_eq!(mask_candidate.eval(&[0b1100, 0b1010]), 0b1000);
        assert_eq!(
            axiom_wasm::run_with_args(&mask_module, &[u32::MAX, 0]).unwrap(),
            0
        );

        let subtract = parse(include_str!("../../../examples/basics/subtract.ax")).unwrap();
        let (subtract_candidate, subtract_module) = synthesize_wasm(&subtract).unwrap();
        assert_eq!(subtract_candidate.eval(&[0]), u32::MAX);
        assert_eq!(
            axiom_wasm::run_with_args(&subtract_module, &[42]).unwrap(),
            41
        );
    }

    #[test]
    fn validates_property_ranges_at_the_input_boundary() {
        let contract = parse(include_str!(
            "../../../examples/properties/property_clamp.ax"
        ))
        .unwrap();

        assert!(check_requires(&contract, &[15, 10, 20]).is_ok());
        assert!(check_requires(&contract, &[15, 20, 10]).is_err());
    }

    #[test]
    fn keeps_max_min_and_clamp_regressions_in_sync_across_backends() {
        let cases = [
            (
                include_str!("../../../examples/properties/property_max.ax"),
                vec![vec![0, 0], vec![0, 1], vec![u32::MAX, 1]],
            ),
            (
                include_str!("../../../examples/friendly/easy_min.ax"),
                vec![vec![0, 0], vec![0, 1], vec![u32::MAX, 1]],
            ),
            (
                include_str!("../../../examples/properties/property_clamp.ax"),
                vec![vec![0, 10, 20], vec![15, 10, 20], vec![30, 10, 20]],
            ),
        ];

        for (source, inputs) in cases {
            let contract = parse(source).unwrap();
            let (candidate, module) = synthesize_wasm(&contract).unwrap();
            assert_eq!(
                axiom_verify::verify(&contract, &candidate).unwrap(),
                ProofResult::Verified
            );

            for args in inputs {
                let reference = axiom_verify::reference::eval_ami(&candidate, &args).unwrap();
                assert_eq!(candidate.eval(&args), reference);
                assert_eq!(
                    axiom_wasm::run_with_args(&module, &args).unwrap(),
                    reference
                );
                assert!(
                    axiom_verify::reference::contract_holds(&contract, &candidate, &args).unwrap()
                );
            }
        }
    }

    #[test]
    fn keeps_scalar_semantics_in_sync_between_reference_z3_and_wasm() {
        let cases = vec![
            (
                "contract add(x: u32, y: u32) -> u32 { ensures result == x +% y; }",
                Expr::Add(Box::new(Expr::Arg(0)), Box::new(Expr::Arg(1))),
                vec![vec![0, 0], vec![u32::MAX, 1], vec![42, 7]],
            ),
            (
                "contract sub(x: u32, y: u32) -> u32 { ensures result == x -% y; }",
                Expr::Sub(Box::new(Expr::Arg(0)), Box::new(Expr::Arg(1))),
                vec![vec![0, 1], vec![u32::MAX, 1], vec![42, 7]],
            ),
            (
                "contract mul(x: u32, y: u32) -> u32 { ensures result == x *% y; }",
                Expr::Mul(Box::new(Expr::Arg(0)), Box::new(Expr::Arg(1))),
                vec![vec![0, 7], vec![u32::MAX, 8], vec![42, 7]],
            ),
            (
                "contract shift(x: u32, amount: u32) -> u32 { ensures result == x << amount; }",
                Expr::Shl(Box::new(Expr::Arg(0)), Box::new(Expr::Arg(1))),
                vec![vec![1, 0], vec![1, 31], vec![1, 32], vec![u32::MAX, 255]],
            ),
            (
                "contract bit_and(x: u32, y: u32) -> u32 { ensures result == x & y; }",
                Expr::BitAnd(Box::new(Expr::Arg(0)), Box::new(Expr::Arg(1))),
                vec![vec![0b1100, 0b1010], vec![u32::MAX, 0]],
            ),
            (
                "contract bit_or(x: u32, y: u32) -> u32 { ensures result == x | y; }",
                Expr::BitOr(Box::new(Expr::Arg(0)), Box::new(Expr::Arg(1))),
                vec![vec![0b1100, 0b1010], vec![u32::MAX, 0]],
            ),
            (
                "contract bit_xor(x: u32, y: u32) -> u32 { ensures result == x ^ y; }",
                Expr::BitXor(Box::new(Expr::Arg(0)), Box::new(Expr::Arg(1))),
                vec![vec![0b1100, 0b1010], vec![u32::MAX, 0]],
            ),
            (
                "contract unsigned_lt(x: u32, y: u32) -> bool { ensures result == (x < y); }",
                Expr::Lt(Box::new(Expr::Arg(0)), Box::new(Expr::Arg(1))),
                vec![vec![0, 1], vec![u32::MAX, 1], vec![7, 7]],
            ),
            (
                "contract signed_lt(x: i32, y: i32) -> bool { ensures result == (x < y); }",
                Expr::SLt(Box::new(Expr::Arg(0)), Box::new(Expr::Arg(1))),
                vec![vec![u32::MAX, 1], vec![1, u32::MAX], vec![7, 7]],
            ),
            (
                "contract equal(x: u32, y: u32) -> bool { ensures result == (x == y); }",
                Expr::Eq(Box::new(Expr::Arg(0)), Box::new(Expr::Arg(1))),
                vec![vec![0, 0], vec![0, 1], vec![u32::MAX, u32::MAX]],
            ),
            (
                "contract not_equal(x: u32, y: u32) -> bool { ensures result == (x != y); }",
                Expr::Ne(Box::new(Expr::Arg(0)), Box::new(Expr::Arg(1))),
                vec![vec![0, 0], vec![0, 1], vec![u32::MAX, u32::MAX]],
            ),
            (
                "contract both(left: bool, right: bool) -> bool { ensures result == (left && right); }",
                Expr::And(Box::new(Expr::Arg(0)), Box::new(Expr::Arg(1))),
                vec![vec![0, 0], vec![0, 1], vec![1, 1]],
            ),
            (
                "contract choose(left: u32, right: u32) -> u32 { ensures result == if left < right { left } else { right }; }",
                Expr::Select {
                    condition: Box::new(Expr::Lt(Box::new(Expr::Arg(0)), Box::new(Expr::Arg(1)))),
                    then_branch: Box::new(Expr::Arg(0)),
                    else_branch: Box::new(Expr::Arg(1)),
                },
                vec![vec![0, 1], vec![7, 7], vec![u32::MAX, 1]],
            ),
        ];

        for (source, candidate, inputs) in cases {
            let contract = parse(source).unwrap();
            assert_eq!(
                axiom_verify::verify(&contract, &candidate).unwrap(),
                ProofResult::Verified,
                "Z3 rejected {source}"
            );
            let module = axiom_wasm::emit(&candidate, contract.params.len() as u32).unwrap();

            for args in inputs {
                let reference = axiom_verify::reference::eval_ami(&candidate, &args).unwrap();
                assert_eq!(
                    candidate.eval(&args),
                    reference,
                    "AMI mismatch for {source}"
                );
                assert_eq!(
                    axiom_wasm::run_with_args(&module, &args).unwrap(),
                    reference,
                    "WASM mismatch for {source}"
                );
                assert!(
                    axiom_verify::reference::contract_holds(&contract, &candidate, &args).unwrap(),
                    "reference contract mismatch for {source}"
                );
            }
        }
    }

    #[test]
    fn backend_conformance_matrix_covers_abi_and_boundaries() {
        let random_inputs = (0u32..32)
            .map(|seed| vec![seed.wrapping_mul(0x9e37_79b9), seed.rotate_left(13)])
            .collect::<Vec<_>>();
        let cases = vec![
            (
                "contract zero() -> u32 { ensures result == 0; }",
                Expr::Const(0),
                vec![vec![]],
            ),
            (
                "contract identity(value: u32) -> u32 { ensures result == value; }",
                Expr::Arg(0),
                vec![vec![0], vec![1], vec![u32::MAX], vec![0x8000_0000]],
            ),
            (
                "contract add(x: u32, y: u32) -> u32 { ensures result == x +% y; }",
                Expr::Add(Box::new(Expr::Arg(0)), Box::new(Expr::Arg(1))),
                random_inputs,
            ),
            (
                "contract signed_min(x: i32, y: i32) -> bool { ensures result == (x < y); }",
                Expr::SLt(Box::new(Expr::Arg(0)), Box::new(Expr::Arg(1))),
                vec![
                    vec![i32::MIN as u32, 0],
                    vec![i32::MAX as u32, i32::MIN as u32],
                    vec![-1i32 as u32, 1],
                ],
            ),
            (
                "contract toggle(done: bool) -> bool { ensures result == (done != true); }",
                Expr::Ne(Box::new(Expr::Arg(0)), Box::new(Expr::Const(1))),
                vec![vec![0], vec![1]],
            ),
        ];

        for (source, candidate, inputs) in cases {
            let contract = parse(source).unwrap();
            assert_eq!(
                axiom_verify::verify(&contract, &candidate).unwrap(),
                ProofResult::Verified,
                "Z3 rejected {source}"
            );
            let module = axiom_wasm::emit(&candidate, contract.params.len() as u32).unwrap();

            for args in inputs {
                let reference = axiom_verify::reference::eval_ami(&candidate, &args).unwrap();
                assert_eq!(
                    candidate.eval(&args),
                    reference,
                    "AMI mismatch for {source}"
                );
                assert_eq!(
                    axiom_wasm::run_with_args(&module, &args).unwrap(),
                    reference,
                    "WASM mismatch for {source}"
                );
                assert!(
                    axiom_verify::reference::contract_holds(&contract, &candidate, &args).unwrap(),
                    "reference contract mismatch for {source}"
                );
            }
        }
    }
}
