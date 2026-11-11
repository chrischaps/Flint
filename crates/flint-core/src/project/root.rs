//! Where a path "from the project root" starts (ADR 0114; #706).
//!
//! Some things are not searched for: a script's `load_ui("ui/hud.ui.toml")`
//! and `load_data("data/karts.toml")`, a `music_session`'s suite, the save
//! file, the procgen disk cache and the project-local input override are
//! each one path under the project root. Before there were projects each
//! of them guessed that root as "the scene directory's parent", in its own
//! copy. They all ask [`Project::root_for`] now, and the guess lives here
//! and nowhere else, as [`guessed_root`]:
//!
//! - it is still the answer for a project found by no marker at all
//!   ([`Found::Fallback`]): with no manifest the engine behaves as it did,
//!   as the asset search does;
//! - TEMPORARY, for one release (ADR 0114 decisions 3 and 5, #765): where
//!   the root and the guess differ (a scene two folders deep, a `.flint/`
//!   beside the scenes) a file that is only at the guessed place is still
//!   read from there, with a warning saying where it belongs
//!   ([`Project::root_holding`], [`Project::old_place`]).
//!
//! Where saves *live* is #726's question: it moves them under the OS user
//! directory, keyed on `[project] id`. Until then they are
//! `<root_for>/save/persist.toml`, and #726 changes that one caller
//! (`flint-player`'s `persist.rs`), not this module.

use super::{same_path, Found, Project};
use crate::vfs;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

/// The project root as every loader guessed it before there were projects:
/// the scene directory's parent (`scenes/menu.scene.toml` is in `scenes`,
/// whose parent is the working directory, `.`). Nothing is read. A
/// directory with no parent is its own root.
///
/// The one copy of that guess. [`Project::root_for`] uses it for a project
/// with no marker; the old-place reads use it for one release.
pub fn guessed_root(scene_dir: &Path) -> PathBuf {
    let scene_dir = if scene_dir.as_os_str().is_empty() {
        Path::new(".")
    } else {
        scene_dir
    };
    match scene_dir.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent.to_path_buf(),
        // `scenes` and `.` both have the empty path above them: the
        // working directory, not "no parent"
        Some(_) => PathBuf::from("."),
        None => scene_dir.to_path_buf(),
    }
}

impl Project {
    /// The directory a path written "from the project root" is joined to,
    /// for a scene in `scene_dir`: the project's root. A project found by
    /// no marker ([`Found::Fallback`]) has only the scene's own directory
    /// for a root, and keeps the root the engine guessed before there were
    /// projects, [`guessed_root`]: with no manifest the engine behaves as
    /// it did.
    ///
    /// What the engine writes under a root (saves, `.flint/`) goes here
    /// too, so a `.flint/` it makes marks the root it was already using.
    pub fn root_for(&self, scene_dir: &Path) -> PathBuf {
        match self.found {
            Found::Fallback => guessed_root(scene_dir),
            _ => self.root.clone(),
        }
    }

    /// TEMPORARY, for one release (#765): `rel` under the root the engine
    /// guessed before there were projects, when that is another place than
    /// [`root_for`](Self::root_for), the file is there, and no file is at
    /// the new place. `None` otherwise, which is nearly always: the two
    /// roots are the same directory for a scene directly in `scenes/` or
    /// `levels/`.
    pub fn old_place(&self, rel: impl AsRef<Path>, scene_dir: &Path) -> Option<PathBuf> {
        let rel = rel.as_ref();
        let old_root = guessed_root(scene_dir);
        let new_root = self.root_for(scene_dir);
        if same_path(&old_root, &new_root) || vfs::is_file(new_root.join(rel)) {
            return None;
        }
        let old = old_root.join(rel);
        vfs::is_file(&old).then_some(old)
    }

    /// The root to read `rel` under: [`root_for`](Self::root_for), or,
    /// TEMPORARY for one release, the old guessed root when the file is
    /// only there ([`old_place`](Self::old_place)), with a warning once a
    /// process saying where `what` belongs now. For a loader that joins
    /// more than one path to the root it gets (a UI document and its style
    /// sheet, a music suite and its chart).
    pub fn root_holding(&self, what: &str, rel: impl AsRef<Path>, scene_dir: &Path) -> PathBuf {
        let rel = rel.as_ref();
        let root = self.root_for(scene_dir);
        let Some(old) = self.old_place(rel, scene_dir) else {
            return root;
        };
        let new = root.join(rel);
        if first_time(&old) {
            tracing::warn!(
                "{what}: {} is read from where the project root used to be guessed (the \
                 scene folder's parent). The project root is {} ({}): move it to {}. The \
                 old place is read for one release",
                super::shown(&old),
                super::shown(&root),
                self.found.describe(),
                super::shown(&new),
            );
        }
        guessed_root(scene_dir)
    }
}

/// Whether this old-place read is reported for the first time.
fn first_time(old: &Path) -> bool {
    static SEEN: OnceLock<Mutex<HashSet<PathBuf>>> = OnceLock::new();
    SEEN.get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(old.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vfs::MemorySource;
    use std::sync::Arc;

    fn with_files<R>(files: &[&str], f: impl FnOnce() -> R) -> R {
        let mut source = MemorySource::new();
        for path in files {
            if path.ends_with('/') {
                source.insert(format!("{path}.keep"), "");
            } else {
                source.insert(path, "");
            }
        }
        vfs::scoped(Arc::new(source), f)
    }

    fn project(scene: &str) -> Project {
        Project::discover_with_home(Path::new(scene), None).unwrap()
    }

    #[test]
    fn the_guess_is_the_scene_folders_parent() {
        for (scene_dir, root) in [
            ("scenes", "."),
            ("", "."),
            (".", "."),
            ("game/scenes", "game"),
            ("game/scenes/act1", "game/scenes"),
            ("/", "/"),
        ] {
            assert_eq!(
                guessed_root(Path::new(scene_dir)),
                Path::new(root),
                "{scene_dir:?}"
            );
        }
    }

    /// The layouts games use today: a scene directly in `scenes/` or
    /// `levels/`. The root is where the guess was, by every marker, so
    /// nothing a game wrote moves.
    #[test]
    fn a_scene_in_scenes_or_levels_keeps_its_root() {
        for (files, scene, found) in [
            (
                &["game/flint.toml", "game/scenes/a.scene.toml"][..],
                "game/scenes/a.scene.toml",
                Found::Manifest,
            ),
            (
                &["game/schemas/", "game/levels/a.scene.toml"][..],
                "game/levels/a.scene.toml",
                Found::Layout,
            ),
            (
                &["game/.flint/", "game/scenes/a.scene.toml"][..],
                "game/scenes/a.scene.toml",
                Found::StateDir,
            ),
            (
                &[
                    "flint/crates/flint-core/Cargo.toml",
                    "flint/examples/a.scene.toml",
                ][..],
                "flint/examples/a.scene.toml",
                Found::Engine,
            ),
            // No marker: the guess itself
            (
                &["loose/levels/a.scene.toml"][..],
                "loose/levels/a.scene.toml",
                Found::Fallback,
            ),
        ] {
            with_files(files, || {
                let p = project(scene);
                assert_eq!(p.found, found);
                let scene_dir = Path::new(scene).parent().unwrap();
                assert_eq!(
                    p.root_for(scene_dir),
                    guessed_root(scene_dir),
                    "{scene} ({found:?})"
                );
                assert_eq!(p.old_place("save/persist.toml", scene_dir), None);
            });
        }
    }

    /// The root no longer moves with the scene: two folders deep it is
    /// still the project's (the guess was `scenes/`).
    #[test]
    fn a_nested_scene_has_the_projects_root() {
        with_files(
            &[
                "game/flint.toml",
                "game/scenes/a.scene.toml",
                "game/scenes/act1/b.scene.toml",
            ],
            || {
                let p = project("game/scenes/act1/b.scene.toml");
                assert_eq!(p, project("game/scenes/a.scene.toml"));
                assert_eq!(p.root_for(Path::new("game/scenes/act1")), Path::new("game"));
                assert_eq!(p.root_for(Path::new("game/scenes")), Path::new("game"));
                assert_eq!(
                    guessed_root(Path::new("game/scenes/act1")),
                    Path::new("game/scenes")
                );
            },
        );
    }

    #[test]
    fn a_file_only_at_the_old_place_is_still_read_until_the_new_one_exists() {
        let nested = Path::new("game/scenes/act1");
        // Only where the guess put it
        with_files(
            &[
                "game/flint.toml",
                "game/scenes/act1/b.scene.toml",
                "game/scenes/ui/hud.ui.toml",
            ],
            || {
                let p = project("game/scenes/act1/b.scene.toml");
                assert_eq!(
                    p.old_place("ui/hud.ui.toml", nested),
                    Some(PathBuf::from("game/scenes/ui/hud.ui.toml"))
                );
                assert_eq!(
                    p.root_holding("load_ui", "ui/hud.ui.toml", nested),
                    Path::new("game/scenes")
                );
                // A file that is nowhere: the root, for the loader's error
                assert_eq!(p.old_place("ui/none.ui.toml", nested), None);
                assert_eq!(
                    p.root_holding("load_ui", "ui/none.ui.toml", nested),
                    Path::new("game")
                );
            },
        );
        // At both: the root's
        with_files(
            &[
                "game/flint.toml",
                "game/scenes/act1/b.scene.toml",
                "game/scenes/ui/hud.ui.toml",
                "game/ui/hud.ui.toml",
            ],
            || {
                let p = project("game/scenes/act1/b.scene.toml");
                assert_eq!(p.old_place("ui/hud.ui.toml", nested), None);
                assert_eq!(
                    p.root_holding("load_ui", "ui/hud.ui.toml", nested),
                    Path::new("game")
                );
            },
        );
    }

    /// A `.flint/` beside the scenes (an old procgen cache, a shipped input
    /// override) makes that folder the root; what was read from its parent
    /// still is.
    #[test]
    fn a_state_dir_beside_the_scenes_reads_the_parents_files() {
        with_files(
            &[
                "loose/levels/.flint/",
                "loose/levels/a.scene.toml",
                "loose/save/persist.toml",
            ],
            || {
                let dir = Path::new("loose/levels");
                let p = project("loose/levels/a.scene.toml");
                assert_eq!(p.found, Found::StateDir);
                assert_eq!(p.root_for(dir), dir);
                assert_eq!(
                    p.old_place("save/persist.toml", dir),
                    Some(PathBuf::from("loose/save/persist.toml"))
                );
            },
        );
    }
}
