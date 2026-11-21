//! Schema registry for loading and managing schemas

use crate::archetype::{
    ArchetypeSchema, ArchetypeSchemaDefinition, ArchetypeSchemaFile, ARCHETYPE_KEYS,
};
use crate::component::{
    ComponentSchema, ComponentSchemaDefinition, ComponentSchemaFile, FieldSchema, TableSchema,
    TableSchemaFile,
};
use crate::lint::SchemaKind;
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

/// What a message names a schema's source by: its file, or `<string>`.
fn source_of(file: Option<&Path>) -> String {
    file.map(|f| f.display().to_string())
        .unwrap_or_else(|| "<string>".into())
}

/// The `.toml` files of `dir`, none when it isn't there.
fn toml_files(dir: &Path) -> Result<Vec<PathBuf>> {
    if !vfs::exists(dir) {
        return Ok(Vec::new());
    }
    Ok(vfs::read_dir(dir)?
        .into_iter()
        .filter(|path| path.extension().is_some_and(|e| e == "toml"))
        .collect())
}

/// Where a component's or scene table's definition came from (#794): the
/// file that declared it, each file that added fields to it with
/// `extend = true`, and the file each field was declared in. What
/// `flint schema <name>` prints beside each field. A schema loaded from a
/// string or registered in code has no file, and none is recorded for it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SchemaOrigin {
    /// The file that declared it whole.
    pub declared_in: Option<PathBuf>,
    /// Each file that added fields to it, in load order.
    pub extended_by: Vec<PathBuf>,
    /// The file each field was last declared in.
    pub fields: HashMap<String, PathBuf>,
}

impl SchemaOrigin {
    /// Declared whole by `file`, with `fields`.
    fn declared<'a>(file: Option<&Path>, fields: impl Iterator<Item = &'a String>) -> Self {
        let Some(file) = file else {
            return Self::default();
        };
        Self {
            declared_in: Some(file.to_path_buf()),
            extended_by: Vec::new(),
            fields: fields.map(|f| (f.clone(), file.to_path_buf())).collect(),
        }
    }

    /// The file `field` was declared in.
    pub fn field_file(&self, field: &str) -> Option<&Path> {
        self.fields.get(field).map(PathBuf::as_path)
    }
}

/// Where an archetype came from (#1054): the file that declared it, each
/// file that added to it with `extend = true`, the file that listed each of
/// its components, and the file that gave each of its defaults. What
/// `flint schema <archetype>` prints beside each component and default. An
/// archetype loaded from a string or registered in code has no file, and
/// none is recorded for it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ArchetypeOrigin {
    /// The file that declared it whole.
    pub declared_in: Option<PathBuf>,
    /// Each file that added to it, in load order.
    pub extended_by: Vec<PathBuf>,
    /// The file that first listed each component.
    pub components: HashMap<String, PathBuf>,
    /// The file that last gave each default, by component, then field.
    pub defaults: HashMap<String, HashMap<String, PathBuf>>,
}

impl ArchetypeOrigin {
    /// Declared whole by `file`, as `schema` is.
    fn declared(file: Option<&Path>, schema: &ArchetypeSchema) -> Self {
        let Some(file) = file else {
            return Self::default();
        };
        let mut origin = Self {
            declared_in: Some(file.to_path_buf()),
            ..Self::default()
        };
        for component in &schema.components {
            origin
                .components
                .insert(component.clone(), file.to_path_buf());
        }
        for (component, table) in &schema.defaults {
            origin.set_defaults(component, table, Some(file));
        }
        origin
    }

    /// `table`'s keys, as the defaults of `component` given by `file`.
    fn set_defaults(&mut self, component: &str, table: &toml::Value, file: Option<&Path>) {
        let fields = self.defaults.entry(component.to_string()).or_default();
        for key in table.as_table().into_iter().flatten().map(|(k, _)| k) {
            match file {
                Some(file) => fields.insert(key.clone(), file.to_path_buf()),
                None => fields.remove(key),
            };
        }
    }

    /// The file that listed `component`.
    pub fn component_file(&self, component: &str) -> Option<&Path> {
        self.components.get(component).map(PathBuf::as_path)
    }

    /// The file that gave `component`'s default for `field`.
    pub fn default_file(&self, component: &str, field: &str) -> Option<&Path> {
        self.defaults
            .get(component)?
            .get(field)
            .map(PathBuf::as_path)
    }
}

/// An `[archetype.x]` with `extend = true`, waiting for the rest of its
/// schema directory to load, as a component's [`Extension`] does.
struct ArchetypeExtension {
    name: String,
    def: ArchetypeSchemaDefinition,
    file: Option<PathBuf>,
}

/// What a component's F3 panel is built from: its `[component.x.panel]`
/// table and each field's `panel` key, in the order the fields were written
/// (an extension's after the earlier layer's). Kept so an extension can
/// build the panel again with its own rows (#794).
#[derive(Debug, Clone, Default)]
struct PanelInputs {
    table: Option<toml::Value>,
    rows: Vec<(String, Option<toml::Value>)>,
}

/// A `[component.x]` or `[table.x]` with `extend = true`, waiting for the
/// rest of its schema directory to load: what it extends may be in a file
/// of the same directory read after it.
struct Extension {
    name: String,
    def: ComponentSchemaDefinition,
    file: Option<PathBuf>,
}

/// One component's or table's fields as their definitions say, each with
/// its `panel` key, in the order written. A type the parser doesn't know is
/// warned about here and checked as a string.
fn fields_of(
    kind: SchemaKind,
    name: &str,
    def_fields: indexmap::IndexMap<String, crate::component::FieldSchemaDefinition>,
    source: &str,
) -> Vec<(String, FieldSchema, Option<toml::Value>)> {
    def_fields
        .into_iter()
        .map(|(field_name, field_def)| {
            if let Some(type_name) = field_def.unknown_type_name() {
                tracing::warn!(
                    "{source}: {} '{name}' field '{field_name}' has unknown type \
                     \"{type_name}\"; validating it as a string",
                    kind.key()
                );
            }
            let panel = field_def.panel().cloned();
            (field_name, field_def.to_field_schema(), panel)
        })
        .collect()
}

/// A component's F3 panel from what it is built of. A mistake in a panel
/// is the lint's to say (`bad-panel`), with the file's name; the loader
/// leaves that row out.
fn panel_from(
    name: &str,
    inputs: &PanelInputs,
    fields: &HashMap<String, FieldSchema>,
) -> Option<crate::panel::PanelSpec> {
    let rows: Vec<_> = inputs
        .rows
        .iter()
        .filter_map(|(field, panel)| Some((field.as_str(), fields.get(field)?, panel.as_ref())))
        .collect();
    crate::panel::panel_of(name, inputs.table.as_ref(), &rows).0
}

/// Registry that holds all loaded component and archetype schemas
#[derive(Debug, Default, Clone)]
pub struct SchemaRegistry {
    components: HashMap<String, ComponentSchema>,
    archetypes: HashMap<String, ArchetypeSchema>,
    /// Scene-level tables (`[post_process]`, a game's `[weather]`), from
    /// `scene/*.toml` (#686).
    tables: HashMap<String, TableSchema>,
    /// The file of each component and of each of its fields (#794).
    component_origins: HashMap<String, SchemaOrigin>,
    /// The same of each scene table.
    table_origins: HashMap<String, SchemaOrigin>,
    /// The file of each archetype, its components and defaults (#1054).
    archetype_origins: HashMap<String, ArchetypeOrigin>,
    /// What each component's panel is built from, for an extension.
    panel_inputs: HashMap<String, PanelInputs>,
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
    /// Later directories override earlier ones (game overrides engine): a
    /// component or table of the same name is replaced whole, unless the
    /// later one says `extend = true`, which adds its fields to it (#794).
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
        registry.load_directory(path)?;
        Ok(registry)
    }

    /// Load schemas from a single directory into this registry (additive/override).
    /// A file's `extend = true` components and tables are applied after
    /// every file of the directory has loaded, so what one extends may be
    /// declared by an earlier directory or by any file of this one.
    pub fn load_directory<P: AsRef<Path>>(&mut self, path: P) -> Result<()> {
        let path = path.as_ref();

        // Scene tables, then their extensions
        let mut extensions = Vec::new();
        for file_path in toml_files(&path.join(SchemaKind::Table.dir()))? {
            extensions.extend(self.read_table_file(&file_path)?);
        }
        self.apply_extensions(SchemaKind::Table, extensions)?;

        // Components, then their extensions
        let mut extensions = Vec::new();
        for file_path in toml_files(&path.join(SchemaKind::Component.dir()))? {
            extensions.extend(self.read_component_file(&file_path)?);
        }
        self.apply_extensions(SchemaKind::Component, extensions)?;

        // Archetypes, then their extensions (#1054)
        let mut extensions = Vec::new();
        for file_path in toml_files(&path.join("archetypes"))? {
            extensions.extend(self.read_archetype_file(&file_path)?);
        }
        self.apply_archetype_extensions(extensions)
    }

    /// Load the scene-table schemas of a TOML file (`[table.<name>]`).
    pub fn load_table_file<P: AsRef<Path>>(&mut self, path: P) -> Result<()> {
        let extensions = self.read_table_file(path.as_ref())?;
        self.apply_extensions(SchemaKind::Table, extensions)
    }

    fn read_table_file(&mut self, path: &Path) -> Result<Vec<Extension>> {
        let content = vfs::read_to_string(path)?;
        let file: TableSchemaFile = toml::from_str(&content).map_err(|e| parse_error(path, e))?;
        self.lints
            .extend(crate::lint::lint_table_file(&content, Some(path)));
        Ok(self.insert_tables(file, Some(path)))
    }

    /// Load scene-table schemas from a TOML string.
    pub fn load_table_string(&mut self, content: &str) -> Result<()> {
        let file: TableSchemaFile = toml::from_str(content)?;
        self.lints
            .extend(crate::lint::lint_table_file(content, None));
        let extensions = self.insert_tables(file, None);
        self.apply_extensions(SchemaKind::Table, extensions)
    }

    /// Each table of `file` replaces the one of its name; those that say
    /// `extend = true` are returned, to be applied by
    /// [`apply_extensions`](Self::apply_extensions).
    fn insert_tables(&mut self, file: TableSchemaFile, path: Option<&Path>) -> Vec<Extension> {
        let source = source_of(path);
        let mut extensions = Vec::new();
        for (name, def) in file.table {
            if def.extend {
                extensions.push(Extension {
                    name,
                    def,
                    file: path.map(Path::to_path_buf),
                });
                continue;
            }
            let mut fields: HashMap<String, FieldSchema> =
                fields_of(SchemaKind::Table, &name, def.fields, &source)
                    .into_iter()
                    .map(|(field, schema, _)| (field, schema))
                    .collect();
            self.table_origins
                .insert(name.clone(), SchemaOrigin::declared(path, fields.keys()));
            // The built-in post effects' keys come from their descriptors
            // (#741), not the file: a key the file declares is the file's
            if name == "post_process" {
                for (key, field) in crate::post_effects::field_schemas() {
                    fields.entry(key.to_string()).or_insert(field);
                }
            }
            let schema = ComponentSchema {
                name: name.clone(),
                description: def.description,
                fields,
                // A scene table is no entity's: it has no F3 panel (the lint
                // says so of a `panel` key)
                panel: None,
            };
            self.tables.insert(name, schema);
        }
        extensions
    }

    /// Load a component schema from a TOML file
    pub fn load_component_file<P: AsRef<Path>>(&mut self, path: P) -> Result<()> {
        let extensions = self.read_component_file(path.as_ref())?;
        self.apply_extensions(SchemaKind::Component, extensions)
    }

    fn read_component_file(&mut self, path: &Path) -> Result<Vec<Extension>> {
        let content = vfs::read_to_string(path)?;
        let file: ComponentSchemaFile =
            toml::from_str(&content).map_err(|e| parse_error(path, e))?;
        self.lints
            .extend(crate::lint::lint_component_file(&content, Some(path)));
        self.insert_components(file, Some(path))
    }

    /// A component's fields are declared under `[component.<name>.fields]`;
    /// any key under `[component.<name>]` but `description`, `fields`,
    /// `panel` and `extend` is an error naming the file and the keys (#792).
    /// Eleven engine files once put their fields there and loaded with none,
    /// so nothing in them was validated and no default applied.
    ///
    /// Each component replaces the one of its name; those that say
    /// `extend = true` are returned, to be applied by
    /// [`apply_extensions`](Self::apply_extensions).
    fn insert_components(
        &mut self,
        file: ComponentSchemaFile,
        path: Option<&Path>,
    ) -> Result<Vec<Extension>> {
        let source = source_of(path);
        let mut stray: Vec<String> = file
            .component
            .iter()
            .filter(|(_, def)| !def.stray.is_empty())
            .map(|(name, def)| {
                let keys: Vec<&str> = def.stray.keys().map(String::as_str).collect();
                let mut message = format!(
                    "component '{name}' has {} outside its `fields` table ({}); fields are \
                     declared under [component.{name}.fields]",
                    if keys.len() == 1 { "a key" } else { "keys" },
                    keys.join(", ")
                );
                // `extned = true` is a misspelt key, not a misplaced field
                for key in &keys {
                    if let Some(near) = flint_core::diagnostic::did_you_mean(
                        key,
                        crate::component::COMPONENT_KEYS.iter().copied(),
                    ) {
                        message.push_str(&format!(" (`{key}`: {near})"));
                    }
                }
                message
            })
            .collect();
        if !stray.is_empty() {
            stray.sort();
            return Err(FlintError::ParseError(format!(
                "{source}: {}",
                stray.join("; ")
            )));
        }
        let mut extensions = Vec::new();
        for (name, def) in file.component {
            if def.extend {
                extensions.push(Extension {
                    name,
                    def,
                    file: path.map(Path::to_path_buf),
                });
                continue;
            }
            let written = fields_of(SchemaKind::Component, &name, def.fields, &source);
            let mut inputs = PanelInputs {
                table: def.panel,
                rows: Vec::new(),
            };
            let mut fields = HashMap::new();
            for (field, schema, panel) in written {
                inputs.rows.push((field.clone(), panel));
                fields.insert(field, schema);
            }
            let panel = panel_from(&name, &inputs, &fields);
            self.component_origins
                .insert(name.clone(), SchemaOrigin::declared(path, fields.keys()));
            self.panel_inputs.insert(name.clone(), inputs);
            let schema = ComponentSchema {
                name: name.clone(),
                description: def.description,
                fields,
                panel,
            };
            self.components.insert(name, schema);
        }
        Ok(extensions)
    }

    /// Add each extension's fields to the component or table it names
    /// (#794). A field the earlier layer declared too is redefined by the
    /// extension; the earlier layer's description stays (the lint says so of
    /// an extension's own). An extension of a name no earlier schema
    /// declares is an error naming the file: it is most likely a misspelt
    /// name, and its fields would check nothing.
    fn apply_extensions(&mut self, kind: SchemaKind, extensions: Vec<Extension>) -> Result<()> {
        for Extension { name, def, file } in extensions {
            let source = source_of(file.as_deref());
            let (schemas, origins) = match kind {
                SchemaKind::Component => (&mut self.components, &mut self.component_origins),
                SchemaKind::Table => (&mut self.tables, &mut self.table_origins),
            };
            let Some(schema) = schemas.get_mut(&name) else {
                let word = kind.key();
                let mut message = format!(
                    "{source}: {word} '{name}' says `extend = true`, and no earlier schema \
                     declares a {word} of that name to add its fields to"
                );
                let known = schemas.keys().map(String::as_str);
                match flint_core::diagnostic::did_you_mean(&name, known) {
                    Some(near) => message.push_str(&format!(": {near}")),
                    None => message.push_str(&format!(
                        " (to declare it whole here, leave `extend` out of [{word}.{name}])"
                    )),
                }
                return Err(FlintError::ParseError(message));
            };
            let origin = origins.entry(name.clone()).or_default();
            if let Some(file) = &file {
                origin.extended_by.push(file.clone());
            }
            // A scene table has no panel, and keeps no inputs for one
            let mut no_panel = PanelInputs::default();
            let inputs = match kind {
                SchemaKind::Component => self.panel_inputs.entry(name.clone()).or_default(),
                SchemaKind::Table => &mut no_panel,
            };
            if def.panel.is_some() {
                inputs.table = def.panel;
            }
            for (field, field_schema, panel) in fields_of(kind, &name, def.fields, &source) {
                match &file {
                    Some(file) => origin.fields.insert(field.clone(), file.clone()),
                    None => origin.fields.remove(&field),
                };
                match inputs.rows.iter_mut().find(|(f, _)| *f == field) {
                    Some(row) => row.1 = panel,
                    None => inputs.rows.push((field.clone(), panel)),
                }
                schema.fields.insert(field, field_schema);
            }
            if kind == SchemaKind::Component {
                schema.panel = panel_from(&name, inputs, &schema.fields);
            }
        }
        Ok(())
    }

    /// Where component `name` and each of its fields was declared (#794).
    pub fn component_origin(&self, name: &str) -> Option<&SchemaOrigin> {
        self.component_origins.get(name)
    }

    /// Where scene table `name` and each of its fields was declared.
    pub fn table_origin(&self, name: &str) -> Option<&SchemaOrigin> {
        self.table_origins.get(name)
    }

    /// Where archetype `name`, each of its components and each of its
    /// defaults was declared (#1054).
    pub fn archetype_origin(&self, name: &str) -> Option<&ArchetypeOrigin> {
        self.archetype_origins.get(name)
    }

    /// Load an archetype schema from a TOML file
    pub fn load_archetype_file<P: AsRef<Path>>(&mut self, path: P) -> Result<()> {
        let extensions = self.read_archetype_file(path.as_ref())?;
        self.apply_archetype_extensions(extensions)
    }

    fn read_archetype_file(&mut self, path: &Path) -> Result<Vec<ArchetypeExtension>> {
        let content = vfs::read_to_string(path)?;
        let file: ArchetypeSchemaFile =
            toml::from_str(&content).map_err(|e| parse_error(path, e))?;
        Ok(self.insert_archetypes(file, Some(path)))
    }

    /// Each archetype of `file` replaces the one of its name; those that
    /// say `extend = true` are returned, to be applied by
    /// [`apply_archetype_extensions`](Self::apply_archetype_extensions). A
    /// key the loader ignores is linted here: archetype files have no lint
    /// pass of their own.
    fn insert_archetypes(
        &mut self,
        file: ArchetypeSchemaFile,
        path: Option<&Path>,
    ) -> Vec<ArchetypeExtension> {
        let mut names: Vec<&String> = file.archetype.keys().collect();
        names.sort();
        for name in names {
            self.lint_archetype(name, &file.archetype[name], path);
        }
        let mut extensions = Vec::new();
        for (name, def) in file.archetype {
            if def.extend {
                extensions.push(ArchetypeExtension {
                    name,
                    def,
                    file: path.map(Path::to_path_buf),
                });
                continue;
            }
            let schema = def.to_archetype_schema(name.clone());
            self.archetype_origins
                .insert(name.clone(), ArchetypeOrigin::declared(path, &schema));
            self.archetypes.insert(name, schema);
        }
        extensions
    }

    /// `ignored-schema-key` for an archetype's stray keys, and for an
    /// extension's `description`, which stays the declaring layer's.
    fn lint_archetype(&mut self, name: &str, def: &ArchetypeSchemaDefinition, path: Option<&Path>) {
        use flint_core::diagnostic::codes;
        let mut found = Vec::new();
        if def.extend && def.description.is_some() {
            found.push(
                Diagnostic::warning(
                    codes::IGNORED_SCHEMA_KEY,
                    format!(
                        "`description` is ignored: `[archetype.{name}]` says `extend = true`, \
                         so it adds to the `{name}` an earlier layer declared, which keeps that \
                         layer's description"
                    ),
                )
                .field("description"),
            );
        }
        for key in def.stray.keys() {
            found.push(
                Diagnostic::warning(
                    codes::IGNORED_SCHEMA_KEY,
                    format!(
                        "`{key}` is ignored: `[archetype.{name}]` holds `description`, \
                         `components`, `defaults`, `visual` and `extend`"
                    ),
                )
                .hint_opt(flint_core::diagnostic::did_you_mean(
                    key,
                    ARCHETYPE_KEYS.iter().copied(),
                ))
                .field(key.clone()),
            );
        }
        for d in &mut found {
            d.file = path.map(Path::to_path_buf);
        }
        self.lints.extend(found);
    }

    /// Add each extension's components and defaults to the archetype it
    /// names (#1054). A component already listed is not listed twice; each
    /// `defaults.<component>` table merges into the earlier one key by key,
    /// a key named again taking the extension's value; a `visual` replaces
    /// the earlier one. An extension of a name no earlier schema declares
    /// is an error naming the file, as a component's is (#794).
    fn apply_archetype_extensions(&mut self, extensions: Vec<ArchetypeExtension>) -> Result<()> {
        for ArchetypeExtension { name, def, file } in extensions {
            let Some(schema) = self.archetypes.get_mut(&name) else {
                let source = source_of(file.as_deref());
                let mut message = format!(
                    "{source}: archetype '{name}' says `extend = true`, and no earlier schema \
                     declares an archetype of that name to add its components to"
                );
                let known = self.archetypes.keys().map(String::as_str);
                match flint_core::diagnostic::did_you_mean(&name, known) {
                    Some(near) => message.push_str(&format!(": {near}")),
                    None => message.push_str(&format!(
                        " (to declare it whole here, leave `extend` out of [archetype.{name}])"
                    )),
                }
                return Err(FlintError::ParseError(message));
            };
            let origin = self.archetype_origins.entry(name.clone()).or_default();
            if let Some(file) = &file {
                origin.extended_by.push(file.clone());
            }
            for component in def.components {
                if schema.has_component(&component) {
                    continue;
                }
                if let Some(file) = &file {
                    origin.components.insert(component.clone(), file.clone());
                }
                schema.components.push(component);
            }
            let mut given: Vec<_> = def.defaults.into_iter().collect();
            given.sort_by(|a, b| a.0.cmp(&b.0));
            for (component, table) in given {
                origin.set_defaults(&component, &table, file.as_deref());
                match (schema.defaults.get_mut(&component), table) {
                    (Some(toml::Value::Table(earlier)), toml::Value::Table(added)) => {
                        earlier.extend(added);
                    }
                    (_, table) => {
                        schema.defaults.insert(component, table);
                    }
                }
            }
            if def.visual.is_some() {
                schema.visual = def.visual;
            }
        }
        Ok(())
    }

    /// Load a component schema from a TOML string
    pub fn load_component_string(&mut self, content: &str) -> Result<()> {
        let file: ComponentSchemaFile = toml::from_str(content)?;
        self.lints
            .extend(crate::lint::lint_component_file(content, None));
        let extensions = self.insert_components(file, None)?;
        self.apply_extensions(SchemaKind::Component, extensions)
    }

    /// Load an archetype schema from a TOML string
    pub fn load_archetype_string(&mut self, content: &str) -> Result<()> {
        let file: ArchetypeSchemaFile = toml::from_str(content)?;
        let extensions = self.insert_archetypes(file, None);
        self.apply_archetype_extensions(extensions)
    }

    /// Register a component schema directly
    pub fn register_component(&mut self, schema: ComponentSchema) {
        // Built in code: no file, and its panel is the one it carries
        self.component_origins.remove(&schema.name);
        self.panel_inputs.remove(&schema.name);
        self.components.insert(schema.name.clone(), schema);
    }

    /// Register an archetype schema directly
    pub fn register_archetype(&mut self, schema: ArchetypeSchema) {
        self.archetype_origins.remove(&schema.name);
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
        self.table_origins.remove(&schema.name);
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
    /// loader ignores (an archetype file's too, #1054). Every file loaded, the engine's included, in load
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
    /// What `flint validate` prints, and the player says once (#1064).
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

    const ENGINE_MATERIAL: &str = "[component.material]\n\
        description = \"The engine's material\"\n\n\
        [component.material.fields]\n\
        roughness = { type = \"f32\", default = 0.5, min = 0.0, max = 1.0, panel = \"tune\" }\n\
        metallic = { type = \"f32\", default = 0.0 }\n";

    const WETNESS: &str = "[component.material]\nextend = true\n\n\
        [component.material.fields]\n\
        wetness = { type = \"f32\", default = 0.0, panel = \"tune\" }\n";

    fn game_over_engine(game_files: &[(&str, &str)]) -> Result<SchemaRegistry> {
        let mut files = MemorySource::new();
        files.insert("engine/schemas/components/material.toml", ENGINE_MATERIAL);
        for (path, text) in game_files {
            files.insert(*path, *text);
        }
        vfs::scoped(Arc::new(files), || {
            SchemaRegistry::load_from_directories(&["engine/schemas", "schemas"])
        })
    }

    fn shown(path: Option<&Path>) -> Option<String> {
        path.map(flint_core::project::shown)
    }

    /// #794: a game's file of a few lines adds one field to an engine
    /// component, keeping the engine's, and says where each came from.
    #[test]
    fn a_later_layer_extends_a_component_with_one_field() {
        let registry = game_over_engine(&[("schemas/components/material.toml", WETNESS)]).unwrap();
        let material = registry.get_component("material").unwrap();
        let mut names = material.field_names();
        names.sort_unstable();
        assert_eq!(names, ["metallic", "roughness", "wetness"]);
        assert_eq!(
            material.fields["roughness"].max,
            Some(1.0),
            "the engine's field, whole"
        );
        assert_eq!(
            material.description.as_deref(),
            Some("The engine's material")
        );

        let origin = registry.component_origin("material").unwrap();
        assert_eq!(
            shown(origin.declared_in.as_deref()).as_deref(),
            Some("engine/schemas/components/material.toml")
        );
        let extended: Vec<String> = origin
            .extended_by
            .iter()
            .map(|p| flint_core::project::shown(p))
            .collect();
        assert_eq!(extended, ["schemas/components/material.toml"]);
        assert_eq!(
            shown(origin.field_file("roughness")).as_deref(),
            Some("engine/schemas/components/material.toml")
        );
        assert_eq!(
            shown(origin.field_file("wetness")).as_deref(),
            Some("schemas/components/material.toml")
        );

        // The panel has the engine's row and the game's, in that order
        let panel = material.panel.as_ref().unwrap();
        let rows: Vec<&str> = panel.rows.iter().map(|r| r.field.as_str()).collect();
        assert_eq!(rows, ["roughness", "wetness"]);

        // ...and a scene using the field passes with nothing said
        let data: toml::Value = toml::from_str("roughness = 0.2\nwetness = 0.8\n").unwrap();
        assert_eq!(crate::validate_component(material, &data), []);
    }

    /// Without `extend`, a later layer still replaces a component whole:
    /// what every overriding layer written before #794 means.
    #[test]
    fn without_extend_a_later_layer_replaces_the_component_whole() {
        let registry = game_over_engine(&[(
            "schemas/components/material.toml",
            "[component.material.fields]\nwetness = \"f32\"\n",
        )])
        .unwrap();
        let material = registry.get_component("material").unwrap();
        assert_eq!(material.field_names(), ["wetness"]);
        assert_eq!(material.description, None);
        assert!(material.panel.is_none());
        let origin = registry.component_origin("material").unwrap();
        assert_eq!(
            shown(origin.declared_in.as_deref()).as_deref(),
            Some("schemas/components/material.toml")
        );
        assert!(origin.extended_by.is_empty());
    }

    /// A field the earlier layer declared too takes the extension's
    /// definition, and is the extension's.
    #[test]
    fn an_extension_redefines_a_field_it_names_again() {
        let registry = game_over_engine(&[(
            "schemas/components/material.toml",
            "[component.material]\nextend = true\n\n\
             [component.material.fields]\nroughness = { type = \"f32\", max = 0.5 }\n",
        )])
        .unwrap();
        let material = registry.get_component("material").unwrap();
        assert_eq!(material.fields["roughness"].max, Some(0.5));
        assert_eq!(
            material.fields["metallic"].default,
            Some(toml::Value::Float(0.0))
        );
        assert_eq!(
            shown(
                registry
                    .component_origin("material")
                    .unwrap()
                    .field_file("roughness")
            )
            .as_deref(),
            Some("schemas/components/material.toml")
        );
        // It gave the field no `panel`, so the row is gone with the old definition
        assert!(material.panel.is_none());
    }

    /// The extensions of a directory apply after all of its files: one may
    /// extend a component a file of the same directory, read after it,
    /// declares.
    #[test]
    fn an_extension_waits_for_the_rest_of_its_directory() {
        let mut files = MemorySource::new();
        files.insert("schemas/components/a_wet.toml", WETNESS);
        files.insert("schemas/components/material.toml", ENGINE_MATERIAL);
        vfs::scoped(Arc::new(files), || {
            let registry = SchemaRegistry::load_from_directory("schemas").unwrap();
            assert_eq!(registry.get_component("material").unwrap().fields.len(), 3);
        });
    }

    /// An extension of a component no earlier layer declares is an error
    /// naming the file and the nearest name: its fields would check nothing.
    #[test]
    fn extending_a_component_nobody_declared_is_an_error() {
        let err = game_over_engine(&[(
            "schemas/components/material.toml",
            &WETNESS.replace("material", "materail"),
        )])
        .unwrap_err()
        .to_string()
        .replace('\\', "/");
        assert_eq!(
            err,
            "Parse error: schemas/components/material.toml: component 'materail' says \
             `extend = true`, and no earlier schema declares a component of that name to add \
             its fields to: did you mean `material`?"
        );

        // A misspelt `extend` is named as one
        let err = game_over_engine(&[(
            "schemas/components/material.toml",
            &WETNESS.replace("extend", "extned"),
        )])
        .unwrap_err()
        .to_string();
        assert!(err.contains("(`extned`: did you mean `extend`?)"), "{err}");
    }

    /// A scene table extends the same way: a game adds a key to the
    /// engine's `[post_process]`, whose own keys stay.
    #[test]
    fn a_scene_table_extends_too() {
        let mut files = MemorySource::new();
        files.insert(
            "engine/schemas/scene/post_process.toml",
            "[table.post_process]\ndescription = \"engine\"\n\n\
             [table.post_process.fields]\nexposure = \"f32\"\n",
        );
        files.insert(
            "schemas/scene/post_process.toml",
            "[table.post_process]\nextend = true\n\n\
             [table.post_process.fields]\nfilm_grain_seed = \"i32\"\n",
        );
        vfs::scoped(Arc::new(files), || {
            let registry =
                SchemaRegistry::load_from_directories(&["engine/schemas", "schemas"]).unwrap();
            let table = registry.get_table("post_process").unwrap();
            assert!(table.fields.contains_key("exposure"));
            assert!(table.fields.contains_key("film_grain_seed"));
            assert_eq!(table.description.as_deref(), Some("engine"));
            let origin = registry.table_origin("post_process").unwrap();
            assert_eq!(
                shown(origin.field_file("film_grain_seed")).as_deref(),
                Some("schemas/scene/post_process.toml")
            );
            assert!(registry.get_component("post_process").is_none());
        });
    }

    const ENGINE_LANTERN: &str = "[archetype.lantern]\n\
        description = \"The engine's lantern\"\n\
        components = [\"transform\", \"light\"]\n\n\
        [archetype.lantern.defaults.light]\n\
        type = \"point\"\nintensity = 2.0\nrange = 8.0\n\n\
        [archetype.lantern.visual]\ncolor = [1.0, 0.8, 0.3]\n";

    const FLICKER: &str = "[archetype.lantern]\nextend = true\n\
        components = [\"light\", \"flicker\"]\n\n\
        [archetype.lantern.defaults.light]\nintensity = 1.5\n\n\
        [archetype.lantern.defaults.flicker]\nrate = 6.0\n";

    fn archetypes_over_engine(game_files: &[(&str, &str)]) -> Result<SchemaRegistry> {
        let mut files = MemorySource::new();
        files.insert("engine/schemas/archetypes/lantern.toml", ENGINE_LANTERN);
        for (path, text) in game_files {
            files.insert(*path, *text);
        }
        vfs::scoped(Arc::new(files), || {
            SchemaRegistry::load_from_directories(&["engine/schemas", "schemas"])
        })
    }

    fn default_of<'a>(
        archetype: &'a ArchetypeSchema,
        component: &str,
        key: &str,
    ) -> Option<&'a toml::Value> {
        archetype.get_default(component, key)
    }

    /// #1054: a game's file of a few lines adds a component and its
    /// defaults to an engine archetype, keeping the engine's, and says
    /// where each came from.
    #[test]
    fn a_later_layer_extends_an_archetype_with_a_component() {
        let registry =
            archetypes_over_engine(&[("schemas/archetypes/lantern.toml", FLICKER)]).unwrap();
        let lantern = registry.get_archetype("lantern").unwrap();
        assert_eq!(
            lantern.components,
            ["transform", "light", "flicker"],
            "the engine's components in its order, the game's after, none twice"
        );
        assert_eq!(
            default_of(lantern, "flicker", "rate"),
            Some(&toml::Value::Float(6.0))
        );
        // The light's defaults merge key by key: the game's intensity, the
        // engine's type and range
        assert_eq!(
            default_of(lantern, "light", "intensity"),
            Some(&toml::Value::Float(1.5))
        );
        assert_eq!(
            default_of(lantern, "light", "type"),
            Some(&toml::Value::String("point".into()))
        );
        assert_eq!(
            default_of(lantern, "light", "range"),
            Some(&toml::Value::Float(8.0))
        );
        assert_eq!(lantern.description.as_deref(), Some("The engine's lantern"));
        assert_eq!(
            lantern.visual.unwrap().color,
            [1.0, 0.8, 0.3, 1.0],
            "no visual given: the engine's stays"
        );

        let origin = registry.archetype_origin("lantern").unwrap();
        let engine = Some("engine/schemas/archetypes/lantern.toml".to_string());
        let game = Some("schemas/archetypes/lantern.toml".to_string());
        assert_eq!(shown(origin.declared_in.as_deref()), engine);
        assert_eq!(
            origin
                .extended_by
                .iter()
                .map(|p| flint_core::project::shown(p))
                .collect::<Vec<_>>(),
            ["schemas/archetypes/lantern.toml"]
        );
        assert_eq!(shown(origin.component_file("transform")), engine);
        assert_eq!(
            shown(origin.component_file("light")),
            engine,
            "listed again, still the engine's"
        );
        assert_eq!(shown(origin.component_file("flicker")), game);
        assert_eq!(shown(origin.default_file("light", "intensity")), game);
        assert_eq!(shown(origin.default_file("light", "range")), engine);
        assert_eq!(shown(origin.default_file("flicker", "rate")), game);
        assert!(registry.lints().is_empty(), "{:?}", registry.lints());
    }

    /// Without `extend`, a later layer still replaces an archetype whole:
    /// what every overriding archetype file written before #1054 means.
    #[test]
    fn without_extend_a_later_layer_replaces_the_archetype_whole() {
        let registry = archetypes_over_engine(&[(
            "schemas/archetypes/lantern.toml",
            &FLICKER.replace("extend = true\n", ""),
        )])
        .unwrap();
        let lantern = registry.get_archetype("lantern").unwrap();
        assert_eq!(lantern.components, ["light", "flicker"]);
        assert_eq!(default_of(lantern, "light", "type"), None);
        assert_eq!(lantern.description, None);
        assert!(lantern.visual.is_none());
        let origin = registry.archetype_origin("lantern").unwrap();
        assert_eq!(
            shown(origin.declared_in.as_deref()).as_deref(),
            Some("schemas/archetypes/lantern.toml")
        );
        assert!(origin.extended_by.is_empty());
    }

    /// An extension's `visual` replaces the earlier one; a default that
    /// isn't a table replaces the earlier one whole.
    #[test]
    fn an_archetype_extension_replaces_the_visual_it_gives() {
        let registry = archetypes_over_engine(&[(
            "schemas/archetypes/lantern.toml",
            "[archetype.lantern]\nextend = true\n\n\
             [archetype.lantern.visual]\ncolor = [0.2, 0.4, 1.0]\nwireframe = true\n",
        )])
        .unwrap();
        let lantern = registry.get_archetype("lantern").unwrap();
        let visual = lantern.visual.unwrap();
        assert_eq!(visual.color, [0.2, 0.4, 1.0, 1.0]);
        assert!(visual.wireframe);
        assert_eq!(lantern.components, ["transform", "light"]);
    }

    /// A directory's archetype extensions apply after all of its files.
    #[test]
    fn an_archetype_extension_waits_for_the_rest_of_its_directory() {
        let mut files = MemorySource::new();
        files.insert("schemas/archetypes/a_flicker.toml", FLICKER);
        files.insert("schemas/archetypes/lantern.toml", ENGINE_LANTERN);
        vfs::scoped(Arc::new(files), || {
            let registry = SchemaRegistry::load_from_directory("schemas").unwrap();
            assert_eq!(
                registry.get_archetype("lantern").unwrap().components,
                ["transform", "light", "flicker"]
            );
        });
    }

    /// An extension of an archetype no earlier layer declares is an error
    /// naming the file and the nearest name.
    #[test]
    fn extending_an_archetype_nobody_declared_is_an_error() {
        let err = archetypes_over_engine(&[(
            "schemas/archetypes/lantern.toml",
            &FLICKER.replace("lantern", "lanturn"),
        )])
        .unwrap_err()
        .to_string()
        .replace('\\', "/");
        assert_eq!(
            err,
            "Parse error: schemas/archetypes/lantern.toml: archetype 'lanturn' says \
             `extend = true`, and no earlier schema declares an archetype of that name to add \
             its components to: did you mean `lantern`?"
        );
    }

    /// An extension's `description` is ignored and said to be; a misspelt
    /// `extend`, which would replace the archetype whole, is named.
    #[test]
    fn an_archetypes_ignored_keys_are_linted() {
        let registry = archetypes_over_engine(&[(
            "schemas/archetypes/lantern.toml",
            &FLICKER.replace("extend = true\n", "extend = true\ndescription = \"Mine\"\n"),
        )])
        .unwrap();
        assert_eq!(
            registry
                .get_archetype("lantern")
                .unwrap()
                .description
                .as_deref(),
            Some("The engine's lantern")
        );
        let lints: Vec<String> = registry.lints().iter().map(|d| d.to_string()).collect();
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert!(
            lints[0]
                .contains("`description` is ignored: `[archetype.lantern]` says `extend = true`"),
            "{lints:?}"
        );

        let registry = archetypes_over_engine(&[(
            "schemas/archetypes/lantern.toml",
            &FLICKER.replace("extend", "extned"),
        )])
        .unwrap();
        let lints: Vec<String> = registry.lints().iter().map(|d| d.one_line()).collect();
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert!(lints[0].contains("`extned` is ignored"), "{lints:?}");
        assert!(lints[0].contains("did you mean `extend`?"), "{lints:?}");
        assert!(lints[0].contains("lantern.toml"), "{lints:?}");
    }

    /// An extension's `description` is not the component's, and the lint
    /// says so.
    #[test]
    fn an_extensions_description_is_reported_as_ignored() {
        let with_description =
            WETNESS.replace("extend = true\n", "extend = true\ndescription = \"Wet\"\n");
        let registry =
            game_over_engine(&[("schemas/components/material.toml", &with_description)]).unwrap();
        assert_eq!(
            registry
                .get_component("material")
                .unwrap()
                .description
                .as_deref(),
            Some("The engine's material")
        );
        let lints: Vec<String> = registry.lints().iter().map(|d| d.to_string()).collect();
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert!(
            lints[0]
                .contains("`description` is ignored: `[component.material]` says `extend = true`"),
            "{lints:?}"
        );
    }
}
