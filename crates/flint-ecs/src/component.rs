//! Dynamic component storage

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Dynamic components stored as TOML values
///
/// This allows archetypes to be defined at runtime in schema files
/// rather than requiring Rust types for each component.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DynamicComponents {
    /// The archetype name for this entity (if any)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub archetype: Option<String>,
    /// Component data: component_name -> field data
    #[serde(flatten)]
    pub data: HashMap<String, toml::Value>,
}

impl DynamicComponents {
    /// Create empty components
    pub fn new() -> Self {
        Self::default()
    }

    /// Create components with an archetype
    pub fn with_archetype(archetype: impl Into<String>) -> Self {
        Self {
            archetype: Some(archetype.into()),
            data: HashMap::new(),
        }
    }

    /// Get component data by name
    pub fn get(&self, component: &str) -> Option<&toml::Value> {
        self.data.get(component)
    }

    /// Get mutable component data by name
    pub fn get_mut(&mut self, component: &str) -> Option<&mut toml::Value> {
        self.data.get_mut(component)
    }

    /// Set component data
    pub fn set(&mut self, component: impl Into<String>, data: toml::Value) {
        self.data.insert(component.into(), data);
    }

    /// Remove a component
    pub fn remove(&mut self, component: &str) -> Option<toml::Value> {
        self.data.remove(component)
    }

    /// Check if a component exists
    pub fn has(&self, component: &str) -> bool {
        self.data.contains_key(component)
    }

    /// Get all component names
    pub fn component_names(&self) -> Vec<&str> {
        self.data.keys().map(|s| s.as_str()).collect()
    }

    /// Get a field value from a component
    pub fn get_field(&self, component: &str, field: &str) -> Option<&toml::Value> {
        self.data.get(component).and_then(|v| v.get(field))
    }

    /// Set a field value in a component
    pub fn set_field(&mut self, component: &str, field: &str, value: toml::Value) {
        let comp = self
            .data
            .entry(component.to_string())
            .or_insert_with(|| toml::Value::Table(toml::map::Map::new()));

        if let Some(table) = comp.as_table_mut() {
            table.insert(field.to_string(), value);
        }
    }

    /// Merge data from another DynamicComponents
    pub fn merge(&mut self, other: &DynamicComponents) {
        for (name, value) in &other.data {
            if let Some(existing) = self.data.get_mut(name) {
                // Merge tables, overwrite scalars
                if let (Some(existing_table), Some(other_table)) =
                    (existing.as_table_mut(), value.as_table())
                {
                    for (k, v) in other_table {
                        existing_table.insert(k.clone(), v.clone());
                    }
                } else {
                    self.data.insert(name.clone(), value.clone());
                }
            } else {
                self.data.insert(name.clone(), value.clone());
            }
        }
    }
}
