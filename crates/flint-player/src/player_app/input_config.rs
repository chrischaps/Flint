//! Input configuration helpers — path resolution, user override persistence, and rebind state.

use anyhow::Result;
use flint_core::FlintError;
use flint_runtime::InputConfig;
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
            if p.exists() {
                p
            } else if let Some(parent) = scene_dir.parent() {
                parent.join(name)
            } else {
                p
            }
        })
        .or_else(|| {
            let candidate = scene_dir.join("config").join("input.toml");
            if candidate.exists() {
                Some(candidate)
            } else {
                scene_dir
                    .parent()
                    .map(|p| p.join("config").join("input.toml"))
                    .filter(|p| p.exists())
            }
        });

    // User override: ~/.flint/input_{game_id}.toml (resolved later once game_id is known)
    // For now, try the project-local fallback
    let user_override = {
        let local = scene_dir.join(".flint").join("input.user.toml");
        if local.exists() {
            Some(local)
        } else {
            dirs::config_dir()
                .map(|d| d.join("flint").join("input.user.toml"))
                .filter(|p| p.exists())
        }
    };

    let cli = cli_override.map(PathBuf::from);

    InputConfigPaths {
        game_default,
        user_override,
        cli_override: cli,
    }
}

pub(super) fn fallback_user_override_path(scene_path: &Path, game_id: &str) -> Option<PathBuf> {
    if let Some(config_dir) = dirs::config_dir() {
        let dir = config_dir.join("flint");
        let filename = if game_id.is_empty() || game_id == "flint" {
            "input.user.toml".to_string()
        } else {
            format!("input_{game_id}.toml")
        };
        return Some(dir.join(filename));
    }
    // Fallback to project-local
    let scene_dir = scene_path.parent().unwrap_or_else(|| Path::new("."));
    Some(scene_dir.join(".flint").join("input.user.toml"))
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
