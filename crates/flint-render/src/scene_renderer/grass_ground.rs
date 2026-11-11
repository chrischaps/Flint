//! A scene's `grass_ground` (ADR 0089), loaded the one way the player,
//! `flint render` and the scene viewer all use (#332: the viewer had none,
//! so a meadow was bare in `flint edit` and full in `flint render`).

use super::SceneRenderer;
use crate::grass_pipeline::{GrassGround, GrassHeight};
use flint_core::components as comp;
use flint_core::project::Project;
use flint_core::toml_util::{toml_f32, toml_vec3};
use flint_ecs::FlintWorld;
use std::path::Path;

/// What [`load_grass_ground_in`] set: the grass settings and the entity
/// they came from (the player's grass panel edits them).
pub struct LoadedGrassGround {
    pub config: flint_terrain::GrassConfig,
    pub entity_name: String,
}

/// Load a scene's `grass_ground` into `renderer`, finding the project from
/// the scene's path. Returns what was set, or `None` when the scene has no
/// enabled `grass_ground` or its mask can't be read (warned).
pub fn load_scene_grass_ground(
    world: &FlintWorld,
    scene_path: &str,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    renderer: &mut SceneRenderer,
) -> Option<LoadedGrassGround> {
    let scene_dir = Path::new(scene_path)
        .parent()
        .unwrap_or_else(|| Path::new("."));
    let project = Project::for_scene(Path::new(scene_path));
    load_grass_ground_in(world, &project, scene_dir, device, queue, renderer)
}

/// Load the first `grass_ground` entity's mask and hand it to the renderer
/// (ADR 0089). The ground is level at the entity's `y`, from its `x`, `z`
/// across `width` × `depth`. The mask is found by the project's search
/// (`Project::file_or_beside`), read through the vfs.
pub fn load_grass_ground_in(
    world: &FlintWorld,
    project: &Project,
    scene_dir: &Path,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    renderer: &mut SceneRenderer,
) -> Option<LoadedGrassGround> {
    for entity in world.all_entities() {
        let Some(ground) = world.get_component(entity.id, comp::GRASS_GROUND) else {
            continue;
        };
        let config = flint_terrain::GrassConfig::from_component(ground, true);
        if !config.enabled {
            return None;
        }
        let mask_rel = ground.get("mask").and_then(|v| v.as_str()).unwrap_or("");
        if mask_rel.is_empty() {
            tracing::warn!("grass_ground '{}': missing mask path", entity.name);
            return None;
        }
        let mask_path = project.file_or_beside("grass_ground mask", mask_rel, scene_dir);
        let mask = match crate::open_image(&mask_path) {
            Ok(img) => img.to_luma8(),
            Err(e) => {
                tracing::warn!(
                    "grass_ground '{}': mask {:?}: {}",
                    entity.name,
                    mask_path,
                    e
                );
                return None;
            }
        };
        let get_f32 = |key: &str, default: f32| -> f32 {
            ground.get(key).and_then(toml_f32).unwrap_or(default)
        };
        let pos = world
            .get_component(entity.id, comp::TRANSFORM)
            .and_then(|t| t.get("position").and_then(toml_vec3))
            .unwrap_or([0.0; 3]);
        let (mask_width, mask_height) = mask.dimensions();
        let result = renderer.set_grass_ground(
            device,
            queue,
            &config,
            &GrassGround {
                origin: [pos[0], pos[2]],
                size: [get_f32("width", 64.0), get_f32("depth", 64.0)],
                mask: mask.as_raw(),
                mask_width,
                mask_height,
                scale_by_mask: ground
                    .get("scale_by_mask")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(true),
                height: GrassHeight::Flat(pos[1]),
            },
        );
        if let Err(e) = result {
            tracing::warn!("grass_ground '{}': {}", entity.name, e);
            return None;
        }
        tracing::info!(
            "Grass ground '{}': {}x{} mask, density={}, max_dist={}",
            entity.name,
            mask_width,
            mask_height,
            config.density,
            config.max_distance
        );
        return Some(LoadedGrassGround {
            config,
            entity_name: entity.name.clone(),
        });
    }
    None
}
