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

    /// Merge fields into an existing component (archetype defaults + entity overrides)
    ///
    /// If the component already exists as a table, merges individual fields
    /// from `data` into it (entity-level fields win). If it doesn't exist
    /// or isn't a table, sets the component outright.
    pub fn merge_component(&mut self, component: impl Into<String>, data: toml::Value) {
        let key = component.into();
        if let Some(existing) = self.data.get_mut(&key) {
            if let (Some(existing_table), Some(override_table)) =
                (existing.as_table_mut(), data.as_table())
            {
                for (k, v) in override_table {
                    existing_table.insert(k.clone(), v.clone());
                }
                return;
            }
        }
        self.data.insert(key, data);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_merge_component_table_into_table() {
        let mut comps = DynamicComponents::new();
        comps.set(
            "material",
            toml::Value::Table({
                let mut m = toml::map::Map::new();
                m.insert("color".into(), toml::Value::String("red".into()));
                m.insert("size".into(), toml::Value::Integer(5));
                m
            }),
        );

        // Merge overrides size and adds visible
        comps.merge_component(
            "material",
            toml::Value::Table({
                let mut m = toml::map::Map::new();
                m.insert("size".into(), toml::Value::Integer(10));
                m.insert("visible".into(), toml::Value::Boolean(true));
                m
            }),
        );

        let mat = comps.get("material").unwrap();
        assert_eq!(mat.get("color").and_then(|v| v.as_str()), Some("red"));
        assert_eq!(mat.get("size").and_then(|v| v.as_integer()), Some(10));
        assert_eq!(mat.get("visible").and_then(|v| v.as_bool()), Some(true));
    }

    #[test]
    fn test_merge_component_into_empty() {
        let mut comps = DynamicComponents::new();
        assert!(comps.get("health").is_none());

        comps.merge_component(
            "health",
            toml::Value::Table({
                let mut m = toml::map::Map::new();
                m.insert("current".into(), toml::Value::Integer(100));
                m
            }),
        );

        assert!(comps.has("health"));
        assert_eq!(
            comps
                .get("health")
                .unwrap()
                .get("current")
                .and_then(|v| v.as_integer()),
            Some(100)
        );
    }

    #[test]
    fn test_merge_non_table_replaces() {
        let mut comps = DynamicComponents::new();
        comps.set("tag", toml::Value::String("old".into()));

        // Merge a table onto a string — should replace
        comps.merge_component(
            "tag",
            toml::Value::Table({
                let mut m = toml::map::Map::new();
                m.insert("name".into(), toml::Value::String("new".into()));
                m
            }),
        );

        let tag = comps.get("tag").unwrap();
        assert!(tag.is_table(), "non-table should be replaced by table");
        assert_eq!(tag.get("name").and_then(|v| v.as_str()), Some("new"));
    }

    #[test]
    fn test_merge_bulk() {
        let mut base = DynamicComponents::new();
        base.set(
            "transform",
            toml::Value::Table({
                let mut m = toml::map::Map::new();
                m.insert(
                    "position".into(),
                    toml::Value::Array(vec![
                        toml::Value::Float(1.0),
                        toml::Value::Float(2.0),
                        toml::Value::Float(3.0),
                    ]),
                );
                m.insert(
                    "scale".into(),
                    toml::Value::Array(vec![
                        toml::Value::Float(1.0),
                        toml::Value::Float(1.0),
                        toml::Value::Float(1.0),
                    ]),
                );
                m
            }),
        );

        let mut overrides = DynamicComponents::new();
        overrides.set(
            "transform",
            toml::Value::Table({
                let mut m = toml::map::Map::new();
                m.insert(
                    "position".into(),
                    toml::Value::Array(vec![
                        toml::Value::Float(10.0),
                        toml::Value::Float(20.0),
                        toml::Value::Float(30.0),
                    ]),
                );
                m
            }),
        );
        overrides.set(
            "health",
            toml::Value::Table({
                let mut m = toml::map::Map::new();
                m.insert("current".into(), toml::Value::Integer(50));
                m
            }),
        );

        base.merge(&overrides);

        // position overridden
        let t = base.get("transform").unwrap();
        let pos = t.get("position").unwrap().as_array().unwrap();
        assert!((pos[0].as_float().unwrap() - 10.0).abs() < 0.001);

        // scale preserved
        assert!(t.get("scale").is_some());

        // new component added
        assert!(base.has("health"));
    }

    #[test]
    fn test_set_field_creates_component() {
        let mut comps = DynamicComponents::new();
        assert!(!comps.has("stats"));

        comps.set_field("stats", "strength", toml::Value::Integer(18));

        assert!(comps.has("stats"));
        assert_eq!(
            comps.get_field("stats", "strength").and_then(|v| v.as_integer()),
            Some(18)
        );
    }
}
