//! What a Flint project is and where its root lies (ADR 0114, #706).
//!
//! [`Project::discover`] finds the project a scene or a folder belongs to the
//! way cargo finds a workspace: it walks up to the nearest directory carrying
//! a project marker, then reads that directory's manifest, `flint.toml`, if
//! it has one. Every read goes through the [`vfs`], so the player and the
//! web build see the same project a command does.
//!
//! A directory is a project root when it holds, in order of strength:
//!
//! 1. a `flint.toml` file, the manifest;
//! 2. a `.flint/` directory, except in the home directory, where
//!    `~/.flint/config.toml` is the *global* config and says nothing about
//!    which project you are in;
//! 3. the layout `flint init` makes and the game repos keep: `schemas/`
//!    beside `levels/` or `scenes/`.
//!
//! A Flint engine checkout (`crates/flint-core/Cargo.toml`) is passed through
//! when there is a project above it, so a path inside a game's `engine/`
//! subtree lands in the game. With nothing above, the engine checkout is its
//! own project. With no marker anywhere, the starting directory is the root,
//! which is what every command did before there were markers.
//!
//! # The manifest
//!
//! ```toml
//! [project]
//! id = "float"                              # saves, web bundle id, user files
//! schemas = ["engine/schemas", "schemas"]   # in override order, later wins
//! scene = "scenes/main.scene.toml"          # the scene to open by default
//! input = "config/input.toml"
//! save_version = 2                          # the version saves are written at
//! ```
//!
//! Every key is optional and so is the table: an empty `flint.toml` is a
//! marker and nothing more. Paths are relative to the root. A key of the
//! wrong type, or a key in `[project]` this version doesn't know, is a
//! [`ProjectError`] naming the file and the key, so a typo is never silently
//! ignored. Tables other than `[project]` are left alone.
//!
//! ```
//! use flint_core::project::{Found, Project};
//! use flint_core::vfs::{self, MemorySource};
//! use std::path::PathBuf;
//! use std::sync::Arc;
//!
//! let mut files = MemorySource::new();
//! files.insert("flint.toml", "[project]\nid = \"float\"\nschemas = [\"engine/schemas\", \"schemas\"]\n");
//! files.insert("scenes/deep/a.scene.toml", "");
//! vfs::scoped(Arc::new(files), || {
//!     let project = Project::discover("scenes/deep/a.scene.toml".as_ref()).unwrap();
//!     assert_eq!(project.found, Found::Manifest);
//!     assert_eq!(project.id.as_deref(), Some("float"));
//!     assert_eq!(project.schemas, [PathBuf::from("engine/schemas"), PathBuf::from("schemas")]);
//! });
//! ```
//!
//! # Relative paths
//!
//! A relative start is taken from the working directory, as a command's
//! arguments are. When the working directory is not in the file source at
//! all (a bundle in memory, the web), relative paths are the source's own and
//! are walked as given, up to the source's root.

use crate::vfs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use thiserror::Error;

mod assets;
pub use assets::{
    shown, AssetKind, AssetScope, KindInfo, Listed, NotFound, OrderChange, Resolution,
};
mod root;
pub use root::guessed_root;

/// The manifest's file name, at the project root.
pub const MANIFEST_FILE: &str = "flint.toml";

/// The keys `[project]` may hold.
const PROJECT_KEYS: [&str; 5] = ["id", "schemas", "scene", "input", "save_version"];

/// Why a directory was taken as the project root.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Found {
    /// `--project <dir>` (or [`Project::at`]) named it.
    Override,
    /// A `flint.toml` file.
    Manifest,
    /// A `.flint/` directory.
    StateDir,
    /// `schemas/` beside `levels/` or `scenes/`.
    Layout,
    /// A Flint engine checkout with no project above it.
    Engine,
    /// No marker anywhere above: the starting directory.
    Fallback,
}

impl Found {
    pub fn describe(self) -> &'static str {
        match self {
            Found::Override => "--project",
            Found::Manifest => "flint.toml",
            Found::StateDir => ".flint/",
            Found::Layout => "schemas/ + scenes/ or levels/",
            Found::Engine => "engine checkout",
            Found::Fallback => "no project marker found; using the working directory",
        }
    }
}

/// A project that could not be read.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ProjectError {
    /// The directory named as the root isn't one.
    #[error("{}: not a directory", dir.display())]
    NotADirectory { dir: PathBuf },

    /// The manifest is there and could not be read.
    #[error("{}: {message}", file.display())]
    Read { file: PathBuf, message: String },

    /// The manifest is not TOML.
    #[error("{}: {message}", file.display())]
    Syntax { file: PathBuf, message: String },

    /// A key holds the wrong kind of value.
    #[error("{}: `{key}` must be {expected}, found {got}", file.display())]
    WrongType {
        file: PathBuf,
        /// Dotted, as written: `project.schemas`, `project.schemas[1]`.
        key: String,
        expected: &'static str,
        got: &'static str,
    },

    /// `[project]` holds a key this version doesn't know.
    #[error(
        "{}: unknown key `{key}` in [project] (the keys are id, schemas, scene, input, save_version)",
        file.display()
    )]
    UnknownKey { file: PathBuf, key: String },
}

/// A located project: its root, how it was found, and what its manifest says.
///
/// Made by [`Project::discover`], [`Project::for_path`], [`Project::current`]
/// or [`Project::at`]; more fields will come (engine mounts), so it can't be
/// built field by field outside this crate.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Project {
    pub root: PathBuf,
    pub found: Found,
    /// `[project] id`: what saves, the web bundle and user files are keyed
    /// on. `None` when the manifest doesn't say (or there is none).
    pub id: Option<String>,
    /// `[project] schemas`: schema directories relative to the root, in
    /// override order (a later one wins). Empty when the manifest doesn't
    /// say. [`schema_dirs`](Self::schema_dirs) gives them under the root.
    pub schemas: Vec<PathBuf>,
    /// `[project] scene`: the scene to open by default, relative to the root.
    pub scene: Option<PathBuf>,
    /// `[project] input`: the input config, relative to the root.
    pub input: Option<PathBuf>,
    /// `[project] save_version`: the version the game's save data is at
    /// (#726). A save file at a lower one is handed to the scripts'
    /// `on_persist_migrate(from)`. 0 when the manifest doesn't say.
    pub save_version: i64,
}

static OVERRIDE: OnceLock<Project> = OnceLock::new();

/// Pin the project for the rest of the process (the CLI's `--project`).
/// Every later [`Project::current`] and [`Project::for_path`] returns it.
/// The first call wins.
pub fn set_override(project: Project) {
    let _ = OVERRIDE.set(project);
}

/// The project pinned by [`set_override`], if any.
pub fn overridden() -> Option<&'static Project> {
    OVERRIDE.get()
}

impl Project {
    /// The project rooted at `root`, as `--project root` names it: no walk,
    /// and its manifest read if it has one. An error if `root` isn't a
    /// directory.
    pub fn at(root: impl AsRef<Path>) -> Result<Self, ProjectError> {
        let root = anchor(root.as_ref());
        if !vfs::is_dir(&root) {
            return Err(ProjectError::NotADirectory { dir: root });
        }
        Self::read(root, Found::Override)
    }

    /// The project for this process: the [`set_override`] one if set, else
    /// the one discovered from the working directory.
    pub fn current() -> Result<Self, ProjectError> {
        if let Some(project) = OVERRIDE.get() {
            return Ok(project.clone());
        }
        Self::discover(Path::new("."))
    }

    /// The project a file belongs to (a scene, say): the [`set_override`]
    /// one if set, else the one discovered from the file's folder. Unlike
    /// [`discover`](Self::discover), a path that doesn't exist yet is taken
    /// as a file to be written, and the walk starts at its folder.
    pub fn for_path(path: &Path) -> Result<Self, ProjectError> {
        if let Some(project) = OVERRIDE.get() {
            return Ok(project.clone());
        }
        let dir = anchor(path);
        let dir = if vfs::is_dir(&dir) {
            dir
        } else {
            parent_of(&dir)
        };
        Self::walk(dir, home_dir().as_deref())
    }

    /// Walk up from a scene file or a directory to the nearest project root
    /// (see the module docs for the markers) and read its manifest.
    pub fn discover(scene_or_dir: &Path) -> Result<Self, ProjectError> {
        Self::discover_with_home(scene_or_dir, home_dir().as_deref())
    }

    /// [`discover`](Self::discover) with the home directory given, so tests
    /// don't depend on the machine's.
    pub fn discover_with_home(
        scene_or_dir: &Path,
        home: Option<&Path>,
    ) -> Result<Self, ProjectError> {
        let start = anchor(scene_or_dir);
        let start = if vfs::is_file(&start) {
            parent_of(&start)
        } else {
            start
        };
        Self::walk(start, home)
    }

    fn walk(start: PathBuf, home: Option<&Path>) -> Result<Self, ProjectError> {
        let home = home.map(anchor);
        let mut engine: Option<PathBuf> = None;
        let mut dir = Some(start.clone());
        while let Some(d) = dir {
            if vfs::is_file(d.join("crates/flint-core/Cargo.toml")) {
                // An engine checkout: a game above it owns the project.
                engine.get_or_insert_with(|| d.clone());
            } else if let Some(found) = marker(&d, home.as_deref()) {
                return Self::read(d, found);
            }
            dir = parent_dir(&d);
        }
        match engine {
            Some(root) => Self::read(root, Found::Engine),
            None => Self::read(start, Found::Fallback),
        }
    }

    /// A project at `root` with no marker and no manifest read.
    fn bare(root: PathBuf) -> Self {
        Self {
            root,
            found: Found::Fallback,
            id: None,
            schemas: Vec::new(),
            scene: None,
            input: None,
            save_version: 0,
        }
    }

    /// The project at `root`, with whatever `root/flint.toml` says.
    fn read(root: PathBuf, found: Found) -> Result<Self, ProjectError> {
        let mut project = Self {
            found,
            ..Self::bare(root)
        };
        let file = project.manifest_path();
        if !vfs::is_file(&file) {
            return Ok(project);
        }
        let text = vfs::read_to_string(&file).map_err(|e| ProjectError::Read {
            file: file.clone(),
            message: e.to_string(),
        })?;
        project.apply_manifest(&file, &text)?;
        Ok(project)
    }

    fn apply_manifest(&mut self, file: &Path, text: &str) -> Result<(), ProjectError> {
        let doc: toml::Table = text
            .parse()
            .map_err(|e: toml::de::Error| ProjectError::Syntax {
                file: file.to_path_buf(),
                message: e.to_string().trim_end().to_string(),
            })?;
        let Some(section) = doc.get("project") else {
            return Ok(());
        };
        let wrong =
            |key: String, expected: &'static str, got: &toml::Value| ProjectError::WrongType {
                file: file.to_path_buf(),
                key,
                expected,
                got: kind_of(got),
            };
        let table = section
            .as_table()
            .ok_or_else(|| wrong("project".into(), "a table", section))?;
        if let Some(key) = table.keys().find(|k| !PROJECT_KEYS.contains(&k.as_str())) {
            return Err(ProjectError::UnknownKey {
                file: file.to_path_buf(),
                key: key.clone(),
            });
        }
        let string = |key: &str| -> Result<Option<&str>, ProjectError> {
            match table.get(key) {
                None => Ok(None),
                Some(toml::Value::String(s)) if !s.trim().is_empty() => Ok(Some(s.as_str())),
                Some(toml::Value::String(_)) => Err(ProjectError::WrongType {
                    file: file.to_path_buf(),
                    key: format!("project.{key}"),
                    expected: "a non-empty string",
                    got: "an empty string",
                }),
                Some(other) => Err(wrong(format!("project.{key}"), "a string", other)),
            }
        };
        self.id = string("id")?.map(str::to_string);
        self.scene = string("scene")?.map(PathBuf::from);
        self.input = string("input")?.map(PathBuf::from);
        self.save_version = match table.get("save_version") {
            None => 0,
            Some(toml::Value::Integer(v)) if *v >= 0 => *v,
            Some(toml::Value::Integer(_)) => {
                return Err(ProjectError::WrongType {
                    file: file.to_path_buf(),
                    key: "project.save_version".into(),
                    expected: "a whole number, 0 or more",
                    got: "a negative number",
                })
            }
            Some(other) => {
                return Err(wrong(
                    "project.save_version".into(),
                    "a whole number, 0 or more",
                    other,
                ))
            }
        };
        if let Some(value) = table.get("schemas") {
            let list = value
                .as_array()
                .ok_or_else(|| wrong("project.schemas".into(), "a list of strings", value))?;
            self.schemas = list
                .iter()
                .enumerate()
                .map(|(i, entry)| match entry {
                    toml::Value::String(s) if !s.trim().is_empty() => Ok(PathBuf::from(s)),
                    toml::Value::String(_) => Err(ProjectError::WrongType {
                        file: file.to_path_buf(),
                        key: format!("project.schemas[{i}]"),
                        expected: "a non-empty string",
                        got: "an empty string",
                    }),
                    other => Err(wrong(format!("project.schemas[{i}]"), "a string", other)),
                })
                .collect::<Result<_, _>>()?;
        }
        Ok(())
    }

    /// Where this project's manifest is, or would be: `<root>/flint.toml`.
    pub fn manifest_path(&self) -> PathBuf {
        self.root.join(MANIFEST_FILE)
    }

    /// The manifest's schema directories under the root, in override order.
    /// Empty when the manifest names none.
    pub fn schema_dirs(&self) -> Vec<PathBuf> {
        self.schemas.iter().map(|s| self.root.join(s)).collect()
    }

    /// The manifest's default scene under the root.
    pub fn scene_path(&self) -> Option<PathBuf> {
        self.scene.as_ref().map(|s| self.root.join(s))
    }

    /// The manifest's input config under the root.
    pub fn input_path(&self) -> Option<PathBuf> {
        self.input.as_ref().map(|s| self.root.join(s))
    }

    /// The scene to open when none was named (`flint play` with no
    /// argument): the manifest's `[project] scene`, spelled from the
    /// working directory where it lies under it. With no such key, or a
    /// key naming a file that isn't there, the error says what to pass and
    /// where a default can be named.
    pub fn default_scene(&self) -> Result<PathBuf, NoDefaultScene> {
        // A root of `.` (a bundle's) adds nothing to the path
        let path = match &self.scene {
            Some(scene) if self.root == Path::new(".") => Some(scene.clone()),
            _ => self.scene_path(),
        };
        let Some(path) = path else {
            return Err(NoDefaultScene {
                manifest: self.manifest_path(),
                has_manifest: vfs::is_file(self.manifest_path()),
                names: None,
            });
        };
        if !vfs::is_file(&path) {
            return Err(NoDefaultScene {
                manifest: self.manifest_path(),
                has_manifest: true,
                names: Some(path),
            });
        }
        let from_here = std::env::current_dir()
            .ok()
            .and_then(|cwd| path.strip_prefix(cwd).ok().map(Path::to_path_buf));
        Ok(from_here.unwrap_or(path))
    }
}

/// No scene was named and the project names no default (or names one that
/// isn't there). Its message is the whole of what to do about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoDefaultScene {
    /// Where the manifest is, or would be.
    pub manifest: PathBuf,
    pub has_manifest: bool,
    /// The file `[project] scene` names, when it names one that is missing.
    pub names: Option<PathBuf>,
}

impl std::fmt::Display for NoDefaultScene {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let manifest = shown(&self.manifest);
        if let Some(scene) = &self.names {
            return write!(
                f,
                "no scene given, and the default scene is not a file: {} (named by \
                 `[project] scene` in {manifest}). Pass a scene, or fix the key",
                shown(scene)
            );
        }
        write!(
            f,
            "no scene given: pass one (a path to a .scene.toml), or name the project's \
             default with `scene = \"scenes/main.scene.toml\"` under [project] in {manifest}"
        )?;
        if !self.has_manifest {
            write!(f, " (there is no such file yet)")?;
        }
        Ok(())
    }
}

impl std::error::Error for NoDefaultScene {}

/// Where a list of schema directories came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SchemaSource {
    /// `--schemas` flags: the whole list, nothing added.
    Flags,
    /// `[project] schemas` in this manifest file.
    Manifest(PathBuf),
    /// A list the host handed over (a bundle's manifest, a game embedding
    /// the player), with any `schemas/` beside the scene or one level up.
    Host,
    /// No flags and no manifest list: `schemas/` in the working directory,
    /// beside the scene and one level up, whichever exist.
    Discovery,
}

impl SchemaSource {
    /// One word for reports: `flags`, `manifest`, `host` or `discovery`.
    pub fn name(&self) -> &'static str {
        match self {
            SchemaSource::Flags => "flags",
            SchemaSource::Manifest(_) => "manifest",
            SchemaSource::Host => "host",
            SchemaSource::Discovery => "discovery",
        }
    }
}

impl std::fmt::Display for SchemaSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SchemaSource::Flags => f.write_str("--schemas"),
            SchemaSource::Manifest(file) => write!(f, "`schemas` in {}", file.display()),
            SchemaSource::Host => f.write_str("the bundle or the embedding game"),
            SchemaSource::Discovery => f.write_str(
                "discovery: schemas/ in the working directory, beside the scene and one level up",
            ),
        }
    }
}

/// The schema directories a run loads, in override order (a later one wins),
/// and where the list came from. Made by [`schema_dirs`] (every command and
/// the viewer) or [`host_schema_dirs`] (a bundle, an embedding game).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchemaDirs {
    pub dirs: Vec<PathBuf>,
    pub source: SchemaSource,
    /// TEMPORARY, for one release (ADR 0114, decision 3: a change of order
    /// is logged for one release, then the log goes; #765). With flags and
    /// a scene: the `schemas/` beside the scene or one level up that the
    /// commands used to add to the flags and no longer do, less any the
    /// flags name themselves. Empty otherwise.
    /// [`dropped_warnings`](Self::dropped_warnings) words them.
    pub dropped: Vec<PathBuf>,
}

impl Default for SchemaDirs {
    /// No directories yet, to be discovered: what a player holds before its
    /// host says anything.
    fn default() -> Self {
        Self {
            dirs: Vec::new(),
            source: SchemaSource::Discovery,
            dropped: Vec::new(),
        }
    }
}

/// Why the schema directories could not be named.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum SchemaDirsError {
    /// A directory named by a flag or the manifest isn't there.
    #[error("schema directory not found: {} (named by {named_by})", dir.display())]
    Missing {
        dir: PathBuf,
        named_by: SchemaSource,
    },
    /// The project's manifest could not be read.
    #[error(transparent)]
    Project(#[from] ProjectError),
}

/// The one answer to "which schema directories?" (ADR 0114, decision 3).
///
/// - `flags` (the `--schemas` values) if any were given: they are the whole
///   list.
/// - Else `[project] schemas` of the project `scene` belongs to (the working
///   directory's when there is no scene, the `--project` one when given).
/// - Else discovery, as before there were manifests: `schemas/` in the
///   working directory, beside the scene and one level up, whichever exist.
///
/// A directory that a flag or the manifest names and that isn't there is an
/// error naming it and who named it. Discovery only returns what exists, so
/// its list may be empty.
pub fn schema_dirs(
    scene: Option<&Path>,
    flags: &[impl AsRef<Path>],
) -> Result<SchemaDirs, SchemaDirsError> {
    if !flags.is_empty() {
        let dirs = flags.iter().map(|f| f.as_ref().to_path_buf()).collect();
        let mut resolved = named(dirs, SchemaSource::Flags)?;
        // TEMPORARY, for one release (ADR 0114 decision 3): what the old
        // rule would have added to these flags
        if let Some(scene) = scene {
            let mut beside = Vec::new();
            add_dirs_beside(scene, &mut beside);
            let named: Vec<String> = resolved.dirs.iter().map(|d| same_dir_key(d)).collect();
            beside.retain(|d| !named.contains(&same_dir_key(d)));
            resolved.dropped = beside;
        }
        return Ok(resolved);
    }
    let project = match scene {
        Some(scene) => Project::for_path(scene)?,
        None => Project::current()?,
    };
    if !project.schemas.is_empty() {
        let source = SchemaSource::Manifest(project.manifest_path());
        return named(project.schema_dirs(), source);
    }
    let mut dirs = Vec::new();
    if vfs::is_dir("schemas") {
        dirs.push(PathBuf::from("schemas"));
    }
    if let Some(scene) = scene {
        add_dirs_beside(scene, &mut dirs);
    }
    Ok(SchemaDirs {
        dirs,
        source: SchemaSource::Discovery,
        dropped: Vec::new(),
    })
}

/// The directories of a list the host already settled (a bundle's manifest,
/// a game embedding the player): those of `listed` that exist, then any
/// `schemas/` beside `scene` or one level up. Nothing here is an error; the
/// packer checked the list when it wrote it.
pub fn host_schema_dirs(listed: &[impl AsRef<Path>], scene: Option<&Path>) -> SchemaDirs {
    let mut dirs: Vec<PathBuf> = Vec::new();
    for dir in listed {
        let dir = dir.as_ref();
        if vfs::is_dir(dir) && !dirs.iter().any(|d| d == dir) {
            dirs.push(dir.to_path_buf());
        }
    }
    if let Some(scene) = scene {
        add_dirs_beside(scene, &mut dirs);
    }
    SchemaDirs {
        dirs,
        source: SchemaSource::Host,
        dropped: Vec::new(),
    }
}

impl SchemaDirs {
    /// TEMPORARY, for one release (ADR 0114 decision 3, #765): one line for
    /// each directory in [`dropped`](Self::dropped), for a command that
    /// used to add it to its `--schemas` flags (play, render, validate, the
    /// spline editor, `flint-player`) to print on stderr. Never stdout:
    /// `flint render --report json` owns that.
    pub fn dropped_warnings(&self) -> Vec<String> {
        self.dropped
            .iter()
            .map(|dir| {
                format!(
                    "warning: {} is no longer added to --schemas: the flags are the whole \
                     list now. Pass it with another --schemas, or list it under \
                     [project] schemas in flint.toml",
                    dir.display()
                )
            })
            .collect()
    }

    /// The directories for the scene a transition leads to. Flags stay as
    /// they are. A manifest's list is read again, since the new scene may
    /// belong to another project. A discovered or host list keeps what it
    /// has and gains whatever lies beside the new scene, as the player
    /// always did.
    pub fn for_scene(&self, scene: &Path) -> Result<SchemaDirs, SchemaDirsError> {
        match self.source {
            SchemaSource::Flags => Ok(self.clone()),
            SchemaSource::Host => {
                let mut next = self.clone();
                add_dirs_beside(scene, &mut next.dirs);
                Ok(next)
            }
            SchemaSource::Manifest(_) | SchemaSource::Discovery => {
                let mut next = schema_dirs(Some(scene), &[] as &[&Path])?;
                if self.source == SchemaSource::Discovery && next.source == SchemaSource::Discovery
                {
                    let found = std::mem::replace(&mut next.dirs, self.dirs.clone());
                    for dir in found {
                        if !next.dirs.contains(&dir) {
                            next.dirs.push(dir);
                        }
                    }
                }
                Ok(next)
            }
        }
    }
}

/// A list somebody named: every directory in it has to be there.
fn named(dirs: Vec<PathBuf>, source: SchemaSource) -> Result<SchemaDirs, SchemaDirsError> {
    match dirs.iter().find(|d| !vfs::is_dir(d)) {
        Some(dir) => Err(SchemaDirsError::Missing {
            dir: dir.clone(),
            named_by: source,
        }),
        None => Ok(SchemaDirs {
            dirs,
            source,
            dropped: Vec::new(),
        }),
    }
}

/// One spelling for a directory, so `schemas`, `./schemas` and an absolute
/// path to the same folder compare equal: taken from the working directory
/// as [`anchor`] does, `.` and `..` folded, and without case on Windows.
/// Whether two paths name one file or folder, however each is spelled
/// (relative or absolute, `.` and `..`, and without case on Windows).
/// Nothing is read: a link is not followed.
pub fn same_path(a: &Path, b: &Path) -> bool {
    a == b || same_dir_key(a) == same_dir_key(b)
}

fn same_dir_key(dir: &Path) -> String {
    let key = vfs::normalize(&anchor(dir));
    if cfg!(windows) {
        key.to_lowercase()
    } else {
        key
    }
}

/// `schemas/` beside a scene and one level up, appended where they exist
/// and aren't listed yet. The whole of what "discovery" means for a scene.
fn add_dirs_beside(scene: &Path, dirs: &mut Vec<PathBuf>) {
    let scene_dir = scene.parent().unwrap_or_else(|| Path::new("."));
    for dir in [Some(scene_dir), scene_dir.parent()].into_iter().flatten() {
        let schemas = dir.join("schemas");
        if vfs::is_dir(&schemas) && !dirs.contains(&schemas) {
            dirs.push(schemas);
        }
    }
}

fn marker(dir: &Path, home: Option<&Path>) -> Option<Found> {
    if vfs::is_file(dir.join(MANIFEST_FILE)) {
        return Some(Found::Manifest);
    }
    if vfs::is_dir(dir.join(".flint")) && Some(dir) != home {
        return Some(Found::StateDir);
    }
    if vfs::is_dir(dir.join("schemas"))
        && (vfs::is_dir(dir.join("levels")) || vfs::is_dir(dir.join("scenes")))
    {
        return Some(Found::Layout);
    }
    None
}

fn kind_of(value: &toml::Value) -> &'static str {
    match value {
        toml::Value::String(_) => "a string",
        toml::Value::Integer(_) => "an integer",
        toml::Value::Float(_) => "a float",
        toml::Value::Boolean(_) => "a boolean",
        toml::Value::Datetime(_) => "a date",
        toml::Value::Array(_) => "a list",
        toml::Value::Table(_) => "a table",
    }
}

/// Where a path is walked from. An absolute path is itself. A relative one
/// is taken from the working directory when that directory is in the file
/// source (the disk), and otherwise is the source's own: cleaned of `.` and
/// folded `..`, with `.` for the source's root.
fn anchor(p: &Path) -> PathBuf {
    if p.is_absolute() {
        return p.to_path_buf();
    }
    match std::env::current_dir() {
        Ok(mut cwd) if vfs::is_dir(&cwd) => {
            // Pushed by component, so `.` is the working directory itself.
            cwd.extend(
                p.components()
                    .filter(|c| !matches!(c, std::path::Component::CurDir)),
            );
            cwd
        }
        _ => {
            let clean = vfs::normalize(p);
            if clean.is_empty() {
                PathBuf::from(".")
            } else {
                PathBuf::from(clean)
            }
        }
    }
}

/// The directory above `dir` in the walk, if there is one. A relative path
/// climbs to `.`, the file source's root, and stops there; one that leads out
/// of the source (`..`) stops where it is.
fn parent_dir(dir: &Path) -> Option<PathBuf> {
    if dir == Path::new(".") || dir.file_name().is_none() {
        // `.`, a filesystem root, or a path ending in `..`.
        return None;
    }
    let parent = dir.parent()?;
    Some(if parent.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        parent.to_path_buf()
    })
}

/// The folder a file is in; the path itself when it has none.
fn parent_of(path: &Path) -> PathBuf {
    parent_dir(path).unwrap_or_else(|| path.to_path_buf())
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vfs::MemorySource;
    use std::sync::Arc;

    /// Run `f` over a memory source holding `files`. A path ending in `/`
    /// stands for a directory: it gets a placeholder file inside.
    fn with_files<R>(files: &[(&str, &str)], f: impl FnOnce() -> R) -> R {
        let mut source = MemorySource::new();
        for (path, text) in files {
            if path.ends_with('/') {
                source.insert(format!("{path}.keep"), *text);
            } else {
                source.insert(path, *text);
            }
        }
        vfs::scoped(Arc::new(source), f)
    }

    fn discover(start: &str) -> Result<Project, ProjectError> {
        Project::discover_with_home(Path::new(start), None)
    }

    const FULL: &str = r#"
[project]
id = "float"
schemas = ["engine/schemas", "schemas"]
scene = "scenes/main.scene.toml"
input = "config/input.toml"
"#;

    #[test]
    fn manifest_is_found_from_a_scene_and_every_key_is_read() {
        with_files(
            &[
                ("game/flint.toml", FULL),
                ("game/scenes/deep/a.scene.toml", ""),
            ],
            || {
                let p = discover("game/scenes/deep/a.scene.toml").unwrap();
                assert_eq!(p.root, Path::new("game"));
                assert_eq!(p.found, Found::Manifest);
                assert_eq!(p.id.as_deref(), Some("float"));
                assert_eq!(
                    p.schemas,
                    [PathBuf::from("engine/schemas"), PathBuf::from("schemas")]
                );
                assert_eq!(
                    p.scene.as_deref(),
                    Some(Path::new("scenes/main.scene.toml"))
                );
                assert_eq!(p.input.as_deref(), Some(Path::new("config/input.toml")));
                assert_eq!(p.manifest_path(), Path::new("game").join("flint.toml"));
                assert_eq!(
                    p.schema_dirs(),
                    [
                        Path::new("game").join("engine/schemas"),
                        Path::new("game").join("schemas")
                    ]
                );
                assert_eq!(
                    p.scene_path(),
                    Some(Path::new("game").join("scenes/main.scene.toml"))
                );
                assert_eq!(
                    p.input_path(),
                    Some(Path::new("game").join("config/input.toml"))
                );
                // The same project from a directory, and from the root.
                assert_eq!(discover("game/scenes/deep").unwrap(), p);
                assert_eq!(discover("game").unwrap(), p);
            },
        );
    }

    #[test]
    fn a_manifest_at_the_sources_root_is_found() {
        with_files(
            &[
                ("flint.toml", "[project]\nid = \"kart\"\n"),
                ("levels/a.scene.toml", ""),
            ],
            || {
                let p = discover("levels/a.scene.toml").unwrap();
                assert_eq!(p.root, Path::new("."));
                assert_eq!(p.found, Found::Manifest);
                assert_eq!(p.id.as_deref(), Some("kart"));
                assert_eq!(discover("./levels/../levels").unwrap(), p);
                assert_eq!(discover(".").unwrap(), p);
            },
        );
    }

    #[test]
    fn the_default_scene_is_the_manifests_or_an_error_that_says_what_to_do() {
        with_files(
            &[
                ("game/flint.toml", FULL),
                ("game/scenes/main.scene.toml", ""),
            ],
            || {
                let p = discover("game").unwrap();
                assert_eq!(
                    p.default_scene().unwrap(),
                    Path::new("game").join("scenes/main.scene.toml")
                );
            },
        );
        // Named, and not there
        with_files(&[("game/flint.toml", FULL)], || {
            let err = discover("game").unwrap().default_scene().unwrap_err();
            let said = err.to_string();
            assert!(said.contains("game/scenes/main.scene.toml"), "{said}");
            assert!(said.contains("`[project] scene`"), "{said}");
            assert!(said.contains("game/flint.toml"), "{said}");
        });
        // A manifest with no scene key; and no manifest at all
        for (files, has) in [
            (&[("game/flint.toml", "[project]\nid = \"x\"\n")][..], true),
            (&[("game/scenes/a.scene.toml", "")][..], false),
        ] {
            with_files(files, || {
                let err = discover("game").unwrap().default_scene().unwrap_err();
                assert_eq!(err.has_manifest, has);
                let said = err.to_string();
                assert!(said.contains("pass one"), "{said}");
                assert!(said.contains("scene = \""), "{said}");
                assert!(said.contains("[project]"), "{said}");
                assert!(said.contains("game/flint.toml"), "{said}");
                assert_eq!(said.contains("no such file yet"), !has, "{said}");
            });
        }
    }

    #[test]
    fn an_empty_manifest_and_one_without_project_are_bare_markers() {
        for text in [
            "",
            "# just a marker\n",
            "[web]\ntitle = \"x\"\n",
            "[project]\n",
        ] {
            with_files(&[("g/flint.toml", text), ("g/scenes/", "")], || {
                let p = discover("g/scenes").unwrap();
                assert_eq!(p.found, Found::Manifest, "{text:?}");
                assert_eq!(p.id, None);
                assert!(p.schemas.is_empty());
                assert_eq!((p.scene, p.input), (None, None));
            });
        }
    }

    #[test]
    fn each_key_alone_is_enough() {
        with_files(&[("flint.toml", "[project]\nschemas = []\n")], || {
            assert!(discover(".").unwrap().schemas.is_empty());
        });
        with_files(
            &[("flint.toml", "[project]\nscene = \"a.scene.toml\"\n")],
            || {
                let p = discover(".").unwrap();
                assert_eq!(p.scene.as_deref(), Some(Path::new("a.scene.toml")));
                assert_eq!(p.id, None);
            },
        );
    }

    /// #726: the version the game's saves are at.
    #[test]
    fn save_version_is_a_whole_number_and_0_when_unsaid() {
        with_files(
            &[(
                "flint.toml",
                "[project]
id = \"x\"
",
            )],
            || {
                assert_eq!(discover(".").unwrap().save_version, 0);
            },
        );
        with_files(
            &[(
                "flint.toml",
                "[project]
save_version = 3
",
            )],
            || {
                assert_eq!(discover(".").unwrap().save_version, 3);
            },
        );
        for (text, got) in [
            (
                "[project]
save_version = -1
",
                "a negative number",
            ),
            (
                "[project]
save_version = \"2\"
",
                "a string",
            ),
            (
                "[project]
save_version = 1.5
",
                "a float",
            ),
        ] {
            with_files(&[("flint.toml", text)], || {
                assert_eq!(
                    discover(".").unwrap_err(),
                    ProjectError::WrongType {
                        file: Path::new(".").join("flint.toml"),
                        key: "project.save_version".into(),
                        expected: "a whole number, 0 or more",
                        got,
                    },
                    "{text}"
                );
            });
        }
    }

    #[test]
    fn a_wrong_type_names_the_file_and_the_key() {
        let cases: [(&str, &str, &str, &str); 7] = [
            (
                "[project]\nid = 7\n",
                "project.id",
                "a string",
                "an integer",
            ),
            (
                "[project]\nschemas = \"schemas\"\n",
                "project.schemas",
                "a list of strings",
                "a string",
            ),
            (
                "[project]\nschemas = [\"schemas\", 2.5]\n",
                "project.schemas[1]",
                "a string",
                "a float",
            ),
            (
                "[project]\nscene = true\n",
                "project.scene",
                "a string",
                "a boolean",
            ),
            (
                "[project]\ninput = [\"a\"]\n",
                "project.input",
                "a string",
                "a list",
            ),
            ("project = \"float\"\n", "project", "a table", "a string"),
            (
                "[project]\nid = \"\"\n",
                "project.id",
                "a non-empty string",
                "an empty string",
            ),
        ];
        for (text, key, expected, got) in cases {
            with_files(&[("game/flint.toml", text)], || {
                let err = discover("game").unwrap_err();
                let file = Path::new("game").join("flint.toml");
                assert_eq!(
                    err,
                    ProjectError::WrongType {
                        file: file.clone(),
                        key: key.to_string(),
                        expected,
                        got,
                    }
                );
                let said = err.to_string();
                assert!(said.contains(&file.display().to_string()), "{said}");
                assert!(said.contains(&format!("`{key}`")), "{said}");
            });
        }
    }

    #[test]
    fn an_unknown_project_key_names_the_file_and_the_key() {
        with_files(
            &[("game/flint.toml", "[project]\nid = \"x\"\nshcemas = []\n")],
            || {
                let err = discover("game").unwrap_err();
                let file = Path::new("game").join("flint.toml");
                assert_eq!(
                    err,
                    ProjectError::UnknownKey {
                        file: file.clone(),
                        key: "shcemas".into()
                    }
                );
                let said = err.to_string();
                assert!(said.contains(&file.display().to_string()), "{said}");
                assert!(said.contains("`shcemas`"), "{said}");
                assert!(said.contains("schemas"), "it lists the real keys: {said}");
            },
        );
    }

    #[test]
    fn unknown_top_level_tables_are_left_alone() {
        with_files(
            &[(
                "flint.toml",
                "name = \"x\"\n[project]\nid = \"x\"\n[tool.other]\nschemas = 3\n",
            )],
            || assert_eq!(discover(".").unwrap().id.as_deref(), Some("x")),
        );
    }

    #[test]
    fn broken_toml_names_the_file() {
        with_files(&[("game/flint.toml", "[project\nid = ")], || {
            let err = discover("game").unwrap_err();
            assert!(matches!(err, ProjectError::Syntax { .. }), "{err:?}");
            let file = Path::new("game").join("flint.toml");
            assert!(err.to_string().contains(&file.display().to_string()));
        });
    }

    #[test]
    fn engine_subtree_passes_through_to_the_game() {
        with_files(
            &[
                ("game/flint.toml", "[project]\nid = \"game\"\n"),
                ("game/engine/crates/flint-core/Cargo.toml", ""),
                // Markers inside the subtree's root don't claim it.
                ("game/engine/.flint/assets/", ""),
                ("game/engine/schemas/", ""),
                ("game/engine/scenes/", ""),
                ("game/engine/specs/oak.procgen.toml", ""),
            ],
            || {
                let p = discover("game/engine/specs/oak.procgen.toml").unwrap();
                assert_eq!(p.root, Path::new("game"));
                assert_eq!(p.found, Found::Manifest);
                assert_eq!(p.id.as_deref(), Some("game"));
                assert_eq!(discover("game/engine").unwrap(), p);
            },
        );
    }

    #[test]
    fn standalone_engine_is_its_own_project() {
        with_files(
            &[
                ("flint/crates/flint-core/Cargo.toml", ""),
                ("flint/examples/a.scene.toml", ""),
            ],
            || {
                let p = discover("flint/examples/a.scene.toml").unwrap();
                assert_eq!(p.root, Path::new("flint"));
                assert_eq!(p.found, Found::Engine);
                assert_eq!(p.id, None);
            },
        );
    }

    #[test]
    fn state_dir_marks_a_root_and_the_nearest_marker_wins() {
        with_files(
            &[
                ("outer/flint.toml", ""),
                ("outer/sub/.flint/config.toml", ""),
                ("outer/sub/x/a.scene.toml", ""),
            ],
            || {
                let p = discover("outer/sub/x/a.scene.toml").unwrap();
                assert_eq!(p.root, Path::new("outer/sub"));
                assert_eq!(p.found, Found::StateDir);
                assert_eq!(discover("outer").unwrap().found, Found::Manifest);
            },
        );
    }

    #[test]
    fn home_dot_flint_is_not_a_project() {
        with_files(
            &[("home/.flint/config.toml", ""), ("home/loose/folder/", "")],
            || {
                let start = Path::new("home/loose/folder");
                let p = Project::discover_with_home(start, Some(Path::new("home"))).unwrap();
                assert_eq!(p.found, Found::Fallback);
                assert_eq!(p.root, start);
                let p = Project::discover_with_home(start, None).unwrap();
                assert_eq!(
                    (p.root.as_path(), p.found),
                    (Path::new("home"), Found::StateDir)
                );
            },
        );
    }

    #[test]
    fn layout_marks_a_root() {
        for scenes in ["scenes/", "levels/"] {
            with_files(
                &[
                    ("game/schemas/components/a.toml", ""),
                    (&format!("game/{scenes}"), ""),
                    ("game/specs/deep/", ""),
                ],
                || {
                    let p = discover("game/specs/deep").unwrap();
                    assert_eq!(p.root, Path::new("game"));
                    assert_eq!(p.found, Found::Layout);
                    assert_eq!(p.id, None);
                    assert!(p.schemas.is_empty());
                },
            );
        }
        // schemas/ alone is not a project.
        with_files(&[("game/schemas/", ""), ("game/specs/", "")], || {
            assert_eq!(discover("game/specs").unwrap().found, Found::Fallback);
        });
    }

    #[test]
    fn no_marker_falls_back_to_the_start() {
        with_files(&[("loose/folder/a.scene.toml", "")], || {
            // A scene: its folder. A directory: itself.
            let p = discover("loose/folder/a.scene.toml").unwrap();
            assert_eq!(p.root, Path::new("loose/folder"));
            assert_eq!(p.found, Found::Fallback);
            assert_eq!(discover("loose/folder").unwrap(), p);
            assert_eq!((p.id, p.scene, p.input), (None, None, None));
            assert!(p.schemas.is_empty());
        });
    }

    #[test]
    fn for_path_starts_at_the_folder_of_a_file_not_yet_written() {
        if overridden().is_some() {
            return;
        }
        with_files(&[("game/flint.toml", ""), ("game/scenes/", "")], || {
            let p = Project::for_path(Path::new("game/scenes/new.scene.toml")).unwrap();
            assert_eq!(p.root, Path::new("game"));
            assert_eq!(p.found, Found::Manifest);
        });
    }

    #[test]
    fn at_reads_the_manifest_and_refuses_what_is_not_a_directory() {
        with_files(&[("game/flint.toml", FULL), ("bare/scenes/", "")], || {
            let p = Project::at("game").unwrap();
            assert_eq!(p.found, Found::Override);
            assert_eq!(p.id.as_deref(), Some("float"));
            assert_eq!(p.schemas.len(), 2);

            let bare = Project::at("bare").unwrap();
            assert_eq!((bare.found, bare.id), (Found::Override, None));

            for missing in ["nope", "game/flint.toml"] {
                let err = Project::at(missing).unwrap_err();
                assert_eq!(
                    err,
                    ProjectError::NotADirectory {
                        dir: PathBuf::from(missing)
                    }
                );
                assert!(err.to_string().contains("not a directory"));
            }
        });
        with_files(&[("game/flint.toml", "[project]\nid = 1\n")], || {
            assert!(matches!(
                Project::at("game"),
                Err(ProjectError::WrongType { .. })
            ));
        });
    }

    const NO_FLAGS: &[&str] = &[];
    const GAME: &[(&str, &str)] = &[
        (
            "flint.toml",
            "[project]\nschemas = [\"engine/schemas\", \"schemas\"]\n",
        ),
        ("engine/schemas/", ""),
        ("schemas/", ""),
        ("scenes/a.scene.toml", ""),
        ("scenes/schemas/", ""),
        ("elsewhere/schemas/", ""),
    ];

    fn paths(dirs: &SchemaDirs) -> Vec<String> {
        dirs.dirs.iter().map(|d| vfs::normalize(d)).collect()
    }

    #[test]
    fn schema_flags_are_the_whole_list() {
        with_files(GAME, || {
            let dirs =
                schema_dirs(Some("scenes/a.scene.toml".as_ref()), &["elsewhere/schemas"]).unwrap();
            assert_eq!(dirs.source, SchemaSource::Flags);
            // Not the manifest's, and nothing beside the scene
            assert_eq!(paths(&dirs), ["elsewhere/schemas"]);
            // In the order given
            let dirs = schema_dirs(None, &["schemas", "engine/schemas"]).unwrap();
            assert_eq!(paths(&dirs), ["schemas", "engine/schemas"]);
        });
    }

    /// TEMPORARY with `SchemaDirs::dropped` (one release, ADR 0114
    /// decision 3).
    #[test]
    fn flags_say_what_the_old_rule_would_have_added() {
        let dropped = |scene: &str, flags: &[&str]| -> Vec<String> {
            let dirs = schema_dirs(Some(scene.as_ref()), flags).unwrap();
            assert_eq!(dirs.dropped_warnings().len(), dirs.dropped.len());
            dirs.dropped.iter().map(|d| vfs::normalize(d)).collect()
        };
        with_files(GAME, || {
            // The engine's alone: the game's own, one level up from the
            // scene, and the one beside the scene were both added before
            assert_eq!(
                dropped("scenes/a.scene.toml", &["engine/schemas"]),
                ["scenes/schemas", "schemas"]
            );
            let dirs =
                schema_dirs(Some("scenes/a.scene.toml".as_ref()), &["engine/schemas"]).unwrap();
            let said = &dirs.dropped_warnings()[1];
            assert!(
                said.starts_with("warning: schemas is no longer added"),
                "{said}"
            );
            assert!(said.contains("another --schemas"), "{said}");
            assert!(said.contains("flint.toml"), "{said}");

            // Named by the flags, however it is spelled: nothing to say
            assert_eq!(
                dropped("scenes/a.scene.toml", &["engine/schemas", "./schemas"]),
                ["scenes/schemas"]
            );
            assert_eq!(
                dropped(
                    "scenes/a.scene.toml",
                    &["engine/schemas", "scenes/../schemas", "scenes/schemas/"]
                ),
                [] as [&str; 0]
            );
            // A game root's usual flags, and the goldens' `--schemas schemas`
            assert_eq!(
                dropped("elsewhere/a.scene.toml", &["engine/schemas", "schemas"]),
                ["elsewhere/schemas"]
            );
            assert_eq!(
                dropped("engine/a.scene.toml", &["schemas"]),
                ["engine/schemas"]
            );
        });
        with_files(
            &[
                ("engine/schemas/", ""),
                ("schemas/", ""),
                ("scenes/a.scene.toml", ""),
                ("examples/b.scene.toml", ""),
            ],
            || {
                assert!(dropped("scenes/a.scene.toml", &["engine/schemas", "schemas"]).is_empty());
                assert!(dropped("examples/b.scene.toml", &["schemas"]).is_empty());
                assert!(dropped("examples/b.scene.toml", &["./schemas"]).is_empty());
                // No scene, no flags, a manifest, a host: never
                assert!(schema_dirs(None, &["engine/schemas"])
                    .unwrap()
                    .dropped
                    .is_empty());
                assert!(schema_dirs(Some("scenes/a.scene.toml".as_ref()), NO_FLAGS)
                    .unwrap()
                    .dropped
                    .is_empty());
                assert!(host_schema_dirs(
                    &["engine/schemas"],
                    Some("scenes/a.scene.toml".as_ref())
                )
                .dropped
                .is_empty());
            },
        );
    }

    /// An absolute flag and a relative discovery are the same folder (on
    /// the disk, where the working directory anchors both).
    #[test]
    fn an_absolute_flag_names_the_same_folder_as_a_relative_one() {
        let cwd = std::env::current_dir().unwrap();
        assert_eq!(
            same_dir_key(&cwd.join("schemas")),
            same_dir_key(Path::new("./schemas"))
        );
        assert_eq!(
            same_dir_key(&cwd.join("scenes").join("..").join("schemas")),
            same_dir_key(Path::new("schemas"))
        );
        assert_ne!(
            same_dir_key(Path::new("a/schemas")),
            same_dir_key(Path::new("schemas"))
        );
    }

    #[test]
    fn the_manifest_names_the_schemas_when_no_flag_does() {
        with_files(GAME, || {
            for scene in [Some("scenes/a.scene.toml"), None] {
                let dirs = schema_dirs(scene.map(Path::new), NO_FLAGS).unwrap();
                assert_eq!(
                    dirs.source,
                    SchemaSource::Manifest(PathBuf::from("./flint.toml")),
                    "{scene:?}"
                );
                assert_eq!(dirs.source.name(), "manifest");
                // In the manifest's order, and nothing beside the scene
                assert_eq!(paths(&dirs), ["engine/schemas", "schemas"], "{scene:?}");
            }
        });
        // The scene's project, not the working directory's
        with_files(
            &[
                ("game/flint.toml", "[project]\nschemas = [\"s\"]\n"),
                ("game/s/", ""),
                ("game/scenes/deep/a.scene.toml", ""),
                ("schemas/", ""),
            ],
            || {
                let dirs =
                    schema_dirs(Some("game/scenes/deep/a.scene.toml".as_ref()), NO_FLAGS).unwrap();
                assert_eq!(paths(&dirs), ["game/s"]);
            },
        );
    }

    #[test]
    fn a_named_schema_directory_that_is_missing_is_an_error() {
        with_files(GAME, || {
            let err = schema_dirs(None, &["schemas", "nope"]).unwrap_err();
            assert_eq!(
                err,
                SchemaDirsError::Missing {
                    dir: PathBuf::from("nope"),
                    named_by: SchemaSource::Flags,
                }
            );
            assert_eq!(
                err.to_string(),
                "schema directory not found: nope (named by --schemas)"
            );
        });
        // A manifest naming engine/schemas in a project with no engine subtree
        with_files(
            &[
                (
                    "flint.toml",
                    "[project]\nschemas = [\"engine/schemas\", \"schemas\"]\n",
                ),
                ("schemas/", ""),
                ("scenes/a.scene.toml", ""),
            ],
            || {
                let err = schema_dirs(Some("scenes/a.scene.toml".as_ref()), NO_FLAGS).unwrap_err();
                let SchemaDirsError::Missing { dir, named_by } = &err else {
                    panic!("{err:?}");
                };
                assert_eq!(vfs::normalize(dir), "engine/schemas");
                assert_eq!(named_by.name(), "manifest");
                let said = err.to_string();
                assert!(said.contains("engine/schemas"), "{said}");
                assert!(said.contains("`schemas` in"), "{said}");
                assert!(said.contains("flint.toml"), "{said}");
                // The flags get round it
                assert!(schema_dirs(Some("scenes/a.scene.toml".as_ref()), &["schemas"]).is_ok());
            },
        );
        // A manifest that doesn't read is the error, unless flags were given
        with_files(
            &[("flint.toml", "[project]\nschemas = 3\n"), ("s/", "")],
            || {
                assert!(matches!(
                    schema_dirs(None, NO_FLAGS),
                    Err(SchemaDirsError::Project(ProjectError::WrongType { .. }))
                ));
                assert!(schema_dirs(None, &["s"]).is_ok());
            },
        );
    }

    #[test]
    fn with_no_flags_and_no_list_schemas_are_discovered() {
        // The working directory's, beside the scene, one level up: those
        // that exist, in that order, each once
        with_files(
            &[
                ("schemas/", ""),
                ("game/schemas/", ""),
                ("game/levels/schemas/", ""),
                ("game/levels/a.scene.toml", ""),
            ],
            || {
                let dirs =
                    schema_dirs(Some("game/levels/a.scene.toml".as_ref()), NO_FLAGS).unwrap();
                assert_eq!(dirs.source, SchemaSource::Discovery);
                assert_eq!(
                    paths(&dirs),
                    ["schemas", "game/levels/schemas", "game/schemas"]
                );
                // No scene: the working directory's alone
                assert_eq!(paths(&schema_dirs(None, NO_FLAGS).unwrap()), ["schemas"]);
            },
        );
        // An engine checkout's demo scene: no manifest list, so discovery
        with_files(
            &[
                ("crates/flint-core/Cargo.toml", ""),
                ("schemas/", ""),
                ("examples/a.scene.toml", ""),
            ],
            || {
                let dirs = schema_dirs(Some("examples/a.scene.toml".as_ref()), NO_FLAGS).unwrap();
                assert_eq!(dirs.source, SchemaSource::Discovery);
                assert_eq!(paths(&dirs), ["schemas"]);
            },
        );
        // A manifest with no `schemas` key is a marker: discovery
        with_files(
            &[
                ("flint.toml", "[project]\nid = \"x\"\n"),
                ("schemas/", ""),
                ("engine/schemas/", ""),
            ],
            || {
                let dirs = schema_dirs(None, NO_FLAGS).unwrap();
                assert_eq!(dirs.source, SchemaSource::Discovery);
                assert_eq!(paths(&dirs), ["schemas"]);
            },
        );
        // Nothing anywhere: an empty list, not an error
        with_files(&[("a.scene.toml", "")], || {
            let dirs = schema_dirs(Some("a.scene.toml".as_ref()), NO_FLAGS).unwrap();
            assert_eq!(dirs, SchemaDirs::default());
        });
    }

    #[test]
    fn a_hosts_list_keeps_what_exists_and_adds_what_is_beside_the_scene() {
        with_files(
            &[
                ("engine/schemas/", ""),
                ("schemas/", ""),
                ("levels/schemas/", ""),
                ("levels/a.scene.toml", ""),
            ],
            || {
                let dirs = host_schema_dirs(
                    &["engine/schemas", "gone", "schemas"],
                    Some("levels/a.scene.toml".as_ref()),
                );
                assert_eq!(dirs.source, SchemaSource::Host);
                // `schemas` one level up is already listed
                assert_eq!(
                    paths(&dirs),
                    ["engine/schemas", "schemas", "levels/schemas"]
                );
            },
        );
    }

    #[test]
    fn a_transition_asks_again_for_the_new_scene() {
        with_files(
            &[
                ("schemas/", ""),
                ("a/x.scene.toml", ""),
                ("a/schemas/", ""),
                ("b/y.scene.toml", ""),
                ("b/schemas/", ""),
                ("game/flint.toml", "[project]\nschemas = [\"s\"]\n"),
                ("game/s/", ""),
                ("game/z.scene.toml", ""),
            ],
            || {
                // Flags stay as they are
                let flags = schema_dirs(Some("a/x.scene.toml".as_ref()), &["schemas"]).unwrap();
                assert_eq!(flags.for_scene("b/y.scene.toml".as_ref()).unwrap(), flags);
                assert_eq!(
                    flags.for_scene("game/z.scene.toml".as_ref()).unwrap(),
                    flags
                );

                // Discovery keeps what it had and gains the new scene's
                let found = schema_dirs(Some("a/x.scene.toml".as_ref()), NO_FLAGS).unwrap();
                assert_eq!(paths(&found), ["schemas", "a/schemas"]);
                let next = found.for_scene("b/y.scene.toml".as_ref()).unwrap();
                assert_eq!(next.source, SchemaSource::Discovery);
                assert_eq!(paths(&next), ["schemas", "a/schemas", "b/schemas"]);

                // Into a project with a manifest: its list, and back out again
                let game = next.for_scene("game/z.scene.toml".as_ref()).unwrap();
                assert_eq!(game.source.name(), "manifest");
                assert_eq!(paths(&game), ["game/s"]);
                let back = game.for_scene("b/y.scene.toml".as_ref()).unwrap();
                assert_eq!(paths(&back), ["schemas", "b/schemas"]);

                // A host's list only grows
                let host = host_schema_dirs(&["schemas"], Some("a/x.scene.toml".as_ref()));
                let next = host.for_scene("b/y.scene.toml".as_ref()).unwrap();
                assert_eq!(next.source, SchemaSource::Host);
                assert_eq!(paths(&next), ["schemas", "a/schemas", "b/schemas"]);

                // A player nobody told anything discovers on its first ask
                let first = SchemaDirs::default()
                    .for_scene("a/x.scene.toml".as_ref())
                    .unwrap();
                assert_eq!(paths(&first), ["schemas", "a/schemas"]);
            },
        );
    }

    #[test]
    fn the_working_directory_is_spelled_plainly() {
        let cwd = std::env::current_dir().unwrap();
        assert_eq!(anchor(Path::new(".")), cwd);
        assert_eq!(anchor(Path::new("")), cwd);
        assert_eq!(anchor(Path::new("./src")), cwd.join("src"));
        assert_eq!(anchor(Path::new("../x")), cwd.join("..").join("x"));
    }

    #[test]
    fn the_disk_is_walked_from_the_working_directory() {
        // No scoped source: the real disk. This crate sits inside an engine
        // checkout, whatever is above it.
        let here = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let p = Project::discover_with_home(&here, None).unwrap();
        assert!(p.root.is_absolute());
        assert!(here.starts_with(&p.root));
        assert_ne!(p.found, Found::Fallback);
    }
}
