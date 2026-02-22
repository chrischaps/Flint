//! Headless scene-to-PNG render command

use anyhow::{Context, Result};
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
}

pub fn run(args: RenderArgs) -> Result<()> {
    // Load schemas from all directories
    let existing: Vec<&str> = args.schemas.iter().map(|s| s.as_str()).filter(|p| Path::new(p).exists()).collect();
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

    // Configure camera
    let mut camera = Camera::new();
    camera.aspect = ctx.aspect_ratio();

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
        RendererConfig { show_grid: !args.no_grid },
    );

    // Load models and textures from the scene
    let config = ModelLoadConfig::from_scene_path(&args.scene);
    model_loader::load_models_from_world(&mut world, &mut renderer, &ctx.device, &ctx.queue, &config);

    // Generate procedural geometry from spline + spline_mesh entities
    spline_gen::load_splines(
        &args.scene,
        &mut world,
        &mut renderer,
        None,
        &ctx.device,
    );

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
        use flint_render::PostProcessConfig;
        let mut pp_config = PostProcessConfig::default();

        // Apply scene-level settings first
        if let Some(pp_def) = &scene_file.post_process {
            pp_config.bloom_enabled = pp_def.bloom_enabled;
            pp_config.bloom_intensity = pp_def.bloom_intensity;
            pp_config.bloom_threshold = pp_def.bloom_threshold;
            pp_config.vignette_enabled = pp_def.vignette_enabled;
            pp_config.vignette_intensity = pp_def.vignette_intensity;
            pp_config.exposure = pp_def.exposure;
            pp_config.ssao_enabled = pp_def.ssao_enabled;
            pp_config.ssao_radius = pp_def.ssao_radius;
            pp_config.ssao_intensity = pp_def.ssao_intensity;
            pp_config.fog_enabled = pp_def.fog_enabled;
            pp_config.fog_color = pp_def.fog_color;
            pp_config.fog_density = pp_def.fog_density;
            pp_config.fog_start = pp_def.fog_start;
            pp_config.fog_end = pp_def.fog_end;
            pp_config.fog_height_enabled = pp_def.fog_height_enabled;
            pp_config.fog_height_falloff = pp_def.fog_height_falloff;
            pp_config.fog_height_origin = pp_def.fog_height_origin;
        }

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

        renderer.set_post_process_config(pp_config);
    }

    // Load terrain (if any)
    load_terrain_for_render(&world, &args.scene, &ctx.device, &ctx.queue, &mut renderer);

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
        let terrain_comp = match world.get_component(entity.id, "terrain") {
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
                if pp.exists() { pp } else { p }
            } else {
                p
            }
        };

        let heightmap = match Heightmap::from_png(&hm_path) {
            Ok(hm) => hm,
            Err(e) => {
                eprintln!("[terrain] Failed to load heightmap: {}", e);
                continue;
            }
        };

        let get_f32 = |key: &str, default: f32| -> f32 {
            terrain_comp
                .get(key)
                .and_then(|v| v.as_float().or_else(|| v.as_integer().map(|i| i as f64)))
                .map(|f| f as f32)
                .unwrap_or(default)
        };

        let get_i32 = |key: &str, default: i32| -> i32 {
            terrain_comp.get(key).and_then(|v| v.as_integer()).map(|i| i as i32).unwrap_or(default)
        };

        let get_str = |key: &str| -> String {
            terrain_comp.get(key).and_then(|v| v.as_str()).unwrap_or("").to_string()
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
            .get_component(entity.id, "transform")
            .and_then(|t| {
                let arr = t.get("position")?.as_array()?;
                if arr.len() >= 3 {
                    let x = arr[0].as_float().or_else(|| arr[0].as_integer().map(|i| i as f64))? as f32;
                    let y = arr[1].as_float().or_else(|| arr[1].as_integer().map(|i| i as f64))? as f32;
                    let z = arr[2].as_float().or_else(|| arr[2].as_integer().map(|i| i as f64))? as f32;
                    Some(Transform {
                        position: Vec3::new(x, y, z),
                        ..Default::default()
                    })
                } else {
                    None
                }
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
            heightmap.width, heightmap.depth, terrain.chunks.len()
        );
        break; // Only one terrain for now
    }
}

