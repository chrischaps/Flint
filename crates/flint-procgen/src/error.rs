//! Error types for the procedural generation subsystem.

/// Errors that can occur during procedural generation.
#[derive(Debug, thiserror::Error)]
pub enum ProcGenError {
    /// The requested generator name was not found in the registry.
    #[error("unknown generator: {0}")]
    UnknownGenerator(String),

    /// A required parameter was not provided to the generator.
    #[error("missing parameter: {0}")]
    MissingParameter(String),

    /// A parameter value was invalid or out of range.
    #[error("invalid parameter `{name}`: {reason}")]
    InvalidParameter {
        /// The parameter name.
        name: String,
        /// Why the value is invalid.
        reason: String,
    },

    /// An error occurred while resolving or creating a seed.
    #[error("seed error: {0}")]
    SeedError(String),

    /// The generated mesh data is malformed.
    #[error("invalid mesh: {0}")]
    InvalidMesh(String),

    /// An error occurred while generating or processing image data.
    #[error("image error: {0}")]
    ImageError(String),

    /// An I/O error occurred.
    #[error(transparent)]
    IoError(#[from] std::io::Error),

    /// Failed to parse a generator spec string.
    #[error("spec parse error: {0}")]
    SpecParseError(String),

    /// The generation exceeded its polygon or memory budget.
    #[error("budget exceeded: {0}")]
    BudgetExceeded(String),
}

/// Convenience alias for `Result<T, ProcGenError>`.
pub type Result<T> = std::result::Result<T, ProcGenError>;
