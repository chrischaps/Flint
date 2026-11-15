//! Schema registry for loading and managing schemas

use crate::archetype::{ArchetypeSchema, ArchetypeSchemaFile};
use crate::component::{
    ComponentSchema, ComponentSchemaDefinition, ComponentSchemaFile, TableSchema, TableSchemaFile,
};
use flint_core::diagnostic::Diagnostic;
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

/// A schema file that isn't the TOML its loader reads, as an error that
/// names the file (#333). The parser's message alone (`expected newline,
/// found #`) was all `flint render` printed for a bad
/// `components/style_mask.toml`.
fn parse_error(path: &Path, e: toml::de::Error) -> FlintError {
    FlintError::TomlParseError(format!(
        "{}: {}",
        flint_core::project::shown(path),
        e.to_string().trim_end()
    ))
}

/// Registry that holds all loaded component and archetype schemas
#[derive(Debug, Default, Clone)]
pub struct SchemaRegistry {
    components: HashMap<String, ComponentSchema>,
    archetypes: HashMap<String, ArchetypeSchema>,
    /// Scene-level tables (`[post_process]`, a game's `[weather]`), from
    /// `scene/*.toml` (#686).
    tables: HashMap<String, TableSchema>,
    /// What [`crate::lint`] found in each component and scene-table file
    /// loaded, in load order.
    lints: Vec<Diagnostic>,
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
    /// - `path/scene/*.toml` for scene-table schemas
    pub fn load_from_directory<P: AsRef<Path>>(path: P) -> Result<Self> {
        let mut registry = Self::new();
        let path = path.as_ref();
        registry.load_table_directory(path)?;

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
        self.load_table_directory(path)?;

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

    /// The `[table.<name>]` files of schema directory `path`: its
    /// `scene/*.toml` (#686). A directory without a `scene/` has none.
    fn load_table_directory(&mut self, path: &Path) -> Result<()> {
        let tables_path = path.join(crate::lint::SchemaKind::Table.dir());
        if vfs::exists(&tables_path) {
            for file_path in vfs::read_dir(&tables_path)? {
                if file_path.extension().map(|e| e == "toml").unwrap_or(false) {
                    self.load_table_file(&file_path)?;
                }
            }
        }
        Ok(())
    }

    /// Load the scene-table schemas of a TOML file (`[table.<name>]`).
    pub fn load_table_file<P: AsRef<Path>>(&mut self, path: P) -> Result<()> {
        let path = path.as_ref();
        let content = vfs::read_to_string(path)?;
        let file: TableSchemaFile = toml::from_str(&content).map_err(|e| parse_error(path, e))?;
        self.lints
            .extend(crate::lint::lint_table_file(&content, Some(path)));
        self.insert_tables(file, &path.display().to_string());
        Ok(())
    }

    /// Load scene-table schemas from a TOML string.
    pub fn load_table_string(&mut self, content: &str) -> Result<()> {
        let file: TableSchemaFile = toml::from_str(content)?;
        self.lints
            .extend(crate::lint::lint_table_file(content, None));
        self.insert_tables(file, "<string>");
        Ok(())
    }

    fn insert_tables(&mut self, file: TableSchemaFile, source: &str) {
        for (name, def) in file.table {
            let schema = Self::table_schema_of(&name, def, source);
            self.tables.insert(name, schema);
        }
    }

    /// A `[table.<name>]` definition as its schema.
    fn table_schema_of(name: &str, def: ComponentSchemaDefinition, source: &str) -> TableSchema {
        let mut fields = HashMap::new();
        for (field_name, field_def) in def.fields {
            if let Some(type_name) = field_def.unknown_type_name() {
                tracing::warn!(
                    "{source}: table '{name}' field '{field_name}' has unknown type \
                     \"{type_name}\"; validating it as a string"
                );
            }
            fields.insert(field_name, field_def.to_field_schema());
        }
        ComponentSchema {
            name: name.to_string(),
            description: def.description,
            fields,
        }
    }

    /// Load a component schema from a TOML file
    pub fn load_component_file<P: AsRef<Path>>(&mut self, path: P) -> Result<()> {
        let path = path.as_ref();
        let content = vfs::read_to_string(path)?;
        let file: ComponentSchemaFile =
            toml::from_str(&content).map_err(|e| parse_error(path, e))?;
        self.lints
            .extend(crate::lint::lint_component_file(&content, Some(path)));
        self.insert_components(file, &path.display().to_string())
    }

    /// A component's fields are declared under `[component.<name>.fields]`;
    /// any key under `[component.<name>]` but `description` and `fields` is
    /// an error naming the file and the keys (#792). Eleven engine files
    /// once put their fields there and loaded with none, so nothing in them
    /// was validated and no default applied.
    fn insert_components(&mut self, file: ComponentSchemaFile, source: &str) -> Result<()> {
        let mut stray: Vec<String> = file
            .component
            .iter()
            .filter(|(_, def)| !def.stray.is_empty())
            .map(|(name, def)| {
                let keys: Vec<&str> = def.stray.keys().map(String::as_str).collect();
                format!(
                    "component '{name}' has {} outside its `fields` table ({}); fields are \
                     declared under [component.{name}.fields]",
                    if keys.len() == 1 { "a key" } else { "keys" },
                    keys.join(", ")
                )
            })
            .collect();
        if !stray.is_empty() {
            stray.sort();
            return Err(FlintError::ParseError(format!(
                "{source}: {}",
                stray.join("; ")
            )));
        }
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
        Ok(())
    }

    /// Load an archetype schema from a TOML file
    pub fn load_archetype_file<P: AsRef<Path>>(&mut self, path: P) -> Result<()> {
        let path = path.as_ref();
        let content = vfs::read_to_string(path)?;
        let file: ArchetypeSchemaFile =
            toml::from_str(&content).map_err(|e| parse_error(path, e))?;

        for (name, def) in file.archetype {
            let schema = def.to_archetype_schema(name.clone());
            self.archetypes.insert(name, schema);
        }

        Ok(())
    }

    /// Load a component schema from a TOML string
    pub fn load_component_string(&mut self, content: &str) -> Result<()> {
        let file: ComponentSchemaFile = toml::from_str(content)?;
        self.lints
            .extend(crate::lint::lint_component_file(content, None));
        self.insert_components(file, "<string>")
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

    /// Register a scene-table schema directly.
    pub fn register_table(&mut self, schema: TableSchema) {
        self.tables.insert(schema.name.clone(), schema);
    }

    /// The schema of the scene-level table `name` (`post_process`, a game's
    /// `weather`), if a loaded directory declares it as `[table.<name>]`.
    pub fn get_table(&self, name: &str) -> Option<&TableSchema> {
        self.tables.get(name)
    }

    /// Every scene-table schema, by name.
    pub fn tables(&self) -> Vec<&TableSchema> {
        let mut all: Vec<&TableSchema> = self.tables.values().collect();
        all.sort_unstable_by(|a, b| a.name.cmp(&b.name));
        all
    }

    /// List all scene-table names.
    pub fn table_names(&self) -> Vec<&str> {
        self.tables.keys().map(|s| s.as_str()).collect()
    }

    /// What is wrong with the component and scene-table schema files themselves
    /// ([`crate::lint`]): an old type spelling, an unknown type, a key the
    /// loader ignores. Every file loaded, the engine's included, in load
    /// order; each names its file.
    pub fn lints(&self) -> &[Diagnostic] {
        &self.lints
    }

    /// [`lints`](Self::lints) about a game's files: those of a file outside
    /// any Flint engine checkout
    /// ([`flint_core::project::in_engine_checkout`]: the engine's
    /// repository, or the `engine/` subtree a game carries). The engine's own
    /// files are rewritten rather than warned about (ADR 0115 decision 3)
    /// and a test holds them, so a game is told only about what it can fix.
    /// What `flint validate` prints.
    pub fn game_lints(&self) -> Vec<Diagnostic> {
        self.lints
            .iter()
            .filter(|d| {
                d.file
                    .as_deref()
                    .is_none_or(|file| !flint_core::project::in_engine_checkout(file))
            })
            .cloned()
            .collect()
    }

    /// Every component schema, by name.
    pub fn components(&self) -> Vec<&ComponentSchema> {
        let mut all: Vec<&ComponentSchema> = self.components.values().collect();
        all.sort_unstable_by(|a, b| a.name.cmp(&b.name));
        all
    }

    /// Every archetype schema, by name.
    pub fn archetypes(&self) -> Vec<&ArchetypeSchema> {
        let mut all: Vec<&ArchetypeSchema> = self.archetypes.values().collect();
        all.sort_unstable_by(|a, b| a.name.cmp(&b.name));
        all
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

    /// A file's lints are kept with the file's name, and only a game's are
    /// what a game is told: a file inside an engine checkout is the
    /// engine's, however its directory was named.
    #[test]
    fn lints_are_kept_and_the_engines_are_not_the_games() {
        let old = "[component.lamp.fields]\nwatts = { type = \"float\" }\n";
        let mut files = MemorySource::new();
        files.insert("game/engine/crates/flint-core/Cargo.toml", "");
        files.insert("game/engine/schemas/components/lamp.toml", old);
        files.insert("game/schemas/components/lamp.toml", old);
        files.insert(
            "game/schemas/components/door.toml",
            // A field written beside `description` fails the load now
            // (#792); a misspelled key of a field still loads, and lints
            "[component.door.fields]\nlocked = { type = \"bool\", defualt = false }\n",
        );
        vfs::scoped(Arc::new(files), || {
            let registry =
                SchemaRegistry::load_from_directories(&["game/engine/schemas", "game/schemas"])
                    .unwrap();
            assert_eq!(registry.lints().len(), 3);
            let said: Vec<String> = registry
                .game_lints()
                .iter()
                .map(|d| d.one_line().replace('\\', "/"))
                .collect();
            assert_eq!(
                said,
                [
                    "game/schemas/components/door.toml: warning[ignored-schema-key] \
                     door.locked.defualt: `defualt` is ignored: it is no key of a field \
                     definition (did you mean `default`?)",
                    "game/schemas/components/lamp.toml: warning[old-type-name] lamp.watts: \
                     `float` is an old spelling of `f32` (write type = \"f32\"; `float` still \
                     loads)",
                ]
            );
        });
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

    /// #792: fields written under `[component.x]` instead of
    /// `[component.x.fields]` loaded as a schema with no fields. Now the
    /// file is refused, naming it, the component and the keys.
    #[test]
    fn fields_outside_the_fields_table_are_an_error() {
        let toml = r#"
[component.collider]
description = "fields in the wrong place"
shape = { type = "enum", values = ["box", "sphere"], default = "box" }
size = { type = "vec3", default = [1, 1, 1] }
"#;
        let mut registry = SchemaRegistry::new();
        let err = registry
            .load_component_string(toml)
            .unwrap_err()
            .to_string();
        assert!(err.contains("<string>"), "{err}");
        assert!(err.contains("component 'collider'"), "{err}");
        assert!(err.contains("(shape, size)"), "{err}");
        assert!(err.contains("[component.collider.fields]"), "{err}");
        assert!(registry.get_component("collider").is_none());

        // `description` beside `fields` is the shape every file has
        let good = "[component.collider]\ndescription = \"ok\"\n\n\
                    [component.collider.fields]\nshape = \"string\"\n";
        registry.load_component_string(good).unwrap();
        assert!(registry
            .get_component("collider")
            .unwrap()
            .fields
            .contains_key("shape"));
    }

    #[test]
    fn a_stray_key_fails_the_directory_load_naming_the_file() {
        let mut files = MemorySource::new();
        files.insert(
            "schemas/components/lamp.toml",
            "[component.lamp]\non = { type = \"bool\", default = true }\n",
        );
        vfs::scoped(Arc::new(files), || {
            let err = SchemaRegistry::load_from_directory("schemas")
                .unwrap_err()
                .to_string();
            assert!(err.contains("lamp.toml"), "{err}");
            assert!(
                err.contains("a key outside its `fields` table (on)"),
                "{err}"
            );
        });
    }
}
