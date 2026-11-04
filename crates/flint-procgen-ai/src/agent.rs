use flint_procgen::ProcGenSpec;

use crate::error::Result;
use crate::http::Usage;
use crate::measure::Distance;

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

/// One request/response round of a conversation.
#[derive(Debug, Clone, Default)]
pub struct Round {
    /// What the validator found in this round's reply; empty when accepted.
    pub issues: Vec<String>,
    /// Tokens this round cost.
    pub usage: Usage,
    /// The model that answered.
    pub model: String,
}

/// How a conversation went. Agents that make no calls return an empty one.
#[derive(Debug, Clone, Default)]
pub struct Report {
    /// Each round, in order.
    pub rounds: Vec<Round>,
    /// Params the schema repair had to fix after the last round (0 when
    /// the agent converged on its own).
    pub repaired: usize,
    /// The model's own account of its choices (`rationale` / `changes`).
    /// For a measured refine, the changes of the round that was kept.
    pub notes: Vec<String>,
    /// A refine's measured distance to the reference, per refine round:
    /// `[0]` is the spec as it came in, `[i]` what round `i` built. Empty
    /// when nothing could be measured (no reference, or no picture of
    /// what the spec builds).
    pub distances: Vec<Distance>,
    /// Which refine round's spec was returned: 0 for the spec as it came in
    /// (no round moved it closer), `i` for round `i`. A refine that isn't
    /// measured always keeps its one round. 0 for create and interpret.
    pub kept: usize,
}

impl Report {
    /// Whether the last round's reply passed validation unaided.
    pub fn converged(&self) -> bool {
        self.rounds.last().is_some_and(|r| r.issues.is_empty())
    }

    /// Tokens across all rounds.
    pub fn usage(&self) -> Usage {
        self.rounds
            .iter()
            .fold(Usage::default(), |a, r| a.plus(r.usage))
    }
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

    /// [`Self::create_spec_from_prompt`] with how the conversation went.
    /// The default reports nothing; agents that talk to a model override it.
    fn create_spec_with_report(
        &self,
        prompt: &str,
        generator: &str,
    ) -> Result<(ProcGenSpec, Report)> {
        Ok((
            self.create_spec_from_prompt(prompt, generator)?,
            Report::default(),
        ))
    }

    /// [`Self::interpret_spec`] with how the conversation went.
    fn interpret_spec_with_report(&self, spec: &ProcGenSpec) -> Result<(SpecFeedback, Report)> {
        Ok((self.interpret_spec(spec)?, Report::default()))
    }

    /// [`Self::refine_spec`] with how the conversation went.
    fn refine_spec_with_report(
        &self,
        spec: &ProcGenSpec,
        reference: Option<&[u8]>,
    ) -> Result<(ProcGenSpec, Report)> {
        Ok((self.refine_spec(spec, reference)?, Report::default()))
    }
}
