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
//!
//! What the terrain is made of is [`crate::source`]'s (#1697): its settings
//! and maps from its component over its recipe ([`make_terrain`]), which
//! needs no GPU; this file uploads them ([`build_terrain`]), at the bind
//! and again when the player's subsystem sees the recipe saved.

use crate::source::{make_terrain, MadeTerrain, SplatMap};
use crate::{GrassConfig, Heightmap, Terrain, TerrainConfig};
use flint_core::components as comp;
use flint_core::{EntityId, Transform, Vec3};
use flint_ecs::FlintWorld;
use flint_render::{BoundGrass, BoundScene, SceneRenderer, TerrainSplat};
use std::path::PathBuf;

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
    /// The recipe it was made from (#1697), as the project's search found
    /// it: what the player's subsystem watches.
    pub spec: Option<PathBuf>,
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
/// `bind_scene`, which unloads the last scene's terrain and grass first. A
/// terrain that names its recipe (`spec`, #1697) is baked from it.
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

    for entity in world.all_entities() {
        // Through the typed view (ADR 0115): a wrong type is reported once
        let view = world.view(entity.id, comp::TERRAIN);
        if !view.exists() {
            continue;
        }
        let made = match make_terrain(&view, &scope.project, &scope.scene_dir) {
            Ok(made) => made,
            Err(e) => {
                tracing::warn!("Terrain entity '{}': {e}", entity.name);
                continue;
            }
        };
        if let Some(ground) = &bound.grass {
            if made.grass.enabled {
                tracing::warn!(
                    "grass_ground '{}' replaces the terrain's grass",
                    ground.entity_name
                );
            }
        }
        let grass_free = bound.grass.is_none();
        let (loaded, grass) = build_terrain(
            renderer,
            device,
            queue,
            world,
            entity.id,
            &entity.name,
            made,
            grass_free,
        );
        if grass_free {
            bound.grass = grass;
        }
        return Some(loaded);
    }
    None
}

/// Build a terrain made of `made` into `renderer` for `entity`: its chunks
/// and material (replacing a terrain loaded before), the style mask's
/// entity, and, when `grass_free` (no `grass_ground` holds the scene's
/// grass), its grass, or none when it has grass no more. Returns the loaded
/// terrain, and its grass when it loaded.
#[allow(clippy::too_many_arguments)]
pub fn build_terrain(
    renderer: &mut SceneRenderer,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    world: &FlintWorld,
    entity: EntityId,
    name: &str,
    made: MadeTerrain,
    grass_free: bool,
) -> (LoadedTerrain, Option<BoundGrass>) {
    let scope = renderer.asset_scope();
    let project = &scope.project;
    let scene_dir = scope.scene_dir.as_path();
    let MadeTerrain {
        config,
        grass,
        heightmap,
        splat,
        recipe,
    } = made;
    let terrain = Terrain::generate(&heightmap, &config);

    // Only the position: a terrain is never rotated or scaled
    let pos = world
        .view(entity, comp::TRANSFORM)
        .vec3("position", [0.0; 3]);
    let transform = Transform {
        position: Vec3::new(pos[0], pos[1], pos[2]),
        ..Default::default()
    };

    let splat_arg = match &splat {
        SplatMap::None => TerrainSplat::None,
        SplatMap::File(name) => TerrainSplat::File(name),
        SplatMap::Baked(maps) => TerrainSplat::Pixels {
            rgba: &maps.splat,
            size: maps.resolution,
        },
    };
    renderer.load_terrain(
        device,
        queue,
        &terrain.chunks,
        &transform,
        config.texture_tile,
        config.metallic,
        config.roughness,
        splat_arg,
        &config.layer_textures,
        scene_dir,
    );
    // A style_mask on the terrain entity masks its chunks (#310)
    renderer.set_terrain_entity(Some(entity));

    let mut bound_grass = None;
    if grass_free {
        if grass.enabled {
            let rgba = match &splat {
                SplatMap::Baked(maps) => {
                    Some((maps.splat.clone(), maps.resolution, maps.resolution))
                }
                SplatMap::File(name) => {
                    let path = project.file_or_beside("terrain splat map", name, scene_dir);
                    match flint_render::open_image(&path) {
                        Ok(img) => {
                            let rgba = img.to_rgba8();
                            let (w, h) = rgba.dimensions();
                            Some((rgba.into_raw(), w, h))
                        }
                        Err(_) => {
                            tracing::warn!("Grass enabled but splat map not found at {path:?}");
                            None
                        }
                    }
                }
                SplatMap::None => {
                    tracing::warn!("Grass enabled but the terrain has no splat map");
                    None
                }
            };
            if let Some((rgba, sw, sh)) = rgba {
                load_terrain_grass(
                    renderer, device, queue, &grass, &heightmap, &config, pos, &rgba, sw, sh,
                );
            }
            // The terrain's grass is in only when its splat map loaded
            if renderer.grass_config().is_some() {
                bound_grass = Some(BoundGrass {
                    config: grass,
                    entity_name: name.to_string(),
                    component: comp::TERRAIN,
                });
            }
        } else if renderer.grass_config().is_some() {
            // Its recipe's grass was turned off since the last build
            renderer.unload_grass();
        }
    }

    let from = recipe
        .as_ref()
        .map(|r| format!(" from {}", r.words()))
        .unwrap_or_default();
    tracing::info!(
        "Loaded terrain '{}'{from}: {}x{} heightmap, {} chunks, {}x{} world units",
        name,
        heightmap.width,
        heightmap.depth,
        terrain.chunks.len(),
        config.width,
        config.depth,
    );
    let loaded = LoadedTerrain {
        entity,
        name: name.to_string(),
        heightmap_size: (heightmap.width, heightmap.depth),
        terrain,
        config,
        offset: pos,
        spec: recipe.map(|r| r.source.path),
    };
    (loaded, bound_grass)
}

/// The terrain's grass, grown over its splat map's weights.
#[allow(clippy::too_many_arguments)]
fn load_terrain_grass(
    renderer: &mut SceneRenderer,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    grass: &GrassConfig,
    heightmap: &Heightmap,
    config: &TerrainConfig,
    pos: [f32; 3],
    splat_rgba: &[u8],
    sw: u32,
    sh: u32,
) {
    renderer.load_grass(
        device,
        queue,
        grass,
        &heightmap.clone_heights(),
        heightmap.width,
        heightmap.depth,
        splat_rgba,
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
