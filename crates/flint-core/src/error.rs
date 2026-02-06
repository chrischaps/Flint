//! Error types for Flint

use thiserror::Error;

/// The main error type for Flint operations
#[derive(Debug, Error)]
pub enum FlintError {
    #[error("Entity not found: {0}")]
    EntityNotFound(String),

    #[error("Component not found: {0}")]
    ComponentNotFound(String),

    #[error("Archetype not found: {0}")]
    ArchetypeNotFound(String),

    #[error("Schema not found: {0}")]
    SchemaNotFound(String),

    #[error("Validation error: {0}")]
    ValidationError(String),

    #[error("Parse error: {0}")]
    ParseError(String),

    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),

    #[error("TOML parse error: {0}")]
    TomlParseError(String),

    #[error("TOML serialization error: {0}")]
    TomlSerError(String),

    #[error("Scene error: {0}")]
    SceneError(String),

    #[error("Query error: {0}")]
    QueryError(String),

    #[error("Render error: {0}")]
    RenderError(String),

    #[error("Duplicate entity name: {0}")]
    DuplicateEntityName(String),

    #[error("Invalid field type: expected {expected}, got {got}")]
    InvalidFieldType { expected: String, got: String },

    #[error("Missing required field: {0}")]
    MissingRequiredField(String),

    #[error("Value out of range: {field} must be between {min} and {max}, got {value}")]
    ValueOutOfRange {
        field: String,
        min: f64,
        max: f64,
        value: f64,
    },

    #[error("Invalid enum value: {value} is not one of {allowed:?}")]
    InvalidEnumValue {
        value: String,
        allowed: Vec<String>,
    },

    #[error("Constraint violation: {0}")]
    ConstraintViolation(String),

    #[error("Constraint load error: {0}")]
    ConstraintLoadError(String),

    #[error("Asset error: {0}")]
    AssetError(String),

    #[error("Import error: {0}")]
    ImportError(String),

    #[error("Fix cycle detected: {0}")]
    FixCycleDetected(String),

    #[error("Physics error: {0}")]
    PhysicsError(String),

    #[error("Audio error: {0}")]
    AudioError(String),

    #[error("Runtime error: {0}")]
    RuntimeError(String),

    #[error("Animation error: {0}")]
    AnimationError(String),

    #[error("Generation error: {0}")]
    GenerationError(String),
}

/// Result type alias for Flint operations
pub type Result<T> = std::result::Result<T, FlintError>;

impl From<toml::de::Error> for FlintError {
    fn from(err: toml::de::Error) -> Self {
        FlintError::TomlParseError(err.to_string())
    }
}

impl From<toml::ser::Error> for FlintError {
    fn from(err: toml::ser::Error) -> Self {
        FlintError::TomlSerError(err.to_string())
    }
}
