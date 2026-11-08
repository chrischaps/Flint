//! Headless scene-to-PNG render command, and since #216 the same at a chosen game
//! time, across a sweep of a wooze knob, or as an animated GIF of a span of time.
//!
//! A [`Stage`] is one scene loaded into one renderer. An ordinary render is one
//! stage captured once. A sweep is a stage a column, each with its own clocks,
//! since a prop's state is the whole of its history. A span is the stages captured
//! again and again as their clocks move on.
//!
//! With `--scripts` (#8) each stage also runs its scene's Rhai scripts, a tick a
//! fixed step, in the player's frame order (`flint_player::HeadlessScripts`), and
//! `--expression-timeline` feeds `expression()` from a file on that clock. Same
//! scene, same timeline, same pixels.

use super::gif_writer;
use super::render_plan::{self, Sweep, TimeSpec};
use super::render_report::{self, CaptureReport, RenderReport};
use super::render_sheet::{self, SheetText};
use anyhow::{anyhow, Context, Result};
use flint_core::components as comp;
use flint_core::toml_util::{toml_f32, toml_vec3};
use flint_core::Vec3;
use flint_player::{
    spline_gen, CameraView, ExpressionTimeline, GpuSink, HeadlessScripts, LivingRun, ScriptLook,
    WoozeClocks,
};
use flint_render::model_loader::{self, ModelLoadConfig};
use flint_render::{Camera, DebugMode, HeadlessContext, RendererConfig, SceneRenderer};
use flint_scene::load_scene;
use flint_schema::SchemaRegistry;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

/// With `--report json`, stdout is the report and everything else goes to stderr.
static HUMAN_TO_STDERR: AtomicBool = AtomicBool::new(false);

/// `println!` for the human lines; see [`HUMAN_TO_STDERR`].
macro_rules! say {
    ($($t:tt)*) => {
        if HUMAN_TO_STDERR.load(Ordering::Relaxed) {
            eprintln!($($t)*)
        } else {
            println!($($t)*)
        }
    };
}

/// How `--report` writes.
#[derive(Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum RenderReportFormat {
    /// One JSON object on stdout, with the human output moved to stderr.
    Json,
}

#[derive(clap::Args)]
pub struct RenderArgs {
    /// Path to scene file
    #[arg(required_unless_present = "list_gpus", default_value = "")]
    pub scene: String,

    /// Output image path
    #[arg(short, long, default_value = "render.png")]
    pub output: String,

    /// Image width in pixels [default: 1920; each column of a sweep, 640; a GIF, 960]
    #[arg(long)]
    pub width: Option<u32>,

    /// Image height in pixels [default: 1080; each column of a sweep, 480; a GIF, 540]
    #[arg(long)]
    pub height: Option<u32>,

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

    /// How far each drawing moves the paint, as a share of the Kuwahara radius
    /// (0 = still; needs the scene's [wooze] table)
    #[arg(long)]
    pub kuwahara_boil: Option<f32>,

    /// Ink lines over the scene, from breaks in depth and luminance
    #[arg(long)]
    pub contour: bool,

    /// Half a contour line's width, in pixels at 1080p (enables contour; default: 1.25)
    #[arg(long)]
    pub contour_width: Option<f32>,

    /// How strong an edge must be before it inks (enables contour; default: 0.08)
    #[arg(long)]
    pub contour_threshold: Option<f32>,

    /// How dark the ink goes, 0-1 (enables contour; default: 0.9)
    #[arg(long)]
    pub contour_strength: Option<f32>,

    /// How much edges in luminance count against depth (enables contour; default: 0.5)
    #[arg(long)]
    pub contour_luma: Option<f32>,

    /// The ink's colour as linear R,G,B (enables contour)
    #[arg(long, value_parser = crate::commands::common_args::parse_vec3)]
    pub contour_color: Option<[f32; 3]>,

    /// Metres at which a line has faded by half (enables contour; 0 = never; default: 30)
    #[arg(long)]
    pub contour_fade: Option<f32>,

    /// How far each drawing moves the lines (enables contour; 0 = still; needs the
    /// scene's [wooze] table)
    #[arg(long)]
    pub contour_boil: Option<f32>,

    /// Film frames each boil drawing is held: 1, 2 or 3 (default: 2)
    #[arg(long)]
    pub boil_on: Option<u32>,

    /// Seconds of the scene's wooze field to run before capturing, which picks
    /// the boil's drawing (default 0: same value, same pixels)
    #[arg(long)]
    pub boil_time: Option<f64>,

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

    /// The game time to capture: living props, the wooze field and everything else
    /// a clock drives, as the player has them at that time. Seconds (3.5), or a span
    /// (0..4) for an animated --output x.gif. Physics and animation clips don't
    /// run, and scripts only with --scripts. Same value, same pixels
    #[arg(long, value_parser = render_plan::parse_time)]
    pub time: Option<TimeSpec>,

    /// Run the scene's scripts as flint-player does, one tick a --frame-step, so
    /// what they write to the world, the wooze field and Living Props is in the
    /// picture. random() is seeded and scripts run in the order of their entities'
    /// names: same scene, same pixels. Sound, physics, the HUD and scene changes
    /// don't run, and are named on stderr when a script asks for them
    #[arg(long)]
    pub scripts: bool,

    /// An expression timeline (`{"events": [{"t": 0, ...}, ...]}`, as
    /// flint-player's flag) whose states scripts hear through expression() and
    /// on_expression(state), each once the clock reaches its t. Implies --scripts
    #[arg(long, value_name = "FILE")]
    pub expression_timeline: Option<String>,

    /// With --time: the seconds each clock tick moves, as flint-player's
    /// --frame-step, to reproduce a player run to the bit [default: 1/60]
    #[arg(long, requires = "time", value_parser = parse_frame_step)]
    pub frame_step: Option<f64>,

    /// With a --time span: frames per second of game time in the GIF
    #[arg(long, requires = "time", default_value_t = 12.0, value_parser = render_plan::parse_gif_fps)]
    pub fps: f32,

    /// Render the same moment once per value of a wooze knob, as labelled columns
    /// on one image: wooze.glide=0,0.25,0.5,1. Takes wooze.amplitude, .step, .glide,
    /// .jitter and .preset. It sets the scene's global value, and zones still apply
    #[arg(long, value_parser = render_plan::parse_sweep)]
    pub sweep: Option<Sweep>,

    /// Report what each living prop was doing when it was captured: lineage seed,
    /// pose indices and the wooze values it read
    #[arg(long, value_enum)]
    pub report: Option<RenderReportFormat>,

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
    /// GPU adapter to use (case-insensitive substring of the adapter name,
    /// e.g. "NVIDIA"). Overrides WGPU_ADAPTER_NAME; default prefers the
    /// high-performance adapter. Use --list-gpus to see names.
    #[arg(long)]
    pub gpu: Option<String>,

    /// List available GPU adapters and exit
    #[arg(long)]
    pub list_gpus: bool,
}

fn parse_frame_step(text: &str) -> Result<f64, String> {
    match text.parse::<f64>() {
        Ok(step) if step.is_finite() && step > 0.0 => Ok(step),
        _ => Err(format!(
            "'{text}' isn't a frame step: it's seconds, more than 0"
        )),
    }
}

pub fn run(args: RenderArgs) -> Result<()> {
    if args.list_gpus {
        for a in flint_render::list_adapters() {
            println!("{a}");
        }
        return Ok(());
    }
    render_plan::check_output(&args.output, args.time)?;
    if let Some(sweep) = &args.sweep {
        check_sweep(&args, sweep)?;
    }
    HUMAN_TO_STDERR.store(args.report.is_some(), Ordering::Relaxed);
    flint_render::set_gpu_override(args.gpu.clone());
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
        say!("Warning: No schemas directories found");
        SchemaRegistry::new()
    };

    // Load the scene, once for each column: a world is changed by loading it
    let columns = args.sweep.as_ref().map_or(1, |sweep| sweep.values.len());
    let mut scenes = Vec::with_capacity(columns);
    for _ in 0..columns {
        scenes.push(load_scene(&args.scene, &registry).context("Failed to load scene")?);
    }
    let scene_name = scenes[0].1.scene.name.clone();
    say!("Loaded scene: {scene_name}");
    say!("Entities: {}", scenes[0].0.entity_count());

    let animated = matches!(args.time, Some(TimeSpec::Range(..)));
    let (default_width, default_height) = render_plan::default_size(args.sweep.is_some(), animated);
    let (width, height) = (
        args.width.unwrap_or(default_width),
        args.height.unwrap_or(default_height),
    );
    let frame_step = args.frame_step.unwrap_or(render_plan::DEFAULT_FRAME_STEP);

    // Create headless context
    let ctx = pollster::block_on(HeadlessContext::new(width, height))
        .context("Failed to create headless render context")?;

    let timeline = match &args.expression_timeline {
        Some(path) => Some(ExpressionTimeline::load(path)?),
        None => None,
    };
    if let Some(timeline) = &timeline {
        say!("Expression timeline: {} states", timeline.len());
    }

    let mut stages = Vec::with_capacity(columns);
    for (column, (world, scene_file)) in scenes.into_iter().enumerate() {
        stages.push(Stage::load(
            &args,
            &ctx,
            world,
            scene_file,
            column,
            timeline.clone(),
        )?);
    }

    let times = render_plan::capture_times(args.time, args.fps);
    let mut warnings = stage_warnings(&args, &stages[0]);
    for warning in &warnings {
        eprintln!("Warning: {warning}");
    }

    let mut captures = Vec::new();
    let mut frames: Vec<image::RgbImage> = Vec::with_capacity(times.len());
    let mut still: Option<image::RgbaImage> = None;
    // Whether each pair of neighbouring columns has differed in any frame yet.
    let mut columns_differ = vec![false; columns.saturating_sub(1)];
    for &t in &times {
        let mut cells = Vec::with_capacity(columns);
        for (column, stage) in stages.iter_mut().enumerate() {
            cells.push(stage.capture(&args, &ctx, t, frame_step)?);
            if args.report.is_some() {
                captures.push(CaptureReport {
                    time: t,
                    column: args.sweep.as_ref().map(|s| s.values[column].clone()),
                    expression: stage.scripts.as_ref().and_then(|scripts| {
                        let state = scripts.expression().get("state")?.clone();
                        state.into_string().ok()
                    }),
                    props: stage
                        .living
                        .as_ref()
                        .map_or_else(Vec::new, render_report::props_report),
                });
            }
        }
        for (differ, pair) in columns_differ.iter_mut().zip(cells.windows(2)) {
            *differ |= pair[0] != pair[1];
        }
        match &args.sweep {
            Some(sweep) => {
                let cells: Vec<image::RgbImage> = cells
                    .into_iter()
                    .map(|cell| image::DynamicImage::ImageRgba8(cell).into_rgb8())
                    .collect();
                let text = sheet_text(sweep, &scene_name, t, &stages);
                frames.push(render_sheet::compose(&cells, &text));
            }
            None if animated => {
                let cell = cells.remove(0);
                frames.push(image::DynamicImage::ImageRgba8(cell).into_rgb8());
            }
            None => still = Some(cells.remove(0)),
        }
    }

    // What the scripts asked for that isn't in the picture, said once as it
    // happened, and kept for the report.
    if let Some(scripts) = &stages[0].scripts {
        warnings.extend(scripts.notes());
    }

    if let Some(sweep) = &args.sweep {
        for (i, differ) in columns_differ.iter().enumerate() {
            if !differ {
                let warning = format!(
                    "{}={} and {} are pixel-identical: nothing on screen is reading this knob \
                     at this moment, or a zone is keeping it off these values",
                    sweep.name(),
                    sweep.values[i],
                    sweep.values[i + 1]
                );
                eprintln!("Warning: {warning}");
                warnings.push(warning);
            }
        }
    }

    // Write the file
    let output = Path::new(&args.output);
    if animated {
        gif_writer::write(output, &frames, args.fps)?;
    } else if let Some(img) = still {
        img.save(&args.output)
            .context(format!("Failed to save image to {}", args.output))?;
    } else {
        frames[0]
            .save(&args.output)
            .context(format!("Failed to save image to {}", args.output))?;
    }

    if animated {
        say!(
            "Rendered {} frames of {}x{} at {} fps to {} ({:.1} MB)",
            frames.len(),
            frames[0].width(),
            frames[0].height(),
            args.fps,
            args.output,
            std::fs::metadata(output).map_or(0.0, |m| m.len() as f64 / 1e6),
        );
    } else if let Some(sheet) = frames.first() {
        say!(
            "Rendered {} columns of {width}x{height} as {}x{} to {}",
            columns,
            sheet.width(),
            sheet.height(),
            args.output
        );
    } else {
        say!("Rendered {width}x{height} image to {}", args.output);
    }

    if args.report.is_some() {
        let field = stages[0].living.as_ref().and_then(|l| l.field());
        let report = RenderReport {
            scene: scene_name,
            output: args.output.clone(),
            width,
            height,
            frame_step,
            wooze_field: field.is_some(),
            wooze_zones: field.map_or(0, |f| f.zones().len()),
            sweep: args.sweep.as_ref().map(Sweep::name),
            captures,
            still_props: still_props(&stages[0]),
            warnings,
        };
        let json = serde_json::to_string(&report).context("serialising the render report")?;
        println!("{json}");
    }
    Ok(())
}

/// Try every column's value on a field of its own, before anything is loaded. A
/// preset column may be a file beside the scene (#249), which has to read.
fn check_sweep(args: &RenderArgs, sweep: &Sweep) -> Result<()> {
    for index in 0..sweep.values.len() {
        if preset_file_of(args, index)?.is_some() {
            continue;
        }
        let mut field = flint_core::wooze::WoozeField::new(flint_core::wooze::WoozeSample::NEUTRAL);
        field
            .set_global(&sweep.table(index), 0.0, 0.0)
            .map_err(|reason| anyhow!("--sweep {}: {reason}", sweep.name()))?;
    }
    Ok(())
}

/// What a reader of this capture has to be told about what it isn't showing.
fn stage_warnings(args: &RenderArgs, stage: &Stage) -> Vec<String> {
    let mut warnings = Vec::new();
    let timed = args.time.is_some() || args.sweep.is_some();
    if !timed {
        return warnings;
    }
    if stage.living.is_none() {
        warnings.push("this scene has no living props, so only its other clocks move".to_string());
    }
    let with = |component: &str| -> Vec<String> {
        stage
            .world
            .all_entities()
            .into_iter()
            .filter(|entity| {
                stage
                    .world
                    .get_components(entity.id)
                    .is_some_and(|comps| comps.get(component).is_some())
            })
            .map(|entity| entity.name)
            .collect()
    };
    let scripts = with(comp::SCRIPT);
    if !scripts.is_empty() && stage.scripts.is_none() {
        warnings.push(format!(
            "scripts don't run without --scripts, so what {} would do to this scene isn't in it",
            scripts.join(", ")
        ));
    }
    let animated = with(comp::ANIMATOR);
    if !animated.is_empty() {
        warnings.push(format!(
            "animation clips don't run here, so {} won't match the player",
            animated.join(", ")
        ));
    }
    warnings
}

/// The entities that asked to be living props and drew as still models.
fn still_props(stage: &Stage) -> Vec<String> {
    let mut living: Vec<flint_core::EntityId> = Vec::new();
    if let Some(run) = &stage.living {
        living.extend(run.props().props().iter().map(|prop| prop.entity()));
    }
    stage
        .world
        .all_entities()
        .into_iter()
        .filter(|entity| {
            !living.contains(&entity.id)
                && stage
                    .world
                    .get_components(entity.id)
                    .is_some_and(|comps| comps.get(comp::LIVING_PROP).is_some())
        })
        .map(|entity| entity.name)
        .collect()
}

/// The preset file a `--sweep wooze.preset=...` column names, if it's a file the
/// scene can see rather than a built-in (#249).
fn preset_file_of(args: &RenderArgs, column: usize) -> Result<Option<flint_scene::WoozePreset>> {
    let Some(sweep) = args.sweep.as_ref().filter(|sweep| sweep.knob == "preset") else {
        return Ok(None);
    };
    let name = &sweep.values[column];
    match flint_scene::read_preset(Path::new(&args.scene), name) {
        None => Ok(None),
        Some(Ok(preset)) => Ok(Some(preset)),
        Some(Err(reason)) => Err(anyhow!("--sweep: preset '{name}': {reason}")),
    }
}

/// A preset's `[wooze]` knobs as the table the field takes.
fn preset_wooze_table(knobs: &flint_scene::WoozeKnobs) -> toml::Table {
    let mut table = toml::Table::new();
    for (key, value) in [
        ("amplitude", knobs.amplitude),
        ("step", knobs.step),
        ("glide", knobs.glide),
        ("jitter", knobs.jitter),
    ] {
        if let Some(value) = value {
            table.insert(key.to_string(), toml::Value::Float(f64::from(value)));
        }
    }
    table
}

/// What a sweep's sheet says, as of the moment each stage last captured.
fn sheet_text(sweep: &Sweep, scene_name: &str, t: f64, stages: &[Stage]) -> SheetText {
    let props = stages[0]
        .living
        .as_ref()
        .map_or(0, |run| run.props().props().len());
    SheetText {
        title: scene_name.to_string(),
        subtitle: format!("{}   {props} living props", sweep.name()),
        time: format!("T {t:.2}S"),
        labels: sweep
            .values
            .iter()
            .map(|value| {
                if sweep.knob == "preset" {
                    value.clone()
                } else {
                    format!("{} {value}", sweep.knob)
                }
            })
            .collect(),
        notes: sweep
            .values
            .iter()
            .zip(stages)
            .map(|(value, stage)| {
                stage.living.as_ref().map_or_else(String::new, |run| {
                    render_report::read_note(&sweep.knob, value.parse().ok(), run.props())
                })
            })
            .collect(),
    }
}

/// One scene in one renderer, with its own clocks.
struct Stage {
    world: flint_ecs::FlintWorld,
    camera: Camera,
    renderer: SceneRenderer,
    /// `None` in a scene that doesn't opt in, which then draws as it always has.
    living: Option<LivingRun>,
    /// `None` in a scene with no `[wooze]` table.
    wooze: Option<WoozeClocks>,
    /// Game seconds so far, added up step by step as the player's clock is.
    clock: f64,
    particles: Option<flint_particles::ParticleSystem>,
    /// How many fixed steps the particles have taken.
    particle_steps: u32,
    /// The scene's scripts, with `--scripts` (#8).
    scripts: Option<HeadlessScripts>,
    /// What the scripts' look was when it last went to the renderer.
    look_applied: ScriptLook,
    /// Whether this is a capture of a moment, which draws shallow wooze.
    moment: bool,
}

impl Stage {
    /// Load `world` into a new renderer, ready for its first capture. `column` is
    /// which of a sweep's values it takes.
    fn load(
        args: &RenderArgs,
        ctx: &HeadlessContext,
        mut world: flint_ecs::FlintWorld,
        scene_file: flint_scene::SceneFile,
        column: usize,
        timeline: Option<ExpressionTimeline>,
    ) -> Result<Self> {
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
        // The player draws from the authored position, and going round through the
        // orbit moves it by a rounding error, which is enough to change thousands of
        // pixels. So a capture that claims to be the player's stays where the scene
        // put it; an ordinary render keeps the pixels it has always had.
        let orbit_asked = args.distance.is_some()
            || args.yaw.is_some()
            || args.pitch.is_some()
            || args.target.is_some();
        let as_authored = scene_set_position
            && !orbit_asked
            && (args.time.is_some() || LivingRun::wanted_by(&world));
        if !as_authored {
            camera.update_orbit();
        }

        // Create scene renderer
        let mut renderer = SceneRenderer::new_headless(
            &ctx.device,
            &ctx.queue,
            ctx.format,
            ctx.width,
            ctx.height,
            RendererConfig {
                // The grid is an editor's. A capture of a moment is the game's view.
                show_grid: !args.no_grid && args.time.is_none() && args.sweep.is_none(),
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

        // Living props (#216), and the procgen assets their scene asks for, before the
        // model loader as in the player: catalog > procgen > file.
        let mut living = LivingRun::wanted_by(&world).then(|| {
            LivingRun::load(
                &args.scene,
                scene_file.living.as_ref(),
                scene_file.wooze.as_ref(),
                &mut world,
                GpuSink {
                    renderer: &mut renderer,
                    device: &ctx.device,
                    queue: &ctx.queue,
                    config: &config,
                },
            )
        });
        // A preset column names a preset file beside the scene before a built-in,
        // as the player's Load does (#249), and the file's [living] tables, species
        // and all, go over the scene's as they do there.
        let column_preset = preset_file_of(args, column)?;
        let column_knobs = match &column_preset {
            Some(preset) => Some(preset_wooze_table(&preset.wooze)),
            None => args.sweep.as_ref().map(|sweep| sweep.table(column)),
        };
        if let (Some(run), Some(knobs)) = (&mut living, &column_knobs) {
            run.change_field(knobs)
                .map_err(|reason| anyhow!("--sweep: {reason}"))?;
        }
        if let (Some(run), Some(preset)) = (&mut living, &column_preset) {
            for problem in run.apply_preset(&preset.living) {
                eprintln!("Warning: --sweep preset: {problem}");
            }
        }
        // What else reads the field: the shading boil always, and shallow wooze in a
        // capture of a moment. A plain render shows those meshes at rest (#215).
        let moment = args.time.is_some() || args.sweep.is_some();
        let mut wooze =
            WoozeClocks::of_scene(scene_file.wooze.as_ref(), column_knobs.as_ref(), moment)
                .map_err(|reason| anyhow!("--sweep: {reason}"))?;
        // As the player's first frame: no time passes, and the field is read.
        if let Some(run) = &mut living {
            run.step(0.0);
        }

        model_loader::load_models_from_world(
            &mut world,
            &mut renderer,
            &ctx.device,
            &ctx.queue,
            &config,
        );

        // Generate procedural geometry from spline + spline_mesh entities
        spline_gen::load_splines(
            &args.scene,
            &mut world,
            &mut renderer,
            None,
            &ctx.device,
            &ctx.queue,
        );

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
            if let Some(boil) = column_preset.as_ref().and_then(|preset| preset.boil) {
                let on = boil.enabled.unwrap_or(true);
                if let Some(paint) = boil.kuwahara {
                    pp_config.kuwahara_boil = if on { paint } else { 0.0 };
                }
                if let Some(ink) = boil.contour {
                    pp_config.contour_boil = if on { ink } else { 0.0 };
                }
                if let Some(cadence) = boil.on {
                    pp_config.boil_on = cadence.clamp(1, 3);
                }
            }
            if let Some(boil) = args.kuwahara_boil {
                pp_config.kuwahara_boil = boil;
            }
            if args.contour {
                pp_config.contour_enabled = true;
            }
            if let Some(width) = args.contour_width {
                pp_config.contour_width = width;
                pp_config.contour_enabled = true;
            }
            if let Some(threshold) = args.contour_threshold {
                pp_config.contour_threshold = threshold;
                pp_config.contour_enabled = true;
            }
            if let Some(strength) = args.contour_strength {
                pp_config.contour_strength = strength;
                pp_config.contour_enabled = true;
            }
            if let Some(luma) = args.contour_luma {
                pp_config.contour_luma = luma;
                pp_config.contour_enabled = true;
            }
            if let Some(color) = args.contour_color {
                pp_config.contour_color = color;
                pp_config.contour_enabled = true;
            }
            if let Some(fade) = args.contour_fade {
                pp_config.contour_fade = fade;
                pp_config.contour_enabled = true;
            }
            if let Some(boil) = args.contour_boil {
                pp_config.contour_boil = boil;
                pp_config.contour_enabled = true;
            }
            if let Some(on) = args.boil_on {
                pp_config.boil_on = on;
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

            // The shading boil's drawing (#214) comes from the field where the camera
            // is. --boil-time picks one and holds it, in one tick at the tempo of that
            // moment. Otherwise each capture stamps the drawing its clock has reached.
            let boil_wanted = pp_config.kuwahara_boil > 0.0 || pp_config.contour_boil > 0.0;
            match (&wooze, args.boil_time) {
                (Some(clocks), Some(time)) => {
                    pp_config.boil =
                        clocks.boil_at(camera.position_array(), time.max(0.0), pp_config.boil_on);
                }
                (None, _) if boil_wanted => {
                    say!("This scene has no [wooze] table, so nothing boils.");
                }
                _ => {}
            }

            renderer.set_post_process_config(pp_config);
            renderer.ensure_kuwahara_resources(&ctx.device, &ctx.queue);
            renderer.ensure_fxaa_resources(&ctx.device);
            renderer.ensure_contour_resources(&ctx.device);
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

        // Scripts (#8), as the player starts them once its models are loaded, and
        // its first frame of them: no time passes, the timeline's states at 0 are
        // heard, and what the scripts ask for is done before the clocks are read.
        let mut scripts = (args.scripts || timeline.is_some()).then(|| {
            HeadlessScripts::load(
                &args.scene,
                &mut world,
                timeline,
                (ctx.width as f32, ctx.height as f32),
            )
        });
        if let Some(scripts) = &mut scripts {
            let commands = scripts.tick(&mut world, 0.0, 0.0, view_of(&camera));
            scripts.apply(commands, living.as_mut(), &mut wooze, moment);
            follow_script_camera(&mut camera, scripts);
            if let Some(run) = &mut living {
                run.step(0.0);
            }
        }

        // Two-bone IK (ADR 0070): pose authored chains before the single frame.
        flint_animation::IkPass::new().run(&mut world);

        // Last, since the model loader expands a GLB into the entities a group holds.
        if let Some(clocks) = &mut wooze {
            let draws = clocks.step(&world, camera.position_array(), 0.0);
            renderer.set_shallow_wooze(draws.iter().copied());
        }

        Ok(Self {
            world,
            camera,
            renderer,
            living,
            wooze,
            clock: 0.0,
            particles: None,
            particle_steps: 0,
            scripts,
            look_applied: ScriptLook::default(),
            moment,
        })
    }

    /// Move the clocks on to `t` and take a picture. `t` only goes forward.
    fn capture(
        &mut self,
        args: &RenderArgs,
        ctx: &HeadlessContext,
        t: f64,
        frame_step: f64,
    ) -> Result<image::RgbaImage> {
        let (steps, rest) = render_plan::steps_within(t - self.clock, frame_step);
        for _ in 0..steps {
            self.step_clocks(frame_step);
        }
        if rest > 0.0 {
            self.step_clocks(rest);
        }
        if let Some(run) = &mut self.living {
            run.upload(&mut self.renderer, &ctx.device, &ctx.queue);
        }
        // The scripts' post-processing, as sticky as the player's.
        if let Some(scripts) = &self.scripts {
            let look = scripts.look();
            if look != self.look_applied {
                let mut config = self.renderer.post_process_config().clone();
                flint_player::apply_look(&mut config, &look);
                self.renderer.set_post_process_config(config);
                self.look_applied = look;
            }
        }
        if let (Some(clocks), None) = (&self.wooze, args.boil_time) {
            let on = self.renderer.post_process_config().boil_on;
            self.renderer
                .set_boil(clocks.boil(self.camera.position_array(), on));
        }
        if args.time.is_some() {
            // The rest of what the player keeps on this clock. Grain has its own flag.
            // Woozy grass (#252) is on the field's clock instead.
            match &self.wooze {
                Some(clocks) => clocks.grass().apply(&mut self.renderer, t),
                None => self.renderer.grass_time = t as f32,
            }
            if args.grain_time.is_none() {
                self.renderer.ocean_time = t;
            }
        }

        self.renderer.update_from_world(&self.world, &ctx.device);

        // Particles (ADR 0068): fixed-step, fixed-seed simulation so a snapshot
        // at a given time is reproducible run to run.
        let particle_time = args
            .particle_time
            .or_else(|| args.time.is_some().then_some(t as f32));
        if let Some(seconds) = particle_time {
            self.step_particles(args, ctx, seconds)?;
        }

        // Render
        self.renderer.render_to(
            &ctx.device,
            &ctx.queue,
            &ctx.depth_view,
            &self.camera,
            &ctx.color_view,
        );

        // Read back pixels
        let pixels =
            pollster::block_on(ctx.read_pixels()).context("Failed to read rendered pixels")?;
        image::RgbaImage::from_raw(ctx.width, ctx.height, pixels)
            .context("Failed to create image from pixel data")
    }

    /// One step of everything on the clock, in the order of the player's frame:
    /// scripts and what they ask for, then the wooze field and living props.
    fn step_clocks(&mut self, dt: f64) {
        self.clock += dt;
        let mut dt_living = dt;
        if let Some(scripts) = &mut self.scripts {
            let commands = scripts.tick(&mut self.world, self.clock, dt, view_of(&self.camera));
            scripts.apply(commands, self.living.as_mut(), &mut self.wooze, self.moment);
            follow_script_camera(&mut self.camera, scripts);
            // freeze_living stops the clock the field and the props run on.
            if scripts.frozen() {
                dt_living = 0.0;
            }
        }
        let dt = dt_living;
        if let Some(clocks) = &mut self.wooze {
            let draws = clocks.step(&self.world, self.camera.position_array(), dt);
            self.renderer.set_shallow_wooze(draws.iter().copied());
            clocks.step_grass(
                self.renderer.grass_config(),
                self.camera.position_array(),
                dt,
            );
        }
        if let Some(run) = &mut self.living {
            run.step(dt);
        }
    }

    fn step_particles(
        &mut self,
        args: &RenderArgs,
        ctx: &HeadlessContext,
        seconds: f32,
    ) -> Result<()> {
        use flint_runtime::RuntimeSystem;
        const DT: f32 = 1.0 / 60.0;
        let first = self.particles.is_none();
        if first {
            let mut particles = flint_particles::ParticleSystem::new();
            flint_particles::load_particle_effects_from_world(&args.scene, &mut particles);
            particles
                .initialize(&mut self.world)
                .context("Failed to initialize particles")?;
            let dirs = flint_particles::texture_search_dirs(&args.scene);
            flint_render::load_particle_textures(
                &mut self.renderer,
                &ctx.device,
                &ctx.queue,
                &particles.sync,
                &dirs,
            );
            self.particles = Some(particles);
        }
        let Some(particles) = &mut self.particles else {
            return Ok(());
        };
        let wanted = (seconds.max(0.0) / DT).round() as u32;
        let steps = wanted.saturating_sub(self.particle_steps);
        particles.simulate_to(&self.world, steps as f32 * DT, DT);
        self.particle_steps += steps;
        particles.pack(Some(self.camera.position_array()));
        self.renderer
            .update_particles_from(&ctx.device, &ctx.queue, &particles.sync);
        if first && (args.particle_time.is_some() || particles.sync.emitter_count() > 0) {
            say!(
                "Particles: {} alive across {} emitter(s) at t = {:.2}s",
                particles.sync.total_alive(),
                particles.sync.emitter_count(),
                seconds
            );
        }
        Ok(())
    }
}

/// Where `camera` is, for the scripts' `world_to_screen` and the like.
fn view_of(camera: &Camera) -> CameraView {
    CameraView {
        position: camera.position_array(),
        forward: camera.forward_vector(),
        view_proj: camera.view_projection_matrix(),
    }
}

/// Move the camera where the scripts' last tick asked (`set_camera_position`,
/// `set_camera_target`, `set_camera_fov`, ...), as the player's frame does.
fn follow_script_camera(camera: &mut Camera, scripts: &HeadlessScripts) {
    let asked = scripts.camera();
    if let Some([x, y, z]) = asked.position {
        camera.position = Vec3::new(x, y, z);
    }
    if let Some([x, y, z]) = asked.target {
        camera.target = Vec3::new(x, y, z);
    }
    if let Some(fov) = asked.fov {
        camera.fov = fov;
    }
    if let Some(orthographic) = asked.orthographic {
        camera.orthographic = orthographic;
    }
    if let Some(height) = asked.ortho_height {
        camera.ortho_height = height;
    }
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
        let grass_config = flint_terrain::GrassConfig::from_component(terrain_comp, false);

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

                say!(
                    "[grass] Enabled: density={}, max_dist={}",
                    grass_config.density,
                    grass_config.max_distance
                );
            } else {
                say!("[grass] Warning: splat map not found at {:?}", splat_path);
            }
        }

        say!(
            "[terrain] Loaded terrain: {}x{} heightmap, {} chunks",
            heightmap.width,
            heightmap.depth,
            terrain.chunks.len()
        );
        break; // Only one terrain for now
    }

    // Grass on a mesh ground (ADR 0089), as the player loads it
    if flint_player::load_scene_grass_ground(world, scene_path, device, queue, renderer) {
        say!("[grass] Enabled on grass_ground");
    }
}
