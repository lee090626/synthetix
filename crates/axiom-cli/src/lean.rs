//! Checks a rule's proof with Lean (`--lean`, issue #211).

use std::fs;
use std::path::Path;
use std::process::Command;

use axiom_ami::Expr;
use axiom_parser::Contract;
use axiom_verify::lean::{LeanExport, export};

pub enum Outcome {
    Proved,
    /// Lean rejected the theorem; holds Lean's output.
    Failed(String),
    /// Lean ran out of time or memory before deciding (`bv_decide`'s SAT
    /// solver or heartbeats); the rule is neither proven nor disproven.
    Unproven,
    /// The export does not translate this rule yet; holds the reason.
    NotCovered(String),
    /// The file was written but `lean` is not installed.
    Skipped,
}

impl Outcome {
    pub fn line(&self) -> String {
        match self {
            Outcome::Proved => "PROVED".to_string(),
            Outcome::Failed(_) => "FAILED".to_string(),
            Outcome::Unproven => "UNPROVEN (Lean timed out)".to_string(),
            Outcome::NotCovered(reason) => format!("NOT_COVERED ({reason})"),
            Outcome::Skipped => "SKIPPED (lean not found)".to_string(),
        }
    }
}

/// Writes the theorem to `path` (unless the rule is not covered) and runs `lean` on it.
pub fn prove(contract: &Contract, candidate: &Expr, path: &Path) -> Result<Outcome, String> {
    let source = match export(contract, candidate) {
        LeanExport::Source(source) => source,
        LeanExport::NotCovered(reason) => return Ok(Outcome::NotCovered(reason)),
    };
    fs::write(path, source).map_err(|error| format!("cannot write {}: {error}", path.display()))?;
    Ok(match Command::new("lean").arg(path).output() {
        Err(_) => Outcome::Skipped,
        Ok(run) if run.status.success() => Outcome::Proved,
        Ok(run) => {
            let output = format!(
                "{}{}",
                String::from_utf8_lossy(&run.stdout),
                String::from_utf8_lossy(&run.stderr)
            );
            if output.contains("timed out") || output.contains("maximum number of heartbeats") {
                Outcome::Unproven
            } else {
                Outcome::Failed(output)
            }
        }
    })
}
