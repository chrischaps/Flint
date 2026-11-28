//! Scene and asset loading helpers — terrain, animation, model resolution,
//! skeletal data registration, and PostProcessDef conversion.

use flint_asset::HeldAssets;
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
/// surface as the chunks' triangles with no seams. The terrain package's
/// rebake (#1697) makes it again the same way: `SurfaceGrid::of_ground`.
pub(super) fn terrain_surface(
    ground: &flint_geometry::ground::SceneGround,
) -> flint_package::surfaces::SurfaceGrid {
    flint_package::surfaces::SurfaceGrid::of_ground(ground)
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
