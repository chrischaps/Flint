//! A scene's terrain, built into a renderer (#1615, the `render` feature).
//!
//! flint-render built a scene's terrain itself inside
//! `SceneRenderer::bind_scene` (#739), which made the kernel's renderer link
//! this crate. It now draws only the plain data it's handed (the chunks'
//! [`TerrainChunk`](crate::TerrainChunk)s and the
//! [`GrassConfig`](crate::GrassConfig), both flint-geometry's), and the
//! terrain is built here. It is the terrain package's scene binder (#1623,
//! [`crate::player::bind`], `Registry::scene_binder`), which every host that
//! binds a scene runs right after `bind_scene` and before the scene's
//! models: the player, the scene viewer and `flint render`, as they did when
//! the loader was the renderer's. None of them names this crate.

use crate::{GrassConfig, Heightmap, HeightmapFiles, Terrain, TerrainConfig};
use flint_core::components as comp;
use flint_core::{EntityId, Transform, Vec3};
use flint_ecs::FlintWorld;
use flint_render::{BoundGrass, BoundScene, SceneRenderer};

/// A terrain [`bind_terrain`] loaded: its entity, what it was generated
/// from, and the generated chunks (whose collider a host with physics
/// registers).
pub struct LoadedTerrain {
    pub entity: EntityId,
    pub name: String,
    pub terrain: Terrain,
    pub config: TerrainConfig,
    /// The heightmap's size in samples, width by depth.
    pub heightmap_size: (u32, u32),
    /// Where the entity's transform puts it.
    pub offset: [f32; 3],
}

impl LoadedTerrain {
    /// The kernel's ground (#1623): what the host is handed, for physics'
    /// heightfield, `terrain_height` and what `flint render` says.
    pub fn ground(&self) -> flint_geometry::ground::SceneGround {
        flint_geometry::ground::SceneGround {
            entity: self.entity,
            name: self.name.clone(),
            grid: self.terrain.height_grid(),
            origin: self.offset,
            heightmap: self.terrain.heightmap.clone(),
            size: [self.config.width, self.config.depth],
            height_scale: self.config.height_scale,
            chunks: self.terrain.chunks.len(),
        }
    }
}

/// Load the first `terrain` entity of `world` into `renderer`: its chunks,
/// its material, and its grass when `grass.enabled` and the splat map loads,
/// and mark it as the entity a `style_mask` on it masks (#310). Files are
/// found by the project's one search (`AssetKind::Files`) from the scene's
/// folder, the scope `bind_scene` gave the renderer, so call it after
/// `bind_scene`, which unloads the last scene's terrain and grass first.
///
/// One grass field a scene (ADR 0089): a `grass_ground`, which `bind_scene`
/// loaded into `bound.grass`, replaces a terrain's grass, so the terrain's
/// is loaded only when `bound.grass` is empty, and then set there.
pub fn bind_terrain(
    renderer: &mut SceneRenderer,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    world: &FlintWorld,
    bound: &mut BoundScene,
) -> Option<LoadedTerrain> {
    flint_core::scope!("load_terrain");
    let scope = renderer.asset_scope();
    let project = &scope.project;
    let scene_dir = scope.scene_dir.as_path();

    for entity in world.all_entities() {
        // Through the typed view (ADR 0115): a wrong type is reported once
        let terrain_comp = world.view(entity.id, comp::TERRAIN);
        if !terrain_comp.exists() {
            continue;
        }
        let config = TerrainConfig::from_component(&terrain_comp);
        if config.heightmap_path.is_empty() {
            tracing::warn!("Terrain entity '{}': missing heightmap path", entity.name);
            continue;
        }
        let hm_path =
            project.file_or_beside("terrain heightmap", &config.heightmap_path, scene_dir);
        let heightmap = match Heightmap::from_png(&hm_path) {
            Ok(hm) => hm,
            Err(e) => {
                tracing::warn!("Failed to load heightmap: {}", e);
                continue;
            }
        };

        let terrain = Terrain::generate(&heightmap, &config);

        // Only the position: a terrain is never rotated or scaled
        let pos = world
            .view(entity.id, comp::TRANSFORM)
            .vec3("position", [0.0; 3]);
        let transform = Transform {
            position: Vec3::new(pos[0], pos[1], pos[2]),
            ..Default::default()
        };

        renderer.load_terrain(
            device,
            queue,
            &terrain.chunks,
            &transform,
            config.texture_tile,
            config.metallic,
            config.roughness,
            &config.splat_map_path,
            &config.layer_textures,
            scene_dir,
        );
        // A style_mask on the terrain entity masks its chunks (#310)
        renderer.set_terrain_entity(Some(entity.id));

        let grass_config = GrassConfig::from_component(&terrain_comp, false);
        if grass_config.enabled {
            if let Some(ground) = &bound.grass {
                tracing::warn!(
                    "grass_ground '{}' replaces the terrain's grass",
                    ground.entity_name
                );
            } else {
                load_terrain_grass(
                    renderer,
                    device,
                    queue,
                    &grass_config,
                    &heightmap,
                    &config,
                    pos,
                    &project.file_or_beside("terrain splat map", &config.splat_map_path, scene_dir),
                );
                // The terrain's grass is in only when its splat map loaded
                if renderer.grass_config().is_some() {
                    bound.grass = Some(BoundGrass {
                        config: grass_config,
                        entity_name: entity.name.clone(),
                        component: comp::TERRAIN,
                    });
                }
            }
        }

        tracing::info!(
            "Loaded terrain '{}': {}x{} heightmap, {} chunks, {}x{} world units",
            entity.name,
            heightmap.width,
            heightmap.depth,
            terrain.chunks.len(),
            config.width,
            config.depth,
        );
        return Some(LoadedTerrain {
            entity: entity.id,
            name: entity.name.clone(),
            heightmap_size: (heightmap.width, heightmap.depth),
            terrain,
            config,
            offset: pos,
        });
    }
    None
}

/// The terrain's grass, grown over its splat map (warned when the map
/// doesn't read, and then there is none).
#[allow(clippy::too_many_arguments)]
fn load_terrain_grass(
    renderer: &mut SceneRenderer,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    grass: &GrassConfig,
    heightmap: &Heightmap,
    config: &TerrainConfig,
    pos: [f32; 3],
    splat_path: &std::path::Path,
) {
    match flint_render::open_image(splat_path) {
        Ok(splat_img) => {
            let splat_rgba = splat_img.to_rgba8();
            let (sw, sh) = splat_rgba.dimensions();
            renderer.load_grass(
                device,
                queue,
                grass,
                &heightmap.clone_heights(),
                heightmap.width,
                heightmap.depth,
                splat_rgba.as_raw(),
                sw,
                sh,
                pos,
                config.width,
                config.depth,
                config.height_scale,
            );
            tracing::info!(
                "Grass enabled: density={}, max_dist={}",
                grass.density,
                grass.max_distance
            );
        }
        Err(_) => {
            tracing::warn!("Grass enabled but splat map not found at {:?}", splat_path)
        }
    }
}
