//! A scene's animation, loaded (moved from the player's `scene_loading.rs`
//! by #1620): every `.anim.toml` and `.sequence.toml` the scene can see, each found by the
//! project's one asset search (ADR 0114). Each loader has a `_saying` form
//! that hands its lines to a closure (`flint render --report json` keeps
//! them); the player's is [`loaded`], a `load` event a line.

use crate::AnimationSystem;
use flint_core::project::{AssetKind, Project};
use std::path::{Path, PathBuf};

/// The player's `say` for the `_saying` loaders: each line a `load` event
/// of `kind`, which the load's summary counts and `RUST_LOG=load=debug`
/// shows (#716). `flint render` hands its own `say`, for its report.
pub fn loaded(kind: &'static str) -> impl FnMut(String) {
    move |line| tracing::debug!(target: "load", kind, "{}", line.trim_start())
}

/// Load every `.anim.toml` clip the scene can see: `animations/` beside the
/// scene, beside each directory above it up to the project root, and in the
/// mounted engine (the project's one asset search, ADR 0114). A nearer file
/// hides a farther one of the same file name.
pub fn load_animations_from_world(
    project: &Project,
    scene_path: &str,
    animation: &mut AnimationSystem,
) {
    load_animations_saying(project, scene_path, animation, &mut loaded("animation"));
}

/// [`load_animations_from_world`], telling `say` what it loads.
pub fn load_animations_saying(
    project: &Project,
    scene_path: &str,
    animation: &mut AnimationSystem,
    say: &mut dyn FnMut(String),
) {
    for path in visible_files(project, AssetKind::Animations, scene_path) {
        match crate::loader::load_clip_from_file(&path) {
            Ok(clip) => {
                say(format!(
                    "Loaded animation: {} ({:.1}s)",
                    clip.name, clip.duration
                ));
                animation.add_property_clip(clip);
            }
            Err(e) => {
                tracing::warn!("Failed to load animation '{}': {:?}", path.display(), e);
            }
        }
    }
}

/// Load every `.sequence.toml` the scene can see, from the same folders as
/// [`load_animations_from_world`]. Sequences are referenced by `name` from
/// `animator.sequence`; a name no file gives is warned about with the
/// folders that were read.
pub fn load_sequences_from_world(
    project: &Project,
    scene_path: &str,
    animation: &mut AnimationSystem,
) {
    load_sequences_saying(project, scene_path, animation, &mut loaded("sequence"));
}

/// [`load_sequences_from_world`], telling `say` what it loads.
pub fn load_sequences_saying(
    project: &Project,
    scene_path: &str,
    animation: &mut AnimationSystem,
    say: &mut dyn FnMut(String),
) {
    animation
        .set_sequence_search(project.search_dirs(AssetKind::Sequences, scene_dir_of(scene_path)));
    for path in visible_files(project, AssetKind::Sequences, scene_path) {
        match crate::load_sequence_from_file(&path) {
            Ok(seq) => {
                say(format!(
                    "Loaded sequence: {} ({:.1}s, {} events)",
                    seq.name,
                    seq.resolved_duration(),
                    seq.events.len()
                ));
                animation.add_sequence(seq);
            }
            Err(e) => {
                tracing::warn!("Failed to load sequence '{}': {:?}", path.display(), e);
            }
        }
    }
}

/// The directory a scene file is in (`.` for a bare file name).
fn scene_dir_of(scene_path: &str) -> &Path {
    Path::new(scene_path)
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
}

/// Every file of `kind` the scene can see (`Project::list`), in the order a
/// loader should register them when a later one of a name replaces an
/// earlier: the farthest folder first, so that the nearest has the last
/// word, and by file name within a folder as before.
pub fn visible_files(project: &Project, kind: AssetKind, scene_path: &str) -> Vec<PathBuf> {
    let listed = project.list(kind, scene_dir_of(scene_path));
    let mut folders: Vec<Vec<PathBuf>> = Vec::new();
    for file in listed {
        match folders.last_mut() {
            Some(folder) if folder[0].parent() == file.path.parent() => folder.push(file.path),
            _ => folders.push(vec![file.path]),
        }
    }
    folders.into_iter().rev().flatten().collect()
}
