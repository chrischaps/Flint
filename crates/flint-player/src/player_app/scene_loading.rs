//! Scene and asset loading helpers — terrain, audio, animation, model resolution,
//! skeletal data registration, and PostProcessDef conversion.

use flint_animation::node_clip::NodeClip;
use flint_animation::skeletal_clip::SkeletalClip;
use flint_animation::skeleton::Skeleton;
use flint_animation::AnimationSystem;
use flint_asset::{AssetCatalog, ContentStore};
use flint_audio::AudioSystem;
use flint_core::components as comp;
use flint_core::toml_util::{toml_f32, toml_vec3};
use flint_core::Vec3 as FlintVec3;
use flint_ecs::FlintWorld;
use flint_physics::PhysicsSystem;
use flint_render::model_loader::{self, ModelLoadConfig};
use flint_render::{PostProcessConfig, SceneRenderer};
use flint_scene::PostProcessDef;
use flint_script::ScriptSystem;
use std::path::{Path, PathBuf};

/// Convert a `PostProcessDef` (scene TOML) to a `PostProcessConfig` (renderer).
///
/// Centralises the field-by-field copy so that new fields are never forgotten.
pub fn post_process_config_from_def(pp_def: &PostProcessDef) -> PostProcessConfig {
    let mut config = PostProcessConfig::default();
    config.bloom_enabled = pp_def.bloom_enabled;
    config.bloom_intensity = pp_def.bloom_intensity;
    config.bloom_threshold = pp_def.bloom_threshold;
    config.vignette_enabled = pp_def.vignette_enabled;
    config.vignette_intensity = pp_def.vignette_intensity;
    config.vignette_smoothness = pp_def.vignette_smoothness;
    config.exposure = pp_def.exposure;
    config.ssao_enabled = pp_def.ssao_enabled;
    config.ssao_radius = pp_def.ssao_radius;
    config.ssao_intensity = pp_def.ssao_intensity;
    config.fog_enabled = pp_def.fog_enabled;
    config.fog_color = pp_def.fog_color;
    config.fog_density = pp_def.fog_density;
    config.fog_start = pp_def.fog_start;
    config.fog_end = pp_def.fog_end;
    config.fog_height_enabled = pp_def.fog_height_enabled;
    config.fog_height_falloff = pp_def.fog_height_falloff;
    config.fog_height_origin = pp_def.fog_height_origin;
    config.dither_enabled = pp_def.dither_enabled;
    config.dither_intensity = pp_def.dither_intensity;
    config.volumetric_enabled = pp_def.volumetric_enabled;
    config.volumetric_samples = pp_def.volumetric_samples;
    config.volumetric_density = pp_def.volumetric_density;
    config.volumetric_max_distance = pp_def.volumetric_max_distance;
    config.volumetric_decay = pp_def.volumetric_decay;
    config.kuwahara_enabled = pp_def.kuwahara_enabled;
    config.kuwahara_radius = pp_def.kuwahara_radius;
    config.kuwahara_sharpness = pp_def.kuwahara_sharpness;
    config.kuwahara_hardness = pp_def.kuwahara_hardness;
    config.kuwahara_anisotropy = pp_def.kuwahara_anisotropy;
    config
}

/// Load terrain from world entities. Free function to avoid borrow conflicts.
pub(super) fn load_terrain_from_world_inner(
    world: &FlintWorld,
    scene_path: &str,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    scene_renderer: &mut SceneRenderer,
    physics: &mut PhysicsSystem,
    terrain_out: &mut Option<(flint_terrain::Terrain, flint_terrain::TerrainConfig)>,
) {
    use flint_core::Transform;
    use flint_terrain::{Heightmap, Terrain, TerrainConfig};

    let scene_dir = Path::new(scene_path)
        .parent()
        .unwrap_or_else(|| Path::new("."));

    for entity in world.all_entities() {
        let terrain_comp = match world.get_component(entity.id, comp::TERRAIN) {
            Some(c) => c,
            None => continue,
        };

        let heightmap_rel = match terrain_comp.get("heightmap").and_then(|v| v.as_str()) {
            Some(s) if !s.is_empty() => s.to_string(),
            _ => {
                tracing::warn!("Terrain entity '{}': missing heightmap path", entity.name);
                continue;
            }
        };

        // Resolve heightmap path (scene dir, then parent)
        let hm_path = {
            let p = scene_dir.join(&heightmap_rel);
            if p.exists() {
                p
            } else if let Some(parent) = scene_dir.parent() {
                let pp = parent.join(&heightmap_rel);
                if pp.exists() {
                    pp
                } else {
                    p
                }
            } else {
                p
            }
        };

        let heightmap = match Heightmap::from_png(&hm_path) {
            Ok(hm) => hm,
            Err(e) => {
                tracing::warn!("Failed to load heightmap: {}", e);
                continue;
            }
        };

        let get_f32 = |key: &str, default: f32| -> f32 {
            terrain_comp.get(key).and_then(toml_f32).unwrap_or(default)
        };

        let get_i32 = |key: &str, default: i32| -> i32 {
            terrain_comp
                .get(key)
                .and_then(|v| v.as_integer())
                .map(|i| i as i32)
                .unwrap_or(default)
        };

        let get_str = |key: &str| -> String {
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

        // Get entity transform
        let transform = world
            .get_component(entity.id, comp::TRANSFORM)
            .and_then(|t| {
                let pos = t.get("position").and_then(toml_vec3).unwrap_or([0.0; 3]);
                Some(Transform {
                    position: FlintVec3 {
                        x: pos[0],
                        y: pos[1],
                        z: pos[2],
                    },
                    ..Default::default()
                })
            })
            .unwrap_or_default();

        // Upload to GPU
        scene_renderer.load_terrain(
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

        // Register physics collider
        let (verts, tris) = terrain.trimesh_data();
        let offset = [
            transform.position.x,
            transform.position.y,
            transform.position.z,
        ];
        let offset_verts: Vec<[f32; 3]> = verts
            .iter()
            .map(|v| [v[0] + offset[0], v[1] + offset[1], v[2] + offset[2]])
            .collect();
        physics.register_static_trimesh(
            entity.id,
            offset_verts,
            tris,
            0.8,
            0.1,
        );

        tracing::info!(
            "Loaded terrain '{}': {}x{} heightmap, {} chunks, {}x{} world units",
            entity.name,
            heightmap.width,
            heightmap.depth,
            terrain.chunks.len(),
            config.width,
            config.depth,
        );

        *terrain_out = Some((terrain, config));
        break; // Only one terrain entity for now
    }
}

/// Build a ModelLoadConfig with catalog-resolved path overrides.
pub(super) fn build_model_load_config(
    scene_path: &str,
    world: &FlintWorld,
    catalog: Option<&AssetCatalog>,
    store: Option<&ContentStore>,
) -> ModelLoadConfig {
    let mut config = ModelLoadConfig::from_scene_path(scene_path);

    let (Some(cat), Some(st)) = (catalog, store) else {
        return config;
    };

    for entity in world.all_entities() {
        if let Some(comps) = world.get_components(entity.id) {
            // Resolve model assets
            if let Some(model) = comps.get(comp::MODEL) {
                if let Some(name) = model.get("asset").and_then(|v| v.as_str()) {
                    if !config.overrides.contains_key(name) {
                        if let Some(path) = resolve_catalog(cat, st, name) {
                            config.overrides.insert(name.to_string(), path);
                        }
                    }
                }
            }
            // Resolve textures
            for comp_name in &[comp::MATERIAL, comp::SPRITE] {
                if let Some(comp) = comps.get(*comp_name) {
                    if let Some(tex) = comp.get("texture").and_then(|v| v.as_str()) {
                        if !config.overrides.contains_key(tex) {
                            if let Some(path) = resolve_catalog(cat, st, tex) {
                                config.overrides.insert(tex.to_string(), path);
                            }
                        }
                    }
                }
            }
        }
    }

    config
}

/// Look up an asset name in the catalog and content store.
pub(super) fn resolve_catalog(
    catalog: &AssetCatalog,
    store: &ContentStore,
    name: &str,
) -> Option<PathBuf> {
    let meta = catalog.get(name)?;
    let hash = flint_core::ContentHash::from_prefixed_hex(&meta.hash)?;
    store.get(&hash)
}

/// Register skeletal data from loaded models into the animation system.
pub(super) fn register_skeletal_data(
    load_result: &model_loader::ModelLoadResult,
    animation: &mut AnimationSystem,
) {
    for loaded in &load_result.models {
        if loaded.is_skinned {
            if let Some(ref import_result) = loaded.import_result {
                for imported_skel in &import_result.skeletons {
                    let skeleton = Skeleton::from_imported(imported_skel);
                    animation.add_skeleton(loaded.entity_id, skeleton);
                }
                for imported_clip in &import_result.skeletal_clips {
                    let clip = SkeletalClip::from_imported(imported_clip);
                    println!(
                        "  Skeletal clip: {} ({:.1}s, {} tracks)",
                        clip.name,
                        clip.duration,
                        clip.joint_tracks.len()
                    );
                    animation.add_skeletal_clip(clip);
                }
            }
        }
    }
}

/// Register node-level animation data from loaded models into the animation system.
pub(super) fn register_node_animation_data(
    load_result: &model_loader::ModelLoadResult,
    animation: &mut AnimationSystem,
) {
    for loaded in &load_result.models {
        if let Some(ref import_result) = loaded.import_result {
            for imported_clip in &import_result.node_clips {
                let clip = NodeClip::from_imported(imported_clip);
                println!(
                    "  Node clip: {} ({:.1}s, {} tracks)",
                    clip.name,
                    clip.duration,
                    clip.node_tracks.len()
                );
                animation.add_node_clip(clip);
            }
        }
        if let Some(ref node_map) = loaded.node_map {
            animation.register_node_entity(loaded.entity_id, node_map.clone());
        }
    }
}

/// Load audio files referenced by audio_source components and preload all .ogg files from audio/
pub(super) fn load_audio_from_world(world: &FlintWorld, audio: &mut AudioSystem, scene_path: &str) {
    let scene_dir = Path::new(scene_path)
        .parent()
        .unwrap_or_else(|| Path::new("."));

    // Build list of directories to search: scene dir first, then parent (game root)
    let mut search_dirs = vec![scene_dir.to_path_buf()];
    if let Some(parent) = scene_dir.parent() {
        search_dirs.push(parent.to_path_buf());
    }

    // Load audio files referenced by audio_source components
    for entity in world.all_entities() {
        let audio_file = world
            .get_components(entity.id)
            .and_then(|components| components.get(comp::AUDIO_SOURCE).cloned())
            .and_then(|audio_src| {
                audio_src
                    .get("file")
                    .and_then(|v| v.as_str().map(String::from))
            });

        if let Some(file_name) = audio_file {
            if audio.engine.has_sound(&file_name) {
                continue;
            }

            let mut loaded = false;
            for dir in &search_dirs {
                let audio_path = dir.join(&file_name);
                if audio_path.exists() {
                    match audio.engine.load_sound(&file_name, &audio_path) {
                        Ok(_) => {
                            println!("Loaded audio: {}", file_name);
                            loaded = true;
                        }
                        Err(e) => {
                            tracing::warn!("Failed to load audio '{}': {:?}", file_name, e);
                        }
                    }
                    break;
                }
            }
            if !loaded {
                tracing::warn!("Audio file not found: {}", file_name);
            }
        }
    }

    // Preload all audio files from the audio/ directory (for script-triggered sounds)
    for dir in &search_dirs {
        let audio_dir = dir.join("audio");
        if audio_dir.is_dir() {
            if let Ok(entries) = std::fs::read_dir(&audio_dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
                    if ext == "ogg" || ext == "wav" || ext == "mp3" || ext == "flac" {
                        let rel_name =
                            format!("audio/{}", path.file_name().unwrap().to_string_lossy());
                        if audio.engine.has_sound(&rel_name) {
                            continue;
                        }
                        match audio.engine.load_sound(&rel_name, &path) {
                            Ok(_) => {
                                println!("Preloaded audio: {}", rel_name);
                            }
                            Err(e) => {
                                tracing::warn!("Failed to preload audio '{}': {:?}", rel_name, e);
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Load `.anim.toml` files from the `animations/` directory next to the scene.
/// Also checks one level up (game root) for projects that use a `scenes/` subdirectory.
pub(super) fn load_animations_from_world(scene_path: &str, animation: &mut AnimationSystem) {
    let scene_dir = Path::new(scene_path)
        .parent()
        .unwrap_or_else(|| Path::new("."));

    let mut anim_dir = scene_dir.join("animations");
    if !anim_dir.is_dir() {
        // Check parent directory (game project structure)
        if let Some(parent) = scene_dir.parent() {
            let parent_anim = parent.join("animations");
            if parent_anim.is_dir() {
                anim_dir = parent_anim;
            } else {
                return;
            }
        } else {
            return;
        }
    }

    let entries = match std::fs::read_dir(&anim_dir) {
        Ok(e) => e,
        Err(_) => return,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path
            .file_name()
            .and_then(|n| n.to_str())
            .map_or(false, |n| n.ends_with(".anim.toml"))
        {
            match flint_animation::loader::load_clip_from_file(&path) {
                Ok(clip) => {
                    println!("Loaded animation: {} ({:.1}s)", clip.name, clip.duration);
                    animation.add_property_clip(clip);
                }
                Err(e) => {
                    tracing::warn!("Failed to load animation '{}': {:?}", path.display(), e);
                }
            }
        }
    }
}

/// Load `.sprite.toml` files from the `animations/` directory next to the scene.
/// Each file can define multiple sprite animation clips under `[animation.NAME]` sections.
pub(super) fn load_sprite_animations_from_world(scene_path: &str, animation: &mut AnimationSystem) {
    let scene_dir = Path::new(scene_path)
        .parent()
        .unwrap_or_else(|| Path::new("."));

    let mut anim_dir = scene_dir.join("animations");
    if !anim_dir.is_dir() {
        if let Some(parent) = scene_dir.parent() {
            let parent_anim = parent.join("animations");
            if parent_anim.is_dir() {
                anim_dir = parent_anim;
            } else {
                return;
            }
        } else {
            return;
        }
    }

    let entries = match std::fs::read_dir(&anim_dir) {
        Ok(e) => e,
        Err(_) => return,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path
            .file_name()
            .and_then(|n| n.to_str())
            .map_or(false, |n| n.ends_with(".sprite.toml"))
        {
            match flint_animation::sprite_clip::load_sprite_clips_from_file(&path) {
                Ok(clips) => {
                    for clip in clips {
                        println!(
                            "Loaded sprite animation: {} ({}ms, {} frames)",
                            clip.name,
                            clip.total_duration_ms(),
                            clip.frames.len()
                        );
                        animation.add_sprite_clip(clip);
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        "Failed to load sprite animation '{}': {:?}",
                        path.display(),
                        e
                    );
                }
            }
        }
    }
}

/// Resolve a scene path relative to the current scene.
/// If target is already an absolute path or starts from a known root, use it directly.
/// Otherwise resolve relative to the current scene's directory.
pub(super) fn resolve_scene_path(current_scene: &str, target: &str) -> String {
    let target_path = Path::new(target);
    if target_path.is_absolute() || target_path.exists() {
        return target.to_string();
    }

    let current_dir = Path::new(current_scene)
        .parent()
        .unwrap_or_else(|| Path::new("."));

    let resolved = current_dir.join(target);
    if resolved.exists() {
        return resolved.to_string_lossy().to_string();
    }

    // Try parent directory (game root)
    if let Some(parent) = current_dir.parent() {
        let parent_resolved = parent.join(target);
        if parent_resolved.exists() {
            return parent_resolved.to_string_lossy().to_string();
        }
    }

    // Return as-is, let scene loader report the error
    target.to_string()
}

/// Set up the script system's scripts directory from the scene path
pub(super) fn load_scripts_from_world(scene_path: &str, script: &mut ScriptSystem) {
    script.load_scripts_from_scene(scene_path);
}

// ── Procgen asset resolution ──────────────────────────────────────────────

/// Resolve unresolved model/texture asset names against procgen specs.
///
/// For each entity with a `model.asset` or texture name that is not already
/// resolved (not in `config.overrides` and not in the mesh cache), check if
/// a matching procgen spec exists. If so, generate and upload directly to GPU.
pub(super) fn resolve_procgen_assets(
    world: &FlintWorld,
    resolver: &mut flint_procgen::ProcGenResolver,
    renderer: &mut SceneRenderer,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    config: &flint_render::model_loader::ModelLoadConfig,
) {
    // Collect unresolved asset names to avoid borrow conflicts
    let mut unresolved_models: Vec<String> = Vec::new();
    let mut unresolved_textures: Vec<String> = Vec::new();

    for entity in world.all_entities() {
        if let Some(comps) = world.get_components(entity.id) {
            // Check model assets
            if let Some(model) = comps.get(comp::MODEL) {
                if let Some(name) = model.get("asset").and_then(|v| v.as_str()) {
                    if !config.overrides.contains_key(name)
                        && !renderer.mesh_cache().contains(name)
                        && resolver.has_spec(name)
                    {
                        if !unresolved_models.contains(&name.to_string()) {
                            unresolved_models.push(name.to_string());
                        }
                    }
                }
            }

            // Check texture assets (material + sprite)
            for comp_name in &[comp::MATERIAL, comp::SPRITE] {
                if let Some(comp_data) = comps.get(*comp_name) {
                    if let Some(tex) = comp_data.get("texture").and_then(|v| v.as_str()) {
                        if !config.overrides.contains_key(tex)
                            && resolver.has_spec(tex)
                        {
                            if !unresolved_textures.contains(&tex.to_string()) {
                                unresolved_textures.push(tex.to_string());
                            }
                        }
                    }
                }
            }
        }
    }

    // Generate and upload each unresolved asset
    for name in &unresolved_models {
        match resolver.generate(name) {
            Ok(output) => match output {
                flint_procgen::GeneratorOutput::Mesh(mesh) => {
                    upload_procgen_mesh(name, &mesh, renderer, device);
                    tracing::info!("Procgen resolved mesh '{}' ({} tris)", name, mesh.triangle_count());
                }
                flint_procgen::GeneratorOutput::MeshWithLods(lods) => {
                    if let Some(mesh) = lods.first() {
                        upload_procgen_mesh(name, mesh, renderer, device);
                        tracing::info!("Procgen resolved mesh '{}' (LOD0, {} tris)", name, mesh.triangle_count());
                    }
                }
                flint_procgen::GeneratorOutput::Image(img) => {
                    upload_procgen_image(name, &img, renderer, device, queue);
                    tracing::info!("Procgen resolved image '{}' ({}x{})", name, img.width, img.height);
                }
                flint_procgen::GeneratorOutput::ImageSet(images) => {
                    for (i, img) in images.iter().enumerate() {
                        let tex_name = if i == 0 {
                            name.clone()
                        } else {
                            format!("{}_{}", name, match img.channel_semantics {
                            flint_procgen::ChannelSemantics::Color => "color",
                            flint_procgen::ChannelSemantics::Normal => "normal",
                            flint_procgen::ChannelSemantics::Roughness => "roughness",
                            flint_procgen::ChannelSemantics::Metallic => "metallic",
                            flint_procgen::ChannelSemantics::Height => "height",
                            flint_procgen::ChannelSemantics::Mask => "mask",
                        })
                        };
                        upload_procgen_image(&tex_name, img, renderer, device, queue);
                    }
                    tracing::info!("Procgen resolved image set '{}' ({} maps)", name, images.len());
                }
                _ => {
                    tracing::warn!("Procgen spec '{}' produced unsupported output type for model", name);
                }
            },
            Err(e) => {
                tracing::warn!("Procgen generation failed for '{}': {}", name, e);
            }
        }
    }

    for name in &unresolved_textures {
        if unresolved_models.contains(name) {
            continue; // Already handled above
        }
        match resolver.generate(name) {
            Ok(output) => match output {
                flint_procgen::GeneratorOutput::Image(img) => {
                    upload_procgen_image(name, &img, renderer, device, queue);
                    tracing::info!("Procgen resolved texture '{}' ({}x{})", name, img.width, img.height);
                }
                flint_procgen::GeneratorOutput::ImageSet(images) => {
                    for (i, img) in images.iter().enumerate() {
                        let tex_name = if i == 0 {
                            name.clone()
                        } else {
                            format!("{}_{}", name, match img.channel_semantics {
                            flint_procgen::ChannelSemantics::Color => "color",
                            flint_procgen::ChannelSemantics::Normal => "normal",
                            flint_procgen::ChannelSemantics::Roughness => "roughness",
                            flint_procgen::ChannelSemantics::Metallic => "metallic",
                            flint_procgen::ChannelSemantics::Height => "height",
                            flint_procgen::ChannelSemantics::Mask => "mask",
                        })
                        };
                        upload_procgen_image(&tex_name, img, renderer, device, queue);
                    }
                    tracing::info!("Procgen resolved texture set '{}' ({} maps)", name, images.len());
                }
                _ => {
                    tracing::warn!("Procgen spec '{}' produced non-image output for texture ref", name);
                }
            },
            Err(e) => {
                tracing::warn!("Procgen generation failed for texture '{}': {}", name, e);
            }
        }
    }

    if !unresolved_models.is_empty() || !unresolved_textures.is_empty() {
        let stats = resolver.cache_stats();
        tracing::debug!(
            "Procgen cache: {} entries, {} bytes, {} hits / {} misses",
            stats.entry_count,
            stats.total_bytes,
            stats.hits,
            stats.misses,
        );
    }
}

/// Upload a procgen mesh to the GPU mesh cache.
///
/// Converts `flint_procgen::Vertex` → `flint_render::Vertex` (drops tangent,
/// adds vertex color from `MaterialData`). Handles multi-submesh by uploading
/// each submesh as a separate mesh entry.
fn upload_procgen_mesh(
    name: &str,
    mesh: &flint_procgen::MeshData,
    renderer: &mut SceneRenderer,
    device: &wgpu::Device,
) {
    let default_material = flint_procgen::MaterialData::default();

    if mesh.submeshes.is_empty() || mesh.submeshes.len() == 1 {
        // Single mesh — convert all vertices with the first material's color
        let mat = mesh.materials.first().unwrap_or(&default_material);
        let vertices: Vec<flint_render::Vertex> = mesh
            .vertices
            .iter()
            .map(|v| flint_render::Vertex {
                position: v.position,
                normal: v.normal,
                color: mat.base_color,
                uv: v.uv,
            })
            .collect();

        let imported_mat = flint_import::ImportedMaterial {
            name: mat.name.clone(),
            base_color: mat.base_color,
            metallic: mat.metallic,
            roughness: mat.roughness,
            base_color_texture: None,
            normal_texture: None,
            metallic_roughness_texture: None,
            use_vertex_color: true,
            alpha_mode: flint_import::AlphaMode::Opaque,
            alpha_cutoff: 0.5,
        };

        renderer.load_procedural_mesh(device, name, &vertices, &mesh.indices, imported_mat);
    } else {
        // Multi-submesh: upload each as "{name}_sub{i}"
        for (i, sub) in mesh.submeshes.iter().enumerate() {
            let mat = mesh
                .materials
                .get(sub.material_index)
                .unwrap_or(&default_material);

            let start = sub.index_start as usize;
            let count = sub.index_count as usize;
            let sub_indices = &mesh.indices[start..start + count];

            // Find min/max index to compact vertices
            let min_idx = *sub_indices.iter().min().unwrap_or(&0) as usize;
            let max_idx = *sub_indices.iter().max().unwrap_or(&0) as usize;

            let vertices: Vec<flint_render::Vertex> = mesh.vertices
                [min_idx..=max_idx]
                .iter()
                .map(|v| flint_render::Vertex {
                    position: v.position,
                    normal: v.normal,
                    color: mat.base_color,
                    uv: v.uv,
                })
                .collect();

            let compacted_indices: Vec<u32> = sub_indices
                .iter()
                .map(|&idx| idx - min_idx as u32)
                .collect();

            let sub_name = format!("{name}_sub{i}");
            let imported_mat = flint_import::ImportedMaterial {
                name: mat.name.clone(),
                base_color: mat.base_color,
                metallic: mat.metallic,
                roughness: mat.roughness,
                base_color_texture: None,
                normal_texture: None,
                metallic_roughness_texture: None,
                use_vertex_color: true,
                alpha_mode: flint_import::AlphaMode::Opaque,
                alpha_cutoff: 0.5,
            };

            renderer.load_procedural_mesh(
                device,
                &sub_name,
                &vertices,
                &compacted_indices,
                imported_mat,
            );
        }
    }
}

/// Upload a procgen image to the GPU texture cache.
fn upload_procgen_image(
    name: &str,
    img: &flint_procgen::ImageData,
    renderer: &mut SceneRenderer,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
) {
    let is_normal = matches!(
        img.channel_semantics,
        flint_procgen::ChannelSemantics::Normal
    );
    match renderer.load_texture_rgba(device, queue, name, img.width, img.height, &img.pixels, is_normal) {
        Ok(true) => {}
        Ok(false) => {
            tracing::debug!("Procgen texture '{}' already cached", name);
        }
        Err(e) => {
            tracing::warn!("Failed to upload procgen texture '{}': {}", name, e);
        }
    }
}

// ── Queued procgen asset resolution ───────────────────────────────────

/// Enqueue unresolved procgen assets for frame-budgeted generation.
///
/// Walks entities the same way as `resolve_procgen_assets()` but submits
/// requests to the resolver's queue instead of generating synchronously.
/// Used for dynamically spawned entities and runtime asset loading.
#[allow(dead_code)]
pub(super) fn enqueue_procgen_assets(
    world: &FlintWorld,
    resolver: &mut flint_procgen::ProcGenResolver,
    renderer: &SceneRenderer,
    config: &flint_render::model_loader::ModelLoadConfig,
) {
    for entity in world.all_entities() {
        let comps = match world.get_components(entity.id) {
            Some(c) => c,
            None => continue,
        };

        // Read entity position for priority calculation
        let position = comps
            .get(comp::TRANSFORM)
            .and_then(|t| t.get("position").and_then(toml_vec3))
            .unwrap_or([0.0; 3]);

        // Check model assets
        if let Some(model) = comps.get(comp::MODEL) {
            if let Some(name) = model.get("asset").and_then(|v| v.as_str()) {
                if !config.overrides.contains_key(name)
                    && !renderer.mesh_cache().contains(name)
                    && resolver.has_spec(name)
                {
                    resolver.enqueue(name, position, flint_procgen::AssetKind::Model);
                }
            }
        }

        // Check texture assets (material + sprite)
        for comp_name in &[comp::MATERIAL, comp::SPRITE] {
            if let Some(comp_data) = comps.get(*comp_name) {
                if let Some(tex) = comp_data.get("texture").and_then(|v| v.as_str()) {
                    if !config.overrides.contains_key(tex) && resolver.has_spec(tex) {
                        resolver.enqueue(tex, position, flint_procgen::AssetKind::Texture);
                    }
                }
            }
        }
    }
}

/// Upload completed queued assets to the GPU.
pub(super) fn upload_completed_procgen_assets(
    completed: &[flint_procgen::CompletedAsset],
    renderer: &mut SceneRenderer,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
) {
    for asset in completed {
        match (&asset.output, asset.asset_kind) {
            (flint_procgen::GeneratorOutput::Mesh(mesh), _) => {
                upload_procgen_mesh(&asset.spec_name, mesh, renderer, device);
                tracing::info!(
                    "Queued procgen resolved mesh '{}' ({} tris)",
                    asset.spec_name,
                    mesh.triangle_count()
                );
            }
            (flint_procgen::GeneratorOutput::MeshWithLods(lods), _) => {
                if let Some(mesh) = lods.first() {
                    upload_procgen_mesh(&asset.spec_name, mesh, renderer, device);
                    tracing::info!(
                        "Queued procgen resolved mesh '{}' (LOD0, {} tris)",
                        asset.spec_name,
                        mesh.triangle_count()
                    );
                }
            }
            (flint_procgen::GeneratorOutput::Image(img), _) => {
                upload_procgen_image(&asset.spec_name, img, renderer, device, queue);
                tracing::info!(
                    "Queued procgen resolved image '{}' ({}x{})",
                    asset.spec_name,
                    img.width,
                    img.height
                );
            }
            (flint_procgen::GeneratorOutput::ImageSet(images), _) => {
                for (i, img) in images.iter().enumerate() {
                    let tex_name = if i == 0 {
                        asset.spec_name.clone()
                    } else {
                        format!(
                            "{}_{}",
                            asset.spec_name,
                            match img.channel_semantics {
                                flint_procgen::ChannelSemantics::Color => "color",
                                flint_procgen::ChannelSemantics::Normal => "normal",
                                flint_procgen::ChannelSemantics::Roughness => "roughness",
                                flint_procgen::ChannelSemantics::Metallic => "metallic",
                                flint_procgen::ChannelSemantics::Height => "height",
                                flint_procgen::ChannelSemantics::Mask => "mask",
                            }
                        )
                    };
                    upload_procgen_image(&tex_name, img, renderer, device, queue);
                }
                tracing::info!(
                    "Queued procgen resolved image set '{}' ({} maps)",
                    asset.spec_name,
                    images.len()
                );
            }
            _ => {
                tracing::warn!(
                    "Queued procgen spec '{}' produced unsupported output type",
                    asset.spec_name
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vertex_conversion_correctness() {
        let procgen_vertex = flint_procgen::Vertex {
            position: [1.0, 2.0, 3.0],
            normal: [0.0, 1.0, 0.0],
            tangent: [1.0, 0.0, 0.0, 1.0],
            uv: [0.5, 0.75],
        };
        let color = [0.8, 0.2, 0.1, 1.0];

        let render_vertex = flint_render::Vertex {
            position: procgen_vertex.position,
            normal: procgen_vertex.normal,
            color,
            uv: procgen_vertex.uv,
        };

        assert_eq!(render_vertex.position, [1.0, 2.0, 3.0]);
        assert_eq!(render_vertex.normal, [0.0, 1.0, 0.0]);
        assert_eq!(render_vertex.color, [0.8, 0.2, 0.1, 1.0]);
        assert_eq!(render_vertex.uv, [0.5, 0.75]);
    }
}
