//! The terrain's settings a script changes while the game runs (#1698).
//!
//! The bind builds the terrain's material and grass once, from the
//! component as the scene wrote it. Until #1698 nothing read them again, so
//! `set_field(island, "terrain", "grass.wind_strength", 0.9)` was stored
//! and drawn nowhere. Now the subsystem reads them once a frame, at
//! [`Stage::Sync`](flint_package::Stage::Sync) (after every script of the
//! frame, before it is drawn), and hands the renderer what changed, as the
//! lights are read every frame:
//!
//! | field | reaches the renderer through |
//! |---|---|
//! | every `grass.*` key | `set_grass_config` (a denser or longer field gets its buffer remade before the next dispatch) |
//! | `grass.enabled` | `false` stops the blades and `true` brings them back; a field the bind never grew is grown then ([`grow_terrain_grass`]) |
//! | `texture_tile`, `metallic`, `roughness` | `set_terrain_material`: one 12-byte write |
//!
//! Comparing costs about thirty table lookups a frame, and nothing is
//! written unless a value moved. A `grass_ground` holds the scene's grass
//! when there is one (ADR 0089), so the terrain's `grass.*` then changes
//! nothing, and says so once.

use crate::scene::grow_terrain_grass;
use crate::{GrassConfig, TerrainConfig};
use flint_core::components as comp;
use flint_core::EntityId;
use flint_ecs::FlintWorld;
use flint_geometry::ground::SceneGround;
use flint_render::SceneRenderer;

/// What scripts can change on a terrain while the game runs: the
/// `terrain` module's `terrain_live_fields()`, in the order a script
/// meets them in a scene file.
pub const LIVE_FIELDS: &[&str] = &[
    "texture_tile",
    "metallic",
    "roughness",
    "grass.enabled",
    "grass.density",
    "grass.max_distance",
    "grass.fade_start",
    "grass.blade_width",
    "grass.blade_height",
    "grass.height_variation",
    "grass.color_base",
    "grass.color_tip",
    "grass.color_dry",
    "grass.dry_amount",
    "grass.wind_direction",
    "grass.wind_speed",
    "grass.wind_strength",
    "grass.bend_radius",
    "grass.bend_strength",
    "grass.density_layer",
    "grass.density_threshold",
    "grass.wooze",
    "grass.wooze_amplitude",
    "grass.wooze_wavelength",
];

/// The scene's terrain as the renderer last drew it.
pub(super) struct LiveTerrain {
    entity: EntityId,
    grass: GrassConfig,
    material: [f32; 3],
    /// A `grass_ground` holds the scene's one grass field (ADR 0089).
    grass_yields: bool,
    /// Said once that a change to the terrain's grass reaches nothing.
    said_yields: bool,
}

impl LiveTerrain {
    /// The scene's first terrain, the one `bind_terrain` builds (the first
    /// entity with a `terrain` that names a heightmap), as the scene wrote
    /// it: what the bind drew. `None` in a scene without one.
    pub(super) fn of(world: &FlintWorld) -> Option<Self> {
        let entity = world.all_entities().into_iter().find_map(|e| {
            let view = world.view(e.id, comp::TERRAIN);
            (view.exists() && !view.str("heightmap", "").is_empty()).then_some(e.id)
        })?;
        let view = world.view(entity, comp::TERRAIN);
        Some(Self {
            entity,
            grass: GrassConfig::from_component(&view, false),
            material: TerrainConfig::material_of(&view),
            grass_yields: !world.entities_with_component(comp::GRASS_GROUND).is_empty(),
            said_yields: false,
        })
    }

    /// Hand the renderer whatever a script changed since the last frame. A
    /// host lending no renderer keeps the change for the frame that does.
    pub(super) fn sync(
        &mut self,
        world: &FlintWorld,
        renderer: Option<&mut SceneRenderer>,
        gpu: Option<(&wgpu::Device, &wgpu::Queue)>,
        ground: Option<&SceneGround>,
    ) {
        let (Some(renderer), Some((device, queue))) = (renderer, gpu) else {
            return;
        };
        let view = world.view(self.entity, comp::TERRAIN);
        if !view.exists() {
            return;
        }

        let material = TerrainConfig::material_of(&view);
        if material != self.material {
            let [tile, metallic, roughness] = material;
            renderer.set_terrain_material(queue, tile, metallic, roughness);
            self.material = material;
        }

        let grass = GrassConfig::from_component(&view, false);
        if grass == self.grass {
            return;
        }
        if self.grass_yields {
            if !self.said_yields {
                tracing::warn!(
                    "a grass_ground holds the scene's grass, so the terrain's grass.* changes nothing"
                );
                self.said_yields = true;
            }
        } else if renderer.grass_config().is_some() {
            renderer.set_grass_config(grass.clone());
        } else if grass.enabled {
            let heightmap = ground
                .filter(|g| g.entity == self.entity)
                .map(|g| &g.heightmap);
            grow_terrain_grass(
                renderer,
                device,
                queue,
                world,
                self.entity,
                &grass,
                heightmap,
            );
        }
        self.grass = grass;
    }
}
