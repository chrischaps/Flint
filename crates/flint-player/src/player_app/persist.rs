//! On-disk persistence for the script-facing `PersistentStore`.
//!
//! The store lives at `<project_root>/save/persist.toml`, where the project
//! root follows the same rule as `fonts/` and `sprites/`: the scene's
//! directory's parent (`scenes/menu.scene.toml` -> `./save/persist.toml`).
//! On Android the app's internal files directory is the root -- the same
//! writable tree the APK assets are extracted into.
//!
//! It is read once at startup (a missing file is simply an empty store) and
//! written: 1 s after the last `persist_set` / `persist_remove` / `persist_clear`
//! (one write per burst of changes), on every scene transition, on exit, and
//! immediately when a script calls `persist_save()`.

use std::path::{Path, PathBuf};
use std::time::Instant;

use super::PlayerApp;

/// Name of the save directory under the project root.
pub const SAVE_DIR: &str = "save";
/// Name of the store file inside [`SAVE_DIR`].
pub const SAVE_FILE: &str = "persist.toml";

/// Where the engine keeps the persistent store for the project a scene
/// belongs to.
pub fn persist_path(scene_path: &str) -> PathBuf {
    project_root(Path::new(scene_path))
        .join(SAVE_DIR)
        .join(SAVE_FILE)
}

/// Project root for a scene path.
///
/// Desktop: the scene directory's parent (`scenes/` sits directly under the
/// project). A relative `scenes/x.scene.toml` has the empty path as that
/// parent, which is the current directory, not "no parent".
#[cfg(not(target_os = "android"))]
fn project_root(scene_path: &Path) -> PathBuf {
    parent_dir_or_cwd(scene_path)
}

fn parent_dir_or_cwd(scene_path: &Path) -> PathBuf {
    let scene_dir = scene_path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    match scene_dir.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent.to_path_buf(),
        Some(_) => PathBuf::from("."),
        None => scene_dir.to_path_buf(),
    }
}

/// Project root for a scene path.
///
/// Android: the app's internal files directory (`.../<package>/files`),
/// which `flint-android` extracts the APK assets into and which is the only
/// tree guaranteed writable. Scenes are loaded from under it, so it is found
/// by walking up from the scene; if the scene somehow lives elsewhere the
/// desktop rule applies.
#[cfg(target_os = "android")]
fn project_root(scene_path: &Path) -> PathBuf {
    if let Some(files_dir) = scene_path
        .ancestors()
        .find(|p| p.file_name().is_some_and(|n| n == "files"))
    {
        return files_dir.to_path_buf();
    }
    parent_dir_or_cwd(scene_path)
}

impl PlayerApp {
    /// Read `save/persist.toml` into the store. A missing file is not an
    /// error and creates nothing; a malformed one is logged and ignored so a
    /// corrupt save never blocks the game from starting.
    pub(super) fn load_persistent_store(&mut self) {
        let path = persist_path(&self.scene_path);
        if !path.is_file() {
            tracing::info!("persist: no save file at {} (fresh store)", path.display());
            return;
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
        let path = persist_path(&self.scene_path);
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
    fn persist_path_is_project_root_save_dir() {
        let p = persist_path("proj/scenes/menu.scene.toml");
        assert_eq!(p, Path::new("proj").join("save").join("persist.toml"));
    }

    #[test]
    fn persist_path_relative_scenes_dir_resolves_to_cwd() {
        // `flint-player scenes/menu.scene.toml` from the project root: the
        // scene dir's parent is the empty path, i.e. the current directory.
        let p = persist_path("scenes/menu.scene.toml");
        assert_eq!(p, Path::new(".").join("save").join("persist.toml"));
    }

    #[test]
    fn persist_path_for_scene_at_root_stays_beside_it() {
        let p = persist_path("menu.scene.toml");
        assert_eq!(p, Path::new(".").join("save").join("persist.toml"));
    }

    #[test]
    fn persist_path_absolute() {
        let root = std::env::temp_dir().join("flint_proj");
        let scene = root.join("scenes").join("menu.scene.toml");
        let p = persist_path(scene.to_str().unwrap());
        assert_eq!(p, root.join("save").join("persist.toml"));
    }
}
