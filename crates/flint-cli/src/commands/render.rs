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

/// Frames `--gpu-time` leaves out at the start of `timed`: the first tenth,
/// at most 30, while pipelines, caches and the GPU's clocks settle (#292).
fn gpu_warmup(timed: usize) -> usize {
    (timed / 10).min(30)
}

/// `--gpu-time N`: the frame `renderer` holds drawn N times with GPU timing,
/// each frame's times kept raw and each pass's spread over them printed
/// (#292), with the style chain's targets; `--gpu-time-json` writes it too.
fn time_gpu(
    ctx: &HeadlessContext,
    renderer: &mut SceneRenderer,
    camera: &Camera,
    frames: u32,
    json: Option<&str>,
) -> Result<()> {
    let (width, height) = (ctx.width, ctx.height);
    if !renderer.set_gpu_timing(&ctx.device, &ctx.queue, true) {
        anyhow::bail!("--gpu-time: this adapter has no TIMESTAMP_QUERY");
    }
    if let Some(timer) = renderer.gpu_timer_mut() {
        timer.record(true);
    }
    for _ in 0..frames.max(1) {
        let mut encoder = ctx
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("GPU Time Frame"),
            });
        renderer.encode_to(
            &ctx.device,
            &ctx.queue,
            &mut encoder,
            ctx.depth_target(),
            camera,
            &ctx.color_view,
        );
        renderer.encode_gpu_timings(&ctx.device, &mut encoder);
        ctx.queue.submit(std::iter::once(encoder.finish()));
        renderer.frame_submitted();
        let _ = flint_render::wait_for_gpu(&ctx.device);
    }
    let spread = match renderer.gpu_timer_mut() {
        Some(timer) => {
            timer.flush(&ctx.device);
            let recorded = timer.recorded();
            let spread = flint_render::gpu_timer::spread(recorded, gpu_warmup(recorded.len()));
            timer.record(false);
            spread
        }
        None => Default::default(),
    };
    let (sw, sh) = renderer.scene_size();
    let adapter = &ctx.adapter_info;
    say!(
        "GPU time over {} frames at {}x{} (scene {}x{}) on {} ({:?}):",
        frames,
        width,
        height,
        sw,
        sh,
        adapter.name,
        adapter.backend
    );
    say!(
        "  {} frames timed, the first {} dropped as warm-up",
        spread.frames + spread.dropped,
        spread.dropped
    );
    for line in spread_lines(&spread) {
        say!("{line}");
    }
    if let Some(path) = json {
        let doc = spread_json(&spread, frames, (width, height), (sw, sh), adapter);
        std::fs::write(path, serde_json::to_string_pretty(&doc)?)
            .context(format!("Failed to write {path}"))?;
        say!("Wrote GPU timing to {path}");
    }
    // What each of the style chain's textures costs (#412): the
    // built-ins' and every user style's targets
    let targets = renderer.style_target_entries();
    if !targets.is_empty() {
        let total: u64 = targets.iter().map(|t| t.bytes).sum();
        say!(
            "Style targets: {} ({:.2} MB)",
            targets.len(),
            total as f64 / (1024.0 * 1024.0)
        );
        let w = targets.iter().map(|t| t.name.len()).max().unwrap_or(0);
        for t in &targets {
            say!(
                "  {:<w$} {:>5}x{:<5} {:<15} {:>7.2} MB",
                t.name,
                t.width,
                t.height,
                format!("{:?}", t.format),
                t.bytes as f64 / (1024.0 * 1024.0)
            );
        }
    }
    Ok(())
}

/// `--gpu-time`'s table: a header, a row a pass, then the frame total's.
/// The label column is as wide as the longest label (a style's, such as
/// "Display bloom").
fn spread_lines(spread: &flint_render::GpuSpread) -> Vec<String> {
    let rows: Vec<(String, &flint_render::PassSpread)> = spread
        .passes
        .iter()
        .map(|p| (p.name(), p))
        .chain(spread.frame.iter().map(|f| ("frame".to_string(), f)))
        .collect();
    let w = rows.iter().map(|(n, _)| n.len()).max().unwrap_or(0).max(12);
    let mut lines = vec![format!(
        "  {:<w$} {:>7} {:>7} {:>7} {:>7}  ms",
        "pass", "min", "p50", "p90", "max"
    )];
    for (name, p) in rows {
        lines.push(format!(
            "  {name:<w$} {:>7.3} {:>7.3} {:>7.3} {:>7.3}",
            p.min, p.p50, p.p90, p.max
        ));
    }
    lines
}

/// `--gpu-time-json`: the spread, each pass with its kept frames' raw times.
fn spread_json(
    spread: &flint_render::GpuSpread,
    frames: u32,
    (width, height): (u32, u32),
    (scene_width, scene_height): (u32, u32),
    adapter: &wgpu::AdapterInfo,
) -> serde_json::Value {
    // The passes in the shape `flint-player --perf-report` writes them too,
    // so `scripts/perf-compare.py` reads both (#1093)
    let mut doc = spread.to_json(true);
    let more = serde_json::json!({
        "format": "flint-gpu-time",
        "version": 1,
        "adapter": {
            "name": adapter.name,
            "backend": format!("{:?}", adapter.backend),
            "driver": adapter.driver,
            "driver_info": adapter.driver_info,
        },
        "build_profile": flint_player::provenance::BUILD_PROFILE,
        "engine_commit": flint_player::provenance::BUILD_COMMIT,
        "width": width,
        "height": height,
        "scene_width": scene_width,
        "scene_height": scene_height,
        "frames_drawn": frames.max(1),
        "frames_timed": spread.frames + spread.dropped,
    });
    if let (Some(doc), serde_json::Value::Object(more)) = (doc.as_object_mut(), more) {
        doc.extend(more);
    }
    doc
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

/// The reference a render measures against (#357): the `--reference`
/// picture and, with `--reference-text`, its glyph grid (#495), read
/// through the glyph set `config` draws with. A reference picture of an
/// ASCII frame with no grid is said once: its letters are scored as pixels.
fn measured_reference(
    args: &RenderArgs,
    image: Option<image::RgbImage>,
    config: &flint_render::PostProcessConfig,
) -> Result<Option<super::look_measure::Reference>> {
    use super::look_measure as lm;
    let Some(image) = image else {
        return Ok(None);
    };
    let ink = match &args.measure.reference_text {
        Some(path) => Some(lm::load_reference_text(path, &ink_scale(args, config))?),
        None => {
            if config.styles.is_enabled("ascii") {
                say!(
                    "Note: the reference is scored by its pixels; give --reference-text \
                     (its --ascii-text) to score glyphs as glyphs (#495)"
                );
            }
            None
        }
    };
    Ok(Some(lm::Reference { image, ink }))
}

/// How a glyph's ink is read under `config`: its ASCII font's atlas.
fn ink_scale(
    args: &RenderArgs,
    config: &flint_render::PostProcessConfig,
) -> super::look_measure::InkScale {
    super::look_measure::InkScale::for_config(config, Path::new(&args.scene))
}

/// The glyph grid the last frame drew, as ink; `None` when ASCII didn't draw.
fn frame_ink(
    renderer: &SceneRenderer,
    ctx: &HeadlessContext,
    scale: &super::look_measure::InkScale,
) -> Option<super::look_measure::InkGrid> {
    renderer
        .style_text(&ctx.device, "ascii")
        .map(|t| super::look_measure::InkGrid::from_text(&t, scale))
}

/// `--ai-refine` (#357): run the loop, then write the refined look, its log
/// and the sheet of rounds.
fn refine_look(
    args: &RenderArgs,
    look: super::look::Look,
    reference: super::look_measure::Reference,
    draw: &mut dyn FnMut(&toml::Table) -> Result<super::look_refine::Frame>,
) -> Result<()> {
    use super::look_refine as lr;
    let (agent, meter) = lr::build_agent(
        &args.refine.ai_provider,
        args.refine.ai_model.as_deref(),
        args.refine.max_cost,
    )?;
    let name = format!("{}_refined", look.name);
    let out = args
        .refine
        .refine_out
        .clone()
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| lr::default_out(&args.output, &name));
    let reference_path = args.measure.reference.clone().unwrap_or_default();
    let log = lr::run(
        lr::Options {
            look: look.clone(),
            reference_path,
            reference,
            rounds: args.refine.refine_rounds.max(1),
            candidates: args.refine.refine_candidates.max(1),
        },
        agent.as_ref(),
        &meter,
        draw,
    )?;
    let text = lr::refined_look_toml(&log, &look, &name)?;
    if let Some(dir) = out.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir).ok();
    }
    std::fs::write(&out, text).context(format!("Failed to write {}", out.display()))?;
    let log_path = out.with_extension("json");
    std::fs::write(&log_path, serde_json::to_string_pretty(&log)?)
        .context(format!("Failed to write {}", log_path.display()))?;
    let (labels, cells): (Vec<String>, Vec<image::RgbaImage>) = log.frames.iter().cloned().unzip();
    let sheet = super::style_sheet::compose(
        &cells,
        &super::style_sheet::SheetText::new(
            format!("refine {} toward {}", look.name, log.reference),
            format!(
                "{}: distance {:.2} -> {:.2}, {} round(s), {} call(s), ${:.4}",
                log.agent, log.start.score, log.end.score, log.rounds_run, log.calls, log.cost_usd
            ),
            labels,
        ),
    );
    sheet
        .save(&args.output)
        .context(format!("Failed to save image to {}", args.output))?;
    say!(
        "Refined {}: distance {:.2} -> {:.2} ({}), {} call(s), ${:.4}",
        look.name,
        log.start.score,
        log.end.score,
        log.stopped,
        log.calls,
        log.cost_usd
    );
    say!(
        "  by {}: start {}; end {}",
        log.metric.name(),
        log.start.terms_line(),
        log.end.terms_line()
    );
    say!(
        "Wrote {} (load it with --look {})",
        out.display(),
        out.display()
    );
    say!(
        "Wrote {} and the rounds' sheet {}",
        log_path.display(),
        args.output
    );
    Ok(())
}

/// What a `--frames` sequence draws with.
struct SequenceRig<'a> {
    ctx: &'a HeadlessContext,
    renderer: &'a mut SceneRenderer,
    world: &'a mut flint_ecs::FlintWorld,
    camera: Camera,
    camera_path: Option<&'a super::frame_seq::CameraPath>,
    particles: Option<&'a mut flint_particles::ParticleSystem>,
    /// `--anim-time` (#307): steps on a frame at a time from there
    animation: Option<&'a mut HeadlessAnimation>,
    /// `--mesh` (#222): each frame shows the next file on each binding
    meshes: &'a [super::mesh_seq::MeshBinding],
    scene_name: &'a str,
}

/// Where a `--mesh` sequence's frames go: a PNG a frame (`seq.png` numbered as
/// --frames numbers them, or `frames/f-%05d.png`), one GIF, or with `--sheet`
/// one contact sheet.
fn check_mesh_output(args: &RenderArgs, files: usize) -> Result<()> {
    let out = &args.output;
    let moving = render_plan::is_gif(out) || render_plan::is_sequence(out);
    if args.seq.sheet && moving {
        anyhow::bail!("--sheet writes one image: give --output a plain .png, not {out}");
    }
    if render_plan::is_gif(out) && files < 2 {
        anyhow::bail!("--mesh matched one file, and a GIF of one frame is a still: -o x.png");
    }
    if args.seq.no_images && (moving || args.seq.sheet) {
        anyhow::bail!("--no-images writes no frames, so {out} would be empty: drop one");
    }
    Ok(())
}

/// Point `binding`'s entity at the asset its frames are uploaded under. Its
/// `model` is marked `runtime`, so the scene's model loader leaves it to the
/// sequence; an entity with no model gets one.
fn bind_mesh(
    world: &mut flint_ecs::FlintWorld,
    binding: &super::mesh_seq::MeshBinding,
) -> Result<()> {
    use flint_core::components::MODEL;
    let Some(id) = world.get_id(&binding.entity) else {
        let mut names: Vec<String> = world
            .all_entities()
            .iter()
            .filter(|e| {
                world
                    .get_components(e.id)
                    .is_some_and(|c| c.get(MODEL).is_some())
            })
            .map(|e| e.name.clone())
            .collect();
        names.sort();
        let known = if names.is_empty() {
            String::new()
        } else {
            format!(" (entities with a model: {})", names.join(", "))
        };
        anyhow::bail!(
            "--mesh: no entity named '{}' in the scene{known}",
            binding.entity
        );
    };
    let mut model = world
        .get_components(id)
        .and_then(|c| c.get(MODEL))
        .and_then(|m| m.as_table().cloned())
        .unwrap_or_default();
    model.insert("asset".into(), binding.asset().into());
    model.insert("runtime".into(), true.into());
    world
        .set_component(id, MODEL, toml::Value::Table(model))
        .map_err(|e| anyhow::anyhow!("--mesh {}: {e}", binding.entity))?;
    Ok(())
}

/// Upload frame `index` of every binding in place of the frame before it,
/// whose namespaced textures go first so that no draw binds the old ones.
fn swap_meshes(
    ctx: &HeadlessContext,
    renderer: &mut SceneRenderer,
    meshes: &[super::mesh_seq::MeshBinding],
    index: usize,
    textures: &mut Vec<Vec<String>>,
) -> Result<()> {
    textures.resize(meshes.len(), Vec::new());
    for (binding, old) in meshes.iter().zip(textures.iter_mut()) {
        let file = &binding.files[index];
        let import = flint_import::import_gltf(file)
            .with_context(|| format!("--mesh {}: importing {}", binding.entity, file.display()))?;
        let import = super::mesh_seq::flatten(import);
        let asset = binding.asset();
        for texture in old.drain(..) {
            renderer.remove_texture(&texture);
        }
        renderer.remove_mesh(&asset);
        renderer.load_model(&ctx.device, &ctx.queue, &asset, &import);
        *old = import
            .materials
            .iter()
            .flat_map(|m| {
                [
                    &m.base_color_texture,
                    &m.emissive_texture,
                    &m.normal_texture,
                    &m.metallic_roughness_texture,
                ]
            })
            .flatten()
            // A texture used as colour and as data is uploaded twice, the
            // data copy suffixed (namespace_textures); a missing name is a no-op
            .flat_map(|t| [format!("{asset}::{t}"), format!("{asset}::{t}#linear")])
            .collect();
    }
    Ok(())
}

/// Where a sequence writes its frames. `--frames` alone always numbers PNGs
/// beside --output, as it did; a `--mesh` sequence also takes `%05d`, a GIF
/// and `--sheet`.
#[derive(Clone, Copy, PartialEq)]
enum MeshOutput {
    /// `seq.png` -> `seq_0000.png`...
    Numbered,
    /// `frames/f-%05d.png`
    Pattern,
    Gif,
    Sheet,
}

impl MeshOutput {
    fn of(args: &RenderArgs, mesh: bool) -> Self {
        match () {
            _ if !mesh => Self::Numbered,
            _ if args.seq.sheet => Self::Sheet,
            _ if render_plan::is_gif(&args.output) => Self::Gif,
            _ if render_plan::is_sequence(&args.output) => Self::Pattern,
            _ => Self::Numbered,
        }
    }

    fn path(self, output: &str, index: u32, frames: u32) -> std::path::PathBuf {
        match self {
            Self::Pattern => render_plan::sequence_path(output, index as usize).into(),
            _ => super::frame_seq::numbered(output, index, frames),
        }
    }
}

/// A frame's name on a `--mesh` sheet: its file's, or each binding's joined.
fn mesh_label(meshes: &[super::mesh_seq::MeshBinding], index: usize) -> String {
    meshes
        .iter()
        .map(|b| super::mesh_seq::label(&b.files[index]))
        .collect::<Vec<_>>()
        .join(" / ")
}

/// `--frames N` (#345): N frames on one renderer, `--frame-dt` apart, the
/// camera moved between them, each written as a numbered PNG (and grid
/// text), with the glyph flips counted when asked.
fn render_sequence(args: &RenderArgs, rig: SequenceRig, frames: u32) -> Result<()> {
    use super::frame_seq::{numbered, FlipCounter};
    let SequenceRig {
        ctx,
        renderer,
        world,
        mut camera,
        camera_path,
        mut particles,
        mut animation,
        meshes,
        scene_name,
    } = rig;
    // A mesh sequence's frames are --fps apart, as its GIF plays them
    let dt = args.seq.frame_dt.unwrap_or(if meshes.is_empty() {
        1.0 / 60.0
    } else {
        1.0 / args.clock.fps
    });
    let wants_flips = args.seq.flip_stats || args.seq.flip_json.is_some();
    let wants_text = args.out.ascii_text.is_some() || args.out.ascii_text_ansi.is_some();
    let base_time = still_time(args);
    let yaw0 = camera.yaw;
    let (width, height) = (ctx.width, ctx.height);
    let mut mesh_textures = Vec::new();
    // Where each frame goes: a numbered PNG, a `%05d` PNG, one GIF, or a sheet
    let out = MeshOutput::of(args, !meshes.is_empty());
    let mut sheet_cells = Vec::new();
    if !args.seq.no_images {
        if let Some(dir) = Path::new(&args.output).parent() {
            if !dir.as_os_str().is_empty() {
                std::fs::create_dir_all(dir)
                    .context(format!("Failed to create {}", dir.display()))?;
            }
        }
    }
    let mut gif = match out {
        MeshOutput::Gif => Some(super::gif_writer::Spool::new(
            Path::new(&args.output),
            width,
            height,
            frames as usize,
        )?),
        _ => None,
    };
    let number = |p: &Option<String>, i: u32| {
        p.as_ref()
            .map(|p| numbered(p, i, frames).to_string_lossy().into_owned())
    };
    let mut flips = FlipCounter::default();
    let motion = match (args.seq.orbit, camera_path) {
        (Some(rate), _) => format!(", orbiting {rate} deg/s"),
        (None, Some(path)) => format!(", on a {}-key camera path", path.key.len()),
        _ => String::new(),
    };
    say!("Sequence: {frames} frames, {dt:.6} s apart{motion}");
    for i in 0..frames {
        let t = i as f32 * dt;
        if let Some(rate) = args.seq.orbit {
            camera.yaw = yaw0 + (rate * t).to_radians();
            camera.update_orbit();
        } else if let Some(path) = camera_path {
            let (position, target) = path.at(t);
            camera.position = position;
            camera.target = target;
        }
        // The post time and held poses (#405) on the frame's clock
        let time = base_time + t as f64;
        renderer.set_motion_time(time);
        if let Some(posed) = animation.as_deref_mut() {
            if i > 0 {
                posed.step(world, dt as f64);
                posed.upload(renderer, &ctx.device, &ctx.queue);
            }
        }
        if let Some(p) = particles.as_deref_mut() {
            if i > 0 {
                p.simulate_to(world, dt, dt.min(1.0 / 60.0));
            }
            p.pack(Some(camera.position_array()));
        }
        if !meshes.is_empty() {
            swap_meshes(ctx, renderer, meshes, i as usize, &mut mesh_textures)?;
        }
        // The frame's inputs, and the world read again: the draw lists hold
        // the poses and meshes they were built with (#739)
        renderer.prepare(
            &ctx.device,
            &ctx.queue,
            world,
            &FrameInputs {
                // The grass's wind from the sequence's start, as it always ran
                grass_time: Some(t),
                // History smoothing scales by the frame's game time (#342)
                dt,
                particles: particles.as_deref().map(|p| &p.sync),
                ..still_inputs(args, &camera, time)
            },
        );
        renderer.render_frame_to(
            &ctx.device,
            &ctx.queue,
            ctx.depth_target(),
            &camera,
            &ctx.color_view,
            Some(world),
        );
        if wants_text || wants_flips {
            let text = renderer.style_text(&ctx.device, "ascii");
            if wants_flips {
                let Some(text) = &text else {
                    anyhow::bail!(
                        "--flip-stats: the ASCII style didn't draw frame {i}; turn it on with \
                         --style ascii, a look, or the scene's [post_process.styles.ascii]"
                    );
                };
                let f = flips.push(text);
                if args.seq.flip_stats {
                    match (f.flips, f.reversals) {
                        (Some(n), Some(r)) => {
                            say!("  frame {i:>4}: {n:>6} flips {r:>6} reversals")
                        }
                        (Some(n), None) => say!("  frame {i:>4}: {n:>6} flips"),
                        _ => {}
                    }
                }
            }
            if wants_text {
                write_ascii_text(
                    number(&args.out.ascii_text, i).as_deref(),
                    number(&args.out.ascii_text_ansi, i).as_deref(),
                    text,
                )?;
            }
        }
        // --last-frame (#478) reads back only the frame it writes
        let kept = !args.seq.no_images && (!args.seq.last_frame || i + 1 == frames);
        if kept {
            let pixels =
                pollster::block_on(ctx.read_pixels()).context("Failed to read rendered pixels")?;
            let img = image::RgbaImage::from_raw(width, height, pixels)
                .context("Failed to create image from pixel data")?;
            match out {
                _ if args.seq.last_frame => {
                    img.save(&args.output)
                        .context(format!("Failed to save image to {}", args.output))?;
                }
                MeshOutput::Numbered | MeshOutput::Pattern => {
                    let path = out.path(&args.output, i, frames);
                    img.save(&path)
                        .context(format!("Failed to save image to {}", path.display()))?;
                }
                MeshOutput::Gif => {
                    let rgb = image::DynamicImage::ImageRgba8(img).into_rgb8();
                    gif.as_mut().expect("made above").push(&rgb)?;
                }
                MeshOutput::Sheet => sheet_cells.push(img),
            }
        }
    }
    if args.seq.last_frame {
        say!(
            "Rendered frame {} of {frames} ({width}x{height}) to {}",
            frames - 1,
            args.output
        );
    } else if !args.seq.no_images {
        match out {
            MeshOutput::Numbered | MeshOutput::Pattern => say!(
                "Rendered {frames} {width}x{height} frames to {} .. {}",
                out.path(&args.output, 0, frames).display(),
                out.path(&args.output, frames - 1, frames).display()
            ),
            MeshOutput::Gif => {
                let fps = 1.0 / dt;
                if fps > render_plan::GIF_MAX_FPS {
                    eprintln!(
                        "warning: --frame-dt {dt} is {fps:.0} fps; the GIF plays at \
                         {} (its delays count hundredths of a second)",
                        render_plan::GIF_MAX_FPS
                    );
                }
                gif.take()
                    .expect("made above")
                    .finish(fps.min(render_plan::GIF_MAX_FPS))?;
                say!(
                    "Rendered {frames} {width}x{height} frames at {fps:.1} fps to {}",
                    args.output
                );
            }
            MeshOutput::Sheet => {
                let labels = (0..frames as usize)
                    .map(|i| mesh_label(meshes, i))
                    .collect();
                let bound: Vec<String> = meshes
                    .iter()
                    .map(|b| format!("{} <- {} files", b.entity, b.files.len()))
                    .collect();
                let text = super::style_sheet::SheetText::new(
                    scene_name.to_string(),
                    format!("--mesh {}", bound.join(", ")),
                    labels,
                );
                let sheet = super::style_sheet::compose(&sheet_cells, &text);
                sheet
                    .save(&args.output)
                    .context(format!("Failed to save image to {}", args.output))?;
                say!(
                    "Rendered {frames} {width}x{height} frames as one {}x{} sheet to {}",
                    sheet.width(),
                    sheet.height(),
                    args.output
                );
            }
        }
    }
    if wants_flips {
        let stats = flips.stats();
        say!("Flips: {}", stats.summary());
        if let Some(path) = &args.seq.flip_json {
            std::fs::write(path, serde_json::to_string_pretty(&stats)?)
                .context(format!("Failed to write {path}"))?;
            say!("Wrote flip counts to {path}");
        }
    }
    Ok(())
}

/// Write the ASCII grid (#317) to --ascii-text / --ascii-text-ansi.
fn write_ascii_text(
    text_path: Option<&str>,
    ansi_path: Option<&str>,
    text: Option<flint_render::stylize::GlyphText>,
) -> Result<()> {
    let Some(text) = text else {
        anyhow::bail!(
            "--ascii-text: the ASCII style didn't draw this frame; turn it on with --ascii, \
             a look, or the scene's [post_process.styles.ascii] / ascii_style entities"
        );
    };
    if let Some(path) = text_path {
        std::fs::write(path, text.to_text()).context(format!("Failed to write {path}"))?;
        say!("Wrote {}x{} ASCII grid to {path}", text.cols, text.rows);
    }
    if let Some(path) = ansi_path {
        std::fs::write(path, text.to_ansi()).context(format!("Failed to write {path}"))?;
        say!(
            "Wrote {}x{} ANSI ASCII grid to {path}",
            text.cols,
            text.rows
        );
    }
    Ok(())
}

/// What `run_in` hands the stage path, worked out already: the schemas, the
/// scene as it loaded it (the first cell's world), the sweep and every cell's
/// post-processing, and what the one-renderer path would have drawn with.
struct StagedRender<'a> {
    registry: &'a SchemaRegistry,
    /// The scene as each stage's renderer binds it (#739).
    binding: &'a flint_render::SceneBinding,
    first: (flint_ecs::FlintWorld, flint_scene::SceneFile),
    grid: Option<&'a super::sweep::SweepGrid>,
    /// A cell's each, a row at a time, as [`super::sweep::SweepGrid::cells`].
    configs: Vec<flint_render::PostProcessConfig>,
    report_json: bool,
    /// What a sweep's cell is of its frame (#358).
    view: super::sweep::CellView,
    /// The scene's `[post_process]`, under its own look, for `--ai-refine`.
    scene_pp: Option<&'a flint_scene::PostProcessDef>,
    look: Option<super::look::Look>,
    reference: Option<super::look_measure::Reference>,
    camera_path: Option<&'a super::frame_seq::CameraPath>,
}

/// Whether game time runs through the captures: a `--time`, or `--frames`
/// `--frame-dt` apart (#596).
fn on_clock(args: &RenderArgs) -> bool {
    args.clock.time.is_some() || args.seq.frames.is_some()
}

/// A render with clocks (#216): a [`Stage`] a sweep cell, each captured at
/// every moment asked for, the cells composed into one sheet a moment (a row,
/// or a grid for two sweeps), and a span written as a GIF or a PNG a frame.
fn run_stages(args: &RenderArgs, host: &mut RenderHost, staged: StagedRender) -> Result<()> {
    use std::hash::{Hash, Hasher};
    let StagedRender {
        registry,
        binding,
        first,
        grid,
        configs,
        report_json,
        view,
        scene_pp,
        look,
        reference,
        camera_path,
    } = staged;

    // A span or a list as one contact sheet (#574): neither a GIF nor stills
    let strip = args.strip.strip;
    let animated = !strip && matches!(args.clock.time, Some(TimeSpec::Range(..)));
    // A still a moment (#614), numbered in the order the moments were given.
    let moments = !strip && matches!(args.clock.time, Some(TimeSpec::List(_)));
    let report_file = args.measure.report.as_deref().filter(|_| !report_json);
    let measured = reference.is_some() || report_file.is_some();
    // What a clocked render can't mean (#596), each refused with why
    if measured && (animated || moments || strip) {
        anyhow::bail!(
            "--report FILE / --reference measure one moment, and this --time captures \
             several: give --time T (--report json reports every capture's props)"
        );
    }
    if args.refine.ai_refine && (animated || moments) {
        anyhow::bail!("--ai-refine refines a look at one moment: give --time T");
    }
    if args.refine.ai_refine && report_json {
        anyhow::bail!("--ai-refine writes its own log beside the refined look: drop --report json");
    }
    let wants_text = args.out.ascii_text.is_some() || args.out.ascii_text_ansi.is_some();
    let wants_flips = args.seq.flip_stats || args.seq.flip_json.is_some();
    let reads_glyphs = reference.as_ref().is_some_and(|r| r.ink.is_some());

    let cells = grid.map_or(vec![(0, 0)], |g| g.cells());
    let count = cells.len();

    // The scene, once for each cell: a world is changed by loading it
    let mut scenes = Vec::with_capacity(count);
    scenes.push(first);
    for _ in 1..count {
        scenes.push(load_settled(&args.scene, registry)?);
    }
    // An echoes column is its scene with the column's [living] echoes (#531),
    // written before anything reads the table
    if let Some(grid) = grid.filter(|g| g.is_living()) {
        for ((_, scene_file), &(column, row)) in scenes.iter_mut().zip(&cells) {
            grid.lay_living(column, row, &mut scene_file.tables);
        }
    }
    let scene_name = scenes[0].1.scene.name.clone();

    let (width, height) = match grid {
        // Cells drawn small, or at the full size to be cropped or shrunk
        Some(_) => view.render_size(args.width, args.height),
        // A refine's rounds are sheet cells
        // A refine's rounds are sheet cells, and so are a strip's tiles
        None if args.refine.ai_refine || strip => {
            let (w, h) = super::sweep::CELL_SIZE;
            (args.width.unwrap_or(w), args.height.unwrap_or(h))
        }
        None => {
            let (w, h) = render_plan::default_size(false, animated);
            (args.width.unwrap_or(w), args.height.unwrap_or(h))
        }
    };
    let frame_step = args
        .clock
        .frame_step
        .unwrap_or(render_plan::DEFAULT_FRAME_STEP);

    let device = load_report::span(load_report::Stage::Device, String::new);
    let ctx = host.context(width, height, &args.debug.gpu)?;
    drop(device);

    let timeline = match &args.clock.expression_timeline {
        Some(path) => Some(ExpressionTimeline::load(path)?),
        None => None,
    };
    if let Some(timeline) = &timeline {
        say!("Expression timeline: {} states", timeline.len());
    }

    let mut stages = Vec::with_capacity(count);
    for (index, (((world, scene_file), config), &(column, row))) in
        scenes.into_iter().zip(configs).zip(&cells).enumerate()
    {
        stages.push(Stage::load(
            args,
            ctx,
            StageScene {
                world,
                scene_file,
                binding,
                index,
                cell: (column, row),
                grid,
                config,
                look: stage_look(args, grid, (column, row), scene_pp),
                timeline: timeline.clone(),
                resolve_procgen: false,
            },
        )?);
    }

    // The load is up; its last stage is the first capture (#1092)
    load_report::bring_up_done();
    let mut load = None;

    // --ai-refine (#357) at a moment: each candidate is the one stage redrawn
    // at that moment under the candidate's look, its clocks never rewound
    if args.refine.ai_refine {
        let look = look.expect("clap requires --look with --ai-refine");
        let reference = reference.expect("clap requires --reference with --ai-refine");
        let t = match args.clock.time {
            Some(TimeSpec::At(t)) => t,
            _ => 0.0,
        };
        let stage = &mut stages[0];
        return refine_look(args, look, reference, &mut |table| {
            let def = super::look::overlay(scene_pp, table)?;
            let config = post_process_config(args, Some(&def), stage.renderer.styles())?;
            let scale = reads_glyphs.then(|| ink_scale(args, &config));
            let image = stage.redraw(args, ctx, t, frame_step, config)?;
            let ink = scale.and_then(|s| frame_ink(&stage.renderer, ctx, &s));
            Ok(super::look_refine::Frame { image, ink })
        });
    }

    // --frames (#345) on the clock: a frame every --frame-dt of game time
    let frame_dt = args.seq.frame_dt.map_or(1.0 / 60.0, f64::from);
    let times = match args.seq.frames {
        Some(frames) => (0..frames).map(|i| f64::from(i) * frame_dt).collect(),
        None if strip => render_plan::strip_times(args.clock.time.as_ref(), args.strip.every)?,
        None => render_plan::capture_times(args.clock.time.as_ref(), args.clock.fps),
    };
    if let Some(frames) = args.seq.frames {
        let motion = match (args.seq.orbit, camera_path) {
            (Some(rate), _) => format!(", orbiting {rate} deg/s"),
            (None, Some(path)) => format!(", on a {}-key camera path", path.key.len()),
            _ => String::new(),
        };
        say!("Sequence: {frames} frames, {frame_dt:.6} s apart{motion}");
    }
    let yaw0: Vec<f32> = stages.iter().map(|stage| stage.camera.yaw).collect();

    let mut warnings = stage_warnings(args, grid, &stages[0]);
    for warning in &warnings {
        eprintln!("Warning: {warning}");
    }

    let output = Path::new(&args.output);
    // Where each frame goes as it's captured, so a span costs memory for one
    // frame however long it runs (#588).
    let sequence = animated && render_plan::is_sequence(&args.output);
    let mut gif = None;
    let mut still = None;
    // A strip's tiles, composed once the last is captured
    let mut tiles = Vec::new();
    let mut written_bytes = 0u64;
    let (mut frame_width, mut frame_height) = (width, height);
    let mut captures = Vec::new();
    let mut flips = super::frame_seq::FlipCounter::default();
    // Each cell's frames so far, hashed: neighbours that drew the same at every
    // moment showed nothing
    let mut drawn: Vec<std::collections::hash_map::DefaultHasher> =
        (0..count).map(|_| Default::default()).collect();
    // A moment's measures; there is one moment when there are any (#357)
    let mut columns = Vec::new();
    // A grid's text, a frame's each when there are several (#317)
    let text_path = |path: &Option<String>, index: usize| {
        path.as_ref().map(|p| match times.len() {
            1 => p.clone(),
            n => super::frame_seq::numbered(p, index as u32, n as u32)
                .to_string_lossy()
                .into_owned(),
        })
    };
    for (index, &t) in times.iter().enumerate() {
        let mut pictures = Vec::with_capacity(count);
        let mut texts = Vec::with_capacity(count);
        for (i, stage) in stages.iter_mut().enumerate() {
            stage.advance(ctx, t, frame_step);
            // A strip's tile is --time t alone: a first frame, whatever came before
            if strip {
                stage.forget_history();
            }
            // The camera's move, over where the scripts left it
            if args.seq.frames.is_some() {
                if let Some(rate) = args.seq.orbit {
                    stage.camera.yaw = yaw0[i] + (rate * t as f32).to_radians();
                    stage.camera.update_orbit();
                } else if let Some(path) = camera_path {
                    let (position, target) = path.at(t as f32);
                    stage.camera.position = position;
                    stage.camera.target = target;
                }
            }
            // The frame's description (#1084) and capture (#1085): the
            // first cell's last capture
            let dump = super::render_frame_dump::Wanted::of(args)
                .filter(|_| i == 0 && index + 1 == times.len())
                .map(|wanted| {
                    (
                        wanted,
                        super::render_frame_dump::arm(ctx, &mut stage.renderer, wanted),
                    )
                });
            pictures.push(stage.capture(args, ctx, t, frame_step)?);
            load = load.or_else(first_capture_done);
            if let Some((wanted, armed)) = dump {
                let said = super::render_frame_dump::write(
                    armed,
                    ctx,
                    &mut stage.renderer,
                    Some(stage.game.world()),
                    wanted,
                    super::render_frame_dump::Origin {
                        scene: &args.scene,
                        game_time: Some(t),
                        written_by: "flint render",
                        notes: Vec::new(),
                    },
                    stage.replay_inputs.take().map(|inputs| {
                        super::render_frame_dump::ReplayFacts {
                            inputs,
                            camera: &stage.camera,
                            particles: Some(&stage.game.particles().sync),
                        }
                    }),
                )?;
                for line in said {
                    say!("{line}");
                }
            }
            if wants_text || wants_flips || report_file.is_some() || reads_glyphs {
                texts.push(stage.renderer.style_text(&ctx.device, "ascii"));
            }
            if report_json {
                let (column, row) = cells[i];
                captures.push(CaptureReport {
                    time: t,
                    column: grid.map(|g| g.columns.values[column].clone()),
                    row: grid
                        .and_then(|g| g.rows.as_ref())
                        .map(|rows| rows.values[row].clone()),
                    distance: None,
                    expression: stage.scripted.then_some(()).and_then(|()| {
                        let state = stage.game.expression().get("state")?.clone();
                        state.into_string().ok()
                    }),
                    props: stage.game.living_props().map_or_else(Vec::new, |props| {
                        render_report::props_report(props, stage.game.wooze())
                    }),
                });
            }
        }
        for (hasher, picture) in drawn.iter_mut().zip(&pictures) {
            picture.as_raw().hash(hasher);
        }

        // The ASCII grid as text, and its flips frame to frame. A sweep is
        // refused text before anything loads, so this is the one stage's
        if wants_flips {
            let Some(text) = &texts[0] else {
                anyhow::bail!(
                    "--flip-stats: the ASCII style didn't draw frame {index}; turn it on with \
                     --style ascii, a look, or the scene's [post_process.styles.ascii]"
                );
            };
            let f = flips.push(text);
            if args.seq.flip_stats {
                match (f.flips, f.reversals) {
                    (Some(n), Some(r)) => say!("  frame {index:>4}: {n:>6} flips {r:>6} reversals"),
                    (Some(n), None) => say!("  frame {index:>4}: {n:>6} flips"),
                    _ => {}
                }
            }
        }
        if wants_text {
            write_ascii_text(
                text_path(&args.out.ascii_text, index).as_deref(),
                text_path(&args.out.ascii_text_ansi, index).as_deref(),
                texts[0].clone(),
            )?;
        }

        // Each cell as the sheet shows it (cropped or shrunk, #358), and measured
        // from those pixels as the one-renderer path measures
        // A crop's first frame stays whole for the sheet's locator (#501)
        let mut locator = None;
        if grid.is_some() {
            locator = pictures.first().and_then(|p| view.locator(p));
            pictures = pictures.into_iter().map(|p| view.cell(p)).collect();
        }
        if measured {
            let used: Vec<_> = stages
                .iter()
                .map(|stage| stage.renderer.post_process_config().clone())
                .collect();
            columns = measure_cells(
                args,
                &pictures,
                &texts,
                &used,
                reference.as_ref(),
                grid,
                &cells,
            );
            let first = captures.len().saturating_sub(count);
            for (capture, column) in captures[first..].iter_mut().zip(&columns) {
                capture.distance = column.distance.as_ref().map(|d| d.score);
            }
        }
        let nearest = super::look_measure::RenderReport::nearest_column(&columns);

        let frame = match grid {
            Some(grid) => stage_sheet(
                grid,
                StageSheet {
                    view,
                    width,
                    height,
                    scene_name: &scene_name,
                    t,
                    reference: args.measure.reference.as_deref(),
                    columns: &columns,
                    nearest,
                    locator,
                },
                &stages,
                &pictures,
            ),
            None => pictures.remove(0),
        };
        (frame_width, frame_height) = frame.dimensions();
        if let Some(frames) = args.seq.frames {
            if args.seq.last_frame {
                // #478: only where the sequence ends, to --output itself
                if index as u32 + 1 == frames {
                    if let Some(parent) = output.parent().filter(|p| !p.as_os_str().is_empty()) {
                        std::fs::create_dir_all(parent)
                            .with_context(|| format!("failed to create {}", parent.display()))?;
                    }
                    frame
                        .save(output)
                        .with_context(|| format!("Failed to save image to {}", args.output))?;
                }
            } else if !args.seq.no_images {
                let path = super::frame_seq::numbered(&args.output, index as u32, frames);
                if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
                    std::fs::create_dir_all(parent)
                        .with_context(|| format!("failed to create {}", parent.display()))?;
                }
                frame
                    .save(&path)
                    .with_context(|| format!("Failed to save image to {}", path.display()))?;
            }
        } else if sequence {
            let path = render_plan::sequence_path(&args.output, index);
            if let Some(parent) = Path::new(&path)
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
            {
                std::fs::create_dir_all(parent)
                    .with_context(|| format!("failed to create {}", parent.display()))?;
            }
            frame
                .save(&path)
                .with_context(|| format!("Failed to save frame {index} to {path}"))?;
            written_bytes += std::fs::metadata(&path).map_or(0, |m| m.len());
        } else if strip {
            tiles.push(frame);
        } else if moments {
            let path = render_plan::numbered_still(&args.output, index, times.len());
            frame
                .save(&path)
                .context(format!("Failed to save image to {}", path.display()))?;
        } else if animated {
            let frame = image::DynamicImage::ImageRgba8(frame).into_rgb8();
            let spool = match &mut gif {
                Some(spool) => spool,
                None => gif.insert(gif_writer::Spool::new(
                    output,
                    frame.width(),
                    frame.height(),
                    times.len(),
                )?),
            };
            spool.push(&frame)?;
        } else {
            still = Some(frame);
        }
    }

    // A span's clocks run on to its end (the end itself is never a frame, so that
    // the GIF loops), so what the scripts log covers all of it.
    if let Some(TimeSpec::Range(_, end)) = args.clock.time {
        for stage in &mut stages {
            stage.advance(ctx, end, frame_step);
        }
    }

    // What the scripts asked for that isn't in the picture, said once as it
    // happened, and kept for the report.
    if stages[0].scripted {
        warnings.extend(stages[0].game.notes());
    }

    if let Some(grid) = grid {
        let drawn: Vec<u64> = drawn.iter().map(Hasher::finish).collect();
        for warning in grid.identical_warnings(&drawn) {
            eprintln!("Warning: {warning}");
            warnings.push(warning);
        }
    }

    // Write the file
    if let Some(frames) = args.seq.frames {
        if args.seq.last_frame {
            say!(
                "Rendered frame {} of {frames} ({frame_width}x{frame_height}) to {}",
                frames - 1,
                args.output
            );
        } else if !args.seq.no_images {
            say!(
                "Rendered {frames} {frame_width}x{frame_height} frames to {} .. {}",
                super::frame_seq::numbered(&args.output, 0, frames).display(),
                super::frame_seq::numbered(&args.output, frames - 1, frames).display()
            );
        }
    } else if sequence {
        say!(
            "Rendered {} frames of {frame_width}x{frame_height} at {} fps to {} ({:.1} MB)",
            times.len(),
            args.clock.fps,
            args.output,
            written_bytes as f64 / 1e6,
        );
    } else if strip {
        let sheet = strip_sheet(args, &scene_name, &times, &tiles, frame_step);
        if let Some(parent) = output.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("failed to create {}", parent.display()))?;
        }
        sheet
            .save(output)
            .with_context(|| format!("Failed to save the strip to {}", args.output))?;
        say!(
            "Rendered {} tiles of {frame_width}x{frame_height} ({} .. {}) as one {}x{} strip to {}",
            tiles.len(),
            render_plan::strip_label(times[0]),
            render_plan::strip_label(times[times.len() - 1]),
            sheet.width(),
            sheet.height(),
            args.output
        );
    } else if animated {
        let spool = gif.context("a span captured no frames")?;
        let (frames, spilled) = (spool.frames(), spool.spilled());
        spool.finish(args.clock.fps)?;
        say!(
            "Rendered {frames} frames of {frame_width}x{frame_height} at {} fps to {} \
             ({:.1} MB; {:.1} MB waited on disk)",
            args.clock.fps,
            args.output,
            std::fs::metadata(output).map_or(0.0, |m| m.len() as f64 / 1e6),
            spilled as f64 / 1e6,
        );
    } else if moments {
        let count = times.len();
        say!(
            "Rendered {count} moments ({}) as {frame_width}x{frame_height} to {} .. {}",
            times
                .iter()
                .map(|t| format!("{t}"))
                .collect::<Vec<_>>()
                .join(", "),
            render_plan::numbered_still(&args.output, 0, count).display(),
            render_plan::numbered_still(&args.output, count - 1, count).display()
        );
    } else {
        let still = still.context("the render captured no frame")?;
        still
            .save(&args.output)
            .context(format!("Failed to save image to {}", args.output))?;
        match grid {
            Some(grid) => {
                let (across, down) = grid.shape();
                let shape = match grid.rows {
                    Some(_) => format!("{across}x{down} cells"),
                    None => format!("{across} columns"),
                };
                say!(
                    "Rendered {shape} of {width}x{height} as {frame_width}x{frame_height} to {}",
                    args.output
                )
            }
            None => say!("Rendered {width}x{height} image to {}", args.output),
        }
    }

    if wants_flips {
        let stats = flips.stats();
        say!("Flips: {}", stats.summary());
        if let Some(path) = &args.seq.flip_json {
            std::fs::write(path, serde_json::to_string_pretty(&stats)?)
                .context(format!("Failed to write {path}"))?;
            say!("Wrote flip counts to {path}");
        }
    }

    if let Some(path) = report_file {
        let nearest = super::look_measure::RenderReport::nearest_column(&columns);
        write_measures(
            args,
            path,
            look.as_ref(),
            grid,
            (width, height),
            columns,
            nearest,
        )?;
    }

    // GPU timing (#596): the first cell's last capture, drawn again and again
    if let Some(frames) = args.measure.gpu_time {
        let stage = &mut stages[0];
        say!(
            "GPU time is of the first cell's capture at t = {:.3} s",
            stage.captured.unwrap_or(0.0)
        );
        time_gpu(
            ctx,
            &mut stage.renderer,
            &stage.camera,
            frames,
            args.measure.gpu_time_json.as_deref(),
        )?;
    }

    // Every script error of the run, each sweep cell's game folded in
    // (#693): once each on stderr, in the report, and a failure under
    // --strict-scripts
    let mut script_errors = flint_script::ScriptErrors::new();
    for stage in &stages {
        script_errors.merge(stage.game.script_errors());
    }
    let scripted = args.clock.scripts || args.clock.expression_timeline.is_some();
    if scripted {
        for line in script_errors.summary() {
            eprintln!("{line}");
        }
    }

    if report_json {
        let field = stages[0].game.wooze().map(|(field, _)| field);
        let report = render_report::RenderReport {
            scene: scene_name,
            output: args.output.clone(),
            width,
            height,
            frame_step,
            wooze_field: field.is_some(),
            wooze_zones: field.map_or(0, |f| f.zones().len()),
            sweep: grid.map(super::sweep::SweepGrid::name),
            frame_dump: args.measure.frame_dump.clone(),
            frame_capture: args.measure.frame_capture.clone(),
            captures,
            still_props: still_props(&stages[0]),
            warnings,
            script_errors: script_errors
                .all()
                .iter()
                .map(render_report::ScriptErrorReport::from)
                .collect(),
            style_drops: flint_render::stylize::variant_table::dropped(),
            load,
        };
        let json = serde_json::to_string(&report).context("serialising the render report")?;
        crate::output::document_text(&json);
    }
    if args.clock.strict_scripts {
        if let Some(why) = script_errors.strict_failure() {
            anyhow::bail!(why);
        }
    }
    Ok(())
}

/// A `--strip`'s sheet (#574): the tiles in rows of `--columns`, under the
/// scene's name and what was asked, each labelled with its time; or with
/// `--label off` the tiles alone.
fn strip_sheet(
    args: &RenderArgs,
    scene_name: &str,
    times: &[f64],
    tiles: &[image::RgbaImage],
    frame_step: f64,
) -> image::RgbaImage {
    let columns = render_plan::strip_columns(tiles.len(), args.strip.columns);
    if args.strip.label == super::render_args::StripLabel::Off {
        return super::style_sheet::compose_plain(tiles, columns);
    }
    let asked = match &args.clock.time {
        Some(TimeSpec::Range(from, to)) => format!(
            "--time {from}..{to}  a tile every {} s",
            args.strip.every.unwrap_or(render_plan::STRIP_EVERY)
        ),
        Some(TimeSpec::List(moments)) => format!(
            "--time {}",
            moments
                .iter()
                .map(|t| t.to_string())
                .collect::<Vec<_>>()
                .join(",")
        ),
        _ => String::new(),
    };
    let (w, h) = tiles.first().map_or((0, 0), image::RgbaImage::dimensions);
    let mut subtitle = format!("{asked}  {} tiles of {w}x{h}", tiles.len());
    if args.clock.scripts {
        subtitle.push_str("  scripts on");
    }
    if let Some(look) = &args.look {
        subtitle.push_str(&format!("  look {look}"));
    }
    if args.clock.frame_step.is_some() {
        subtitle.push_str(&format!("  frame step {frame_step} s"));
    }
    let mut text = super::style_sheet::SheetText::new(
        scene_name.to_string(),
        subtitle,
        times.iter().map(|&t| render_plan::strip_label(t)).collect(),
    );
    text.per_row = columns;
    super::style_sheet::compose(tiles, &text)
}

/// Measures (#357) of each cell as the sheet shows it, a line each in the
/// terminal. The ASCII section counts the whole drawn frame's grid, which a
/// crop shows only part of, and so does the glyph distance (#495).
fn measure_cells(
    args: &RenderArgs,
    pictures: &[image::RgbaImage],
    texts: &[Option<flint_render::stylize::GlyphText>],
    configs: &[flint_render::PostProcessConfig],
    reference: Option<&super::look_measure::Reference>,
    grid: Option<&super::sweep::SweepGrid>,
    cells: &[(usize, usize)],
) -> Vec<super::look_measure::ColumnReport> {
    use super::look_measure as lm;
    let mut columns = Vec::with_capacity(pictures.len());
    for (i, img) in pictures.iter().enumerate() {
        let image = lm::measure(img);
        let text = texts.get(i).cloned().flatten();
        let ascii = text
            .as_ref()
            .map(|t| lm::measure_ascii(t, lm::ascii_ramp(&configs[i], Path::new(&args.scene))));
        let distance = reference.map(|r| {
            let ink = r
                .ink
                .as_ref()
                .and(text.as_ref())
                .map(|t| lm::InkGrid::from_text(t, &ink_scale(args, &configs[i])));
            r.distance(img, ink.as_ref())
        });
        let value = grid.map(|g| {
            let (c, r) = cells[i];
            g.describe(c, r)
        });
        say!(
            "  [{}] {}{}",
            value.as_deref().unwrap_or("frame"),
            lm::summary(&image, ascii.as_ref()),
            distance
                .as_ref()
                .map(|d| format!(
                    "  distance {:.2} by {} ({})",
                    d.score,
                    d.metric.name(),
                    d.terms_line()
                ))
                .unwrap_or_default()
        );
        columns.push(lm::ColumnReport {
            index: i,
            value,
            image,
            ascii,
            distance,
        });
    }
    columns
}

/// A sweep's labels with each column's distance to the reference, the nearest
/// starred (#357); as they were without a reference.
fn distance_labels(
    labels: Vec<String>,
    columns: &[super::look_measure::ColumnReport],
    nearest: Option<usize>,
) -> Vec<String> {
    labels
        .into_iter()
        .enumerate()
        .map(
            |(i, l)| match columns.get(i).and_then(|c| c.distance.as_ref()) {
                Some(d) => {
                    let star = if nearest == Some(i) { " *" } else { "" };
                    format!("{l}  d {:.1}{star}", d.score)
                }
                None => l,
            },
        )
        .collect()
}

/// `--report FILE` (#357): the measures as JSON.
fn write_measures(
    args: &RenderArgs,
    path: &str,
    look: Option<&super::look::Look>,
    grid: Option<&super::sweep::SweepGrid>,
    (width, height): (u32, u32),
    columns: Vec<super::look_measure::ColumnReport>,
    nearest: Option<usize>,
) -> Result<()> {
    let report = super::look_measure::RenderReport {
        scene: args.scene.clone(),
        look: look.map(|l| l.name.clone()),
        sweep: grid.map(|g| g.name()),
        reference: args.measure.reference.clone(),
        width,
        height,
        thresholds: super::look_measure::Thresholds::USED,
        columns,
        nearest,
    };
    std::fs::write(path, serde_json::to_string_pretty(&report)?)
        .context(format!("Failed to write {path}"))?;
    say!("Wrote measures to {path}");
    Ok(())
}

/// What a stage sheet says beside its cells.
struct StageSheet<'a> {
    view: super::sweep::CellView,
    width: u32,
    height: u32,
    scene_name: &'a str,
    t: f64,
    reference: Option<&'a str>,
    columns: &'a [super::look_measure::ColumnReport],
    nearest: Option<usize>,
    /// Where a crop's cells sit in the frame (#501).
    locator: Option<super::style_sheet::Locator>,
}

/// One moment's sheet of a clocked sweep: a row of columns, or a grid for two
/// sweeps (#596), with the moment top right.
fn stage_sheet(
    grid: &super::sweep::SweepGrid,
    sheet: StageSheet,
    stages: &[Stage],
    pictures: &[image::RgbaImage],
) -> image::RgbaImage {
    // Only a sheet that isn't of small frames says how big they were, and only
    // one with a reference what its labels' d is
    let scale = match sheet.view {
        super::sweep::CellView::Small => String::new(),
        view => format!("  {}", view.describe(sheet.width, sheet.height)),
    };
    let reference = sheet
        .reference
        .map_or_else(String::new, |r| format!("  d = distance to {r}"));
    let label = |s: &super::sweep::Sweep| (0..s.values.len()).map(|i| s.label(i)).collect();
    match &grid.rows {
        None => {
            let mut text = sheet_text(&grid.columns, sheet.scene_name, sheet.t, stages);
            text.subtitle.push_str(&scale);
            text.subtitle.push_str(&reference);
            text.labels = distance_labels(text.labels, sheet.columns, sheet.nearest);
            text.locator = sheet.locator;
            super::style_sheet::compose(pictures, &text)
        }
        Some(rows) => super::style_sheet::compose_grid(
            pictures,
            &super::style_sheet::GridText {
                title: sheet.scene_name.to_string(),
                subtitle: format!(
                    "sweep {}{}{scale}{reference}",
                    grid.name(),
                    living_note(stages)
                ),
                row_key: rows.name(),
                column_labels: label(&grid.columns),
                row_labels: label(rows),
                time: format!("T {:.2}S", sheet.t),
                locator: sheet.locator,
            },
        ),
    }
}

/// Try every cell of a sweep that turns a wooze knob on a field of its own,
/// before anything is loaded. A preset may be a file beside the scene (#249),
/// which has to read.
fn check_wooze_sweep(args: &RenderArgs, grid: &super::sweep::SweepGrid) -> Result<()> {
    for (column, row) in grid.cells() {
        let (_, Some(table)) = cell_knobs(args, Some(grid), (column, row))? else {
            continue;
        };
        let mut field = flint_wooze::WoozeField::new(flint_wooze::WoozeSample::NEUTRAL);
        field
            .set_global(&table, 0.0, 0.0)
            .map_err(|reason| anyhow!("--sweep {}: {reason}", grid.name()))?;
    }
    Ok(())
}

/// Whether a render is of a moment of the game (#216): a time, frames of it, or
/// a sweep of the field. It hides the editor's grid and draws shallow wooze.
fn is_moment(args: &RenderArgs, grid: Option<&super::sweep::SweepGrid>) -> bool {
    on_clock(args) || grid.is_some_and(|g| g.wooze_knob().is_some())
}

/// The names of `world`'s entities that carry `component`.
fn named_with(world: &flint_ecs::FlintWorld, component: &str) -> Vec<String> {
    world
        .all_entities()
        .into_iter()
        .filter(|entity| {
            world
                .get_components(entity.id)
                .is_some_and(|comps| comps.get(component).is_some())
        })
        .map(|entity| entity.name)
        .collect()
}

/// What a render that didn't run `scripted`'s scripts is missing (#371): a
/// scene whose look a script sets draws plain without them.
fn scripts_not_run(scripted: &[String]) -> String {
    format!(
        "scripts don't run without --scripts, so what {} would do to this scene isn't in it \
         (--script-time T runs them to game time T)",
        scripted.join(", ")
    )
}

/// What a reader of this capture has to be told about what it isn't showing.
fn stage_warnings(
    args: &RenderArgs,
    grid: Option<&super::sweep::SweepGrid>,
    stage: &Stage,
) -> Vec<String> {
    let mut warnings = Vec::new();
    if !is_moment(args, grid) {
        return warnings;
    }
    // Scripts are a clock of their own, so with them running this says nothing
    if stage.game.living_props().is_none() && !stage.scripted {
        warnings.push("this scene has no living props, so only its other clocks move".to_string());
    }
    let with = |component: &str| named_with(stage.game.world(), component);
    let scripts = with(comp::SCRIPT);
    if !scripts.is_empty() && !stage.scripted {
        warnings.push(scripts_not_run(&scripts));
    }
    let animated = with(comp::ANIMATOR);
    if !animated.is_empty() && stage.game.animation().is_none() {
        warnings.push(format!(
            "animation clips don't run without --anim-time, so {} won't match the player",
            animated.join(", ")
        ));
    }
    warnings
}

/// The entities that asked to be living props and drew as still models.
fn still_props(stage: &Stage) -> Vec<String> {
    let mut living: Vec<flint_core::EntityId> = Vec::new();
    if let Some(props) = stage.game.living_props() {
        living.extend(props.props().iter().map(|prop| prop.entity()));
    }
    let world = stage.game.world();
    world
        .all_entities()
        .into_iter()
        .filter(|entity| {
            !living.contains(&entity.id)
                && world
                    .get_components(entity.id)
                    .is_some_and(|comps| comps.get(comp::LIVING_PROP).is_some())
        })
        .map(|entity| entity.name)
        .collect()
}

/// The preset file a `--sweep wooze.preset=...` value names, if it's a file the
/// scene can see rather than a built-in (#249).
fn preset_file_of(
    args: &RenderArgs,
    sweep: &super::sweep::Sweep,
    index: usize,
) -> Result<Option<flint_procgen::WoozePreset>> {
    if sweep.wooze_knob() != Some("preset") {
        return Ok(None);
    }
    let name = &sweep.values[index];
    match flint_procgen::read_preset(Path::new(&args.scene), name) {
        None => Ok(None),
        Some(Ok(preset)) => Ok(Some(preset)),
        Some(Err(reason)) => Err(anyhow!("--sweep: preset '{name}': {reason}")),
    }
}

/// What a sweep cell lays over the scene's `[wooze]` table: a preset file one
/// of its axes names (its knobs first), then every other wooze value it takes,
/// so a `wooze.glide` row turns the glide a preset column set (#596). `None`
/// for a cell no axis of which is a wooze knob.
fn cell_knobs(
    args: &RenderArgs,
    grid: Option<&super::sweep::SweepGrid>,
    (column, row): (usize, usize),
) -> Result<(Option<flint_procgen::WoozePreset>, Option<toml::Table>)> {
    let Some(grid) = grid else {
        return Ok((None, None));
    };
    let axes = grid.axes(column, row);
    let mut preset = None;
    for &(sweep, i) in &axes {
        if let Some(file) = preset_file_of(args, sweep, i)? {
            preset = Some(file);
        }
    }
    let mut table = preset.as_ref().map(|p| preset_wooze_table(&p.wooze));
    for &(sweep, i) in &axes {
        if preset.is_some() && sweep.wooze_knob() == Some("preset") {
            continue;
        }
        if let Some(knobs) = sweep.wooze_table(i) {
            table.get_or_insert_with(toml::Table::new).extend(knobs);
        }
    }
    Ok((preset, table))
}

/// A preset's `[wooze]` knobs as the table the field takes.
fn preset_wooze_table(knobs: &flint_procgen::WoozeKnobs) -> toml::Table {
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

/// `   N living props` for a sheet's subtitle, or nothing in a scene without.
fn living_note(stages: &[Stage]) -> String {
    let props = stages[0]
        .game
        .living_props()
        .map_or(0, |props| props.props().len());
    if props > 0 {
        format!("   {props} living props")
    } else {
        String::new()
    }
}

/// What a sweep's sheet says, as of the moment each stage last captured: the
/// moment top right, and under a wooze column's label what the props read.
fn sheet_text(
    sweep: &super::sweep::Sweep,
    scene_name: &str,
    t: f64,
    stages: &[Stage],
) -> super::style_sheet::SheetText {
    let mut text = super::style_sheet::SheetText::new(
        scene_name,
        format!("sweep {}{}", sweep.name(), living_note(stages)),
        (0..sweep.values.len()).map(|i| sweep.label(i)).collect(),
    );
    text.time = format!("T {t:.2}S");
    if let Some(knob) = sweep.wooze_knob() {
        text.notes = sweep
            .values
            .iter()
            .zip(stages)
            .map(|(value, stage)| {
                stage.game.living_props().map_or_else(String::new, |props| {
                    render_report::read_note(knob, value.parse().ok(), props)
                })
            })
            .collect();
    }
    text
}

/// One cell's scene, as [`Stage::load`] takes it.
struct StageScene<'a> {
    world: flint_ecs::FlintWorld,
    scene_file: flint_scene::SceneFile,
    /// The scene as the stage's renderer binds it (#739).
    binding: &'a flint_render::SceneBinding,
    /// Which cell this is, a row at a time; the first says what all share.
    index: usize,
    /// Which of the sweep's values this stage takes: (column, row).
    cell: (usize, usize),
    grid: Option<&'a super::sweep::SweepGrid>,
    /// The cell's post-processing: the scene's, under any look and the
    /// cell's values, with the CLI's flags over it.
    config: flint_render::PostProcessConfig,
    /// The look the cell draws with before a script asks for another, for
    /// `get_look()` (#497).
    look: Option<String>,
    timeline: Option<ExpressionTimeline>,
    /// Resolve the scene's procgen assets as the player does, whatever the
    /// scene: a replay of the player's capture (#1089). A render resolves
    /// them only in a scene with living props.
    resolve_procgen: bool,
}

/// The look a stage's cell draws with before a script asks for another
/// (#497), as the player names it: a `look=` sweep's value, else `--look`
/// as given, else the scene's own (none when it couldn't be found).
fn stage_look(
    args: &RenderArgs,
    grid: Option<&super::sweep::SweepGrid>,
    (column, row): (usize, usize),
    scene_pp: Option<&flint_scene::PostProcessDef>,
) -> Option<String> {
    grid.and_then(|g| g.cell_look(column, row))
        .or(args.look.as_deref())
        .or_else(|| scene_pp.and_then(|d| d.look.as_deref()))
        .map(str::to_string)
}

/// A captured frame drawn again (#1089): the host takes a `Stage`.
#[path = "render_replay.rs"]
pub(crate) mod replay_host;

/// One scene in one renderer, with its own clocks.
struct Stage {
    /// The scene's game (#725): its world, clock, living props and wooze
    /// field, scripts, particles and `--anim-time`'s animation, stepped in
    /// the player's frame order.
    game: HeadlessGame,
    camera: Camera,
    renderer: SceneRenderer,
    /// The moment of the last capture, which the next one's style history
    /// follows on from.
    captured: Option<f64>,
    /// Whether the game's particles are loaded (they are drawn).
    particles_loaded: bool,
    /// The project the scene is in (ADR 0114), found once when it loads:
    /// what every asset search of this stage asks.
    project: flint_core::project::Project,
    /// How many fixed steps the particles have taken.
    particle_steps: u32,
    /// Whether the particles step with the other clocks, for the scripts' sake,
    /// rather than all at once at a capture.
    particles_on_clock: bool,
    /// Whether the first capture's particle count has been said.
    particles_said: bool,
    /// Whether the scene's scripts run (`--scripts`, #8, or a timeline).
    scripted: bool,
    /// The renderer's post config generation at the last capture: a style the
    /// scripts switched on since gets its resources then.
    post_generation: u64,
    /// The whole-frame styles as the last capture left them, to tell the
    /// scripts' changes since from a look's.
    laid_styles: flint_render::stylize::StyleSettings,
    /// What the scripts' look was when it last went to the renderer.
    look_applied: ScriptLook,
    /// What the deprecated render mode overwrote (#746), to put back.
    mode_shim: flint_render::render_mode::RenderModeShim,
    /// A script's cross-fade to a look (#496) and the game time it was asked
    /// at: each capture lays it at its own moment, as the player's frame does.
    look_fade: Option<(flint_render::look_fade::LookFade, f64)>,
    /// The scene's own `[post_process]` block, which a script's `set_look`
    /// lays a look under (#697), as the player's does.
    scene_post: Option<flint_scene::PostProcessDef>,
    /// What each capture's frame inputs keep from the load (#739): the
    /// pixels per point, the 2D view, and the boil `--boil-time` holds.
    pixels_per_point: f32,
    view_2d: flint_render::View2d,
    pinned_boil: flint_wooze::BoilFrame,
    /// Seconds between the last two captures, for styles whose history is a
    /// rate (#342); 1/60 until a second capture follows on.
    style_dt: f32,
    /// A captured frame's inputs as data (#1089), for its `replay/`.
    replay_inputs: Option<flint_render::frame_replay::FrameInputsState>,
    /// The HUD's painter when scripts run (#731): their draw calls and UI
    /// documents over each capture, as the player draws them.
    hud: Option<flint_player::HeadlessHud>,
}

impl Stage {
    /// Load a column's scene into a new renderer, ready for its first capture.
    fn load(args: &RenderArgs, ctx: &HeadlessContext, scene: StageScene) -> Result<Self> {
        let StageScene {
            world,
            scene_file,
            binding,
            index,
            cell,
            grid,
            config: mut pp_config,
            look: cell_look,
            timeline,
            resolve_procgen,
        } = scene;
        let moment = is_moment(args, grid);

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
        // The player draws from the authored position, and going round through the
        // orbit moves it by a rounding error, which is enough to change thousands of
        // pixels. So a capture that claims to be the player's stays where the scene
        // put it; an ordinary render keeps the pixels it has always had.
        let orbit_asked = args.camera.distance.is_some()
            || args.camera.yaw.is_some()
            || args.camera.pitch.is_some()
            || args.camera.target.is_some();
        let as_authored = scene_set_position
            && !orbit_asked
            && (on_clock(args) || flint_player::living_wanted(&world));
        if !as_authored {
            camera.update_orbit();
        }

        // Create scene renderer
        let made = load_report::span(load_report::Stage::Renderer, String::new);
        let mut renderer = SceneRenderer::new_headless(
            &ctx.device,
            &ctx.queue,
            ctx.format,
            ctx.width,
            ctx.height,
            RendererConfig {
                // The grid is an editor's. A capture of a moment is the game's view.
                show_grid: !args.camera.no_grid && !moment,
                sample_count: args.shading.msaa,
            },
        );
        drop(made);
        // Parts skipped or repeated by label (#1317)
        renderer.set_pass_knobs(args.measure.pass_knobs.knobs()?);

        // The scene bound, its levers set (#739), as the one-renderer path does
        bind_capture_scene(args, ctx, &mut renderer, &world, &scene_file, binding);

        // Load models and textures from the scene
        let config = ModelLoadConfig::from_scene_path(&args.scene);

        // The scene's game (#725), which owns the world from here
        let wants_living = flint_player::living_wanted(&world);
        let mut game = HeadlessGame::new(world, &args.scene, config.project.clone());

        // Living props (#216), and the procgen assets their scene asks for, before the
        // model loader as in the player: catalog > procgen > file.
        if wants_living || resolve_procgen {
            game.resolve_procgen(
                flint_procgen::LivingTables::or_warn(&scene_file)
                    .living
                    .as_ref(),
                GpuSink {
                    renderer: &mut renderer,
                    device: &ctx.device,
                    queue: &ctx.queue,
                    config: &config,
                },
            );
        }
        // A preset column names a preset file beside the scene before a built-in,
        // as the player's Load does (#249), and the file's [living] tables, species
        // and all, go over the scene's as they do there.
        let (column_preset, column_knobs) = cell_knobs(args, grid, cell)?;
        if let (true, Some(preset)) = (wants_living, &column_preset) {
            let props = game.living_props_mut();
            for problem in flint_player::apply_living_preset(props, &preset.living) {
                eprintln!("Warning: --sweep preset: {problem}");
            }
        }
        // The stage's one wooze field (#581), with the column's knobs over the scene's:
        // the living props read it, the shading boil always, and shallow wooze in a
        // capture of a moment. A plain render shows those meshes at rest (#215).
        let field = flint_player::wooze_of_scene(
            flint_procgen::LivingTables::or_warn(&scene_file)
                .wooze
                .as_ref(),
            column_knobs.as_ref(),
        )
        .map_err(|reason| anyhow!("--sweep: {reason}"))?;
        // The player's living subsystem runs both (as its first frame, the
        // props read the field without moving)
        let field_for_boil = field.clone();
        game.start_living(field, moment);

        let models = model_loader::load_models_from_world(
            game.world_mut(),
            &mut renderer,
            &ctx.device,
            &ctx.queue,
            &config,
        );

        // Meshes swept along the scene's splines (#750)
        flint_render::load_scene_splines(
            &args.scene,
            game.world_mut(),
            &mut renderer,
            &ctx.device,
            &ctx.queue,
        );

        // Post-processing: the column's, with a preset column's shading boil
        // under the CLI's boil flags
        if let Some(boil) = column_preset.as_ref().and_then(|preset| preset.boil) {
            let on = boil.enabled.unwrap_or(true);
            let (paint, ink) = pp_config.boil_amounts();
            let amount = |v: Option<f32>, was: f32| v.map_or(was, |v| if on { v } else { 0.0 });
            pp_config.set_boil_amounts(amount(boil.kuwahara, paint), amount(boil.contour, ink));
            if let Some(cadence) = boil.on {
                pp_config.boil_on = cadence.clamp(1, 3);
            }
            render_post::apply_flags(args, &mut pp_config, binding.styles())?;
        }
        // The shading boil's drawing (#214) comes from the field where the camera
        // is. --boil-time picks one and holds it, in one tick at the tempo of that
        // moment. Otherwise each capture stamps the drawing its clock has reached.
        let (paint, ink) = pp_config.boil_amounts();
        let boil_wanted = paint > 0.0 || ink > 0.0;
        match (&field_for_boil, args.clock.boil_time) {
            (Some(field), Some(time)) => {
                pp_config.boil = flint_player::boil_at(
                    field,
                    camera.position_array(),
                    time.max(0.0),
                    pp_config.boil_on,
                );
            }
            (None, _) if boil_wanted && index == 0 => {
                say!("This scene has no [wooze] table, so nothing boils.");
            }
            _ => {}
        }
        // The drawing --boil-time holds every capture at (none without it)
        let pinned_boil = pp_config.boil;
        // The ASCII grid read back, for text, flips and measures (#596)
        let report_file = args
            .measure
            .report
            .as_deref()
            .is_some_and(|r| r != REPORT_TO_STDOUT);
        // The debug views, shadows, the column's post-processing and the
        // panels its scripts may cut between, render scale and quality, as
        // the one-renderer path
        set_up_capture(
            args,
            ctx,
            &mut renderer,
            CaptureSetup {
                scene_file: &scene_file,
                config: pp_config,
                text_capture: args.out.ascii_text.is_some()
                    || args.out.ascii_text_ansi.is_some()
                    || args.seq.flip_stats
                    || args.seq.flip_json.is_some()
                    || report_file
                    || args.measure.reference_text.is_some(),
                first: index == 0,
            },
        )?;
        // What scripts are told the capture draws at
        let pixels_per_point = args.out.pixels_per_point.unwrap_or(1.0);
        game.set_pixels_per_point(pixels_per_point);

        // Scripts (#8), as the player starts them once its models are loaded.
        let scripted = args.clock.scripts || timeline.is_some();
        game.set_scene_look(cell_look.as_deref());
        // Their HUD's painter first (#731), so measure_text and the UI
        // documents lay text out as it draws
        let hud = scripted.then(|| {
            flint_player::HeadlessHud::new(
                &ctx.device,
                &ctx.queue,
                ctx.format,
                flint_core::project::AssetScope::new(
                    config.project.clone(),
                    Path::new(&args.scene)
                        .parent()
                        .filter(|p| !p.as_os_str().is_empty())
                        .unwrap_or(Path::new(".")),
                ),
            )
        });
        if let Some(hud) = &hud {
            game.set_text_measurer(hud.measurer());
        }
        // The screen in points, as the player tells its scripts
        game.start_scripts(
            scripted,
            timeline,
            (
                ctx.width as f32 / pixels_per_point,
                ctx.height as f32 / pixels_per_point,
            ),
            renderer.post_process_config(),
        );
        // A menu nothing opens without input (#731)
        if scripted {
            let (show, focus) = (&args.clock.ui_show, args.clock.ui_focus.as_deref());
            let missing = game.with_ui(|ui| {
                let mut missing = Vec::new();
                for id in show.iter().map(String::as_str).chain(focus) {
                    if !ui.exists(id) {
                        missing.push(id.to_string());
                    }
                }
                for id in show {
                    ui.show(id);
                }
                if focus.is_some() {
                    ui.focus(focus);
                }
                missing
            });
            if !missing.is_empty() {
                anyhow::bail!(
                    "--ui-show / --ui-focus: no UI document the scripts loaded has {}",
                    missing.join(", ")
                );
            }
        }
        // A capture that draws particles has them from here, so what the scripts
        // emit reaches them where it would in the player (#579), and they step
        // on the scripts' clock. One drawing them at --particle-time steps there.
        let particles_drawn = args.clock.particle_time.is_some() || args.clock.time.is_some();
        let mut particles_loaded = false;
        if scripted && particles_drawn {
            load_particles(&config.project, &args.scene, &mut game, &mut renderer, ctx)?;
            particles_loaded = true;
        }
        let particles_on_clock = particles_loaded && args.clock.particle_time.is_none();
        // The game's first frame, the player's: no time passes, the timeline's
        // states at 0 are heard, what the scripts ask for is done, and the
        // living clocks and the field are read where they stand.
        game.step(
            0.0,
            &mut camera,
            Some(&mut renderer),
            Some((&ctx.device, &ctx.queue)),
        );

        // Animation (#307) after the scripts' first frame, as in the player's, and
        // IK after it; without it, IK (ADR 0070) poses authored chains once. A
        // sweep's later cells say nothing the first didn't.
        let step = args.clock.frame_step.unwrap_or(ANIM_DT);
        let animation = pose_animation(
            args,
            &models,
            &config.project,
            game.world_mut(),
            step,
            index == 0,
        );
        if animation.is_none() {
            flint_animation::IkPass::new().run(game.world_mut());
        }
        game.set_animation(animation);

        if let Some(name) = &args.camera.select {
            let id = game
                .world()
                .get_id(name)
                .with_context(|| format!("--select: no entity named '{name}'"))?;
            renderer.set_selected_entity(Some(id));
        }

        let post_generation = renderer.post_config_generation();
        let laid_styles = renderer.post_process_config().styles.clone();
        let view_2d = capture_view_2d(&camera);
        Ok(Self {
            game,
            camera,
            renderer,
            captured: None,
            particles_loaded,
            project: config.project,
            particle_steps: 0,
            particles_on_clock,
            particles_said: false,
            scripted,
            post_generation,
            laid_styles,
            look_applied: ScriptLook::default(),
            mode_shim: Default::default(),
            look_fade: None,
            scene_post: scene_file.post_process.clone(),
            pixels_per_point,
            view_2d,
            pinned_boil,
            style_dt: FrameInputs::default().dt,
            replay_inputs: None,
            hud,
        })
    }

    /// Lay on the renderer what the scripts have asked of it that waits for
    /// the capture: a look (it replaces the config, as the player's does, or
    /// fades to it from the moment it was asked, #496), then the
    /// post-processing overrides, sticky as the player's. Their styles, boil
    /// tunes, distance and quality levers landed as they asked. `t` is the
    /// capture's moment on the game clock.
    fn lay_script_requests(&mut self, args: &RenderArgs, t: f64) -> Result<()> {
        let asked = self.game.take_look_request();
        let mut config = self.renderer.post_process_config().clone();
        let mut changed = false;
        // Whole-frame styles the scripts changed since the last capture,
        // which go over a look's, as they did before the look
        let styled = config.styles != self.laid_styles;
        if let Some(flint_player::LookRequest { name, seconds, at }) = &asked {
            let before = config.clone();
            let base = match &self.scene_post {
                Some(def) => def.clone(),
                None => flint_scene::PostProcessDef::from_authored(toml::Table::new())
                    .map_err(|e| anyhow::anyhow!("set_look: {e}"))?,
            };
            let scope = flint_core::project::AssetScope::new(
                self.project.clone(),
                Path::new(&args.scene)
                    .parent()
                    .filter(|p| !p.as_os_str().is_empty())
                    .unwrap_or(Path::new(".")),
            );
            match flint_render::look::resolve_in(&base, name.as_deref(), &scope) {
                Ok(def) => {
                    let boil = config.boil;
                    let styles = styled.then(|| config.styles.clone());
                    config = post_process_config(args, Some(&def), self.renderer.styles())?;
                    // The shading boil's drawing is the clock's, not the look's
                    config.boil = boil;
                    if let Some(styles) = styles {
                        config.styles = styles;
                    }
                    // The scripts' overrides go over the new look
                    self.look_applied = ScriptLook::default();
                    self.mode_shim.forget();
                    changed = true;
                    self.look_fade = None;
                    if *seconds > 0.0 && seconds.is_finite() {
                        // Drawn from where the frame was; a style the new
                        // look turns on is built at the capture it shows in
                        let fade = flint_render::look_fade::LookFade::new(
                            before.clone(),
                            config,
                            *seconds as f32,
                            self.renderer.styles(),
                        );
                        config = before;
                        self.look_fade = Some((fade, *at));
                    }
                }
                Err(e) => tracing::warn!("set_look: {e}"),
            }
        }
        let fading = self.look_fade.is_some();
        if let Some((fade, at)) = &mut self.look_fade {
            let mut at_moment = fade.clone();
            at_moment.step((t - *at) as f32);
            at_moment.apply(&mut config);
            changed = true;
            if at_moment.done() {
                self.look_fade = None;
            }
        }
        let look = self.game.look();
        // A fade rewrites what the looks differ in each capture, so the
        // scripts' overrides go over it again
        if look != self.look_applied || fading {
            flint_player::apply_look(&mut config, &look, &mut self.mode_shim);
            self.look_applied = look;
            changed = true;
        }
        if changed {
            self.renderer.set_post_process_config(config);
        }
        self.laid_styles = self.renderer.post_process_config().styles.clone();
        Ok(())
    }

    fn capture(
        &mut self,
        args: &RenderArgs,
        ctx: &HeadlessContext,
        t: f64,
        frame_step: f64,
    ) -> Result<image::RgbaImage> {
        self.advance(ctx, t, frame_step);
        // Poses held on twos tick on the capture's moment (#405), the post
        // time's own seconds, stamped before the bones go up
        self.renderer.set_motion_time(t);
        self.game
            .upload_living(&mut self.renderer, &ctx.device, &ctx.queue);
        if let Some(posed) = self.game.animation() {
            posed.upload(&mut self.renderer, &ctx.device, &ctx.queue);
        }
        // What the scripts asked of the renderer that waits for a capture
        // (#697): a look, then the post overrides
        if self.scripted {
            self.lay_script_requests(args, t)?;
        }
        // A style switched on since the last capture (by a script, a look)
        // gets what it draws with
        if self.renderer.post_config_generation() != self.post_generation {
            self.renderer.ensure_style_resources(&ctx.device);
            self.renderer.ensure_fxaa_resources(&ctx.device);
            self.post_generation = self.renderer.post_config_generation();
        }
        // Styles with history (ASCII's, #277) follow on from the last capture
        if let Some(before) = self.captured {
            if t > before {
                self.style_dt = (t - before) as f32;
            }
        }
        self.captured = Some(t);

        // Particles (ADR 0068): fixed-step, fixed-seed simulation so a snapshot
        // at a given time is reproducible run to run.
        let particle_time = args
            .clock
            .particle_time
            .or_else(|| on_clock(args).then_some(t as f32));
        if let Some(seconds) = particle_time {
            self.step_particles(args, ctx, seconds)?;
        }

        // The frame's inputs (#739). On the clock, the post time and the
        // grass are the player's: the capture's moment (grain has its own
        // flag, woozy grass the field's clock). Off it, the post time stays
        // where the load put it, and the grass where the load's first step
        // left it: woozy grass's sway and comb, else at rest.
        let clocked = on_clock(args);
        let style_time = args
            .style
            .style_time
            .or(args.clock.grain_time)
            .map(f64::from);
        let mut inputs = FrameInputs {
            time: t,
            dt: self.style_dt,
            look_time: if clocked {
                style_time
            } else {
                Some(style_time.unwrap_or(0.0))
            },
            boil: self.pinned_boil,
            pixels_per_point: self.pixels_per_point,
            view_2d: self.view_2d,
            // The deprecated render mode's sea (#746), a script's or the flag's
            ocean_tint: self
                .mode_shim
                .ocean_tint()
                .or_else(|| legacy_render_mode(args).and_then(|m| m.ocean_tint())),
            ..Default::default()
        };
        // The game's boil, shallow wooze and woozy grass, as the player's
        let on = self.renderer.post_process_config().boil_on;
        self.game.frame_inputs(&self.camera, on, &mut inputs);
        if args.clock.boil_time.is_some() {
            inputs.boil = self.pinned_boil;
        }
        if !clocked {
            inputs.grass_time = inputs.grass_time.or(Some(0.0));
        }
        inputs.particles = particle_time.map(|_| &self.game.particles().sync);
        // A captured frame keeps its inputs for its replay (#1089)
        self.replay_inputs = self
            .renderer
            .capturing_frame()
            .then(|| flint_render::frame_replay::FrameInputsState::of(&inputs, self.game.world()));
        self.renderer
            .prepare(&ctx.device, &ctx.queue, self.game.world(), &inputs);

        // Render, as comic panels when the scene has them (#880)
        self.renderer.render_frame_to(
            &ctx.device,
            &ctx.queue,
            ctx.depth_target(),
            &self.camera,
            &ctx.color_view,
            Some(self.game.world()),
        );

        // The HUD over it, as the player draws it over its frame (#731)
        if let Some(hud) = &mut self.hud {
            hud.paint(
                &ctx.device,
                &ctx.queue,
                &ctx.hud_view(),
                [ctx.width, ctx.height],
                self.pixels_per_point,
                self.game.hud(),
            );
        }

        // Read back pixels
        let pixels =
            pollster::block_on(ctx.read_pixels()).context("Failed to read rendered pixels")?;
        image::RgbaImage::from_raw(ctx.width, ctx.height, pixels)
            .context("Failed to create image from pixel data")
    }

    /// Draw the moment `t` again under `config` (an `--ai-refine` candidate,
    /// #596): the clocks stay where they are, and the style history starts
    /// over, as the one-renderer path draws each candidate as a first frame.
    fn redraw(
        &mut self,
        args: &RenderArgs,
        ctx: &HeadlessContext,
        t: f64,
        frame_step: f64,
        mut config: flint_render::PostProcessConfig,
    ) -> Result<image::RgbaImage> {
        // The shading boil's drawing is the clock's, not the look's
        config.boil = self.renderer.post_process_config().boil;
        self.renderer.set_post_process_config(config);
        self.renderer.ensure_style_resources(&ctx.device);
        self.renderer.ensure_fxaa_resources(&ctx.device);
        self.renderer.reset_style_history();
        // The scripts' look goes over the candidate's, as over the scene's
        self.look_applied = ScriptLook::default();
        self.mode_shim.forget();
        self.captured = None;
        self.capture(args, ctx, t, frame_step)
    }

    /// Draw the next capture as a first frame (#574): no style history to
    /// follow on from and the load's frame dt, so a strip's tile at `t` is
    /// what `--time t` rendered alone draws. The clocks are left as they are.
    fn forget_history(&mut self) {
        self.renderer.reset_style_history();
        self.captured = None;
        self.style_dt = FrameInputs::default().dt;
    }

    /// Move the clocks on to `t` a `frame_step` at a time, landing on `t` exactly,
    /// without a picture. `t` only goes forward.
    fn advance(&mut self, ctx: &HeadlessContext, t: f64, frame_step: f64) {
        let (steps, rest) = render_plan::steps_within(t - self.game.clock(), frame_step);
        for _ in 0..steps {
            self.step_clocks(ctx, frame_step);
        }
        if rest > 0.0 {
            self.step_clocks(ctx, rest);
        }
    }

    /// One frame of the game (#725): the player's frame order, scripts and what
    /// they ask for, animation, then the wooze field and living props. The
    /// particles step after it, on their fixed step.
    fn step_clocks(&mut self, ctx: &HeadlessContext, dt: f64) {
        self.game.step(
            dt,
            &mut self.camera,
            Some(&mut self.renderer),
            Some((&ctx.device, &ctx.queue)),
        );
        // The player's particles run after its scripts, on the frame's clock.
        if self.particles_on_clock {
            self.step_particles_to(self.game.clock() as f32);
        }
    }

    /// Step the particles on to `seconds` in fixed steps, counting them, so that
    /// a capture at `seconds` finds nothing left to do.
    fn step_particles_to(&mut self, seconds: f32) {
        if !self.particles_loaded {
            return;
        }
        let wanted = (seconds.max(0.0) / PARTICLE_DT).round() as u32;
        let steps = wanted.saturating_sub(self.particle_steps);
        let whole = self.renderer.effective_post_process_config().motion_hold;
        let (world, particles) = self.game.world_and_particles();
        // Emitters on held entities (#873) hold on each step's own time, as
        // the player stamps its frame's before the particles step
        particles.sync.set_motion_hold(whole);
        for k in 1..=steps {
            let at = (self.particle_steps + k) as f64 * PARTICLE_DT as f64;
            particles.sync.set_motion_time(at);
            particles.simulate_to(world, PARTICLE_DT, PARTICLE_DT);
        }
        self.particle_steps += steps;
    }

    fn step_particles(
        &mut self,
        args: &RenderArgs,
        ctx: &HeadlessContext,
        seconds: f32,
    ) -> Result<()> {
        if !self.particles_loaded {
            load_particles(
                &self.project,
                &args.scene,
                &mut self.game,
                &mut self.renderer,
                ctx,
            )?;
            self.particles_loaded = true;
        }
        self.step_particles_to(seconds);
        let (_, particles) = self.game.world_and_particles();
        // Packed for the capture's frame inputs
        particles.pack(Some(self.camera.position_array()));
        let first = !std::mem::replace(&mut self.particles_said, true);
        if first && (args.clock.particle_time.is_some() || particles.sync.emitter_count() > 0) {
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

/// The particles' fixed step (ADR 0068), so a capture at a time is the same run
/// to run.
const PARTICLE_DT: f32 = 1.0 / 60.0;

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

/// The scene's particle system, in its game: every effect beside it
/// registered, the world's emitters found, and their textures uploaded.
fn load_particles(
    project: &flint_core::project::Project,
    scene: &str,
    game: &mut HeadlessGame,
    renderer: &mut SceneRenderer,
    ctx: &HeadlessContext,
) -> Result<()> {
    use flint_runtime::RuntimeSystem;
    // What the scripts emit reaches them from here
    game.set_particles_drawn(true);
    let (world, particles) = game.world_and_particles();
    flint_particles::load_particle_effects_from_world(project, scene, particles);
    particles
        .initialize(world)
        .context("Failed to initialize particles")?;
    flint_render::load_scene_particle_textures(
        renderer,
        &ctx.device,
        &ctx.queue,
        &particles.sync,
        project,
        Path::new(scene).parent().unwrap_or(Path::new(".")),
    );
    Ok(())
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
