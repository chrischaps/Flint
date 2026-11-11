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

/// The manifest's file name, at the project root.
pub const MANIFEST_FILE: &str = "flint.toml";

/// The keys `[project]` may hold.
const PROJECT_KEYS: [&str; 4] = ["id", "schemas", "scene", "input"];

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
        "{}: unknown key `{key}` in [project] (the keys are id, schemas, scene, input)",
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

    /// The project at `root`, with whatever `root/flint.toml` says.
    fn read(root: PathBuf, found: Found) -> Result<Self, ProjectError> {
        let mut project = Self {
            root,
            found,
            id: None,
            schemas: Vec::new(),
            scene: None,
            input: None,
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
                ("flint/demo/a.scene.toml", ""),
            ],
            || {
                let p = discover("flint/demo/a.scene.toml").unwrap();
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
