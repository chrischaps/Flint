//! The scene a renderer draws (ADR 0118 decision 5, #739): what a host binds
//! once, when a scene loads, so that no host can skip a step of it.
//!
//! - [`SceneBinding::new`] (or [`SceneBinding::over`] a host's own registry)
//!   makes the [`StyleRegistry`] the scene is drawn with: the built-ins, a
//!   game's styles, and the user styles the scene can see (#742). A host
//!   makes one before it reads the scene's `[post_process]`, against
//!   [`SceneBinding::styles`] (a table naming a style it doesn't hold is
//!   dropped), and `bind_scene` hands it to the renderer.
//! - [`SceneRenderer::bind_scene`] gives the renderer the scene's project and
//!   folder, forgets everything per entity and per scene the last one left
//!   (style history, bone buffers and holds, the `[environment]` levers, the
//!   terrain and its grass), then loads the scene's terrain and grass from
//!   its world. The player, the viewer and `flint render` call it; before
//!   #739 each wired these by hand, and the viewer loaded no terrain at all.
//!
//! What changes every frame is [`FrameInputs`](super::FrameInputs), in
//! `frame.rs`.

use super::SceneRenderer;
use crate::stylize::StyleRegistry;
use flint_core::components as comp;
use flint_core::project::{AssetScope, Project};
use flint_core::{EntityId, Transform, Vec3};
use flint_ecs::FlintWorld;
use flint_schema::{ArchetypeVisual, SchemaRegistry};
use std::collections::HashMap;
use std::path::Path;

/// A scene as a renderer binds it: its project and folder, with the user
/// styles they reach registered. Made once a scene load, before the scene's
/// `[post_process]` is read, and handed to [`SceneRenderer::bind_scene`].
#[derive(Clone, Debug)]
pub struct SceneBinding {
    assets: AssetScope,
    /// The styles the scene is drawn with (#742).
    styles: StyleRegistry,
    style_messages: Vec<String>,
    /// The scene's `[environment]`, whose lighting levers `bind_scene` sets.
    environment: Option<flint_scene::EnvironmentDef>,
    /// Each archetype's placeholder box, from its schema's
    /// `[archetype.<name>.visual]` (#751).
    archetype_visuals: HashMap<String, ArchetypeVisual>,
}

impl SceneBinding {
    /// The scene whose project and folder `assets` names, drawn with the
    /// built-in styles and every `styles/*.shader.toml` the project's
    /// search lists from there (ADR 0112, 0114), the nearest of a name
    /// winning. What went wrong is in
    /// [`style_messages`](Self::style_messages), for the host to say.
    pub fn new(assets: AssetScope) -> Self {
        Self::over(assets, &StyleRegistry::new())
    }

    /// [`new`](Self::new) over `base`, a host's registry (#742): its
    /// built-ins and game styles (#419), with this scene's user styles in
    /// place of any `base` held. A style `base` loaded from the same
    /// unchanged file is shared, so its built pass is kept.
    pub fn over(assets: AssetScope, base: &StyleRegistry) -> Self {
        let (styles, style_messages) = base.for_scene(&assets);
        Self {
            assets,
            styles,
            style_messages,
            environment: None,
            archetype_visuals: HashMap::new(),
        }
    }

    /// The archetypes the scene's entities were made from, in `schemas`:
    /// the box each one's `bounds` draws as when it has nothing else to
    /// draw, its `[archetype.<name>.visual]` (#751). The renderer names no
    /// archetype, so without this every placeholder box is grey.
    pub fn with_archetypes(mut self, schemas: &SchemaRegistry) -> Self {
        self.archetype_visuals = schemas
            .archetypes()
            .into_iter()
            .filter_map(|archetype| Some((archetype.name.clone(), archetype.visual?)))
            .collect();
        self
    }

    /// The scene's `[environment]` table: its `shadows` and
    /// `default_lights` (#627), hemisphere ambient, diffuse wrap,
    /// Oren-Nayar and sheen, set by `bind_scene` over the built-ins. None,
    /// or a key left out, is the built-in. Its skybox is the host's.
    pub fn with_environment(mut self, environment: Option<&flint_scene::EnvironmentDef>) -> Self {
        self.environment = environment.cloned();
        self
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

    /// The same scene over `base` (#742): its user styles loaded again
    /// against `base` (one unchanged since is shared), so a host that binds
    /// a scene again after a hot reload keeps what the reload brought.
    pub fn restyled(mut self, base: &StyleRegistry) -> Self {
        let (styles, messages) = base.for_scene(&self.assets);
        self.styles = styles;
        self.style_messages = messages;
        self
    }

    /// The registry the scene is drawn with: read its `[post_process]`
    /// against this (`post_process_config_from_def`).
    pub fn styles(&self) -> &StyleRegistry {
        &self.styles
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
    /// prepared; a host's own lever overrides (a flag, a panel) go after it.
    /// In order:
    ///
    /// 1. the project and folder fonts, glyph sets, palettes and looks are
    ///    found from ([`set_asset_scope`](Self::set_asset_scope));
    /// 2. no style history (#277), no bone buffers or motion holds (entity
    ///    ids are never reused, so the last scene's are garbage), and the
    ///    built-in ambient and lighting levers, then the scene's
    ///    `[environment]` over them ([`SceneBinding::with_environment`]),
    ///    and the archetypes' placeholder boxes in place of the last
    ///    scene's ([`SceneBinding::with_archetypes`]);
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
        flint_core::scope!("bind_scene");
        self.set_asset_scope(binding.assets.clone());
        self.styles = binding.styles.clone();
        // A user style's component has no schema file for the loader to
        // check it against (it loads at run time), so it is checked here
        // against the schema its descriptor makes (#556): a misspelt field,
        // a wrong type, an unknown choice, a value past its range (#492)
        for warning in crate::stylize::component_schema::user_style_component_warnings(
            world,
            &self.styles,
            &binding.assets.project.allow,
            None,
        ) {
            tracing::warn!("{warning}");
        }
        self.reset_style_history();
        self.clear_entity_bone_buffers();
        self.reset_ambient();
        self.set_scene_shadows(true);
        self.set_default_lights(true);
        self.lights.forget_warnings();
        // A new scene says its own styles' dropped entities (#740)
        crate::stylize::variant_table::forget_dropped();
        if let Some(env) = &binding.environment {
            self.set_environment(env);
        }
        self.archetype_visuals = binding.archetype_visuals.clone();
        self.clear_terrain();
        self.unload_grass();

        let project = &binding.assets.project;
        let scene_dir = binding.assets.scene_dir.as_path();
        let terrain = load_scene_terrain(world, project, scene_dir, device, queue, self);
        let mut grass = terrain.as_ref().and_then(|t| {
            let config = flint_terrain::GrassConfig::from_component(
                &world.view(t.entity, comp::TERRAIN),
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

impl SceneRenderer {
    /// A scene's `[environment]` levers, over the built-ins: no shadows and
    /// no built-in lights where it says so (#627), and each lever it names.
    fn set_environment(&mut self, env: &flint_scene::EnvironmentDef) {
        self.set_scene_shadows(env.casts_shadows());
        self.set_default_lights(env.uses_default_lights());
        if env.ambient_sky.is_some() || env.ambient_ground.is_some() {
            self.set_ambient(
                env.ambient_sky
                    .unwrap_or(crate::LightUniforms::DEFAULT_AMBIENT_SKY),
                env.ambient_ground
                    .unwrap_or(crate::LightUniforms::DEFAULT_AMBIENT_GROUND),
            );
        }
        if let Some(wrap) = env.diffuse_wrap {
            self.set_diffuse_wrap(wrap);
        }
        if let Some(oren) = env.oren_nayar {
            self.set_oren_nayar(oren);
        }
        if let Some(strength) = env.sheen_strength {
            self.set_sheen(env.sheen_color.unwrap_or([1.0; 3]), strength);
        }
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
    flint_core::scope!("load_terrain");

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

        let grass_config = flint_terrain::GrassConfig::from_component(&terrain_comp, false);
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
