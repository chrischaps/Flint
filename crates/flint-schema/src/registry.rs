//! Schema registry for loading and managing schemas

use crate::archetype::{ArchetypeSchema, ArchetypeSchemaFile};
use crate::component::{ComponentSchema, ComponentSchemaFile};
use flint_core::project::SchemaDirs;
use flint_core::vfs;
use flint_core::{FlintError, Result};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// The `schemas/` directories beside a scene and one level up, whichever
/// exist.
#[deprecated(note = "one resolver decides the schema directories now (ADR 0114): \
            `SchemaRegistry::resolve`, or `flint_core::project::schema_dirs` for the list alone")]
pub fn discover_schema_dirs(scene_path: &str) -> Vec<PathBuf> {
    flint_core::project::host_schema_dirs(&[] as &[&Path], Some(Path::new(scene_path))).dirs
}

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

    /// The registry for a scene (or for the working directory when a command
    /// has none), with the directories it was read from and where that list
    /// came from. The one way a command, the viewer or the player gets its
    /// schemas (ADR 0114): `flags` (`--schemas`) if any, else the project
    /// manifest's `schemas`, else `schemas/` in the working directory,
    /// beside the scene and one level up. See
    /// [`flint_core::project::schema_dirs`]. A named directory that isn't
    /// there is an error; a discovery that finds none is an empty registry
    /// and an empty list, for the caller to remark on.
    pub fn resolve(scene: Option<&Path>, flags: &[impl AsRef<Path>]) -> Result<(Self, SchemaDirs)> {
        let dirs = flint_core::project::schema_dirs(scene, flags)?;
        Ok((Self::load_from_directories(&dirs.dirs)?, dirs))
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
        if vfs::exists(&components_path) {
            for file_path in vfs::read_dir(&components_path)? {
                if file_path.extension().map(|e| e == "toml").unwrap_or(false) {
                    registry.load_component_file(&file_path)?;
                }
            }
        }

        // Load archetypes
        let archetypes_path = path.join("archetypes");
        if vfs::exists(&archetypes_path) {
            for file_path in vfs::read_dir(&archetypes_path)? {
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
        if vfs::exists(&components_path) {
            for file_path in vfs::read_dir(&components_path)? {
                if file_path.extension().map(|e| e == "toml").unwrap_or(false) {
                    self.load_component_file(&file_path)?;
                }
            }
        }

        // Load archetypes
        let archetypes_path = path.join("archetypes");
        if vfs::exists(&archetypes_path) {
            for file_path in vfs::read_dir(&archetypes_path)? {
                if file_path.extension().map(|e| e == "toml").unwrap_or(false) {
                    self.load_archetype_file(&file_path)?;
                }
            }
        }

        Ok(())
    }

    /// Load a component schema from a TOML file
    pub fn load_component_file<P: AsRef<Path>>(&mut self, path: P) -> Result<()> {
        let path = path.as_ref();
        let content = vfs::read_to_string(path)?;
        let file: ComponentSchemaFile = toml::from_str(&content)?;
        self.insert_components(file, &path.display().to_string());
        Ok(())
    }

    fn insert_components(&mut self, file: ComponentSchemaFile, source: &str) {
        for (name, def) in file.component {
            let mut fields = HashMap::new();
            for (field_name, field_def) in def.fields {
                if let Some(type_name) = field_def.unknown_type_name() {
                    tracing::warn!(
                        "{source}: component '{name}' field '{field_name}' has unknown type                          \"{type_name}\"; validating it as a string"
                    );
                }
                fields.insert(field_name, field_def.to_field_schema());
            }

            let schema = ComponentSchema {
                name: name.clone(),
                description: def.description,
                fields,
            };
            self.components.insert(name, schema);
        }
    }

    /// Load an archetype schema from a TOML file
    pub fn load_archetype_file<P: AsRef<Path>>(&mut self, path: P) -> Result<()> {
        let content = vfs::read_to_string(path)?;
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
        self.insert_components(file, "<string>");
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
    use flint_core::project::SchemaSource;
    use flint_core::vfs::MemorySource;
    use std::fs;
    use std::sync::Arc;

    fn lamp(description: &str) -> String {
        format!(
            "[component.lamp]\ndescription = \"{description}\"\n\n\
             [component.lamp.fields]\non = {{ type = \"bool\", default = true }}\n"
        )
    }

    fn describe(registry: &SchemaRegistry) -> Option<&str> {
        registry.get_component("lamp")?.description.as_deref()
    }

    /// The one resolver, end to end over the vfs: the manifest's list in
    /// its order (the game over the engine), flags instead when given, and
    /// a missing named directory as the error.
    #[test]
    fn resolve_loads_what_the_flags_or_the_manifest_name() {
        let mut files = MemorySource::new();
        files.insert(
            "flint.toml",
            "[project]\nschemas = [\"engine/schemas\", \"schemas\"]\n",
        );
        files.insert("engine/schemas/components/lamp.toml", lamp("engine"));
        files.insert("schemas/components/lamp.toml", lamp("game"));
        files.insert("scenes/a.scene.toml", "");
        vfs::scoped(Arc::new(files), || {
            let scene = Some(Path::new("scenes/a.scene.toml"));
            let none: &[&str] = &[];
            let (registry, dirs) = SchemaRegistry::resolve(scene, none).unwrap();
            assert_eq!(dirs.source.name(), "manifest");
            assert_eq!(dirs.dirs.len(), 2);
            assert_eq!(
                describe(&registry),
                Some("game"),
                "the later directory wins"
            );

            let (registry, dirs) = SchemaRegistry::resolve(scene, &["engine/schemas"]).unwrap();
            assert_eq!(dirs.source, SchemaSource::Flags);
            assert_eq!(describe(&registry), Some("engine"));

            // Flag order is override order too
            let (registry, _) =
                SchemaRegistry::resolve(scene, &["schemas", "engine/schemas"]).unwrap();
            assert_eq!(describe(&registry), Some("engine"));

            let err = SchemaRegistry::resolve(scene, &["engine/schemas", "nope"]).unwrap_err();
            assert_eq!(
                err.to_string(),
                "schema directory not found: nope (named by --schemas)"
            );
        });
    }

    /// No flags, no manifest: `schemas/` beside the scene and one level up,
    /// and an empty registry (no error) when there is none.
    #[test]
    fn resolve_discovers_without_a_manifest() {
        let mut files = MemorySource::new();
        files.insert("game/schemas/components/lamp.toml", lamp("one up"));
        files.insert("game/levels/a.scene.toml", "");
        files.insert("bare/a.scene.toml", "");
        vfs::scoped(Arc::new(files), || {
            let none: &[&str] = &[];
            let (registry, dirs) =
                SchemaRegistry::resolve(Some("game/levels/a.scene.toml".as_ref()), none).unwrap();
            assert_eq!(dirs.source, SchemaSource::Discovery);
            assert_eq!(describe(&registry), Some("one up"));

            let (registry, dirs) =
                SchemaRegistry::resolve(Some("bare/a.scene.toml".as_ref()), none).unwrap();
            assert!(dirs.dirs.is_empty());
            assert!(registry.get_component("lamp").is_none());
        });
    }

    /// Every field in the engine's own schemas names a type the parser knows,
    /// so none silently validates as a string (H8: `"integer"` did).
    #[test]
    fn engine_schemas_name_only_known_field_types() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../schemas/components");
        let mut unknown = Vec::new();
        let mut files = 0;
        for entry in fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().map(|e| e != "toml").unwrap_or(true) {
                continue;
            }
            files += 1;
            let file: ComponentSchemaFile =
                toml::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
            for (name, def) in &file.component {
                for (field, field_def) in &def.fields {
                    if let Some(t) = field_def.unknown_type_name() {
                        let file_name = path.file_name().unwrap().to_string_lossy();
                        unknown.push(format!("{file_name}: {name}.{field} = \"{t}\""));
                    }
                }
            }
        }
        assert!(files > 0, "no schemas found in {}", dir.display());
        assert!(
            unknown.is_empty(),
            "unknown field types:
{}",
            unknown.join(
                "
"
            )
        );
    }

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

    /// An array of free-form tables (animator layers) validates as tables,
    /// where it used to validate each element as a string and fail.
    #[test]
    fn array_of_tables_validates() {
        let toml = r#"
[component.animator.fields]
layers = { type = "array", element = "table", default = [] }
"#;
        let mut registry = SchemaRegistry::new();
        registry.load_component_string(toml).unwrap();
        let schema = registry.get_component("animator").unwrap();

        let good: toml::Value =
            toml::from_str("layers = [{ clip = \"wave\", weight = 0.5 }]").unwrap();
        crate::validate_component_data(schema, &good).unwrap();
        let bad: toml::Value = toml::from_str("layers = [\"wave\"]").unwrap();
        assert!(crate::validate_component_data(schema, &bad).is_err());
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
