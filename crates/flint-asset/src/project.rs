//! Where a Flint project's asset state lives (#179).
//!
//! The content store (`.flint/assets`), the procgen provenance index
//! (`.flint/procgen`), the job store, the project config and the sidecar
//! catalog (`assets/`) all belong to the project, not to whichever folder a
//! command happens to run from. [`Project::discover`] finds the project root
//! the way cargo finds a workspace: it walks up from a starting directory to
//! the nearest one carrying a project marker.
//!
//! A directory is a project root when it holds, in order of strength:
//!
//! 1. a `flint.toml` file (an explicit marker, nothing reads it yet);
//! 2. a `.flint/` directory, except in the home directory, where
//!    `~/.flint/config.toml` is the *global* config and says nothing about
//!    which project you are in;
//! 3. the layout `flint init` makes and the game repos keep: `schemas/`
//!    beside `levels/` or `scenes/`.
//!
//! A Flint engine checkout (`crates/flint-core/Cargo.toml`) is passed through
//! when there is a project above it, so a command run inside a game's
//! `engine/` subtree lands in the game. With nothing above, the engine
//! checkout is its own project. With no marker anywhere, the starting
//! directory is the root, which is what every command did before.
//!
//! This is tooling: it reads the real disk with `std::fs`, never the
//! runtime [`vfs`](flint_core::vfs). The player does not call it.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// Content store, relative to the project root.
pub const STORE_DIR: &str = ".flint/assets";
/// Procgen provenance index, relative to the project root.
pub const PROCGEN_DIR: &str = ".flint/procgen";
/// Generation jobs, relative to the project root.
pub const JOBS_DIR: &str = ".flint/jobs";
/// Default output folder for AI-generated files, relative to the root.
pub const GENERATED_DIR: &str = ".flint/generated";
/// Project-local config, relative to the project root.
pub const CONFIG_FILE: &str = ".flint/config.toml";
/// Sidecar catalog (`*.asset.toml`), relative to the project root.
pub const CATALOG_DIR: &str = "assets";

/// Why a directory was taken as the project root.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Found {
    /// `--project <dir>` (or [`set_override`]) named it.
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

/// A located project root and the paths of the state it owns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Project {
    pub root: PathBuf,
    pub found: Found,
}

static OVERRIDE: OnceLock<PathBuf> = OnceLock::new();

/// Pin the project root for the rest of the process (the CLI's `--project`).
/// Every later [`Project::current`] returns it. The first call wins.
pub fn set_override(root: impl Into<PathBuf>) {
    let _ = OVERRIDE.set(root.into());
}

impl Project {
    /// A project rooted at `root`, as if `--project root` had been given.
    pub fn at(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            found: Found::Override,
        }
    }

    /// The project for this process: the [`set_override`] root if one was
    /// set, else the one discovered from the working directory.
    pub fn current() -> Self {
        if let Some(root) = OVERRIDE.get() {
            return Self::at(root.clone());
        }
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        Self::discover_with_home(&cwd, home_dir().as_deref())
    }

    /// The project a file belongs to (a scene, say): the [`set_override`]
    /// root if one was set, else the one discovered from the file's folder.
    pub fn for_path(path: &Path) -> Self {
        if let Some(root) = OVERRIDE.get() {
            return Self::at(root.clone());
        }
        let dir = absolute(path);
        let dir = if dir.is_dir() {
            dir
        } else {
            dir.parent().map(Path::to_path_buf).unwrap_or(dir)
        };
        Self::discover(&dir)
    }

    /// Walk up from `start` to the nearest project root (see the module
    /// docs for the markers).
    pub fn discover(start: &Path) -> Self {
        Self::discover_with_home(start, home_dir().as_deref())
    }

    /// [`discover`](Self::discover) with the home directory given, so tests
    /// don't depend on the machine's.
    pub fn discover_with_home(start: &Path, home: Option<&Path>) -> Self {
        let start = absolute(start);
        let home = home.map(absolute);
        let mut engine: Option<PathBuf> = None;
        for dir in start.ancestors() {
            if dir.join("crates/flint-core/Cargo.toml").is_file() {
                // An engine checkout: a game above it owns the state.
                engine.get_or_insert_with(|| dir.to_path_buf());
                continue;
            }
            if let Some(found) = marker(dir, home.as_deref()) {
                return Self {
                    root: dir.to_path_buf(),
                    found,
                };
            }
        }
        match engine {
            Some(root) => Self {
                root,
                found: Found::Engine,
            },
            None => Self {
                root: start,
                found: Found::Fallback,
            },
        }
    }

    pub fn store_dir(&self) -> PathBuf {
        join_rel(&self.root, STORE_DIR)
    }
    pub fn procgen_dir(&self) -> PathBuf {
        join_rel(&self.root, PROCGEN_DIR)
    }
    pub fn jobs_dir(&self) -> PathBuf {
        join_rel(&self.root, JOBS_DIR)
    }
    pub fn generated_dir(&self) -> PathBuf {
        join_rel(&self.root, GENERATED_DIR)
    }
    pub fn config_file(&self) -> PathBuf {
        join_rel(&self.root, CONFIG_FILE)
    }
    pub fn catalog_dir(&self) -> PathBuf {
        join_rel(&self.root, CATALOG_DIR)
    }

    /// `path` shown relative to the working directory (`.flint\assets`,
    /// `..\.flint\assets`), so a message says where the store is from here.
    /// Absolute when the two share no ancestor (another drive).
    pub fn display(path: &Path) -> String {
        match std::env::current_dir() {
            Ok(cwd) => relative_to(path, &cwd).display().to_string(),
            Err(_) => path.display().to_string(),
        }
    }
}

fn marker(dir: &Path, home: Option<&Path>) -> Option<Found> {
    if dir.join("flint.toml").is_file() {
        return Some(Found::Manifest);
    }
    if dir.join(".flint").is_dir() && Some(dir) != home {
        return Some(Found::StateDir);
    }
    if dir.join("schemas").is_dir() && (dir.join("levels").is_dir() || dir.join("scenes").is_dir())
    {
        return Some(Found::Layout);
    }
    None
}

/// `root` joined with a `/`-separated relative path, one segment at a time
/// so the separators are the platform's.
fn join_rel(root: &Path, rel: &str) -> PathBuf {
    let mut p = root.to_path_buf();
    p.extend(rel.split('/'));
    p
}

/// `path` relative to `base`, climbing with `..` as needed.
fn relative_to(path: &Path, base: &Path) -> PathBuf {
    let (path, base) = (absolute(path), absolute(base));
    let (pc, bc): (Vec<_>, Vec<_>) = (path.components().collect(), base.components().collect());
    let common = pc.iter().zip(&bc).take_while(|(a, b)| a == b).count();
    if common == 0 {
        return path;
    }
    let mut rel = PathBuf::new();
    for _ in common..bc.len() {
        rel.push("..");
    }
    for c in &pc[common..] {
        rel.push(c);
    }
    if rel.as_os_str().is_empty() {
        rel.push(".");
    }
    rel
}

fn absolute(p: &Path) -> PathBuf {
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|c| c.join(p))
            .unwrap_or_else(|_| p.to_path_buf())
    }
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
    use std::fs;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("flint_project_{tag}_{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn paths_are_shown_from_where_you_stand() {
        let base = Path::new("/game/specs");
        let up = relative_to(Path::new("/game/.flint/assets"), base);
        assert_eq!(up, Path::new("..").join(".flint").join("assets"));
        assert_eq!(relative_to(Path::new("/game/specs"), base), Path::new("."));
        assert_eq!(
            relative_to(Path::new("/game/specs/x.png"), base),
            Path::new("x.png")
        );
    }

    #[test]
    fn layout_root_found_from_a_subfolder() {
        let root = tmp("layout");
        fs::create_dir_all(root.join("schemas")).unwrap();
        fs::create_dir_all(root.join("scenes")).unwrap();
        fs::create_dir_all(root.join("specs/deep")).unwrap();
        let p = Project::discover_with_home(&root.join("specs/deep"), None);
        assert_eq!(p.root, root);
        assert_eq!(p.found, Found::Layout);
        assert_eq!(Project::discover_with_home(&root, None).root, root);
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn nearest_marker_wins() {
        let outer = tmp("nearest");
        fs::write(outer.join("flint.toml"), "").unwrap();
        let inner = outer.join("sub");
        fs::create_dir_all(inner.join(".flint")).unwrap();
        fs::create_dir_all(inner.join("x")).unwrap();
        let p = Project::discover_with_home(&inner.join("x"), None);
        assert_eq!(p.root, inner);
        assert_eq!(p.found, Found::StateDir);
        fs::remove_dir_all(&outer).ok();
    }

    #[test]
    fn home_dot_flint_is_not_a_project() {
        let home = tmp("home");
        fs::create_dir_all(home.join(".flint")).unwrap();
        fs::create_dir_all(home.join("loose/folder")).unwrap();
        let start = home.join("loose/folder");
        let p = Project::discover_with_home(&start, Some(&home));
        assert_eq!(p.found, Found::Fallback);
        assert_eq!(p.root, start);
        fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn engine_subtree_passes_through_to_the_game() {
        let game = tmp("game");
        fs::create_dir_all(game.join("schemas")).unwrap();
        fs::create_dir_all(game.join("scenes")).unwrap();
        let engine = game.join("engine");
        fs::create_dir_all(engine.join("crates/flint-core")).unwrap();
        fs::write(engine.join("crates/flint-core/Cargo.toml"), "").unwrap();
        // Even a stray .flint/ left in the subtree by the old behaviour.
        fs::create_dir_all(engine.join(".flint/assets")).unwrap();
        fs::create_dir_all(engine.join("specs")).unwrap();
        let p = Project::discover_with_home(&engine.join("specs"), None);
        assert_eq!(p.root, game);
        assert_eq!(p.found, Found::Layout);
        fs::remove_dir_all(&game).ok();
    }

    #[test]
    fn standalone_engine_is_its_own_project() {
        let engine = tmp("engine");
        fs::create_dir_all(engine.join("crates/flint-core")).unwrap();
        fs::write(engine.join("crates/flint-core/Cargo.toml"), "").unwrap();
        fs::create_dir_all(engine.join("specs")).unwrap();
        let p = Project::discover_with_home(&engine.join("specs"), None);
        assert_eq!(p.root, engine);
        assert_eq!(p.found, Found::Engine);
        assert_eq!(p.store_dir(), engine.join(".flint/assets"));
        fs::remove_dir_all(&engine).ok();
    }
}
