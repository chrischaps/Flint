//! Constraint type definitions

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Severity level for constraint violations
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Error,
    Warning,
    Info,
}

/// The kind of constraint to enforce
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ConstraintKind {
    /// Entity with given archetype must have a specific component
    RequiredComponent {
        archetype: String,
        component: String,
    },
    /// Entity must have a child with the given archetype
    RequiredChild { child_archetype: String },
    /// A numeric field must be within a range
    ValueRange {
        field: String,
        min: f64,
        max: f64,
    },
    /// A reference field must point to a valid entity
    ReferenceValid { field: String },
    /// A simple query-based rule expression
    QueryRule { rule: String },
}

/// Strategy for automatically fixing a constraint violation
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "strategy", rename_all = "snake_case")]
pub enum AutoFixStrategy {
    /// Add a child entity with the given archetype and defaults
    AddChild {
        archetype: String,
        #[serde(default)]
        defaults: HashMap<String, toml::Value>,
    },
    /// Set a field to a default value
    SetDefault { field: String, value: toml::Value },
    /// Remove the invalid field/component
    RemoveInvalid,
    /// Copy a field value from the parent entity
    AssignFromParent {
        field: String,
        source_field: String,
    },
}

/// Auto-fix configuration for a constraint
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutoFix {
    pub enabled: bool,
    #[serde(flatten)]
    pub strategy: AutoFixStrategy,
}

/// A complete constraint definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConstraintDef {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    /// Query in flint-query syntax to select entities
    pub query: String,
    pub kind: ConstraintKind,
    pub severity: Severity,
    /// Message template with `{name}` and `{archetype}` placeholders
    pub message: String,
    #[serde(default)]
    pub auto_fix: Option<AutoFix>,
}

/// TOML file format for constraint definitions
#[derive(Debug, Deserialize)]
pub struct ConstraintFile {
    pub constraint: Vec<ConstraintDef>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_constraint_from_toml() {
        let toml_str = r#"
[[constraint]]
name = "doors_have_handles"
query = "entities where archetype == 'door'"
severity = "warning"
message = "Door '{name}' is missing a handle"

[constraint.kind]
type = "required_child"
child_archetype = "handle"
"#;

        let file: ConstraintFile = toml::from_str(toml_str).unwrap();
        assert_eq!(file.constraint.len(), 1);
        let c = &file.constraint[0];
        assert_eq!(c.name, "doors_have_handles");
        assert_eq!(c.severity, Severity::Warning);
        assert!(matches!(&c.kind, ConstraintKind::RequiredChild { child_archetype } if child_archetype == "handle"));
        assert!(c.auto_fix.is_none());
    }

    #[test]
    fn test_parse_constraint_with_autofix() {
        let toml_str = r#"
[[constraint]]
name = "doors_have_handles"
query = "entities where archetype == 'door'"
severity = "warning"
message = "Door '{name}' is missing a handle"

[constraint.kind]
type = "required_child"
child_archetype = "handle"

[constraint.auto_fix]
enabled = true
strategy = "add_child"
archetype = "handle"
"#;

        let file: ConstraintFile = toml::from_str(toml_str).unwrap();
        let c = &file.constraint[0];
        let fix = c.auto_fix.as_ref().unwrap();
        assert!(fix.enabled);
        assert!(matches!(&fix.strategy, AutoFixStrategy::AddChild { archetype, .. } if archetype == "handle"));
    }

    #[test]
    fn test_parse_value_range_constraint() {
        let toml_str = r#"
[[constraint]]
name = "door_angle_valid"
query = "entities where archetype == 'door'"
severity = "error"
message = "Door '{name}' has invalid open_angle"

[constraint.kind]
type = "value_range"
field = "door.open_angle"
min = 0.0
max = 180.0
"#;

        let file: ConstraintFile = toml::from_str(toml_str).unwrap();
        let c = &file.constraint[0];
        assert!(matches!(&c.kind, ConstraintKind::ValueRange { field, min, max }
            if field == "door.open_angle" && *min == 0.0 && *max == 180.0));
    }

    #[test]
    fn test_parse_required_component_constraint() {
        let toml_str = r#"
[[constraint]]
name = "doors_need_transform"
query = "entities where archetype == 'door'"
severity = "error"
message = "Door '{name}' is missing transform component"

[constraint.kind]
type = "required_component"
archetype = "door"
component = "transform"
"#;

        let file: ConstraintFile = toml::from_str(toml_str).unwrap();
        let c = &file.constraint[0];
        assert!(matches!(&c.kind, ConstraintKind::RequiredComponent { archetype, component }
            if archetype == "door" && component == "transform"));
    }

    #[test]
    fn test_severity_ordering() {
        assert_eq!(Severity::Error, Severity::Error);
        assert_ne!(Severity::Error, Severity::Warning);
        assert_ne!(Severity::Warning, Severity::Info);
    }
}
