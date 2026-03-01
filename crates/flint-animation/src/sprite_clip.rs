//! Sprite sheet animation clips
//!
//! A `SpriteAnimClip` defines a sequence of frames in a sprite sheet,
//! each with a column/row position and a per-frame duration. Clips are
//! loaded from `.sprite.toml` files and driven by `SpriteAnimSync`.

use flint_core::{FlintError, Result};
use serde::Deserialize;
use std::path::Path;

/// A single frame in a sprite animation clip.
#[derive(Debug, Clone, Deserialize)]
pub struct SpriteAnimFrame {
    pub col: u32,
    pub row: u32,
    pub duration_ms: u32,
}

/// How a clip behaves when it reaches the end.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LoopMode {
    Loop,
    PingPong,
    Once,
}

/// A named animation clip referencing frames in a sprite sheet.
#[derive(Debug, Clone, Deserialize)]
pub struct SpriteAnimClip {
    pub name: String,
    pub texture: String,
    pub frame_width: u32,
    pub frame_height: u32,
    pub frames: Vec<SpriteAnimFrame>,
    pub loop_mode: LoopMode,
}

impl SpriteAnimClip {
    /// Total duration of one pass through the clip, in milliseconds.
    pub fn total_duration_ms(&self) -> u32 {
        self.frames.iter().map(|f| f.duration_ms).sum()
    }
}

/// Intermediate TOML structure: a file may contain multiple `[animation.NAME]` sections.
#[derive(Deserialize)]
struct SpriteAnimFile {
    animation: std::collections::HashMap<String, RawSpriteAnim>,
}

#[derive(Deserialize)]
struct RawSpriteAnim {
    texture: String,
    frame_size: [u32; 2],
    frames: Vec<SpriteAnimFrame>,
    #[serde(default = "default_loop_mode")]
    loop_mode: LoopMode,
}

fn default_loop_mode() -> LoopMode {
    LoopMode::Loop
}

/// Load all sprite animation clips from a `.sprite.toml` file.
///
/// A single file can define multiple clips under `[animation.NAME]` sections.
pub fn load_sprite_clips_from_file(path: &Path) -> Result<Vec<SpriteAnimClip>> {
    let content = std::fs::read_to_string(path).map_err(|e| {
        FlintError::AnimationError(format!("Failed to read {}: {}", path.display(), e))
    })?;
    load_sprite_clips_from_str(&content, path)
}

fn load_sprite_clips_from_str(content: &str, path: &Path) -> Result<Vec<SpriteAnimClip>> {
    let file: SpriteAnimFile = toml::from_str(content).map_err(|e| {
        FlintError::AnimationError(format!("Failed to parse {}: {}", path.display(), e))
    })?;

    let mut clips = Vec::new();
    for (name, raw) in file.animation {
        if raw.frames.is_empty() {
            return Err(FlintError::AnimationError(format!(
                "Sprite clip '{}' in {} has no frames",
                name,
                path.display()
            )));
        }
        if raw.frame_size[0] == 0 || raw.frame_size[1] == 0 {
            return Err(FlintError::AnimationError(format!(
                "Sprite clip '{}' in {} has zero frame size",
                name,
                path.display()
            )));
        }
        clips.push(SpriteAnimClip {
            name,
            texture: raw.texture,
            frame_width: raw.frame_size[0],
            frame_height: raw.frame_size[1],
            frames: raw.frames,
            loop_mode: raw.loop_mode,
        });
    }
    Ok(clips)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn parse_sprite_clip() {
        let toml_str = r#"
[animation.player_run]
texture = "assets/textures/player_sheet.png"
frame_size = [64, 64]
frames = [
    { col = 0, row = 1, duration_ms = 100 },
    { col = 1, row = 1, duration_ms = 100 },
    { col = 2, row = 1, duration_ms = 100 },
    { col = 3, row = 1, duration_ms = 100 },
]
loop_mode = "loop"
"#;
        let clips =
            load_sprite_clips_from_str(toml_str, &PathBuf::from("test.sprite.toml")).unwrap();
        assert_eq!(clips.len(), 1);
        let clip = &clips[0];
        assert_eq!(clip.name, "player_run");
        assert_eq!(clip.texture, "assets/textures/player_sheet.png");
        assert_eq!(clip.frame_width, 64);
        assert_eq!(clip.frame_height, 64);
        assert_eq!(clip.frames.len(), 4);
        assert_eq!(clip.loop_mode, LoopMode::Loop);
        assert_eq!(clip.total_duration_ms(), 400);
    }

    #[test]
    fn parse_multiple_clips() {
        let toml_str = r#"
[animation.idle]
texture = "sheet.png"
frame_size = [32, 32]
frames = [
    { col = 0, row = 0, duration_ms = 200 },
    { col = 1, row = 0, duration_ms = 200 },
]

[animation.jump]
texture = "sheet.png"
frame_size = [32, 32]
frames = [
    { col = 0, row = 2, duration_ms = 150 },
    { col = 1, row = 2, duration_ms = 300 },
]
loop_mode = "once"
"#;
        let clips =
            load_sprite_clips_from_str(toml_str, &PathBuf::from("test.sprite.toml")).unwrap();
        assert_eq!(clips.len(), 2);
        let jump = clips.iter().find(|c| c.name == "jump").unwrap();
        assert_eq!(jump.loop_mode, LoopMode::Once);
    }

    #[test]
    fn parse_ping_pong() {
        let toml_str = r#"
[animation.sway]
texture = "tree.png"
frame_size = [48, 48]
frames = [
    { col = 0, row = 0, duration_ms = 120 },
    { col = 1, row = 0, duration_ms = 120 },
    { col = 2, row = 0, duration_ms = 120 },
]
loop_mode = "ping_pong"
"#;
        let clips =
            load_sprite_clips_from_str(toml_str, &PathBuf::from("test.sprite.toml")).unwrap();
        assert_eq!(clips[0].loop_mode, LoopMode::PingPong);
    }

    #[test]
    fn reject_empty_frames() {
        let toml_str = r#"
[animation.empty]
texture = "sheet.png"
frame_size = [32, 32]
frames = []
"#;
        let result = load_sprite_clips_from_str(toml_str, &PathBuf::from("bad.sprite.toml"));
        assert!(result.is_err());
    }

    #[test]
    fn reject_zero_frame_size() {
        let toml_str = r#"
[animation.bad]
texture = "sheet.png"
frame_size = [0, 32]
frames = [
    { col = 0, row = 0, duration_ms = 100 },
]
"#;
        let result = load_sprite_clips_from_str(toml_str, &PathBuf::from("bad.sprite.toml"));
        assert!(result.is_err());
    }

    #[test]
    fn default_loop_mode_is_loop() {
        let toml_str = r#"
[animation.test]
texture = "sheet.png"
frame_size = [16, 16]
frames = [
    { col = 0, row = 0, duration_ms = 100 },
]
"#;
        let clips =
            load_sprite_clips_from_str(toml_str, &PathBuf::from("test.sprite.toml")).unwrap();
        assert_eq!(clips[0].loop_mode, LoopMode::Loop);
    }
}
