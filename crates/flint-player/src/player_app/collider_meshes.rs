//! What the player gives physics for its bodies (#729, #1222): the
//! project's collision layers, and the triangles of every `mesh` and
//! `convex` collider, from wherever the model it names came from.
//!
//! **A model's file.** The requests are read before the renderer loads the
//! scene's models: a model of several nodes is expanded into child entities
//! and its parent loses its `model`, so afterwards the parent no longer says
//! which model it drew. The triangles come from the model file by the
//! project's one search (the model loader's own `resolve_model_path`),
//! through `flint_import::import_gltf_geometry`, which skips the textures; a
//! model used by several colliders is read once a scene.
//!
//! **A procgen spec** (catalog > procgen > file, #32). The resolver's
//! uploads go through [`Recording`], which keeps each generated mesh's
//! positions and indices in [`ColliderSources`] by name before the mesh is
//! drawn; a request names one of those ahead of a file. One the queue
//! finishes later (`ProcGenMode::Deferred`) reaches physics on the next
//! frame, and its body's collider is made again.
//!
//! **A runtime mesh** (`model.runtime = true`, `ExtensionGpu::upload_mesh`).
//! The renderer already keeps the triangles of a mesh uploaded as a ray
//! target (`MeshOptions::ray_target`, ADR 0105); those are its collider's,
//! so what a ray meets with `raycast_meshes` and what `raycast()` and a
//! body meet are one set. They come after the scene's bodies (a game
//! uploads in `pre_update`), so physics makes the collider again. A mesh
//! uploaded without `ray_target` keeps no triangles, and that is warned
//! about once on the entity.
//!
//! Each frame [`PlayerApp`]'s core asks [`ColliderSources::refresh`] before
//! the physics steps. It walks the colliders only when something could have
//! changed: a procgen mesh made, a runtime mesh uploaded or removed (the
//! renderer's `runtime_mesh_generation`), or an entity spawned.

use super::scene_loading::build_model_load_config;
use super::PlayerApp;
use flint_core::components as comp;
use flint_core::EntityId;
use flint_ecs::FlintWorld;
use flint_geometry::ray_mesh::RayMesh;
use flint_physics::{ColliderMesh, CollisionLayers, PhysicsSystem};
use flint_render::model_loader::{resolve_model_path, ModelLoadConfig};
use flint_render::SceneRenderer;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

/// Every `mesh` or `convex` collider and the model it names, read now.
pub(super) fn requests(world: &FlintWorld) -> Vec<(EntityId, String)> {
    PhysicsSystem::collider_mesh_requests(world)
}

/// The triangles of meshes the player made or was handed rather than read
/// from a file, for the colliders that name them (#1222). Kept for the run,
/// as the renderer keeps the meshes: a later scene that names a procgen
/// model already drawn generates nothing, and finds its triangles here.
#[derive(Default)]
pub(crate) struct ColliderSources {
    /// Each procgen mesh's triangles, by its spec's name
    made: HashMap<String, Arc<ColliderMesh>>,
    /// Bumped whenever `made` changes
    made_generation: u64,
    /// Each runtime mesh's triangles, from the ray target they were taken
    /// from, so many entities drawing one mesh share one set
    runtime: HashMap<String, (Arc<RayMesh>, Arc<ColliderMesh>)>,
    /// What the colliders were last walked at: `made_generation`, the
    /// renderer's runtime mesh generation, the world's next id. `None`
    /// walks at the next refresh (a scene was just entered).
    walked_at: Option<(u64, u64, u64)>,
    /// Entities told their runtime mesh keeps no triangles
    warned: HashSet<EntityId>,
}

impl ColliderSources {
    /// Keep a procgen mesh's triangles under its name.
    #[cfg(feature = "procgen")]
    pub(crate) fn made_mesh(&mut self, name: &str, mesh: &flint_procgen::MeshData) {
        let positions = mesh.vertices.iter().map(|v| v.position).collect();
        let triangles = mesh.indices.as_chunks::<3>().0.to_vec();
        self.made.insert(
            name.to_string(),
            Arc::new(ColliderMesh {
                positions,
                triangles,
            }),
        );
        self.made_generation += 1;
    }

    /// Forget a procgen mesh: a placeholder or a file stands under the name
    /// now.
    #[cfg_attr(not(feature = "procgen"), allow(dead_code))]
    pub(crate) fn forget(&mut self, name: &str) {
        if self.made.remove(name).is_some() {
            self.made_generation += 1;
        }
    }

    /// The triangles a procgen spec made under `name`, if it made a mesh.
    pub(crate) fn made(&self, name: &str) -> Option<&Arc<ColliderMesh>> {
        self.made.get(name)
    }

    /// Walk every collider again at the next refresh, whatever changed.
    pub(crate) fn walk_again(&mut self) {
        self.walked_at = None;
        self.warned.clear();
    }

    /// Hand physics the triangles that arrived since the last call: a
    /// procgen mesh finished, a runtime mesh uploaded or replaced, or an
    /// entity spawned that names one. One comparison when none of those
    /// moved.
    pub(crate) fn refresh(
        &mut self,
        world: &FlintWorld,
        physics: &mut PhysicsSystem,
        renderer: Option<&SceneRenderer>,
    ) {
        let now = (
            self.made_generation,
            renderer.map_or(0, SceneRenderer::runtime_mesh_generation),
            world.next_id().raw(),
        );
        let before = self.walked_at.replace(now);
        if before == Some(now) {
            return;
        }
        flint_core::scope!("collider_meshes::refresh");
        match before {
            // Only spawns since: ids are never reused, so the new ones are
            // the ids from the last walk's next id on (a game spawning a
            // bullet a frame walks one id, not every collider)
            Some((made, meshes, from)) if made == now.0 && meshes == now.1 => {
                for raw in from..now.2 {
                    self.offer(EntityId::from_raw(raw), world, physics, renderer);
                }
            }
            _ => {
                for &entity in world.entities_with_component(comp::COLLIDER) {
                    self.offer(entity, world, physics, renderer);
                }
                // Runtime meshes no longer ray targets aren't kept
                if let Some(renderer) = renderer {
                    self.runtime.retain(|name, (ray, _)| {
                        renderer.ray_mesh(name).is_some_and(|r| Arc::ptr_eq(r, ray))
                    });
                }
            }
        }
    }

    /// Hand physics an entity's procgen or runtime triangles, when its
    /// `mesh` or `convex` collider names a model that has some it doesn't
    /// hold yet.
    fn offer(
        &mut self,
        entity: EntityId,
        world: &FlintWorld,
        physics: &mut PhysicsSystem,
        renderer: Option<&SceneRenderer>,
    ) {
        let col = world.view(entity, comp::COLLIDER);
        if !matches!(col.opt_str("shape"), Some("mesh" | "convex")) {
            return;
        }
        let own = col.opt_str("model").filter(|m| !m.is_empty());
        let model = world.view(entity, comp::MODEL);
        let runtime = own.is_none() && model.bool("runtime", false);
        let Some(name) = own
            .or_else(|| model.opt_str("asset"))
            .filter(|m| !m.is_empty())
        else {
            return;
        };
        let mesh = if runtime {
            self.runtime_mesh(entity, name, world, renderer)
        } else {
            self.made.get(name).cloned()
        };
        let Some(mesh) = mesh else {
            return;
        };
        if physics
            .collider_mesh_of(entity)
            .is_some_and(|held| Arc::ptr_eq(held, &mesh))
        {
            return;
        }
        tracing::debug!(target: "load", kind = "collider",
            "Collider mesh: {} '{name}' for {} ({} triangles)",
            if runtime { "runtime mesh" } else { "procgen" },
            world.get_name(entity).unwrap_or("?"),
            mesh.triangles.len()
        );
        physics.set_collider_mesh(entity, mesh);
    }

    /// The triangles of the runtime mesh `name`, from its ray target. `None`
    /// before it is uploaded; warned about once when it was uploaded without
    /// `ray_target`, which keeps none.
    fn runtime_mesh(
        &mut self,
        entity: EntityId,
        name: &str,
        world: &FlintWorld,
        renderer: Option<&SceneRenderer>,
    ) -> Option<Arc<ColliderMesh>> {
        let renderer = renderer?;
        let Some(ray) = renderer.ray_mesh(name) else {
            if renderer.mesh_cache().contains(name) && self.warned.insert(entity) {
                tracing::warn!(
                    "collider: {}'s `{}` collider has no triangles: its runtime mesh '{name}' \
                     was uploaded without `ray_target`, so it stays a box of `size`; upload it \
                     with `upload_mesh_with(.., MeshOptions {{ ray_target: true }})`",
                    world.get_name(entity).unwrap_or("?"),
                    world
                        .view(entity, comp::COLLIDER)
                        .opt_str("shape")
                        .unwrap_or("mesh"),
                );
            }
            return None;
        };
        if let Some((kept, mesh)) = self.runtime.get(name) {
            if Arc::ptr_eq(kept, ray) {
                return Some(mesh.clone());
            }
        }
        let mesh = Arc::new(ColliderMesh {
            positions: ray.positions().to_vec(),
            triangles: ray.triangles(),
        });
        self.runtime
            .insert(name.to_string(), (ray.clone(), mesh.clone()));
        self.warned.remove(&entity);
        Some(mesh)
    }
}

/// An [`AssetSink`](super::procgen_assets::AssetSink) that keeps the
/// triangles of each mesh it passes on in [`ColliderSources`], and forgets
/// a name a placeholder or a file takes.
#[cfg(feature = "procgen")]
pub(crate) struct Recording<'s> {
    pub sink: &'s mut dyn super::procgen_assets::AssetSink,
    pub sources: &'s mut ColliderSources,
}

#[cfg(feature = "procgen")]
impl super::procgen_assets::AssetSink for Recording<'_> {
    fn has_mesh(&self, name: &str) -> bool {
        self.sink.has_mesh(name)
    }
    fn has_file(&self, name: &str, kind: flint_procgen::AssetKind) -> bool {
        self.sink.has_file(name, kind)
    }
    fn missing_file(&self, name: &str, kind: flint_procgen::AssetKind) -> Option<String> {
        self.sink.missing_file(name, kind)
    }
    fn upload_mesh(&mut self, name: &str, mesh: &flint_procgen::MeshData) {
        self.sources.made_mesh(name, mesh);
        self.sink.upload_mesh(name, mesh);
    }
    fn upload_living(&mut self, name: &str, rig: &flint_procgen::LivingRig) {
        self.sources.forget(name);
        self.sink.upload_living(name, rig);
    }
    fn upload_bones(&mut self, entity: EntityId, name: &str, palette: &[[[f32; 4]; 4]]) {
        self.sink.upload_bones(entity, name, palette);
    }
    fn upload_image(&mut self, name: &str, image: &flint_procgen::ImageData) {
        self.sink.upload_image(name, image);
    }
    fn upload_placeholder(&mut self, name: &str, kind: flint_procgen::AssetKind) {
        if kind == flint_procgen::AssetKind::Model {
            self.sources.forget(name);
        }
        self.sink.upload_placeholder(name, kind);
    }
    fn load_file(&mut self, name: &str, kind: flint_procgen::AssetKind) -> bool {
        if kind == flint_procgen::AssetKind::Model {
            self.sources.forget(name);
        }
        self.sink.load_file(name, kind)
    }
}

/// The triangles of each requested model, handed to `physics`: a procgen
/// mesh of the name first (unless the catalog names a file for it), then
/// the model's file. A model that isn't found is warned about with every
/// path tried; one that doesn't read is warned about with why.
pub(super) fn hand_over(
    physics: &mut PhysicsSystem,
    requests: &[(EntityId, String)],
    config: &ModelLoadConfig,
    sources: &ColliderSources,
) {
    let mut read: HashMap<PathBuf, Option<Arc<ColliderMesh>>> = HashMap::new();
    for (entity, asset) in requests {
        if !config.overrides.contains_key(asset) {
            if let Some(mesh) = sources.made(asset) {
                tracing::debug!(target: "load", kind = "collider",
                    "Collider mesh: procgen '{asset}' ({} triangles)",
                    mesh.triangles.len()
                );
                physics.set_collider_mesh(*entity, mesh.clone());
                continue;
            }
        }
        let path = match resolve_model_path(config, asset) {
            Ok(path) => path,
            Err(missing) => {
                tracing::warn!("collider: {}", config.not_found_text(&missing));
                continue;
            }
        };
        let mesh = read
            .entry(path.clone())
            .or_insert_with(|| match flint_import::import_gltf_geometry(&path) {
                Ok(model) => {
                    let (positions, triangles) = model.triangles();
                    tracing::debug!(target: "load", kind = "collider",
                        "Collider mesh: {} ({} triangles)",
                        flint_core::project::shown(&path),
                        triangles.len()
                    );
                    Some(Arc::new(ColliderMesh {
                        positions,
                        triangles,
                    }))
                }
                Err(e) => {
                    tracing::warn!("collider: {}: {e}", flint_core::project::shown(&path));
                    None
                }
            })
            .clone();
        if let Some(mesh) = mesh {
            physics.set_collider_mesh(*entity, mesh);
        }
    }
}

impl PlayerApp {
    /// The project's collision layers (`[project.physics] layers`), and
    /// the triangles of the mesh colliders `requests` names, for the
    /// bodies the scene's physics is about to make.
    pub(super) fn prepare_physics(&mut self, requests: &[(EntityId, String)]) {
        let layers = CollisionLayers::new(&self.core.project.physics_layers).unwrap_or_else(|e| {
            // The manifest's reader refuses these first; a project made
            // in code might not have been
            tracing::warn!("physics layers: {e}; only `default` is used");
            CollisionLayers::default()
        });
        self.core.physics.set_layers(layers);
        // A new scene's (or chunk's) colliders are walked at the next frame,
        // for the runtime meshes the renderer already holds
        self.core.collider_sources.walk_again();
        if requests.is_empty() {
            return;
        }
        let config = build_model_load_config(
            &self.core.project,
            &self.core.scene_path,
            &self.core.world,
            self.assets.of(&self.core.project),
        );
        // A model still on the procgen queue (`ProcGenMode::Deferred`) has
        // no triangles yet: they come when it's made, and physics waits for
        // them quietly rather than warn or read a file in its place
        #[cfg(feature = "procgen")]
        let requests: Vec<(EntityId, String)> = {
            let resolver = &self.core.procgen_resolver;
            let physics = &mut self.core.physics;
            requests
                .iter()
                .filter(|(entity, asset)| {
                    let queued = resolver.is_pending(asset);
                    if queued {
                        physics.expect_collider_mesh(*entity);
                    }
                    !queued
                })
                .cloned()
                .collect()
        };
        hand_over(
            &mut self.core.physics,
            &requests,
            &config,
            &self.core.collider_sources,
        );
    }
}
