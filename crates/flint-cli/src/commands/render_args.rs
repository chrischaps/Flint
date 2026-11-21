//! `flint render`'s flags (#708): grouped by what they do, so `--help` reads
//! by heading. The per-effect post flags it had are `--set` and `--style`
//! now ([`super::render_post`]); given, each is an error naming its spelling
//! ([`super::render_retired`], #1144).

use super::render_plan::{self, TimeSpec};
use crate::commands::common_args::{parse_debug_mode, parse_vec3};
use flint_render::DebugMode;

const OUTPUT: &str = "Output";
const CAMERA: &str = "Camera";
const POST: &str = "Styles and post-processing";
const QUALITY: &str = "Quality and lighting";
const TIME: &str = "Time";
const SEQUENCE: &str = "Sequences";
const SWEEP: &str = "Sweep and variants";
const MEASURE: &str = "Measure and report";
const REFINE: &str = "Refine a look";
const DEBUG_LISTS: &str = "Debug, lists, GPU and schemas";

/// Every `flint render` flag. Each help heading is its own flattened struct,
/// and that is for the stack as much as for `--help`: in a debug build a
/// derived `augment_args` is one frame of about 8 KB a flag, and the
/// 130-flag struct this was needed 1.3 MB to parse any command line (#708).
/// Keep a struct to a dozen or so flags; `lib.rs`'s `stack_tests` hold the
/// whole parser to 1 MB.
#[derive(clap::Args)]
#[command(group(clap::ArgGroup::new("sequence").multiple(true).args(["frames", "mesh"])))]
#[command(group(clap::ArgGroup::new("span").multiple(true).args(["time", "script_time", "mesh"])))]
#[command(group(clap::ArgGroup::new("clock").multiple(true).args(["time", "script_time"])))]
#[command(group(clap::ArgGroup::new("listing").args(["list_styles", "list_effects"])))]
#[command(after_help = flint_core::env::help_section(&[flint_core::env::Group::Render]))]
pub struct RenderArgs {
    /// Path to scene file
    #[arg(
        required_unless_present_any = ["list_gpus", "look", "list_styles", "list_effects"],
        default_value = ""
    )]
    pub scene: String,

    /// Output image path
    #[arg(short, long, default_value = "render.png", help_heading = OUTPUT)]
    pub output: String,

    /// Image width in pixels [default: 1920; each cell of a --sweep, 640; a GIF, 960]
    #[arg(long, help_heading = OUTPUT)]
    pub width: Option<u32>,

    /// Image height in pixels [default: 1080; each cell of a --sweep, 360; a GIF, 540]
    #[arg(long, help_heading = OUTPUT)]
    pub height: Option<u32>,

    #[command(flatten)]
    pub out: OutputArgs,

    #[command(flatten)]
    pub camera: CameraArgs,

    /// A look over the scene's post-processing (#279): a built-in
    /// (ascii_noir, matrix, ink_wash, graphite, watercolor,
    /// coloured_pencil, blueprint, charcoal, rotoscope), a
    /// `styles/<name>.look.toml` beside the scene, or a path. Merged over
    /// the scene's [post_process] key by key; --set and --style apply over
    /// it. `--look help` lists every look and what to tune
    #[arg(long, help_heading = POST)]
    pub look: Option<String>,

    /// Set one key as the scene would write it (#708), repeatable:
    /// `post.<key>=v`, a [post_process] key (`post.exposure=1.5`,
    /// `post.fog_color=0.7,0.75,0.82`, `post.fxaa=true`; `--list-effects`
    /// lists every key with its kind and range), or `<style>.<key>=v`, a key
    /// of a style's table, leaving the style on or off as it was
    /// (`ascii.cell=12`; `--style` turns a style on, `--list-styles` lists
    /// its keys). The keys `--sweep` takes, one value each. Over the scene
    /// and the look, in the order given; --style applies after
    #[arg(long, value_name = "KEY=VALUE", value_parser = super::render_post::parse_set,
          action = clap::ArgAction::Append, help_heading = POST)]
    pub set: Vec<super::render_post::Set>,

    #[command(flatten, next_help_heading = POST)]
    pub style: crate::commands::common_args::StyleArgs,

    /// Disable post-processing (bloom, vignette, tonemapping in composite pass)
    #[arg(long, help_heading = POST)]
    pub no_postprocess: bool,

    /// The words after `render` on the command line, for --variant to
    /// build each variant's own on. Set by whoever parsed them.
    #[arg(skip)]
    pub command_line: Vec<std::ffi::OsString>,

    #[command(flatten)]
    pub shading: ShadingArgs,

    #[command(flatten)]
    pub clock: ClockArgs,

    #[command(flatten)]
    pub strip: StripArgs,

    #[command(flatten)]
    pub seq: SequenceArgs,

    #[command(flatten)]
    pub sweeps: SweepArgs,

    #[command(flatten)]
    pub measure: MeasureArgs,

    #[command(flatten)]
    pub refine: RefineArgs,

    #[command(flatten)]
    pub debug: DebugArgs,

    /// The deprecated render mode (#746), hidden until #1036.
    #[command(flatten)]
    pub render_mode: super::render_post::RenderModeArgs,
}

/// What a render writes besides -o, and --watch.
#[derive(clap::Args)]
#[command(next_help_heading = OUTPUT)]
pub struct OutputArgs {
    /// Physical pixels per point, as a high-DPI window's scale factor:
    /// styles sized in points (ASCII cells) scale by it (default 1)
    #[arg(long)]
    pub pixels_per_point: Option<f32>,

    /// Also write the ASCII style's character grid as UTF-8 text (#317):
    /// one line per cell row, one character per cell, the same cells as
    /// the PNG (a cell the picture passes through is a space). Needs the
    /// ASCII style on (`--style ascii`, a look, or the scene's own ascii)
    #[arg(long, value_name = "FILE")]
    pub ascii_text: Option<String>,

    /// The same grid with 24-bit ANSI colours (each glyph's ink over its
    /// ground), for `cat` in a terminal
    #[arg(long, value_name = "FILE")]
    pub ascii_text_ansi: Option<String>,

    /// Keep running and write -o again on every save (#432): the scene,
    /// its folder, its style folders, its schemas and the files the flags
    /// name (--look, --variants, --reference, --camera-path,
    /// --expression-timeline). A still or a sweep keeps its GPU, scene and
    /// pipelines, and a user style's WGSL is built again on them; anything
    /// else loads the scene again on the same GPU. One line a render, with
    /// its time and any style error. Ctrl-C stops it
    #[arg(long,
          conflicts_with_all = ["ai_refine", "list_gpus", "list_styles", "list_effects"])]
    pub watch: bool,
}

/// Where the camera is and what it draws over the scene.
#[derive(clap::Args)]
#[command(next_help_heading = CAMERA)]
pub struct CameraArgs {
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
    #[arg(long, value_parser = parse_vec3)]
    pub target: Option<[f32; 3]>,

    /// Field of view in degrees
    #[arg(long)]
    pub fov: Option<f32>,

    /// Disable ground grid
    #[arg(long)]
    pub no_grid: bool,

    /// Draw the editor's selection outline around this entity, by name, as
    /// `flint edit` does when it is picked (#602)
    #[arg(long, value_name = "ENTITY")]
    pub select: Option<String>,
}

/// The renderer's quality and lighting levers.
#[derive(clap::Args)]
#[command(next_help_heading = QUALITY)]
pub struct ShadingArgs {
    /// Quality settings as `key=value,...`, as flint-player's --quality
    /// (FlintKart#55), applied after every other flag: e.g.
    /// `render_scale=0.6,shadow_cascades=1,ssao=0,bloom=0` renders a tier
    #[arg(long, value_name = "KEY=VALUE,...")]
    pub quality: Option<String>,

    /// Draw the scene at this fraction of the image on each side
    /// (0.25-1), upscaled in the composite, as flint-player's
    /// --render-scale (#122)
    #[arg(long)]
    pub render_scale: Option<f32>,

    /// MSAA sample count for the scene passes: 1 (off) or 4 (ADR 0058).
    /// Default 1 keeps headless pixel gates single-sample.
    #[arg(long, default_value = "1")]
    pub msaa: u32,

    /// Disable shadow mapping
    #[arg(long)]
    pub no_shadows: bool,

    /// Shadow map resolution per cascade (default: 2048, the renderer's
    /// construction default)
    #[arg(long, default_value = "2048")]
    pub shadow_resolution: u32,

    /// Disable tone mapping for raw linear output
    #[arg(long)]
    pub no_tonemapping: bool,

    /// Oren-Nayar diffuse blend (0 = legacy Lambert, 1 = full Oren-Nayar;
    /// sigma comes from material roughness)
    #[arg(long)]
    pub oren_nayar: Option<f32>,

    /// Charlie-sheen rim strength (0 = off; keep <= ~0.3)
    #[arg(long)]
    pub sheen_strength: Option<f32>,

    /// Charlie-sheen rim tint as comma-separated R,G,B (default: 1,1,1)
    #[arg(long, value_parser = parse_vec3)]
    pub sheen_color: Option<[f32; 3]>,
}

/// The game time a render captures, and what runs to reach it.
#[derive(clap::Args)]
#[command(next_help_heading = TIME)]
pub struct ClockArgs {
    /// The game time to capture: living props, the wooze field and everything else
    /// a clock drives, as the player has them at that time. Seconds (3.5), a span
    /// (0..4) for an animated --output x.gif or a PNG a frame to --output
    /// frames/f-%05d.png, in one frame's memory however long, or a list (2,5,9.5) of
    /// moments of one run, each written as a still beside --output (x.png gives
    /// x_0000.png...). Physics and animation clips don't run, and scripts only with
    /// --scripts. Same value, same pixels
    #[arg(long, value_parser = render_plan::parse_time)]
    pub time: Option<TimeSpec>,

    /// Run the scene's scripts as flint-player does, one tick a --frame-step, so
    /// what they write to the world, the wooze field and Living Props is in the
    /// picture. random() is seeded and scripts run in the order of their entities'
    /// names: same scene, same pixels. The HUD is drawn over it as the player
    /// draws it: on_draw_ui's draw calls and the UI documents scripts load (#731).
    /// Sound, physics and scene changes don't run, and are named on stderr when a
    /// script asks for them
    #[arg(long)]
    pub scripts: bool,

    /// Show a UI document's element (and what is inside it) before the first
    /// step, as a script's ui_show would: a menu nothing opens without input,
    /// such as a pause menu. Repeatable. Implies --scripts
    #[arg(long, value_name = "ELEMENT")]
    pub ui_show: Vec<String>,

    /// Give a UI document's element focus before the first step, as the arrows
    /// or a pad would, so the capture shows its focused state. Implies --scripts
    #[arg(long, value_name = "ELEMENT")]
    pub ui_focus: Option<String>,

    /// Fail (exit 1) when any script failed, to load, to compile or at run
    /// time (#693), after writing the picture. Implies --scripts. Every
    /// error is counted either way: once each on stderr with its count, and
    /// in --report json's script_errors
    #[arg(long)]
    pub strict_scripts: bool,

    /// Run the scene's scripts to game time T and capture there: --scripts
    /// --time T in one flag, beside --particle-time and --anim-time (#371), so a
    /// style beat, a flickering light or a door a script drives is in the
    /// picture. Takes what --time takes: 3, a span 0..8 with -o x.gif, a list 2,5
    #[arg(long, value_name = "T", value_parser = render_plan::parse_time,
          conflicts_with = "time")]
    pub script_time: Option<TimeSpec>,

    /// An expression timeline (`{"events": [{"t": 0, ...}, ...]}`, as
    /// flint-player's flag) whose states scripts hear through expression() and
    /// on_expression(state), each once the clock reaches its t. Implies --scripts
    #[arg(long, value_name = "FILE")]
    pub expression_timeline: Option<String>,

    /// With --time or --script-time: the seconds each clock tick moves, as
    /// flint-player's --frame-step, to reproduce a player run to the bit [default: 1/60]
    #[arg(long, requires = "clock", value_parser = parse_frame_step)]
    pub frame_step: Option<f64>,

    /// With a --time span: frames per second of game time in the GIF. With
    /// --mesh: frames per second of its GIF, and (unless --frame-dt) the
    /// time post effects and particles step between its frames
    #[arg(long, requires = "span", default_value_t = 12.0,
          value_parser = render_plan::parse_gif_fps)]
    pub fps: f32,

    /// Run the scene's animation for N seconds before capturing, as flint-player
    /// has it at that time: skeletal clips, blend layers, sequences, property
    /// tweens, node and sprite clips, then two-bone IK. Fixed 1/60 s steps (or
    /// --frame-step's): same value, same pixels. With --time or --frames it runs
    /// on with the clock from there. Default: none, so skinned characters draw
    /// in their bind pose and existing snapshots are unchanged
    #[arg(long, value_name = "SECONDS")]
    pub anim_time: Option<f64>,

    /// Simulate particle emitters and effects for N seconds at a fixed
    /// 1/60 s step before capturing (deterministic: same value, same
    /// pixels). Default: no particles, so existing snapshots are unchanged
    #[arg(long)]
    pub particle_time: Option<f32>,

    /// Seconds of the scene's wooze field to run before capturing, which picks
    /// the boil's drawing (default 0: same value, same pixels)
    #[arg(long)]
    pub boil_time: Option<f64>,

    /// Post time in seconds for grain/mode animation (default 0.0 —
    /// headless renders are deterministic; two renders at the same value
    /// are identical, different values differ)
    #[arg(long)]
    pub grain_time: Option<f32>,
}

/// `--strip` (#574): a span as one contact sheet, for a reviewer that can
/// open a PNG but sees only a GIF's first frame.
#[derive(clap::Args)]
#[command(next_help_heading = TIME)]
pub struct StripArgs {
    /// With a --time span or list: write one PNG contact sheet of it to
    /// --output (#574), a tile every --every seconds, each labelled with
    /// its time. A span's end is a tile when an --every lands on it (0..8
    /// --every 1 is 9 tiles). On the GIF's clock (--frame-step), and each
    /// tile drawn as a first frame, so a tile is the pixels of --time
    /// <its t> rendered alone at the tile's size. Tiles default to 640x360
    #[arg(long, requires = "clock",
          conflicts_with_all = ["sweep", "frames", "mesh", "variant", "variants", "ai_refine"])]
    pub strip: bool,

    /// With --strip and a span: seconds of game time from one tile to the
    /// next [default: 1]
    #[arg(long, value_name = "SECONDS", requires = "strip", value_parser = parse_every)]
    pub every: Option<f64>,

    /// With --strip: tiles a row [default: the fewest that make the sheet
    /// no taller than it is wide]
    #[arg(long = "columns", visible_alias = "cols", requires = "strip",
          value_parser = clap::value_parser!(u32).range(1..))]
    pub columns: Option<u32>,

    /// With --strip: `off` draws the tiles alone, no title and no times, 4
    /// pixels apart with no border, so tile (c, r) of WxH starts at
    /// (c*(W+4), r*(H+4))
    #[arg(long, requires = "strip", value_enum, default_value_t = StripLabel::On)]
    pub label: StripLabel,
}

/// `--label`: whether a strip writes what each tile is.
#[derive(clap::ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum StripLabel {
    On,
    Off,
}

fn parse_every(text: &str) -> Result<f64, String> {
    match text.parse::<f64>() {
        Ok(every) if every.is_finite() && every > 0.0 => Ok(every),
        _ => Err(format!(
            "'{text}' isn't a strip's spacing: it's seconds, more than 0"
        )),
    }
}

/// A sequence of frames on one renderer: --frames or --mesh.
#[derive(clap::Args)]
#[command(next_help_heading = SEQUENCE)]
pub struct SequenceArgs {
    /// Draw a sequence of N frames on one renderer (#345), written as
    /// numbered PNGs beside --output (`seq.png` gives `seq_0000.png`...).
    /// Temporal styles (ASCII's history, #277) see consecutive frames, so
    /// they behave as in the player. Move the camera with --orbit or
    /// --camera-path
    #[arg(long, value_parser = clap::value_parser!(u32).range(1..))]
    pub frames: Option<u32>,

    /// With --frames: seconds between frames (default 1/60). Post time,
    /// grass wind and particles advance by it too
    #[arg(long, requires = "sequence")]
    pub frame_dt: Option<f32>,

    /// With --frames: orbit the camera about its target at this many
    /// degrees a second (negative turns the other way)
    #[arg(
        long,
        requires = "sequence",
        allow_hyphen_values = true,
        conflicts_with = "camera_path",
        help_heading = SEQUENCE
    )]
    pub orbit: Option<f32>,

    /// With --frames: a TOML file of `[[key]] time/position/target`, the
    /// camera interpolated linearly between keys
    #[arg(long, requires = "sequence")]
    pub camera_path: Option<String>,

    /// With --frames: write only the last frame, to --output itself (not
    /// numbered): where the sequence ends, a temporal style's history
    /// settled (#478)
    #[arg(long, requires = "frames", conflicts_with_all = ["no_images", "mesh"],
          help_heading = SEQUENCE)]
    pub last_frame: bool,

    /// With --frames: write no PNGs (for a --flip-stats measurement)
    #[arg(long, requires = "sequence")]
    pub no_images: bool,

    /// With --frames: count the ASCII style's glyph flips and A-B-A
    /// reversals frame to frame, exactly, from its own character grid,
    /// and print them per frame and in sum
    #[arg(long, requires = "sequence")]
    pub flip_stats: bool,

    /// With --frames: write the flip counts (as --flip-stats) to this JSON
    #[arg(long, requires = "sequence")]
    pub flip_json: Option<String>,

    /// Draw a frame per mesh file (#222): `<entity>=<files>` shows each file
    /// in turn on that entity, all on one renderer. Files are one GLB, a
    /// folder of them or a wildcard in the name (`out/f*.glb`), in natural
    /// order (f2 before f10); a GLB of several nodes is baked into one mesh.
    /// Repeat it to move several entities at once, frame for frame. Output as
    /// --frames (numbered PNGs), or `-o x.gif` at --fps, or `--sheet`
    #[arg(long, value_name = "ENTITY=FILES", value_parser = super::mesh_seq::parse_mesh_arg,
          help_heading = SEQUENCE)]
    pub mesh: Vec<super::mesh_seq::MeshArg>,

    /// With --mesh: write one labelled contact sheet of the frames (each
    /// cell named for its file) to --output instead of a file a frame
    #[arg(long, requires = "mesh")]
    pub sheet: bool,
}

/// One render's settings side by side: a sweep or variants.
#[derive(clap::Args)]
#[command(next_help_heading = SWEEP)]
pub struct SweepArgs {
    /// Render one setting at several values as one labelled contact sheet
    /// (#279, #353): `ascii.cell=6,8,12` (any `--style` key),
    /// `post.exposure=0.8,1.2` (a [post_process] field; split on `|` to sweep
    /// a vector), `look=ascii_noir,matrix`, or a Living Props knob,
    /// `wooze.glide=0,0.5,1` (#216: .amplitude, .step, .glide, .jitter and
    /// .preset; it sets the scene's global value, and zones still apply), or
    /// `living.echoes.spread=0,0.9,1.8` (#531: .count, .spread and .alpha,
    /// each column drawing what the scene would with that `[living] echoes`).
    /// --width/--height size each cell. Give it twice for a grid (#358):
    /// the first key across, the second down
    #[arg(long, value_parser = super::sweep::parse_sweep, action = clap::ArgAction::Append,
          help_heading = SWEEP)]
    pub sweep: Vec<super::sweep::Sweep>,

    /// A sweep's cells at the final render's scale (#358): draw each at
    /// --width/--height (default 1920x1080) and keep this region pixel for
    /// pixel, X,Y,W,H. Point-sized styles (ASCII cells) then draw as many
    /// glyphs across it as the final render does there. In pixels, or with
    /// a decimal point as fractions of the frame (0.4,0.48,0.33,0.33), the
    /// same region at any size (#501). The sheet's header maps the region
    /// onto the whole frame
    #[arg(long, value_parser = super::sweep::parse_crop, conflicts_with = "sweep_scale",
          help_heading = SWEEP)]
    pub sweep_crop: Option<super::sweep::CellView>,

    /// `full`: draw each sweep cell at --width/--height (default 1920x1080)
    /// and shrink it to 640 wide, so the whole frame is composed as the final
    /// render is (glyphs blur); `cell` (default): each cell a 640x360 frame
    #[arg(long, value_parser = ["cell", "full"])]
    pub sweep_scale: Option<String>,

    /// Render the scene again under other flags, at the same camera, and
    /// write one labelled sheet to -o (#291): `"label: --flag value ..."`,
    /// repeatable. Each variant is this command line with its flags after
    /// it (a flag in both takes the variant's; --set and --style add), and
    /// its own frame is written beside the sheet as `<out>-<label>.png`. All
    /// in one process on one GPU device
    #[arg(long, value_parser = super::variants::parse_variant,
          action = clap::ArgAction::Append, allow_hyphen_values = true,
          conflicts_with_all = ["ai_refine", "frames", "mesh"])]
    pub variant: Vec<super::variants::Variant>,

    /// A file of variants (#291): `[[variant]]` tables of `label` and
    /// `flags` (a line, or an array of words), with an optional `title` and
    /// `zoom = ["X0,Y0,X1,Y1@N"]`. Its variants come before any --variant
    #[arg(long, value_name = "FILE.toml", conflicts_with_all = ["ai_refine", "frames", "mesh"],
          help_heading = SWEEP)]
    pub variants: Option<String>,

    /// With --variant(s): also crop this region of every variant's frame,
    /// X0,Y0,X1,Y1 in its pixels, drawn N times larger with nearest
    /// neighbour (`@N`; default as large as fits 640 across). Writes
    /// `<out>-zoom.png` (labelled) and `<out>-<label>-zoom.png` a variant,
    /// and outlines the region on the main sheet. Repeatable
    #[arg(long, value_parser = super::variants::parse_zoom, action = clap::ArgAction::Append,
          help_heading = SWEEP)]
    pub zoom: Vec<super::variants::Zoom>,
}

/// What a render measures and reports.
#[derive(clap::Args)]
#[command(next_help_heading = MEASURE)]
pub struct MeasureArgs {
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

    /// The reference's glyph grid, as `--ascii-text` wrote it (#495). With
    /// it, a frame the ASCII style drew is scored by its glyphs (mean
    /// colour, glyph coverage, glyph layout) instead of its pixels, so a
    /// halo that smears ink into the ground isn't read as the letters
    /// changing. `--reference x.png --ascii-text x.txt` on the reference's
    /// own render writes both
    #[arg(long, value_name = "TXT", requires = "reference")]
    pub reference_text: Option<String>,

    /// Render this many frames with GPU timing on and print each pass's
    /// min, p50, p90 and max GPU time over them, the first tenth (at most
    /// 30) dropped as warm-up, before writing the image (#292)
    #[arg(long)]
    pub gpu_time: Option<u32>,

    /// With --gpu-time: also write the timing as JSON to this file, every
    /// kept frame's raw time per pass included
    #[arg(long, requires = "gpu_time")]
    pub gpu_time_json: Option<String>,

    /// Write the rendered frame's description as JSON to this file
    /// (#1084, ADR 0126): every pass in the order drawn, with the targets
    /// it read and wrote, its draws by entity, what a cull set aside and
    /// why, why a pass was skipped, and its GPU time in that frame; then
    /// the frame's settings, targets, lights and stats. Of a span, the
    /// last frame; not with a sweep or a sequence
    #[arg(long)]
    pub frame_dump: Option<String>,

    /// Capture the rendered frame into this folder (#1085, ADR 0126): its
    /// description as `manifest.json` (written last) and, under
    /// `targets/`, every target each pass wrote, as that pass left it:
    /// 8-bit as PNG, float as OpenEXR half, depth as OpenEXR f32 (`.flintframe`
    /// by convention). The last image is the render's output byte for byte.
    /// Of a span, the last frame; not with a sweep or a sequence
    #[arg(long)]
    pub frame_capture: Option<String>,

    /// Time the load (#1092): the scene read, the GPU device, the
    /// renderer, every model, texture, pipeline and script, up to the first
    /// capture. Prints a table of each stage's ms, share and count and the
    /// five slowest assets by name; `--report json` carries it as `load`
    #[arg(long)]
    pub load_report: bool,

    /// `--skip-pass LABEL` and `--repeat-pass LABEL:N` (#1317): draw a
    /// part of the frame not at all, or N times, by its `--gpu-time` name,
    /// so its cost shows on a GPU whose timestamps can't be read; with
    /// `--gpu-time`, a repeated part's row grows N-fold
    #[command(flatten)]
    pub pass_knobs: flint_player::PassKnobArgs,
}

/// `--ai-refine`: a look refined toward a reference.
#[derive(clap::Args)]
#[command(next_help_heading = REFINE)]
pub struct RefineArgs {
    /// Refine --look toward --reference (#357): an agent proposes edits,
    /// each is rendered and measured, the best is kept while the distance
    /// falls. Writes the refined look (--refine-out), a JSON log beside it
    /// and a sheet of the rounds (-o). Calls go through the cost meter and
    /// ledger (.flint/procgen_costs.json)
    #[arg(long, requires_all = ["look", "reference"],
          conflicts_with_all = ["sweep", "frames", "mesh"])]
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
}

/// Debug views, the lists, the GPU and the schemas.
#[derive(clap::Args)]
#[command(next_help_heading = DEBUG_LISTS)]
pub struct DebugArgs {
    /// Debug visualization mode: wireframe, wireframe-overlay, normals,
    /// depth, uv, unlit, metalrough, style, edges
    #[arg(long, value_parser = parse_debug_mode)]
    pub debug_mode: Option<DebugMode>,

    /// Enable wireframe overlay on solid geometry
    #[arg(long)]
    pub wireframe_overlay: bool,

    /// Show face-normal direction arrows
    #[arg(long)]
    pub show_normals: bool,

    /// List every registered style (#303): its stage, where a scene sets
    /// it, and each parameter's default and range, then exit. With a
    /// scene, the user styles in its `styles/` folder are listed too
    #[arg(long)]
    pub list_styles: bool,

    /// List every built-in post effect (#741): each `[post_process]` key
    /// with its kind, default, the range its effect is tuned for and its
    /// help, by effect, then exit. Each is a `--set post.<key>=v`
    #[arg(long, conflicts_with = "list_styles")]
    pub list_effects: bool,

    /// GPU adapter to use (case-insensitive substring of the adapter name,
    /// e.g. "NVIDIA"). Overrides WGPU_ADAPTER_NAME; default prefers the
    /// high-performance adapter. Use --list-gpus to see names.
    #[arg(long)]
    pub gpu: Option<String>,

    /// List available GPU adapters and exit
    #[arg(long)]
    pub list_gpus: bool,

    /// Schema directories, later ones overriding earlier (repeatable).
    /// Without it: the project's flint.toml `schemas`, else schemas/ in the
    /// working directory, beside the scene and one level up
    #[arg(long, action = clap::ArgAction::Append)]
    pub schemas: Vec<String>,
}

fn parse_frame_step(text: &str) -> Result<f64, String> {
    match text.parse::<f64>() {
        Ok(step) if step.is_finite() && step > 0.0 => Ok(step),
        _ => Err(format!(
            "'{text}' isn't a frame step: it's seconds, more than 0"
        )),
    }
}
