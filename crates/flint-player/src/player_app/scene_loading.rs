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
        physics.sync.register_static_trimesh(
            entity.id,
            &mut physics.physics_world,
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
                    animation
                        .skeletal_sync
                        .add_skeleton(loaded.entity_id, skeleton);
                }
                for imported_clip in &import_result.skeletal_clips {
                    let clip = SkeletalClip::from_imported(imported_clip);
                    println!(
                        "  Skeletal clip: {} ({:.1}s, {} tracks)",
                        clip.name,
                        clip.duration,
                        clip.joint_tracks.len()
                    );
                    animation.skeletal_sync.add_clip(clip);
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
                animation.node_sync.add_clip(clip);
            }
        }
        if let Some(ref node_map) = loaded.node_map {
            animation
                .node_sync
                .register_entity(loaded.entity_id, node_map.clone());
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
                    animation.player.add_clip(clip);
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
                        animation.sprite_sync.add_clip(clip);
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
    flint_script::sync::load_scripts_from_scene(scene_path, &mut script.sync);
}
