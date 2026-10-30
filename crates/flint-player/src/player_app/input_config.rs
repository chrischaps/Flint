//! Input configuration helpers — path resolution, user override persistence, and rebind state.

use anyhow::Result;
use flint_core::FlintError;
use flint_runtime::InputConfig;
use flint_core::vfs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub(super) struct PendingRebind {
    pub action: String,
    pub mode: flint_runtime::RebindMode,
}

#[derive(Debug, Clone)]
pub(super) struct InputConfigPaths {
    pub game_default: Option<PathBuf>,
    pub user_override: Option<PathBuf>,
    pub cli_override: Option<PathBuf>,
}

pub(super) fn resolve_input_paths(
    scene_path: &Path,
    scene_input_config: Option<&str>,
    cli_override: Option<&str>,
) -> InputConfigPaths {
    let scene_dir = scene_path.parent().unwrap_or_else(|| Path::new("."));

    // Game default: look next to the scene, then parent (game root)
    let game_default = scene_input_config
        .map(|name| {
            let p = scene_dir.join(name);
            if vfs::exists(&p) {
                p
            } else if let Some(parent) = scene_dir.parent() {
                parent.join(name)
            } else {
                p
            }
        })
        .or_else(|| {
            let candidate = scene_dir.join("config").join("input.toml");
            if vfs::exists(&candidate) {
                Some(candidate)
            } else {
                scene_dir
                    .parent()
                    .map(|p| p.join("config").join("input.toml"))
                    .filter(|p| vfs::exists(p))
            }
        });

    // The user override needs the game id, which the game default sets, so
    // it's resolved once that's merged (`resolve_user_override`).
    let cli = cli_override.map(PathBuf::from);

    InputConfigPaths {
        game_default,
        user_override: None,
        cli_override: cli,
    }
}

/// The user override to load: the project-local `.flint/input.user.toml`
/// if there is one, else this game's own `<config>/flint/input_<game_id>.toml`
/// (where rebinds are saved), else the shared `<config>/flint/input.user.toml`
/// that games without an id use, and older builds read for every game.
pub(super) fn resolve_user_override(scene_path: &Path, game_id: &str) -> Option<PathBuf> {
    resolve_user_override_in(scene_path, game_id, dirs::config_dir().as_deref())
}

fn resolve_user_override_in(
    scene_path: &Path,
    game_id: &str,
    config_dir: Option<&Path>,
) -> Option<PathBuf> {
    let local = project_local_override(scene_path);
    if vfs::exists(&local) {
        return Some(local);
    }
    let config_dir = config_dir?;
    let game = user_override_in(config_dir, game_id);
    if vfs::exists(&game) {
        return Some(game);
    }
    Some(user_override_in(config_dir, "")).filter(|shared| *shared != game && vfs::exists(shared))
}

/// Where a rebind is saved: back to the project-local file when that's the
/// one loaded, else this game's own file under the OS config directory. An
/// override loaded from the shared file moves to the game's on its first
/// rebind, carrying what it held.
pub(super) fn user_override_save_path(
    scene_path: &Path,
    game_id: &str,
    loaded: Option<&Path>,
) -> PathBuf {
    user_override_save_path_in(scene_path, game_id, loaded, dirs::config_dir().as_deref())
}

fn user_override_save_path_in(
    scene_path: &Path,
    game_id: &str,
    loaded: Option<&Path>,
    config_dir: Option<&Path>,
) -> PathBuf {
    let local = project_local_override(scene_path);
    if loaded == Some(local.as_path()) {
        return local;
    }
    fallback_user_override_path_in(scene_path, game_id, config_dir)
}

pub(super) fn fallback_user_override_path(scene_path: &Path, game_id: &str) -> PathBuf {
    fallback_user_override_path_in(scene_path, game_id, dirs::config_dir().as_deref())
}

fn fallback_user_override_path_in(
    scene_path: &Path,
    game_id: &str,
    config_dir: Option<&Path>,
) -> PathBuf {
    match config_dir {
        Some(dir) => user_override_in(dir, game_id),
        None => project_local_override(scene_path),
    }
}

/// `<config>/flint/input_<game_id>.toml`, or `input.user.toml` for a game
/// with no id of its own.
fn user_override_in(config_dir: &Path, game_id: &str) -> PathBuf {
    let game_id = game_id.trim();
    let filename = if game_id.is_empty() || game_id == "flint" {
        "input.user.toml".to_string()
    } else {
        format!("input_{game_id}.toml")
    };
    config_dir.join("flint").join(filename)
}

fn project_local_override(scene_path: &Path) -> PathBuf {
    let scene_dir = scene_path.parent().unwrap_or_else(|| Path::new("."));
    scene_dir.join(".flint").join("input.user.toml")
}

pub(super) fn write_user_override_file(path: &Path, config: &InputConfig) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            FlintError::RuntimeError(format!(
                "failed to create directory '{}': {e}",
                parent.display()
            ))
        })?;
    }
    let toml_str = toml::to_string_pretty(config)
        .map_err(|e| FlintError::RuntimeError(format!("failed to serialize input config: {e}")))?;
    std::fs::write(path, toml_str).map_err(|e| {
        FlintError::RuntimeError(format!(
            "failed to write input config '{}': {e}",
            path.display()
        ))
    })?;
    Ok(())
}

pub(super) fn gamepad_id_to_u32(id: gilrs::GamepadId) -> u32 {
    // gilrs GamepadId is opaque; convert via usize
    let raw: usize = id.into();
    raw as u32
}

#[cfg(test)]
mod tests {
    use super::*;
    use flint_runtime::{Binding, InputState, RebindMode};

    /// A fresh directory under the temp dir, gone when dropped.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "flint-input-{name}-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn game_config(game_id: &str) -> InputConfig {
        InputConfig::from_toml_str(&format!(
            "version = 1\ngame_id = \"{game_id}\"\n\n[actions.interact]\nkind = \"button\"\n\
             bindings = [{{ type = \"key\", code = \"KeyE\" }}]\n"
        ))
        .unwrap()
    }

    /// Start the input as the player does: built-in defaults, the game's
    /// config, then whatever user override resolves. Returns the input and
    /// the override's path.
    fn start(scene: &Path, config_dir: &Path, game_id: &str) -> (InputState, Option<PathBuf>) {
        let mut input = InputState::new();
        input
            .load_bindings(InputConfig::built_in_defaults())
            .unwrap();
        input.merge_bindings(game_config(game_id)).unwrap();
        let path = resolve_user_override_in(scene, &input.config().game_id, Some(config_dir));
        if let Some(path) = &path {
            input
                .merge_bindings(InputConfig::load_from_file(path).unwrap())
                .unwrap();
        }
        (input, path)
    }

    fn interact_bindings(input: &InputState) -> Vec<Binding> {
        input.action_config("interact").unwrap().bindings
    }

    fn key(code: &str) -> Binding {
        Binding::Key {
            code: code.into(),
            scale: 1.0,
        }
    }

    #[test]
    fn a_rebind_is_loaded_on_the_next_start() {
        let tmp = TempDir::new("reload");
        let scene = tmp.0.join("game").join("scenes").join("main.scene.toml");
        let config_dir = tmp.0.join("config");

        let (mut input, loaded) = start(&scene, &config_dir, "capitol");
        assert_eq!(loaded, None);
        input
            .rebind_action("interact", key("KeyF"), RebindMode::Replace)
            .unwrap();
        let mut user = InputConfig {
            version: 1,
            game_id: input.config().game_id.clone(),
            actions: Default::default(),
        };
        user.actions
            .insert("interact".into(), input.action_config("interact").unwrap());
        let saved = user_override_save_path_in(&scene, "capitol", None, Some(&config_dir));
        assert_eq!(saved, config_dir.join("flint").join("input_capitol.toml"));
        write_user_override_file(&saved, &user).unwrap();

        let (input, loaded) = start(&scene, &config_dir, "capitol");
        assert_eq!(loaded.as_deref(), Some(saved.as_path()));
        assert_eq!(interact_bindings(&input), vec![key("KeyF")]);

        // Another game keeps its own bindings.
        let (other, loaded) = start(&scene, &config_dir, "doom_fps");
        assert_eq!(loaded, None);
        assert_eq!(interact_bindings(&other), vec![key("KeyE")]);
    }

    #[test]
    fn the_project_local_override_wins_and_is_saved_back_to() {
        let tmp = TempDir::new("local");
        let scene = tmp.0.join("scenes").join("main.scene.toml");
        let config_dir = tmp.0.join("config");
        let local = tmp.0.join("scenes").join(".flint").join("input.user.toml");
        let game = config_dir.join("flint").join("input_capitol.toml");
        let mut user = game_config("capitol");
        write_user_override_file(&game, &user).unwrap();
        user.actions.get_mut("interact").unwrap().bindings = vec![key("KeyG")];
        write_user_override_file(&local, &user).unwrap();

        let (input, loaded) = start(&scene, &config_dir, "capitol");
        assert_eq!(loaded.as_deref(), Some(local.as_path()));
        assert_eq!(interact_bindings(&input), vec![key("KeyG")]);
        assert_eq!(
            user_override_save_path_in(&scene, "capitol", Some(&local), Some(&config_dir)),
            local
        );
    }

    #[test]
    fn the_shared_override_is_a_fallback_that_moves_to_the_game_on_save() {
        let tmp = TempDir::new("shared");
        let scene = tmp.0.join("scenes").join("main.scene.toml");
        let config_dir = tmp.0.join("config");
        let shared = config_dir.join("flint").join("input.user.toml");
        let mut user = game_config("");
        user.actions.get_mut("interact").unwrap().bindings = vec![key("KeyH")];
        write_user_override_file(&shared, &user).unwrap();

        let (input, loaded) = start(&scene, &config_dir, "capitol");
        assert_eq!(loaded.as_deref(), Some(shared.as_path()));
        assert_eq!(interact_bindings(&input), vec![key("KeyH")]);
        assert_eq!(
            user_override_save_path_in(&scene, "capitol", Some(&shared), Some(&config_dir)),
            config_dir.join("flint").join("input_capitol.toml")
        );

        // A game without an id of its own reads and writes the shared file.
        let (_, loaded) = start(&scene, &config_dir, "flint");
        assert_eq!(loaded.as_deref(), Some(shared.as_path()));
        assert_eq!(
            user_override_save_path_in(&scene, "flint", Some(&shared), Some(&config_dir)),
            shared
        );
    }
}
