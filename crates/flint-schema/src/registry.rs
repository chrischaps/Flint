//! Schema registry for loading and managing schemas

use crate::archetype::{ArchetypeSchema, ArchetypeSchemaFile};
use crate::component::{ComponentSchema, ComponentSchemaFile};
use flint_core::{FlintError, Result};
use std::collections::HashMap;
use std::fs;
use std::path::Path;

/// Registry that holds all loaded component and archetype schemas
#[derive(Debug, Default)]
pub struct SchemaRegistry {
    components: HashMap<String, ComponentSchema>,
    archetypes: HashMap<String, ArchetypeSchema>,
}

impl SchemaRegistry {
    /// Create a new empty registry
    pub fn new() -> Self {
        Self::default()
    }

    /// Load schemas from multiple directories, merging them in order.
    /// Later directories override earlier ones (game overrides engine).
    pub fn load_from_directories(paths: &[impl AsRef<Path>]) -> Result<Self> {
        let mut registry = Self::new();
        for path in paths {
            registry.load_directory(path)?;
        }
        Ok(registry)
    }

    /// Load schemas from a directory structure
    ///
    /// Expects:
    /// - `path/components/*.toml` for component schemas
    /// - `path/archetypes/*.toml` for archetype schemas
    pub fn load_from_directory<P: AsRef<Path>>(path: P) -> Result<Self> {
        let mut registry = Self::new();
        let path = path.as_ref();

        // Load components
        let components_path = path.join("components");
        if components_path.exists() {
            for entry in fs::read_dir(&components_path)? {
                let entry = entry?;
                let file_path = entry.path();
                if file_path.extension().map(|e| e == "toml").unwrap_or(false) {
                    registry.load_component_file(&file_path)?;
                }
            }
        }

        // Load archetypes
        let archetypes_path = path.join("archetypes");
        if archetypes_path.exists() {
            for entry in fs::read_dir(&archetypes_path)? {
                let entry = entry?;
                let file_path = entry.path();
                if file_path.extension().map(|e| e == "toml").unwrap_or(false) {
                    registry.load_archetype_file(&file_path)?;
                }
            }
        }

        Ok(registry)
    }

    /// Load schemas from a single directory into this registry (additive/override)
    pub fn load_directory<P: AsRef<Path>>(&mut self, path: P) -> Result<()> {
        let path = path.as_ref();

        // Load components
        let components_path = path.join("components");
        if components_path.exists() {
            for entry in fs::read_dir(&components_path)? {
                let entry = entry?;
                let file_path = entry.path();
                if file_path.extension().map(|e| e == "toml").unwrap_or(false) {
                    self.load_component_file(&file_path)?;
                }
            }
        }

        // Load archetypes
        let archetypes_path = path.join("archetypes");
        if archetypes_path.exists() {
            for entry in fs::read_dir(&archetypes_path)? {
                let entry = entry?;
                let file_path = entry.path();
                if file_path.extension().map(|e| e == "toml").unwrap_or(false) {
                    self.load_archetype_file(&file_path)?;
                }
            }
        }

        Ok(())
    }

    /// Load a component schema from a TOML file
    pub fn load_component_file<P: AsRef<Path>>(&mut self, path: P) -> Result<()> {
        let content = fs::read_to_string(path)?;
        let file: ComponentSchemaFile = toml::from_str(&content)?;

        for (name, def) in file.component {
            let mut fields = HashMap::new();
            for (field_name, field_def) in def.fields {
                fields.insert(field_name, field_def.to_field_schema());
            }

            let schema = ComponentSchema {
                name: name.clone(),
                description: def.description,
                fields,
            };
            self.components.insert(name, schema);
        }

        Ok(())
    }

    /// Load an archetype schema from a TOML file
    pub fn load_archetype_file<P: AsRef<Path>>(&mut self, path: P) -> Result<()> {
        let content = fs::read_to_string(path)?;
        let file: ArchetypeSchemaFile = toml::from_str(&content)?;

        for (name, def) in file.archetype {
            let schema = def.to_archetype_schema(name.clone());
            self.archetypes.insert(name, schema);
        }

        Ok(())
    }

    /// Load a component schema from a TOML string
    pub fn load_component_string(&mut self, content: &str) -> Result<()> {
        let file: ComponentSchemaFile = toml::from_str(content)?;

        for (name, def) in file.component {
            let mut fields = HashMap::new();
            for (field_name, field_def) in def.fields {
                fields.insert(field_name, field_def.to_field_schema());
            }

            let schema = ComponentSchema {
                name: name.clone(),
                description: def.description,
                fields,
            };
            self.components.insert(name, schema);
        }

        Ok(())
    }

    /// Load an archetype schema from a TOML string
    pub fn load_archetype_string(&mut self, content: &str) -> Result<()> {
        let file: ArchetypeSchemaFile = toml::from_str(content)?;

        for (name, def) in file.archetype {
            let schema = def.to_archetype_schema(name.clone());
            self.archetypes.insert(name, schema);
        }

        Ok(())
    }

    /// Register a component schema directly
    pub fn register_component(&mut self, schema: ComponentSchema) {
        self.components.insert(schema.name.clone(), schema);
    }

    /// Register an archetype schema directly
    pub fn register_archetype(&mut self, schema: ArchetypeSchema) {
        self.archetypes.insert(schema.name.clone(), schema);
    }

    /// Get a component schema by name
    pub fn get_component(&self, name: &str) -> Option<&ComponentSchema> {
        self.components.get(name)
    }

    /// Get an archetype schema by name
    pub fn get_archetype(&self, name: &str) -> Option<&ArchetypeSchema> {
        self.archetypes.get(name)
    }

    /// List all component names
    pub fn component_names(&self) -> Vec<&str> {
        self.components.keys().map(|s| s.as_str()).collect()
    }

    /// List all archetype names
    pub fn archetype_names(&self) -> Vec<&str> {
        self.archetypes.keys().map(|s| s.as_str()).collect()
    }

    /// Get all components for an archetype
    pub fn get_archetype_components(&self, archetype: &str) -> Result<Vec<&ComponentSchema>> {
        let arch = self
            .archetypes
            .get(archetype)
            .ok_or_else(|| FlintError::ArchetypeNotFound(archetype.to_string()))?;

        let mut components = Vec::new();
        for comp_name in &arch.components {
            if let Some(comp) = self.components.get(comp_name) {
                components.push(comp);
            }
        }
        Ok(components)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_load_component_string() {
        let toml = r#"
[component.door]
description = "A door component"

[component.door.fields]
locked = { type = "bool", default = false }
style = { type = "enum", values = ["hinged", "sliding"], default = "hinged" }
"#;

        let mut registry = SchemaRegistry::new();
        registry.load_component_string(toml).unwrap();

        let door = registry.get_component("door").unwrap();
        assert_eq!(door.name, "door");
        assert!(door.fields.contains_key("locked"));
        assert!(door.fields.contains_key("style"));
    }

    #[test]
    fn test_load_archetype_string() {
        let toml = r#"
[archetype.door]
description = "A door entity"
components = ["transform", "door"]

[archetype.door.defaults.door]
locked = false
"#;

        let mut registry = SchemaRegistry::new();
        registry.load_archetype_string(toml).unwrap();

        let door = registry.get_archetype("door").unwrap();
        assert_eq!(door.name, "door");
        assert!(door.has_component("transform"));
        assert!(door.has_component("door"));
    }
}
