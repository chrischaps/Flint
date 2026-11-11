//! On-disk persistence for the script-facing `PersistentStore`.
//!
//! The store lives at `<project root>/save/persist.toml`, the root being
//! the scene's project's (`Project::root_for`, ADR 0114): the folder with
//! the `flint.toml`, whichever of the project's scenes is playing. Where a
//! game has no project marker it is the scene directory's parent, as it
//! always was (`scenes/menu.scene.toml` -> `./save/persist.toml`). On
//! Android the app's internal files directory is the root -- the same
//! writable tree the APK assets are extracted into. #726 moves the store
//! under the OS user directory, keyed on `[project] id`.
//!
//! It is read once at startup (a missing file is simply an empty store) and
//! written: 1 s after the last `persist_set` / `persist_remove` / `persist_clear`
//! (one write per burst of changes), on every scene transition, on exit, and
//! immediately when a script calls `persist_save()`.
//!
//! TEMPORARY, for one release (#765): before the project was asked, the
//! root was guessed from the scene alone, so a scene two folders deep (or
//! with a `.flint/` beside it) kept its store one folder off. When there is
//! no store at the root and there is one at that old place, it is read from
//! there, and saved at the root from then on (`Project::old_place`).

use flint_core::project::Project;
use flint_core::vfs;
use std::path::{Path, PathBuf};
use web_time::Instant;

use super::PlayerApp;

/// Name of the save directory under the project root.
pub const SAVE_DIR: &str = "save";
/// Name of the store file inside [`SAVE_DIR`].
pub const SAVE_FILE: &str = "persist.toml";

/// The store's path from the project root.
fn save_file() -> PathBuf {
    Path::new(SAVE_DIR).join(SAVE_FILE)
}

fn scene_dir(scene_path: &str) -> &Path {
    Path::new(scene_path)
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
}

/// Where the engine keeps the persistent store for a scene of `project`:
/// the same file for every scene of the project.
pub fn persist_path(project: &Project, scene_path: &str) -> PathBuf {
    save_root(project, scene_path).join(save_file())
}

/// TEMPORARY, for one release: the store at the root guessed before the
/// project was asked, when it is another place, holds a store, and the
/// project's root holds none.
pub fn old_persist_path(project: &Project, scene_path: &str) -> Option<PathBuf> {
    if cfg!(target_os = "android") {
        return None;
    }
    project.old_place(save_file(), scene_dir(scene_path))
}

/// The root saves are written under, on the desktop: the project's.
#[cfg(not(target_os = "android"))]
fn save_root(project: &Project, scene_path: &str) -> PathBuf {
    project.root_for(scene_dir(scene_path))
}

/// The root saves are written under.
///
/// Android: the app's internal files directory (`.../<package>/files`),
/// which `flint-android` extracts the APK assets into and which is the only
/// tree guaranteed writable. Scenes are loaded from under it, so it is found
/// by walking up from the scene; if the scene somehow lives elsewhere the
/// desktop rule applies.
#[cfg(target_os = "android")]
fn save_root(project: &Project, scene_path: &str) -> PathBuf {
    if let Some(files_dir) = Path::new(scene_path)
        .ancestors()
        .find(|p| p.file_name().is_some_and(|n| n == "files"))
    {
        return files_dir.to_path_buf();
    }
    project.root_for(scene_dir(scene_path))
}

/// Where the store lives on the web (#104): a browser has no project folder
/// to write `save/` into, and every game on one site shares its
/// `localStorage`, so each game's store is its own [`user_files`] entry,
/// named for the input config's `game_id`:
/// `/config/flint/save_<game_id>.toml` (`flint:/config/flint/save_flint_kart.toml`).
/// A game with no id shares `save.toml`.
///
/// [`user_files`]: crate::user_files
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub fn web_persist_path(game_id: &str) -> PathBuf {
    let game_id = game_id.trim();
    let filename = if game_id.is_empty() || game_id == "flint" {
        "save.toml".to_string()
    } else {
        format!("save_{game_id}.toml")
    };
    crate::user_files::config_dir()
        .unwrap_or_else(|| PathBuf::from("/config"))
        .join("flint")
        .join(filename)
}

impl PlayerApp {
    /// Read `save/persist.toml` into the store. A missing file is not an
    /// error and creates nothing; a malformed one is logged and ignored so a
    /// corrupt save never blocks the game from starting.
    pub(super) fn load_persistent_store(&mut self) {
        #[cfg(target_arch = "wasm32")]
        {
            let path = web_persist_path(&self.input.config().game_id);
            let loaded = crate::user_files::read(&path)
                .map_err(|e| e.to_string())
                .and_then(|text| match text {
                    Some(text) => self.persistent_store.load_from_str(&text).map(|()| true),
                    None => Ok(false),
                });
            match loaded {
                Ok(true) => tracing::info!(
                    "persist: loaded {} keys from {}",
                    self.persistent_store.keys().len(),
                    crate::user_files::storage_key(&path)
                ),
                Ok(false) => tracing::info!(
                    "persist: nothing saved at {} (fresh store)",
                    crate::user_files::storage_key(&path)
                ),
                Err(e) => tracing::warn!("persist: failed to load {}: {e}", path.display()),
            }
            return;
        }
        #[allow(unreachable_code)]
        self.refresh_project();
        let mut path = persist_path(&self.project, &self.scene_path);
        if !vfs::is_file(&path) {
            // TEMPORARY, for one release: the store from where the root
            // used to be guessed
            match old_persist_path(&self.project, &self.scene_path) {
                Some(old) => {
                    tracing::warn!(
                        "persist: no save file at {}; reading the one at {}, where the \
                         project root used to be guessed. It is saved at the first from \
                         now on, and the old place is read for one release",
                        path.display(),
                        old.display()
                    );
                    path = old;
                }
                None => {
                    tracing::info!("persist: no save file at {} (fresh store)", path.display());
                    return;
                }
            }
        }
        match self.persistent_store.load_from_file(&path) {
            Ok(()) => tracing::info!(
                "persist: loaded {} keys from {}",
                self.persistent_store.keys().len(),
                path.display()
            ),
            Err(e) => tracing::warn!("persist: failed to load {}: {e}", path.display()),
        }
    }

    /// Write the store to `save/persist.toml` now, creating `save/` on
    /// demand. `reason` is for the log line only.
    pub(super) fn save_persistent_store(&mut self, reason: &str) {
        #[cfg(target_arch = "wasm32")]
        {
            let path = web_persist_path(&self.input.config().game_id);
            let saved = self
                .persistent_store
                .to_toml_string()
                .and_then(|text| crate::user_files::write(&path, &text).map_err(|e| e.to_string()));
            match saved {
                Ok(()) => {
                    self.persistent_store.mark_clean();
                    tracing::info!(
                        "persist: saved {} keys to {} ({reason})",
                        self.persistent_store.keys().len(),
                        crate::user_files::storage_key(&path)
                    );
                }
                // Mark it clean anyway: a full or blocked localStorage would
                // otherwise retry (and warn) every second for the session.
                Err(e) => {
                    self.persistent_store.mark_clean();
                    tracing::warn!("persist: failed to save {}: {e}", path.display());
                }
            }
            return;
        }
        #[allow(unreachable_code)]
        let path = persist_path(&self.project, &self.scene_path);
        if let Some(dir) = path.parent() {
            if let Err(e) = std::fs::create_dir_all(dir) {
                tracing::warn!("persist: cannot create {}: {e}", dir.display());
                return;
            }
        }
        match self.persistent_store.save_to_file(&path) {
            Ok(()) => {
                self.persistent_store.mark_clean();
                tracing::info!(
                    "persist: saved {} keys to {} ({reason})",
                    self.persistent_store.keys().len(),
                    path.display()
                );
            }
            Err(e) => tracing::warn!("persist: failed to save {}: {e}", path.display()),
        }
    }

    /// Save if the store changed and either the debounce window has passed
    /// or `force` is set (transition / exit). A script's `persist_save()`
    /// also forces.
    pub(super) fn flush_persistent_store(&mut self, force: bool, reason: &str) {
        let now = Instant::now();
        if self.persistent_store.is_dirty() {
            self.persist_debounce.note_dirty(now);
        }
        let force = force || self.persistent_store.take_flush_request();
        if self.persist_debounce.should_flush(now, force) {
            self.save_persistent_store(reason);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn web_persist_path_is_one_entry_per_game() {
        let kart = web_persist_path("flint_kart");
        assert_eq!(kart.file_name().unwrap(), "save_flint_kart.toml");
        assert_eq!(kart.parent().unwrap().file_name().unwrap(), "flint");
        assert_ne!(kart, web_persist_path("float"));
        assert_eq!(web_persist_path("").file_name().unwrap(), "save.toml");
        assert_eq!(web_persist_path("flint"), web_persist_path(" "));
    }

    /// Run `f` over files in memory (a path ending in `/` is a folder).
    fn with_files<R>(files: &[&str], f: impl FnOnce() -> R) -> R {
        let mut source = vfs::MemorySource::new();
        for path in files {
            if path.ends_with('/') {
                source.insert(format!("{path}.keep"), "");
            } else {
                source.insert(path, "");
            }
        }
        vfs::scoped(std::sync::Arc::new(source), f)
    }

    fn paths(scene: &str) -> (PathBuf, Option<PathBuf>) {
        let project = Project::discover_with_home(Path::new(scene), None).unwrap();
        (
            persist_path(&project, scene),
            old_persist_path(&project, scene),
        )
    }

    /// The layouts games use today: the store stays exactly where it was,
    /// `<the scene folder's parent>/save/persist.toml`, and there is no old
    /// place to read.
    #[test]
    fn a_scene_in_scenes_or_levels_keeps_its_save_file() {
        let cases: [(&[&str], &str); 7] = [
            // No marker at all
            (
                &["proj/scenes/menu.scene.toml"],
                "proj/scenes/menu.scene.toml",
            ),
            (&["scenes/menu.scene.toml"], "scenes/menu.scene.toml"),
            (&["menu.scene.toml"], "menu.scene.toml"),
            // A manifest, the layout, a state directory
            (
                &["proj/flint.toml", "proj/scenes/menu.scene.toml"],
                "proj/scenes/menu.scene.toml",
            ),
            (
                &["proj/schemas/", "proj/levels/menu.scene.toml"],
                "proj/levels/menu.scene.toml",
            ),
            (
                &["proj/.flint/", "proj/scenes/menu.scene.toml"],
                "proj/scenes/menu.scene.toml",
            ),
            (
                &["flint.toml", "levels/menu.scene.toml"],
                "levels/menu.scene.toml",
            ),
        ];
        for (files, scene) in cases {
            with_files(files, || {
                let scene_dir = Path::new(scene).parent().unwrap();
                let was = flint_core::project::guessed_root(scene_dir)
                    .join("save")
                    .join("persist.toml");
                let (now, old) = paths(scene);
                assert_eq!(now, was, "{scene} in {files:?}");
                assert_eq!(old, None);
            });
        }
    }

    /// #726's first box, as far as the root goes: the store is one file for
    /// the whole project, so a transition into a deeper scene no longer
    /// moves it.
    #[test]
    fn every_scene_of_a_project_shares_one_save_file() {
        with_files(
            &[
                "proj/flint.toml",
                "proj/scenes/menu.scene.toml",
                "proj/scenes/levels/one.scene.toml",
            ],
            || {
                let (menu, _) = paths("proj/scenes/menu.scene.toml");
                let (level, old) = paths("proj/scenes/levels/one.scene.toml");
                assert_eq!(menu, Path::new("proj").join("save").join("persist.toml"));
                assert_eq!(level, menu);
                assert_eq!(old, None, "nothing was saved at the old place");
            },
        );
    }

    /// TEMPORARY with `old_persist_path`: a store only at the old place is
    /// read; one at the root wins.
    #[test]
    fn a_save_at_the_old_place_is_read_until_one_is_at_the_root() {
        let deep = "proj/scenes/levels/one.scene.toml";
        let old_file = "proj/scenes/save/persist.toml";
        with_files(&["proj/flint.toml", deep, old_file], || {
            let (now, old) = paths(deep);
            assert_eq!(now, Path::new("proj").join("save").join("persist.toml"));
            assert_eq!(
                old,
                Some(Path::new("proj/scenes").join("save/persist.toml"))
            );
        });
        with_files(
            &["proj/flint.toml", deep, old_file, "proj/save/persist.toml"],
            || assert_eq!(paths(deep).1, None),
        );
        // A `.flint/` beside the scenes (an old procgen cache) makes that
        // folder the root; the store that was read from its parent still is
        with_files(
            &[
                "loose/levels/.flint/",
                "loose/levels/a.scene.toml",
                "loose/save/persist.toml",
            ],
            || {
                let (now, old) = paths("loose/levels/a.scene.toml");
                assert_eq!(
                    now,
                    Path::new("loose/levels").join("save").join("persist.toml")
                );
                assert_eq!(old, Some(Path::new("loose").join("save/persist.toml")));
            },
        );
    }

    /// The whole path, through the player: read from the old place, saved
    /// at the root, and read from the root the next time.
    #[test]
    fn the_player_reads_the_old_save_and_writes_the_new() {
        if flint_core::project::overridden().is_some() {
            return;
        }
        let root = std::env::temp_dir().join(format!("flint_persist_706_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let deep = root.join("scenes").join("levels");
        std::fs::create_dir_all(&deep).unwrap();
        std::fs::create_dir_all(root.join("scenes").join("save")).unwrap();
        std::fs::write(root.join("flint.toml"), "[project]\nid = \"saves\"\n").unwrap();
        let scene = deep.join("one.scene.toml");
        std::fs::write(&scene, "[scene]\nname = \"one\"\n").unwrap();
        let old = root.join("scenes").join("save").join("persist.toml");
        std::fs::write(&old, "coins = 7\n").unwrap();
        let new = root.join("save").join("persist.toml");

        let app = || {
            PlayerApp::new(
                flint_ecs::FlintWorld::new(),
                scene.to_string_lossy().into_owned(),
                false,
                None,
                None,
            )
        };
        let coins = |app: &PlayerApp| {
            app.persistent_store
                .get("coins")
                .and_then(|v| v.as_integer())
        };
        let mut first = app();
        first.load_persistent_store();
        assert_eq!(coins(&first), Some(7), "read from the old place");
        assert!(!new.exists());
        first.persistent_store.set("coins", toml::Value::Integer(8));
        first.save_persistent_store("test");
        assert!(new.is_file(), "saved at the project root");
        assert_eq!(std::fs::read_to_string(&old).unwrap(), "coins = 7\n");

        let mut second = app();
        second.load_persistent_store();
        assert_eq!(
            coins(&second),
            Some(8),
            "the root's store wins once it exists"
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}
