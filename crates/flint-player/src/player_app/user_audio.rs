//! The player's own bus volumes, kept between runs (#1217).
//!
//! A game turns its buses with `set_bus_volume` (fades, ducking, a scene's
//! mix). The person playing has a say of their own, "music at 40%", which
//! a settings menu sets with `set_user_bus_volume`. The two are separate
//! factors in the engine (`AudioEngine::set_user_bus_volume`), so a fade
//! to 0.5 under a setting of 0.4 is heard at 0.2 and never overwrites it.
//!
//! # Where
//!
//! One small file through [`user_files`] (the OS config directory, or
//! `localStorage` on the web), decided once at the run's start:
//!
//! 1. **`--save-dir <dir>`**: `<dir>/audio.user.toml` (a slot is named
//!    `[a-z0-9_-]`, so no slot can be this file).
//! 2. **A game with an id** (the save's id: `[project] id`, else the input
//!    config's `game_id`): `<config>/<id>/audio.toml`, beside `saves/`. On
//!    the web `flint:/config/<id>/audio.toml`.
//! 3. **No id**, or one that can't name a folder: `<config>/flint/audio.toml`,
//!    shared by the engine's examples and loose scenes.
//!
//! A playtest (`saves_in_memory`) reads and writes none.
//!
//! # When
//!
//! Read at start, right after the save store and before any sound or
//! script, so the first note plays at the player's volume and `on_init`
//! reads it. Written half a second after the last change (a slider
//! dragged is one write), and at every scene transition and on exit, with
//! `user_files::write` (atomic). The format:
//!
//! ```toml
//! [buses]
//! music = 0.4
//! ```
//!
//! A file that doesn't parse is said once and left as it is; the run plays
//! at unity, and the first change writes over it.

use super::persist::{save_id, user_folder_name};
use super::PlayerApp;
use crate::user_files;
use flint_core::project::Project;
use flint_runtime::SaveDebounce;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;
use web_time::Instant;

/// The file's name in a game's own folder under the user directory.
pub const USER_AUDIO_FILE: &str = "audio.toml";
/// Its name in a `--save-dir`, where slots are files too.
pub const SAVE_DIR_AUDIO_FILE: &str = "audio.user.toml";

/// How long after the last change the file is written.
const WRITE_AFTER: Duration = Duration::from_millis(500);

/// Where the player's bus volumes are kept, and whether they need writing.
pub(super) struct UserAudio {
    /// The file, once the run's start has decided it; `None` keeps the
    /// volumes in memory only (a playtest, a host that never started).
    pub(super) file: Option<PathBuf>,
    pub(super) debounce: SaveDebounce,
    dirty: bool,
    /// The last write failure said, so one that lasts is said once.
    error: Option<String>,
}

impl Default for UserAudio {
    fn default() -> Self {
        Self {
            file: None,
            debounce: SaveDebounce::new(WRITE_AFTER),
            dirty: false,
            error: None,
        }
    }
}

/// The player's bus-volume file: see the module's list.
pub fn user_audio_path(
    project: &Project,
    input_game_id: &str,
    save_dir: Option<&Path>,
    config_dir: Option<&Path>,
) -> Option<PathBuf> {
    if let Some(dir) = save_dir {
        return Some(dir.join(SAVE_DIR_AUDIO_FILE));
    }
    let config = config_dir?;
    let folder = save_id(project, input_game_id)
        .and_then(|id| user_folder_name(id).ok())
        .unwrap_or("flint");
    Some(config.join(folder).join(USER_AUDIO_FILE))
}

/// The volumes a file holds, by bus name, or why it can't be read.
pub fn parse_user_audio(text: &str) -> Result<BTreeMap<String, f64>, String> {
    let table: toml::Table = text
        .parse()
        .map_err(|e: toml::de::Error| e.message().to_string())?;
    let Some(buses) = table.get("buses") else {
        return Ok(BTreeMap::new());
    };
    let buses = buses
        .as_table()
        .ok_or_else(|| "`buses` is not a table".to_string())?;
    let mut volumes = BTreeMap::new();
    for (name, value) in buses {
        let v = flint_core::toml_util::toml_f64(value)
            .ok_or_else(|| format!("`buses.{name}` is not a number"))?;
        volumes.insert(name.clone(), v);
    }
    Ok(volumes)
}

/// The file's text for these volumes.
pub fn format_user_audio(volumes: &BTreeMap<String, f64>) -> String {
    let mut text = String::from(
        "# The player's own bus volumes (0..1), times the game's; written by the game's settings.\n[buses]\n",
    );
    for (name, v) in volumes {
        // A bus name is a TOML key as the manifest wrote it; quote any
        // that isn't bare.
        let bare = !name.is_empty()
            && name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
        let key = if bare {
            name.clone()
        } else {
            toml::Value::String(name.clone()).to_string()
        };
        text.push_str(&format!("{key} = {v}\n"));
    }
    text
}

/// A path as a log line shows it: the `localStorage` key on the web.
fn shown(path: &Path) -> String {
    if cfg!(target_arch = "wasm32") {
        user_files::storage_key(path)
    } else {
        path.display().to_string()
    }
}

impl PlayerApp {
    /// Read the player's bus volumes into the mixer and the scripts, once
    /// at start (`enter_scene(Start)`, after the save store, before sound).
    pub(super) fn load_user_audio(&mut self) {
        if self.launch.saves_in_memory {
            self.saves.audio.file = None;
            self.publish_user_audio();
            return;
        }
        let Some(file) = user_audio_path(
            &self.core.project,
            &self.input.config().game_id,
            self.launch.save_dir.as_deref(),
            self.saves.user_config_dir.as_deref(),
        ) else {
            self.publish_user_audio();
            return;
        };
        let shown = shown(&file);
        match user_files::read(&file) {
            Ok(Some(text)) => match parse_user_audio(&text) {
                Ok(volumes) => {
                    let said: Vec<String> =
                        volumes.iter().map(|(n, v)| format!("{n} {v}")).collect();
                    tracing::info!(
                        "audio: the player's bus volumes from {shown}: {}",
                        said.join(", ")
                    );
                    self.audio.engine.load_user_bus_volumes(volumes);
                }
                Err(why) => tracing::warn!(
                    "audio: the player's bus volumes at {shown} don't read ({why}); \
                     playing at unity, and the next change writes over it"
                ),
            },
            Ok(None) => {}
            Err(e) => tracing::warn!("audio: can't read {shown}: {e}"),
        }
        self.saves.audio.file = Some(file);
        self.publish_user_audio();
    }

    /// A script's `set_user_bus_volume`: into the mixer, back to the
    /// scripts, and to the file soon.
    pub(super) fn set_user_bus_volume(&mut self, bus: &str, volume: f64) {
        let before = self.audio.engine.user_bus_volume(bus);
        if self.audio.engine.set_user_bus_volume(bus, volume).is_ok()
            && self.audio.engine.user_bus_volume(bus) != before
        {
            self.saves.audio.dirty = true;
            self.saves.audio.debounce.note_dirty(Instant::now());
        }
        // Also after a refusal: the script wrote the name into its own
        // copy, which the engine's table replaces
        self.publish_user_audio();
    }

    /// Hand the scripts the player's volumes as the engine holds them.
    fn publish_user_audio(&mut self) {
        let volumes = self.audio.engine.user_bus_volumes();
        self.core
            .script
            .set_user_bus_volumes(volumes.iter().map(|(n, v)| (n.as_str(), *v)));
    }

    /// Write the player's volumes if they changed and the debounce has
    /// passed, or now with `force` (a transition, exit).
    pub(super) fn flush_user_audio(&mut self, force: bool) {
        if !self.saves.audio.dirty
            || !self
                .saves
                .audio
                .debounce
                .should_flush(Instant::now(), force)
        {
            return;
        }
        self.saves.audio.dirty = false;
        let Some(file) = self.saves.audio.file.clone() else {
            return;
        };
        let text = format_user_audio(self.audio.engine.user_bus_volumes());
        let written = {
            let _note =
                flint_core::frame_notes::begin(flint_core::frame_notes::NoteKind::Save, || {
                    "audio settings".to_string()
                });
            user_files::write(&file, &text)
        };
        let shown = shown(&file);
        match written {
            Ok(()) => {
                self.saves.audio.error = None;
                tracing::info!("audio: saved the player's bus volumes to {shown}");
            }
            Err(e) => {
                let message = format!("audio: failed to save {shown}: {e}");
                if self.saves.audio.error.as_deref() != Some(message.as_str()) {
                    tracing::warn!("{message}");
                    self.saves.audio.error = Some(message);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_file_reads_back_what_it_wrote() {
        let volumes = BTreeMap::from([
            ("music".to_string(), 0.4),
            ("voice".to_string(), 1.0),
            ("odd name".to_string(), 0.25),
        ]);
        let text = format_user_audio(&volumes);
        assert!(text.contains("music = 0.4\n"), "{text}");
        assert!(text.contains("\"odd name\" = 0.25\n"), "{text}");
        assert_eq!(parse_user_audio(&text).unwrap(), volumes);
        // An integer is a number too; a file with no table is no setting.
        assert_eq!(
            parse_user_audio("[buses]\nmusic = 1\n").unwrap()["music"],
            1.0
        );
        assert!(parse_user_audio("# nothing yet\n").unwrap().is_empty());
        assert!(parse_user_audio("[buses]\nmusic = \"loud\"\n")
            .unwrap_err()
            .contains("buses.music"));
        assert!(parse_user_audio("buses = 3").unwrap_err().contains("table"));
        assert!(parse_user_audio("[buses").is_err());
    }

    #[test]
    fn the_file_is_the_games_own_else_the_engines_else_the_save_dirs() {
        let config = Path::new("cfg");
        let mut files = flint_core::vfs::MemorySource::new();
        files.insert("loose/a.scene.toml", "");
        let mut project = flint_core::vfs::scoped(std::sync::Arc::new(files), || {
            Project::discover_with_home(Path::new("loose/a.scene.toml"), None).unwrap()
        });
        project.id = None;
        // No id at all: the engine's shared file
        assert_eq!(
            user_audio_path(&project, "flint", None, Some(config)).unwrap(),
            config.join("flint").join("audio.toml")
        );
        // The input config's game id, then the manifest's over it
        assert_eq!(
            user_audio_path(&project, "kart", None, Some(config)).unwrap(),
            config.join("kart").join("audio.toml")
        );
        project.id = Some("radio_play".into());
        assert_eq!(
            user_audio_path(&project, "kart", None, Some(config)).unwrap(),
            config.join("radio_play").join("audio.toml")
        );
        // An id that can't be a folder falls back to the shared file
        project.id = Some("../up".into());
        assert_eq!(
            user_audio_path(&project, "", None, Some(config)).unwrap(),
            config.join("flint").join("audio.toml")
        );
        // --save-dir keeps everything in it
        assert_eq!(
            user_audio_path(&project, "", Some(Path::new("runs")), Some(config)).unwrap(),
            Path::new("runs").join("audio.user.toml")
        );
        assert_eq!(user_audio_path(&project, "", None, None), None);
    }
}
