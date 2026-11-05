//! `flint gen <spec> --strip`: one living prop over time, as a contact sheet.
//!
//! Motion can't be tuned from a still, and until the runtime plays living
//! props there's nothing else to look at. Each frame evaluates hold-and-glide
//! on the CPU, rebuilds the whole mesh and renders it from one fixed camera,
//! so what changes between cells is the prop and nothing else.
//!
//! The labels are drawn with a small bitmap font rather than a font
//! rasterizer, so the same spec gives the same bytes on any dependency
//! versions. Below each cell: a bar per branch level for how far that level
//! has blended toward its next pose, then the frame, t' and the two poses.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use anyhow::{anyhow, bail, Context, Result};
use flint_core::components as comp;
use flint_core::vfs::{self, MemorySource};
use flint_core::Vec3;
use flint_ecs::FlintWorld;
use flint_import::{import_gltf, ImportResult};
use flint_procgen::{
    boil_bones, mesh_to_glb, BranchSegment, EchoCopy, Generator, LivingClock, LivingConfig,
    LivingRig, LivingSkeleton, PoseStream, ProcGenSpec, Stagger, WarpedTime, JITTER_CHANNELS,
};
use flint_render::{Camera, HeadlessContext, RendererConfig, SceneRenderer};
use image::{Rgb, RgbImage};

use super::gen_preview::{build_mesh_world, PREVIEW_ASSET};
use super::gen_store;

/// What `--strip` was asked for.
#[derive(Debug, Clone, Copy)]
pub struct StripOptions {
    /// Frames to render.
    pub frames: u32,
    /// Frames per second of game time.
    pub fps: f32,
    /// Columns in the grid; `None` picks a 4:3 sheet.
    pub columns: Option<u32>,
    /// Each frame's width and height in pixels.
    pub cell_size: u32,
}

/// Where the camera sits: off to one side and a little above, low enough to
/// read the silhouette against the backdrop.
const CAMERA_YAW: f32 = std::f32::consts::FRAC_PI_4;
const CAMERA_PITCH: f32 = 0.2;

/// Behind the frames: bone paper, light enough that a crown's edge reads as a
/// dark shape, since the silhouette is what a strip is for.
const BACKDROP: [f32; 4] = [0.42, 0.38, 0.31, 1.0];

/// How much room to leave around the prop.
const FRAME_MARGIN: f32 = 1.04;

/// Thin twigs alias, and at this size a crawling edge reads as boil. MSAA keeps
/// what moves in the strip to what moved in the prop.
const SAMPLE_COUNT: u32 = 4;

/// A GLB's path inside the in-memory source the importer reads it from.
const FRAME_PATH: &str = "living_strip_frame.glb";

// ── The plan ───────────────────────────────────────────────────────────────

/// One cell's clock readings, which are also its label.
#[derive(Debug, Clone, PartialEq)]
struct FrameInfo {
    index: u32,
    /// Game time in seconds.
    t: f64,
    /// The clock with no phase offset.
    clock: WarpedTime,
    /// Each drawn level's blend weight, trunk side first.
    levels: Vec<f32>,
}

/// The segments the sheet draws a bar for: one per level when the levels are
/// out of phase, and otherwise just the first, since every other bar would
/// either repeat it or stand for one arbitrary branch.
fn bar_segments(segments: &[BranchSegment], stagger: Stagger) -> Vec<&BranchSegment> {
    if stagger != Stagger::PerLevel {
        return segments.first().into_iter().collect();
    }
    let mut seen = BTreeSet::new();
    let mut bars: Vec<&BranchSegment> = segments.iter().filter(|s| seen.insert(s.depth)).collect();
    bars.sort_by_key(|s| s.depth);
    bars
}

fn frame_infos(
    options: &StripOptions,
    clock: &LivingClock,
    segments: &[BranchSegment],
) -> Vec<FrameInfo> {
    let bars = bar_segments(segments, clock.stagger);
    (0..options.frames)
        .map(|index| {
            let t = f64::from(index) / f64::from(options.fps);
            FrameInfo {
                index,
                t,
                clock: clock.at(t),
                levels: bars
                    .iter()
                    .map(|s| clock.for_segment(t, s).weight)
                    .collect(),
            }
        })
        .collect()
}

/// Columns for a 4:3 sheet of square cells. 48 frames make 8 x 6.
fn auto_columns(frames: u32) -> u32 {
    (f64::from(frames) * 4.0 / 3.0).sqrt().ceil().max(1.0) as u32
}

// ── The layout ─────────────────────────────────────────────────────────────

/// Where everything goes on the sheet. Pure arithmetic, so it can be tested
/// without a GPU.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Layout {
    columns: u32,
    rows: u32,
    cell: u32,
    /// How many bars each cell has.
    bar_rows: u32,
    /// Label text scale: 5x7 glyphs, each pixel this many pixels wide.
    scale: u32,
    width: u32,
    height: u32,
}

const MARGIN: u32 = 28;
const GUTTER: u32 = 16;
const GLYPH_WIDTH: u32 = 5;
const GLYPH_HEIGHT: u32 = 7;
/// Two strips of film perforations, above and below the frames.
const PERFORATION_BAND: u32 = 26;

impl Layout {
    fn new(frames: u32, columns: Option<u32>, cell: u32, bar_rows: u32) -> Self {
        let columns = columns.unwrap_or_else(|| auto_columns(frames)).max(1);
        let rows = frames.div_ceil(columns).max(1);
        let scale = if cell >= 180 { 2 } else { 1 };
        let mut layout = Self {
            columns,
            rows,
            cell,
            bar_rows,
            scale,
            width: 0,
            height: 0,
        };
        layout.width = MARGIN * 2 + columns * cell + (columns - 1) * GUTTER;
        layout.height = layout.grid_top()
            + rows * layout.cell_height()
            + (rows - 1) * GUTTER
            + PERFORATION_BAND
            + MARGIN;
        layout
    }

    /// The title, the settings line and the key, each with space below.
    fn header_height(&self) -> u32 {
        let title = GLYPH_HEIGHT * (self.scale + 1);
        let line = GLYPH_HEIGHT * self.scale;
        title + line * 2 + 6 * self.scale + 8 * self.scale
    }

    fn grid_top(&self) -> u32 {
        MARGIN + self.header_height() + PERFORATION_BAND
    }

    /// A bar is 2 pixels and its gap 1, at text scale 1.
    fn bars_height(&self) -> u32 {
        self.bar_rows * 3 * self.scale
    }

    /// A frame with its bars and caption.
    fn cell_height(&self) -> u32 {
        self.cell + 3 * self.scale + self.bars_height() + GLYPH_HEIGHT * self.scale
    }

    /// The top-left corner of frame `index`.
    fn cell_origin(&self, index: u32) -> (u32, u32) {
        let (col, row) = (index % self.columns, index / self.columns);
        (
            MARGIN + col * (self.cell + GUTTER),
            self.grid_top() + row * (self.cell_height() + GUTTER),
        )
    }
}

// ── Running it ─────────────────────────────────────────────────────────────

/// Render `spec` as a contact sheet at `output`, or say what that would do.
pub fn run(
    generator: &dyn Generator,
    spec: &ProcGenSpec,
    label: &str,
    options: StripOptions,
    output: Option<&str>,
    dry_run: bool,
) -> Result<()> {
    let living = living_config(spec, label)?;
    let alive = generator.as_living().ok_or_else(|| {
        anyhow!(
            "{label}: --strip needs a living prop, and generator '{}' can't separate a \
             structure from its poses",
            spec.generator
        )
    })?;
    alive
        .check_living(spec)
        .with_context(|| format!("{label}: --strip needs a living prop"))?;

    let output = output_path(spec, output)?;
    let skeleton = alive
        .structure(spec, living.lineage_seed)
        .with_context(|| format!("{label}: couldn't grow the living structure"))?;

    let clock = LivingClock::from(living);
    let infos = frame_infos(&options, &clock, &skeleton.segments);
    let bar_rows = bar_segments(&skeleton.segments, clock.stagger).len() as u32;
    let layout = Layout::new(options.frames, options.columns, options.cell_size, bar_rows);

    println!("Spec:       {}", spec.meta.name);
    println!(
        "Living:     lineage {}: {} segments, {} leaf cards",
        living.lineage_seed,
        skeleton.segments.len(),
        skeleton.leaf_bindings.len()
    );
    println!(
        "Clock:      step {} s, glide {}, stagger {}",
        living.step,
        living.glide,
        stagger_name(living.stagger)
    );
    let echoes = living.echoes.copies();
    if !echoes.is_empty() {
        println!(
            "Echoes:     {} copies, out to {} of a step, alpha {}",
            echoes.len(),
            living.echoes.spread,
            living.echoes.alpha
        );
    }
    println!(
        "Strip:      {} frames at {} fps, {} x {} cells of {} px, {} x {} px",
        options.frames,
        options.fps,
        layout.columns,
        layout.rows,
        options.cell_size,
        layout.width,
        layout.height
    );
    if dry_run {
        println!("Output:     {}", output.display());
        return Ok(());
    }

    let started = Instant::now();

    let mut stream = PoseStream::new(alive, spec, &skeleton, clock);
    let frames = if echoes.is_empty() {
        // Every frame's mesh first: the camera has to frame all of them, and it
        // can't move once it starts.
        let mut positions: Vec<Vec3> = Vec::new();
        let mut glbs = Vec::with_capacity(infos.len());
        for info in &infos {
            let mesh = stream
                .at(info.t)
                .and_then(|pose| alive.posed_mesh(spec, &skeleton, &pose))
                .with_context(|| format!("{label}: couldn't build frame {}", info.index))?;
            positions.extend(mesh.vertices.iter().map(|v| Vec3::from_array(v.position)));
            glbs.push(mesh_to_glb(&mesh).context("failed to encode a frame")?);
        }
        render_frames(&glbs, &positions, options.cell_size)?
    } else {
        let rig = alive
            .rig(spec, living.lineage_seed)
            .with_context(|| format!("{label}: couldn't build the rig"))?;
        let times: Vec<f64> = infos.iter().map(|info| info.t).collect();
        render_echo_frames(
            &mut stream,
            &skeleton,
            &rig,
            &times,
            &echoes,
            f64::from(clock.step),
            options.cell_size,
        )
        .with_context(|| format!("{label}: couldn't render its echoes"))?
    };
    let sheet = compose(spec, living, &options, &layout, &infos, &frames);

    if let Some(parent) = output.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    sheet
        .save_with_format(&output, image::ImageFormat::Png)
        .with_context(|| format!("failed to write {}", output.display()))?;
    println!(
        "Output:     {} ({:.1} s)",
        output.display(),
        started.elapsed().as_secs_f64()
    );
    Ok(())
}

fn living_config<'a>(spec: &'a ProcGenSpec, label: &str) -> Result<&'a LivingConfig> {
    spec.living.as_ref().ok_or_else(|| {
        anyhow!(
            "{label} has no [living] table, so there's nothing to strip; \
             see specs/examples/oak_living.procgen.toml"
        )
    })
}

fn output_path(spec: &ProcGenSpec, output: Option<&str>) -> Result<PathBuf> {
    let path = match output {
        Some(out) => PathBuf::from(out),
        None => PathBuf::from(format!("{}_strip.png", gen_store::slug(&spec.meta.name))),
    };
    let is_png = path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("png"));
    if !is_png {
        bail!(
            "--strip writes one PNG; {} doesn't end in .png",
            path.display()
        );
    }
    Ok(path)
}

fn stagger_name(stagger: Stagger) -> &'static str {
    match stagger {
        Stagger::None => "none",
        Stagger::PerLevel => "per_level",
        Stagger::PerBranch => "per_branch",
    }
}

fn import_glb(glb: &[u8]) -> Result<ImportResult> {
    let mut source = MemorySource::new();
    source.insert(FRAME_PATH, glb.to_vec());
    vfs::scoped(Arc::new(source), || import_gltf(Path::new(FRAME_PATH)))
        .context("failed to read a frame back")
}

/// What every frame of a strip is drawn with.
struct StripStage {
    ctx: HeadlessContext,
    camera: Camera,
    renderer: SceneRenderer,
    world: FlintWorld,
}

impl StripStage {
    /// A renderer and a camera that frames `positions`.
    fn new(positions: &[Vec3], size: u32) -> Result<Self> {
        let ctx = pollster::block_on(HeadlessContext::new(size, size))
            .context("Failed to create headless render context")?;

        let mut camera = Camera::new();
        camera.aspect = ctx.aspect_ratio();
        camera.yaw = CAMERA_YAW;
        camera.pitch = CAMERA_PITCH;
        let (target, distance) = fit_view(positions, camera.yaw, camera.pitch, camera.fov);
        camera.target = target;
        camera.distance = distance;
        camera.update_orbit();

        let mut renderer = SceneRenderer::new_headless(
            &ctx.device,
            &ctx.queue,
            ctx.format,
            ctx.width,
            ctx.height,
            RendererConfig {
                show_grid: false,
                sample_count: SAMPLE_COUNT,
            },
        );
        renderer.set_clear_color(BACKDROP);
        let world = strip_world()?;
        Ok(Self {
            ctx,
            camera,
            renderer,
            world,
        })
    }

    /// The prop as it's loaded now, as an image.
    fn capture(&mut self) -> Result<RgbImage> {
        let ctx = &self.ctx;
        self.renderer.update_from_world(&self.world, &ctx.device);
        self.renderer.render_to(
            &ctx.device,
            &ctx.queue,
            &ctx.depth_view,
            &self.camera,
            &ctx.color_view,
        );
        let pixels =
            pollster::block_on(ctx.read_pixels()).context("Failed to read rendered pixels")?;
        let (rgba, _) = pixels.as_chunks::<4>();
        let rgb = rgba.iter().flat_map(|p| [p[0], p[1], p[2]]).collect();
        RgbImage::from_raw(ctx.width, ctx.height, rgb).context("a frame came back the wrong size")
    }
}

/// Each GLB as an image, all from the camera that frames `positions`.
fn render_frames(glbs: &[Vec<u8>], positions: &[Vec3], size: u32) -> Result<Vec<RgbImage>> {
    let mut stage = StripStage::new(positions, size)?;
    let mut frames = Vec::with_capacity(glbs.len());
    for glb in glbs {
        let ctx = &stage.ctx;
        stage
            .renderer
            .load_model(&ctx.device, &ctx.queue, PREVIEW_ASSET, &import_glb(glb)?);
        frames.push(stage.capture()?);
    }
    Ok(frames)
}

/// A pose as what the skinned pipeline reads, with its leaf cards held still:
/// a strip draws the slow layer.
fn still_palette(
    stream: &mut PoseStream,
    skeleton: &LivingSkeleton,
    rig: &LivingRig,
    t: f64,
) -> Result<Vec<[[f32; 4]; 4]>> {
    let mut bones = Vec::new();
    rig.palette(skeleton, &stream.at(t)?, &mut bones)?;
    boil_bones(&[0.0; JITTER_CHANNELS], &mut bones);
    Ok(bones)
}

/// The frames with their echoes, through the skinned draw `flint play` uses:
/// the rest mesh goes up once, and a frame is a palette for the prop and one
/// for each echo.
fn render_echo_frames(
    stream: &mut PoseStream,
    skeleton: &LivingSkeleton,
    rig: &LivingRig,
    times: &[f64],
    echoes: &[EchoCopy],
    step: f64,
    size: u32,
) -> Result<Vec<RgbImage>> {
    // The camera frames every frame's prop, and the echoes that reach past
    // both ends of the strip, which no frame's prop will ever show.
    let reach = echoes
        .iter()
        .map(|e| f64::from(e.offset.abs()))
        .fold(0.0, f64::max)
        * step;
    let ends = times.first().map(|t| t - reach).into_iter();
    let ends = ends.chain(times.last().map(|t| t + reach));
    let mut positions: Vec<Vec3> = Vec::new();
    for t in times.iter().copied().chain(ends) {
        let mesh = rig.skinned_mesh(&still_palette(stream, skeleton, rig, t)?);
        positions.extend(mesh.vertices.iter().map(|v| Vec3::from_array(v.position)));
    }

    let mut stage = StripStage::new(&positions, size)?;
    let ctx = &stage.ctx;
    let import = flint_player::living_import(PREVIEW_ASSET, rig);
    stage
        .renderer
        .load_skinned_model(&ctx.device, &ctx.queue, PREVIEW_ASSET, &import);
    let entity = stage
        .world
        .get_id(PREVIEW_ASSET)
        .context("the strip's world has no prop")?;

    let mut frames = Vec::with_capacity(times.len());
    for &t in times {
        let ctx = &stage.ctx;
        let bones = still_palette(stream, skeleton, rig, t)?;
        stage
            .renderer
            .write_entity_bones(&ctx.device, &ctx.queue, entity, 0, &bones);
        for (index, echo) in echoes.iter().enumerate() {
            let bones = still_palette(stream, skeleton, rig, t + f64::from(echo.offset) * step)?;
            stage.renderer.write_entity_echo_bones(
                &ctx.device,
                &ctx.queue,
                entity,
                index,
                echo.opacity,
                0,
                &bones,
            );
        }
        frames.push(stage.capture()?);
    }
    Ok(frames)
}

/// The prop's entity, with a warm sun from the camera's side and a cool fill
/// from the other, so the bark and the crown's far side both read.
fn strip_world() -> Result<FlintWorld> {
    let mut world = build_mesh_world(1);
    for (name, direction, color, intensity) in [
        ("sun", [0.75, 1.0, 0.35], [1.0, 0.95, 0.85], 2.6),
        ("fill", [-0.7, 0.4, 0.5], [0.75, 0.85, 1.0], 0.7),
    ] {
        let array = |v: [f64; 3]| toml::Value::Array(v.map(toml::Value::Float).to_vec());
        let mut light = toml::map::Map::new();
        light.insert("type".to_string(), "directional".into());
        light.insert("direction".to_string(), array(direction));
        light.insert("color".to_string(), array(color));
        light.insert("intensity".to_string(), toml::Value::Float(intensity));
        let entity = world.spawn(name).context("failed to add a light")?;
        world
            .set_component(entity, comp::LIGHT, toml::Value::Table(light))
            .context("failed to add a light")?;
    }
    Ok(world)
}

/// The target and distance that fit every point in a square view of `fov`
/// degrees from `yaw` and `pitch`. Fitted on the points themselves: a box's
/// corners stand well outside a tree, and more so across frames that lean
/// different ways.
fn fit_view(points: &[Vec3], yaw: f32, pitch: f32, fov: f32) -> (Vec3, f32) {
    let to_camera = Vec3::new(
        pitch.cos() * yaw.sin(),
        pitch.sin(),
        pitch.cos() * yaw.cos(),
    );
    let right = Vec3::UP.cross(&to_camera).normalized();
    let up = to_camera.cross(&right);

    // Centre the points' spread across the view.
    let spread = |axis: &Vec3| {
        points.iter().fold((f32::MAX, f32::MIN), |(lo, hi), p| {
            let along = p.dot(axis);
            (lo.min(along), hi.max(along))
        })
    };
    let (across, high) = (spread(&right), spread(&up));
    let middle = |(lo, hi): (f32, f32)| (lo + hi) * 0.5;
    let depth = points
        .iter()
        .map(|p| p.dot(&to_camera))
        .fold((f32::MAX, f32::MIN), |(lo, hi), d| (lo.min(d), hi.max(d)));
    let target = right * middle(across) + up * middle(high) + to_camera * middle(depth);

    // The nearest of each point's own distances: a point nearer the camera
    // has less room in the view, so it asks for more.
    let tan = (fov.to_radians() * 0.5).tan();
    let distance = points.iter().fold(0.5_f32, |distance, p| {
        let from_target = *p - target;
        let off_axis = from_target
            .dot(&right)
            .abs()
            .max(from_target.dot(&up).abs());
        distance.max(off_axis * FRAME_MARGIN / tan + from_target.dot(&to_camera))
    });
    (target, distance)
}

// ── The sheet ──────────────────────────────────────────────────────────────

const PAPER: Rgb<u8> = Rgb([18, 16, 14]);
const PERFORATION: Rgb<u8> = Rgb([46, 41, 36]);
const BAR_EMPTY: Rgb<u8> = Rgb([40, 36, 32]);
const AMBER: Rgb<u8> = Rgb([232, 176, 74]);
const BONE: Rgb<u8> = Rgb([207, 198, 180]);
const DIM: Rgb<u8> = Rgb([130, 120, 106]);

fn compose(
    spec: &ProcGenSpec,
    living: &LivingConfig,
    options: &StripOptions,
    layout: &Layout,
    infos: &[FrameInfo],
    frames: &[RgbImage],
) -> RgbImage {
    let mut sheet = RgbImage::from_pixel(layout.width, layout.height, PAPER);
    let scale = layout.scale;

    // The header: what this is, what it was run with, how to read it.
    let mut y = MARGIN;
    let title = scale + 1;
    draw_text(
        &mut sheet,
        MARGIN,
        y,
        title,
        AMBER,
        &spec.meta.name.to_uppercase(),
    );
    draw_text_right(
        &mut sheet,
        layout.width - MARGIN,
        y,
        title,
        BONE,
        &format!("LINEAGE {}", living.lineage_seed),
    );
    y += GLYPH_HEIGHT * title + 6 * scale;
    let echoes = living.echoes.copies().len();
    draw_text(
        &mut sheet,
        MARGIN,
        y,
        scale,
        BONE,
        &format!(
            "STEP {:.2}S   GLIDE {:.2}   STAGGER {}   {} FPS   {} FRAMES{}",
            living.step,
            living.glide,
            stagger_name(living.stagger).to_uppercase(),
            options.fps,
            options.frames,
            if echoes > 0 {
                format!("   {echoes} ECHOES")
            } else {
                String::new()
            }
        ),
    );
    y += GLYPH_HEIGHT * scale + 4 * scale;
    draw_text(&mut sheet, MARGIN, y, scale, DIM, key(living.stagger));

    let top_band = layout.grid_top() - PERFORATION_BAND;
    let bottom_band = layout.height - MARGIN - PERFORATION_BAND;
    draw_perforations(&mut sheet, top_band);
    draw_perforations(&mut sheet, bottom_band);

    for (info, frame) in infos.iter().zip(frames) {
        draw_cell(&mut sheet, layout, info, frame);
    }
    sheet
}

/// How to read a cell.
fn key(stagger: Stagger) -> &'static str {
    match stagger {
        Stagger::PerLevel => {
            "BARS: BLEND TO THE NEXT POSE, ONE PER BRANCH LEVEL. THEN FRAME, T' WARPED TIME, POSES A>B"
        }
        Stagger::None | Stagger::PerBranch => {
            "BAR: BLEND TO THE NEXT POSE. THEN FRAME, T' WARPED TIME, POSES A>B"
        }
    }
}

/// A row of film sprocket holes, centred in its band.
fn draw_perforations(sheet: &mut RgbImage, y: u32) {
    const WIDTH: u32 = 14;
    const HEIGHT: u32 = 8;
    const PITCH: u32 = 28;
    let y = y + (PERFORATION_BAND - HEIGHT) / 2;
    let count = (sheet.width() - MARGIN * 2 + PITCH - WIDTH) / PITCH;
    let spare = sheet.width() - MARGIN * 2 - (count * PITCH - (PITCH - WIDTH));
    for i in 0..count {
        let x = MARGIN + spare / 2 + i * PITCH;
        fill_rect(sheet, x + 1, y, WIDTH - 2, HEIGHT, PERFORATION);
        fill_rect(sheet, x, y + 1, WIDTH, HEIGHT - 2, PERFORATION);
    }
}

fn draw_cell(sheet: &mut RgbImage, layout: &Layout, info: &FrameInfo, frame: &RgbImage) {
    let (x, y) = layout.cell_origin(info.index);
    let scale = layout.scale;
    image::imageops::replace(sheet, frame, i64::from(x), i64::from(y));

    // A bar a level, trunk side on top: how far it has blended.
    let mut bar_y = y + layout.cell + 2 * scale;
    for &weight in &info.levels {
        fill_rect(sheet, x, bar_y, layout.cell, 2 * scale, BAR_EMPTY);
        let filled = (weight * layout.cell as f32).round() as u32;
        fill_rect(sheet, x, bar_y, filled, 2 * scale, AMBER);
        bar_y += 3 * scale;
    }

    let caption = y + layout.cell + 3 * scale + layout.bars_height();
    let after = draw_text(
        sheet,
        x,
        caption,
        scale,
        AMBER,
        &format!("{:03}", info.index),
    );
    draw_text(
        sheet,
        after + (GLYPH_WIDTH + 1) * scale,
        caption,
        scale,
        BONE,
        &format!("T'{:.2}", info.clock.value()),
    );
    draw_text_right(
        sheet,
        x + layout.cell,
        caption,
        scale,
        DIM,
        &format!("{}>{}", info.clock.index, info.clock.index + 1),
    );
}

/// A rectangle, clipped to the sheet.
fn fill_rect(sheet: &mut RgbImage, x: u32, y: u32, width: u32, height: u32, color: Rgb<u8>) {
    let (right, bottom) = (
        (x + width).min(sheet.width()),
        (y + height).min(sheet.height()),
    );
    for py in y..bottom {
        for px in x..right {
            sheet.put_pixel(px, py, color);
        }
    }
}

/// Draw `text`, clipped to the sheet, and return the x just past it.
fn draw_text(sheet: &mut RgbImage, x: u32, y: u32, scale: u32, color: Rgb<u8>, text: &str) -> u32 {
    let mut pen = x;
    for c in text.chars() {
        for (row, bits) in glyph(c).iter().enumerate() {
            for col in 0..GLYPH_WIDTH {
                if bits >> (GLYPH_WIDTH - 1 - col) & 1 == 1 {
                    fill_rect(
                        sheet,
                        pen + col * scale,
                        y + row as u32 * scale,
                        scale,
                        scale,
                        color,
                    );
                }
            }
        }
        pen += (GLYPH_WIDTH + 1) * scale;
    }
    pen
}

/// Draw `text` so it ends at `right`.
fn draw_text_right(
    sheet: &mut RgbImage,
    right: u32,
    y: u32,
    scale: u32,
    color: Rgb<u8>,
    text: &str,
) {
    let width = text.chars().count() as u32 * (GLYPH_WIDTH + 1) * scale;
    draw_text(sheet, right.saturating_sub(width), y, scale, color, text);
}

/// A 5x7 glyph, a row of 5 bits per line, top first. Upper case only: the
/// sheet prints like the edge markings on a roll of film. Anything without a
/// glyph is a `?`, so a label never silently loses a character.
fn glyph(c: char) -> [u8; 7] {
    match c.to_ascii_uppercase() {
        ' ' => [0, 0, 0, 0, 0, 0, 0],
        'A' => [0x0E, 0x11, 0x11, 0x1F, 0x11, 0x11, 0x11],
        'B' => [0x1E, 0x11, 0x11, 0x1E, 0x11, 0x11, 0x1E],
        'C' => [0x0E, 0x11, 0x10, 0x10, 0x10, 0x11, 0x0E],
        'D' => [0x1E, 0x11, 0x11, 0x11, 0x11, 0x11, 0x1E],
        'E' => [0x1F, 0x10, 0x10, 0x1E, 0x10, 0x10, 0x1F],
        'F' => [0x1F, 0x10, 0x10, 0x1E, 0x10, 0x10, 0x10],
        'G' => [0x0E, 0x11, 0x10, 0x17, 0x11, 0x11, 0x0F],
        'H' => [0x11, 0x11, 0x11, 0x1F, 0x11, 0x11, 0x11],
        'I' => [0x0E, 0x04, 0x04, 0x04, 0x04, 0x04, 0x0E],
        'J' => [0x07, 0x02, 0x02, 0x02, 0x02, 0x12, 0x0C],
        'K' => [0x11, 0x12, 0x14, 0x18, 0x14, 0x12, 0x11],
        'L' => [0x10, 0x10, 0x10, 0x10, 0x10, 0x10, 0x1F],
        'M' => [0x11, 0x1B, 0x15, 0x15, 0x11, 0x11, 0x11],
        'N' => [0x11, 0x11, 0x19, 0x15, 0x13, 0x11, 0x11],
        'O' => [0x0E, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0E],
        'P' => [0x1E, 0x11, 0x11, 0x1E, 0x10, 0x10, 0x10],
        'Q' => [0x0E, 0x11, 0x11, 0x11, 0x15, 0x12, 0x0D],
        'R' => [0x1E, 0x11, 0x11, 0x1E, 0x14, 0x12, 0x11],
        'S' => [0x0F, 0x10, 0x10, 0x0E, 0x01, 0x01, 0x1E],
        'T' => [0x1F, 0x04, 0x04, 0x04, 0x04, 0x04, 0x04],
        'U' => [0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0E],
        'V' => [0x11, 0x11, 0x11, 0x0A, 0x0A, 0x0A, 0x04],
        'W' => [0x11, 0x11, 0x11, 0x15, 0x15, 0x15, 0x0A],
        'X' => [0x11, 0x11, 0x0A, 0x04, 0x0A, 0x11, 0x11],
        'Y' => [0x11, 0x11, 0x11, 0x0A, 0x04, 0x04, 0x04],
        'Z' => [0x1F, 0x01, 0x02, 0x04, 0x08, 0x10, 0x1F],
        '0' => [0x0E, 0x11, 0x13, 0x15, 0x19, 0x11, 0x0E],
        '1' => [0x04, 0x0C, 0x04, 0x04, 0x04, 0x04, 0x0E],
        '2' => [0x0E, 0x11, 0x01, 0x02, 0x04, 0x08, 0x1F],
        '3' => [0x1F, 0x02, 0x04, 0x02, 0x01, 0x11, 0x0E],
        '4' => [0x02, 0x06, 0x0A, 0x12, 0x1F, 0x02, 0x02],
        '5' => [0x1F, 0x10, 0x1E, 0x01, 0x01, 0x11, 0x0E],
        '6' => [0x06, 0x08, 0x10, 0x1E, 0x11, 0x11, 0x0E],
        '7' => [0x1F, 0x01, 0x02, 0x04, 0x08, 0x08, 0x08],
        '8' => [0x0E, 0x11, 0x11, 0x0E, 0x11, 0x11, 0x0E],
        '9' => [0x0E, 0x11, 0x11, 0x0F, 0x01, 0x02, 0x0C],
        '.' => [0, 0, 0, 0, 0, 0x0C, 0x0C],
        ',' => [0, 0, 0, 0, 0x0C, 0x04, 0x08],
        ':' => [0, 0x0C, 0x0C, 0, 0x0C, 0x0C, 0],
        '\'' => [0x04, 0x04, 0x08, 0, 0, 0, 0],
        '_' => [0, 0, 0, 0, 0, 0, 0x1F],
        '-' => [0, 0, 0, 0x1F, 0, 0, 0],
        '>' => [0x08, 0x04, 0x02, 0x01, 0x02, 0x04, 0x08],
        '/' => [0, 0x01, 0x02, 0x04, 0x08, 0x10, 0],
        _ => [0x0E, 0x11, 0x01, 0x02, 0x04, 0, 0x04],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn segment(id: u32, depth: u32) -> BranchSegment {
        BranchSegment {
            start: Vec3::ZERO,
            end: Vec3::UP,
            radius_start: 0.1,
            radius_end: 0.1,
            depth,
            id,
            parent: None,
        }
    }

    fn options(frames: u32) -> StripOptions {
        StripOptions {
            frames,
            fps: 24.0,
            columns: None,
            cell_size: 240,
        }
    }

    #[test]
    fn forty_eight_frames_make_an_eight_by_six_sheet() {
        assert_eq!(auto_columns(48), 8);
        let layout = Layout::new(48, None, 240, 6);
        assert_eq!((layout.columns, layout.rows), (8, 6));
    }

    #[test]
    fn a_short_last_row_still_gets_its_row() {
        let layout = Layout::new(9, Some(4), 64, 1);
        assert_eq!((layout.columns, layout.rows), (4, 3));
        let layout = Layout::new(1, None, 64, 1);
        assert_eq!((layout.columns, layout.rows), (2, 1));
    }

    #[test]
    fn every_cell_and_caption_fits_on_the_sheet_without_touching() {
        for (frames, columns, cell, bars) in [
            (48, None, 240, 6),
            (48, Some(5), 240, 1),
            (7, None, 64, 3),
            (1, None, 64, 1),
            (100, Some(10), 179, 6),
        ] {
            let layout = Layout::new(frames, columns, cell, bars);
            for i in 0..frames {
                let (x, y) = layout.cell_origin(i);
                assert!(x + cell <= layout.width - MARGIN + 1, "{layout:?} cell {i}");
                assert!(
                    y + layout.cell_height() <= layout.height - MARGIN - PERFORATION_BAND,
                    "{layout:?} cell {i}"
                );
                if (i + 1) % layout.columns != 0 && i + 1 < frames {
                    assert!(
                        x + cell < layout.cell_origin(i + 1).0,
                        "{layout:?} cell {i}"
                    );
                }
                if i + layout.columns < frames {
                    assert!(
                        y + layout.cell_height() < layout.cell_origin(i + layout.columns).1,
                        "{layout:?} cell {i}"
                    );
                }
            }
        }
    }

    #[test]
    fn a_caption_fits_its_cell_at_the_default_size() {
        // The longest ordinary caption: frame, t', poses, with a gap.
        let layout = Layout::new(48, None, 240, 6);
        let chars = "012".len() + 1 + "T'10.30".len() + 1 + "10>11".len();
        assert!(chars as u32 * (GLYPH_WIDTH + 1) * layout.scale <= layout.cell);
    }

    #[test]
    fn the_camera_fits_every_point_and_wastes_little() {
        // A leaning shape off the origin, with a stray point nearer the camera.
        let points: Vec<Vec3> = [
            [10.0, 0.0, -2.0],
            [10.2, 9.0, -2.0],
            [13.0, 6.0, 1.0],
            [8.0, 7.0, -4.0],
            [12.0, 2.0, 3.0],
        ]
        .into_iter()
        .map(Vec3::from_array)
        .collect();
        let (yaw, pitch, fov) = (CAMERA_YAW, CAMERA_PITCH, 45.0_f32);
        let (target, distance) = fit_view(&points, yaw, pitch, fov);

        let to_camera = Vec3::new(
            pitch.cos() * yaw.sin(),
            pitch.sin(),
            pitch.cos() * yaw.cos(),
        );
        let right = Vec3::UP.cross(&to_camera).normalized();
        let up = to_camera.cross(&right);
        let tan = (fov.to_radians() * 0.5).tan();
        let widest = points.iter().fold(0.0_f32, |widest, p| {
            let from_target = *p - target;
            let in_front = distance - from_target.dot(&to_camera);
            assert!(in_front > 0.0, "{p:?} is behind the camera");
            let off_axis = from_target
                .dot(&right)
                .abs()
                .max(from_target.dot(&up).abs());
            widest.max(off_axis / in_front / tan)
        });
        // 1 is the view's edge: inside it, by no more than the margin and a bit.
        assert!(widest <= 1.0, "{widest}");
        assert!(widest > 0.9, "{widest}");
    }

    #[test]
    fn a_held_clock_leaves_every_bar_empty() {
        let clock = LivingClock {
            step: 0.5,
            glide: 0.0,
            stagger: Stagger::None,
        };
        let infos = frame_infos(&options(48), &clock, &[segment(0, 0)]);
        assert_eq!(infos.len(), 48);
        assert!(infos.iter().all(|f| f.levels == [0.0]));
        assert_eq!(infos[11].clock.index, 0);
        assert_eq!(infos[12].clock.index, 1);
    }

    #[test]
    fn a_full_glide_fills_the_bar_across_the_step() {
        let clock = LivingClock {
            step: 0.5,
            glide: 1.0,
            stagger: Stagger::None,
        };
        let infos = frame_infos(&options(12), &clock, &[segment(0, 0)]);
        for pair in infos.windows(2) {
            assert!(pair[0].levels[0] < pair[1].levels[0], "{pair:?}");
        }
    }

    #[test]
    fn per_level_stagger_draws_a_bar_a_level_trunk_side_first() {
        let segments = [segment(0, 2), segment(1, 1), segment(2, 2), segment(3, 3)];
        let depths = |stagger| {
            bar_segments(&segments, stagger)
                .iter()
                .map(|s| s.depth)
                .collect::<Vec<_>>()
        };
        assert_eq!(depths(Stagger::PerLevel), [1, 2, 3]);
        assert_eq!(depths(Stagger::None), [2]);
        assert_eq!(depths(Stagger::PerBranch), [2]);

        let clock = LivingClock {
            step: 0.5,
            glide: 1.0,
            stagger: Stagger::PerLevel,
        };
        let infos = frame_infos(&options(2), &clock, &segments);
        let levels = &infos[1].levels;
        assert_eq!(levels.len(), 3);
        assert_ne!(levels[0], levels[1], "out of phase: {levels:?}");
    }

    #[test]
    fn output_must_be_a_png_and_is_named_for_the_spec() {
        let spec = ProcGenSpec::parse(
            r#"
generator = "tree_v1"
[meta]
name = "my oak"
version = "1.0.0"
[seed]
mode = "fixed"
value = 1
"#,
        )
        .unwrap();
        assert_eq!(
            output_path(&spec, None).unwrap(),
            Path::new("my_oak_strip.png")
        );
        assert!(output_path(&spec, Some("a/b.PNG")).is_ok());
        assert!(output_path(&spec, Some("tree.glb")).is_err());
        assert!(output_path(&spec, Some("noextension")).is_err());
    }

    #[test]
    fn every_character_the_labels_use_has_its_own_glyph() {
        let unknown = glyph('\u{2603}');
        let used = "ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789.,:'_->/";
        let mut seen = BTreeSet::new();
        for c in used.chars() {
            let g = glyph(c);
            assert_ne!(g, unknown, "{c} has no glyph");
            assert!(g.iter().all(|row| *row < 32), "{c} is wider than 5 bits");
            assert!(seen.insert(g), "{c} looks like another character");
        }
        assert_eq!(glyph('a'), glyph('A'));
    }

    #[test]
    fn text_past_the_edge_is_clipped_not_a_panic() {
        let mut sheet = RgbImage::from_pixel(16, 16, PAPER);
        let end = draw_text(&mut sheet, 8, 12, 2, AMBER, "CLIPPED");
        assert!(end > 16);
        assert!(sheet.pixels().any(|p| *p == AMBER));
    }
}
