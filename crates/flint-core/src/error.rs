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

    /// The schema directories could not be named (a missing one, or a
    /// manifest that doesn't read): [`crate::project::schema_dirs`].
    #[error(transparent)]
    SchemaDirs(#[from] crate::project::SchemaDirsError),

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

    /// `set_parent(child, parent)` refused because `parent` is `child` or
    /// one of its descendants (#680). `chain` is the walk from `parent` up
    /// to `child`, by name: `[child]` for an entity given itself.
    #[error("{}", parent_cycle_message(.child, .parent, .chain))]
    ParentCycle {
        child: String,
        parent: String,
        chain: Vec<String>,
    },

    /// `field` is the dotted path inside the component (ADR 0115).
    #[error("Invalid field type: `{field}` expected {expected}, got {got}")]
    InvalidFieldType {
        field: String,
        expected: String,
        got: String,
    },

    #[error("Missing required field: {0}")]
    MissingRequiredField(String),

    #[error("Value out of range: {field} must be between {min} and {max}, got {value}")]
    ValueOutOfRange {
        field: String,
        min: f64,
        max: f64,
        value: f64,
    },

    #[error("Invalid enum value: `{field}` = \"{value}\" is not one of {allowed:?}")]
    InvalidEnumValue {
        field: String,
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

    #[error("Input error: {0}")]
    InputError(String),

    #[error("Animation error: {0}")]
    AnimationError(String),

    #[error("Generation error: {0}")]
    GenerationError(String),

    #[error("Project error: {0}")]
    ProjectError(#[from] crate::project::ProjectError),
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

/// [`FlintError::ParentCycle`]'s message: `parent = "lamp"` on `lamp`
/// itself, or the chain that would close into a loop.
fn parent_cycle_message(child: &str, parent: &str, chain: &[String]) -> String {
    if chain.len() <= 1 {
        format!("Parent cycle: `{child}` can't be parented to itself")
    } else {
        format!(
            "Parent cycle: `{child}` can't be parented to `{parent}`, which is below it \
             ({}, each the child of the next)",
            chain.join(" -> ")
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_parent_cycle_names_every_entity_on_the_loop() {
        let itself = FlintError::ParentCycle {
            child: "lamp".into(),
            parent: "lamp".into(),
            chain: vec!["lamp".into()],
        };
        assert_eq!(
            itself.to_string(),
            "Parent cycle: `lamp` can't be parented to itself"
        );
        let two = FlintError::ParentCycle {
            child: "cart".into(),
            parent: "wheel".into(),
            chain: vec!["wheel".into(), "axle".into(), "cart".into()],
        };
        assert_eq!(
            two.to_string(),
            "Parent cycle: `cart` can't be parented to `wheel`, which is below it \
             (wheel -> axle -> cart, each the child of the next)"
        );
    }
}
