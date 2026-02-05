//! Archetype schema definitions

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Schema definition for an archetype (a bundle of components)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArchetypeSchema {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    /// List of component names this archetype includes
    pub components: Vec<String>,
    /// Default values for component fields
    #[serde(default)]
    pub defaults: HashMap<String, toml::Value>,
}

impl ArchetypeSchema {
    /// Check if this archetype includes a specific component
    pub fn has_component(&self, component: &str) -> bool {
        self.components.iter().any(|c| c == component)
    }

    /// Get the default value for a component's field
    pub fn get_default(&self, component: &str, field: &str) -> Option<&toml::Value> {
        self.defaults
            .get(component)
            .and_then(|v| v.get(field))
    }
}

/// TOML file format for archetype schemas
#[derive(Debug, Deserialize)]
pub struct ArchetypeSchemaFile {
    pub archetype: HashMap<String, ArchetypeSchemaDefinition>,
}

#[derive(Debug, Deserialize)]
pub struct ArchetypeSchemaDefinition {
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub components: Vec<String>,
    #[serde(default)]
    pub defaults: HashMap<String, toml::Value>,
}

impl ArchetypeSchemaDefinition {
    pub fn to_archetype_schema(self, name: String) -> ArchetypeSchema {
        ArchetypeSchema {
            name,
            description: self.description,
            components: self.components,
            defaults: self.defaults,
        }
    }
}
