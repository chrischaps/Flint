//! A scene's sounds into the engine (moved from the player with the
//! package, #1619): each `audio_source`'s file, then (unless the scene opts
//! out) every file under the `audio/` folders it can see, for the sounds
//! scripts play by name.

use crate::decode::Need;
use crate::AudioSystem;
use flint_core::components as comp;
use flint_core::project::{AssetKind, Project};
use flint_ecs::FlintWorld;
use std::path::Path;

/// Load audio files referenced by audio_source components and (unless the
/// scene opts out via `[scene] preload_audio = false`) preload every audio
/// file from audio/ for script-triggered sounds.
///
/// With `queue`, nothing is decoded here: each sound joins the engine's
/// decode queue, which `Stage::Sound` works through a few milliseconds a
/// frame, sources that are heard at once (autoplay, above silence) first,
/// then the rest smallest first. That is how a late audio bundle arrives
/// mid-game without a stall (#124). Without it, everything is decoded now,
/// as a scene load can afford.
pub fn load_audio_from_world(
    world: &FlintWorld,
    audio: &mut AudioSystem,
    project: &Project,
    scene_path: &str,
    preload_all: bool,
    queue: bool,
) {
    // A sound a script plays by a name nothing here caches is looked for
    // by the same search on its first play (#975), which is the search
    // `flint check` holds a script's sound to. Set again when a late bundle
    // lands, which forgets the names it found nothing for.
    audio
        .engine
        .set_sound_search(Some(flint_core::project::AssetScope::new(
            project.clone(),
            scene_dir_of(scene_path),
        )));

    let mut load = |name: &str, path: &Path, need: Need| {
        if audio.engine.has_sound(name) {
            return;
        }
        let result = if queue {
            audio.engine.queue_sound(name, path, need)
        } else {
            audio.engine.load_sound(name, path)
        };
        match result {
            Err(e) => tracing::warn!("Failed to load audio '{}': {:?}", name, e),
            Ok(()) if !queue => {
                tracing::debug!(target: "load", kind = "sound", "Loaded audio: {}", name)
            }
            Ok(()) => {}
        }
    };
    let scene_dir = scene_dir_of(scene_path);
    // Load audio files referenced by audio_source components: a `file` is a
    // path from the scene's directory or from any directory up to the
    // project root (`audio/hit.ogg`), by the project's one search.
    for entity in world.all_entities() {
        // Through the typed view (ADR 0115), as flint-audio reads it
        let source = world.view(entity.id, comp::AUDIO_SOURCE);
        let Some(file_name) = source.opt_str("file") else {
            continue;
        };
        // Heard the moment it loads: an autoplay source above silence.
        let audible = source.bool("autoplay", true) && source.f64("volume", 1.0) > 0.0;
        let need = if audible { Need::Audible } else { Need::Later };

        match project.resolve(AssetKind::Audio, file_name, scene_dir) {
            Ok(audio_path) => load(file_name, &audio_path, need),
            Err(missing) => tracing::warn!("{missing}"),
        }
    }

    // Preload all audio files from the audio/ directory (for script-triggered
    // sounds). Scenes with no audio can skip this — decoding a large stem
    // library dominates their load time.
    if !preload_all {
        tracing::info!("Scene opts out of audio preload (preload_audio = false)");
        return;
    }
    // Every `audio/` the scene can see, a nearer file hiding a farther one
    // of the same file name. Each is kept as `audio/<file>`, whichever
    // `audio/` it came from, which is the name a script plays it by.
    for sound in project.list(AssetKind::Audio, scene_dir) {
        load(&format!("audio/{}", sound.name), &sound.path, Need::Later);
    }
}

/// The directory a scene file is in (`.` for a bare file name).
fn scene_dir_of(scene_path: &str) -> &Path {
    Path::new(scene_path)
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
}
