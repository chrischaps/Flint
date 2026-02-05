//! TOML-based animation clip loading

use crate::clip::AnimationClip;
use flint_core::{FlintError, Result};
use std::path::Path;

/// Load an animation clip from a `.anim.toml` file.
///
/// The file format mirrors the `AnimationClip` struct:
/// ```toml
/// name = "platform_bob"
/// duration = 4.0
///
/// [[tracks]]
/// interpolation = "Linear"
///
/// [tracks.target]
/// type = "Position"
///
/// [[tracks.keyframes]]
/// time = 0.0
/// value = [0.0, 2.0, 0.0]
/// # ...more keyframes
/// ```
pub fn load_clip_from_file(path: &Path) -> Result<AnimationClip> {
    let content = std::fs::read_to_string(path).map_err(|e| {
        FlintError::AnimationError(format!("Failed to read {}: {}", path.display(), e))
    })?;
    load_clip_from_str(&content, path)
}

/// Parse an animation clip from a TOML string.
fn load_clip_from_str(content: &str, path: &Path) -> Result<AnimationClip> {
    let clip: AnimationClip = toml::from_str(content).map_err(|e| {
        FlintError::AnimationError(format!("Failed to parse {}: {}", path.display(), e))
    })?;

    // Validate: duration must be positive
    if clip.duration <= 0.0 {
        return Err(FlintError::AnimationError(format!(
            "Clip '{}' has non-positive duration: {}",
            clip.name, clip.duration
        )));
    }

    // Validate: each track should have at least one keyframe
    for (i, track) in clip.tracks.iter().enumerate() {
        if track.keyframes.is_empty() {
            return Err(FlintError::AnimationError(format!(
                "Clip '{}' track {} has no keyframes",
                clip.name, i
            )));
        }
    }

    Ok(clip)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn parse_minimal_clip() {
        let toml_str = r#"
name = "test"
duration = 1.0

[[tracks]]
interpolation = "Linear"

[tracks.target]
type = "Position"

[[tracks.keyframes]]
time = 0.0
value = [0.0, 0.0, 0.0]

[[tracks.keyframes]]
time = 1.0
value = [1.0, 1.0, 1.0]
"#;
        let clip = load_clip_from_str(toml_str, &PathBuf::from("test.anim.toml")).unwrap();
        assert_eq!(clip.name, "test");
        assert_eq!(clip.duration, 1.0);
        assert_eq!(clip.tracks.len(), 1);
        assert_eq!(clip.tracks[0].keyframes.len(), 2);
    }

    #[test]
    fn reject_zero_duration() {
        let toml_str = r#"
name = "bad"
duration = 0.0

[[tracks]]
interpolation = "Linear"

[tracks.target]
type = "Position"

[[tracks.keyframes]]
time = 0.0
value = [0.0, 0.0, 0.0]
"#;
        let result = load_clip_from_str(toml_str, &PathBuf::from("bad.anim.toml"));
        assert!(result.is_err());
    }

    #[test]
    fn reject_empty_track() {
        let toml_str = r#"
name = "empty_track"
duration = 1.0

[[tracks]]
interpolation = "Linear"
keyframes = []

[tracks.target]
type = "Position"
"#;
        let result = load_clip_from_str(toml_str, &PathBuf::from("empty.anim.toml"));
        assert!(result.is_err());
    }
}
