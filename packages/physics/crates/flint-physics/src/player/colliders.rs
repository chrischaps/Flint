//! The triangles of every `mesh` and `convex` collider (#729, #1222), from
//! wherever the model it names came from: the player's until #1617, the
//! physics package's since.
//!
//! **A model's file.** The requests are read before the host loads the
//! scene's models ([`read_requests`], the package's `scene_read`): a model
//! of several nodes is expanded into child entities and its parent loses
//! its `model`, so afterwards the parent no longer says which model it
//! drew. The triangles come from the model file by the project's one search
//! (the model loader's own `resolve_model_path`), through
//! `flint_import::import_gltf_geometry`, which skips the textures; a model
//! used by several colliders is read once a scene.
//!
//! **A made mesh** (catalog > procgen > file, #32). An asset maker leaves
//! each mesh's triangles in the kernel's
//! [`MadeTriangles`](flint_package::surfaces::MadeTriangles) by name before
//! the mesh is drawn; a request names one of those ahead of a file. One
//! the queue finishes later (`ProcGenMode::Deferred`) reaches physics on
//! the next frame, and its body's collider is made again.
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
//! Each frame the package's subsystem asks [`Colliders::refresh`] at
//! `Stage::Scripts`, before the fixed steps. It walks the colliders only
//! when something could have changed: a made mesh, a runtime mesh uploaded
//! or removed (the renderer's `runtime_mesh_generation`), or an entity
//! spawned.

use crate::{ColliderMesh, PhysicsSystem};
use flint_core::components as comp;
use flint_core::EntityId;
use flint_ecs::FlintWorld;
use flint_geometry::ray_mesh::RayMesh;
use flint_package::surfaces::MadeTriangles;
use flint_render::model_loader::{resolve_model_path, ModelLoadConfig};
use flint_render::SceneRenderer;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

/// What physics keeps of the scene's mesh colliders between a scene's
/// models and its bodies, and for the run: a run resource.
#[derive(Default)]
pub struct Colliders {
    /// Every `mesh` or `convex` collider and the model it names, read
    /// before the models loaded; taken by the entry or the chunk's load
    pub(crate) requests: Vec<(EntityId, String)>,
    /// Each runtime mesh's triangles, from the ray target they were taken
    /// from, so many entities drawing one mesh share one set
    runtime: HashMap<String, (Arc<RayMesh>, Arc<ColliderMesh>)>,
    /// What the colliders were last walked at: the made meshes' generation,
    /// the renderer's runtime mesh generation, the world's next id. `None`
    /// walks at the next refresh (a scene was just entered).
    walked_at: Option<(u64, u64, u64)>,
    /// Entities told their runtime mesh keeps no triangles
    warned: HashSet<EntityId>,
}

/// The package's `scene_read`: which model each mesh collider takes its
/// triangles from, before the host expands a model of several nodes and
/// its parent loses its `model` (#729). A chunk's: its entities' alone.
pub(crate) fn read_requests(
    world: &FlintWorld,
    ids: Option<&[EntityId]>,
    resources: &mut flint_package::Resources,
) {
    let mut requests = PhysicsSystem::collider_mesh_requests(world);
    if let Some(ids) = ids {
        requests.retain(|(id, _)| ids.contains(id));
    }
    resources.get_or_insert_with(Colliders::default).requests = requests;
}

impl Colliders {
    /// Walk every collider again at the next refresh, whatever changed.
    pub(crate) fn walk_again(&mut self) {
        self.walked_at = None;
        self.warned.clear();
    }

    /// Whether [`refresh`](Self::refresh) has anything to look at: the
    /// made meshes, the runtime meshes or the world's ids moved since the
    /// last walk.
    pub(crate) fn due(
        &self,
        world: &FlintWorld,
        renderer: Option<&SceneRenderer>,
        made_generation: u64,
    ) -> bool {
        self.walked_at != Some(Self::stamp(world, renderer, made_generation))
    }

    fn stamp(world: &FlintWorld, renderer: Option<&SceneRenderer>, made: u64) -> (u64, u64, u64) {
        (
            made,
            renderer.map_or(0, SceneRenderer::runtime_mesh_generation),
            world.next_id().raw(),
        )
    }

    /// Hand physics the triangles that arrived since the last call: a made
    /// mesh, a runtime mesh uploaded or replaced, or an entity spawned that
    /// names one. One comparison when none of those moved.
    pub(crate) fn refresh(
        &mut self,
        world: &FlintWorld,
        physics: &mut PhysicsSystem,
        renderer: Option<&SceneRenderer>,
        made: &MadeTriangles,
    ) {
        let now = Self::stamp(world, renderer, made.generation());
        let before = self.walked_at.replace(now);
        if before == Some(now) {
            return;
        }
        flint_core::scope!("collider_meshes::refresh");
        match before {
            // Only spawns since: ids are never reused, so the new ones are
            // the ids from the last walk's next id on (a game spawning a
            // bullet a frame walks one id, not every collider)
            Some((made_at, meshes, from)) if made_at == now.0 && meshes == now.1 => {
                for raw in from..now.2 {
                    self.offer(EntityId::from_raw(raw), world, physics, renderer, made);
                }
            }
            _ => {
                for &entity in world.entities_with_component(comp::COLLIDER) {
                    self.offer(entity, world, physics, renderer, made);
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

    /// Hand physics an entity's made or runtime triangles, when its `mesh`
    /// or `convex` collider names a model that has some it doesn't hold
    /// yet.
    fn offer(
        &mut self,
        entity: EntityId,
        world: &FlintWorld,
        physics: &mut PhysicsSystem,
        renderer: Option<&SceneRenderer>,
        made: &MadeTriangles,
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
            made.get(name).cloned()
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
            // An asset maker's (procgen's, #1557): physics names none
            if runtime { "runtime mesh" } else { "made mesh" },
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

/// The triangles of each requested model, handed to `physics`: a made mesh
/// of the name first (unless the catalog names a file for it), then the
/// model's file. A model that isn't found is warned about with every path
/// tried; one that doesn't read is warned about with why.
pub(crate) fn hand_over(
    physics: &mut PhysicsSystem,
    requests: &[(EntityId, String)],
    config: &ModelLoadConfig,
    made: &MadeTriangles,
) {
    let mut read: HashMap<PathBuf, Option<Arc<ColliderMesh>>> = HashMap::new();
    for (entity, asset) in requests {
        if !config.overrides.contains_key(asset) {
            if let Some(mesh) = made.get(asset) {
                tracing::debug!(target: "load", kind = "collider",
                    "Collider mesh: made mesh '{asset}' ({} triangles)",
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
