//! Scene and asset loading helpers — terrain, audio, animation, model resolution,
//! skeletal data registration, and PostProcessDef conversion.

use flint_asset::HeldAssets;
use flint_audio::decode::Need;
use flint_audio::AudioSystem;
use flint_core::components as comp;
use flint_core::project::{AssetKind, Project};
use flint_core::vfs;
use flint_core::EntityId;
use flint_ecs::FlintWorld;
use flint_render::model_loader::ModelLoadConfig;
use flint_render::SceneRenderer;
use flint_script::ScriptSystem;
use std::path::Path;

/// Convert a `PostProcessDef` (scene TOML) to a `PostProcessConfig` (renderer).
/// Moved to flint-render so the viewer shares it; re-exported to keep the
/// `flint_player::post_process_config_from_def` path (flint-cli uses it).
pub use flint_render::post_process_config_from_def;

/// The built-in styles with the user-authored ones (`styles/*.shader.toml`,
/// ADR 0112) a scene at `scene_path` can see (#742): the registry to read
/// its `[post_process]` against, where a table naming a style it doesn't
/// hold is dropped with a warning. What didn't load is warned about.
pub fn load_user_styles(scene_path: &str) -> flint_render::stylize::StyleRegistry {
    load_user_styles_in(&flint_core::project::AssetScope::for_scene(Path::new(
        scene_path,
    )))
}

/// [`load_user_styles`] for a host that holds the scene's project: every
/// `styles/*.shader.toml` the project's one asset search lists from
/// `scope`, the nearest of a name winning (ADR 0114, #426).
pub fn load_user_styles_in(
    scope: &flint_core::project::AssetScope,
) -> flint_render::stylize::StyleRegistry {
    let (styles, messages) = flint_render::stylize::StyleRegistry::new().for_scene(scope);
    for message in messages {
        tracing::warn!("{message}");
    }
    styles
}

/// A scene's `[post_process]` as the player draws it: the look the scene
/// names laid under its own keys (#356), then the `--look` named, if any,
/// merged over the result as `flint render --look` does (#435). A `--look`
/// that won't apply leaves the scene's own, with a warning (the player
/// checked the name at startup). The result keeps the scene's authored
/// keys, so a later `set_look` or F4 Look re-resolves from the scene.
/// Looks are searched for from `scope`, the scene's project and folder.
pub fn scene_post_process_in(
    look: Option<&str>,
    scene_path: &str,
    scope: &flint_core::project::AssetScope,
    pp: Option<flint_scene::PostProcessDef>,
) -> Option<flint_scene::PostProcessDef> {
    let pp = pp.map(|d| flint_render::look::resolve_scene_in(&d, Path::new(scene_path), scope));
    let Some(name) = look else {
        return pp;
    };
    match flint_render::look::find_in(name, scope).and_then(|l| l.apply(pp.as_ref())) {
        Ok(mut def) => {
            def.look = Some(name.to_string());
            def.authored = pp.and_then(|p| p.authored);
            Some(def)
        }
        Err(e) => {
            tracing::warn!("--look {name}: {e}");
            pp
        }
    }
}

/// The grass a scene loaded, for its debug panel. The fields are only read
/// by the `debug-hud` grass panel construction.
pub(super) struct LoadedGrassInfo {
    pub config: flint_render::GrassConfig,
    pub terrain_entity_name: String,
    /// The component holding the `grass.*` fields: `terrain` or `grass_ground`.
    pub component: &'static str,
}

/// The surface of the ground a scene binder made (the terrain package's,
/// #1623): its drawn vertex grid where its transform puts it, which the
/// physics package makes a heightfield collider of (#1219, #1617), the same
/// surface as the chunks' triangles with no seams.
pub(super) fn terrain_surface(
    ground: &flint_geometry::ground::SceneGround,
) -> flint_package::surfaces::SurfaceGrid {
    let grid = &ground.grid;
    flint_package::surfaces::SurfaceGrid {
        entity: ground.entity,
        columns: grid.columns,
        rows: grid.rows,
        heights: grid.heights.clone(),
        width: grid.width,
        depth: grid.depth,
        origin: ground.origin,
        friction: 0.8,
        restitution: 0.1,
    }
}

/// Load a scene's `grass_ground` into `scene_renderer`, as the player does
/// (ADR 0089). Returns whether grass was set. Kept for games and
/// `flint render`; the loader is `flint_render::load_scene_grass_ground`,
/// which the scene viewer shares (#332).
pub fn load_scene_grass_ground(
    world: &FlintWorld,
    scene_path: &str,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    scene_renderer: &mut SceneRenderer,
) -> bool {
    flint_render::load_scene_grass_ground(world, scene_path, device, queue, scene_renderer)
        .is_some()
}

/// What a model's not-found warning says was asked before its files when
/// an asset maker runs: the procgen package's specs (#32).
pub(super) const SPECS_FIRST: &str = "a procgen spec of the same name (specs/<name>.procgen.toml)";

/// Keep [`SPECS_FIRST`] in `config`'s not-found note only when an asset
/// maker runs (`makers`): in a build without one, or where the project's
/// `[packages]` leaves its package out (#1578), no spec was asked.
pub(super) fn say_makers_asked(config: &mut ModelLoadConfig, makers: bool) {
    if !makers {
        config.asked_first.retain(|asked| *asked != SPECS_FIRST);
    }
}

/// Build a ModelLoadConfig with catalog-resolved path overrides: `held` is
/// the project's catalog and store (`AssetSources::of`, #1065), the same
/// `flint validate` and `flint check` ask.
pub(super) fn build_model_load_config(
    project: &Project,
    scene_path: &str,
    world: &FlintWorld,
    held: Option<&HeldAssets>,
) -> ModelLoadConfig {
    let ids: Vec<EntityId> = world.all_entities().iter().map(|e| e.id).collect();
    build_model_load_config_of(project, scene_path, world, &ids, held)
}

/// [`build_model_load_config`] with the catalog asked only for the names
/// the entities `ids` use: a chunk's, or a script's spawns (#1107).
pub(super) fn build_model_load_config_of(
    project: &Project,
    scene_path: &str,
    world: &FlintWorld,
    ids: &[EntityId],
    held: Option<&HeldAssets>,
) -> ModelLoadConfig {
    let mut config = ModelLoadConfig::in_project(project.clone(), scene_path);
    // The player asks procgen for every name before its file (#32), and the
    // catalog before that: a not-found warning says so beside the paths.
    config.asked_first.push(SPECS_FIRST);

    let Some(held) = held else {
        return config;
    };
    config.asked_first.insert(0, "the asset catalog");

    for &id in ids {
        let model = world.view(id, comp::MODEL).opt_str("asset");
        let textures =
            [comp::MATERIAL, comp::SPRITE].map(|table| world.view(id, table).opt_str("texture"));
        for name in std::iter::once(model).chain(textures).flatten() {
            if !config.overrides.contains_key(name) {
                if let Some(path) = held.path_of(name) {
                    config.overrides.insert(name.to_string(), path);
                }
            }
        }
    }

    config
}

/// Load audio files referenced by audio_source components and (unless the
/// scene opts out via `[scene] preload_audio = false`) preload every audio
/// file from audio/ for script-triggered sounds.
///
/// With `queue`, nothing is decoded here: each sound joins the engine's
/// decode queue, which `tick_av_systems` works through a few milliseconds a
/// frame, sources that are heard at once (autoplay, above silence) first,
/// then the rest smallest first. That is how a late audio bundle arrives
/// mid-game without a stall (#124). Without it, everything is decoded now,
/// as a scene load can afford.
pub(super) fn load_audio_from_world(
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

/// The scene a transition names, from the current scene: a path found by
/// the project's one search (`AssetKind::Scenes`, ADR 0114), which tries it
/// from the current scene's directory, then from each directory above it up
/// to the project root, then in the engine. One that isn't found there is
/// returned as written (a path from the working directory, as it was
/// before), for the scene loader to open or report.
pub(super) fn resolve_scene_path(project: &Project, current_scene: &str, target: &str) -> String {
    let answer = project.explain(AssetKind::Scenes, target, scene_dir_of(current_scene));
    match answer.found {
        Some(found) => {
            if let Some(change) = &answer.order_change {
                change.log();
            }
            found.to_string_lossy().to_string()
        }
        None => {
            if !vfs::exists(target) {
                let not_found = flint_core::project::NotFound {
                    kind: AssetKind::Scenes,
                    name: answer.name,
                    tried: answer.tried,
                };
                tracing::warn!("{not_found}");
            }
            target.to_string()
        }
    }
}

/// Point the script system at the scripts the scene can see in `project`
pub(super) fn load_scripts_from_world(
    project: &Project,
    scene_path: &str,
    script: &mut ScriptSystem,
) {
    script.load_scripts_from_scene(scene_path, project);
}
