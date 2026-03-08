//! Headless scene-to-PNG render command

use anyhow::{Context, Result};
use flint_core::components as comp;
use flint_core::toml_util::{toml_f32, toml_vec3};
use flint_core::Vec3;
use flint_player::spline_gen;
use flint_render::model_loader::{self, ModelLoadConfig};
use flint_render::{Camera, DebugMode, HeadlessContext, RendererConfig, SceneRenderer};
use flint_scene::load_scene;
use flint_schema::SchemaRegistry;
use std::path::Path;

pub struct RenderArgs {
    pub scene: String,
    pub output: String,
    pub width: u32,
    pub height: u32,
    pub schemas: Vec<String>,
    pub distance: Option<f32>,
    pub yaw: Option<f32>,
    pub pitch: Option<f32>,
    pub target: Option<[f32; 3]>,
    pub fov: Option<f32>,
    pub no_grid: bool,
    pub debug_mode: Option<String>,
    pub wireframe_overlay: bool,
    pub show_normals: bool,
    pub no_tonemapping: bool,
    pub no_shadows: bool,
    pub shadow_resolution: u32,
    pub no_postprocess: bool,
    pub bloom_intensity: Option<f32>,
    pub bloom_threshold: Option<f32>,
    pub exposure: Option<f32>,
    pub ssao_radius: Option<f32>,
    pub ssao_intensity: Option<f32>,
    pub fog_density: Option<f32>,
    pub fog_color: Option<[f32; 3]>,
    pub fog_height_falloff: Option<f32>,
    pub dither_intensity: Option<f32>,
    pub volumetric_density: Option<f32>,
    pub volumetric_samples: Option<u32>,
    pub kuwahara_radius: Option<u32>,
    pub kuwahara_sharpness: Option<f32>,
    pub kuwahara_hardness: Option<f32>,
    pub kuwahara_anisotropy: Option<f32>,
}

pub fn run(args: RenderArgs) -> Result<()> {
    // Merge explicit schemas with auto-discovered dirs from scene path
    let mut all_schemas = args.schemas.clone();
    for dir in flint_schema::discover_schema_dirs(&args.scene) {
        let s = dir.to_string_lossy().into_owned();
        if !all_schemas.contains(&s) {
            all_schemas.push(s);
        }
    }

    // Load schemas from all directories
    let existing: Vec<&str> = all_schemas
        .iter()
        .map(|s| s.as_str())
        .filter(|p| Path::new(p).exists())
        .collect();
    let registry = if !existing.is_empty() {
        SchemaRegistry::load_from_directories(&existing).context("Failed to load schemas")?
    } else {
        println!("Warning: No schemas directories found");
        SchemaRegistry::new()
    };

    // Load scene
    let (mut world, scene_file) =
        load_scene(&args.scene, &registry).context("Failed to load scene")?;
    println!("Loaded scene: {}", scene_file.scene.name);
    println!("Entities: {}", world.entity_count());

    // Create headless context
    let ctx = pollster::block_on(HeadlessContext::new(args.width, args.height))
        .context("Failed to create headless render context")?;

    // Configure camera — scene-level first, then CLI overrides take precedence
    let mut camera = Camera::new();
    camera.aspect = ctx.aspect_ratio();

    // Apply scene-level camera configuration
    let mut scene_set_position = false;
    if let Some(cam_def) = &scene_file.camera {
        if cam_def.projection == "orthographic" {
            camera.orthographic = true;
            if cam_def.ortho_height > 0.0 {
                camera.ortho_height = cam_def.ortho_height;
            }
        }
        if let Some(pos) = cam_def.position {
            camera.position = Vec3::new(pos[0], pos[1], pos[2]);
            scene_set_position = true;
        }
        if let Some(target) = cam_def.target {
            camera.target = Vec3::new(target[0], target[1], target[2]);
        }
        if let Some(fov) = cam_def.fov {
            camera.fov = fov;
        }
        if let Some(near) = cam_def.near {
            camera.near = near;
        }
        if let Some(far) = cam_def.far {
            camera.far = far;
        }
    }

    // Derive orbit parameters from position/target so update_orbit() is consistent
    if scene_set_position {
        let dir = camera.position - camera.target;
        camera.distance = dir.length();
        if camera.distance > 0.001 {
            let n = dir * (1.0 / camera.distance);
            camera.pitch = n.y.asin();
            camera.yaw = n.x.atan2(n.z);
        }
    }

    // CLI overrides take precedence
    if let Some(d) = args.distance {
        camera.distance = d;
    }
    if let Some(y) = args.yaw {
        camera.yaw = y.to_radians();
    }
    if let Some(p) = args.pitch {
        camera.pitch = p.to_radians();
    }
    if let Some(t) = args.target {
        camera.target = Vec3::new(t[0], t[1], t[2]);
    }
    if let Some(f) = args.fov {
        camera.fov = f;
    }
    camera.update_orbit();

    // Create scene renderer
    let mut renderer = SceneRenderer::new_headless(
        &ctx.device,
        &ctx.queue,
        ctx.format,
        ctx.width,
        ctx.height,
        RendererConfig {
            show_grid: !args.no_grid,
        },
    );

    // Load models and textures from the scene
    let config = ModelLoadConfig::from_scene_path(&args.scene);
    model_loader::load_models_from_world(
        &mut world,
        &mut renderer,
        &ctx.device,
        &ctx.queue,
        &config,
    );

    // Generate procedural geometry from spline + spline_mesh entities
    spline_gen::load_splines(&args.scene, &mut world, &mut renderer, None, &ctx.device);

    // Apply debug state
    if let Some(mode_str) = &args.debug_mode {
        let mode = match mode_str.as_str() {
            "wireframe" => DebugMode::WireframeOnly,
            "normals" => DebugMode::Normals,
            "depth" => DebugMode::Depth,
            "uv" => DebugMode::UvChecker,
            "unlit" => DebugMode::Unlit,
            "metalrough" => DebugMode::MetallicRoughness,
            _ => DebugMode::Pbr,
        };
        renderer.set_debug_mode(mode);
    }
    if args.wireframe_overlay {
        renderer.toggle_wireframe_overlay();
    }
    if args.show_normals {
        renderer.toggle_normal_arrows();
    }
    if args.no_tonemapping {
        renderer.set_tonemapping(false);
    }
    if args.no_shadows {
        renderer.set_shadows(false);
    }
    if args.shadow_resolution != 1024 {
        renderer.set_shadow_resolution(&ctx.device, args.shadow_resolution);
    }

    // Post-processing configuration
    {
        let mut pp_config = if let Some(pp_def) = &scene_file.post_process {
            flint_player::post_process_config_from_def(pp_def)
        } else {
            flint_render::PostProcessConfig::default()
        };

        // CLI overrides
        if args.no_postprocess {
            pp_config.enabled = false;
        }
        if let Some(intensity) = args.bloom_intensity {
            pp_config.bloom_intensity = intensity;
            pp_config.bloom_enabled = true;
        }
        if let Some(threshold) = args.bloom_threshold {
            pp_config.bloom_threshold = threshold;
        }
        if let Some(exposure) = args.exposure {
            pp_config.exposure = exposure;
        }
        if let Some(radius) = args.ssao_radius {
            pp_config.ssao_radius = radius;
        }
        if let Some(intensity) = args.ssao_intensity {
            pp_config.ssao_intensity = intensity;
            if intensity <= 0.0 {
                pp_config.ssao_enabled = false;
            }
        }
        if let Some(density) = args.fog_density {
            pp_config.fog_density = density;
            pp_config.fog_enabled = density > 0.0;
        }
        if let Some(color) = args.fog_color {
            pp_config.fog_color = color;
        }
        if let Some(falloff) = args.fog_height_falloff {
            pp_config.fog_height_falloff = falloff;
            pp_config.fog_height_enabled = true;
        }
        if let Some(intensity) = args.dither_intensity {
            pp_config.dither_intensity = intensity;
            pp_config.dither_enabled = intensity > 0.0;
        }
        if let Some(density) = args.volumetric_density {
            pp_config.volumetric_density = density;
            pp_config.volumetric_enabled = density > 0.0;
        }
        if let Some(samples) = args.volumetric_samples {
            pp_config.volumetric_samples = samples;
        }
        if let Some(radius) = args.kuwahara_radius {
            pp_config.kuwahara_radius = radius;
            pp_config.kuwahara_enabled = true;
        }
        if let Some(sharpness) = args.kuwahara_sharpness {
            pp_config.kuwahara_sharpness = sharpness;
            pp_config.kuwahara_enabled = true;
        }
        if let Some(hardness) = args.kuwahara_hardness {
            pp_config.kuwahara_hardness = hardness;
            pp_config.kuwahara_enabled = true;
        }
        if let Some(anisotropy) = args.kuwahara_anisotropy {
            pp_config.kuwahara_anisotropy = anisotropy;
            pp_config.kuwahara_enabled = true;
        }

        renderer.set_post_process_config(pp_config);
        renderer.ensure_kuwahara_resources(&ctx.device, &ctx.queue, &ctx.adapter);
    }

    // Load terrain (if any)
    load_terrain_for_render(&world, &args.scene, &ctx.device, &ctx.queue, &mut renderer);

    // Set scene_dir for font/texture resolution and viewport params for screen anchoring
    renderer.scene_dir = Path::new(&args.scene).parent().map(|p| p.to_path_buf());
    renderer.ortho_height = camera.ortho_height;
    renderer.aspect_ratio = camera.aspect;

    renderer.update_from_world(&world, &ctx.device);

    // Render
    renderer.render_to(
        &ctx.device,
        &ctx.queue,
        &ctx.depth_view,
        &camera,
        &ctx.color_view,
    );

    // Read back pixels
    let pixels = pollster::block_on(ctx.read_pixels()).context("Failed to read rendered pixels")?;

    // Encode as PNG
    let img = image::RgbaImage::from_raw(args.width, args.height, pixels)
        .context("Failed to create image from pixel data")?;
    img.save(&args.output)
        .context(format!("Failed to save image to {}", args.output))?;

    println!(
        "Rendered {}x{} image to {}",
        args.width, args.height, args.output
    );

    Ok(())
}

/// Load terrain from world entities for headless rendering (no physics).
fn load_terrain_for_render(
    world: &flint_ecs::FlintWorld,
    scene_path: &str,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    renderer: &mut SceneRenderer,
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
            _ => continue,
        };

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
                tracing::warn!("[terrain] Failed to load heightmap: {}", e);
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

        let transform = world
            .get_component(entity.id, comp::TRANSFORM)
            .and_then(|t| {
                let pos = t.get("position").and_then(toml_vec3)?;
                Some(Transform {
                    position: Vec3::new(pos[0], pos[1], pos[2]),
                    ..Default::default()
                })
            })
            .unwrap_or_default();

        renderer.load_terrain(
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

        println!(
            "[terrain] Loaded terrain: {}x{} heightmap, {} chunks",
            heightmap.width,
            heightmap.depth,
            terrain.chunks.len()
        );
        break; // Only one terrain for now
    }
}
