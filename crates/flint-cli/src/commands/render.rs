//! Headless scene-to-PNG render command

use anyhow::{Context, Result};
use flint_core::Vec3;
use flint_import::import_gltf;
use flint_render::{Camera, DebugMode, HeadlessContext, SceneRenderer};
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
    pub shadows: bool,
    pub shadow_resolution: u32,
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
    let (world, scene_file) =
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
    let mut renderer = SceneRenderer::new_headless(&ctx.device, &ctx.queue, ctx.format);
    if args.no_grid {
        renderer.disable_grid();
    }

    // Load models from the scene
    let scene_dir = Path::new(&args.scene)
        .parent()
        .unwrap_or_else(|| Path::new("."));

    for entity in world.all_entities() {
        let model_asset = world
            .get_components(entity.id)
            .and_then(|components| components.get("model").cloned())
            .and_then(|model| {
                model
                    .get("asset")
                    .and_then(|v| v.as_str().map(String::from))
            });

        if let Some(asset_name) = model_asset {
            if renderer.mesh_cache().contains(&asset_name) {
                continue;
            }

            let model_path = scene_dir.join("models").join(format!("{}.glb", asset_name));

            if model_path.exists() {
                match import_gltf(&model_path) {
                    Ok(import_result) => {
                        println!(
                            "Loaded model: {} ({} meshes, {} materials)",
                            asset_name,
                            import_result.meshes.len(),
                            import_result.materials.len()
                        );
                        renderer.load_model(&ctx.device, &ctx.queue, &asset_name, &import_result);
                    }
                    Err(e) => {
                        eprintln!("Failed to load model '{}': {:?}", asset_name, e);
                    }
                }
            } else {
                eprintln!(
                    "Model file not found: {} (tried {})",
                    asset_name,
                    model_path.display()
                );
            }
        }
    }

    // Load texture files referenced by material components
    {
        let mut loaded: std::collections::HashSet<String> = std::collections::HashSet::new();

        for entity in world.all_entities() {
            let texture_name = world
                .get_components(entity.id)
                .and_then(|components| components.get("material").cloned())
                .and_then(|material| {
                    material
                        .get("texture")
                        .and_then(|v| v.as_str().map(String::from))
                });

            if let Some(tex_name) = texture_name {
                if loaded.contains(&tex_name) {
                    continue;
                }
                loaded.insert(tex_name.clone());

                let tex_path = scene_dir.join(&tex_name);
                if tex_path.exists() {
                    match renderer.load_texture_file(&ctx.device, &ctx.queue, &tex_name, &tex_path) {
                        Ok(true) => {
                            println!("Loaded texture: {}", tex_name);
                        }
                        Ok(false) => {}
                        Err(e) => {
                            eprintln!("Failed to load texture '{}': {}", tex_name, e);
                        }
                    }
                } else {
                    eprintln!(
                        "Texture file not found: {} (tried {})",
                        tex_name,
                        tex_path.display()
                    );
                }
            }
        }
    }

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
    if args.shadows {
        renderer.set_shadows(true);
    }
    if args.shadow_resolution != 1024 {
        renderer.set_shadow_resolution(&ctx.device, args.shadow_resolution);
    }

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
