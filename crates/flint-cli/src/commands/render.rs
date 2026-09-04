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

#[derive(clap::Args)]
pub struct RenderArgs {
    /// Path to scene file
    pub scene: String,

    /// Output image path
    #[arg(short, long, default_value = "render.png")]
    pub output: String,

    /// Image width in pixels
    #[arg(long, default_value = "1920")]
    pub width: u32,

    /// Image height in pixels
    #[arg(long, default_value = "1080")]
    pub height: u32,

    /// Paths to schemas directories (can specify multiple)
    #[arg(long, default_value = "schemas", action = clap::ArgAction::Append)]
    pub schemas: Vec<String>,

    /// Camera orbit distance
    #[arg(long)]
    pub distance: Option<f32>,

    /// Camera horizontal angle in degrees
    #[arg(long)]
    pub yaw: Option<f32>,

    /// Camera vertical angle in degrees
    #[arg(long)]
    pub pitch: Option<f32>,

    /// Camera look-at point (comma-separated x,y,z)
    #[arg(long, value_parser = crate::commands::common_args::parse_vec3)]
    pub target: Option<[f32; 3]>,

    /// Field of view in degrees
    #[arg(long)]
    pub fov: Option<f32>,

    /// Disable ground grid
    #[arg(long)]
    pub no_grid: bool,

    /// Debug visualization mode
    #[arg(long, value_parser = crate::commands::common_args::parse_debug_mode)]
    pub debug_mode: Option<String>,

    /// Enable wireframe overlay on solid geometry
    #[arg(long)]
    pub wireframe_overlay: bool,

    /// Show face-normal direction arrows
    #[arg(long)]
    pub show_normals: bool,

    /// Disable tone mapping for raw linear output
    #[arg(long)]
    pub no_tonemapping: bool,

    /// Disable shadow mapping
    #[arg(long)]
    pub no_shadows: bool,

    /// Shadow map resolution per cascade (default: 2048, the renderer's
    /// construction default)
    #[arg(long, default_value = "2048")]
    pub shadow_resolution: u32,

    /// Disable post-processing (bloom, vignette, tonemapping in composite pass)
    #[arg(long)]
    pub no_postprocess: bool,

    /// MSAA sample count for the scene passes: 1 (off) or 4 (ADR 0058).
    /// Default 1 keeps headless pixel gates single-sample.
    #[arg(long, default_value = "1")]
    pub msaa: u32,

    /// Bloom intensity (enables bloom; default: 0.04)
    #[arg(long)]
    pub bloom_intensity: Option<f32>,

    /// Bloom brightness threshold (default: 1.0)
    #[arg(long)]
    pub bloom_threshold: Option<f32>,

    /// Exposure multiplier (default: 1.0)
    #[arg(long)]
    pub exposure: Option<f32>,

    /// SSAO sample radius (default: 0.5)
    #[arg(long)]
    pub ssao_radius: Option<f32>,

    /// SSAO intensity multiplier (default: 1.0, 0 = disabled)
    #[arg(long)]
    pub ssao_intensity: Option<f32>,

    /// SSAO hemisphere samples per pixel, 1-64 (default: 64; the kernel is
    /// strided so lower counts keep full radius coverage)
    #[arg(long)]
    pub ssao_samples: Option<u32>,

    /// Fog density (enables fog; default: 0.02, 0 = disabled)
    #[arg(long)]
    pub fog_density: Option<f32>,

    /// Fog color as comma-separated R,G,B (default: 0.7,0.75,0.82)
    #[arg(long, value_parser = crate::commands::common_args::parse_vec3)]
    pub fog_color: Option<[f32; 3]>,

    /// Fog height falloff (enables height fog; default: 0.1)
    #[arg(long)]
    pub fog_height_falloff: Option<f32>,

    /// Dither intensity (enables ordered dither; default: 0.03, 0 = disabled)
    #[arg(long)]
    pub dither_intensity: Option<f32>,

    /// Desaturation toward ash-grey (0 = full color, 1 = fully drained)
    #[arg(long)]
    pub desaturate: Option<f32>,

    /// Oren-Nayar diffuse blend (0 = legacy Lambert, 1 = full Oren-Nayar;
    /// sigma comes from material roughness)
    #[arg(long)]
    pub oren_nayar: Option<f32>,

    /// Charlie-sheen rim strength (0 = off; keep <= ~0.3)
    #[arg(long)]
    pub sheen_strength: Option<f32>,

    /// Charlie-sheen rim tint as comma-separated R,G,B (default: 1,1,1)
    #[arg(long, value_parser = crate::commands::common_args::parse_vec3)]
    pub sheen_color: Option<[f32; 3]>,

    /// Depth-of-field strength (0 = off/sharp, 1 = full defocus)
    #[arg(long)]
    pub dof: Option<f32>,

    /// Depth-of-field focus plane distance in world units (default: 10.0)
    #[arg(long)]
    pub dof_focus: Option<f32>,

    /// Depth-of-field focus half-width in world units (default: 5.0)
    #[arg(long)]
    pub dof_range: Option<f32>,

    /// Volumetric light density (enables god rays; default: 1.0)
    #[arg(long)]
    pub volumetric_density: Option<f32>,

    /// Volumetric ray-march sample count (default: 32)
    #[arg(long)]
    pub volumetric_samples: Option<u32>,

    /// Kuwahara filter radius (enables Kuwahara; default: 4)
    #[arg(long)]
    pub kuwahara_radius: Option<u32>,

    /// Kuwahara sector sharpness (default: 8.0)
    #[arg(long)]
    pub kuwahara_sharpness: Option<f32>,

    /// Kuwahara sector hardness (default: 8.0)
    #[arg(long)]
    pub kuwahara_hardness: Option<f32>,

    /// Kuwahara anisotropy strength (0=isotropic, 1=full; default: 1.0)
    #[arg(long)]
    pub kuwahara_anisotropy: Option<f32>,

    /// Stylized render mode (0=none, 1=matrix, 2=blood, 3=drunk, 4=tron,
    /// 5=underwater)
    #[arg(long)]
    pub render_mode: Option<u32>,

    /// Render mode blend strength 0..1 (default: 0.0)
    #[arg(long)]
    pub mode_mix: Option<f32>,

    /// Render mode params as X,Y,Z,W. Tears 1-4: mask scale, mask style,
    /// rate, spare. Underwater 5: eye depth m, sea energy, daylight, biolum
    #[arg(long, value_parser = crate::commands::common_args::parse_vec4)]
    pub mode_params: Option<[f32; 4]>,

    /// Film grain intensity (0 = off; ~0.02-0.05 is subtle)
    #[arg(long)]
    pub film_grain: Option<f32>,

    /// Post time in seconds for grain/mode animation (default 0.0 —
    /// headless renders are deterministic; two renders at the same value
    /// are identical, different values differ)
    #[arg(long)]
    pub grain_time: Option<f32>,

    /// Simulate particle emitters and effects for N seconds at a fixed
    /// 1/60 s step before capturing (deterministic: same value, same
    /// pixels). Default: no particles, so existing snapshots are unchanged
    #[arg(long)]
    pub particle_time: Option<f32>,

    /// Color grade lift as R,G,B (per-channel add post-ACES; neutral 0,0,0)
    #[arg(long, value_parser = crate::commands::common_args::parse_vec3)]
    pub grade_lift: Option<[f32; 3]>,

    /// Color grade gamma as R,G,B (per-channel curve; neutral 1,1,1)
    #[arg(long, value_parser = crate::commands::common_args::parse_vec3)]
    pub grade_gamma: Option<[f32; 3]>,

    /// Color grade gain as R,G,B (per-channel multiply; neutral 1,1,1)
    #[arg(long, value_parser = crate::commands::common_args::parse_vec3)]
    pub grade_gain: Option<[f32; 3]>,

    /// Enable the FXAA anti-aliasing pass
    #[arg(long)]
    pub fxaa: bool,
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
            sample_count: args.msaa,
        },
    );

    // Scene-authored hemisphere ambient + diffuse wrap (absent = renderer default)
    if let Some(env) = &scene_file.environment {
        if env.ambient_sky.is_some() || env.ambient_ground.is_some() {
            renderer.set_ambient(
                env.ambient_sky
                    .unwrap_or(flint_render::LightUniforms::DEFAULT_AMBIENT_SKY),
                env.ambient_ground
                    .unwrap_or(flint_render::LightUniforms::DEFAULT_AMBIENT_GROUND),
            );
        }
        if let Some(wrap) = env.diffuse_wrap {
            renderer.set_diffuse_wrap(wrap);
        }
        if let Some(oren) = env.oren_nayar {
            renderer.set_oren_nayar(oren);
        }
        if let Some(strength) = env.sheen_strength {
            renderer.set_sheen(env.sheen_color.unwrap_or([1.0; 3]), strength);
        }
    }
    // CLI overrides win over scene-authored values
    if let Some(oren) = args.oren_nayar {
        renderer.set_oren_nayar(oren);
    }
    if let Some(strength) = args.sheen_strength {
        renderer.set_sheen(args.sheen_color.unwrap_or([1.0; 3]), strength);
    }

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
            "wireframe-overlay" => DebugMode::WireframeOverlay,
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
        renderer.set_debug_mode(DebugMode::WireframeOverlay);
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
    // The renderer constructs at 2048; only recreate the pass when the flag
    // actually deviates. Historically the flag defaulted to 1024 but the
    // != 1024 guard meant an explicit 1024 was silently ignored — with the
    // texel size now uploaded per-resolution (ADR 0049) the flag is real.
    if args.shadow_resolution != 2048 {
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
        if let Some(samples) = args.ssao_samples {
            pp_config.ssao_samples = samples.clamp(1, 64);
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
        if let Some(desaturate) = args.desaturate {
            pp_config.desaturate = desaturate;
        }
        if let Some(strength) = args.dof {
            pp_config.dof_strength = strength;
        }
        if let Some(distance) = args.dof_focus {
            pp_config.dof_focus_distance = distance;
        }
        if let Some(range) = args.dof_range {
            pp_config.dof_focus_range = range;
        }
        if let Some(mode) = args.render_mode {
            pp_config.render_mode = mode;
            // A mode with no explicit mix defaults to fully torn through.
            pp_config.mode_mix = args.mode_mix.unwrap_or(1.0);
        }
        if let Some(mix) = args.mode_mix {
            pp_config.mode_mix = mix;
        }
        if let Some(params) = args.mode_params {
            pp_config.mode_params = params;
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
        if let Some(grain) = args.film_grain {
            pp_config.film_grain = grain;
        }
        if let Some(lift) = args.grade_lift {
            pp_config.grade_lift = lift;
        }
        if let Some(gamma) = args.grade_gamma {
            pp_config.grade_gamma = gamma;
        }
        if let Some(gain) = args.grade_gain {
            pp_config.grade_gain = gain;
        }
        if args.fxaa {
            pp_config.fxaa_enabled = true;
        }

        renderer.set_post_process_config(pp_config);
        renderer.ensure_kuwahara_resources(&ctx.device, &ctx.queue);
        renderer.ensure_fxaa_resources(&ctx.device);
    }

    // Post time for grain/mode animation. Headless renders leave this 0.0 so
    // PNG snapshots stay deterministic; --grain-time pins an explicit frame.
    if let Some(t) = args.grain_time {
        renderer.ocean_time = t as f64;
    }

    // Load terrain (if any)
    load_terrain_for_render(&world, &args.scene, &ctx.device, &ctx.queue, &mut renderer);

    // Set scene_dir for font/texture resolution and viewport params for screen anchoring
    renderer.scene_dir = Path::new(&args.scene).parent().map(|p| p.to_path_buf());
    renderer.ortho_height = camera.ortho_height;
    renderer.aspect_ratio = camera.aspect;

    renderer.update_from_world(&world, &ctx.device);

    // Particles (ADR 0068): fixed-step, fixed-seed simulation so a snapshot
    // at a given --particle-time is reproducible run to run.
    if let Some(seconds) = args.particle_time {
        use flint_runtime::RuntimeSystem;
        let mut particles = flint_particles::ParticleSystem::new();
        flint_particles::load_particle_effects_from_world(&args.scene, &mut particles);
        particles
            .initialize(&mut world)
            .context("Failed to initialize particles")?;
        let dirs = flint_particles::texture_search_dirs(&args.scene);
        flint_render::load_particle_textures(
            &mut renderer,
            &ctx.device,
            &ctx.queue,
            &particles.sync,
            &dirs,
        );
        particles.simulate_to(&world, seconds.max(0.0), 1.0 / 60.0);
        particles.pack(Some(camera.position_array()));
        renderer.update_particles_from(&ctx.device, &ctx.queue, &particles.sync);
        println!(
            "Particles: {} alive across {} emitter(s) at t = {:.2}s",
            particles.sync.total_alive(),
            particles.sync.emitter_count(),
            seconds
        );
    }

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
            grass: None,
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

        // Load grass if enabled
        let grass_config = {
            let mut gc = flint_terrain::GrassConfig::default();
            if let Some(enabled) = terrain_comp.get("grass.enabled") {
                if enabled.as_bool().unwrap_or(false) {
                    gc.enabled = true;
                    gc.density = get_f32("grass.density", gc.density);
                    gc.max_distance = get_f32("grass.max_distance", gc.max_distance);
                    gc.fade_start = get_f32("grass.fade_start", gc.fade_start);
                    gc.blade_width = get_f32("grass.blade_width", gc.blade_width);
                    gc.blade_height = get_f32("grass.blade_height", gc.blade_height);
                    gc.height_variation = get_f32("grass.height_variation", gc.height_variation);
                    gc.wind_speed = get_f32("grass.wind_speed", gc.wind_speed);
                    gc.wind_strength = get_f32("grass.wind_strength", gc.wind_strength);
                    gc.bend_radius = get_f32("grass.bend_radius", gc.bend_radius);
                    gc.bend_strength = get_f32("grass.bend_strength", gc.bend_strength);
                    gc.density_threshold = get_f32("grass.density_threshold", gc.density_threshold);
                    gc.density_layer =
                        get_i32("grass.density_layer", gc.density_layer as i32) as u32;
                    gc.dry_amount = get_f32("grass.dry_amount", gc.dry_amount);

                    if let Some(v) = terrain_comp.get("grass.color_base").and_then(toml_vec3) {
                        gc.color_base = v;
                    }
                    if let Some(v) = terrain_comp.get("grass.color_tip").and_then(toml_vec3) {
                        gc.color_tip = v;
                    }
                    if let Some(v) = terrain_comp.get("grass.color_dry").and_then(toml_vec3) {
                        gc.color_dry = v;
                    }
                    if let Some(v) = terrain_comp.get("grass.wind_direction").and_then(toml_vec3) {
                        gc.wind_direction = v;
                    }
                }
            }
            gc
        };

        if grass_config.enabled {
            let hm_data = heightmap.clone_heights();
            let hm_w = heightmap.width;
            let hm_d = heightmap.depth;

            let splat_path = {
                let p = scene_dir.join(&config.splat_map_path);
                if p.exists() {
                    p
                } else {
                    scene_dir
                        .parent()
                        .map(|pp| pp.join(&config.splat_map_path))
                        .filter(|pp| pp.exists())
                        .unwrap_or(p)
                }
            };

            let offset = [
                transform.position.x,
                transform.position.y,
                transform.position.z,
            ];

            if let Ok(splat_img) = image::open(&splat_path) {
                let splat_rgba = splat_img.to_rgba8();
                let (sw, sh) = splat_rgba.dimensions();

                renderer.load_grass(
                    device,
                    queue,
                    &grass_config,
                    &hm_data,
                    hm_w,
                    hm_d,
                    splat_rgba.as_raw(),
                    sw,
                    sh,
                    offset,
                    config.width,
                    config.depth,
                    config.height_scale,
                );

                println!(
                    "[grass] Enabled: density={}, max_dist={}",
                    grass_config.density, grass_config.max_distance
                );
            } else {
                println!("[grass] Warning: splat map not found at {:?}", splat_path);
            }
        }

        println!(
            "[terrain] Loaded terrain: {}x{} heightmap, {} chunks",
            heightmap.width,
            heightmap.depth,
            terrain.chunks.len()
        );
        break; // Only one terrain for now
    }
}
