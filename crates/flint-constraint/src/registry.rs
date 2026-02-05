//! Constraint registry for loading and managing constraint definitions

use crate::types::{ConstraintDef, ConstraintFile, Severity};
use flint_core::{FlintError, Result};
use std::fs;
use std::path::Path;

/// Registry that holds all loaded constraint definitions
#[derive(Debug, Default)]
pub struct ConstraintRegistry {
    constraints: Vec<ConstraintDef>,
}

impl ConstraintRegistry {
    /// Create a new empty registry
    pub fn new() -> Self {
        Self::default()
    }

    /// Load constraints from a directory of TOML files
    ///
    /// Expects `path/constraints/*.toml` files
    pub fn load_from_directory<P: AsRef<Path>>(path: P) -> Result<Self> {
        let mut registry = Self::new();
        let constraints_path = path.as_ref().join("constraints");

        if constraints_path.exists() {
            for entry in fs::read_dir(&constraints_path)? {
                let entry = entry?;
                let file_path = entry.path();
                if file_path.extension().map(|e| e == "toml").unwrap_or(false) {
                    registry.load_file(&file_path)?;
                }
            }
        }

        Ok(registry)
    }

    /// Load constraints from a TOML file
    pub fn load_file<P: AsRef<Path>>(&mut self, path: P) -> Result<()> {
        let content = fs::read_to_string(path)?;
        self.load_string(&content)
    }

    /// Load constraints from a TOML string
    pub fn load_string(&mut self, content: &str) -> Result<()> {
        let file: ConstraintFile = toml::from_str(content).map_err(|e| {
            FlintError::ConstraintLoadError(format!("Failed to parse constraint TOML: {}", e))
        })?;

        for constraint in file.constraint {
            self.constraints.push(constraint);
        }

        Ok(())
    }

    /// Register a constraint directly
    pub fn register(&mut self, constraint: ConstraintDef) {
        self.constraints.push(constraint);
    }

    /// Get all constraints
    pub fn all(&self) -> &[ConstraintDef] {
        &self.constraints
    }

    /// Get constraints filtered by severity
    pub fn by_severity(&self, severity: Severity) -> Vec<&ConstraintDef> {
        self.constraints
            .iter()
            .filter(|c| c.severity == severity)
            .collect()
    }

    /// Get constraints that apply to a specific archetype
    ///
    /// This checks the query string for archetype references — a simple heuristic
    /// that works for the common pattern `"entities where archetype == '<name>'"`.
    pub fn for_archetype(&self, archetype: &str) -> Vec<&ConstraintDef> {
        self.constraints
            .iter()
            .filter(|c| c.query.contains(archetype))
            .collect()
    }

    /// Get the number of loaded constraints
    pub fn len(&self) -> usize {
        self.constraints.len()
    }

    /// Check if the registry is empty
    pub fn is_empty(&self) -> bool {
        self.constraints.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ConstraintKind;

    fn sample_toml() -> &'static str {
        r#"
[[constraint]]
name = "doors_have_handles"
query = "entities where archetype == 'door'"
severity = "warning"
message = "Door '{name}' is missing a handle"

[constraint.kind]
type = "required_child"
child_archetype = "handle"

[[constraint]]
name = "rooms_have_bounds"
query = "entities where archetype == 'room'"
severity = "error"
message = "Room '{name}' is missing bounds"

[constraint.kind]
type = "required_component"
archetype = "room"
component = "bounds"
"#
    }

    #[test]
    fn test_load_from_string() {
        let mut registry = ConstraintRegistry::new();
        registry.load_string(sample_toml()).unwrap();
        assert_eq!(registry.len(), 2);
    }

    #[test]
    fn test_all_constraints() {
        let mut registry = ConstraintRegistry::new();
        registry.load_string(sample_toml()).unwrap();
        assert_eq!(registry.all().len(), 2);
    }

    #[test]
    fn test_by_severity() {
        let mut registry = ConstraintRegistry::new();
        registry.load_string(sample_toml()).unwrap();

        let warnings = registry.by_severity(Severity::Warning);
        assert_eq!(warnings.len(), 1);
        assert_eq!(warnings[0].name, "doors_have_handles");

        let errors = registry.by_severity(Severity::Error);
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].name, "rooms_have_bounds");
    }

    #[test]
    fn test_for_archetype() {
        let mut registry = ConstraintRegistry::new();
        registry.load_string(sample_toml()).unwrap();

        let door_constraints = registry.for_archetype("door");
        assert_eq!(door_constraints.len(), 1);
        assert_eq!(door_constraints[0].name, "doors_have_handles");

        let room_constraints = registry.for_archetype("room");
        assert_eq!(room_constraints.len(), 1);
        assert_eq!(room_constraints[0].name, "rooms_have_bounds");
    }

    #[test]
    fn test_register_directly() {
        let mut registry = ConstraintRegistry::new();
        registry.register(ConstraintDef {
            name: "test_constraint".to_string(),
            description: None,
            query: "entities".to_string(),
            kind: ConstraintKind::RequiredChild {
                child_archetype: "child".to_string(),
            },
            severity: Severity::Info,
            message: "Test message".to_string(),
            auto_fix: None,
        });
        assert_eq!(registry.len(), 1);
    }

    #[test]
    fn test_empty_registry() {
        let registry = ConstraintRegistry::new();
        assert!(registry.is_empty());
        assert_eq!(registry.len(), 0);
    }
}
