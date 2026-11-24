//! Models of entities that arrive after their scene (#1107): a chunk's
//! (`load_chunk`) and a script's (`spawn_entity`, then `set_field(id,
//! "model", "asset", ..)`). A scene's entry loads its own models once
//! (`stage_scene`); before this, nothing loaded a model that came later, and
//! such an entity drew nothing.
//!
//! Entity ids are never given out twice (#688), so what arrived since the
//! last look is the ids from where it stopped to the world's
//! [`next_id`](flint_ecs::FlintWorld::next_id): a frame that spawned nothing
//! costs one comparison, and no walk of the world. A cached mesh is drawn as
//! it is, unless its file expanded into nodes: then the new entity gets
//! children of its own, from the import kept the first time a late load
//! read the file (`model_loader::Expansions`), so a script spawning one a
//! frame reads it once. A model that wasn't found is not looked for again
//! this scene (it would warn a frame).

use super::scene_loading::{
    build_model_load_config_of, register_node_animation_data, register_skeletal_data,
};
use super::PlayerApp;
use flint_core::components as comp;
use flint_core::EntityId;
use flint_ecs::FlintWorld;
use flint_render::model_loader::{self, Expansions, ModelLoadResult};
use flint_render::SceneRenderer;
use std::collections::HashSet;

/// What the player remembers between late loads, for one scene.
#[derive(Default)]
pub(super) struct LateModels {
    /// The first id not yet looked at.
    from: u64,
    /// Assets that expanded into nodes, with the imports they expanded
    /// from once a late load has read one.
    expansions: Expansions,
    /// Assets looked for and not found (or not loaded) this scene.
    missing: HashSet<String>,
}

impl LateModels {
    /// A scene is coming in: everything spawned so far is the scene's, and
    /// the memory of the last scene's files goes. Its entry then says what
    /// it expanded ([`remember_expanded`](Self::remember_expanded)) and
    /// moves past what its loading spawned ([`skip_to`](Self::skip_to)).
    pub(super) fn enter_scene(&mut self, world: &FlintWorld) {
        self.expansions.clear();
        self.missing.clear();
        self.from = world.next_id().raw();
    }

    /// The live entities spawned since the last look, and the look moved
    /// past them.
    fn take_new(&mut self, world: &FlintWorld) -> Vec<EntityId> {
        let to = world.next_id().raw();
        let new = (self.from..to)
            .map(EntityId::from_raw)
            .filter(|&id| world.contains(id))
            .collect();
        self.from = to;
        new
    }

    /// Past what a load itself spawned (an expanded model's nodes, a
    /// scene's procgen).
    pub(super) fn skip_to(&mut self, world: &FlintWorld) {
        self.from = self.from.max(world.next_id().raw());
    }

    pub(super) fn remember_expanded(&mut self, loaded: &ModelLoadResult) {
        self.expansions.remember(loaded);
    }

    /// Of `ids`, those with an asset to load: a model not known missing, or
    /// a material's or sprite's texture.
    fn wanting_assets(&self, world: &FlintWorld, ids: &[EntityId]) -> Vec<EntityId> {
        ids.iter()
            .copied()
            .filter(|&id| {
                let model = model_loader::model_assets_of(world, &[id]);
                let model = model.iter().any(|(_, a)| !self.missing.contains(a));
                let texture = [comp::MATERIAL, comp::SPRITE]
                    .iter()
                    .any(|table| world.view(id, table).opt_str("texture").is_some());
                model || texture
            })
            .collect()
    }
}

impl PlayerApp {
    /// Load the models of the entities spawned since the last call: once a
    /// frame after the scripts, and at the end of `load_chunk`, so a chunk's
    /// models load inside its chunk note and load report. A host with no
    /// renderer moves past them.
    pub(super) fn load_late_models(&mut self) {
        let new = self.late_models.take_new(&self.core.world);
        if new.is_empty() {
            return;
        }
        let ids = self.late_models.wanting_assets(&self.core.world, &new);
        if ids.is_empty() {
            return;
        }
        let context = self.render_context.take();
        let mut renderer = self.scene_renderer.take();
        let gpu = context.as_ref().map(|c| (&c.device, &c.queue));
        let gpu = gpu.or(self.windowless_gpu().map(|g| (&g.device, &g.queue)));
        if let (Some(renderer), Some((device, queue))) = (renderer.as_mut(), gpu) {
            self.load_models_of(&ids, renderer, device, queue);
        }
        self.render_context = context;
        self.scene_renderer = renderer;
        self.late_models.skip_to(&self.core.world);
    }

    /// [`load_late_models`](Self::load_late_models)' load of `ids`, as
    /// `stage_scene` loads a scene's: the catalog, then procgen, then the
    /// file (#32), then their skeletons and node clips.
    fn load_models_of(
        &mut self,
        ids: &[EntityId],
        renderer: &mut SceneRenderer,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
    ) {
        let config = build_model_load_config_of(
            &self.core.project,
            &self.core.scene_path,
            &self.core.world,
            ids,
            self.assets.of(&self.core.project),
        );

        // Procgen before the file, for the names it has a spec of
        #[cfg(feature = "procgen")]
        {
            use super::procgen_assets::{collect_requests_of, resolve, GpuSink};
            let mut sink = GpuSink {
                renderer,
                device,
                queue,
                config: &config,
            };
            let requests = collect_requests_of(
                &self.core.world,
                ids,
                &config,
                &self.core.procgen_resolver,
                &sink,
            );
            if !requests.is_empty() {
                // Each mesh's triangles kept for the colliders naming it
                // (#1222)
                let mut recording = super::collider_meshes::Recording {
                    sink: &mut sink,
                    sources: &mut self.core.collider_sources,
                };
                resolve(
                    &requests,
                    &mut self.core.procgen_resolver,
                    &mut recording,
                    self.launch.procgen_mode,
                );
            }
        }

        let wanted = model_loader::model_assets_of(&self.core.world, ids);
        let mut loaded = model_loader::load_models_of(
            &mut self.core.world,
            ids,
            renderer,
            device,
            queue,
            &config,
            &mut self.late_models.expansions,
        );
        for (_, asset) in wanted {
            if !renderer.mesh_cache().contains(&asset) {
                self.late_models.missing.insert(asset);
            }
        }

        // A skinned mesh already cached gives its new entity no import to
        // build a skeleton from: it takes a copy of one an entity drawing
        // the same asset has, in that entity's pose until it animates
        loaded.models.retain(|model| {
            if !model.is_skinned || model.import_result.is_some() {
                return true;
            }
            let donor = self
                .skeletal_entity_assets
                .iter()
                .filter(|(_, asset)| **asset == model.asset_name)
                .find_map(|(id, _)| self.core.animation.skeleton(id).cloned());
            match donor {
                Some(skeleton) => {
                    self.core.animation.add_skeleton(model.entity_id, skeleton);
                    false
                }
                None => true,
            }
        });
        register_skeletal_data(&loaded, &mut self.core.animation);
        register_node_animation_data(&loaded, &mut self.core.animation);
        self.skeletal_entity_assets.extend(loaded.skinned_entities);
    }
}
