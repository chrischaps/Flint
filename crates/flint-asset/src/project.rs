//! Where a Flint project's asset state lives (#179).
//!
//! The content store (`.flint/assets`), the procgen provenance index
//! (`.flint/procgen`), the job store, the project config and the sidecar
//! catalog (`assets/`) all belong to the project, not to whichever folder a
//! command happens to run from.
//!
//! The project itself is [`flint_core::project::Project`] (ADR 0114, #706),
//! re-exported here: it finds the root by walking up to a `flint.toml`, a
//! `.flint/` directory or the `schemas/` + `scenes/` layout, and reads the
//! manifest's `[project]` table (`id`, `schemas`, `scene`, `input`). The
//! table is parsed and checked; nothing consumes `schemas`, `scene` or
//! `input` yet (the schema resolver and the asset resolver are ADR 0114's
//! next steps). See that module for the markers and the manifest.
//!
//! This module adds what the asset tooling needs on top: [`ProjectStore`],
//! the paths of the state a project owns. Bring the trait into scope to call
//! them (`project.store_dir()`, `Project::display(path)`).
//!
//! This is tooling: the stores are read and written on the real disk with
//! `std::fs`, and the player does not use them.

use std::path::{Path, PathBuf};

pub use flint_core::project::{set_override, Found, Project, ProjectError};

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

/// The paths of the asset state a [`Project`] owns.
pub trait ProjectStore {
    fn store_dir(&self) -> PathBuf;
    fn procgen_dir(&self) -> PathBuf;
    fn jobs_dir(&self) -> PathBuf;
    fn generated_dir(&self) -> PathBuf;
    fn config_file(&self) -> PathBuf;
    fn catalog_dir(&self) -> PathBuf;

    /// `path` shown relative to the working directory (`.flint\assets`,
    /// `..\.flint\assets`), so a message says where the store is from here.
    /// Absolute when the two share no ancestor (another drive).
    fn display(path: &Path) -> String
    where
        Self: Sized,
    {
        match std::env::current_dir() {
            Ok(cwd) => relative_to(path, &cwd).display().to_string(),
            Err(_) => path.display().to_string(),
        }
    }
}

impl ProjectStore for Project {
    fn store_dir(&self) -> PathBuf {
        join_rel(&self.root, STORE_DIR)
    }
    fn procgen_dir(&self) -> PathBuf {
        join_rel(&self.root, PROCGEN_DIR)
    }
    fn jobs_dir(&self) -> PathBuf {
        join_rel(&self.root, JOBS_DIR)
    }
    fn generated_dir(&self) -> PathBuf {
        join_rel(&self.root, GENERATED_DIR)
    }
    fn config_file(&self) -> PathBuf {
        join_rel(&self.root, CONFIG_FILE)
    }
    fn catalog_dir(&self) -> PathBuf {
        join_rel(&self.root, CATALOG_DIR)
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("flint_project_{tag}_{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn discover(start: &Path, home: Option<&Path>) -> Project {
        Project::discover_with_home(start, home).unwrap()
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
        let p = discover(&root.join("specs/deep"), None);
        assert_eq!(p.root, root);
        assert_eq!(p.found, Found::Layout);
        assert_eq!(discover(&root, None).root, root);
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn nearest_marker_wins() {
        let outer = tmp("nearest");
        fs::write(outer.join("flint.toml"), "").unwrap();
        let inner = outer.join("sub");
        fs::create_dir_all(inner.join(".flint")).unwrap();
        fs::create_dir_all(inner.join("x")).unwrap();
        let p = discover(&inner.join("x"), None);
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
        let p = discover(&start, Some(&home));
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
        let p = discover(&engine.join("specs"), None);
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
        let p = discover(&engine.join("specs"), None);
        assert_eq!(p.root, engine);
        assert_eq!(p.found, Found::Engine);
        assert_eq!(p.store_dir(), engine.join(".flint/assets"));
        fs::remove_dir_all(&engine).ok();
    }

    #[test]
    fn a_manifest_on_disk_is_read_and_the_store_sits_at_its_root() {
        let game = tmp("manifest");
        fs::write(
            game.join("flint.toml"),
            "[project]\nid = \"float\"\nschemas = [\"engine/schemas\", \"schemas\"]\n",
        )
        .unwrap();
        fs::create_dir_all(game.join("scenes/deep")).unwrap();
        fs::write(game.join("scenes/deep/a.scene.toml"), "").unwrap();
        let p = discover(&game.join("scenes/deep/a.scene.toml"), None);
        assert_eq!(p.root, game);
        assert_eq!(p.found, Found::Manifest);
        assert_eq!(p.id.as_deref(), Some("float"));
        assert_eq!(p.schemas.len(), 2);
        assert_eq!(p.catalog_dir(), game.join("assets"));
        assert_eq!(p.config_file(), game.join(".flint").join("config.toml"));

        fs::write(game.join("flint.toml"), "[project]\nidd = \"float\"\n").unwrap();
        let err = Project::discover_with_home(&game, None).unwrap_err();
        assert!(err.to_string().contains("flint.toml"), "{err}");
        assert!(err.to_string().contains("`idd`"), "{err}");
        fs::remove_dir_all(&game).ok();
    }
}
