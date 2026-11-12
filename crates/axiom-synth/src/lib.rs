//! Minimal enumerative synthesizer for the first `mul8` vertical slice.

use std::{
    collections::{HashMap, HashSet},
    time::{Duration, Instant},
};

use axiom_ami::Expr;
use axiom_parser::{BinaryOp, Contract, DeclarationKind, SpecExpr, Type};
use axiom_verify::{ContractCheck, ProofResult, check_contract, verify};

struct PreconditionsSatisfiable;

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
    pub rejected_candidate_count: usize,
    pub synthesis_iterations: usize,
    pub search_batches: usize,
    pub counterexample_count: usize,
    pub initial_sample_count: usize,
    pub peak_sample_count: usize,
    pub candidate_generation_time_ns: u128,
    pub sample_generation_time_ns: u128,
    pub typed_filter_time_ns: u128,
    pub resource_estimation_time_ns: u128,
    pub candidate_sort_time_ns: u128,
    pub sample_evaluation_time_ns: u128,
    pub total_synthesis_time_ns: u128,
    pub other_search_residual_time_ns: u128,
    pub precondition_check_invocations: usize,
    pub precondition_translation_time_ns: u128,
    pub solver_invocations: usize,
    pub solver_time_ns: u128,
    pub peak_resource_estimate_bytes: usize,
}

impl SynthesisMetrics {
    fn merge_verification(&mut self, metrics: axiom_verify::VerificationMetrics) {
        self.precondition_check_invocations += metrics.precondition_check_invocations;
        self.precondition_translation_time_ns += metrics.precondition_translation_time_ns;
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
    let started = Instant::now();
    let result = synthesize_result_inner(contract, options, &mut Some(&mut metrics))?;
    metrics.total_synthesis_time_ns = started.elapsed().as_nanos();
    let measured_phases = metrics
        .sample_generation_time_ns
        .saturating_add(metrics.candidate_generation_time_ns)
        .saturating_add(metrics.typed_filter_time_ns)
        .saturating_add(metrics.resource_estimation_time_ns)
        .saturating_add(metrics.candidate_sort_time_ns)
        .saturating_add(metrics.sample_evaluation_time_ns)
        .saturating_add(metrics.solver_time_ns);
    metrics.other_search_residual_time_ns = metrics
        .total_synthesis_time_ns
        .checked_sub(measured_phases)
        .ok_or_else(|| "synthesis phase timings overlap or exceed total time".to_string())?;
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
    search_with_options(contract, options, metrics, PreconditionsSatisfiable)
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
    _preconditions_satisfiable: PreconditionsSatisfiable,
) -> Result<SynthesisResult, String> {
    let (argument_offsets, argument_count) = axiom_parser::verified_abi_layout(&contract.params)?;
    if argument_count > usize::from(u8::MAX) + 1 {
        return Err("verified ABI exceeds 256 flattened inputs".to_string());
    }
    let started = Instant::now();
    let mut checked = 0;
    let bytes_per_sample = std::mem::size_of::<Vec<u32>>()
        .saturating_add(
            contract
                .params
                .iter()
                .map(|param| match &param.ty {
                    Type::FixedArray(_, length) => *length,
                    _ => 1,
                })
                .sum::<usize>()
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
            if !timed_out(started, options.timeout)
                && let Some((candidate, candidates_checked)) = verify_resource_fallback_candidates(
                    contract,
                    options,
                    0,
                    resource_fallback_candidates(contract),
                    &mut *metrics,
                )?
            {
                return Ok(SynthesisResult::Found {
                    candidate,
                    diagnostic: None,
                    candidates_checked,
                });
            }
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
    let reusable_offsets = options
        .timeout
        .is_none()
        .then_some(argument_offsets.as_slice());
    let sample_evaluation_started = metrics.as_ref().is_some().then(Instant::now);
    let cache_result = if should_cache_requirements(contract, options.timeout) {
        cegis.cache_requirements(contract, reusable_offsets, started, options.timeout)
    } else {
        Ok(())
    };
    if let (Some(metrics), Some(started)) = (metrics.as_deref_mut(), sample_evaluation_started) {
        metrics.sample_evaluation_time_ns += started.elapsed().as_nanos();
    }
    if let Err(error) = cache_result {
        return Ok(sample_error_result(error, checked));
    }
    let mut first_verified = None;
    let generation_limit = options
        .max_candidates
        .min(options.max_memory_bytes / (std::mem::size_of::<Expr>().saturating_mul(32).max(1)));

    if (contract.return_type == Type::Bool
        || contract.ensures.iter().any(contains_conditional_construct))
        && let Some(candidate) = candidate_from_contract(contract)?
    {
        if let Some(metrics) = metrics.as_deref_mut() {
            metrics.generated_candidate_count += 1;
        }
        let sample_eval_started = metrics.as_ref().is_some().then(Instant::now);
        let matches_samples = cegis.matches(contract, &candidate, reusable_offsets);
        if let (Some(metrics), Some(started)) = (metrics.as_deref_mut(), sample_eval_started) {
            metrics.sample_evaluation_time_ns += started.elapsed().as_nanos();
        }
        if matches_samples {
            if let Some(metrics) = metrics.as_deref_mut() {
                metrics.synthesis_iterations += 1;
            }
            match verify_for_search(contract, &candidate, metrics)? {
                ProofResult::Verified => first_verified = Some(candidate),
                ProofResult::Counterexample(values) => {
                    if let Some(metrics) = metrics.as_deref_mut() {
                        metrics.counterexample_count += 1;
                        metrics.rejected_candidate_count += 1;
                        metrics.peak_sample_count =
                            metrics.peak_sample_count.max(cegis.samples.len() + 1);
                    }
                    let sample_eval_started = metrics.as_ref().is_some().then(Instant::now);
                    let observe =
                        cegis.observe(contract, values, reusable_offsets, started, options.timeout);
                    if let (Some(metrics), Some(started)) =
                        (metrics.as_deref_mut(), sample_eval_started)
                    {
                        metrics.sample_evaluation_time_ns += started.elapsed().as_nanos();
                    }
                    match observe {
                        Ok(true) => {}
                        Ok(false) => {
                            return Ok(SynthesisResult::ResourceLimit {
                                candidates_checked: checked,
                                limit: sample_limit,
                            });
                        }
                        Err(error) => return Ok(sample_error_result(error, checked)),
                    }
                }
                ProofResult::SolverUnknown => {
                    return Err("SOLVER_UNKNOWN: SMT solver returned unknown".to_string());
                }
            }
        } else if let Some(metrics) = metrics.as_deref_mut() {
            metrics.rejected_candidate_count += 1;
        }
    }

    let mut generation_truncated = generation_limit == 0;
    for batch_index in 0..6 {
        let generation_started = metrics.as_ref().is_some().then(Instant::now);
        let mut candidates = match batch_index {
            0 => base_candidates(argument_count, generation_limit, started, options.timeout),
            1 => depth_one_candidates(argument_count, generation_limit, started, options.timeout),
            2 => conditional_candidates(argument_count, generation_limit, started, options.timeout),
            3 => constant_conditional_candidates(
                argument_count,
                generation_limit,
                started,
                options.timeout,
            ),
            4 => {
                let conditional = conditional_candidates(
                    argument_count,
                    generation_limit,
                    started,
                    options.timeout,
                );
                nested_conditional_candidates(
                    argument_count,
                    &conditional,
                    generation_limit,
                    started,
                    options.timeout,
                )
            }
            5 => depth_two_candidates(argument_count, generation_limit, started, options.timeout),
            _ => unreachable!("fixed synthesis batch index"),
        };
        if contract
            .params
            .iter()
            .any(|param| matches!(param.ty, Type::FixedArray(_, _)))
        {
            let mut reads =
                array_read_candidates(contract, generation_limit, started, options.timeout)?;
            reads.extend(candidates);
            candidates = dedupe_and_cap(reads, generation_limit);
        }
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
        let generated_count = candidates.len();
        let typed_started = metrics.as_ref().is_some().then(Instant::now);
        let mut candidates = typed_candidates(candidates, contract);
        if let (Some(metrics), Some(started)) = (metrics.as_deref_mut(), typed_started) {
            metrics.typed_filter_time_ns += started.elapsed().as_nanos();
        }
        if let Some(metrics) = metrics.as_deref_mut() {
            metrics.rejected_candidate_count += generated_count - candidates.len();
        }
        let estimate_started = metrics.as_ref().is_some().then(Instant::now);
        let estimated_memory = candidates
            .iter()
            .map(estimated_candidate_bytes)
            .sum::<usize>();
        if let Some(metrics) = metrics.as_deref_mut() {
            if let Some(started) = estimate_started {
                metrics.resource_estimation_time_ns += started.elapsed().as_nanos();
            }
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
        let sort_started = metrics.as_ref().is_some().then(Instant::now);
        candidates.sort_by_key(axiom_wasm::instruction_cost);
        if let (Some(metrics), Some(started)) = (metrics.as_deref_mut(), sort_started) {
            metrics.candidate_sort_time_ns += started.elapsed().as_nanos();
        }
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
            let sample_eval_started = metrics.as_ref().is_some().then(Instant::now);
            let matches_samples = cegis.matches(contract, &candidate, reusable_offsets);
            if let (Some(metrics), Some(started)) = (metrics.as_deref_mut(), sample_eval_started) {
                metrics.sample_evaluation_time_ns += started.elapsed().as_nanos();
            }
            if !matches_samples {
                if let Some(metrics) = metrics.as_deref_mut() {
                    metrics.rejected_candidate_count += 1;
                }
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
                        metrics.rejected_candidate_count += 1;
                        metrics.peak_sample_count =
                            metrics.peak_sample_count.max(cegis.samples.len() + 1);
                    }
                    let sample_eval_started = metrics.as_ref().is_some().then(Instant::now);
                    let observe =
                        cegis.observe(contract, values, reusable_offsets, started, options.timeout);
                    if let (Some(metrics), Some(started)) =
                        (metrics.as_deref_mut(), sample_eval_started)
                    {
                        metrics.sample_evaluation_time_ns += started.elapsed().as_nanos();
                    }
                    match observe {
                        Ok(true) => {}
                        Ok(false) => {
                            return Ok(SynthesisResult::ResourceLimit {
                                candidates_checked: checked,
                                limit: sample_limit,
                            });
                        }
                        Err(error) => return Ok(sample_error_result(error, checked)),
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
        if let Some((candidate, fallback_checked)) = verify_resource_fallback_candidates(
            contract,
            options,
            checked,
            resource_fallback_candidates(contract),
            &mut *metrics,
        )? {
            return Ok(SynthesisResult::Found {
                candidate,
                diagnostic: None,
                candidates_checked: checked + fallback_checked,
            });
        }
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

fn resource_fallback_candidates(contract: &Contract) -> Vec<Expr> {
    let mut candidates = candidate_from_contract(contract)
        .ok()
        .flatten()
        .into_iter()
        .collect::<Vec<_>>();
    if let Some(candidate) = candidate_from_result_bounds(contract) {
        candidates.push(candidate);
    }
    let mut seen = HashSet::new();
    candidates
        .into_iter()
        .filter(|candidate| seen.insert(candidate.clone()))
        .collect()
}

fn verify_resource_fallback_candidates(
    contract: &Contract,
    options: SynthesisOptions,
    already_checked: usize,
    candidates: Vec<Expr>,
    metrics: &mut Option<&mut SynthesisMetrics>,
) -> Result<Option<(Expr, usize)>, String> {
    let mut checked = 0usize;
    for candidate in candidates {
        let candidate_bytes = estimated_candidate_bytes(&candidate);
        if let Some(metrics) = metrics.as_deref_mut() {
            metrics.generated_candidate_count += 1;
            metrics.peak_resource_estimate_bytes =
                metrics.peak_resource_estimate_bytes.max(candidate_bytes);
        }
        if already_checked.saturating_add(checked) >= options.max_candidates
            || candidate_bytes > options.max_memory_bytes
        {
            continue;
        }
        if let Some(metrics) = metrics.as_deref_mut() {
            metrics.synthesis_iterations += 1;
        }
        checked += 1;
        match verify_for_search(contract, &candidate, metrics)? {
            ProofResult::Verified => return Ok(Some((candidate, checked))),
            ProofResult::Counterexample(_) => {
                if let Some(metrics) = metrics.as_deref_mut() {
                    metrics.counterexample_count += 1;
                    metrics.rejected_candidate_count += 1;
                }
            }
            ProofResult::SolverUnknown => {
                return Err("SOLVER_UNKNOWN: SMT solver returned unknown".to_string());
            }
        }
    }
    Ok(None)
}

fn candidate_from_result_bounds(contract: &Contract) -> Option<Expr> {
    if contract.return_type != Type::U32
        || contract.params.len() < 3
        || contract.params.iter().any(|param| param.ty != Type::U32)
    {
        return None;
    }

    let has_bound = |operator, parameter: &str| {
        contract.ensures.iter().any(|clause| {
            let SpecExpr::Binary(actual, lhs, rhs) = clause else {
                return false;
            };
            *actual == operator
                && matches!(lhs.as_ref(), SpecExpr::Ident(name) if name == "result")
                && matches!(rhs.as_ref(), SpecExpr::Ident(name) if name == parameter)
        })
    };
    let is_maximum = contract
        .params
        .iter()
        .all(|param| has_bound(BinaryOp::Ge, &param.name));
    let is_minimum = contract
        .params
        .iter()
        .all(|param| has_bound(BinaryOp::Le, &param.name));
    if is_maximum == is_minimum {
        return None;
    }

    let comparison = if is_maximum {
        BinaryOp::Gt
    } else {
        BinaryOp::Lt
    };
    let mut candidate = Expr::Arg(0);
    for index in 1..contract.params.len() {
        let next = Expr::Arg(index as u8);
        let condition = match comparison {
            BinaryOp::Gt => Expr::Gt(Box::new(candidate.clone()), Box::new(next.clone())),
            BinaryOp::Lt => Expr::Lt(Box::new(candidate.clone()), Box::new(next.clone())),
            _ => unreachable!("only extremum comparisons are generated"),
        };
        candidate = Expr::Select {
            condition: Box::new(condition),
            then_branch: Box::new(candidate),
            else_branch: Box::new(next),
        };
    }
    Some(candidate)
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
    required_samples: Option<Vec<bool>>,
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
            required_samples: None,
            last_counterexample: None,
            max_samples,
        })
    }

    fn cache_requirements(
        &mut self,
        contract: &Contract,
        offsets: Option<&[usize]>,
        started: Instant,
        timeout: Option<Duration>,
    ) -> Result<(), SampleError> {
        let mut required_samples = Vec::with_capacity(self.samples.len());
        for args in &self.samples {
            required_samples.push(sample_satisfies_requirements(
                contract, args, 0, offsets, started, timeout,
            )?);
        }
        self.required_samples = Some(required_samples);
        Ok(())
    }

    fn matches(&self, contract: &Contract, candidate: &Expr, offsets: Option<&[usize]>) -> bool {
        matches_samples(
            contract,
            candidate,
            &self.samples,
            self.required_samples.as_deref(),
            offsets,
        )
    }

    fn observe(
        &mut self,
        contract: &Contract,
        counterexample: Vec<u32>,
        offsets: Option<&[usize]>,
        started: Instant,
        timeout: Option<Duration>,
    ) -> Result<bool, SampleError> {
        self.last_counterexample = Some(counterexample.clone());
        if !self.samples.contains(&counterexample) {
            if self.samples.len() >= self.max_samples {
                return Ok(false);
            }
            if let Some(required_samples) = &mut self.required_samples {
                let required = sample_satisfies_requirements(
                    contract,
                    &counterexample,
                    0,
                    offsets,
                    started,
                    timeout,
                )?;
                required_samples.push(required);
            }
            self.samples.push(counterexample);
        }
        Ok(true)
    }
}

#[derive(Debug)]
enum SampleError {
    Timeout,
    ResourceLimit(usize),
}

fn sample_error_result(error: SampleError, candidates_checked: usize) -> SynthesisResult {
    match error {
        SampleError::Timeout => SynthesisResult::Timeout { candidates_checked },
        SampleError::ResourceLimit(limit) => SynthesisResult::ResourceLimit {
            candidates_checked,
            limit,
        },
    }
}

fn should_cache_requirements(contract: &Contract, timeout: Option<Duration>) -> bool {
    timeout.is_none() && requirements_are_candidate_independent(contract)
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
    let (_, arity) = axiom_parser::verified_abi_layout(&contract.params)?;
    let module = axiom_wasm::emit(&candidate, arity as u32)?;
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
    let (_, expected) = axiom_parser::verified_abi_layout(&contract.params)?;
    if args.len() != expected {
        return Err(format!(
            "expected {expected} flattened inputs, got {}",
            args.len()
        ));
    }

    for expr in &contract.requires {
        match eval_spec(expr, contract, args, 0, &HashMap::new(), None)? {
            Value::Bool(true) => {}
            Value::Bool(false) => return Err("input violates a requires clause".to_string()),
            Value::U32(_) | Value::I32(_) | Value::Array(_) => {
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

fn array_read_candidates(
    contract: &Contract,
    limit: usize,
    started: Instant,
    timeout: Option<Duration>,
) -> Result<Vec<Expr>, String> {
    let (offsets, _) = axiom_parser::verified_abi_layout(&contract.params)?;
    let mut candidates = Vec::new();
    for (param_index, param) in contract.params.iter().enumerate() {
        let Type::FixedArray(element, length) = &param.ty else {
            continue;
        };
        if **element != Type::U32 {
            continue;
        }
        let elements = (0..*length)
            .map(|index| {
                u8::try_from(offsets[param_index] + index)
                    .map(Expr::Arg)
                    .map_err(|_| "verified ABI exceeds 256 flattened inputs".to_string())
            })
            .collect::<Result<Vec<_>, _>>()?;
        for index in 0..*length {
            if candidates.len() == limit || timed_out(started, timeout) {
                return Ok(candidates);
            }
            candidates.push(Expr::ArrayGet {
                elements: elements.clone(),
                index: Box::new(Expr::Const(index as u32)),
            });
        }
    }
    Ok(candidates)
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
        Expr::Arg(index) => candidate_argument_type(*index as usize, contract),
        Expr::Const(_) => Some(Type::U32),
        Expr::ArrayGet { elements, index }
            if !elements.is_empty()
                && elements
                    .iter()
                    .all(|element| candidate_type(element, contract) == Some(Type::U32))
                && matches!(candidate_type(index, contract), Some(Type::U32 | Type::I32)) =>
        {
            Some(Type::U32)
        }
        Expr::Guard { condition, value }
            if candidate_type(condition, contract) == Some(Type::Bool) =>
        {
            candidate_type(value, contract)
        }
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
        Expr::Select { .. } | Expr::ArrayGet { .. } | Expr::Guard { .. } => None,
    }
}

fn candidate_argument_type(index: usize, contract: &Contract) -> Option<Type> {
    let (offsets, arity) = axiom_parser::verified_abi_layout(&contract.params).ok()?;
    if index >= arity {
        return None;
    }
    for (param_index, param) in contract.params.iter().enumerate() {
        let width = match &param.ty {
            Type::FixedArray(_, length) => *length,
            _ => 1,
        };
        if (offsets[param_index]..offsets[param_index] + width).contains(&index) {
            return Some(match &param.ty {
                Type::FixedArray(_, _) => Type::U32,
                ty => ty.clone(),
            });
        }
    }
    None
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
        Expr::ArrayGet { elements, index } => {
            own + estimated_candidate_bytes(index)
                + elements
                    .iter()
                    .map(estimated_candidate_bytes)
                    .sum::<usize>()
        }
        Expr::Guard { condition, value } => {
            own + estimated_candidate_bytes(condition) + estimated_candidate_bytes(value)
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

fn and_expr(lhs: Expr, rhs: Expr) -> Expr {
    match (lhs, rhs) {
        (Expr::Const(0), _) | (_, Expr::Const(0)) => Expr::Const(0),
        (Expr::Const(1), rhs) => rhs,
        (lhs, Expr::Const(1)) => lhs,
        (lhs, rhs) => Expr::And(Box::new(lhs), Box::new(rhs)),
    }
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
        let width = match &param.ty {
            Type::FixedArray(_, length) => *length,
            _ => 1,
        };
        let values: &[u32] = if param.ty == Type::Bool {
            &[0, 1]
        } else {
            &VALUES
        };
        for _ in 0..width {
            if timed_out(started, timeout) {
                return Err(SampleError::Timeout);
            }
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
    }
    Ok(samples)
}

fn sample_satisfies_requirements(
    contract: &Contract,
    args: &[u32],
    result: u32,
    offsets: Option<&[usize]>,
    started: Instant,
    timeout: Option<Duration>,
) -> Result<bool, SampleError> {
    for expr in &contract.requires {
        if timed_out(started, timeout) {
            return Err(SampleError::Timeout);
        }
        let satisfied = matches!(
            eval_spec(expr, contract, args, result, &HashMap::new(), offsets),
            Ok(Value::Bool(true))
        );
        if timed_out(started, timeout) {
            return Err(SampleError::Timeout);
        }
        if !satisfied {
            return Ok(false);
        }
    }
    Ok(true)
}

fn matches_samples(
    contract: &Contract,
    candidate: &Expr,
    samples: &[Vec<u32>],
    required_samples: Option<&[bool]>,
    offsets: Option<&[usize]>,
) -> bool {
    if let Some(required_samples) = required_samples.filter(|cache| cache.len() == samples.len()) {
        samples
            .iter()
            .zip(required_samples)
            .all(|(args, required)| {
                let Ok(result) = candidate.try_eval(args) else {
                    return false;
                };
                !required
                    || contract.ensures.iter().all(|expr| {
                        matches!(
                            eval_spec(expr, contract, args, result, &HashMap::new(), offsets),
                            Ok(Value::Bool(true))
                        )
                    })
            })
    } else {
        samples.iter().all(|args| {
            let Ok(result) = candidate.try_eval(args) else {
                return false;
            };
            let requires = contract.requires.iter().all(|expr| {
                matches!(
                    eval_spec(expr, contract, args, result, &HashMap::new(), offsets),
                    Ok(Value::Bool(true))
                )
            });
            !requires
                || contract.ensures.iter().all(|expr| {
                    matches!(
                        eval_spec(expr, contract, args, result, &HashMap::new(), offsets),
                        Ok(Value::Bool(true))
                    )
                })
        })
    }
}

fn contains_result_identifier(expr: &SpecExpr) -> bool {
    match expr {
        SpecExpr::Ident(name) => name == "result",
        SpecExpr::List(values) | SpecExpr::Call(_, values) => {
            values.iter().any(contains_result_identifier)
        }
        SpecExpr::Record { fields, .. } => fields
            .iter()
            .any(|(_, value)| contains_result_identifier(value)),
        SpecExpr::Field(value, _) => contains_result_identifier(value),
        SpecExpr::Index(array, index) => {
            contains_result_identifier(array) || contains_result_identifier(index)
        }
        SpecExpr::ArrayUpdate(array, index, value) => {
            contains_result_identifier(array)
                || contains_result_identifier(index)
                || contains_result_identifier(value)
        }
        SpecExpr::For {
            initial,
            invariant,
            body,
            ..
        } => {
            contains_result_identifier(initial)
                || contains_result_identifier(invariant)
                || contains_result_identifier(body)
        }
        SpecExpr::Binary(_, lhs, rhs) => {
            contains_result_identifier(lhs) || contains_result_identifier(rhs)
        }
        SpecExpr::Let(_, value, body) => {
            contains_result_identifier(value) || contains_result_identifier(body)
        }
        SpecExpr::If(condition, then_branch, else_branch) => {
            contains_result_identifier(condition)
                || contains_result_identifier(then_branch)
                || contains_result_identifier(else_branch)
        }
        SpecExpr::Number(_) | SpecExpr::Bool(_) | SpecExpr::Text(_) => false,
    }
}

fn requirements_are_candidate_independent(contract: &Contract) -> bool {
    !contract.requires.is_empty()
        && contract
            .requires
            .iter()
            .all(|expr| !contains_result_identifier(expr))
}

#[derive(Clone)]
enum Value {
    U32(u32),
    I32(i32),
    Bool(bool),
    Array(Vec<u32>),
}

fn checked_sample_index(index: Value, length: usize) -> Result<usize, String> {
    let index = match index {
        Value::U32(index) => index as usize,
        Value::I32(index) if index >= 0 => index as usize,
        Value::I32(_) => return Err("fixed array index is negative".to_string()),
        _ => return Err("array index must be u32 or i32".to_string()),
    };
    if index >= length {
        return Err(format!("fixed array index {index} is out of bounds"));
    }
    Ok(index)
}

fn eval_spec(
    expr: &SpecExpr,
    contract: &Contract,
    args: &[u32],
    result: u32,
    locals: &HashMap<String, Value>,
    offsets: Option<&[usize]>,
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
            let offset = match offsets {
                Some(offsets) => offsets[index],
                None => axiom_parser::verified_abi_layout(&contract.params)?.0[index],
            };
            let value = args
                .get(offset)
                .copied()
                .ok_or_else(|| format!("missing argument: {name}"))?;
            match contract.params[index].ty {
                Type::Bool => Ok(Value::Bool(value != 0)),
                Type::I32 => Ok(Value::I32(value as i32)),
                Type::FixedArray(_, length) => {
                    Ok(Value::Array(args[offset..offset + length].to_vec()))
                }
                _ => Ok(Value::U32(value)),
            }
        }
        SpecExpr::Number(value) => Ok(Value::U32(*value)),
        SpecExpr::Bool(value) => Ok(Value::Bool(*value)),
        SpecExpr::Field(value, field) => {
            match eval_spec(value, contract, args, result, locals, offsets)? {
                Value::Array(elements) if field == "length" => {
                    Ok(Value::U32(elements.len() as u32))
                }
                _ => Err("field access is only supported for fixed array length".to_string()),
            }
        }
        SpecExpr::Index(array, index) => {
            let Value::Array(elements) = eval_spec(array, contract, args, result, locals, offsets)?
            else {
                return Err("indexed reads require a fixed verified array".to_string());
            };
            let index = eval_spec(index, contract, args, result, locals, offsets)?;
            let index = checked_sample_index(index, elements.len())?;
            Ok(Value::U32(elements[index]))
        }
        SpecExpr::ArrayUpdate(array, index, value) => {
            let Value::Array(mut elements) =
                eval_spec(array, contract, args, result, locals, offsets)?
            else {
                return Err("array_update requires a fixed verified array".to_string());
            };
            let index = eval_spec(index, contract, args, result, locals, offsets)?;
            let index = checked_sample_index(index, elements.len())?;
            let Value::U32(value) = eval_spec(value, contract, args, result, locals, offsets)?
            else {
                return Err("array update value must be u32".to_string());
            };
            elements[index] = value;
            Ok(Value::Array(elements))
        }
        SpecExpr::For { .. } => {
            Err("bounded loops are only allowed in fn implementation expressions".to_string())
        }
        SpecExpr::Text(_) | SpecExpr::List(_) | SpecExpr::Record { .. } => {
            Err("non-scalar expressions cannot be synthesized".to_string())
        }
        SpecExpr::Let(name, value, body) => {
            let value = eval_spec(value, contract, args, result, locals, offsets)?;
            let mut nested = locals.clone();
            nested.insert(name.clone(), value);
            eval_spec(body, contract, args, result, &nested, offsets)
        }
        SpecExpr::If(condition, then_branch, else_branch) => {
            let condition = match eval_spec(condition, contract, args, result, locals, offsets)? {
                Value::Bool(value) => value,
                Value::U32(_) | Value::I32(_) | Value::Array(_) => {
                    return Err("if condition must be boolean".to_string());
                }
            };
            if condition {
                eval_spec(then_branch, contract, args, result, locals, offsets)
            } else {
                eval_spec(else_branch, contract, args, result, locals, offsets)
            }
        }
        SpecExpr::Call(name, _) => Err(format!(
            "unresolved contract call: {name}; link the project before synthesis"
        )),
        SpecExpr::Binary(op, lhs, rhs) => {
            let lhs = eval_spec(lhs, contract, args, result, locals, offsets)?;
            let rhs = eval_spec(rhs, contract, args, result, locals, offsets)?;
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
                (BinaryOp::Eq, Value::Array(lhs), Value::Array(rhs)) => Ok(Value::Bool(lhs == rhs)),
                (BinaryOp::Ne, Value::Array(lhs), Value::Array(rhs)) => Ok(Value::Bool(lhs != rhs)),
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
        SpecExpr::Let(_, _, _) | SpecExpr::If(_, _, _) | SpecExpr::For { .. } => true,
        SpecExpr::Binary(_, lhs, rhs) => {
            contains_conditional_construct(lhs) || contains_conditional_construct(rhs)
        }
        SpecExpr::Ident(_) | SpecExpr::Number(_) | SpecExpr::Bool(_) | SpecExpr::Text(_) => false,
        SpecExpr::List(values) => values.iter().any(contains_conditional_construct),
        SpecExpr::Record { fields, .. } => fields
            .iter()
            .any(|(_, value)| contains_conditional_construct(value)),
        SpecExpr::Field(value, _) => contains_conditional_construct(value),
        SpecExpr::Index(array, index) => {
            contains_conditional_construct(array) || contains_conditional_construct(index)
        }
        SpecExpr::ArrayUpdate(array, index, value) => {
            contains_conditional_construct(array)
                || contains_conditional_construct(index)
                || contains_conditional_construct(value)
        }
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
        SpecExpr::Ident(name) => {
            let index = contract
                .params
                .iter()
                .position(|param| param.name == *name)
                .ok_or_else(|| format!("unknown identifier: {name}"))?;
            if matches!(contract.params[index].ty, Type::FixedArray(_, _)) {
                return Err(format!("fixed array {name} requires an index"));
            }
            let (offsets, _) = axiom_parser::verified_abi_layout(&contract.params)?;
            Ok(Expr::Arg(u8::try_from(offsets[index]).map_err(|_| {
                "verified ABI exceeds 256 flattened inputs".to_string()
            })?))
        }
        SpecExpr::Number(value) => Ok(Expr::Const(*value)),
        SpecExpr::Bool(value) => Ok(Expr::Const(u32::from(*value))),
        SpecExpr::Field(array, field) if field == "length" => {
            let (elements, defined) = lower_array_spec_expr(array, contract, locals)?;
            Ok(Expr::Guard {
                condition: Box::new(defined),
                value: Box::new(Expr::Const(elements.len() as u32)),
            })
        }
        SpecExpr::Index(array, index) => {
            let (elements, array_defined) = lower_array_spec_expr(array, contract, locals)?;
            let index = lower_spec_expr(index, contract, locals)?;
            let defined = and_expr(
                array_defined,
                Expr::Lt(
                    Box::new(index.clone()),
                    Box::new(Expr::Const(elements.len() as u32)),
                ),
            );
            Ok(Expr::Guard {
                condition: Box::new(defined),
                value: Box::new(Expr::ArrayGet {
                    elements,
                    index: Box::new(index),
                }),
            })
        }
        SpecExpr::Text(_)
        | SpecExpr::List(_)
        | SpecExpr::Record { .. }
        | SpecExpr::Field(_, _)
        | SpecExpr::ArrayUpdate(_, _, _)
        | SpecExpr::For { .. } => {
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

fn lower_array_spec_expr(
    expr: &SpecExpr,
    contract: &Contract,
    locals: &HashMap<String, Expr>,
) -> Result<(Vec<Expr>, Expr), String> {
    match expr {
        SpecExpr::Ident(name) => {
            let index = contract
                .params
                .iter()
                .position(|param| param.name == *name)
                .ok_or_else(|| format!("unknown identifier: {name}"))?;
            let Type::FixedArray(_, length) = &contract.params[index].ty else {
                return Err(format!("{name} is not a fixed verified array"));
            };
            let (offsets, _) = axiom_parser::verified_abi_layout(&contract.params)?;
            let elements = (offsets[index]..offsets[index] + *length)
                .map(|offset| {
                    u8::try_from(offset)
                        .map(Expr::Arg)
                        .map_err(|_| "verified ABI exceeds 256 flattened inputs".to_string())
                })
                .collect::<Result<Vec<_>, _>>()?;
            Ok((elements, Expr::Const(1)))
        }
        SpecExpr::ArrayUpdate(array, index, value) => {
            let (elements, array_defined) = lower_array_spec_expr(array, contract, locals)?;
            let index = lower_spec_expr(index, contract, locals)?;
            let value = lower_spec_expr(value, contract, locals)?;
            let in_bounds = Expr::Lt(
                Box::new(index.clone()),
                Box::new(Expr::Const(elements.len() as u32)),
            );
            let updated = elements
                .into_iter()
                .enumerate()
                .map(|(position, element)| Expr::Select {
                    condition: Box::new(Expr::Eq(
                        Box::new(index.clone()),
                        Box::new(Expr::Const(position as u32)),
                    )),
                    then_branch: Box::new(value.clone()),
                    else_branch: Box::new(element),
                })
                .collect();
            Ok((updated, and_expr(array_defined, in_bounds)))
        }
        _ => Err("expected a fixed verified array expression".to_string()),
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
        let contract = parse(include_str!("../../../tests/legacy/basics/mul8.ax")).unwrap();
        let candidate = synthesize(&contract).unwrap();

        assert_eq!(
            candidate,
            Expr::Shl(Box::new(Expr::Arg(0)), Box::new(Expr::Const(3)))
        );
    }

    #[test]
    fn completes_mul8_pipeline() {
        let contract = parse(include_str!("../../../tests/legacy/basics/mul8.ax")).unwrap();
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
    fn metrics_keep_candidate_precondition_proofs_after_preflight_reuse() {
        let contract = parse("contract identity(x: u32) -> u32 { ensures result == x; }").unwrap();

        let (result, metrics) =
            synthesize_result_with_metrics(&contract, SynthesisOptions::default()).unwrap();

        assert!(matches!(result, SynthesisResult::Found { .. }));
        assert!(metrics.synthesis_iterations > 0);
        assert_eq!(
            metrics.precondition_check_invocations,
            1 + metrics.synthesis_iterations
        );
    }

    #[test]
    fn synthesizes_static_fixed_array_reads_deterministically() {
        let contract =
            parse("contract first(values: [u32; 3]) -> u32 { ensures result == values[0]; }")
                .unwrap();
        let options = SynthesisOptions {
            max_candidates: 5_000,
            max_memory_bytes: 8 * 1024 * 1024,
            ..SynthesisOptions::default()
        };
        let first = synthesize_result_with_options(&contract, options).unwrap();
        let second = synthesize_result_with_options(&contract, options).unwrap();
        assert_eq!(first, second);
        let SynthesisResult::Found { candidate, .. } = first else {
            panic!("expected a fixed-array read candidate, got {first:?}");
        };
        assert_eq!(
            candidate.try_eval(&[17, 29, 43]).unwrap(),
            17,
            "candidate must use the first flattened array element"
        );
    }

    #[test]
    fn resource_limited_search_fallbacks_are_proved_and_run_in_wasm() {
        let cases = [
            (
                include_str!("../../../tests/fixtures/synthesis/maximum_3.ax"),
                vec![
                    (vec![0, u32::MAX, 2], u32::MAX),
                    (vec![4, 4, 1], 4),
                    (vec![7, 0, 9], 9),
                ],
            ),
            (
                include_str!("../../../tests/fixtures/synthesis/bitwise_combo.ax"),
                [0, 1, 15, 16, u32::MAX]
                    .into_iter()
                    .map(|value| (vec![value], (value & 15) ^ value.wrapping_add(1)))
                    .collect(),
            ),
            (
                include_str!("../../../tests/fixtures/synthesis/sum_8.ax"),
                vec![
                    (vec![u32::MAX, 1, 2, 3, 4, 5, 6, 7], 27),
                    (vec![1, 2, 3, 4, 5, 6, 7, 8], 36),
                ],
            ),
        ];

        for (source, cases) in cases {
            let contract = parse(source).unwrap();
            let (result, metrics) =
                synthesize_result_with_metrics(&contract, SynthesisOptions::default()).unwrap();
            let SynthesisResult::Found { candidate, .. } = result else {
                panic!("expected a verified resource fallback for {source}, got {result:?}");
            };
            assert_eq!(metrics.candidate_count, 1);
            assert_eq!(metrics.synthesis_iterations, 1);
            assert!(metrics.generated_candidate_count > 0);
            assert!(metrics.solver_invocations > 0);
            assert_eq!(
                verify(&contract, &candidate).unwrap(),
                ProofResult::Verified
            );

            let module = axiom_wasm::emit(&candidate, contract.params.len() as u32).unwrap();
            for (inputs, expected) in cases {
                assert_eq!(
                    axiom_wasm::run_with_args(&module, &inputs).unwrap(),
                    expected
                );
            }
        }
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
            "../../../tests/legacy/errors/unsat_postcondition.ax"
        ))
        .unwrap();

        let error = synthesize(&contract).unwrap_err();
        assert!(error.contains("may be contradictory"));
    }

    #[test]
    fn rejects_unsatisfiable_preconditions_before_search() {
        let contract = parse(include_str!(
            "../../../tests/legacy/errors/unsat_precondition.ax"
        ))
        .unwrap();

        let error = synthesize(&contract).unwrap_err();
        assert!(error.starts_with("UNSATISFIABLE_PRECONDITION"));
    }

    #[test]
    fn exposes_timeout_and_resource_limit_as_search_states() {
        let contract = parse(include_str!("../../../tests/legacy/basics/mul8.ax")).unwrap();

        let timed_out = synthesize_result_with_options(
            &contract,
            SynthesisOptions {
                timeout: Some(Duration::ZERO),
                ..SynthesisOptions::default()
            },
        )
        .unwrap();
        assert_eq!(timed_out.status_name(), "TIMEOUT");
        assert!(matches!(timed_out, SynthesisResult::Timeout { .. }));

        let finite_timeout = synthesize_result_with_options(
            &contract,
            SynthesisOptions {
                timeout: Some(Duration::from_nanos(1)),
                ..SynthesisOptions::default()
            },
        )
        .unwrap();
        assert_eq!(finite_timeout.status_name(), "TIMEOUT");
        assert!(matches!(finite_timeout, SynthesisResult::Timeout { .. }));

        let resource_limited = synthesize_result_with_options(
            &contract,
            SynthesisOptions {
                max_candidates: 0,
                ..SynthesisOptions::default()
            },
        )
        .unwrap();
        assert_eq!(resource_limited.status_name(), "RESOURCE_LIMIT");
        assert!(matches!(
            resource_limited,
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
        let found = synthesize_result_with_options(&contract, SynthesisOptions::default()).unwrap();
        assert_eq!(found.status_name(), "FOUND");
        assert!(matches!(found, SynthesisResult::Found { .. }));
    }

    #[test]
    fn resource_fallback_uses_proofs_without_cartesian_sample_products() {
        let contract = parse(
            "contract many(a: u32, b: u32, c: u32, d: u32, e: u32, f: u32) -> u32 { ensures result == a; }",
        )
        .unwrap();

        let (result, metrics) = synthesize_result_with_metrics(
            &contract,
            SynthesisOptions {
                max_candidates: 100,
                max_memory_bytes: 1024 * 1024,
                ..SynthesisOptions::default()
            },
        )
        .unwrap();
        assert!(matches!(
            result,
            SynthesisResult::Found {
                candidate: Expr::Arg(0),
                candidates_checked: 1,
                ..
            }
        ));
        assert_eq!(metrics.initial_sample_count, 0);
        assert_eq!(metrics.peak_sample_count, 0);
        assert_eq!(metrics.candidate_count, 1);
        assert!(metrics.solver_invocations > 0);

        let invalid = parse(
            "contract invalid(a: u32, b: u32, c: u32, d: u32, e: u32, f: u32) -> u32 { ensures result == a; ensures result == a +% 1; }",
        )
        .unwrap();
        let error = synthesize_result_with_metrics(
            &invalid,
            SynthesisOptions {
                max_candidates: 100,
                max_memory_bytes: 1024 * 1024,
                ..SynthesisOptions::default()
            },
        )
        .unwrap_err();
        assert!(error.contains("UNSATISFIABLE_POSTCONDITION"));
    }

    #[test]
    fn sample_requirement_cache_tracks_added_and_duplicate_counterexamples() {
        let contract =
            parse("contract positive(x: u32) -> u32 { requires x > 0; ensures result == x; }")
                .unwrap();
        let offsets = axiom_parser::verified_abi_layout(&contract.params)
            .unwrap()
            .0;
        let mut cegis = CegisState {
            samples: vec![vec![1]],
            required_samples: Some(vec![true]),
            last_counterexample: None,
            max_samples: 3,
        };

        assert!(
            cegis
                .observe(&contract, vec![0], Some(&offsets), Instant::now(), None)
                .unwrap()
        );
        assert_eq!(cegis.samples, vec![vec![1], vec![0]]);
        assert_eq!(cegis.required_samples, Some(vec![true, false]));
        assert!(
            cegis
                .observe(&contract, vec![0], Some(&offsets), Instant::now(), None)
                .unwrap()
        );
        assert_eq!(
            cegis.samples.len(),
            cegis.required_samples.as_ref().unwrap().len()
        );
        assert_eq!(cegis.samples.len(), 2);
    }

    #[test]
    fn result_dependent_requirements_keep_candidate_specific_evaluation() {
        let mut contract =
            parse("contract result_guard(x: u32) -> u32 { ensures result == x; }").unwrap();
        contract.requires.push(SpecExpr::Binary(
            BinaryOp::Eq,
            Box::new(SpecExpr::Ident("result".to_string())),
            Box::new(SpecExpr::Ident("x".to_string())),
        ));
        let samples = vec![vec![0]];
        let candidate = Expr::Const(1);
        let offsets = axiom_parser::verified_abi_layout(&contract.params)
            .unwrap()
            .0;

        assert!(!requirements_are_candidate_independent(&contract));
        assert!(matches_samples(
            &contract,
            &candidate,
            &samples,
            None,
            Some(&offsets)
        ));
        assert!(!matches_samples(
            &contract,
            &candidate,
            &samples,
            Some(&[true]),
            Some(&offsets)
        ));
    }

    #[test]
    fn mismatched_requirement_cache_falls_back_to_uncached_matching() {
        let contract =
            parse("contract positive(x: u32) -> u32 { requires x > 0; ensures result == 1; }")
                .unwrap();
        let samples = vec![vec![1], vec![0]];
        let candidate = Expr::Const(0);
        let offsets = axiom_parser::verified_abi_layout(&contract.params)
            .unwrap()
            .0;
        let uncached = matches_samples(&contract, &candidate, &samples, None, Some(&offsets));

        assert!(!uncached);
        assert_eq!(
            matches_samples(
                &contract,
                &candidate,
                &samples,
                Some(&[false]),
                Some(&offsets)
            ),
            uncached
        );
    }

    #[test]
    fn expired_observation_preserves_sample_limit_classification() {
        let contract =
            parse("contract positive(x: u32) -> u32 { requires x > 0; ensures result == x; }")
                .unwrap();
        let mut cegis = CegisState {
            samples: vec![vec![1]],
            required_samples: None,
            last_counterexample: None,
            max_samples: 1,
        };
        let timeout = Duration::from_millis(1);
        let expired = Instant::now() - timeout - Duration::from_millis(1);

        let observe = cegis.observe(&contract, vec![0], None, expired, Some(timeout));
        assert!(matches!(observe, Ok(false)));
        assert_eq!(cegis.samples, vec![vec![1]]);
        assert_eq!(cegis.required_samples, None);
    }

    #[test]
    fn no_timeout_cache_is_retained_and_finite_timeout_bypasses_it() {
        let contract =
            parse("contract positive(x: u32) -> u32 { requires x > 0; ensures result == x; }")
                .unwrap();
        let timeout = Duration::from_millis(1);

        assert!(should_cache_requirements(&contract, None));
        assert!(!should_cache_requirements(&contract, Some(timeout)));
    }

    #[test]
    fn reuses_a_counterexample_in_the_next_candidate_filter() {
        let contract =
            parse("contract identity(value: u32) -> u32 { ensures result == value; }").unwrap();
        let offsets = axiom_parser::verified_abi_layout(&contract.params)
            .unwrap()
            .0;
        let mut cegis = CegisState::new(&contract, 100, Instant::now(), None).unwrap();
        let candidate = Expr::Select {
            condition: Box::new(Expr::Eq(Box::new(Expr::Arg(0)), Box::new(Expr::Const(123)))),
            then_branch: Box::new(Expr::Const(0)),
            else_branch: Box::new(Expr::Arg(0)),
        };

        assert!(cegis.matches(&contract, &candidate, Some(&offsets)));
        assert!(
            cegis
                .observe(&contract, vec![123], Some(&offsets), Instant::now(), None)
                .unwrap()
        );
        assert!(!cegis.matches(&contract, &candidate, Some(&offsets)));
    }

    #[test]
    fn search_result_and_candidate_order_are_deterministic() {
        let contract = parse(include_str!("../../../tests/legacy/basics/mul8.ax")).unwrap();
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
        let contract = parse(include_str!("../../../tests/legacy/basics/mul8.ax")).unwrap();
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
        let contract = parse(include_str!("../../../tests/legacy/friendly/easy_min.ax")).unwrap();
        let (candidate, module) = synthesize_wasm(&contract).unwrap();

        assert_eq!(candidate.eval(&[2, 5]), 2);
        assert_eq!(candidate.eval(&[8, 5]), 5);
        assert_eq!(axiom_wasm::run_with_args(&module, &[2, 5]).unwrap(), 2);
        assert_eq!(axiom_wasm::run_with_args(&module, &[8, 5]).unwrap(), 5);
    }

    #[test]
    fn synthesizes_signed_conditional_contract() {
        let contract = parse(include_str!("../../../tests/legacy/basics/signed_min.ax")).unwrap();
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
        let contract = parse(include_str!("../../../tests/legacy/friendly/easy_let.ax")).unwrap();
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
            "../../../tests/legacy/properties/property_at_least_ten.ax"
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
        let mask = parse(include_str!("../../../tests/legacy/basics/bitwise.ax")).unwrap();
        let (mask_candidate, mask_module) = synthesize_wasm(&mask).unwrap();
        assert_eq!(mask_candidate.eval(&[0b1100, 0b1010]), 0b1000);
        assert_eq!(
            axiom_wasm::run_with_args(&mask_module, &[u32::MAX, 0]).unwrap(),
            0
        );

        let subtract = parse(include_str!("../../../tests/legacy/basics/subtract.ax")).unwrap();
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
            "../../../tests/legacy/properties/property_clamp.ax"
        ))
        .unwrap();

        assert!(check_requires(&contract, &[15, 10, 20]).is_ok());
        assert!(check_requires(&contract, &[15, 20, 10]).is_err());
    }

    #[test]
    fn keeps_max_min_and_clamp_regressions_in_sync_across_backends() {
        let cases = [
            (
                include_str!("../../../tests/legacy/properties/property_max.ax"),
                vec![vec![0, 0], vec![0, 1], vec![u32::MAX, 1]],
            ),
            (
                include_str!("../../../tests/legacy/friendly/easy_min.ax"),
                vec![vec![0, 0], vec![0, 1], vec![u32::MAX, 1]],
            ),
            (
                include_str!("../../../tests/legacy/properties/property_clamp.ax"),
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
