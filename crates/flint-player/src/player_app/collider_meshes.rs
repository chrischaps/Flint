//! What the player gives physics before a scene's bodies are made (#729):
//! the project's collision layers, and the triangles of every `mesh` and
//! `convex` collider, read from the model each names.
//!
//! The requests are read before the renderer loads the scene's models: a
//! model of several nodes is expanded into child entities and its parent
//! loses its `model`, so afterwards the parent no longer says which model
//! it drew. The triangles come from the model file by the project's one
//! search (the model loader's own `resolve_model_path`), through
//! `flint_import::import_gltf_geometry`, which skips the textures; a model
//! used by several colliders is read once a scene. A model made at runtime
//! (procgen) has no file and gives none, and physics says so on the entity.

use super::scene_loading::build_model_load_config;
use super::PlayerApp;
use flint_core::EntityId;
use flint_physics::{ColliderMesh, CollisionLayers, PhysicsSystem};
use flint_render::model_loader::{resolve_model_path, ModelLoadConfig};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

/// Every `mesh` or `convex` collider and the model it names, read now.
pub(super) fn requests(world: &flint_ecs::FlintWorld) -> Vec<(EntityId, String)> {
    PhysicsSystem::collider_mesh_requests(world)
}

/// The triangles of each requested model, handed to `physics`. A model
/// that isn't found is warned about with every path tried; one that
/// doesn't read is warned about with why.
pub(super) fn hand_over(
    physics: &mut PhysicsSystem,
    requests: &[(EntityId, String)],
    config: &ModelLoadConfig,
) {
    let mut read: HashMap<PathBuf, Option<Arc<ColliderMesh>>> = HashMap::new();
    for (entity, asset) in requests {
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
        if requests.is_empty() {
            return;
        }
        let config = build_model_load_config(
            &self.core.project,
            &self.core.scene_path,
            &self.core.world,
            self.assets.of(&self.core.project),
        );
        hand_over(&mut self.core.physics, requests, &config);
    }
}
