//! The scene a renderer draws (ADR 0118 decision 5, #739): what a host binds
//! once, when a scene loads, so that no host can skip a step of it.
//!
//! - [`SceneBinding::new`] registers the user styles the scene can see, so a
//!   host makes one before it reads the scene's `[post_process]` (a table
//!   naming a style the registry doesn't know is dropped).
//! - [`SceneRenderer::bind_scene`] gives the renderer the scene's project and
//!   folder, forgets everything per entity and per scene the last one left
//!   (style history, bone buffers and holds, the `[environment]` levers, the
//!   terrain and its grass), then loads the scene's terrain and grass from
//!   its world. The player and `flint render` call it; before #739 each
//!   wired these by hand, and the viewer, which doesn't call it yet, loads
//!   no terrain at all.
//!
//! What changes every frame is [`FrameInputs`](super::FrameInputs), in
//! `frame.rs`.

use super::SceneRenderer;
use flint_core::components as comp;
use flint_core::project::{AssetScope, Project};
use flint_core::toml_util::{toml_f32, toml_vec3};
use flint_core::{EntityId, Transform, Vec3};
use flint_ecs::FlintWorld;
use std::path::Path;

/// A scene as a renderer binds it: its project and folder, with the user
/// styles they reach registered. Made once a scene load, before the scene's
/// `[post_process]` is read, and handed to [`SceneRenderer::bind_scene`].
#[derive(Clone, Debug)]
pub struct SceneBinding {
    assets: AssetScope,
    style_messages: Vec<String>,
}

impl SceneBinding {
    /// The scene whose project and folder `assets` names. Registers every
    /// `styles/*.shader.toml` the project's search lists from there (ADR
    /// 0112, 0114), the nearest of a name winning; styles registered before
    /// are kept, so a scene change only adds (#275). What went wrong is in
    /// [`style_messages`](Self::style_messages), for the host to say.
    pub fn new(assets: AssetScope) -> Self {
        let style_messages = crate::stylize::custom::load_for(&assets);
        Self {
            assets,
            style_messages,
        }
    }

    /// [`new`](Self::new) for the scene file at `scene_path`, its project
    /// found from there.
    pub fn for_scene(scene_path: &Path) -> Self {
        Self::new(AssetScope::for_scene(scene_path))
    }

    /// The scene's project and folder.
    pub fn assets(&self) -> &AssetScope {
        &self.assets
    }

    /// What registering the scene's user styles had to say: one line each.
    pub fn style_messages(&self) -> &[String] {
        &self.style_messages
    }
}

/// What [`SceneRenderer::bind_scene`] loaded from the scene's world, for the
/// host's own share of it (the player's colliders, the grass debug panel,
/// `flint render`'s lines).
#[derive(Default)]
pub struct BoundScene {
    /// The scene's terrain, when it has one that loaded.
    pub terrain: Option<LoadedTerrain>,
    /// The scene's grass: a `grass_ground`'s, or else the terrain's.
    pub grass: Option<BoundGrass>,
}

/// A terrain [`SceneRenderer::bind_scene`] loaded: its entity, what it was
/// generated from, and the generated chunks (whose collider a host with
/// physics registers).
pub struct LoadedTerrain {
    pub entity: EntityId,
    pub name: String,
    pub terrain: flint_terrain::Terrain,
    pub config: flint_terrain::TerrainConfig,
    /// The heightmap's size in samples, width by depth.
    pub heightmap_size: (u32, u32),
    /// Where the entity's transform puts it.
    pub offset: [f32; 3],
}

/// The grass [`SceneRenderer::bind_scene`] loaded, and where its settings
/// live.
pub struct BoundGrass {
    pub config: flint_terrain::GrassConfig,
    /// The entity whose component holds the `grass.*` fields.
    pub entity_name: String,
    /// That component: `terrain` or `grass_ground`.
    pub component: &'static str,
}

impl SceneRenderer {
    /// Bind the scene `binding` names, whose world is `world`, in place of
    /// whatever the last one left (ADR 0118 decision 5, #739). Call it once a
    /// scene load, after the world is loaded and before its first frame is
    /// prepared, and set the scene's `[environment]` levers after it (it puts
    /// the built-in ones back). In order:
    ///
    /// 1. the project and folder fonts, glyph sets, palettes and looks are
    ///    found from ([`set_asset_scope`](Self::set_asset_scope));
    /// 2. no style history (#277), no bone buffers or motion holds (entity
    ///    ids are never reused, so the last scene's are garbage), and the
    ///    built-in ambient and lighting levers;
    /// 3. the last scene's terrain and grass unloaded, then this scene's
    ///    terrain (its chunks masked when the entity is styled, #310) and
    ///    grass loaded: a `grass_ground`'s, which replaces a terrain's (ADR
    ///    0089).
    pub fn bind_scene(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        world: &FlintWorld,
        binding: &SceneBinding,
    ) -> BoundScene {
        self.set_asset_scope(binding.assets.clone());
        self.reset_style_history();
        self.clear_entity_bone_buffers();
        self.reset_ambient();
        self.clear_terrain();
        self.unload_grass();

        let project = &binding.assets.project;
        let scene_dir = binding.assets.scene_dir.as_path();
        let terrain = load_scene_terrain(world, project, scene_dir, device, queue, self);
        let mut grass = terrain.as_ref().and_then(|t| {
            let config = flint_terrain::GrassConfig::from_component(
                world.get_component(t.entity, comp::TERRAIN)?,
                false,
            );
            // The terrain's grass is in only when its splat map loaded
            self.grass_config()?;
            Some(BoundGrass {
                config,
                entity_name: t.name.clone(),
                component: comp::TERRAIN,
            })
        });
        // One grass field a scene: a grass_ground replaces a terrain's grass
        if let Some(ground) =
            super::load_grass_ground_in(world, project, scene_dir, device, queue, self)
        {
            if grass.is_some() {
                tracing::warn!(
                    "grass_ground '{}' replaces the terrain's grass",
                    ground.entity_name
                );
            }
            grass = Some(BoundGrass {
                config: ground.config,
                entity_name: ground.entity_name,
                component: comp::GRASS_GROUND,
            });
        }
        BoundScene { terrain, grass }
    }
}

/// Load the first `terrain` entity of `world` into `renderer` (its chunks,
/// its material, and its grass when `grass.enabled` and the splat map
/// loads), and mark it as the entity a `style_mask` on it masks (#310).
/// Files are found by the project's one search (`AssetKind::Files`) from
/// `scene_dir`. One loader for every host since #739; the player and
/// `flint render` each had a copy, and the viewer had none.
fn load_scene_terrain(
    world: &FlintWorld,
    project: &Project,
    scene_dir: &Path,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    renderer: &mut SceneRenderer,
) -> Option<LoadedTerrain> {
    use flint_terrain::{Heightmap, Terrain, TerrainConfig};

    for entity in world.all_entities() {
        let Some(terrain_comp) = world.get_component(entity.id, comp::TERRAIN) else {
            continue;
        };
        let heightmap_rel = match terrain_comp.get("heightmap").and_then(|v| v.as_str()) {
            Some(s) if !s.is_empty() => s.to_string(),
            _ => {
                tracing::warn!("Terrain entity '{}': missing heightmap path", entity.name);
                continue;
            }
        };
        let hm_path = project.file_or_beside("terrain heightmap", &heightmap_rel, scene_dir);
        let heightmap = match Heightmap::from_png(&hm_path) {
            Ok(hm) => hm,
            Err(e) => {
                tracing::warn!("Failed to load heightmap: {}", e);
                continue;
            }
        };

        let get_f32 =
            |key: &str, default: f32| terrain_comp.get(key).and_then(toml_f32).unwrap_or(default);
        let get_i32 = |key: &str, default: i32| {
            terrain_comp
                .get(key)
                .and_then(|v| v.as_integer())
                .map(|i| i as i32)
                .unwrap_or(default)
        };
        let get_str = |key: &str| {
            terrain_comp
                .get(key)
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string()
        };
        let config = TerrainConfig {
            heightmap_path: heightmap_rel,
            width: get_f32("width", 256.0),
            depth: get_f32("depth", 256.0),
            height_scale: get_f32("height_scale", 50.0),
            chunk_resolution: get_i32("chunk_resolution", 64) as u32,
            texture_tile: get_f32("texture_tile", 16.0),
            splat_map_path: get_str("splat_map"),
            layer_textures: [
                get_str("layer0_texture"),
                get_str("layer1_texture"),
                get_str("layer2_texture"),
                get_str("layer3_texture"),
            ],
            metallic: get_f32("metallic", 0.0),
            roughness: get_f32("roughness", 0.85),
            grass: None,
        };
        let terrain = Terrain::generate(&heightmap, &config);

        // Only the position: a terrain is never rotated or scaled
        let pos = world
            .get_component(entity.id, comp::TRANSFORM)
            .and_then(|t| t.get("position").and_then(toml_vec3))
            .unwrap_or([0.0; 3]);
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

        let grass_config = flint_terrain::GrassConfig::from_component(terrain_comp, false);
        if grass_config.enabled {
            let splat_path =
                project.file_or_beside("terrain splat map", &config.splat_map_path, scene_dir);
            match crate::open_image(&splat_path) {
                Ok(splat_img) => {
                    let splat_rgba = splat_img.to_rgba8();
                    let (sw, sh) = splat_rgba.dimensions();
                    renderer.load_grass(
                        device,
                        queue,
                        &grass_config,
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
                        grass_config.density,
                        grass_config.max_distance
                    );
                }
                Err(_) => {
                    tracing::warn!("Grass enabled but splat map not found at {:?}", splat_path)
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
