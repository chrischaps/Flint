//! Shared model and texture loading from ECS world
//!
//! Provides [`load_models_from_world`] and [`load_textures_from_world`] that handle
//! the full pipeline: entity scanning, path resolution, GLB import, multi-node
//! expansion, skinned mesh detection, and GPU upload.
//!
//! Callers can perform skeletal animation registration and catalog pre-resolution
//! on top of the returned [`ModelLoadResult`].

use crate::SceneRenderer;
use flint_core::components as comp;
use flint_core::project::{AssetKind, NotFound, Project};
use flint_core::vfs;
use flint_core::EntityId;
use flint_ecs::FlintWorld;
use flint_import::{import_gltf, ImportResult};
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

/// Where a scene's models and textures are found: the project's one asset
/// search (ADR 0114) from the scene's directory, after anything a host
/// resolved by other means.
pub struct ModelLoadConfig {
    /// The project the scene belongs to. Its resolver does the file search.
    pub project: Project,
    pub scene_dir: PathBuf,
    /// Pre-resolved paths for specific asset names (e.g. from catalog lookup).
    /// Checked before the file search.
    pub overrides: HashMap<String, PathBuf>,
    /// What the host asked before the file search, for a not-found warning
    /// to name beside the paths tried: "the asset catalog", "a procgen spec
    /// of the same name".
    pub asked_first: Vec<&'static str>,
}

impl ModelLoadConfig {
    /// A config for a scene file, with no overrides, in the project found
    /// from the scene. A host that already holds the scene's [`Project`]
    /// uses [`in_project`](Self::in_project) rather than discover it again.
    pub fn from_scene_path(scene_path: &str) -> Self {
        Self::in_project(Project::for_scene(Path::new(scene_path)), scene_path)
    }

    /// A config for a scene file in `project`, with no overrides.
    pub fn in_project(project: Project, scene_path: &str) -> Self {
        let scene_dir = Path::new(scene_path)
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .to_path_buf();
        Self {
            project,
            scene_dir,
            overrides: HashMap::new(),
            asked_first: Vec::new(),
        }
    }

    /// A not-found warning's text: every path tried, and what was asked
    /// before the files.
    pub fn not_found_text(&self, missing: &NotFound) -> String {
        if self.asked_first.is_empty() {
            return missing.to_string();
        }
        format!(
            "{missing}
  and before the files: {}",
            self.asked_first.join(", ")
        )
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

/// Whether a `model` component names a mesh that code uploads at runtime
/// (`runtime = true`) rather than a file on disk.
pub fn is_runtime_model(model: &toml::Value) -> bool {
    model
        .get("runtime")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
}

/// The file for a model asset name: the catalog override, then the
/// project's search for a model (`models/<name>.glb`, then `.gltf`, beside
/// the scene and beside each directory up to the project root, then in the
/// mounted engine). The error lists every path tried.
pub fn resolve_model_path(config: &ModelLoadConfig, asset_name: &str) -> Result<PathBuf, NotFound> {
    resolve_for(config, AssetKind::Models, asset_name)
}

/// The file for a texture name: the catalog override, then the project's
/// search for a texture (each directory of the walk and its `textures/`; a
/// name with an extension as written, a bare one also as `<name>.png` and
/// `<name>_color.png`, which is how procgen textures are saved).
pub fn resolve_texture_path(config: &ModelLoadConfig, tex_name: &str) -> Result<PathBuf, NotFound> {
    resolve_for(config, AssetKind::Textures, tex_name)
}

fn resolve_for(config: &ModelLoadConfig, kind: AssetKind, name: &str) -> Result<PathBuf, NotFound> {
    if let Some(path) = config.overrides.get(name) {
        if vfs::exists(path) {
            return Ok(path.clone());
        }
    }
    config.project.resolve(kind, name, &config.scene_dir)
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
                tracing::warn!("Failed to spawn child entity '{}': {:?}", child_name, e);
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
        let _ = world.set_component(child_id, comp::TRANSFORM, transform);

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
            // The subset clones materials straight from the import, whose
            // texture references are bare names; the textures themselves were
            // uploaded by `load_model` namespaced by asset, so patch to match
            // (otherwise a textured node draws with the default white texture).
            if let Some(meshes) = renderer.mesh_cache_mut().get_mut(&cache_key) {
                for mesh in meshes.iter_mut() {
                    for tex in [
                        &mut mesh.material.base_color_texture,
                        &mut mesh.material.normal_texture,
                        &mut mesh.material.metallic_roughness_texture,
                        &mut mesh.material.emissive_texture,
                    ] {
                        if let Some(t) = tex.take() {
                            *tex = Some(format!("{}::{}", asset_name, t));
                        }
                    }
                }
            }

            let model = toml::Value::Table({
                let mut m = toml::map::Map::new();
                m.insert("asset".to_string(), toml::Value::String(cache_key));
                m
            });
            let _ = world.set_component(child_id, comp::MODEL, model);
        }

        // Set parent to preserve hierarchy
        let _ = world.set_parent(child_id, entry.parent_ecs_id);

        // Record in maps
        node_map.insert(node.name.clone(), child_id);
        index_to_entity.insert(entry.node_idx, child_id);

        eprintln!("  Expanded node: {} (animated, local TRS)", child_name);

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
///
/// Entities whose model sets `runtime = true` are skipped: their mesh is
/// uploaded by code at runtime (`SceneRenderer::load_procedural_mesh`), so
/// there's no file to find.
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
                .and_then(|c| c.get(comp::MODEL).cloned())
                .filter(|model| !is_runtime_model(model))
                .and_then(|model| {
                    model
                        .get("asset")
                        .and_then(|v| v.as_str().map(String::from))
                });
            let has_animator = components
                .map(|c| c.get(comp::ANIMATOR).is_some())
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
                let nmap = expand_nodes_animated(
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
                    components.remove(comp::MODEL);
                }
                result.models.push(LoadedModel {
                    entity_id: *entity_id,
                    asset_name: asset_name.clone(),
                    import_result: None,
                    is_skinned: false,
                    was_expanded: true,
                    node_map: Some(nmap),
                });
                continue;
            }

            // Mesh cached from a previous scene/load — re-import to check
            // if this is a multi-node model that needs per-entity expansion.
            // Not for a mesh made at runtime (procgen): a file of the same
            // name must not replace it (catalog > procgen > file, #32).
            // Nor for a skinned model: the first entity to load it drew it
            // skinned, and so does every other one (below). Expanding the
            // second into nodes took its model away, so of five figures one
            // drew, in its bind pose (#624). A scene reload (`flint edit
            // --watch`) is the same case for a lone figure: the cache keeps
            // the GLB from the first load, and expanding it there made the
            // figure vanish (#331).
            let file = if renderer.mesh_cache().is_procedural(asset_name)
                || renderer.mesh_cache().contains_skinned(asset_name)
            {
                None
            } else {
                resolve_model_path(config, asset_name).ok()
            };
            if let Some(model_path) = file {
                if let Ok(import_result) = import_gltf(&model_path) {
                    if import_result.needs_expansion() {
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
                            components.remove(comp::MODEL);
                        }
                        result.models.push(LoadedModel {
                            entity_id: *entity_id,
                            asset_name: asset_name.clone(),
                            import_result: None,
                            is_skinned: false,
                            was_expanded: true,
                            node_map: Some(nmap),
                        });
                        expansion_cache.insert(asset_name.clone(), import_result);
                        continue;
                    }
                }
            }

            if renderer.mesh_cache().contains_skinned(asset_name) {
                result
                    .skinned_entities
                    .insert(*entity_id, asset_name.clone());
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
            Ok(p) => p,
            Err(missing) => {
                tracing::warn!("{}", config.not_found_text(&missing));
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

                eprintln!(
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
                if let Some(chain) =
                    flint_import::lod_chain(&import_result.nodes, &import_result.root_nodes)
                {
                    // The importer banded it (#193), unless the file did.
                    let switches: Vec<String> = chain
                        .iter()
                        .skip(1)
                        .filter_map(|&n| import_result.nodes[n].mesh_primitive_indices.first())
                        .filter_map(|&p| import_result.meshes[p].lod)
                        .map(|band| format!("{:.1}", band.near))
                        .collect();
                    eprintln!(
                        "  LOD chain: {} levels, switching at {} m",
                        chain.len(),
                        switches.join(", ")
                    );
                }

                let was_expanded;
                let kept_result;
                let node_map;

                if is_skinned {
                    renderer.load_skinned_model(device, queue, asset_name, &import_result);
                    renderer.load_model(device, queue, asset_name, &import_result);
                    result
                        .skinned_entities
                        .insert(*entity_id, asset_name.clone());
                    was_expanded = false;
                    kept_result = Some(import_result);
                    node_map = None;
                } else if import_result.needs_expansion() {
                    // Use the whole-model upload for any code paths that look
                    // up the asset by its base name (e.g. bounds queries).
                    renderer.load_model(device, queue, asset_name, &import_result);

                    // Always use hierarchy-preserving expansion: keep local TRS
                    // on each child entity so (a) the scene tree shows the real
                    // glTF node hierarchy, and (b) animation can write to them.
                    let has_animations = import_result.has_node_animations() && *has_animator;
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
                        components.remove(comp::MODEL);
                    }

                    was_expanded = true;
                    if has_animations {
                        // Animated models need the import result for NodeSync
                        // registration; also cache for subsequent entities.
                        kept_result = Some(import_result);
                    } else {
                        // Cache for subsequent entities referencing the same asset
                        expansion_cache.insert(asset_name.clone(), import_result);
                        kept_result = None;
                    }
                    node_map = Some(nmap);
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
                tracing::warn!("Failed to load model '{}': {:?}", asset_name, e);
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
            if let Some(material) = comps.get(comp::MATERIAL) {
                if let Some(tex) = material.get("texture").and_then(|v| v.as_str()) {
                    tex_names.push(tex.to_string());
                }
            }
            if let Some(sprite) = comps.get(comp::SPRITE) {
                if let Some(tex) = sprite.get("texture").and_then(|v| v.as_str()) {
                    if !tex.is_empty() {
                        tex_names.push(tex.to_string());
                    }
                }
            }
            // Fonts ui_text names: built into glyph atlases and uploaded
            // here, so extraction only lays text out (ADR 0113)
            if let Some(ui_text) = comps.get(comp::UI_TEXT) {
                if let Some(font) = ui_text.get("font").and_then(|v| v.as_str()) {
                    if !font.is_empty() {
                        let scope = flint_core::project::AssetScope::new(
                            config.project.clone(),
                            config.scene_dir.clone(),
                        );
                        renderer.load_ui_font(device, queue, font, &scope);
                    }
                }
            }
        }

        for tex_name in tex_names {
            if loaded.contains(&tex_name) {
                continue;
            }
            loaded.insert(tex_name.clone());

            // Already uploaded (procgen made it, or an earlier load did):
            // nothing to find on disk, and nothing missing to warn about.
            if renderer
                .texture_cache()
                .is_some_and(|tc| tc.contains(&tex_name))
            {
                continue;
            }

            let tex_path = match resolve_texture_path(config, &tex_name) {
                Ok(p) => p,
                Err(missing) => {
                    tracing::warn!("{}", config.not_found_text(&missing));
                    continue;
                }
            };

            match renderer.load_texture_file(device, queue, &tex_name, &tex_path) {
                Ok(true) => {
                    eprintln!("Loaded texture: {}", tex_name);
                }
                Ok(false) => {}
                Err(e) => {
                    tracing::warn!("Failed to load texture '{}': {}", tex_name, e);
                }
            }

            // Try loading companion PBR maps (_normal, _roughness) alongside the color texture
            for suffix in &["_normal", "_roughness"] {
                let companion_name = format!("{}{}", tex_name, suffix);
                if loaded.contains(&companion_name) {
                    continue;
                }
                if let Ok(companion_path) = resolve_texture_path(config, &companion_name) {
                    loaded.insert(companion_name.clone());
                    match renderer.load_texture_file(
                        device,
                        queue,
                        &companion_name,
                        &companion_path,
                    ) {
                        Ok(true) => {
                            eprintln!("Loaded texture: {}", companion_name);
                        }
                        Ok(false) => {}
                        Err(e) => {
                            tracing::warn!("Failed to load texture '{}': {}", companion_name, e);
                        }
                    }
                }
            }
        }
    }
}
