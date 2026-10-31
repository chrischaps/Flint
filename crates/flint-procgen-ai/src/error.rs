use thiserror::Error;

/// Errors specific to AI-assisted procedural generation.
#[derive(Debug, Error)]
pub enum ProcGenAiError {
    /// Invalid or malformed spec.
    #[error("spec error: {0}")]
    SpecError(String),

    /// Agent communication or processing failure.
    #[error("agent error: {0}")]
    AgentError(String),

    /// Configuration issue (missing API key, bad model name, etc.).
    #[error("config error: {0}")]
    ConfigError(String),

    /// An error from the procgen subsystem itself.
    #[error(transparent)]
    ProcGen(#[from] flint_procgen::ProcGenError),
}

pub type Result<T> = std::result::Result<T, ProcGenAiError>;
