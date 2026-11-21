//! Headless scene-to-PNG render command, and since #216 the same at a chosen game
//! time, across a sweep of a wooze knob, or as an animated GIF of a span of time.
//!
//! Two paths share the flags. A still of a scene with nothing on a clock, a
//! style, post or look sweep (#353), a `--frames` sequence (#345) and a look
//! refine (#357) draw on one renderer, a column's post-processing swapped in
//! before each draw. A render that needs clocks (`--time`, a `wooze.*` sweep,
//! `--scripts`, `--report json`, or a scene with living props or a `[wooze]`
//! table) goes through [`Stage`]s instead: one scene loaded into one renderer a
//! column, each with its own clocks, since a prop's state is the whole of its
//! history. A span is the stages captured again and again as their clocks move
//! on (#487 joined the two behind one `--sweep`).
//!
//! A stage's scene is a `flint_player::HeadlessGame` (#725): the player's
//! game core, stepped a fixed step at a time through the player's own frame
//! order (`game_core::step`), with its living props, wooze field and, with
//! `--scripts` (#8), the scene's Rhai scripts and their commands.
//! `--expression-timeline` feeds `expression()` from a file on that clock.
//! Same scene, same timeline, same pixels.

use super::gif_writer;
use super::render_plan::{self, TimeSpec};
use super::render_post::{self, legacy_render_mode, post_process_config};
use super::render_report::{self, CaptureReport};
use anyhow::{anyhow, Context, Result};
use flint_core::components as comp;
use flint_core::load_report;
use flint_core::Vec3;
use flint_player::{ExpressionTimeline, GpuSink, HeadlessAnimation, HeadlessGame, ScriptLook};
use flint_render::model_loader::{self, ModelLoadConfig};
use flint_render::{
    Camera, DebugMode, FrameInputs, HeadlessContext, RendererConfig, SceneRenderer,
};
// A scene loaded for drawing, its failure naming the file (#1044)
use super::common_args::load_settled;
use flint_schema::SchemaRegistry;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

/// With `--report json`, stdout is the report and everything else goes to stderr.
static HUMAN_TO_STDERR: AtomicBool = AtomicBool::new(false);

/// `println!` for the human lines; see [`HUMAN_TO_STDERR`]. The global
/// `--json` sends them to stderr too (`crate::output`).
macro_rules! say {
    ($($t:tt)*) => {
        if HUMAN_TO_STDERR.load(Ordering::Relaxed) || crate::output::stdout_reserved() {
            eprintln!($($t)*)
        } else {
            println!($($t)*)
        }
    };
}

/// `--report json`: the living props' report on stdout (#216), not a file.
const REPORT_TO_STDOUT: &str = "json";

pub use super::render_args::RenderArgs;

// One module a render path (#1145), each a child of this one so it shares
// these imports, `say!` and the helpers every path draws with.
#[path = "render_sequence.rs"]
mod sequence;
#[path = "render_stage.rs"]
mod stage;
#[path = "render_stages.rs"]
mod stages;
#[path = "render_still.rs"]
mod still;
use sequence::*;
use stage::*;
use stages::*;
use still::*;

/// `--list-styles [--json]`: the registry as text or JSON, with the user
/// styles beside the scene when one is given.
fn list_styles(args: &RenderArgs) -> Result<()> {
    use flint_render::stylize::{catalog, StyleRegistry};
    // The scene's user styles, and its palettes and fonts as choices (#322)
    // found from the scene's project by the one asset search (ADR 0114);
    // with no scene, from the working directory
    let assets = match args.scene.is_empty() {
        true => flint_core::project::AssetScope::for_dir(Path::new(".")),
        false => flint_core::project::AssetScope::for_scene(Path::new(&args.scene)),
    };
    let mut styles = StyleRegistry::new();
    if !args.scene.is_empty() {
        for message in styles.load_for(&assets) {
            eprintln!("warning: {message}");
        }
    }
    if crate::output::json() {
        crate::output::document(&serde_json::to_value(catalog::catalog_in(
            &styles, &assets,
        ))?);
    } else {
        print!("{}", catalog::catalog_text_in(&styles, &assets));
    }
    Ok(())
}

/// `--list-effects [--json]`: every built-in post effect's keys (#741).
fn list_effects(_args: &RenderArgs) -> Result<()> {
    use flint_schema::post_effects;
    if crate::output::json() {
        crate::output::document(&serde_json::to_value(post_effects::EFFECTS)?);
    } else {
        print!("{}", post_effects::catalog_text());
    }
    Ok(())
}

/// A GPU kept between renders in one process (#329). `flint render` makes
/// its own; `flint goldens` hands every case the same one, so a suite pays
/// for one device instead of one a case. Each render still builds its own
/// `SceneRenderer`, so its state is its own; with
/// `flint_render::pipeline_cache` on (`flint goldens`, #911) the pipelines
/// it builds are the ones an earlier case built on this device.
#[derive(Default)]
pub struct RenderHost {
    ctx: Option<HeadlessContext>,
    /// The `--gpu` the device was made for.
    gpu: Option<String>,
    /// Devices made so far (one, unless a case asked for another GPU).
    pub devices_made: usize,
    /// The styles the last render drew with (#742): each render has its
    /// own, and `flint goldens` checks a case's user-style errors here.
    styles: Option<flint_render::stylize::StyleRegistry>,
}

impl RenderHost {
    /// Every user style error the last render met, as `flint render` warns
    /// of them: a `.shader.toml` that didn't load, or a shader that didn't
    /// build.
    pub fn style_errors(&self) -> Vec<(String, String)> {
        self.styles.as_ref().map_or_else(Vec::new, |s| s.errors())
    }

    /// Let the device go; the next render makes a new one. For after a
    /// panic, when the device may be in no state to trust.
    pub fn drop_device(&mut self) {
        self.ctx = None;
        // What was kept holds the old device; none of it is the next one's
        flint_render::pipeline_cache::clear();
    }

    fn context(
        &mut self,
        width: u32,
        height: u32,
        gpu: &Option<String>,
    ) -> Result<&HeadlessContext> {
        if self.ctx.is_some() && self.gpu != *gpu {
            self.drop_device();
        }
        match &mut self.ctx {
            Some(ctx) => ctx.resize(width, height),
            None => {
                let ctx = pollster::block_on(HeadlessContext::new(width, height))
                    .context("Failed to create headless render context")?;
                // What draws, once a device: WGPU_BACKEND is otherwise
                // invisible, and on DX12 so is the shader compiler (#912)
                say!(
                    "GPU: {}",
                    flint_render::gpu_select::adapter_line(&ctx.adapter_info)
                );
                self.ctx = Some(ctx);
                self.gpu = gpu.clone();
                self.devices_made += 1;
            }
        }
        Ok(self.ctx.as_ref().unwrap())
    }
}

pub fn run(args: RenderArgs) -> Result<()> {
    if args.out.watch {
        return super::render_watch::run(fold_script_time(args));
    }
    run_in(args, &mut RenderHost::default())
}

/// `flint render` on `host`'s GPU (made on first use).
pub fn run_in(args: RenderArgs, host: &mut RenderHost) -> Result<()> {
    render(&fold_script_time(args), host, None)
}

/// --script-time T is --scripts --time T (#371): everything after reads those.
/// --strict-scripts runs them too (#693).
fn fold_script_time(mut args: RenderArgs) -> RenderArgs {
    if let Some(t) = args.clock.script_time.take() {
        args.clock.time = Some(t);
        args.clock.scripts = true;
    }
    if args.clock.strict_scripts || !args.clock.ui_show.is_empty() || args.clock.ui_focus.is_some()
    {
        args.clock.scripts = true;
    }
    args
}

/// One `flint render`. Under `--watch` a still (or a sweep) stays here
/// after writing, drawing again on the same renderer for each save that
/// only a user style's pass needs to see, and returns when one needs the
/// scene loaded again ([`super::render_watch::Watch::redraws`]).
pub(super) fn render(
    args: &RenderArgs,
    host: &mut RenderHost,
    watch: Option<&mut super::render_watch::Watch>,
) -> Result<()> {
    if args.debug.list_gpus {
        let adapters = flint_render::list_adapters();
        if adapters.is_empty() {
            eprintln!("No GPU adapters found: {}", flint_render::no_adapter_hint());
        }
        for a in &adapters {
            say!("{a}");
        }
        crate::output::when_json(
            || serde_json::json!({ "adapters": adapters.iter().map(|a| a.to_string()).collect::<Vec<_>>() }),
        );
        return Ok(());
    }
    if args.debug.list_styles {
        return list_styles(args);
    }
    if args.debug.list_effects {
        return list_effects(args);
    }
    if args.look.as_deref() == Some("help") {
        let help = super::look::help(&args.scene);
        if crate::output::json() {
            crate::output::document(&serde_json::json!({ "help": help }));
        } else {
            print!("{help}");
        }
        return Ok(());
    }
    if args.scene.is_empty() {
        anyhow::bail!("a scene is required (flint render <scene.toml>)");
    }
    // `--load-report` (#1092): from here to the first capture, the scene's
    // read its first stage
    if args.measure.load_report {
        load_report::begin(
            load_report::LoadKind::Render,
            flint_core::project::shown(Path::new(&args.scene)),
        );
    }
    let read = flint_core::frame_notes::begin(flint_core::frame_notes::NoteKind::SceneRead, || {
        args.scene.clone()
    });
    if !args.sweeps.variant.is_empty() || args.sweeps.variants.is_some() {
        return super::variants::run(args, host);
    }
    if !args.sweeps.zoom.is_empty() {
        anyhow::bail!("--zoom crops each variant's frame: add --variant or --variants");
    }
    let report_json = args.measure.report.as_deref() == Some(REPORT_TO_STDOUT);
    HUMAN_TO_STDERR.store(report_json, Ordering::Relaxed);
    // A mesh sequence (#222) is its own span: its files are the frames
    let meshes = super::mesh_seq::resolve(&args.seq.mesh)?;
    if args.strip.strip {
        render_plan::check_strip_output(&args.output)?;
        render_plan::strip_times(args.clock.time.as_ref(), args.strip.every)?;
    } else if meshes.is_empty() {
        render_plan::check_output(&args.output, args.clock.time.as_ref())?;
    } else {
        check_mesh_output(args, meshes[0].files.len())?;
    }
    let wants_flips = args.seq.flip_stats || args.seq.flip_json.is_some();
    let wants_text = args.out.ascii_text.is_some() || args.out.ascii_text_ansi.is_some();
    let grid = super::sweep::SweepGrid::new(&args.sweeps.sweep).map_err(|e| anyhow::anyhow!(e))?;
    let view = super::sweep::CellView::from_args(
        args.sweeps.sweep_crop,
        args.sweeps.sweep_scale.as_deref(),
    );
    if grid.is_none() && (args.sweeps.sweep_crop.is_some() || args.sweeps.sweep_scale.is_some()) {
        anyhow::bail!("--sweep-crop and --sweep-scale shape a sweep's cells: add a --sweep");
    }
    if wants_text && grid.is_some() {
        anyhow::bail!("--ascii-text writes one frame's grid: drop --sweep");
    }
    let sequence = args.seq.frames.is_some() || !meshes.is_empty();
    if sequence && grid.is_some() {
        anyhow::bail!("--frames and --mesh draw one sequence: drop --sweep");
    }
    if let Some(grid) = grid.as_ref().filter(|g| g.wooze_knob().is_some()) {
        check_wooze_sweep(args, grid)?;
    }
    if let Some(dt) = args.seq.frame_dt {
        if !(dt.is_finite() && dt > 0.0) {
            anyhow::bail!("--frame-dt must be a positive number of seconds");
        }
    }
    let camera_path = args
        .seq
        .camera_path
        .as_deref()
        .map(super::frame_seq::CameraPath::load)
        .transpose()?;
    // User-authored styles (#274): registered before anything reads a
    // `[post_process.styles]` table, a look or a --style that names one
    // The scene's project, found once: its user styles, looks, palettes,
    // glyph sets and papers are all searched from it (ADR 0114). Its binding
    // registers the user styles, and every renderer binds it (#739)
    let binding = flint_render::SceneBinding::for_scene(Path::new(&args.scene));
    for message in binding.style_messages() {
        eprintln!("warning: {message}");
    }
    // The registry this render draws with (#742), shared with the host for
    // its errors: a build error lands in both
    let styles = binding.styles().clone();
    host.styles = Some(styles.clone());
    let assets = binding.assets().clone();
    args.style
        .check(&styles)
        .map_err(|e| anyhow::anyhow!("--style: {e}"))?;
    render_post::check(args, &styles)?;
    render_post::warn_legacy(args);
    let look = args
        .look
        .as_deref()
        .map(|name| flint_render::look::find_in(name, &assets).map_err(anyhow::Error::msg))
        .transpose()?;
    let (width, height) = match grid {
        Some(_) => view.render_size(args.width, args.height),
        // A sheet's cells, and a GIF small enough to move
        None if args.seq.sheet || (!meshes.is_empty() && render_plan::is_gif(&args.output)) => {
            let (w, h) = render_plan::default_size(args.seq.sheet, !args.seq.sheet);
            (args.width.unwrap_or(w), args.height.unwrap_or(h))
        }
        // A refine's rounds are sheet cells too
        None if args.refine.ai_refine => {
            let (w, h) = super::sweep::CELL_SIZE;
            (args.width.unwrap_or(w), args.height.unwrap_or(h))
        }
        None => (args.width.unwrap_or(1920), args.height.unwrap_or(1080)),
    };
    view.check(width, height).map_err(|e| anyhow::anyhow!(e))?;
    let reference = args
        .measure
        .reference
        .as_deref()
        .map(super::look_measure::load_reference)
        .transpose()?;
    if (args.measure.report.is_some() || reference.is_some()) && sequence {
        anyhow::bail!(
            "--report / --reference measure one frame or a sweep: drop --frames / --mesh"
        );
    }
    if (args.measure.frame_dump.is_some() || args.measure.frame_capture.is_some())
        && (sequence || grid.is_some())
    {
        anyhow::bail!(
            "--frame-dump / --frame-capture record one frame: drop --frames / --mesh / --sweep \
             (a span's last frame is recorded: --time 0..4 --frame-capture f.flintframe)"
        );
    }
    if args.clock.time.is_some() && sequence {
        anyhow::bail!(
            "--time and --frames / --mesh each draw a span: --time 0..4 -o x.gif, or a sequence"
        );
    }
    if let (Some(frames), Some(first)) = (args.seq.frames, meshes.first()) {
        if frames as usize != first.files.len() {
            anyhow::bail!(
                "--frames {frames} but --mesh {} has {} files: a mesh sequence draws a frame \
                 a file, so drop --frames",
                first.entity,
                first.files.len()
            );
        }
    }
    flint_render::set_gpu_override(args.debug.gpu.clone());
    // The one resolver (ADR 0114): --schemas, else the manifest, else discovery
    let (registry, schema_dirs) =
        super::common_args::load_schemas(Some(&args.scene), &args.debug.schemas)?;
    super::common_args::warn_dropped_schemas(&schema_dirs);
    if schema_dirs.dirs.is_empty() {
        eprintln!("Warning: No schemas directories found");
    }
    // Its archetypes' placeholder boxes, from the same schemas (#751)
    let binding = binding.with_archetypes(&registry);

    // Load scene. Its styles are its own registry's (#742), so a user style
    // an earlier render in this process loaded (a case before this one in
    // `flint goldens`, #911) from a folder this scene can't see is not one
    let (mut world, scene_file) = load_settled(&args.scene, &registry)?;
    drop(read);
    say!("Loaded scene: {}", scene_file.scene.name);
    say!("Entities: {}", world.entity_count());
    // An echoes column with nothing to echo would draw the same picture
    // in every column (#531)
    if let Some(grid) = grid.as_ref().filter(|g| g.is_living()) {
        if !flint_player::living_wanted(&world) {
            anyhow::bail!(
                "--sweep {}: this scene has no living props, so [living] echoes draws \
                 nothing (a living prop is a model with a living_prop component whose \
                 asset names a [living] spec)",
                grid.name()
            );
        }
    }
    for binding in &meshes {
        bind_mesh(&mut world, binding)?;
    }

    // Every column's post-processing, worked out (and checked) before any
    // GPU work: the scene's block, under the look, under the column.
    // A scene's own look (#356) goes under its keys first; --look then
    // goes over the result
    let scene_pp = scene_file
        .post_process
        .as_ref()
        .map(|def| flint_render::look::resolve_scene_in(def, Path::new(&args.scene), &assets));
    let scene_pp = scene_pp.as_ref();
    if let Some(name) = scene_pp.and_then(|d| d.look.as_deref()) {
        say!("Scene look: {name}");
    }
    let base_pp = super::sweep::base_def(scene_pp, look.as_ref())?;
    let cells = grid.as_ref().map_or(vec![(0, 0)], |g| g.cells());
    let mut configs = Vec::new();
    for &(c, r) in &cells {
        let def = match &grid {
            Some(grid) => grid.cell_def(c, r, scene_pp, look.as_ref(), &args.scene)?,
            None => base_pp.clone(),
        };
        let mut config = post_process_config(args, def.as_ref(), &styles)?;
        for spec in grid.as_ref().map_or(Vec::new(), |g| g.cell_styles(c, r)) {
            flint_render::stylize::apply_style_spec(&mut config, &spec, &styles)
                .map_err(|e| anyhow::anyhow!("--sweep: {e}"))?;
        }
        configs.push(config);
    }
    if let Some(look) = &look {
        say!("Look: {} ({})", look.name, look.source);
    }
    let reference = measured_reference(args, reference, &configs[0])?;
    // A reference grid scores frames by their glyphs: every frame's grid is read
    let reads_glyphs = reference.as_ref().is_some_and(|r| r.ink.is_some());

    // A render with clocks goes through stages (#216, #487)
    let timed = args.clock.time.is_some()
        || grid.as_ref().is_some_and(|g| g.wooze_knob().is_some())
        || args.clock.scripts
        || args.clock.expression_timeline.is_some()
        || report_json
        || flint_player::living_wanted(&world)
        || scene_file.tables.contains_key("wooze");
    if timed && !meshes.is_empty() {
        anyhow::bail!(
            "--mesh draws its files on one still renderer, and this render has clocks \
             (--time, --scripts, --report json, a wooze sweep, living props or a [wooze] \
             table): render the meshes in a scene without them"
        );
    }
    if timed {
        return run_stages(
            args,
            host,
            StagedRender {
                registry: &registry,
                binding: &binding,
                first: (world, scene_file),
                grid: grid.as_ref(),
                configs,
                report_json,
                view,
                scene_pp,
                look,
                reference,
                camera_path: camera_path.as_ref(),
            },
        );
    }
    // A still draws the scene as authored, before any script (#371): said, as
    // the clocked path says it, since a look a script sets is otherwise missing
    // from the picture with no word why
    let scripted = named_with(&world, comp::SCRIPT);
    if !scripted.is_empty() {
        eprintln!("Warning: {}", scripts_not_run(&scripted));
    }

    let device = load_report::span(load_report::Stage::Device, String::new);
    let ctx = host.context(width, height, &args.debug.gpu)?;
    drop(device);

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
    if let Some(d) = args.camera.distance {
        camera.distance = d;
    }
    if let Some(y) = args.camera.yaw {
        camera.yaw = y.to_radians();
    }
    if let Some(p) = args.camera.pitch {
        camera.pitch = p.to_radians();
    }
    if let Some(t) = args.camera.target {
        camera.target = Vec3::new(t[0], t[1], t[2]);
    }
    if let Some(f) = args.camera.fov {
        camera.fov = f;
    }
    camera.update_orbit();

    // Create scene renderer
    let made = load_report::span(load_report::Stage::Renderer, String::new);
    let mut renderer = SceneRenderer::new_headless(
        &ctx.device,
        &ctx.queue,
        ctx.format,
        ctx.width,
        ctx.height,
        RendererConfig {
            show_grid: !args.camera.no_grid,
            sample_count: args.shading.msaa,
        },
    );
    drop(made);
    // Parts skipped or repeated by label (#1317)
    renderer.set_pass_knobs(args.measure.pass_knobs.knobs()?);

    // The scene bound, its levers set (#739), as every stage does
    bind_capture_scene(args, ctx, &mut renderer, &world, &scene_file, &binding);

    // Load models and textures from the scene
    let config = ModelLoadConfig::in_project(assets.project.clone(), &args.scene);
    let models = model_loader::load_models_from_world(
        &mut world,
        &mut renderer,
        &ctx.device,
        &ctx.queue,
        &config,
    );

    // Meshes swept along the scene's splines (#750); a still has no
    // physics, so their colliders are dropped
    flint_render::load_scene_splines(
        &args.scene,
        &mut world,
        &mut renderer,
        &ctx.device,
        &ctx.queue,
    );

    // The debug views, shadows, post-processing (the first column's, the
    // only one without --sweep), render scale and quality, as every stage
    set_up_capture(
        args,
        ctx,
        &mut renderer,
        CaptureSetup {
            scene_file: &scene_file,
            config: configs[0].clone(),
            text_capture: wants_text
                || wants_flips
                || args.measure.report.is_some()
                || reads_glyphs,
            first: true,
        },
    )?;
    // Post time for grain/mode animation and animated styles. Headless
    // renders leave this 0.0 so PNG snapshots stay deterministic;
    // --style-time (or the older --grain-time) pins an explicit frame.
    let time = still_time(args);
    // Poses held on twos (#405) tick with the post time from the first
    // frame, so a --frames sequence holds frame 0's pose through its tick
    renderer.set_motion_time(time);

    // Animation (#307), which runs IK after each step; without it, two-bone IK
    // (ADR 0070) poses authored chains before the single frame.
    let mut animation = pose_animation(args, &models, &config.project, &mut world, ANIM_DT, true);
    match &animation {
        Some(posed) => posed.upload(&mut renderer, &ctx.device, &ctx.queue),
        None => flint_animation::IkPass::new().run(&mut world),
    }

    if let Some(name) = &args.camera.select {
        let id = world
            .get_id(name)
            .with_context(|| format!("--select: no entity named '{name}'"))?;
        renderer.set_selected_entity(Some(id));
    }

    // Particles (ADR 0068): fixed-step, fixed-seed simulation so a snapshot
    // at a given --particle-time is reproducible run to run.
    let mut particle_sim = None;
    if let Some(seconds) = args.clock.particle_time {
        use flint_runtime::RuntimeSystem;
        let mut particles = flint_particles::ParticleSystem::new();
        let project = &config.project;
        flint_particles::load_particle_effects_from_world(project, &args.scene, &mut particles);
        particles
            .initialize(&mut world)
            .context("Failed to initialize particles")?;
        flint_render::load_scene_particle_textures(
            &mut renderer,
            &ctx.device,
            &ctx.queue,
            &particles.sync,
            project,
            Path::new(&args.scene).parent().unwrap_or(Path::new(".")),
        );
        particles.simulate_to(&world, seconds.max(0.0), 1.0 / 60.0);
        particles.pack(Some(camera.position_array()));
        say!(
            "Particles: {} alive across {} emitter(s) at t = {:.2}s",
            particles.sync.total_alive(),
            particles.sync.emitter_count(),
            seconds
        );
        particle_sim = Some(particles);
    }

    // The frame's inputs, and the world read into the draw lists (#739)
    renderer.prepare(
        &ctx.device,
        &ctx.queue,
        &world,
        &FrameInputs {
            particles: particle_sim.as_ref().map(|p| &p.sync),
            ..still_inputs(args, &camera, time)
        },
    );

    // GPU timing: the same frame many times, each pass's spread (#292)
    if let Some(frames) = args.measure.gpu_time {
        time_gpu(
            ctx,
            &mut renderer,
            &camera,
            frames,
            args.measure.gpu_time_json.as_deref(),
        )?;
    }

    let frames = match meshes.first() {
        Some(first) => Some(first.files.len() as u32),
        None => args.seq.frames,
    };
    if let Some(frames) = frames {
        return render_sequence(
            args,
            SequenceRig {
                ctx,
                renderer: &mut renderer,
                world: &mut world,
                camera,
                camera_path: camera_path.as_ref(),
                particles: particle_sim.as_mut(),
                animation: animation.as_mut(),
                meshes: &meshes,
                scene_name: &scene_file.scene.name,
            },
            frames,
        );
    }

    if args.refine.ai_refine {
        let look = look.expect("clap requires --look with --ai-refine");
        let reference = reference.expect("clap requires --reference with --ai-refine");
        return refine_look(args, look, reference, &mut |table| {
            let def = super::look::overlay(scene_pp, table)?;
            let config = post_process_config(args, Some(&def), renderer.styles())?;
            let scale = reads_glyphs.then(|| ink_scale(args, &config));
            let image = draw_frame(ctx, &mut renderer, &camera, Some(config), Some(&world))?;
            let ink = scale.and_then(|s| frame_ink(&renderer, ctx, &s));
            Ok(super::look_refine::Frame { image, ink })
        });
    }

    // The load is up; its last stage is the first draw (#1092)
    load_report::bring_up_done();
    // Draw and write the still or sheet; under --watch again on this same
    // renderer for each save of a user style's sources (#432). A redraw
    // sets the first column's config too: the last draw left the last's.
    let mut draw_and_write = |redraw: bool| -> Result<()> {
        // Render each column (one without --sweep), reading each back
        let mut frames = Vec::with_capacity(configs.len());
        let mut texts = Vec::with_capacity(configs.len());
        for (i, config) in configs.iter().enumerate() {
            // The frame's description (#1084) and capture (#1085): the
            // still's own frame
            let dump = super::render_frame_dump::Wanted::of(args)
                .filter(|_| i == 0)
                .map(|wanted| {
                    (
                        wanted,
                        super::render_frame_dump::arm(ctx, &mut renderer, wanted),
                    )
                });
            let pixels = draw_pixels(
                ctx,
                &mut renderer,
                &camera,
                (i > 0 || redraw).then(|| config.clone()),
                Some(&world),
            )?;
            first_capture_done();
            if let Some((wanted, armed)) = dump {
                let said = super::render_frame_dump::write(
                    armed,
                    ctx,
                    &mut renderer,
                    Some(&world),
                    wanted,
                    super::render_frame_dump::Origin {
                        scene: &args.scene,
                        game_time: None,
                        written_by: "flint render",
                        notes: Vec::new(),
                    },
                    Some(super::render_frame_dump::ReplayFacts {
                        inputs: flint_render::frame_replay::FrameInputsState::of(
                            &FrameInputs {
                                particles: particle_sim.as_ref().map(|p| &p.sync),
                                ..still_inputs(args, &camera, time)
                            },
                            &world,
                        ),
                        camera: &camera,
                        particles: particle_sim.as_ref().map(|p| &p.sync),
                    }),
                )?;
                for line in said {
                    say!("{line}");
                }
            }
            // The grid is read back once a frame: a second read finds nothing
            let measured = args.measure.report.is_some() || reads_glyphs;
            let text = (measured || wants_text)
                .then(|| renderer.style_text(&ctx.device, "ascii"))
                .flatten();
            if measured {
                texts.push(text.clone());
            }
            if wants_text {
                write_ascii_text(
                    args.out.ascii_text.as_deref(),
                    args.out.ascii_text_ansi.as_deref(),
                    text,
                )?;
            }
            let mut img = image::RgbaImage::from_raw(width, height, pixels)
                .context("Failed to create image from pixel data")?;
            // The style mask view says which hue is which style (#309): in the
            // terminal once, and on every frame's corner
            if renderer.debug_state().mode == DebugMode::StyleMask && !args.no_postprocess {
                let ids = renderer.style_ids();
                if i == 0 {
                    say!("{}", flint_render::style_mask::legend_line(ids));
                }
                super::style_sheet::draw_legend(&mut img, &flint_render::style_mask::legend(ids));
            }
            // The edge view says which colour is which source and decision
            // (#349), and the thresholds it was drawn at
            if renderer.debug_state().mode == DebugMode::StyleEdges && !args.no_postprocess {
                let thresholds = flint_render::stylize::edge_view_thresholds(
                    renderer.effective_post_process_config(),
                );
                if i == 0 {
                    let names: Vec<&str> = flint_render::stylize::EDGE_VIEW_LEGEND
                        .iter()
                        .map(|(_, name)| *name)
                        .collect();
                    say!("ascii edges: {thresholds}");
                    say!(
                        "  pixels cyan/amber/magenta: {}; cells green/dashed/yellow/red/grey: {}",
                        names[..3].join(", "),
                        names[3..].join(", ")
                    );
                }
                let mut rows: Vec<(Option<[f32; 3]>, String)> =
                    flint_render::stylize::EDGE_VIEW_LEGEND
                        .iter()
                        .map(|(color, name)| (Some(*color), name.to_string()))
                        .collect();
                rows.extend(thresholds.split("; ").map(|t| (None, t.to_string())));
                // Nine rows: half the mask legend's size, so the card leaves the
                // scene readable
                let scale = (img.width() / 1280).clamp(1, 3);
                super::style_sheet::draw_card(&mut img, "ascii edges", &rows, scale);
            }
            if let Some(grid) = &grid {
                let (c, r) = cells[i];
                say!("  {}", grid.describe(c, r));
            }
            frames.push(img);
        }

        // A sweep whose neighbours drew the same showed nothing: say so (#216).
        // Judged on the frames as drawn, before a crop could hide a difference.
        if let Some(grid) = &grid {
            for warning in grid.identical_warnings(&frames) {
                eprintln!("Warning: {warning}");
            }
        }

        // The pictures the sheet is made of: each frame as its cell shows it
        // (cropped or shrunk under --sweep-crop / --sweep-scale, #358), and
        // for a crop the first frame whole, to show where they sit (#501)
        let locator = grid
            .as_ref()
            .and_then(|_| frames.first())
            .and_then(|f| view.locator(f));
        let pictures: Vec<image::RgbaImage> = match &grid {
            Some(_) => frames.into_iter().map(|f| view.cell(f)).collect(),
            None => frames,
        };

        // Measures (#357), from those same pixels. The ASCII section counts the
        // whole drawn frame's grid, which a crop shows only part of.
        let mut columns = Vec::new();
        if args.measure.report.is_some() || reference.is_some() {
            columns = measure_cells(
                args,
                &pictures,
                &texts,
                &configs,
                reference.as_ref(),
                grid.as_ref(),
                &cells,
            );
        }
        let nearest = super::look_measure::RenderReport::nearest_column(&columns);

        let mut pictures = pictures;
        let img = match &grid {
            None => pictures.swap_remove(0),
            Some(grid) => {
                let over = match &look {
                    Some(look) => format!("  over {}", look.name),
                    None => String::new(),
                };
                // Only a sheet that isn't of small frames says how big they were
                let scale = match view {
                    super::sweep::CellView::Small => String::new(),
                    _ => format!("  {}", view.describe(width, height)),
                };
                let reference_note = match &args.measure.reference {
                    Some(r) => format!("  d = distance to {r}"),
                    None => String::new(),
                };
                let subtitle = format!("sweep {}{over}{scale}{reference_note}", grid.name());
                let label =
                    |s: &super::sweep::Sweep| (0..s.values.len()).map(|i| s.label(i)).collect();
                match &grid.rows {
                    None => {
                        // With a reference, each label carries its distance and
                        // the nearest is starred (#357)
                        let labels = distance_labels(label(&grid.columns), &columns, nearest);
                        let mut text = super::style_sheet::SheetText::new(
                            scene_file.scene.name.clone(),
                            subtitle,
                            labels,
                        );
                        text.locator = locator;
                        super::style_sheet::compose(&pictures, &text)
                    }
                    Some(rows) => super::style_sheet::compose_grid(
                        &pictures,
                        &super::style_sheet::GridText {
                            title: scene_file.scene.name.clone(),
                            subtitle,
                            row_key: rows.name(),
                            column_labels: label(&grid.columns),
                            row_labels: label(rows),
                            time: String::new(),
                            locator,
                        },
                    ),
                }
            }
        };
        if let Some(path) = &args.measure.report {
            write_measures(
                args,
                path,
                look.as_ref(),
                grid.as_ref(),
                (width, height),
                columns,
                nearest,
            )?;
        }
        img.save(&args.output)
            .context(format!("Failed to save image to {}", args.output))?;

        say!(
            "Rendered {}x{} image to {}",
            img.width(),
            img.height(),
            args.output
        );

        Ok(())
    };
    draw_and_write(false)?;
    if let Some(watch) = watch {
        // The renderer's user styles are shared with this copy, so a WGSL
        // save it reloads is the pass's to see (#742)
        let mut watched = styles.clone();
        watch.redraws(&assets, &mut watched, || draw_and_write(true))?;
    }
    Ok(())
}

/// One still frame, read back: under `config` when given (a sweep column,
/// a refine candidate), each drawn as a first frame so no column inherits
/// the last one's ASCII history (#277's hysteresis would carry glyphs
/// across).
/// The first capture is drawn: a load being recorded (`--load-report`,
/// #1092) ends, says its table and is returned, for `--report json`.
fn first_capture_done() -> Option<load_report::LoadReport> {
    let report = load_report::finish()?;
    say!("{}", report.table());
    Some(report)
}

fn draw_pixels(
    ctx: &HeadlessContext,
    renderer: &mut SceneRenderer,
    camera: &Camera,
    config: Option<flint_render::PostProcessConfig>,
    world: Option<&flint_ecs::FlintWorld>,
) -> Result<Vec<u8>> {
    if let Some(config) = config {
        renderer.set_post_process_config(config);
        renderer.ensure_style_resources(&ctx.device);
        renderer.ensure_fxaa_resources(&ctx.device);
    }
    renderer.reset_style_history();
    // A scene's [panels] draw each panel's camera (#880)
    renderer.render_frame_to(
        &ctx.device,
        &ctx.queue,
        ctx.depth_target(),
        camera,
        &ctx.color_view,
        world,
    );
    pollster::block_on(ctx.read_pixels()).context("Failed to read rendered pixels")
}

/// [`draw_pixels`] as an image.
fn draw_frame(
    ctx: &HeadlessContext,
    renderer: &mut SceneRenderer,
    camera: &Camera,
    config: Option<flint_render::PostProcessConfig>,
    world: Option<&flint_ecs::FlintWorld>,
) -> Result<image::RgbaImage> {
    let pixels = draw_pixels(ctx, renderer, camera, config, world)?;
    image::RgbaImage::from_raw(ctx.width, ctx.height, pixels)
        .context("Failed to create image from pixel data")
}

/// A captured frame drawn again (#1089): the host takes a `Stage`.
#[path = "render_replay.rs"]
pub(crate) mod replay_host;

/// Animation's fixed step (#307) where nothing else names one: the player's
/// frame at 60 Hz.
const ANIM_DT: f64 = 1.0 / 60.0;

/// `--anim-time` (#307): the scene's animation loaded as the player loads it and
/// stepped to that time, `step` seconds at a time and landing on it exactly, IK
/// after every step. `None` without the flag. With `tell`, says what it posed,
/// or warns that nothing in the scene animates.
fn pose_animation(
    args: &RenderArgs,
    models: &model_loader::ModelLoadResult,
    project: &flint_core::project::Project,
    world: &mut flint_ecs::FlintWorld,
    step: f64,
    tell: bool,
) -> Option<HeadlessAnimation> {
    let seconds = args.clock.anim_time?.max(0.0);
    let mut posed = HeadlessAnimation::load(models, project, &args.scene);
    let (steps, rest) = render_plan::steps_within(seconds, step);
    if steps == 0 && rest == 0.0 {
        posed.settle(world);
    }
    for _ in 0..steps {
        posed.step(world, step);
    }
    if rest > 0.0 {
        posed.step(world, rest);
    }
    if !tell {
        return Some(posed);
    }
    let animated = posed.animated(world);
    if animated.is_empty() {
        eprintln!(
            "warning: --anim-time: nothing in this scene animates (no animator, no skinned model)"
        );
    } else {
        say!(
            "Animation: {} at t = {:.3}s ({} skinned, {} step(s) of {:.6}s)",
            animated.join(", "),
            posed.time(),
            posed.skinned_count(),
            steps + u64::from(rest > 0.0),
            step
        );
    }
    for name in posed.unposed(world) {
        eprintln!(
            "warning: --anim-time: {name} has an animator and a skinned model but no \
             `skeleton` component, so its clip doesn't play (in the player either); \
             add [entities.{name}.skeleton]"
        );
    }
    Some(posed)
}

/// The renderer's share of a capture's scene, the same for the one-renderer
/// path and every stage (#739): the scene bound (its project and folder,
/// nothing of a last scene, its `[environment]` levers, its terrain and
/// grass), with the CLI's levers over them.
fn bind_capture_scene(
    args: &RenderArgs,
    ctx: &HeadlessContext,
    renderer: &mut SceneRenderer,
    world: &flint_ecs::FlintWorld,
    scene_file: &flint_scene::SceneFile,
    binding: &flint_render::SceneBinding,
) {
    let binding = binding
        .clone()
        .with_environment(scene_file.environment.as_ref());
    let bound = renderer.bind_scene(&ctx.device, &ctx.queue, world, &binding);
    if let Some(grass) = bound
        .grass
        .as_ref()
        .filter(|g| g.component == comp::TERRAIN)
    {
        say!(
            "[grass] Enabled: density={}, max_dist={}",
            grass.config.density,
            grass.config.max_distance
        );
    }
    if let Some(terrain) = &bound.terrain {
        say!(
            "[terrain] Loaded terrain: {}x{} heightmap, {} chunks",
            terrain.heightmap_size.0,
            terrain.heightmap_size.1,
            terrain.terrain.chunks.len()
        );
    }
    if bound
        .grass
        .as_ref()
        .is_some_and(|g| g.component == comp::GRASS_GROUND)
    {
        say!("[grass] Enabled on grass_ground");
    }

    // CLI overrides win over scene-authored values
    if let Some(oren) = args.shading.oren_nayar {
        renderer.set_oren_nayar(oren);
    }
    if let Some(strength) = args.shading.sheen_strength {
        renderer.set_sheen(args.shading.sheen_color.unwrap_or([1.0; 3]), strength);
    }
}

/// What [`set_up_capture`] sets beyond the flags.
struct CaptureSetup<'a> {
    scene_file: &'a flint_scene::SceneFile,
    /// The post-processing the capture starts with.
    config: flint_render::PostProcessConfig,
    /// Whether the ASCII grid is read back (text, flips, measures, #596).
    text_capture: bool,
    /// Whether this is the first renderer of the run, which says what all
    /// of them would.
    first: bool,
}

/// A capture's renderer once its scene's models are in, the same for the
/// one-renderer path and every stage (#739): the debug views, shadows,
/// post-processing and the scene's comic panels, the style text read-back,
/// then render scale and quality over them.
fn set_up_capture(
    args: &RenderArgs,
    ctx: &HeadlessContext,
    renderer: &mut SceneRenderer,
    setup: CaptureSetup,
) -> Result<()> {
    if let Some(mode) = args.debug.debug_mode {
        if setup.first
            && matches!(mode, DebugMode::StyleMask | DebugMode::StyleEdges)
            && args.no_postprocess
        {
            eprintln!(
                "warning: --debug-mode {} draws through post-processing, \
                 which --no-postprocess turns off; the image shows none of it",
                crate::commands::common_args::debug_mode_name(mode)
            );
        }
        renderer.set_debug_mode(mode);
    }
    if args.debug.wireframe_overlay {
        renderer.set_debug_mode(DebugMode::WireframeOverlay);
    }
    if args.debug.show_normals {
        renderer.toggle_normal_arrows();
    }
    if args.shading.no_tonemapping {
        renderer.set_tonemapping(false);
    }
    if args.shading.no_shadows {
        renderer.set_shadows(false);
    }
    // The renderer constructs at 2048; only recreate the pass when the flag
    // actually deviates. Historically the flag defaulted to 1024 but the
    // != 1024 guard meant an explicit 1024 was silently ignored — with the
    // texel size now uploaded per-resolution (ADR 0049) the flag is real.
    if args.shading.shadow_resolution != 2048 {
        renderer.set_shadow_resolution(&ctx.device, args.shading.shadow_resolution);
    }

    renderer.set_post_process_config(setup.config);
    // A scene laid out as comic panels (#880, ADR 0120); its scripts may cut
    // between layouts as it plays
    renderer.set_panels(
        flint_render::panels::PanelLayout::from_scene_tables(&setup.scene_file.tables, &args.scene),
        setup.scene_file.post_process.as_ref(),
    );
    renderer.set_style_text_capture(setup.text_capture);
    renderer.ensure_style_resources(&ctx.device);
    renderer.ensure_fxaa_resources(&ctx.device);

    if let Some(scale) = args.shading.render_scale {
        renderer.set_render_scale(&ctx.device, scale);
    }
    // Quality settings over the flags above: the renderer's own state is the
    // starting point, so `--quality` names only what it changes.
    if let Some(quality) = &args.shading.quality {
        let mut q = flint_render::QualitySettings {
            render_scale: renderer.render_scale(),
            msaa: renderer.msaa(),
            shadows: renderer.shadows_enabled(),
            shadow_resolution: renderer.shadow_resolution(),
            ..Default::default()
        };
        if let Some(e) = q.apply_str(quality).first() {
            anyhow::bail!("--quality: {e}");
        }
        let applied = renderer.set_quality(&ctx.device, &q);
        if setup.first {
            say!("Quality: {}", applied.to_query());
        }
    }
    Ok(())
}

/// A still's post time: `--style-time` (or the older `--grain-time`), else
/// 0, so PNG snapshots stay deterministic.
fn still_time(args: &RenderArgs) -> f64 {
    args.style
        .style_time
        .or(args.clock.grain_time)
        .map_or(0.0, f64::from)
}

/// The 2D view a capture lays sprites out in: the camera's orthographic
/// height and aspect, from the origin.
fn capture_view_2d(camera: &Camera) -> flint_render::View2d {
    flint_render::View2d {
        camera_offset: [0.0, 0.0],
        ortho_height: camera.ortho_height,
        aspect_ratio: camera.aspect,
    }
}

/// A still's frame inputs at post time `time` (#739): its grass at rest
/// whatever the post time, `--pixels-per-point`, and the capture's 2D view.
fn still_inputs(args: &RenderArgs, camera: &Camera, time: f64) -> FrameInputs<'static> {
    FrameInputs {
        time,
        grass_time: Some(0.0),
        pixels_per_point: args.out.pixels_per_point.unwrap_or(1.0),
        view_2d: capture_view_2d(camera),
        ocean_tint: legacy_render_mode(args).and_then(|m| m.ocean_tint()),
        ..Default::default()
    }
}
