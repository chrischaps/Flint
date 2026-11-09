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
//! With `--scripts` (#8) each stage also runs its scene's Rhai scripts, a tick a
//! fixed step, in the player's frame order (`flint_player::HeadlessScripts`), and
//! `--expression-timeline` feeds `expression()` from a file on that clock. Same
//! scene, same timeline, same pixels.

use super::gif_writer;
use super::render_plan::{self, TimeSpec};
use super::render_report::{self, CaptureReport};
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

/// `--report json`: the living props' report on stdout (#216), not a file.
const REPORT_TO_STDOUT: &str = "json";

#[derive(clap::Args)]
pub struct RenderArgs {
    /// Path to scene file
    #[arg(
        required_unless_present_any = ["list_gpus", "look", "list_styles"],
        default_value = ""
    )]
    pub scene: String,

    /// Output image path
    #[arg(short, long, default_value = "render.png")]
    pub output: String,

    /// Image width in pixels [default: 1920; each cell of a --sweep, 640; a GIF, 960]
    #[arg(long)]
    pub width: Option<u32>,

    /// Image height in pixels [default: 1080; each cell of a --sweep, 360; a GIF, 540]
    #[arg(long)]
    pub height: Option<u32>,

    /// A look over the scene's post-processing (#279): a built-in
    /// (ascii_noir, matrix, ink_wash, graphite, watercolor,
    /// coloured_pencil, blueprint, charcoal), a
    /// `styles/<name>.look.toml` beside the scene, or a path. Merged over
    /// the scene's [post_process] key by key; the other flags apply over
    /// it. `--look help` lists every look and what to tune
    #[arg(long)]
    pub look: Option<String>,

    /// Render one setting at several values as one labelled contact sheet
    /// (#279, #353): `ascii.cell=6,8,12` (any `--style` key),
    /// `post.exposure=0.8,1.2` (a [post_process] field; split on `|` to sweep
    /// a vector), `look=ascii_noir,matrix`, or a Living Props knob,
    /// `wooze.glide=0,0.5,1` (#216: .amplitude, .step, .glide, .jitter and
    /// .preset; it sets the scene's global value, and zones still apply).
    /// --width/--height size each cell. Give it twice for a grid (#358):
    /// the first key across, the second down
    #[arg(long, value_parser = super::sweep::parse_sweep, action = clap::ArgAction::Append)]
    pub sweep: Vec<super::sweep::Sweep>,

    /// A sweep's cells at the final render's scale (#358): draw each at
    /// --width/--height (default 1920x1080) and keep this region pixel for
    /// pixel, X,Y,W,H. Point-sized styles (ASCII cells) then draw as many
    /// glyphs across it as the final render does there
    #[arg(long, value_parser = super::sweep::parse_crop, conflicts_with = "sweep_scale")]
    pub sweep_crop: Option<super::sweep::CellView>,

    /// `full`: draw each sweep cell at --width/--height (default 1920x1080)
    /// and shrink it to 640 wide, so the whole frame is composed as the final
    /// render is (glyphs blur); `cell` (default): each cell a 640x360 frame
    #[arg(long, value_parser = ["cell", "full"])]
    pub sweep_scale: Option<String>,

    /// Write the frame's (or each --sweep column's) measures as JSON
    /// (#357): mean luminance, near-black / near-white shares, edge
    /// density, the ASCII grid's ramp-step histogram when it drew, and the
    /// distance to --reference. Measured from the sheet's own pixels.
    /// `--report json` instead prints what each living prop was doing when
    /// it was captured (#216: lineage seed, pose indices, the wooze values
    /// it read, every species value and its layer) as one JSON object on
    /// stdout, with the human lines moved to stderr
    #[arg(long, value_name = "FILE.json|json")]
    pub report: Option<String>,

    /// A target image: each frame or column gets its distance to it (the
    /// metric `flint gen --ai-refine` uses, a score out of 100, lower is
    /// closer), printed, labelled on a sweep sheet and in --report
    #[arg(long, value_name = "PNG")]
    pub reference: Option<String>,

    /// Refine --look toward --reference (#357): an agent proposes edits,
    /// each is rendered and measured, the best is kept while the distance
    /// falls. Writes the refined look (--refine-out), a JSON log beside it
    /// and a sheet of the rounds (-o). Calls go through the cost meter and
    /// ledger (.flint/procgen_costs.json)
    #[arg(long, requires_all = ["look", "reference"], conflicts_with_all = ["sweep", "frames"])]
    pub ai_refine: bool,

    /// Most refine rounds
    #[arg(long, default_value_t = 4)]
    pub refine_rounds: u32,

    /// Candidates rendered a round
    #[arg(long, default_value_t = 4)]
    pub refine_candidates: usize,

    /// Where the refined look goes [default: <look>_refined.look.toml
    /// beside -o]; its log is the same path with .json
    #[arg(long, value_name = "FILE.look.toml")]
    pub refine_out: Option<String>,

    /// Who proposes refine edits: `anthropic` (Claude; ANTHROPIC_API_KEY)
    /// or `mock` (free, offline: walks the look's tune sweeps)
    #[arg(long, default_value = "anthropic")]
    pub ai_provider: String,

    /// The Claude model for --ai-refine
    #[arg(long)]
    pub ai_model: Option<String>,

    /// Refuse any call that could take the run's spend past this (USD)
    #[arg(long)]
    pub max_cost: Option<f64>,

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

    /// Quality settings as `key=value,...`, as flint-player's --quality
    /// (FlintKart#55), applied after every other flag: e.g.
    /// `render_scale=0.6,shadow_cascades=1,ssao=0,bloom=0` renders a tier
    #[arg(long, value_name = "KEY=VALUE,...")]
    pub quality: Option<String>,

    /// Turn on the full-screen ASCII style (#265,
    /// `[post_process.styles.ascii]`)
    #[arg(long)]
    pub ascii: bool,

    /// Also write the ASCII style's character grid as UTF-8 text (#317):
    /// one line per cell row, one character per cell, the same cells as
    /// the PNG (a cell the picture passes through is a space). Needs the
    /// ASCII style on (--ascii, a look, or the scene's own ascii)
    #[arg(long, value_name = "FILE")]
    pub ascii_text: Option<String>,

    /// The same grid with 24-bit ANSI colours (each glyph's ink over its
    /// ground), for `cat` in a terminal
    #[arg(long, value_name = "FILE")]
    pub ascii_text_ansi: Option<String>,

    /// ASCII cell width in points (default 8; the height follows the font)
    #[arg(long)]
    pub ascii_cell: Option<f32>,

    /// ASCII style: glyph set, `builtin` or a `fonts/<name>.glyphs.toml`
    /// beside the scene (default builtin)
    #[arg(long)]
    pub ascii_font: Option<String>,

    /// ASCII style: the glyph set's ramp (built-in: classic, blocks,
    /// minimal, detailed; default the set's own)
    #[arg(long)]
    pub ascii_ramp: Option<String>,

    /// ASCII style colour: `source` (the cell's hue), `tint` (one colour,
    /// --ascii-tint), `raw` (the cell's colour as it is), `mono` (white on
    /// grey) or `palette` (--ascii-palette)
    #[arg(long)]
    pub ascii_color: Option<String>,

    /// ASCII style palette for `--ascii-color palette`: built-in (cga,
    /// cga-warm, cga16, phosphor, amber, mono) or a
    /// `palettes/<name>.palette.toml` beside the scene. Sets `--ascii-color
    /// palette` unless that's given
    #[arg(long)]
    pub ascii_palette: Option<String>,

    /// ASCII style ground behind glyphs: `dim` (the mode's own), `black`
    /// or `inverse` (the ink fills the cell, the glyph is cut out)
    #[arg(long)]
    pub ascii_ground: Option<String>,

    /// ASCII style halo around bright glyphs (0 off, default 0; 0.6 reads
    /// as a phosphor glow)
    #[arg(long)]
    pub ascii_glow: Option<f32>,

    /// ASCII style tint for `--ascii-color tint` (r,g,b 0-1; default
    /// 0.35,1,0.45, phosphor green)
    #[arg(long, value_parser = crate::commands::common_args::parse_vec3)]
    pub ascii_tint: Option<[f32; 3]>,

    /// ASCII style: how much of the cell's colour fills behind its glyph
    /// (0 black, 1 the cell's colour; default 0.15)
    #[arg(long)]
    pub ascii_background: Option<f32>,

    /// ASCII style: luminance offset before the ramp (-1..1, default 0)
    #[arg(long, allow_hyphen_values = true)]
    pub ascii_brightness: Option<f32>,

    /// ASCII style: luminance contrast about mid-grey (default 1)
    #[arg(long)]
    pub ascii_contrast: Option<f32>,

    /// ASCII style: glyph ink brightness (default 1)
    #[arg(long)]
    pub ascii_ink: Option<f32>,

    /// ASCII style: ordered dither before the ramp, fixed to the cell grid
    /// (0-1, default 0). The style's other temporal knobs (smoothing,
    /// hysteresis, #277) need a frame before, which one render hasn't
    #[arg(long)]
    pub ascii_dither: Option<f32>,

    /// ASCII style: stops of gain on each cell's colour before its glyph
    /// is picked (-4 to 4, default 0; #289)
    #[arg(long, allow_hyphen_values = true)]
    pub ascii_exposure: Option<f32>,

    /// ASCII style: stretch each style id's luminance range over the
    /// glyph ramp, measured from this frame (0-1, default 0; #289)
    #[arg(long)]
    pub ascii_normalize: Option<f32>,

    /// ASCII style coverage (#266): `screen` glyphizes the whole frame;
    /// `entities` only the entities with an `ascii_style` (or a
    /// `style_mask` naming ascii), the other flags being their preset.
    /// A scene with such entities needs no flag at all
    #[arg(long, value_parser = ["screen", "entities"])]
    pub ascii_scope: Option<String>,

    /// ASCII style grid (#266): `screen` (one grid centred on the frame)
    /// or `object` (each styled entity's cells pinned to its projected
    /// origin)
    #[arg(long, value_parser = ["screen", "object"])]
    pub ascii_grid: Option<String>,

    /// Physical pixels per point, as a high-DPI window's scale factor:
    /// styles sized in points (ASCII cells) scale by it (default 1)
    #[arg(long)]
    pub pixels_per_point: Option<f32>,

    /// ASCII tone bands (default 0, which keeps the whole ramp)
    #[arg(long)]
    pub ascii_levels: Option<f32>,

    /// ASCII tone footprint in cells (default 1 = each cell alone)
    #[arg(long)]
    pub ascii_footprint: Option<f32>,

    /// Turn off the ASCII edge glyphs (| / - \ on depth, normal and
    /// silhouette edges); the style's `edges = 0`
    #[arg(long)]
    pub ascii_no_edges: bool,

    /// ASCII style: edge glyph strength over the tone glyph (0 off .. 1,
    /// default 1; #267)
    #[arg(long)]
    pub ascii_edges: Option<f32>,

    /// ASCII style: how much edge, in cell widths, a cell needs to draw a
    /// line glyph (default 0.5; lower finds more)
    #[arg(long)]
    pub ascii_edge_threshold: Option<f32>,

    /// ASCII style: more edge than this, in cell widths, is texture-like
    /// geometry (grass) and keeps its tone glyph (default 3); a styled
    /// entity's outline ignores it
    #[arg(long)]
    pub ascii_edge_max: Option<f32>,

    /// ASCII edge sensitivity to depth: the relative view-distance step that
    /// counts as an edge (default 0.6; lower finds more)
    #[arg(long)]
    pub ascii_edge_depth: Option<f32>,

    /// ASCII edge sensitivity to creases: the normal change that counts as
    /// an edge (default 2.5; lower finds more)
    #[arg(long)]
    pub ascii_edge_normal: Option<f32>,

    /// Draw the scene at this fraction of the image on each side
    /// (0.25-1), upscaled in the composite, as flint-player's
    /// --render-scale (#122)
    #[arg(long)]
    pub render_scale: Option<f32>,

    /// Render this many frames with GPU timing on and print each pass's
    /// smoothed GPU time before writing the image
    #[arg(long)]
    pub gpu_time: Option<u32>,

    /// Draw a sequence of N frames on one renderer (#345), written as
    /// numbered PNGs beside --output (`seq.png` gives `seq_0000.png`...).
    /// Temporal styles (ASCII's history, #277) see consecutive frames, so
    /// they behave as in the player. Move the camera with --orbit or
    /// --camera-path
    #[arg(long, value_parser = clap::value_parser!(u32).range(1..))]
    pub frames: Option<u32>,

    /// With --frames: seconds between frames (default 1/60). Post time,
    /// grass wind and particles advance by it too
    #[arg(long, requires = "frames")]
    pub frame_dt: Option<f32>,

    /// With --frames: orbit the camera about its target at this many
    /// degrees a second (negative turns the other way)
    #[arg(
        long,
        requires = "frames",
        allow_hyphen_values = true,
        conflicts_with = "camera_path"
    )]
    pub orbit: Option<f32>,

    /// With --frames: a TOML file of `[[key]] time/position/target`, the
    /// camera interpolated linearly between keys
    #[arg(long, requires = "frames")]
    pub camera_path: Option<String>,

    /// With --frames: count the ASCII style's glyph flips and A-B-A
    /// reversals frame to frame, exactly, from its own character grid,
    /// and print them per frame and in sum
    #[arg(long, requires = "frames")]
    pub flip_stats: bool,

    /// With --frames: write the flip counts (as --flip-stats) to this JSON
    #[arg(long, requires = "frames")]
    pub flip_json: Option<String>,

    /// With --frames: write no PNGs (for a --flip-stats measurement)
    #[arg(long, requires = "frames")]
    pub no_images: bool,

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

    /// Shared geometry edges (#301): the relative view-distance step that
    /// is a silhouette, for every style that reads them (default: 0.6)
    #[arg(long)]
    pub edge_depth: Option<f32>,

    /// Shared geometry edges (#301): the rebuilt-normal change that is a
    /// crease (default: 2.5)
    #[arg(long)]
    pub edge_normal: Option<f32>,

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
    /// a clock drives, as the player has them at that time. Seconds (3.5), a span
    /// (0..4) for an animated --output x.gif, or a list (2,5,9.5) of moments of one
    /// run, each written as a still beside --output (x.png gives x_0000.png...).
    /// Physics and animation clips don't run, and scripts only with --scripts.
    /// Same value, same pixels
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

    #[command(flatten)]
    pub style: crate::commands::common_args::StyleArgs,

    /// GPU adapter to use (case-insensitive substring of the adapter name,
    /// e.g. "NVIDIA"). Overrides WGPU_ADAPTER_NAME; default prefers the
    /// high-performance adapter. Use --list-gpus to see names.
    #[arg(long)]
    pub gpu: Option<String>,

    /// List available GPU adapters and exit
    #[arg(long)]
    pub list_gpus: bool,

    /// List every registered style (#303): its stage, where a scene sets
    /// it, and each parameter's default and range, then exit. With a
    /// scene, the user styles in its `styles/` folder are listed too
    #[arg(long)]
    pub list_styles: bool,

    /// With --list-styles: print the list as JSON
    #[arg(long, requires = "list_styles")]
    pub json: bool,
}

fn parse_frame_step(text: &str) -> Result<f64, String> {
    match text.parse::<f64>() {
        Ok(step) if step.is_finite() && step > 0.0 => Ok(step),
        _ => Err(format!(
            "'{text}' isn't a frame step: it's seconds, more than 0"
        )),
    }
}

/// `--list-styles [--json]`: the registry as text or JSON, with the user
/// styles beside the scene when one is given.
fn list_styles(args: &RenderArgs) -> Result<()> {
    use flint_render::stylize::{catalog, custom};
    // The scene's user styles, and its palettes and fonts as choices (#322)
    let scene_dir =
        (!args.scene.is_empty()).then(|| Path::new(&args.scene).parent().unwrap_or(Path::new(".")));
    if let Some(dir) = scene_dir {
        for message in custom::load_dir(dir) {
            eprintln!("warning: {message}");
        }
    }
    if args.json {
        say!(
            "{}",
            serde_json::to_string_pretty(&catalog::catalog_for(scene_dir))?
        );
    } else {
        print!("{}", catalog::catalog_text_for(scene_dir));
    }
    Ok(())
}

/// A GPU kept between renders in one process (#329). `flint render` makes
/// its own; `render-suite` hands every case the same one, so a suite pays
/// for one device instead of one a case. Each render still builds its own
/// `SceneRenderer`, so nothing but the device and its two targets carries
/// over.
#[derive(Default)]
pub struct RenderHost {
    ctx: Option<HeadlessContext>,
    /// The `--gpu` the device was made for.
    gpu: Option<String>,
    /// Devices made so far (one, unless a case asked for another GPU).
    pub devices_made: usize,
}

impl RenderHost {
    /// Let the device go; the next render makes a new one. For after a
    /// panic, when the device may be in no state to trust.
    pub fn drop_device(&mut self) {
        self.ctx = None;
    }

    fn context(
        &mut self,
        width: u32,
        height: u32,
        gpu: &Option<String>,
    ) -> Result<&HeadlessContext> {
        if self.ctx.is_some() && self.gpu != *gpu {
            self.ctx = None;
        }
        match &mut self.ctx {
            Some(ctx) => ctx.resize(width, height),
            None => {
                let ctx = pollster::block_on(HeadlessContext::new(width, height))
                    .context("Failed to create headless render context")?;
                self.ctx = Some(ctx);
                self.gpu = gpu.clone();
                self.devices_made += 1;
            }
        }
        Ok(self.ctx.as_ref().unwrap())
    }
}

pub fn run(args: RenderArgs) -> Result<()> {
    run_in(args, &mut RenderHost::default())
}

/// `flint render` on `host`'s GPU (made on first use).
pub fn run_in(args: RenderArgs, host: &mut RenderHost) -> Result<()> {
    if args.list_gpus {
        for a in flint_render::list_adapters() {
            say!("{a}");
        }
        return Ok(());
    }
    if args.list_styles {
        return list_styles(&args);
    }
    if args.look.as_deref() == Some("help") {
        print!("{}", super::look::help(&args.scene));
        return Ok(());
    }
    if args.scene.is_empty() {
        anyhow::bail!("a scene is required (flint render <scene.toml>)");
    }
    let report_json = args.report.as_deref() == Some(REPORT_TO_STDOUT);
    HUMAN_TO_STDERR.store(report_json, Ordering::Relaxed);
    render_plan::check_output(&args.output, args.time.as_ref())?;
    let wants_flips = args.flip_stats || args.flip_json.is_some();
    let wants_text = args.ascii_text.is_some() || args.ascii_text_ansi.is_some();
    let grid = super::sweep::SweepGrid::new(&args.sweep).map_err(|e| anyhow::anyhow!(e))?;
    let view = super::sweep::CellView::from_args(args.sweep_crop, args.sweep_scale.as_deref());
    if grid.is_none() && (args.sweep_crop.is_some() || args.sweep_scale.is_some()) {
        anyhow::bail!("--sweep-crop and --sweep-scale shape a sweep's cells: add a --sweep");
    }
    if wants_text && grid.is_some() {
        anyhow::bail!("--ascii-text writes one frame's grid: drop --sweep");
    }
    if args.frames.is_some() && grid.is_some() {
        anyhow::bail!("--frames draws one sequence: drop --sweep");
    }
    if let Some(sweep) = grid.as_ref().filter(|g| g.wooze_knob().is_some()) {
        check_wooze_sweep(&args, &sweep.columns)?;
    }
    if let Some(dt) = args.frame_dt {
        if !(dt.is_finite() && dt > 0.0) {
            anyhow::bail!("--frame-dt must be a positive number of seconds");
        }
    }
    let camera_path = args
        .camera_path
        .as_deref()
        .map(super::frame_seq::CameraPath::load)
        .transpose()?;
    // User-authored styles (#274): registered before anything reads a
    // `[post_process.styles]` table, a look or a --style that names one
    let scene_dir = Path::new(&args.scene)
        .parent()
        .unwrap_or(Path::new("."))
        .to_path_buf();
    for message in flint_render::stylize::custom::load_dir(&scene_dir) {
        eprintln!("warning: {message}");
    }
    args.style
        .check()
        .map_err(|e| anyhow::anyhow!("--style: {e}"))?;
    let look = args
        .look
        .as_deref()
        .map(|name| super::look::find(name, &args.scene))
        .transpose()?;
    let (width, height) = match grid {
        Some(_) => view.render_size(args.width, args.height),
        // A refine's rounds are sheet cells too
        None if args.ai_refine => {
            let (w, h) = super::sweep::CELL_SIZE;
            (args.width.unwrap_or(w), args.height.unwrap_or(h))
        }
        None => (args.width.unwrap_or(1920), args.height.unwrap_or(1080)),
    };
    view.check(width, height).map_err(|e| anyhow::anyhow!(e))?;
    let reference = args
        .reference
        .as_deref()
        .map(super::look_measure::load_reference)
        .transpose()?;
    if (args.report.is_some() || reference.is_some()) && args.frames.is_some() {
        anyhow::bail!("--report / --reference measure one frame or a sweep: drop --frames");
    }
    if args.time.is_some() && args.frames.is_some() {
        anyhow::bail!("--time and --frames both draw a span: --time 0..4 -o x.gif, or --frames");
    }
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

    // Load scene
    let (mut world, scene_file) =
        load_scene(&args.scene, &registry).context("Failed to load scene")?;
    say!("Loaded scene: {}", scene_file.scene.name);
    say!("Entities: {}", world.entity_count());

    // Every column's post-processing, worked out (and checked) before any
    // GPU work: the scene's block, under the look, under the column.
    // A scene's own look (#356) goes under its keys first; --look then
    // goes over the result
    let scene_pp = scene_file
        .post_process
        .as_ref()
        .map(|def| flint_render::look::resolve_scene(def, &args.scene));
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
        let mut config = post_process_config(&args, def.as_ref())?;
        for spec in grid.as_ref().map_or(Vec::new(), |g| g.cell_styles(c, r)) {
            flint_render::stylize::apply_style_spec(&mut config, &spec)
                .map_err(|e| anyhow::anyhow!("--sweep: {e}"))?;
        }
        configs.push(config);
    }
    if let Some(look) = &look {
        say!("Look: {} ({})", look.name, look.source);
    }

    // A render with clocks goes through stages (#216, #487)
    let timed = args.time.is_some()
        || grid.as_ref().is_some_and(|g| g.wooze_knob().is_some())
        || args.scripts
        || args.expression_timeline.is_some()
        || report_json
        || LivingRun::wanted_by(&world)
        || scene_file.wooze.is_some();
    if timed {
        return run_stages(
            &args,
            host,
            StagedRender {
                registry: &registry,
                first: (world, scene_file),
                grid: grid.as_ref(),
                configs,
                report_json,
            },
        );
    }

    let ctx = host.context(width, height, &args.gpu)?;

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
            "style" => DebugMode::StyleMask,
            "edges" => DebugMode::StyleEdges,
            _ => DebugMode::Pbr,
        };
        if matches!(mode, DebugMode::StyleMask | DebugMode::StyleEdges) && args.no_postprocess {
            eprintln!(
                "warning: --debug-mode {mode_str} draws through post-processing, \
                 which --no-postprocess turns off; the image shows none of it"
            );
        }
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

    // Post-processing: the first column's (the only one without --sweep)
    renderer.set_post_process_config(configs[0].clone());
    renderer.set_style_text_capture(wants_text || wants_flips || args.report.is_some());
    renderer.ensure_style_resources(&ctx.device);
    renderer.ensure_fxaa_resources(&ctx.device);

    if let Some(scale) = args.render_scale {
        renderer.set_render_scale(&ctx.device, scale);
    }

    // Quality settings over the flags above: the renderer's own state is the
    // starting point, so `--quality` names only what it changes.
    if let Some(quality) = &args.quality {
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
        say!("Quality: {}", applied.to_query());
    }
    if let Some(px) = args.pixels_per_point {
        renderer.pixels_per_point = px;
    }
    // Post time for grain/mode animation and animated styles. Headless
    // renders leave this 0.0 so PNG snapshots stay deterministic;
    // --style-time (or the older --grain-time) pins an explicit frame.
    if let Some(t) = args.style.style_time.or(args.grain_time) {
        renderer.ocean_time = t as f64;
    }

    // Load terrain (if any)
    load_terrain_for_render(&world, &args.scene, &ctx.device, &ctx.queue, &mut renderer);

    // Set scene_dir for font/texture resolution and viewport params for screen anchoring
    renderer.scene_dir = Path::new(&args.scene).parent().map(|p| p.to_path_buf());
    renderer.ortho_height = camera.ortho_height;
    renderer.aspect_ratio = camera.aspect;

    // Two-bone IK (ADR 0070): pose authored chains before the single frame.
    flint_animation::IkPass::new().run(&mut world);

    renderer.update_from_world(&world, &ctx.device);

    // Particles (ADR 0068): fixed-step, fixed-seed simulation so a snapshot
    // at a given --particle-time is reproducible run to run.
    let mut particle_sim = None;
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
        say!(
            "Particles: {} alive across {} emitter(s) at t = {:.2}s",
            particles.sync.total_alive(),
            particles.sync.emitter_count(),
            seconds
        );
        particle_sim = Some(particles);
    }

    // GPU timing: the same frame many times, each pass's time smoothed
    if let Some(frames) = args.gpu_time {
        if !renderer.set_gpu_timing(&ctx.device, &ctx.queue, true) {
            anyhow::bail!("--gpu-time: this adapter has no TIMESTAMP_QUERY");
        }
        for _ in 0..frames.max(1) {
            let mut encoder = ctx
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
            renderer.encode_to(
                &ctx.device,
                &ctx.queue,
                &mut encoder,
                ctx.depth_target(),
                &camera,
                &ctx.color_view,
            );
            renderer.encode_gpu_timings(&ctx.device, &mut encoder);
            ctx.queue.submit(std::iter::once(encoder.finish()));
            renderer.frame_submitted();
            ctx.device.poll(wgpu::Maintain::Wait);
        }
        let stats = renderer.collect_stats();
        let (sw, sh) = renderer.scene_size();
        say!(
            "GPU time over {} frames at {}x{} (scene {}x{}):",
            frames, width, height, sw, sh
        );
        // As wide as the longest label (a style's, such as "Display bloom")
        let w = stats
            .gpu_passes
            .iter()
            .map(|p| p.label.len())
            .max()
            .unwrap_or(0)
            .max(12);
        for pass in &stats.gpu_passes {
            say!("  {:<w$} {:>7.3} ms", pass.label, pass.ms);
        }
        say!("  {:<w$} {:>7.3} ms", "frame", stats.gpu_frame_ms);
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
    }

    if let Some(frames) = args.frames {
        return render_sequence(
            &args,
            SequenceRig {
                ctx,
                renderer: &mut renderer,
                world: &world,
                camera,
                camera_path: camera_path.as_ref(),
                particles: particle_sim.as_mut(),
            },
            frames,
        );
    }

    if args.ai_refine {
        let look = look.expect("clap requires --look with --ai-refine");
        let reference = reference.expect("clap requires --reference with --ai-refine");
        return refine_look(&args, look, reference, &mut |table| {
            let def = super::look::overlay(scene_pp, table)?;
            let config = post_process_config(&args, Some(&def))?;
            draw_frame(ctx, &mut renderer, &camera, Some(config))
        });
    }

    // Render each column (one without --sweep), reading each back
    let mut frames = Vec::with_capacity(configs.len());
    let mut texts = Vec::with_capacity(configs.len());
    for (i, config) in configs.iter().enumerate() {
        let pixels = draw_pixels(ctx, &mut renderer, &camera, (i > 0).then(|| config.clone()))?;
        if args.report.is_some() {
            texts.push(renderer.style_text(&ctx.device, "ascii"));
        }
        if wants_text {
            write_ascii_text(
                args.ascii_text.as_deref(),
                args.ascii_text_ansi.as_deref(),
                renderer.style_text(&ctx.device, "ascii"),
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
                    "  pixels cyan/amber/magenta: {}; cells green/dashed/red/grey: {}",
                    names[..3].join(", "),
                    names[3..].join(", ")
                );
            }
            let mut rows: Vec<(Option<[f32; 3]>, String)> = flint_render::stylize::EDGE_VIEW_LEGEND
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
    // (cropped or shrunk under --sweep-crop / --sweep-scale, #358)
    let pictures: Vec<image::RgbaImage> = match &grid {
        Some(_) => frames.into_iter().map(|f| view.cell(f)).collect(),
        None => frames,
    };

    // Measures (#357), from those same pixels. The ASCII section counts the
    // whole drawn frame's grid, which a crop shows only part of.
    let mut columns = Vec::new();
    if args.report.is_some() || reference.is_some() {
        use super::look_measure as lm;
        for (i, img) in pictures.iter().enumerate() {
            let image = lm::measure(img);
            let ascii = texts.get(i).cloned().flatten().map(|t| {
                lm::measure_ascii(&t, lm::ascii_ramp(&configs[i], Path::new(&args.scene)))
            });
            let distance = reference.as_ref().map(|r| lm::distance_to(r, img));
            let value = grid.as_ref().map(|g| {
                let (c, r) = cells[i];
                g.describe(c, r)
            });
            say!(
                "  [{}] {}{}",
                value.as_deref().unwrap_or("frame"),
                lm::summary(&image, ascii.as_ref()),
                distance
                    .map(|d| format!("  distance {:.2}", d.score))
                    .unwrap_or_default()
            );
            columns.push(lm::ColumnReport {
                index: i,
                value,
                image,
                ascii,
                distance: distance.map(Into::into),
            });
        }
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
            let reference_note = match &args.reference {
                Some(r) => format!("  d = distance to {r}"),
                None => String::new(),
            };
            let subtitle = format!("sweep {}{over}{scale}{reference_note}", grid.name());
            let label = |s: &super::sweep::Sweep| (0..s.values.len()).map(|i| s.label(i)).collect();
            match &grid.rows {
                None => {
                    // With a reference, each label carries its distance and
                    // the nearest is starred (#357)
                    let labels: Vec<String> = label(&grid.columns);
                    let labels = labels
                        .into_iter()
                        .enumerate()
                        .map(|(i, l)| match columns.get(i).and_then(|c| c.distance) {
                            Some(d) => {
                                let star = if nearest == Some(i) { " *" } else { "" };
                                format!("{l}  d {:.1}{star}", d.score)
                            }
                            None => l,
                        })
                        .collect();
                    super::style_sheet::compose(
                        &pictures,
                        &super::style_sheet::SheetText::new(
                            scene_file.scene.name.clone(),
                            subtitle,
                            labels,
                        ),
                    )
                }
                Some(rows) => super::style_sheet::compose_grid(
                    &pictures,
                    &super::style_sheet::GridText {
                        title: scene_file.scene.name.clone(),
                        subtitle,
                        row_key: rows.name(),
                        column_labels: label(&grid.columns),
                        row_labels: label(rows),
                    },
                ),
            }
        }
    };
    if let Some(path) = &args.report {
        let report = super::look_measure::RenderReport {
            scene: args.scene.clone(),
            look: look.as_ref().map(|l| l.name.clone()),
            sweep: grid.as_ref().map(|g| g.name()),
            reference: args.reference.clone(),
            width,
            height,
            thresholds: super::look_measure::Thresholds::USED,
            columns,
            nearest,
        };
        std::fs::write(path, serde_json::to_string_pretty(&report)?)
            .context(format!("Failed to write {path}"))?;
        say!("Wrote measures to {path}");
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
}

/// One still frame, read back: under `config` when given (a sweep column,
/// a refine candidate), each drawn as a first frame so no column inherits
/// the last one's ASCII history (#277's hysteresis would carry glyphs
/// across).
fn draw_pixels(
    ctx: &HeadlessContext,
    renderer: &mut SceneRenderer,
    camera: &Camera,
    config: Option<flint_render::PostProcessConfig>,
) -> Result<Vec<u8>> {
    if let Some(config) = config {
        renderer.set_post_process_config(config);
        renderer.ensure_style_resources(&ctx.device);
        renderer.ensure_fxaa_resources(&ctx.device);
    }
    renderer.reset_style_history();
    renderer.render_to(
        &ctx.device,
        &ctx.queue,
        ctx.depth_target(),
        camera,
        &ctx.color_view,
    );
    pollster::block_on(ctx.read_pixels()).context("Failed to read rendered pixels")
}

/// [`draw_pixels`] as an image.
fn draw_frame(
    ctx: &HeadlessContext,
    renderer: &mut SceneRenderer,
    camera: &Camera,
    config: Option<flint_render::PostProcessConfig>,
) -> Result<image::RgbaImage> {
    let pixels = draw_pixels(ctx, renderer, camera, config)?;
    image::RgbaImage::from_raw(ctx.width, ctx.height, pixels)
        .context("Failed to create image from pixel data")
}

/// `--ai-refine` (#357): run the loop, then write the refined look, its log
/// and the sheet of rounds.
fn refine_look(
    args: &RenderArgs,
    look: super::look::Look,
    reference: image::RgbImage,
    draw: &mut dyn FnMut(&toml::Table) -> Result<image::RgbaImage>,
) -> Result<()> {
    use super::look_refine as lr;
    let (agent, meter) =
        lr::build_agent(&args.ai_provider, args.ai_model.as_deref(), args.max_cost)?;
    let name = format!("{}_refined", look.name);
    let out = args
        .refine_out
        .clone()
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| lr::default_out(&args.output, &name));
    let reference_path = args.reference.clone().unwrap_or_default();
    let log = lr::run(
        lr::Options {
            look: look.clone(),
            reference_path,
            reference,
            rounds: args.refine_rounds.max(1),
            candidates: args.refine_candidates.max(1),
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
        look.name, log.start.score, log.end.score, log.stopped, log.calls, log.cost_usd
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
    world: &'a flint_ecs::FlintWorld,
    camera: Camera,
    camera_path: Option<&'a super::frame_seq::CameraPath>,
    particles: Option<&'a mut flint_particles::ParticleSystem>,
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
    } = rig;
    let dt = args.frame_dt.unwrap_or(1.0 / 60.0);
    let wants_flips = args.flip_stats || args.flip_json.is_some();
    let wants_text = args.ascii_text.is_some() || args.ascii_text_ansi.is_some();
    let base_time = renderer.ocean_time;
    let yaw0 = camera.yaw;
    let (width, height) = (ctx.width, ctx.height);
    if !args.no_images {
        if let Some(dir) = Path::new(&args.output).parent() {
            if !dir.as_os_str().is_empty() {
                std::fs::create_dir_all(dir)
                    .context(format!("Failed to create {}", dir.display()))?;
            }
        }
    }
    let number = |p: &Option<String>, i: u32| {
        p.as_ref()
            .map(|p| numbered(p, i, frames).to_string_lossy().into_owned())
    };
    let mut flips = FlipCounter::default();
    let motion = match (args.orbit, camera_path) {
        (Some(rate), _) => format!(", orbiting {rate} deg/s"),
        (None, Some(path)) => format!(", on a {}-key camera path", path.key.len()),
        _ => String::new(),
    };
    say!("Sequence: {frames} frames, {dt:.6} s apart{motion}");
    for i in 0..frames {
        let t = i as f32 * dt;
        if let Some(rate) = args.orbit {
            camera.yaw = yaw0 + (rate * t).to_radians();
            camera.update_orbit();
        } else if let Some(path) = camera_path {
            let (position, target) = path.at(t);
            camera.position = position;
            camera.target = target;
        }
        renderer.ocean_time = base_time + t as f64;
        renderer.grass_time = t;
        // History smoothing scales by the frame's game time (#342)
        renderer.style_dt = dt;
        if let Some(p) = particles.as_deref_mut() {
            if i > 0 {
                p.simulate_to(world, dt, dt.min(1.0 / 60.0));
            }
            p.pack(Some(camera.position_array()));
            renderer.update_particles_from(&ctx.device, &ctx.queue, &p.sync);
        }
        renderer.render_to(
            &ctx.device,
            &ctx.queue,
            ctx.depth_target(),
            &camera,
            &ctx.color_view,
        );
        if wants_text || wants_flips {
            let text = renderer.style_text(&ctx.device, "ascii");
            if wants_flips {
                let Some(text) = &text else {
                    anyhow::bail!(
                        "--flip-stats: the ASCII style didn't draw frame {i}; turn it on with \
                         --ascii, a look, or the scene's [post_process.styles.ascii]"
                    );
                };
                let f = flips.push(text);
                if args.flip_stats {
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
                    number(&args.ascii_text, i).as_deref(),
                    number(&args.ascii_text_ansi, i).as_deref(),
                    text,
                )?;
            }
        }
        if !args.no_images {
            let pixels =
                pollster::block_on(ctx.read_pixels()).context("Failed to read rendered pixels")?;
            let img = image::RgbaImage::from_raw(width, height, pixels)
                .context("Failed to create image from pixel data")?;
            let path = numbered(&args.output, i, frames);
            img.save(&path)
                .context(format!("Failed to save image to {}", path.display()))?;
        }
    }
    if !args.no_images {
        say!(
            "Rendered {frames} {width}x{height} frames to {} .. {}",
            numbered(&args.output, 0, frames).display(),
            numbered(&args.output, frames - 1, frames).display()
        );
    }
    if wants_flips {
        let stats = flips.stats();
        say!("Flips: {}", stats.summary());
        if let Some(path) = &args.flip_json {
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
            text.cols, text.rows
        );
    }
    Ok(())
}

/// `def` (the scene's block under any look and sweep column, or none) as
/// the renderer's config, with the CLI's post flags and `--style` words
/// over it.
fn post_process_config(
    args: &RenderArgs,
    def: Option<&flint_scene::PostProcessDef>,
) -> Result<flint_render::PostProcessConfig> {
    {
        let mut pp_config = if let Some(pp_def) = def {
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
        // The --kuwahara-* flags write Kuwahara's style table (#298)
        let kuwahara = [
            ("radius", args.kuwahara_radius.map(|r| r as f32)),
            ("sharpness", args.kuwahara_sharpness),
            ("hardness", args.kuwahara_hardness),
            ("anisotropy", args.kuwahara_anisotropy),
        ];
        for (key, value) in kuwahara {
            if let Some(value) = value {
                pp_config.styles.set_param("kuwahara", key, value);
                pp_config.styles.set_enabled("kuwahara", true);
            }
        }
        if let Some(depth) = args.edge_depth {
            pp_config.edge_depth = depth;
        }
        if let Some(normal) = args.edge_normal {
            pp_config.edge_normal = normal;
        }
        apply_boil_flags(args, &mut pp_config);
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
        apply_ascii_style_flags(args, &mut pp_config)?;
        // `--style name[:k=v,...]` (#271) last: an explicit word wins
        args.style.apply(&mut pp_config);

        Ok(pp_config)
    }
}

/// The shading boil's and the contour's flags (#214) over `pp`. Applied again
/// over a sweep column's preset, whose `[boil]` they win over.
fn apply_boil_flags(args: &RenderArgs, pp: &mut flint_render::PostProcessConfig) {
    if let Some(boil) = args.kuwahara_boil {
        pp.kuwahara_boil = boil;
    }
    if args.contour {
        pp.contour_enabled = true;
    }
    let contour = [
        (args.contour_width, &mut pp.contour_width),
        (args.contour_threshold, &mut pp.contour_threshold),
        (args.contour_strength, &mut pp.contour_strength),
        (args.contour_luma, &mut pp.contour_luma),
        (args.contour_fade, &mut pp.contour_fade),
        (args.contour_boil, &mut pp.contour_boil),
    ];
    let mut any = false;
    for (flag, field) in contour {
        if let Some(value) = flag {
            *field = value;
            any = true;
        }
    }
    if let Some(color) = args.contour_color {
        pp.contour_color = color;
        any = true;
    }
    if any {
        pp.contour_enabled = true;
    }
    if let Some(on) = args.boil_on {
        pp.boil_on = on;
    }
}

/// The ASCII style's flags (#265) onto `[post_process.styles.ascii]`.
/// `--ascii`, or any flag that sets the look (font, ramp, colour, scope,
/// edges...), turns the style on; the tuning flags (`--ascii-cell`,
/// `--ascii-levels`, `--ascii-footprint`, `--ascii-no-edges`,
/// `--ascii-edge-depth`, `--ascii-edge-normal`) tune it once it's on,
/// from the flag or the scene.
fn apply_ascii_style_flags(
    args: &RenderArgs,
    pp: &mut flint_render::PostProcessConfig,
) -> Result<()> {
    const NAME: &str = "ascii";
    let style_only = args.ascii_font.is_some()
        || args.ascii_ramp.is_some()
        || args.ascii_color.is_some()
        || args.ascii_tint.is_some()
        || args.ascii_background.is_some()
        || args.ascii_brightness.is_some()
        || args.ascii_contrast.is_some()
        || args.ascii_ink.is_some()
        || args.ascii_palette.is_some()
        || args.ascii_ground.is_some()
        || args.ascii_glow.is_some()
        || args.ascii_edges.is_some()
        || args.ascii_edge_threshold.is_some()
        || args.ascii_edge_max.is_some()
        || args.ascii_dither.is_some()
        || args.ascii_exposure.is_some()
        || args.ascii_normalize.is_some()
        || args.ascii_scope.is_some()
        || args.ascii_grid.is_some();
    if args.ascii || style_only {
        pp.styles.set_enabled(NAME, true);
    }
    if !pp.styles.is_enabled(NAME) {
        return Ok(());
    }
    let styles = &mut pp.styles;
    if let Some(font) = &args.ascii_font {
        styles.set_option(NAME, "font", font);
    }
    if let Some(ramp) = &args.ascii_ramp {
        styles.set_option(NAME, "ramp", ramp);
    }
    if let Some(scope) = &args.ascii_scope {
        styles.set_option(NAME, "scope", scope);
    }
    if let Some(grid) = &args.ascii_grid {
        styles.set_option(NAME, "grid", grid);
    }
    let choices = |option: &str| {
        flint_render::stylize::find(NAME)
            .and_then(|d| d.options.iter().find(|o| o.name == option))
            .map_or(&[][..], |o| o.choices)
    };
    if let Some(color) = &args.ascii_color {
        let modes = choices("color");
        if !modes.contains(&color.as_str()) {
            anyhow::bail!(
                "--ascii-color: unknown mode '{color}' ({})",
                modes.join(", ")
            );
        }
        styles.set_option(NAME, "color", color);
    }
    if let Some(ground) = &args.ascii_ground {
        let modes = choices("ground");
        if !modes.contains(&ground.as_str()) {
            anyhow::bail!(
                "--ascii-ground: unknown mode '{ground}' ({})",
                modes.join(", ")
            );
        }
        styles.set_option(NAME, "ground", ground);
    }
    if let Some(palette) = &args.ascii_palette {
        // Any name goes (a file beside the scene); the style warns once
        // and falls back when it can't be found
        styles.set_option(NAME, "palette", palette);
        if args.ascii_color.is_none() {
            styles.set_option(NAME, "color", "palette");
        }
    }
    if let Some(rgb) = args.ascii_tint {
        styles.set_color(NAME, "tint", rgb);
        if args.ascii_color.is_none() {
            styles.set_option(NAME, "color", "tint");
        }
    }
    for (param, value) in [
        ("cell", args.ascii_cell),
        ("levels", args.ascii_levels),
        ("footprint", args.ascii_footprint),
        ("background", args.ascii_background),
        ("brightness", args.ascii_brightness),
        ("contrast", args.ascii_contrast),
        ("ink", args.ascii_ink),
        ("glow", args.ascii_glow),
        ("edges", args.ascii_edges),
        ("edge_threshold", args.ascii_edge_threshold),
        ("edge_max", args.ascii_edge_max),
        ("edge_depth", args.ascii_edge_depth),
        ("edge_normal", args.ascii_edge_normal),
        ("dither", args.ascii_dither),
        ("exposure", args.ascii_exposure),
        ("normalize", args.ascii_normalize),
    ] {
        if let Some(value) = value {
            styles.set_param(NAME, param, value);
        }
    }
    if args.ascii_no_edges {
        styles.set_param(NAME, "edges", 0.0);
    }
    Ok(())
}

/// What `run_in` hands the stage path, worked out already: the schemas, the
/// scene as it loaded it (the first column's world), the sweep and every
/// column's post-processing.
struct StagedRender<'a> {
    registry: &'a SchemaRegistry,
    first: (flint_ecs::FlintWorld, flint_scene::SceneFile),
    grid: Option<&'a super::sweep::SweepGrid>,
    configs: Vec<flint_render::PostProcessConfig>,
    report_json: bool,
}

/// A render with clocks (#216): a [`Stage`] a sweep column, each captured at
/// every moment asked for, the columns composed into one sheet a moment, and a
/// span written as a GIF.
fn run_stages(args: &RenderArgs, host: &mut RenderHost, staged: StagedRender) -> Result<()> {
    let StagedRender {
        registry,
        first,
        grid,
        configs,
        report_json,
    } = staged;

    // What only the one-renderer path draws so far
    let not_yet = |flag: &str| {
        anyhow!(
            "{flag} isn't drawn with clocks yet: this render runs them (--time, a wooze.* \
             sweep, --scripts, --report json, or a scene with living props or a [wooze] table)"
        )
    };
    if grid.is_some_and(|g| g.rows.is_some()) {
        return Err(not_yet("a second --sweep (a grid)"));
    }
    if args.sweep_crop.is_some() || args.sweep_scale.is_some() {
        return Err(not_yet("--sweep-crop / --sweep-scale"));
    }
    if args.frames.is_some() {
        return Err(not_yet("--frames (use --time 0..N -o x.gif)"));
    }
    if args.ai_refine {
        return Err(not_yet("--ai-refine"));
    }
    if args.reference.is_some() {
        return Err(not_yet("--reference"));
    }
    if args.report.is_some() && !report_json {
        return Err(not_yet("--report FILE.json (--report json is)"));
    }
    if args.ascii_text.is_some() || args.ascii_text_ansi.is_some() {
        return Err(not_yet("--ascii-text"));
    }
    if args.gpu_time.is_some() {
        return Err(not_yet("--gpu-time"));
    }

    let sweep = grid.map(|g| &g.columns);
    let columns = sweep.map_or(1, |s| s.values.len());

    // The scene, once for each column: a world is changed by loading it
    let mut scenes = Vec::with_capacity(columns);
    scenes.push(first);
    for _ in 1..columns {
        scenes.push(load_scene(&args.scene, registry).context("Failed to load scene")?);
    }
    let scene_name = scenes[0].1.scene.name.clone();

    let animated = matches!(args.time, Some(TimeSpec::Range(..)));
    let (default_width, default_height) = render_plan::default_size(sweep.is_some(), animated);
    let (width, height) = (
        args.width.unwrap_or(default_width),
        args.height.unwrap_or(default_height),
    );
    let frame_step = args.frame_step.unwrap_or(render_plan::DEFAULT_FRAME_STEP);

    let ctx = host.context(width, height, &args.gpu)?;

    let timeline = match &args.expression_timeline {
        Some(path) => Some(ExpressionTimeline::load(path)?),
        None => None,
    };
    if let Some(timeline) = &timeline {
        say!("Expression timeline: {} states", timeline.len());
    }

    let mut stages = Vec::with_capacity(columns);
    for (column, ((world, scene_file), config)) in scenes.into_iter().zip(configs).enumerate() {
        stages.push(Stage::load(
            args,
            ctx,
            StageScene {
                world,
                scene_file,
                column,
                sweep,
                config,
                timeline: timeline.clone(),
            },
        )?);
    }

    let times = render_plan::capture_times(args.time.as_ref(), args.fps);
    let mut warnings = stage_warnings(args, sweep, &stages[0]);
    for warning in &warnings {
        eprintln!("Warning: {warning}");
    }

    let mut captures = Vec::new();
    let mut frames: Vec<image::RgbaImage> = Vec::with_capacity(times.len());
    // Whether each pair of neighbouring columns has differed in any frame yet.
    let mut columns_differ = vec![false; columns.saturating_sub(1)];
    for &t in &times {
        let mut cells = Vec::with_capacity(columns);
        for (column, stage) in stages.iter_mut().enumerate() {
            cells.push(stage.capture(args, ctx, t, frame_step)?);
            if report_json {
                captures.push(CaptureReport {
                    time: t,
                    column: sweep.map(|s| s.values[column].clone()),
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
        frames.push(match sweep {
            Some(sweep) => {
                super::style_sheet::compose(&cells, &sheet_text(sweep, &scene_name, t, &stages))
            }
            None => cells.remove(0),
        });
    }

    // What the scripts asked for that isn't in the picture, said once as it
    // happened, and kept for the report.
    if let Some(scripts) = &stages[0].scripts {
        warnings.extend(scripts.notes());
    }

    if let Some(sweep) = sweep {
        for (i, differ) in columns_differ.iter().enumerate() {
            if !differ {
                let warning = sweep.identical_warning(i);
                eprintln!("Warning: {warning}");
                warnings.push(warning);
            }
        }
    }

    // Write the file
    let output = Path::new(&args.output);
    if animated {
        let rgb: Vec<image::RgbImage> = frames
            .iter()
            .map(|f| image::DynamicImage::ImageRgba8(f.clone()).into_rgb8())
            .collect();
        gif_writer::write(output, &rgb, args.fps)?;
        say!(
            "Rendered {} frames of {}x{} at {} fps to {} ({:.1} MB)",
            rgb.len(),
            rgb[0].width(),
            rgb[0].height(),
            args.fps,
            args.output,
            std::fs::metadata(output).map_or(0.0, |m| m.len() as f64 / 1e6),
        );
    } else if matches!(args.time, Some(TimeSpec::List(_))) {
        // A still a moment (#614), numbered in the order the moments were given.
        let count = frames.len();
        for (i, frame) in frames.iter().enumerate() {
            let path = render_plan::numbered_still(&args.output, i, count);
            frame
                .save(&path)
                .context(format!("Failed to save image to {}", path.display()))?;
        }
        say!(
            "Rendered {count} moments ({}) as {}x{} to {} .. {}",
            times
                .iter()
                .map(|t| format!("{t}"))
                .collect::<Vec<_>>()
                .join(", "),
            frames[0].width(),
            frames[0].height(),
            render_plan::numbered_still(&args.output, 0, count).display(),
            render_plan::numbered_still(&args.output, count - 1, count).display()
        );
    } else {
        frames[0]
            .save(&args.output)
            .context(format!("Failed to save image to {}", args.output))?;
        match sweep {
            Some(_) => say!(
                "Rendered {} columns of {width}x{height} as {}x{} to {}",
                columns,
                frames[0].width(),
                frames[0].height(),
                args.output
            ),
            None => say!("Rendered {width}x{height} image to {}", args.output),
        }
    }

    if report_json {
        let field = stages[0].living.as_ref().and_then(|l| l.field());
        let report = render_report::RenderReport {
            scene: scene_name,
            output: args.output.clone(),
            width,
            height,
            frame_step,
            wooze_field: field.is_some(),
            wooze_zones: field.map_or(0, |f| f.zones().len()),
            sweep: sweep.map(super::sweep::Sweep::name),
            captures,
            still_props: still_props(&stages[0]),
            warnings,
        };
        let json = serde_json::to_string(&report).context("serialising the render report")?;
        println!("{json}");
    }
    Ok(())
}

/// Try every column of a wooze sweep on a field of its own, before anything is
/// loaded. A preset column may be a file beside the scene (#249), which has to
/// read.
fn check_wooze_sweep(args: &RenderArgs, sweep: &super::sweep::Sweep) -> Result<()> {
    for index in 0..sweep.values.len() {
        if preset_file_of(args, Some(sweep), index)?.is_some() {
            continue;
        }
        let Some(table) = sweep.wooze_table(index) else {
            continue;
        };
        let mut field = flint_core::wooze::WoozeField::new(flint_core::wooze::WoozeSample::NEUTRAL);
        field
            .set_global(&table, 0.0, 0.0)
            .map_err(|reason| anyhow!("--sweep {}: {reason}", sweep.name()))?;
    }
    Ok(())
}

/// Whether a render is of a moment of the game (#216): a time, or a sweep of the
/// field. It hides the editor's grid and draws shallow wooze.
fn is_moment(args: &RenderArgs, sweep: Option<&super::sweep::Sweep>) -> bool {
    args.time.is_some() || sweep.is_some_and(|s| s.wooze_knob().is_some())
}

/// What a reader of this capture has to be told about what it isn't showing.
fn stage_warnings(
    args: &RenderArgs,
    sweep: Option<&super::sweep::Sweep>,
    stage: &Stage,
) -> Vec<String> {
    let mut warnings = Vec::new();
    if !is_moment(args, sweep) {
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
fn preset_file_of(
    args: &RenderArgs,
    sweep: Option<&super::sweep::Sweep>,
    column: usize,
) -> Result<Option<flint_scene::WoozePreset>> {
    let Some(sweep) = sweep.filter(|sweep| sweep.wooze_knob() == Some("preset")) else {
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

/// What a sweep's sheet says, as of the moment each stage last captured: the
/// moment top right, and under a wooze column's label what the props read.
fn sheet_text(
    sweep: &super::sweep::Sweep,
    scene_name: &str,
    t: f64,
    stages: &[Stage],
) -> super::style_sheet::SheetText {
    let props = stages[0]
        .living
        .as_ref()
        .map_or(0, |run| run.props().props().len());
    let living = if props > 0 {
        format!("   {props} living props")
    } else {
        String::new()
    };
    let mut text = super::style_sheet::SheetText::new(
        scene_name,
        format!("sweep {}{living}", sweep.name()),
        (0..sweep.values.len()).map(|i| sweep.label(i)).collect(),
    );
    text.time = format!("T {t:.2}S");
    if let Some(knob) = sweep.wooze_knob() {
        text.notes = sweep
            .values
            .iter()
            .zip(stages)
            .map(|(value, stage)| {
                stage.living.as_ref().map_or_else(String::new, |run| {
                    render_report::read_note(knob, value.parse().ok(), run.props())
                })
            })
            .collect();
    }
    text
}

/// One column's scene, as [`Stage::load`] takes it.
struct StageScene<'a> {
    world: flint_ecs::FlintWorld,
    scene_file: flint_scene::SceneFile,
    /// Which of the sweep's values this stage takes.
    column: usize,
    sweep: Option<&'a super::sweep::Sweep>,
    /// The column's post-processing: the scene's, under any look and the
    /// column's value, with the CLI's flags over it.
    config: flint_render::PostProcessConfig,
    timeline: Option<ExpressionTimeline>,
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
    /// The moment of the last capture, which the next one's style history
    /// follows on from.
    captured: Option<f64>,
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
    /// Load a column's scene into a new renderer, ready for its first capture.
    fn load(args: &RenderArgs, ctx: &HeadlessContext, scene: StageScene) -> Result<Self> {
        let StageScene {
            mut world,
            scene_file,
            column,
            sweep,
            config: mut pp_config,
            timeline,
        } = scene;
        let moment = is_moment(args, sweep);

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
                show_grid: !args.no_grid && !moment,
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
        let column_preset = preset_file_of(args, sweep, column)?;
        let column_knobs = match &column_preset {
            Some(preset) => Some(preset_wooze_table(&preset.wooze)),
            None => sweep.and_then(|sweep| sweep.wooze_table(column)),
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
                "style" => DebugMode::StyleMask,
                "edges" => DebugMode::StyleEdges,
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
        // actually deviates (ADR 0049).
        if args.shadow_resolution != 2048 {
            renderer.set_shadow_resolution(&ctx.device, args.shadow_resolution);
        }

        // Post-processing: the column's, with a preset column's shading boil
        // under the CLI's boil flags
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
            apply_boil_flags(args, &mut pp_config);
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
            (None, _) if boil_wanted && column == 0 => {
                say!("This scene has no [wooze] table, so nothing boils.");
            }
            _ => {}
        }
        renderer.set_post_process_config(pp_config);
        renderer.ensure_style_resources(&ctx.device);
        renderer.ensure_fxaa_resources(&ctx.device);
        renderer.ensure_contour_resources(&ctx.device);

        if let Some(scale) = args.render_scale {
            renderer.set_render_scale(&ctx.device, scale);
        }
        // Quality settings over the flags above, as in the one-renderer path
        if let Some(quality) = &args.quality {
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
            if column == 0 {
                say!("Quality: {}", applied.to_query());
            }
        }
        if let Some(px) = args.pixels_per_point {
            renderer.pixels_per_point = px;
        }

        // Post time for grain/mode animation and animated styles. Headless
        // renders leave this 0.0 so PNG snapshots stay deterministic;
        // --style-time (or the older --grain-time) pins an explicit frame.
        if let Some(t) = args.style.style_time.or(args.grain_time) {
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
            captured: None,
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
                self.renderer.ensure_style_resources(&ctx.device);
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
            if args.style.style_time.or(args.grain_time).is_none() {
                self.renderer.ocean_time = t;
            }
        }
        // Styles with history (ASCII's, #277) follow on from the last capture
        if let Some(before) = self.captured {
            if t > before {
                self.renderer.style_dt = (t - before) as f32;
            }
        }
        self.captured = Some(t);

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
            ctx.depth_target(),
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
        // A style_mask on the terrain entity masks its chunks (#310)
        renderer.set_terrain_entity(Some(entity.id));

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
                    grass_config.density, grass_config.max_distance
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
