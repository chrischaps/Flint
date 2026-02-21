//! Shared model and texture loading from ECS world
//!
//! Provides [`load_models_from_world`] and [`load_textures_from_world`] that handle
//! the full pipeline: entity scanning, path resolution, GLB import, multi-node
//! expansion, skinned mesh detection, and GPU upload.
//!
//! Callers can perform skeletal animation registration and catalog pre-resolution
//! on top of the returned [`ModelLoadResult`].

use flint_core::{mat4_mul, EntityId, Transform, Vec3};
use flint_ecs::FlintWorld;
use flint_import::{import_gltf, ImportResult, ImportedNode};
use crate::SceneRenderer;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Metadata about a single loaded model.
pub struct LoadedModel {
    pub entity_id: EntityId,
    pub asset_name: String,
    /// The full import result — present for skinned models or models with node
    /// animations that were freshly imported (not already cached).
    pub import_result: Option<ImportResult>,
    pub is_skinned: bool,
    pub was_expanded: bool,
    /// For animated multi-node models: maps glTF node names to child entity IDs.
    /// Used by NodeSync to write animation transforms to the correct entities.
    pub node_map: Option<HashMap<String, EntityId>>,
}

/// Result of loading all models from the world.
pub struct ModelLoadResult {
    pub models: Vec<LoadedModel>,
    /// Entity ID → asset name for entities with skinned meshes.
    pub skinned_entities: HashMap<EntityId, String>,
}

/// Configuration for model and texture path resolution.
pub struct ModelLoadConfig {
    pub scene_dir: PathBuf,
    /// Pre-resolved paths for specific asset names (e.g. from catalog lookup).
    /// Checked before the standard scene_dir/models/ search.
    pub overrides: HashMap<String, PathBuf>,
}

impl ModelLoadConfig {
    /// Create a config from a scene file path, with no overrides.
    pub fn from_scene_path(scene_path: &str) -> Self {
        let scene_dir = Path::new(scene_path)
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .to_path_buf();
        Self {
            scene_dir,
            overrides: HashMap::new(),
        }
    }
}

/// Convert a quaternion [x, y, z, w] to Euler angles (degrees) in XYZ order.
pub fn quat_to_euler_xyz(q: [f32; 4]) -> [f32; 3] {
    let (x, y, z, w) = (q[0], q[1], q[2], q[3]);
    let sinr_cosp = 2.0 * (w * x + y * z);
    let cosr_cosp = 1.0 - 2.0 * (x * x + y * y);
    let roll = sinr_cosp.atan2(cosr_cosp);
    let sinp = 2.0 * (w * y - z * x);
    let pitch = if sinp.abs() >= 1.0 {
        std::f32::consts::FRAC_PI_2.copysign(sinp)
    } else {
        sinp.asin()
    };
    let siny_cosp = 2.0 * (w * z + x * y);
    let cosy_cosp = 1.0 - 2.0 * (y * y + z * z);
    let yaw = siny_cosp.atan2(cosy_cosp);
    [roll.to_degrees(), pitch.to_degrees(), yaw.to_degrees()]
}

/// Resolve the file path for a model asset name.
fn resolve_model_path(config: &ModelLoadConfig, asset_name: &str) -> Option<PathBuf> {
    if let Some(path) = config.overrides.get(asset_name) {
        if path.exists() {
            return Some(path.clone());
        }
    }
    let p = config.scene_dir.join("models").join(format!("{}.glb", asset_name));
    if p.exists() {
        return Some(p);
    }
    if let Some(parent) = config.scene_dir.parent() {
        let p = parent.join("models").join(format!("{}.glb", asset_name));
        if p.exists() {
            return Some(p);
        }
    }
    None
}

/// Resolve the file path for a texture name.
fn resolve_texture_path(config: &ModelLoadConfig, tex_name: &str) -> Option<PathBuf> {
    if let Some(path) = config.overrides.get(tex_name) {
        if path.exists() {
            return Some(path.clone());
        }
    }
    let p = config.scene_dir.join(tex_name);
    if p.exists() {
        return Some(p);
    }
    if let Some(parent) = config.scene_dir.parent() {
        let p = parent.join(tex_name);
        if p.exists() {
            return Some(p);
        }
    }
    None
}

/// Build a 4x4 column-major matrix from a glTF node's TRS.
fn node_to_matrix(node: &ImportedNode) -> [[f32; 4]; 4] {
    Transform {
        position: Vec3::new(node.translation[0], node.translation[1], node.translation[2]),
        rotation: Vec3::ZERO,
        scale: Vec3::new(node.scale[0], node.scale[1], node.scale[2]),
        rotation_quat: Some(node.rotation),
    }
    .to_matrix()
}

/// Compute the accumulated world matrix for every node, recursing from the
/// given root node indices with `parent_matrix` as the starting transform.
fn compute_node_world_matrices(
    import_result: &ImportResult,
    node_indices: &[usize],
    parent_matrix: &[[f32; 4]; 4],
    out: &mut HashMap<usize, [[f32; 4]; 4]>,
) {
    for &idx in node_indices {
        let node = &import_result.nodes[idx];
        let local = node_to_matrix(node);
        let world = mat4_mul(parent_matrix, &local);
        out.insert(idx, world);
        if !node.children.is_empty() {
            compute_node_world_matrices(import_result, &node.children, &world, out);
        }
    }
}

/// Create flat child entities for every mesh-bearing node in the glTF tree.
///
/// Unlike a naive hierarchy mirror, this bakes each node's accumulated world
/// transform into the GPU vertex data at upload time, then stores an identity
/// transform on the entity. This eliminates visual distortion from non-uniform
/// parent scales, which is common in Blender exports.
fn expand_nodes_flat(
    world: &mut FlintWorld,
    import_result: &ImportResult,
    node_indices: &[usize],
    parent_entity_id: EntityId,
    parent_entity_name: &str,
    asset_name: &str,
    renderer: &mut SceneRenderer,
    device: &wgpu::Device,
) {
    // Pre-compute world matrices for every node in the tree
    let identity = Transform::IDENTITY.to_matrix();
    let mut world_matrices = HashMap::new();
    compute_node_world_matrices(import_result, node_indices, &identity, &mut world_matrices);

    // Walk the full tree and create a flat entity for each mesh-bearing node
    let mut stack: Vec<(usize, String)> = node_indices
        .iter()
        .rev()
        .map(|&idx| (idx, parent_entity_name.to_string()))
        .collect();

    let default_color = [0.5_f32, 0.5, 0.5, 1.0];

    while let Some((node_idx, parent_name)) = stack.pop() {
        let node = &import_result.nodes[node_idx];
        let child_name = format!("{}__{}", parent_name, node.name);

        if !node.mesh_primitive_indices.is_empty() {
            let child_id = match world.spawn(&child_name) {
                Ok(id) => id,
                Err(e) => {
                    eprintln!("Failed to spawn child entity '{}': {:?}", child_name, e);
                    // Still push children for further traversal
                    for &c in node.children.iter().rev() {
                        stack.push((c, child_name.clone()));
                    }
                    continue;
                }
            };

            // Identity transform — geometry is already in GLB root space
            let transform = toml::Value::Table({
                let mut t = toml::map::Map::new();
                t.insert(
                    "position".to_string(),
                    toml::Value::Array(vec![
                        toml::Value::Float(0.0),
                        toml::Value::Float(0.0),
                        toml::Value::Float(0.0),
                    ]),
                );
                t
            });
            let _ = world.set_component(child_id, "transform", transform);

            // Upload mesh with baked world transform
            let cache_key = format!("{}/{}", asset_name, node.name);
            let world_mat = world_matrices.get(&node_idx).unwrap_or(&identity);
            renderer.mesh_cache_mut().upload_mesh_subset(
                device,
                &cache_key,
                import_result,
                &node.mesh_primitive_indices,
                default_color,
                Some(world_mat),
            );

            let model = toml::Value::Table({
                let mut m = toml::map::Map::new();
                m.insert("asset".to_string(), toml::Value::String(cache_key));
                m
            });
            let _ = world.set_component(child_id, "model", model);

            // All mesh entities parent directly to the root entity
            let _ = world.set_parent(child_id, parent_entity_id);
            println!("  Expanded node: {} (flat, baked transform)", child_name);
        }

        // Push children for traversal (non-mesh nodes are traversed but not spawned)
        for &c in node.children.iter().rev() {
            stack.push((c, child_name.clone()));
        }
    }
}

/// Create child entities preserving the glTF parent-child hierarchy.
///
/// Unlike `expand_nodes_flat()`, this stores each node's LOCAL TRS on the entity
/// (no vertex baking) and sets parent to the node's actual parent entity.
/// This allows the animation system to write to child transforms and have the
/// renderer's `get_world_matrix()` parent chain compose them correctly.
///
/// Returns a map from glTF node names to their corresponding entity IDs, used
/// by `NodeSync` to route animation tracks to the right entities.
fn expand_nodes_animated(
    world: &mut FlintWorld,
    import_result: &ImportResult,
    root_node_indices: &[usize],
    parent_entity_id: EntityId,
    parent_entity_name: &str,
    asset_name: &str,
    renderer: &mut SceneRenderer,
    device: &wgpu::Device,
) -> HashMap<String, EntityId> {
    let mut node_map: HashMap<String, EntityId> = HashMap::new();
    // Map from ImportedNode index to spawned EntityId
    let mut index_to_entity: HashMap<usize, EntityId> = HashMap::new();
    let default_color = [0.5_f32, 0.5, 0.5, 1.0];

    // Walk the node tree recursively (DFS), creating entities for ALL nodes
    // (not just mesh-bearing ones) to preserve the full hierarchy.
    struct StackEntry {
        node_idx: usize,
        parent_ecs_id: EntityId,
        parent_name: String,
    }

    let mut stack: Vec<StackEntry> = root_node_indices
        .iter()
        .rev()
        .map(|&idx| StackEntry {
            node_idx: idx,
            parent_ecs_id: parent_entity_id,
            parent_name: parent_entity_name.to_string(),
        })
        .collect();

    while let Some(entry) = stack.pop() {
        let node = &import_result.nodes[entry.node_idx];
        let child_name = format!("{}__{}", entry.parent_name, node.name);

        // Spawn entity for this node
        let child_id = match world.spawn(&child_name) {
            Ok(id) => id,
            Err(e) => {
                eprintln!("Failed to spawn child entity '{}': {:?}", child_name, e);
                for &c in node.children.iter().rev() {
                    stack.push(StackEntry {
                        node_idx: c,
                        parent_ecs_id: entry.parent_ecs_id,
                        parent_name: child_name.clone(),
                    });
                }
                continue;
            }
        };

        // Set transform to node's LOCAL TRS (not baked)
        let transform = toml::Value::Table({
            let mut t = toml::map::Map::new();
            t.insert(
                "position".to_string(),
                toml::Value::Array(vec![
                    toml::Value::Float(node.translation[0] as f64),
                    toml::Value::Float(node.translation[1] as f64),
                    toml::Value::Float(node.translation[2] as f64),
                ]),
            );
            // Store quaternion to preserve exact rotation (avoid Euler conversion)
            t.insert(
                "rotation_quat".to_string(),
                toml::Value::Array(vec![
                    toml::Value::Float(node.rotation[0] as f64),
                    toml::Value::Float(node.rotation[1] as f64),
                    toml::Value::Float(node.rotation[2] as f64),
                    toml::Value::Float(node.rotation[3] as f64),
                ]),
            );
            t.insert(
                "scale".to_string(),
                toml::Value::Array(vec![
                    toml::Value::Float(node.scale[0] as f64),
                    toml::Value::Float(node.scale[1] as f64),
                    toml::Value::Float(node.scale[2] as f64),
                ]),
            );
            t
        });
        let _ = world.set_component(child_id, "transform", transform);

        // Upload mesh without baking (bake_transform: None)
        if !node.mesh_primitive_indices.is_empty() {
            let cache_key = format!("{}/{}", asset_name, node.name);
            renderer.mesh_cache_mut().upload_mesh_subset(
                device,
                &cache_key,
                import_result,
                &node.mesh_primitive_indices,
                default_color,
                None, // No vertex baking — transforms live on the entity
            );

            let model = toml::Value::Table({
                let mut m = toml::map::Map::new();
                m.insert("asset".to_string(), toml::Value::String(cache_key));
                m
            });
            let _ = world.set_component(child_id, "model", model);
        }

        // Set parent to preserve hierarchy
        let _ = world.set_parent(child_id, entry.parent_ecs_id);

        // Record in maps
        node_map.insert(node.name.clone(), child_id);
        index_to_entity.insert(entry.node_idx, child_id);

        println!("  Expanded node: {} (animated, local TRS)", child_name);

        // Push children for traversal
        for &c in node.children.iter().rev() {
            stack.push(StackEntry {
                node_idx: c,
                parent_ecs_id: child_id,
                parent_name: child_name.clone(),
            });
        }
    }

    node_map
}

/// Load all models referenced by entities in the world.
///
/// Handles path resolution, GLB import, multi-node expansion, skinned mesh
/// detection, and GPU upload. Returns metadata so callers can perform
/// skeletal animation registration on top.
pub fn load_models_from_world(
    world: &mut FlintWorld,
    renderer: &mut SceneRenderer,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    config: &ModelLoadConfig,
) -> ModelLoadResult {
    let mut result = ModelLoadResult {
        models: Vec::new(),
        skinned_entities: HashMap::new(),
    };

    // Pass 1: Collect entity-model pairs (can't mutate world while iterating)
    // Also collect whether each entity has an animator component (for animated expansion)
    let entity_models: Vec<(EntityId, String, String, bool)> = world
        .all_entities()
        .iter()
        .filter_map(|e| {
            let components = world.get_components(e.id);
            let model_asset = components
                .and_then(|c| c.get("model").cloned())
                .and_then(|model| model.get("asset").and_then(|v| v.as_str().map(String::from)));
            let has_animator = components
                .map(|c| c.get("animator").is_some())
                .unwrap_or(false);
            model_asset.map(|asset| (e.id, e.name.clone(), asset, has_animator))
        })
        .collect();

    // Cache import results for multi-node GLBs so subsequent entities
    // referencing the same asset can be expanded into their own children.
    let mut expansion_cache: HashMap<String, ImportResult> = HashMap::new();

    // Pass 2: Load and expand
    for (entity_id, entity_name, asset_name, has_animator) in &entity_models {
        if renderer.mesh_cache().contains(asset_name) {
            // If this asset was previously expanded (same load call), expand for this entity too
            if let Some(cached_import) = expansion_cache.get(asset_name.as_str()) {
                expand_nodes_flat(
                    world,
                    cached_import,
                    &cached_import.root_nodes,
                    *entity_id,
                    entity_name,
                    asset_name,
                    renderer,
                    device,
                );
                if let Some(components) = world.get_components_mut(*entity_id) {
                    components.remove("model");
                }
                result.models.push(LoadedModel {
                    entity_id: *entity_id,
                    asset_name: asset_name.clone(),
                    import_result: None,
                    is_skinned: false,
                    was_expanded: true,
                    node_map: None,
                });
                continue;
            }

            // Mesh cached from a previous scene/load — re-import to check
            // if this is a multi-node model that needs per-entity expansion.
            if let Some(model_path) = resolve_model_path(config, asset_name) {
                if let Ok(import_result) = import_gltf(&model_path) {
                    if import_result.needs_expansion() {
                        expand_nodes_flat(
                            world,
                            &import_result,
                            &import_result.root_nodes,
                            *entity_id,
                            entity_name,
                            asset_name,
                            renderer,
                            device,
                        );
                        if let Some(components) = world.get_components_mut(*entity_id) {
                            components.remove("model");
                        }
                        result.models.push(LoadedModel {
                            entity_id: *entity_id,
                            asset_name: asset_name.clone(),
                            import_result: None,
                            is_skinned: false,
                            was_expanded: true,
                            node_map: None,
                        });
                        expansion_cache.insert(asset_name.clone(), import_result);
                        continue;
                    }
                }
            }

            if renderer.mesh_cache().contains_skinned(asset_name) {
                result.skinned_entities.insert(*entity_id, asset_name.clone());
                result.models.push(LoadedModel {
                    entity_id: *entity_id,
                    asset_name: asset_name.clone(),
                    import_result: None,
                    is_skinned: true,
                    was_expanded: false,
                    node_map: None,
                });
            }
            continue;
        }

        let model_path = match resolve_model_path(config, asset_name) {
            Some(p) => p,
            None => {
                eprintln!("Model file not found: {}", asset_name);
                continue;
            }
        };

        match import_gltf(&model_path) {
            Ok(import_result) => {
                let has_skins = !import_result.skeletons.is_empty();
                let has_skinned_meshes = import_result
                    .meshes
                    .iter()
                    .any(|m| m.joint_indices.is_some());
                let is_skinned = has_skinned_meshes && has_skins;

                let bounds_info = import_result
                    .bounds()
                    .map(|b| format!(", bounds: {}", b))
                    .unwrap_or_default();

                println!(
                    "Loaded model: {} ({} meshes, {} nodes{}{})",
                    asset_name,
                    import_result.meshes.len(),
                    import_result.nodes.len(),
                    if has_skins {
                        format!(
                            ", {} skins, {} skeletal clips",
                            import_result.skeletons.len(),
                            import_result.skeletal_clips.len()
                        )
                    } else {
                        String::new()
                    },
                    bounds_info,
                );

                let was_expanded;
                let kept_result;
                let node_map;

                if is_skinned {
                    renderer.load_skinned_model(device, queue, asset_name, &import_result);
                    renderer.load_model(device, queue, asset_name, &import_result);
                    result.skinned_entities.insert(*entity_id, asset_name.clone());
                    was_expanded = false;
                    kept_result = Some(import_result);
                    node_map = None;
                } else if import_result.needs_expansion() {
                    // Use the whole-model upload for any code paths that look
                    // up the asset by its base name (e.g. bounds queries).
                    renderer.load_model(device, queue, asset_name, &import_result);

                    if import_result.has_node_animations() && *has_animator {
                        // Hierarchy-preserving expansion: keep local TRS on
                        // each child entity so animation can write to them.
                        let nmap = expand_nodes_animated(
                            world,
                            &import_result,
                            &import_result.root_nodes,
                            *entity_id,
                            entity_name,
                            asset_name,
                            renderer,
                            device,
                        );

                        if let Some(components) = world.get_components_mut(*entity_id) {
                            components.remove("model");
                        }

                        was_expanded = true;
                        kept_result = Some(import_result);
                        node_map = Some(nmap);
                    } else {
                        // Flatten the glTF node hierarchy: bake each node's
                        // accumulated world transform into vertex data so entities
                        // can use identity transforms. This avoids visual distortion
                        // from non-uniform parent scales in the glTF tree.
                        expand_nodes_flat(
                            world,
                            &import_result,
                            &import_result.root_nodes,
                            *entity_id,
                            entity_name,
                            asset_name,
                            renderer,
                            device,
                        );

                        if let Some(components) = world.get_components_mut(*entity_id) {
                            components.remove("model");
                        }

                        // Cache for subsequent entities referencing the same asset
                        expansion_cache.insert(asset_name.clone(), import_result);
                        was_expanded = true;
                        kept_result = None;
                        node_map = None;
                    }
                } else {
                    renderer.load_model(device, queue, asset_name, &import_result);
                    was_expanded = false;
                    kept_result = None;
                    node_map = None;
                }

                result.models.push(LoadedModel {
                    entity_id: *entity_id,
                    asset_name: asset_name.clone(),
                    import_result: kept_result,
                    is_skinned,
                    was_expanded,
                    node_map,
                });
            }
            Err(e) => {
                eprintln!("Failed to load model '{}': {:?}", asset_name, e);
            }
        }
    }

    // Load textures
    load_textures_from_world(world, renderer, device, queue, config);

    result
}

/// Load texture files referenced by material and sprite components.
pub fn load_textures_from_world(
    world: &FlintWorld,
    renderer: &mut SceneRenderer,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    config: &ModelLoadConfig,
) {
    let mut loaded = std::collections::HashSet::new();

    for entity in world.all_entities() {
        let components = world.get_components(entity.id);
        let mut tex_names = Vec::new();

        if let Some(comps) = &components {
            if let Some(material) = comps.get("material") {
                if let Some(tex) = material.get("texture").and_then(|v| v.as_str()) {
                    tex_names.push(tex.to_string());
                }
            }
            if let Some(sprite) = comps.get("sprite") {
                if let Some(tex) = sprite.get("texture").and_then(|v| v.as_str()) {
                    if !tex.is_empty() {
                        tex_names.push(tex.to_string());
                    }
                }
            }
        }

        for tex_name in tex_names {
            if loaded.contains(&tex_name) {
                continue;
            }
            loaded.insert(tex_name.clone());

            let tex_path = match resolve_texture_path(config, &tex_name) {
                Some(p) => p,
                None => {
                    eprintln!("Texture file not found: {}", tex_name);
                    continue;
                }
            };

            match renderer.load_texture_file(device, queue, &tex_name, &tex_path) {
                Ok(true) => {
                    println!("Loaded texture: {}", tex_name);
                }
                Ok(false) => {}
                Err(e) => {
                    eprintln!("Failed to load texture '{}': {}", tex_name, e);
                }
            }
        }
    }
}
