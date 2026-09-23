//! Application values and the one-way boundary into verified scalar WASM.

use std::{collections::BTreeMap, fmt};

use axiom_parser::{Contract, DeclarationKind, Type};
use axiom_synth::{SynthesisOptions, SynthesisResult};
use axiom_verify::{ContractCheck, ProofResult, check_contract, verify};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum ApplicationValue {
    Integer(i128),
    Boolean(bool),
    Text(String),
    List(Vec<ApplicationValue>),
    Record(BTreeMap<String, ApplicationValue>),
}

impl ApplicationValue {
    /// Parses a CLI token as an application value without coercing text to a scalar.
    pub fn from_cli_token(value: &str) -> Self {
        match value {
            "true" => Self::Boolean(true),
            "false" => Self::Boolean(false),
            _ => value
                .parse::<i128>()
                .map(Self::Integer)
                .unwrap_or_else(|_| Self::Text(value.to_string())),
        }
    }

    fn kind(&self) -> &'static str {
        match self {
            Self::Integer(_) => "integer",
            Self::Boolean(_) => "boolean",
            Self::Text(_) => "text",
            Self::List(_) => "list",
            Self::Record(_) => "record",
        }
    }

    /// Reads one named record field without coercing its application value.
    pub fn record_field(&self, name: &str) -> Result<&Self, ApplicationDataError> {
        let Self::Record(fields) = self else {
            return Err(ApplicationDataError::NotRecord(self.kind()));
        };
        fields
            .get(name)
            .ok_or_else(|| ApplicationDataError::MissingField(name.to_string()))
    }

    /// Reads one list element without treating the list itself as verified data.
    pub fn list_item(&self, index: usize) -> Result<&Self, ApplicationDataError> {
        let Self::List(values) = self else {
            return Err(ApplicationDataError::NotList(self.kind()));
        };
        values
            .get(index)
            .ok_or(ApplicationDataError::IndexOutOfBounds {
                index,
                len: values.len(),
            })
    }

    /// Explicitly extracts a supported scalar for subsequent contract validation.
    pub fn verified_scalar(&self) -> Result<Self, ApplicationDataError> {
        match self {
            Self::Integer(_) | Self::Boolean(_) => Ok(self.clone()),
            value => Err(ApplicationDataError::NotScalar(value.kind())),
        }
    }

    pub fn to_json(&self) -> Result<String, ApplicationDataError> {
        serde_json::to_string(self)
            .map_err(|error| ApplicationDataError::Serialization(error.to_string()))
    }

    pub fn from_json(json: &str) -> Result<Self, ApplicationDataError> {
        serde_json::from_str(json)
            .map_err(|error| ApplicationDataError::Serialization(error.to_string()))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApplicationDataError {
    NotRecord(&'static str),
    MissingField(String),
    NotList(&'static str),
    IndexOutOfBounds { index: usize, len: usize },
    NotScalar(&'static str),
    Serialization(String),
}

impl fmt::Display for ApplicationDataError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotRecord(received) => write!(f, "expected record, received {received}"),
            Self::MissingField(name) => write!(f, "record field not found: {name}"),
            Self::NotList(received) => write!(f, "expected list, received {received}"),
            Self::IndexOutOfBounds { index, len } => {
                write!(f, "list index {index} is outside length {len}")
            }
            Self::NotScalar(received) => {
                write!(f, "application {received} value is not a verified scalar")
            }
            Self::Serialization(message) => write!(f, "application value JSON error: {message}"),
        }
    }
}

/// A pure computation whose contract passed contract checking and whose AMI
/// candidate was verified before WASM emission.
#[derive(Debug, Clone)]
pub struct VerifiedComputation {
    contract: Contract,
    wasm: Vec<u8>,
}

impl VerifiedComputation {
    pub fn synthesize(contract: &Contract) -> Result<Self, CompileError> {
        Self::synthesize_with_options(contract, SynthesisOptions::default())
    }

    pub fn synthesize_with_options(
        contract: &Contract,
        options: SynthesisOptions,
    ) -> Result<Self, CompileError> {
        if contract.kind == DeclarationKind::Extern {
            return Err(CompileError::ExternalBoundary);
        }
        match check_contract(contract).map_err(CompileError::Engine)? {
            ContractCheck::Valid => {}
            result => return Err(CompileError::Contract(result)),
        }

        let synthesis = axiom_synth::synthesize_result_with_options(contract, options)
            .map_err(CompileError::Engine)?;
        let candidate = match synthesis {
            SynthesisResult::Found { candidate, .. } => candidate,
            result => return Err(CompileError::Synthesis(result)),
        };

        match verify(contract, &candidate).map_err(CompileError::Engine)? {
            ProofResult::Verified => {}
            result => return Err(CompileError::Proof(result)),
        }

        let wasm = axiom_wasm::emit(&candidate, contract.params.len() as u32)
            .map_err(CompileError::Emit)?;
        Ok(Self {
            contract: contract.clone(),
            wasm,
        })
    }

    pub fn name(&self) -> &str {
        &self.contract.name
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompileError {
    Contract(ContractCheck),
    ExternalBoundary,
    Synthesis(SynthesisResult),
    Proof(ProofResult),
    Engine(String),
    Emit(String),
}

impl fmt::Display for CompileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Contract(ContractCheck::Invalid(message)) => {
                write!(f, "contract_check INVALID: {message}")
            }
            Self::Contract(result) => write!(f, "contract_check {}", result.status_name()),
            Self::ExternalBoundary => {
                f.write_str("extern declaration requires an outer host adapter")
            }
            Self::Synthesis(result) => write!(f, "synthesis_result {}", result.status_name()),
            Self::Proof(ProofResult::Counterexample(inputs)) => {
                write!(f, "proof_result COUNTEREXAMPLE at {inputs:?}")
            }
            Self::Proof(result) => write!(f, "proof_result {}", result.status_name()),
            Self::Engine(message) => f.write_str(message),
            Self::Emit(message) => write!(f, "WASM emission failed: {message}"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BoundaryError {
    InputCount {
        expected: usize,
        received: usize,
    },
    InputOutOfRange {
        index: usize,
        expected: &'static str,
        value: i128,
    },
    ValueTypeMismatch {
        index: usize,
        expected: &'static str,
        received: &'static str,
    },
    UnsupportedCoreType {
        index: usize,
        ty: Type,
    },
    InvalidCoreBooleanOutput(u32),
    UnsupportedCoreOutput(Type),
}

impl fmt::Display for BoundaryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InputCount { expected, received } => {
                write!(
                    f,
                    "application input count: expected {expected}, received {received}"
                )
            }
            Self::InputOutOfRange {
                index,
                expected,
                value,
            } => write!(
                f,
                "application input {index}: {value} is outside {expected}"
            ),
            Self::ValueTypeMismatch {
                index,
                expected,
                received,
            } => write!(
                f,
                "application input {index}: expected {expected}, received {received}"
            ),
            Self::UnsupportedCoreType { index, ty } => {
                write!(
                    f,
                    "application input {index}: core type {ty:?} is unsupported"
                )
            }
            Self::InvalidCoreBooleanOutput(value) => {
                write!(f, "verified core returned non-boolean value {value}")
            }
            Self::UnsupportedCoreOutput(ty) => {
                write!(f, "verified core output type {ty:?} is unsupported")
            }
        }
    }
}

/// Validation, WASM, and effect failures stay outside the v4 proof result model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BoundaryFailure {
    PreconditionRejected(String),
    CoreRuntime(String),
    Effect(EffectFailure),
    Output(BoundaryError),
}

impl fmt::Display for BoundaryFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PreconditionRejected(message) => {
                write!(f, "application precondition rejected: {message}")
            }
            Self::CoreRuntime(message) => write!(f, "verified WASM runtime failed: {message}"),
            Self::Effect(error) => write!(f, "effect failed: {error}"),
            Self::Output(error) => error.fmt(f),
        }
    }
}

/// Application computation: validated application values around a pure core call.
#[derive(Debug, Clone)]
pub struct ApplicationCall {
    verified: Box<VerifiedComputation>,
    lowered_inputs: Vec<u32>,
}

impl ApplicationCall {
    pub fn new(
        verified: VerifiedComputation,
        inputs: &[ApplicationValue],
    ) -> Result<Self, BoundaryError> {
        if inputs.len() != verified.contract.params.len() {
            return Err(BoundaryError::InputCount {
                expected: verified.contract.params.len(),
                received: inputs.len(),
            });
        }
        let lowered_inputs = inputs
            .iter()
            .zip(&verified.contract.params)
            .enumerate()
            .map(|(index, (value, param))| lower_input(value, &param.ty, index))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self {
            verified: Box::new(verified),
            lowered_inputs,
        })
    }

    pub fn execute(self) -> Result<ApplicationValue, BoundaryFailure> {
        axiom_synth::check_requires(&self.verified.contract, &self.lowered_inputs)
            .map_err(BoundaryFailure::PreconditionRejected)?;
        let result = axiom_wasm::run_with_args(&self.verified.wasm, &self.lowered_inputs)
            .map_err(BoundaryFailure::CoreRuntime)?;
        lift_output(result, &self.verified.contract.return_type).map_err(BoundaryFailure::Output)
    }
}

/// An external effect request is data for an outer host adapter; it is never
/// executed by the verified core.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectRequest {
    pub name: String,
    pub inputs: Vec<ApplicationValue>,
}

impl EffectRequest {
    pub fn new(name: impl Into<String>, inputs: Vec<ApplicationValue>) -> Self {
        Self {
            name: name.into(),
            inputs,
        }
    }

    pub fn dispatch_without_adapter(self) -> Result<ApplicationValue, BoundaryFailure> {
        Err(BoundaryFailure::Effect(EffectFailure::AdapterUnavailable(
            self.name,
        )))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EffectFailure {
    AdapterUnavailable(String),
}

impl fmt::Display for EffectFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AdapterUnavailable(name) => write!(f, "no host adapter registered for {name}"),
        }
    }
}

/// One-step outer runtime representation. Effects are classified but not run.
#[derive(Debug, Clone)]
pub enum BoundaryOperation {
    Application(ApplicationCall),
    Effect(EffectRequest),
}

impl BoundaryOperation {
    pub fn execute(self) -> Result<ApplicationValue, BoundaryFailure> {
        match self {
            Self::Application(call) => call.execute(),
            Self::Effect(request) => request.dispatch_without_adapter(),
        }
    }
}

fn lower_input(value: &ApplicationValue, ty: &Type, index: usize) -> Result<u32, BoundaryError> {
    match ty {
        Type::U32 => match value {
            ApplicationValue::Integer(value) if (0..=u32::MAX as i128).contains(value) => {
                Ok(*value as u32)
            }
            ApplicationValue::Integer(value) => Err(BoundaryError::InputOutOfRange {
                index,
                expected: "u32",
                value: *value,
            }),
            value => Err(type_mismatch(index, "u32", value)),
        },
        Type::I32 => match value {
            ApplicationValue::Integer(value)
                if (i32::MIN as i128..=i32::MAX as i128).contains(value) =>
            {
                Ok(*value as i32 as u32)
            }
            ApplicationValue::Integer(value) => Err(BoundaryError::InputOutOfRange {
                index,
                expected: "i32",
                value: *value,
            }),
            value => Err(type_mismatch(index, "i32", value)),
        },
        Type::Bool => match value {
            ApplicationValue::Boolean(value) => Ok(u32::from(*value)),
            value => Err(type_mismatch(index, "bool", value)),
        },
        ty => Err(BoundaryError::UnsupportedCoreType {
            index,
            ty: ty.clone(),
        }),
    }
}

fn lift_output(value: u32, ty: &Type) -> Result<ApplicationValue, BoundaryError> {
    match ty {
        Type::U32 => Ok(ApplicationValue::Integer(i128::from(value))),
        Type::I32 => Ok(ApplicationValue::Integer(i128::from(value as i32))),
        Type::Bool => match value {
            0 => Ok(ApplicationValue::Boolean(false)),
            1 => Ok(ApplicationValue::Boolean(true)),
            value => Err(BoundaryError::InvalidCoreBooleanOutput(value)),
        },
        ty => Err(BoundaryError::UnsupportedCoreOutput(ty.clone())),
    }
}

fn type_mismatch(index: usize, expected: &'static str, value: &ApplicationValue) -> BoundaryError {
    BoundaryError::ValueTypeMismatch {
        index,
        expected,
        received: value.kind(),
    }
}

#[cfg(test)]
mod tests {
    use axiom_parser::parse;

    use super::*;

    fn mul8() -> Contract {
        parse(include_str!("../../../examples/basics/mul8.ax")).unwrap()
    }

    #[test]
    fn rejects_out_of_range_and_unsupported_values_before_core_execution() {
        let verified = VerifiedComputation::synthesize(&mul8()).unwrap();
        assert!(matches!(
            ApplicationCall::new(
                verified.clone(),
                &[ApplicationValue::Integer(i128::from(u32::MAX) + 1)]
            ),
            Err(BoundaryError::InputOutOfRange { .. })
        ));
        for value in [
            ApplicationValue::Text("5".into()),
            ApplicationValue::List(vec![ApplicationValue::Integer(5)]),
            ApplicationValue::Record(BTreeMap::new()),
        ] {
            assert!(matches!(
                ApplicationCall::new(verified.clone(), &[value]),
                Err(BoundaryError::ValueTypeMismatch { .. })
            ));
        }
    }

    #[test]
    fn application_precondition_failure_is_not_a_proof_counterexample() {
        let contract = parse(
            "contract guarded(value: u32) -> u32 { requires value > 0; ensures result == value; }",
        )
        .unwrap();
        let verified = VerifiedComputation::synthesize(&contract).unwrap();
        let call = ApplicationCall::new(verified, &[ApplicationValue::Integer(0)]).unwrap();
        assert!(matches!(
            call.execute(),
            Err(BoundaryFailure::PreconditionRejected(_))
        ));
    }

    #[test]
    fn application_boundary_does_not_coerce_integer_to_boolean() {
        let contract =
            parse("contract identity(value: bool) -> bool { ensures result == value; }").unwrap();
        let verified = VerifiedComputation::synthesize(&contract).unwrap();
        let call =
            ApplicationCall::new(verified.clone(), &[ApplicationValue::Boolean(true)]).unwrap();
        assert_eq!(call.execute().unwrap(), ApplicationValue::Boolean(true));
        assert!(matches!(
            ApplicationCall::new(verified, &[ApplicationValue::Integer(1)]),
            Err(BoundaryError::ValueTypeMismatch {
                expected: "bool",
                received: "integer",
                ..
            })
        ));
    }

    #[test]
    fn application_boundary_round_trips_signed_i32_values() {
        let contract = parse(include_str!("../../../examples/basics/signed_min.ax")).unwrap();
        let verified = VerifiedComputation::synthesize(&contract).unwrap();
        let call = ApplicationCall::new(
            verified.clone(),
            &[ApplicationValue::Integer(-1), ApplicationValue::Integer(1)],
        )
        .unwrap();
        assert_eq!(call.execute().unwrap(), ApplicationValue::Integer(-1));

        assert!(matches!(
            ApplicationCall::new(
                verified,
                &[
                    ApplicationValue::Integer(i128::from(i32::MAX) + 1),
                    ApplicationValue::Integer(1),
                ]
            ),
            Err(BoundaryError::InputOutOfRange {
                expected: "i32",
                ..
            })
        ));
    }

    #[test]
    fn rejects_non_boolean_core_output_when_lifting() {
        assert_eq!(
            lift_output(2, &Type::Bool),
            Err(BoundaryError::InvalidCoreBooleanOutput(2))
        );
    }

    #[test]
    fn wasm_runtime_failure_is_not_a_proof_counterexample() {
        let mut verified = VerifiedComputation::synthesize(&mul8()).unwrap();
        verified.wasm = vec![0];
        let call = ApplicationCall::new(verified, &[ApplicationValue::Integer(5)]).unwrap();
        assert!(matches!(
            call.execute(),
            Err(BoundaryFailure::CoreRuntime(_))
        ));
    }

    #[test]
    fn effect_failure_is_separate_from_proof_results() {
        let operation = BoundaryOperation::Effect(EffectRequest::new("clock.now", vec![]));
        assert!(matches!(
            operation.execute(),
            Err(BoundaryFailure::Effect(EffectFailure::AdapterUnavailable(name)))
                if name == "clock.now"
        ));
    }

    #[test]
    fn extern_declarations_stay_outside_the_pure_computation() {
        let external = parse("extern fn read_count(key: u32) -> u32;").unwrap();
        assert!(matches!(
            VerifiedComputation::synthesize(&external),
            Err(CompileError::ExternalBoundary)
        ));
    }
}
