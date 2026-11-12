//! Where the script-facing `PersistentStore` is saved, and when (#726).
//!
//! # Where
//!
//! Decided once, when the store is first loaded, and kept for the whole
//! run: no scene transition moves it. In order:
//!
//! 1. **`--save-dir <dir>`** (`PlayerApp::save_dir`): `<dir>/<slot>.toml`,
//!    and nothing else is read.
//! 2. **A game with an id**: `<config>/<id>/saves/<slot>.toml`, `<config>`
//!    being the OS user directory ([`user_files::config_dir`]:
//!    `%APPDATA%`, `~/.config`, `~/Library/Application Support`) and the id
//!    `[project] id` in `flint.toml` (ADR 0114 decision 5), else the input
//!    config's `game_id` when that is set to something other than the
//!    engine's `flint`. Nothing is written into the game's own folder, so a
//!    game installed read-only can save.
//! 3. **No id at all** (the engine's examples, a loose scene), or an id
//!    that can't be a folder name: `<project root>/save/<slot>.toml` as
//!    before, the root being `Project::root_for` of the scene the run
//!    began in. No id is made up from a folder name: two games would share
//!    one.
//!
//! The default slot's file is `default.toml`, except in the project folder
//! (case 3, and Android), where it is the `persist.toml` it always was.
//!
//! **Android** keeps the place it had: the app's internal files directory
//! (`.../<package>/files/save/`), found as the scene's `files` ancestor.
//! It is the app's own private, writable user directory already.
//!
//! **The web** has no files: the default slot is the `localStorage` entry
//! it always was, `flint:/config/flint/save_<id>.toml` (#104), and a named
//! slot is `flint:/config/flint/saves_<id>/<slot>.toml`.
//!
//! # TEMPORARY, for one release (#765): the old places
//!
//! When the default slot has no file where it is saved now, it is read
//! once from where it used to be: `<project root>/save/persist.toml`, then
//! the root guessed before #706 (`Project::old_place`, `old_persist_path`);
//! on the web, the entry keyed on the input config's `game_id` when the
//! manifest's id is another. One warning says where the save now lives, it
//! is written there within a second, and the old file is left as it is.
//!
//! # When
//!
//! Read once at startup, before any script's `on_init` (a missing file is
//! an empty store). Written 1 s after the last change (one write per burst
//! of changes), on every scene transition, on exit, at once when a script
//! calls `persist_save()`, and when a script switches slots. Every write is
//! a temp file renamed over the target (`flint_runtime::atomic_file`).
//!
//! A file that doesn't parse is renamed `<file>.bak` (`.bak.1`, `.bak.2`,
//! ... when that is taken) before anything can be saved over it, an error
//! naming both paths is logged, and scripts read it from
//! `persist_corrupt()`.

use crate::user_files;
use flint_core::project::Project;
use flint_core::vfs;
use flint_runtime::{check_slot_name, SaveBackend, SaveText, DEFAULT_SLOT};
use std::path::{Path, PathBuf};
use web_time::Instant;

use super::PlayerApp;

/// Name of the save directory under the project root.
pub const SAVE_DIR: &str = "save";
/// Name of the default slot's file inside [`SAVE_DIR`].
pub const SAVE_FILE: &str = "persist.toml";
/// Name of the folder slots are kept in under `<config>/<id>/`.
pub const USER_SAVE_DIR: &str = "saves";

/// The default slot's path from the project root, where saves are kept in
/// the project folder.
fn save_file() -> PathBuf {
    Path::new(SAVE_DIR).join(SAVE_FILE)
}

fn scene_dir(scene_path: &str) -> &Path {
    Path::new(scene_path)
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
}

/// The default slot's file in the project folder: where every save was
/// kept before #726, and where a game with no id still keeps them. The same
/// file for every scene of the project.
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

/// The root in-project saves are written under, on the desktop: the
/// project's.
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

/// The default slot on the web (#104): a browser has no project folder
/// to write `save/` into, and every game on one site shares its
/// `localStorage`, so each game's store is its own [`user_files`] entry,
/// named for the game's id:
/// `/config/flint/save_<game_id>.toml` (`flint:/config/flint/save_flint_kart.toml`).
/// A game with no id shares `save.toml`.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub fn web_persist_path(game_id: &str) -> PathBuf {
    web_dir().join(format!("save{}.toml", web_suffix(game_id)))
}

/// Where a game's named slots are on the web:
/// `/config/flint/saves_<game_id>/<slot>.toml` (`saves/` with no id). A
/// folder of its own, so no slot can be taken for another game's store.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub fn web_slot_dir(game_id: &str) -> PathBuf {
    web_dir().join(format!("saves{}", web_suffix(game_id)))
}

fn web_dir() -> PathBuf {
    user_files::config_dir()
        .unwrap_or_else(|| PathBuf::from("/config"))
        .join("flint")
}

fn web_suffix(game_id: &str) -> String {
    let game_id = game_id.trim();
    if game_id.is_empty() || game_id == "flint" {
        String::new()
    } else {
        format!("_{game_id}")
    }
}

/// `id` as the name of a game's folder under the user directory, or why it
/// can't be one. An id is whatever the manifest says, so it is checked
/// here rather than trusted: letters, digits, `_`, `-`, `.` and spaces, no
/// leading dot, no trailing dot or space, none of Windows' device names.
fn user_folder_name(id: &str) -> Result<&str, String> {
    let name = id.trim();
    let ok_char = |c: char| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | ' ');
    if name.is_empty() || name.len() > 64 {
        return Err("it is empty or longer than 64 characters".into());
    }
    if !name.chars().all(ok_char) {
        return Err("a folder is named with letters, digits, _, -, . and spaces".into());
    }
    // All ASCII from here on
    let stem = name
        .split('.')
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    let device = ["con", "prn", "aux", "nul"].contains(&stem.as_str())
        || (stem.len() == 4
            && (stem.starts_with("com") || stem.starts_with("lpt"))
            && stem.ends_with(|c: char| c.is_ascii_digit()));
    if name.starts_with('.') || name.ends_with('.') || device {
        return Err("the name is reserved".into());
    }
    Ok(name)
}

/// The id saves are keyed on: the manifest's, else the input config's
/// `game_id` when a game set one (`flint` is the engine's default).
fn save_id<'a>(project: &'a Project, input_game_id: &'a str) -> Option<&'a str> {
    let input = Some(input_game_id.trim()).filter(|id| !id.is_empty() && *id != "flint");
    project.id.as_deref().or(input)
}

/// Slots as files (or, on the web, `localStorage` entries) through
/// [`user_files`]: the player's [`SaveBackend`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SaveFiles {
    /// The default slot's file.
    pub default_file: PathBuf,
    /// Where a named slot's `<slot>.toml` is.
    pub slot_dir: PathBuf,
    /// TEMPORARY: where the default slot is read from, in order, when
    /// `default_file` isn't there.
    pub old_places: Vec<PathBuf>,
    /// Why the files are here, for the log.
    pub why: String,
}

impl SaveFiles {
    /// `<dir>/<slot>.toml`, and nothing read from anywhere else.
    fn in_dir(dir: &Path, why: String) -> Self {
        Self {
            default_file: dir.join(format!("{DEFAULT_SLOT}.toml")),
            slot_dir: dir.to_path_buf(),
            old_places: Vec::new(),
            why,
        }
    }

    /// Where a game's saves are kept: see the module's list.
    pub fn locate(
        project: &Project,
        scene_path: &str,
        input_game_id: &str,
        save_dir: Option<&Path>,
        config_dir: Option<&Path>,
    ) -> Self {
        if let Some(dir) = save_dir {
            return Self::in_dir(dir, "--save-dir".into());
        }
        let id = save_id(project, input_game_id);
        if cfg!(target_arch = "wasm32") {
            let id = id.unwrap_or_default();
            let default_file = web_persist_path(id);
            // TEMPORARY: before the manifest's id was asked, the entry was
            // keyed on the input config's
            let old = web_persist_path(input_game_id);
            return Self {
                old_places: Vec::from_iter((old != default_file).then_some(old)),
                default_file,
                slot_dir: web_slot_dir(id),
                why: format!("localStorage, game id '{id}'"),
            };
        }
        let in_project = persist_path(project, scene_path);
        let old_guess = old_persist_path(project, scene_path);
        let user_dir = match (id, config_dir) {
            _ if cfg!(target_os = "android") => None,
            (Some(id), Some(config)) => match user_folder_name(id) {
                Ok(folder) => Some((config.join(folder).join(USER_SAVE_DIR), id)),
                Err(why) => {
                    tracing::warn!(
                        "persist: the game id '{id}' can't name a folder under the user \
                         directory ({why}); saves stay in the project folder, {}",
                        in_project.display()
                    );
                    None
                }
            },
            _ => None,
        };
        match user_dir {
            Some((dir, id)) => Self {
                old_places: std::iter::once(in_project).chain(old_guess).collect(),
                ..Self::in_dir(&dir, format!("the user directory, game id '{id}'"))
            },
            None => Self {
                slot_dir: in_project
                    .parent()
                    .map(Path::to_path_buf)
                    .unwrap_or_default(),
                default_file: in_project,
                old_places: Vec::from_iter(old_guess),
                why: if cfg!(target_os = "android") {
                    "the app's files directory".into()
                } else {
                    "the project folder: the game has no id".into()
                },
            },
        }
    }

    /// The file `slot` is saved to.
    pub fn file(&self, slot: &str) -> PathBuf {
        if slot == DEFAULT_SLOT {
            self.default_file.clone()
        } else {
            self.slot_dir.join(format!("{slot}.toml"))
        }
    }
}

/// How a path is shown in a log line or to a script: the `localStorage`
/// key on the web, the path elsewhere.
fn shown(path: &Path) -> String {
    if cfg!(target_arch = "wasm32") {
        user_files::storage_key(path)
    } else {
        path.display().to_string()
    }
}

/// The text at `path`, whether the player wrote it ([`user_files`]) or the
/// game ships it (the read-only [`vfs`]).
fn read_text(path: &Path) -> Result<Option<String>, String> {
    match user_files::read(path) {
        Ok(Some(text)) => Ok(Some(text)),
        Ok(None) if vfs::is_file(path) => vfs::read_to_string(path)
            .map(Some)
            .map_err(|e| e.to_string()),
        Ok(None) => Ok(None),
        Err(e) => Err(e.to_string()),
    }
}

impl SaveBackend for SaveFiles {
    fn read(&self, slot: &str) -> Result<Option<SaveText>, String> {
        let file = self.file(slot);
        if let Some(text) = read_text(&file)? {
            return Ok(Some(SaveText {
                text,
                from: shown(&file),
                old_place: false,
            }));
        }
        if slot != DEFAULT_SLOT {
            return Ok(None);
        }
        for old in &self.old_places {
            if let Some(text) = read_text(old)? {
                return Ok(Some(SaveText {
                    text,
                    from: shown(old),
                    old_place: true,
                }));
            }
        }
        Ok(None)
    }

    fn write(&self, slot: &str, text: &str) -> Result<(), String> {
        let file = self.file(slot);
        user_files::write(&file, text).map_err(|e| format!("failed to save {}: {e}", shown(&file)))
    }

    fn set_aside(&self, slot: &str) -> Result<String, String> {
        user_files::set_aside(&self.file(slot))
            .map(|backup| shown(&backup))
            .map_err(|e| e.to_string())
    }

    fn slots(&self) -> Vec<String> {
        let mut slots = Vec::new();
        if user_files::exists(&self.default_file) {
            slots.push(DEFAULT_SLOT.to_string());
        }
        for name in user_files::list(&self.slot_dir) {
            let Some(slot) = name.strip_suffix(".toml") else {
                continue;
            };
            // `default` is the default slot's file whatever that is called,
            // and that file is listed above
            if slot != DEFAULT_SLOT
                && check_slot_name(slot).is_ok()
                && self.slot_dir.join(&name) != self.default_file
            {
                slots.push(slot.to_string());
            }
        }
        slots.sort();
        slots
    }

    fn place(&self, slot: &str) -> String {
        shown(&self.file(slot))
    }
}

impl PlayerApp {
    /// Decide where this run's saves are kept, tell the store, and read
    /// the default slot. A missing file is not an error and creates
    /// nothing; one that doesn't parse is set aside as `.bak` and reported,
    /// so a corrupt save never blocks the game from starting and is never
    /// saved over.
    pub(super) fn load_persistent_store(&mut self) {
        self.refresh_project();
        let files = SaveFiles::locate(
            &self.core.project,
            &self.core.scene_path,
            &self.input.config().game_id,
            self.launch.save_dir.as_deref(),
            self.saves.user_config_dir.as_deref(),
        );
        tracing::info!(
            "persist: saves are kept at {} ({})",
            shown(&files.file(DEFAULT_SLOT)),
            files.why
        );
        self.core
            .store
            .set_game_version(self.core.project.save_version);
        self.core.store.set_backend(Box::new(files));
        let loaded = self.core.store.load();
        log_loaded(&loaded);
    }

    /// Write the slot held now (a temp file, renamed over the slot's).
    /// `reason` is for the log line only.
    pub(super) fn save_persistent_store(&mut self, reason: &str) {
        if !self.core.store.has_backend() {
            // A player that never loaded a store (a headless test) has
            // nowhere to keep one
            self.core.store.mark_clean();
            return;
        }
        match self.core.store.save() {
            Ok(place) => {
                self.saves.error = None;
                tracing::info!(
                    "persist: saved {} keys to {place} ({reason})",
                    self.core.store.keys().len()
                );
            }
            Err(e) => {
                // Said once, not every second while it lasts
                if self.saves.error.as_deref() != Some(e.as_str()) {
                    tracing::error!("persist: {e}");
                    self.saves.error = Some(e);
                }
                // On the web, mark it clean anyway: a full or blocked
                // localStorage would otherwise be retried every second for
                // the session.
                if cfg!(target_arch = "wasm32") {
                    self.core.store.mark_clean();
                }
            }
        }
    }

    /// Save if the store changed and either the debounce window has passed
    /// or `force` is set (transition / exit). A script's `persist_save()`
    /// also forces.
    pub(super) fn flush_persistent_store(&mut self, force: bool, reason: &str) {
        let now = Instant::now();
        if self.core.store.is_dirty() {
            self.saves.debounce.note_dirty(now);
        }
        let force = force || self.core.store.take_flush_request();
        if self.saves.debounce.should_flush(now, force) {
            self.save_persistent_store(reason);
        }
    }
}

/// One log line for a load: an error for a file that was lost to the game,
/// a warning (once) for a save that moved.
pub(crate) fn log_loaded(loaded: &flint_runtime::Loaded) {
    if loaded.is_error() {
        tracing::error!("persist: {loaded}");
    } else if loaded.is_move() {
        tracing::warn!("persist: {loaded}");
    } else {
        tracing::info!("persist: {loaded}");
    }
}

#[cfg(test)]
mod tests {
    use super::super::extension_tests::attach_recording_script;
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

    /// A game's named slots on the web are in a folder of their own, so the
    /// entry a web save is in today stays the default slot and no slot can
    /// be mistaken for another game's store.
    #[test]
    fn web_slots_sit_beside_the_entry_games_already_have() {
        let dir = web_slot_dir("flint_kart");
        assert_eq!(dir.file_name().unwrap(), "saves_flint_kart");
        assert_eq!(dir.parent(), web_persist_path("flint_kart").parent());
        assert_eq!(web_slot_dir("").file_name().unwrap(), "saves");
        assert_ne!(
            web_slot_dir("a").join("b.toml"),
            web_persist_path("a.b"),
            "game a's slot b is not game a.b's store"
        );
    }

    /// Run `f` over files in memory (a path ending in `/` is a folder).
    fn with_files<R>(files: &[&str], f: impl FnOnce() -> R) -> R {
        let mut source = vfs::MemorySource::new();
        for path in files {
            if path.ends_with('/') {
                source.insert(format!("{path}.keep"), "");
            } else if path.ends_with("flint.toml") {
                source.insert(path, "[project]\nid = \"kart\"\n");
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

    /// The layouts games use today: the in-project file is exactly where it
    /// was, `<the scene folder's parent>/save/persist.toml`, and there is no
    /// older place to read.
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

    /// The in-project file is one file for the whole project (#706).
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

    // ─── #726: where saves are kept ───────────────────────────────────

    fn locate(scene: &str, input_id: &str, save_dir: Option<&str>, config: bool) -> SaveFiles {
        let project = Project::discover_with_home(Path::new(scene), None).unwrap();
        SaveFiles::locate(
            &project,
            scene,
            input_id,
            save_dir.map(Path::new),
            config.then_some(Path::new("cfg")),
        )
    }

    /// Case by case: the manifest's id, the input config's, none, one that
    /// can't be a folder, no user directory, and `--save-dir` over all.
    #[cfg(not(target_os = "android"))]
    #[test]
    fn where_a_games_saves_are_kept() {
        let scene = "proj/scenes/levels/one.scene.toml";
        let in_project = Path::new("proj").join("save").join("persist.toml");
        let user = |id: &str| Path::new("cfg").join(id).join("saves");

        // `[project] id = "kart"`: the user directory, the in-project file
        // read once; the input config's id doesn't matter
        with_files(&["proj/flint.toml", scene], || {
            let files = locate(scene, "other", None, true);
            assert_eq!(files.default_file, user("kart").join("default.toml"));
            assert_eq!(files.file("b"), user("kart").join("b.toml"));
            assert_eq!(files.old_places, std::slice::from_ref(&in_project));
            // No user directory on this machine: the project folder
            assert_eq!(locate(scene, "", None, false).default_file, in_project);
            // --save-dir: that folder, and no old place
            let files = locate(scene, "", Some("elsewhere"), true);
            assert_eq!(
                files.default_file,
                Path::new("elsewhere").join("default.toml")
            );
            assert_eq!(files.file("b"), Path::new("elsewhere").join("b.toml"));
            assert!(files.old_places.is_empty());
        });
        // ... and the root guessed before #706 after it, when it holds one
        with_files(
            &["proj/flint.toml", scene, "proj/scenes/save/persist.toml"],
            || {
                assert_eq!(
                    locate(scene, "", None, true).old_places,
                    [
                        in_project.clone(),
                        Path::new("proj/scenes").join("save/persist.toml")
                    ]
                );
            },
        );
        // No manifest id: the input config's `game_id`, when a game set one
        let loose = "loose/scenes/menu.scene.toml";
        let loose_file = Path::new("loose").join("save").join("persist.toml");
        with_files(&[loose], || {
            let files = locate(loose, "lanterns", None, true);
            assert_eq!(files.default_file, user("lanterns").join("default.toml"));
            assert_eq!(files.old_places, std::slice::from_ref(&loose_file));
            // No id at all (`flint` is the engine's default): the project
            // folder, as before, slots beside the file
            for no_id in ["", "flint", "  "] {
                let files = locate(loose, no_id, None, true);
                assert_eq!(files.default_file, loose_file, "{no_id:?}");
                assert_eq!(
                    files.file("b"),
                    Path::new("loose").join("save").join("b.toml")
                );
                assert!(files.old_places.is_empty());
            }
            // An id that can't be a folder name is not made into one
            for bad in ["../up", "a/b", "a:b", ".hidden", "con", "COM1.x", "dot."] {
                assert_eq!(
                    locate(loose, bad, None, true).default_file,
                    loose_file,
                    "{bad}"
                );
            }
            for good in ["Flint Kart", "float", "a.b", "kart_2-x", "comet"] {
                assert_eq!(
                    locate(loose, good, None, true).default_file,
                    user(good).join("default.toml")
                );
            }
        });
    }

    // ─── #726, through the player, on the disk ─────────────────────────

    /// A project in a temp folder: `files` are paths from its root, with
    /// their text. Returns the root, and a folder standing in for the OS
    /// user directory.
    fn project_on_disk(name: &str, files: &[(&str, &str)]) -> (PathBuf, PathBuf) {
        let base = std::env::temp_dir().join(format!("flint_726_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let root = base.join("game");
        for (path, text) in files {
            let path = root.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
        (root, base.join("user"))
    }

    fn app_at(scene: &Path, user_dir: &Path) -> PlayerApp {
        let mut app = PlayerApp::new(
            flint_ecs::FlintWorld::new(),
            scene.to_string_lossy().into_owned(),
            false,
            None,
            None,
        );
        app.saves.user_config_dir = Some(user_dir.to_path_buf());
        app
    }

    /// Every file under `dir`, as a path from it with forward slashes, and
    /// its text.
    fn files_under(dir: &Path) -> Vec<(String, String)> {
        fn walk(dir: &Path, base: &Path, out: &mut Vec<(String, String)>) {
            let Ok(entries) = std::fs::read_dir(dir) else {
                return;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    walk(&path, base, out);
                } else {
                    let name = path.strip_prefix(base).unwrap().to_string_lossy();
                    let text = std::fs::read_to_string(&path).unwrap_or_default();
                    out.push((name.replace('\\', "/"), text));
                }
            }
        }
        let mut out = Vec::new();
        walk(dir, dir, &mut out);
        out.sort();
        out
    }

    fn names(files: &[(String, String)]) -> Vec<&str> {
        files.iter().map(|(name, _)| name.as_str()).collect()
    }

    fn coins(app: &PlayerApp) -> Option<i64> {
        app.core.store.get("coins").and_then(|v| v.as_integer())
    }

    fn set_coins(app: &mut PlayerApp, coins: i64) {
        app.core.store.set("coins", toml::Value::Integer(coins));
    }

    const SCENE: &str = "[scene]\nname = \"s\"\n";

    /// Done-when 1: through a real transition into a deeper scene, the
    /// store is still saved to the one file it was read from. `saved_in` is
    /// the folder that file is in.
    fn save_file_across_a_transition(
        name: &str,
        marker: &[(&str, &str)],
        saved_in: impl Fn(&Path, &Path) -> PathBuf,
    ) {
        if flint_core::project::overridden().is_some() {
            return;
        }
        let mut files = vec![
            ("scenes/menu.scene.toml", SCENE),
            ("scenes/levels/one.scene.toml", SCENE),
        ];
        files.extend_from_slice(marker);
        let (root, user) = project_on_disk(name, &files);
        let base = root.parent().unwrap().to_path_buf();
        let saved_in = saved_in(&root, &user);
        let mut app = app_at(&root.join("scenes/menu.scene.toml"), &user);
        app.load_persistent_store();
        set_coins(&mut app, 1);
        app.save_persistent_store("test");
        let before = files_under(&base);
        let saves = files_under(&saved_in);
        assert_eq!(saves.len(), 1, "one save file: {saves:?}");

        app.execute_scene_transition("levels/one.scene.toml");
        assert!(
            Path::new(&app.core.scene_path).ends_with("scenes/levels/one.scene.toml"),
            "the transition happened: {}",
            app.core.scene_path
        );
        set_coins(&mut app, 2);
        app.save_persistent_store("test");

        let after = files_under(&base);
        assert_eq!(
            names(&after),
            names(&before),
            "no second save file appeared"
        );
        let saves_after = files_under(&saved_in);
        assert_eq!(names(&saves_after), names(&saves));
        assert!(saves_after[0].1.contains("coins = 2"), "{saves_after:?}");
        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn the_save_file_stays_put_across_a_transition_with_a_manifest() {
        save_file_across_a_transition(
            "moves_manifest",
            &[("flint.toml", "[project]\n")],
            |r, _| r.join("save"),
        );
    }

    /// The one the audit's claim still held for after #706: no project
    /// marker, so the root was guessed again from the deeper scene.
    #[test]
    fn the_save_file_stays_put_across_a_transition_with_no_marker() {
        save_file_across_a_transition("moves_loose", &[], |r, _| r.join("save"));
    }

    #[test]
    fn the_save_file_stays_put_across_a_transition_in_the_user_directory() {
        save_file_across_a_transition(
            "moves_user",
            &[("flint.toml", "[project]\nid = \"kart\"\n")],
            |_, user| user.join("kart").join("saves"),
        );
    }

    /// Done-when 2, through the player: a save file that doesn't parse is
    /// kept as `.bak`, text intact, through the load and the saves that
    /// follow; the game starts empty; scripts can read what happened.
    #[test]
    fn a_corrupt_save_is_kept_as_bak_and_reported() {
        if flint_core::project::overridden().is_some() {
            return;
        }
        let garbage = "coins = 7\nbest_lap = [61.2, 59.8\n";
        let (root, user) = project_on_disk(
            "corrupt",
            &[
                ("flint.toml", "[project]\n"),
                ("scenes/menu.scene.toml", SCENE),
                ("save/persist.toml", garbage),
            ],
        );
        let scene = root.join("scenes/menu.scene.toml");
        let save = root.join("save");
        let mut app = app_at(&scene, &user);
        app.load_persistent_store();
        assert_eq!(
            files_under(&save),
            [("persist.toml.bak".to_string(), garbage.to_string())],
            "set aside at load, before any save"
        );
        assert_eq!(coins(&app), None);
        let report = app.core.store.corrupt().expect("reported").clone();
        assert_eq!(report.file, save.join("persist.toml").display().to_string());
        assert_eq!(
            report.backup,
            save.join("persist.toml.bak").display().to_string()
        );

        let log = attach_recording_script(
            &mut app,
            "corrupt",
            r#"
fn on_init() {
    let bad = persist_corrupt();
    if bad != () {
        record(`slot ${bad.slot} unreadable; kept as a .bak: ${bad.backup.ends_with("persist.toml.bak")}`);
        record(bad.error.sub_string(0, 11));
    }
    persist_set("coins", 0);
}
"#,
        );
        app.tick();
        assert_eq!(
            *log.lock().unwrap(),
            [
                "slot default unreadable; kept as a .bak: true",
                "parse error"
            ]
        );
        app.save_persistent_store("test");
        assert_eq!(
            files_under(&save),
            [
                ("persist.toml".to_string(), "coins = 0\n".to_string()),
                ("persist.toml.bak".to_string(), garbage.to_string()),
            ]
        );

        // The same thing a second time: the first backup is not replaced
        let worse = "coins = = 3\n";
        std::fs::write(save.join("persist.toml"), worse).unwrap();
        let mut again = app_at(&scene, &user);
        again.load_persistent_store();
        set_coins(&mut again, 1);
        again.save_persistent_store("test");
        assert_eq!(
            files_under(&save),
            [
                ("persist.toml".to_string(), "coins = 1\n".to_string()),
                ("persist.toml.bak".to_string(), garbage.to_string()),
                ("persist.toml.bak.1".to_string(), worse.to_string()),
            ]
        );
        // A clean load reports nothing
        let mut clean = app_at(&scene, &user);
        clean.load_persistent_store();
        assert_eq!(clean.core.store.corrupt(), None);
        assert_eq!(coins(&clean), Some(1));
        let _ = std::fs::remove_dir_all(root.parent().unwrap());
    }

    /// Done-when 3, through the player: a save is a temp file renamed over
    /// the slot's. The hard link is a second name for the old file's bytes;
    /// a write through the save path would change what it reads.
    #[test]
    fn the_players_save_is_a_temp_file_renamed_over_the_old_one() {
        if flint_core::project::overridden().is_some() {
            return;
        }
        let (root, user) = project_on_disk(
            "atomic",
            &[
                ("flint.toml", "[project]\nid = \"kart\"\n"),
                ("scenes/menu.scene.toml", SCENE),
            ],
        );
        let saves = user.join("kart").join("saves");
        std::fs::create_dir_all(&saves).unwrap();
        std::fs::write(saves.join("default.toml"), "coins = 7\n").unwrap();
        let witness = user.join("witness");
        std::fs::hard_link(saves.join("default.toml"), &witness).unwrap();

        let mut app = app_at(&root.join("scenes/menu.scene.toml"), &user);
        app.load_persistent_store();
        assert_eq!(coins(&app), Some(7));
        set_coins(&mut app, 8);
        app.save_persistent_store("test");

        assert_eq!(
            files_under(&saves),
            [("default.toml".to_string(), "coins = 8\n".to_string())],
            "the new save, and no temp file left"
        );
        assert_eq!(
            std::fs::read_to_string(&witness).unwrap(),
            "coins = 7\n",
            "the old file's bytes were never written into"
        );
        let _ = std::fs::remove_dir_all(root.parent().unwrap());
    }

    /// Each old place is read once: the save is then written under the user
    /// directory, the old file is left as it was, and the next run reads
    /// the new one.
    #[test]
    fn the_player_reads_each_old_place_once_and_saves_to_the_user_directory() {
        if flint_core::project::overridden().is_some() {
            return;
        }
        for (name, old_file) in [
            // Where every save was before #726
            ("old_root", "save/persist.toml"),
            // Where the root was guessed before #706, for a scene two deep
            ("old_guess", "scenes/save/persist.toml"),
        ] {
            let (root, user) = project_on_disk(
                name,
                &[
                    ("flint.toml", "[project]\nid = \"saves\"\n"),
                    ("scenes/levels/one.scene.toml", SCENE),
                    (old_file, "coins = 7\n"),
                ],
            );
            let scene = root.join("scenes/levels/one.scene.toml");
            let new = user.join("saves").join("saves").join("default.toml");
            let game_before = files_under(&root);

            let mut first = app_at(&scene, &user);
            first.load_persistent_store();
            assert_eq!(coins(&first), Some(7), "{name}: read from the old place");
            assert!(
                first.core.store.is_dirty(),
                "owed to the new place, so the debounce writes it"
            );
            first.flush_persistent_store(true, "test");
            assert_eq!(std::fs::read_to_string(&new).unwrap(), "coins = 7\n");
            set_coins(&mut first, 8);
            first.save_persistent_store("test");
            assert_eq!(
                files_under(&root),
                game_before,
                "{name}: the old file is left as it was, and nothing new is in the game"
            );

            let mut second = app_at(&scene, &user);
            second.load_persistent_store();
            assert_eq!(coins(&second), Some(8), "{name}: the new place wins");
            assert!(!second.core.store.is_dirty());
            let _ = std::fs::remove_dir_all(root.parent().unwrap());
        }
    }

    /// `--save-dir` is the whole answer: nothing is read from the project
    /// or the user directory, and nothing is written to either.
    #[test]
    fn save_dir_overrides_everything() {
        if flint_core::project::overridden().is_some() {
            return;
        }
        let (root, user) = project_on_disk(
            "save_dir",
            &[
                ("flint.toml", "[project]\nid = \"kart\"\n"),
                ("scenes/menu.scene.toml", SCENE),
                ("save/persist.toml", "coins = 7\n"),
            ],
        );
        let user_file = user.join("kart").join("saves").join("default.toml");
        std::fs::create_dir_all(user_file.parent().unwrap()).unwrap();
        std::fs::write(&user_file, "coins = 70\n").unwrap();
        let dir = root.parent().unwrap().join("chosen");

        let mut app = app_at(&root.join("scenes/menu.scene.toml"), &user);
        app.launch.save_dir = Some(dir.clone());
        app.load_persistent_store();
        assert_eq!(coins(&app), None, "neither other save is read");
        set_coins(&mut app, 1);
        app.save_persistent_store("test");
        assert!(app.core.store.open_slot("b").is_ok());
        set_coins(&mut app, 2);
        app.save_persistent_store("test");

        assert_eq!(
            files_under(&dir),
            [
                ("b.toml".to_string(), "coins = 2\n".to_string()),
                ("default.toml".to_string(), "coins = 1\n".to_string()),
            ]
        );
        assert_eq!(std::fs::read_to_string(&user_file).unwrap(), "coins = 70\n");
        assert_eq!(
            std::fs::read_to_string(root.join("save/persist.toml")).unwrap(),
            "coins = 7\n"
        );
        let _ = std::fs::remove_dir_all(root.parent().unwrap());
    }

    /// A game installed where it can't write still saves: with an id, not
    /// one byte goes into the game's folder. (The folder here is writable;
    /// what is shown is that nothing tries.)
    #[test]
    fn a_game_with_an_id_saves_to_the_user_directory_and_never_into_its_own_folder() {
        if flint_core::project::overridden().is_some() {
            return;
        }
        let (root, user) = project_on_disk(
            "read_only",
            &[
                ("flint.toml", "[project]\nid = \"kart\"\n"),
                ("scenes/menu.scene.toml", SCENE),
            ],
        );
        let game_before = files_under(&root);
        let scene = root.join("scenes/menu.scene.toml");
        let mut app = app_at(&scene, &user);
        app.load_persistent_store();
        set_coins(&mut app, 3);
        app.flush_persistent_store(true, "exit");
        assert!(app.core.store.open_slot("second").is_ok());
        set_coins(&mut app, 4);
        app.flush_persistent_store(true, "exit");

        assert_eq!(
            files_under(&root),
            game_before,
            "the game's folder is untouched"
        );
        assert_eq!(
            files_under(&user),
            [
                (
                    "kart/saves/default.toml".to_string(),
                    "coins = 3\n".to_string()
                ),
                (
                    "kart/saves/second.toml".to_string(),
                    "coins = 4\n".to_string()
                ),
            ]
        );
        let mut next = app_at(&scene, &user);
        next.load_persistent_store();
        assert_eq!(coins(&next), Some(3));
        let _ = std::fs::remove_dir_all(root.parent().unwrap());
    }

    /// Done-when 4: slots and the version, read and used from Rhai, through
    /// the player and onto the disk.
    #[test]
    fn slots_and_the_version_can_be_read_from_rhai() {
        if flint_core::project::overridden().is_some() {
            return;
        }
        let (root, user) = project_on_disk(
            "rhai",
            &[
                ("flint.toml", "[project]\nid = \"kart\"\nsave_version = 2\n"),
                ("scenes/menu.scene.toml", SCENE),
            ],
        );
        let saves = user.join("kart").join("saves");
        std::fs::create_dir_all(&saves).unwrap();
        // A save from before versions, and one already at version 2
        std::fs::write(saves.join("default.toml"), "coins = 7\n").unwrap();
        std::fs::write(
            saves.join("hard.toml"),
            "gold = 40\n\n[meta]\nversion = 2\n",
        )
        .unwrap();

        let mut app = app_at(&root.join("scenes/menu.scene.toml"), &user);
        app.load_persistent_store();
        let log = attach_recording_script(&mut app, "slots", SLOTS_SCRIPT);
        app.tick();
        app.tick();
        app.flush_persistent_store(true, "exit");

        let said = log.lock().unwrap().clone();
        println!("--- script output ---");
        for line in &said {
            println!("{line}");
        }
        assert_eq!(
            said,
            [
                "migrate from 0: 7 coins become 70 gold",
                "init: slot default at version 2, 70 gold",
                "slots: [\"default\", \"hard\"]",
                "bad name refused: true, still in default",
                "switched to hard: version 2, 40 gold",
                "switched to new: version 2, keys []",
                "slots now: [\"default\", \"hard\", \"new\"]",
            ]
        );
        let on_disk = files_under(&saves);
        println!("--- kart/saves ---");
        for (name, text) in &on_disk {
            println!("{name}:\n{text}");
        }
        assert_eq!(
            on_disk,
            [
                (
                    "default.toml".to_string(),
                    "gold = 70\n\n[meta]\nversion = 2\n".to_string()
                ),
                (
                    "hard.toml".to_string(),
                    "gold = 41\n\n[meta]\nversion = 2\n".to_string()
                ),
                (
                    "new.toml".to_string(),
                    "gold = 0\n\n[meta]\nversion = 2\n".to_string()
                ),
            ]
        );
        let _ = std::fs::remove_dir_all(root.parent().unwrap());
    }

    /// The script of `slots_and_the_version_can_be_read_from_rhai` (and of
    /// the book's example).
    const SLOTS_SCRIPT: &str = r#"
// Version 1 kept `coins`; version 2 keeps `gold`, ten to a coin.
fn on_persist_migrate(from) {
    if from < 2 {
        let coins = persist_get("coins");
        record(`migrate from ${from}: ${coins} coins become ${coins * 10} gold`);
        persist_set("gold", coins * 10);
        persist_remove("coins");
    }
}

fn on_init() {
    record(`init: slot ${persist_slot()} at version ${persist_version()}, ${persist_get("gold")} gold`);
    record(`slots: ${persist_slots()}`);

    let ok = persist_slot("../somewhere");
    record(`bad name refused: ${!ok}, still in ${persist_slot()}`);

    persist_slot("hard");
    record(`switched to hard: version ${persist_version()}, ${persist_get("gold")} gold`);
    persist_set("gold", persist_get("gold") + 1);

    persist_slot("new");
    record(`switched to new: version ${persist_version()}, keys ${persist_keys()}`);
    persist_set("gold", 0);
    persist_save();
}

// persist_save() writes at the end of the frame that asked
let frame = 0;
fn on_update() {
    frame += 1;
    if frame == 2 {
        record(`slots now: ${persist_slots()}`);
    }
}
"#;

    /// A slot loaded at an older version, after startup, is migrated before
    /// the next frame's `on_update`; and data no loaded script can migrate
    /// keeps its version until one can.
    #[test]
    fn a_slot_switched_to_is_migrated_before_the_next_update() {
        if flint_core::project::overridden().is_some() {
            return;
        }
        let (root, user) = project_on_disk(
            "migrate_slot",
            &[
                ("flint.toml", "[project]\nid = \"kart\"\nsave_version = 3\n"),
                ("scenes/menu.scene.toml", SCENE),
            ],
        );
        let saves = user.join("kart").join("saves");
        std::fs::create_dir_all(&saves).unwrap();
        std::fs::write(saves.join("old.toml"), "coins = 1\n\n[meta]\nversion = 1\n").unwrap();

        let mut app = app_at(&root.join("scenes/menu.scene.toml"), &user);
        app.load_persistent_store();
        assert_eq!(app.core.store.version(), 3, "a fresh store is current");
        // No script yet: a slot at version 1 stays at 1, pending
        app.core.store.open_slot("old").unwrap();
        app.tick();
        assert_eq!(app.core.store.version(), 1);
        assert_eq!(app.core.store.pending_migration(), Some(1));
        app.core.store.open_slot("default").unwrap();

        let log = attach_recording_script(
            &mut app,
            "migrate_slot",
            r#"
let frame = 0;
fn on_persist_migrate(from) { record(`migrate ${from} -> 3 in ${persist_slot()}`); }
fn on_update() {
    frame += 1;
    record(`update ${frame}: ${persist_slot()} v${persist_version()}`);
    if frame == 1 {
        persist_slot("old");
        record(`just switched: v${persist_version()}`);
    }
}
"#,
        );
        app.tick();
        app.tick();
        assert_eq!(
            *log.lock().unwrap(),
            [
                "update 1: default v3",
                "just switched: v1",
                "migrate 1 -> 3 in old",
                "update 2: old v3",
            ]
        );
        let _ = std::fs::remove_dir_all(root.parent().unwrap());
    }
}
