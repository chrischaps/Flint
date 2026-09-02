use flint_procgen::ProcGenSpec;

use crate::error::Result;

/// Feedback from an AI agent after analyzing a spec.
#[derive(Debug, Clone)]
pub struct SpecFeedback {
    /// Suggested improvements or observations.
    pub suggestions: Vec<String>,
    /// Agent's confidence in the spec quality (0.0 = low, 1.0 = high).
    pub confidence: f32,
    /// An optional revised spec incorporating the suggestions.
    pub revised_spec: Option<ProcGenSpec>,
}

/// An AI agent that can create, interpret, and refine procedural generation specs.
///
/// Object-safe (`Box<dyn ProcGenAgent>` works). Implementations must be `Send + Sync`
/// to support future async/threaded usage.
pub trait ProcGenAgent: Send + Sync {
    /// Analyze a spec and return feedback with suggestions.
    fn interpret_spec(&self, spec: &ProcGenSpec) -> Result<SpecFeedback>;

    /// Create a new spec from a natural-language prompt and target generator type.
    fn create_spec_from_prompt(&self, prompt: &str, generator: &str) -> Result<ProcGenSpec>;

    /// Refine an existing spec, optionally using a reference image for guidance.
    fn refine_spec(&self, spec: &ProcGenSpec, reference: Option<&[u8]>) -> Result<ProcGenSpec>;
}
