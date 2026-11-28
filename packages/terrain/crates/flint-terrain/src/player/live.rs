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
use crate::source::SpecSource;
use crate::{GrassConfig, TerrainConfig};
use flint_core::components as comp;
use flint_core::EntityId;
use flint_ecs::FlintWorld;
use flint_geometry::ground::SceneGround;
use flint_render::SceneRenderer;
use std::path::Path;

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
    /// What a key the component leaves out reads as: its recipe's settings
    /// when it names one (#1697), else the defaults.
    base_grass: GrassConfig,
    base_material: [f32; 3],
    /// A `grass_ground` holds the scene's one grass field (ADR 0089).
    grass_yields: bool,
    /// Said once that a change to the terrain's grass reaches nothing.
    said_yields: bool,
}

impl LiveTerrain {
    /// The scene's first terrain, the one `bind_terrain` builds (the first
    /// entity with a `terrain` that names a heightmap or a recipe), as the
    /// bind drew it: the component over its recipe's settings when it names
    /// one (#1697; the spec read again, not baked). `None` in a scene
    /// without one.
    pub(super) fn of(
        world: &FlintWorld,
        project: &flint_core::project::Project,
        scene_dir: &Path,
    ) -> Option<Self> {
        let entity = world.all_entities().into_iter().find_map(|e| {
            let view = world.view(e.id, comp::TERRAIN);
            let names = |key| !view.str(key, "").is_empty();
            (view.exists() && (names("heightmap") || names("spec"))).then_some(e.id)
        })?;
        let recipe =
            crate::source::spec_path(&world.view(entity, comp::TERRAIN), project, scene_dir)
                .and_then(Result::ok)
                .and_then(|path| SpecSource::read(&path).ok());
        let recipe = recipe.as_ref();
        let mut live = Self {
            entity,
            grass: GrassConfig::default(),
            material: [0.0; 3],
            base_grass: GrassConfig::default(),
            base_material: TerrainConfig::default().material(),
            grass_yields: !world.entities_with_component(comp::GRASS_GROUND).is_empty(),
            said_yields: false,
        };
        live.rebase(world, recipe, scene_dir);
        Some(live)
    }

    /// The terrain was built again from `recipe` (its rebake, #1697): what a
    /// key the component leaves out reads as is the new recipe's, and what
    /// the renderer draws now is the component over it.
    pub(super) fn rebase(
        &mut self,
        world: &FlintWorld,
        recipe: Option<&SpecSource>,
        scene_dir: &Path,
    ) {
        let (base_grass, base_material) = match recipe {
            Some(source) => (
                source.spec.grass.clone().unwrap_or_default(),
                TerrainConfig::from_spec(source, scene_dir).material(),
            ),
            None => (GrassConfig::default(), TerrainConfig::default().material()),
        };
        let view = world.view(self.entity, comp::TERRAIN);
        self.grass = GrassConfig::over(&view, &base_grass);
        self.material = TerrainConfig::material_over(&view, base_material);
        self.base_grass = base_grass;
        self.base_material = base_material;
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

        let material = TerrainConfig::material_over(&view, self.base_material);
        if material != self.material {
            let [tile, metallic, roughness] = material;
            renderer.set_terrain_material(queue, tile, metallic, roughness);
            self.material = material;
        }

        let grass = GrassConfig::over(&view, &self.base_grass);
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A terrain made from its recipe (#1697) is found, and the frame's
    /// reread starts from what the bind drew: the recipe's grass and
    /// material under the component's own keys, not the defaults (which the
    /// first frame's sync would otherwise hand the renderer). A rebase onto
    /// a saved recipe takes its new settings.
    #[test]
    fn a_recipe_terrain_is_reread_over_its_recipe_and_rebased_on_a_new_one() {
        let dir = std::env::temp_dir().join(format!("flint-live-recipe-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("land")).unwrap();
        let recipe = |density: f32, tile: f32| {
            format!(
                "[meta]\nname = \"hill\"\n[textures]\ntexture_tile = {tile}\nroughness = 0.6\n\
                 [grass]\nenabled = true\ndensity = {density}\nblade_height = 0.7\n"
            )
        };
        std::fs::write(dir.join("land/hill.terrain.toml"), recipe(30.0, 7.0)).unwrap();
        let project = flint_core::project::Project::for_scene(&dir.join("hill.scene.toml"));

        let mut world = FlintWorld::new();
        let id = world.spawn("hill").unwrap();
        let terrain: toml::Value = toml::from_str(
            "spec = \"land/hill.terrain.toml\"\n\"grass.blade_height\" = 0.2\nmetallic = 0.5",
        )
        .unwrap();
        world.set_component(id, comp::TERRAIN, terrain).unwrap();

        let mut live = LiveTerrain::of(&world, &project, &dir).expect("found by its spec");
        assert_eq!(live.entity, id);
        assert!(live.grass.enabled);
        assert_eq!((live.grass.density, live.grass.blade_height), (30.0, 0.2));
        assert_eq!(live.material, [7.0, 0.5, 0.6]);

        // Saved with a sparser field and a finer tile: the new base, the
        // component's keys still over it
        std::fs::write(dir.join("land/hill.terrain.toml"), recipe(5.0, 2.0)).unwrap();
        let saved = SpecSource::read(&dir.join("land/hill.terrain.toml")).unwrap();
        live.rebase(&world, Some(&saved), &dir);
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!((live.grass.density, live.grass.blade_height), (5.0, 0.2));
        assert_eq!(live.material, [2.0, 0.5, 0.6]);
    }
}
