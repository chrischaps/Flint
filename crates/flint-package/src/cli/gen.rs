//! `flint gen`'s arguments (#1543, #1557): the kernel's command, in every
//! build, so `flint gen --help` is the same whichever packages a flint was
//! built with. Here, in the Package API, because the procgen package runs
//! it: flint-cli runs a glyph set (the kernel's) and hands anything else to
//! the [`SpecGen`](super::SpecGen) the procgen package registered, or
//! names the package when none did.
//!
//! The ai package's flags are its own groups (#1592): `GenArgs` declares
//! three slots where their headings have always been ([`GenAiFlags`] for
//! "AI assistance", [`GenCostFlags`] for "Cost tracking", [`GenGuideFlags`]
//! for `--style-guide`), so a flint without the package has none of them
//! and `flint gen --help` of the stock build reads as it did. What ties
//! them to procgen's flags is on the package's side: a flag that makes a
//! spec needless joins the [`NO_SPEC`] group, and its conflicts with
//! `--list-generators`, `--describe` and `--strip` are its own (clap's
//! conflicts are two-way).

use super::flags::{brought, FlagPlace, PackageFlags};
use clap::{ArgGroup, Args, ValueEnum};

/// The group of `gen`'s flags that make the spec needless:
/// `--list-generators`, `--describe`, and a package's that join it (the ai
/// package's `--ai-create` and `--cost-report`).
pub const NO_SPEC: &str = "no_spec";

/// The ai package's "AI assistance" slot of `gen`.
pub struct GenAiPlace;

impl FlagPlace for GenAiPlace {
    const PATH: &'static [&'static str] = &["gen"];
    const SLOT: &'static str = "ai";
}

/// The ai package's "Cost tracking" slot of `gen`.
pub struct GenCostPlace;

impl FlagPlace for GenCostPlace {
    const PATH: &'static [&'static str] = &["gen"];
    const SLOT: &'static str = "cost";
}

/// The slot of `gen`'s validation flags a package adds (the ai package's
/// `--style-guide`, whose guide its crate reads).
pub struct GenGuidePlace;

impl FlagPlace for GenGuidePlace {
    const PATH: &'static [&'static str] = &["gen"];
    const SLOT: &'static str = "guide";
}

pub type GenAiFlags = PackageFlags<GenAiPlace>;
pub type GenCostFlags = PackageFlags<GenCostPlace>;
pub type GenGuideFlags = PackageFlags<GenGuidePlace>;

/// The heading of `--print-spec` and `--save-spec`, procgen's own: under
/// "AI assistance" beside the AI's flags, since what they keep is the
/// AI's spec as often as not; among the options in a flint without them.
fn spec_heading() -> Option<&'static str> {
    brought::<GenAiPlace>().then_some("AI assistance")
}

/// The spec's help, which names the package's flags that make it needless
/// only in a flint that has them.
fn spec_help() -> String {
    let needless = if brought::<GenAiPlace>() && brought::<GenCostPlace>() {
        "--ai-create, --cost-report, --list-generators or --describe"
    } else {
        "--list-generators or --describe"
    };
    format!(
        "Path to a .procgen.toml spec file (not needed with {needless}), or a .glyphs.toml \
         glyph set to preview: its atlas, ramps and a specimen as one sheet (-o x.png) or every \
         piece (-o dir/), or a .terrain.toml, which is a terrain_v1 spec: its 16-bit heightmap \
         and splat map, x_height.png and x_splat.png for -o x.png"
    )
}

/// How `--validate` reports.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum ReportFormat {
    /// One `[ PASS]` / `[ WARN]` / `[ FAIL]` line per check and a summary.
    Human,
    /// One JSON object per variant on stdout (JSON Lines); human output
    /// moves to stderr.
    Json,
}

#[derive(Args)]
#[command(group(
    ArgGroup::new(NO_SPEC)
        .args(["list_generators", "describe"])
        .multiple(true)
))]
pub struct GenArgs {
    #[arg(required_unless_present = NO_SPEC, help = spec_help())]
    pub spec: Option<String>,

    /// List every generator a spec's `generator` may name: its output, its
    /// revision, how many parameters it takes and whether it can live
    #[arg(
        long,
        conflicts_with_all = ["spec", "describe"],
        help_heading = "Generators"
    )]
    pub list_generators: bool,

    /// Print every parameter a generator reads: type, range, default and
    /// what it does, grouped by the mode that reads it (texture_v1: each
    /// pipeline op too, with its ports)
    #[arg(
        long,
        value_name = "GENERATOR",
        conflicts_with = "spec",
        help_heading = "Generators"
    )]
    pub describe: Option<String>,

    /// With --describe: the parameters as Markdown tables, as the
    /// flint-procgen README's catalog prints them. (The global --json gives
    /// either catalog as one JSON document: --describe's is the schema
    /// itself, and texture_v1's ops.)
    #[arg(long, requires = "describe", help_heading = "Generators")]
    pub markdown: bool,

    // The ai package's "AI assistance" group: `--ai-create`,
    // `--ai-assist`, `--ai-refine` and their options.
    #[command(flatten)]
    pub ai: GenAiFlags,

    /// Print the final spec (after any AI steps and --seed) to stdout as
    /// TOML; the generation summary moves to stderr
    #[arg(long, conflicts_with = "validate", help_heading = spec_heading())]
    pub print_spec: bool,

    /// Write the final spec to this file
    #[arg(long, value_name = "PATH", help_heading = spec_heading())]
    pub save_spec: Option<String>,

    // The ai package's "Cost tracking" group: `--max-cost`,
    // `--cost-report` and `--since`.
    #[command(flatten)]
    pub cost: GenCostFlags,

    /// Output path (file or directory). Derived from spec name if omitted.
    #[arg(short, long)]
    pub output: Option<String>,

    /// Override the spec's seed with a fixed value (a batch takes --seed-start)
    #[arg(long, conflicts_with = "batch")]
    pub seed: Option<u64>,

    /// Print estimated cost without generating
    #[arg(long)]
    pub dry_run: bool,

    /// Force output format (auto-detected from extension by default)
    #[arg(long, value_parser = parse_format)]
    pub format: Option<String>,

    /// Number of variants to generate, one after another, with sequential
    /// seeds; each is written as `<stem>_<seed>.<ext>` (or fills `{seed}`)
    #[arg(long, value_parser = clap::value_parser!(u32).range(1..))]
    pub batch: Option<u32>,

    /// First seed of a batch: seeds S, S+1, ..., S+N-1 (default: 0)
    #[arg(long, requires = "batch")]
    pub seed_start: Option<u64>,

    /// Store output(s) in the project's content store (<project>/.flint/assets,
    /// the project found from the working directory or --project), write a
    /// provenance sidecar beside each file, and reuse an earlier registered
    /// result for the same spec + seed instead of generating
    #[arg(long)]
    pub register: bool,

    /// With --register, generate even if this spec + seed is already stored
    #[arg(long, requires = "register")]
    pub force: bool,

    /// Validate output after generation
    #[arg(long)]
    pub validate: bool,

    /// Treat warnings as failures (exit code 1)
    #[arg(long)]
    pub strict: bool,

    /// Validation report format. `json` writes one object per variant to
    /// stdout: spec, seed, output, verdict, counts and every check's name,
    /// status, detail and, where it measured one, `actual` against `limit`
    /// (a ceiling). Everything else goes to stderr.
    #[arg(long, value_enum, requires = "validate")]
    pub report: Option<ReportFormat>,

    // A package's validation flags: the ai package's `--style-guide`.
    #[command(flatten)]
    pub guide: GenGuideFlags,

    /// Render a [living] spec over time as one labelled contact sheet, a PNG:
    /// hold-and-glide evaluated per frame from the spec's step, glide and
    /// stagger, the whole mesh rebuilt each frame, one fixed camera
    #[arg(
        long,
        help_heading = "Living props",
        conflicts_with_all = [
            "batch", "format", "print_spec", "register", "save_spec", "seed", "validate",
        ]
    )]
    pub strip: bool,

    /// With --strip: how many frames
    #[arg(
        long,
        default_value_t = 48,
        requires = "strip",
        value_parser = clap::value_parser!(u32).range(1..=1024),
        help_heading = "Living props"
    )]
    pub frames: u32,

    /// With --strip: frames per second of game time
    #[arg(
        long,
        default_value_t = 24.0,
        requires = "strip",
        value_parser = parse_fps,
        help_heading = "Living props"
    )]
    pub fps: f32,

    /// With --strip: columns in the grid [default: whatever makes a 4:3 sheet]
    #[arg(
        long,
        requires = "strip",
        value_parser = clap::value_parser!(u32).range(1..=256),
        help_heading = "Living props"
    )]
    pub columns: Option<u32>,

    /// With --strip: each frame's width and height in pixels
    #[arg(
        long,
        default_value_t = 240,
        requires = "strip",
        value_parser = clap::value_parser!(u32).range(32..=2048),
        help_heading = "Living props"
    )]
    pub cell_size: u32,
}

fn parse_fps(s: &str) -> std::result::Result<f32, String> {
    match s.parse::<f32>() {
        Ok(fps) if fps.is_finite() && fps > 0.0 => Ok(fps),
        _ => Err(format!("'{s}' isn't a frame rate above 0")),
    }
}

fn parse_format(s: &str) -> std::result::Result<String, String> {
    match s {
        "glb" | "png" | "wav" => Ok(s.to_string()),
        _ => Err(format!(
            "unsupported format '{s}'; valid values: glb, png, wav"
        )),
    }
}

/// The kernel's model still (`flint edit model.glb --render` without the
/// file): a model auto-framed on the default camera at `width` x
/// `height`. A point flint-cli registers in every build, so a package's
/// step of `gen` can show a model it made (the ai package's
/// `--ai-refine` previews, #1572).
pub struct ModelStill(pub fn(&std::path::Path, u32, u32) -> anyhow::Result<image::RgbaImage>);

/// A dollar amount: `0.50` or `$0.50`, not negative.
pub fn parse_usd(s: &str) -> std::result::Result<f64, String> {
    let v: f64 = s
        .trim()
        .trim_start_matches('$')
        .parse()
        .map_err(|_| format!("'{s}' isn't an amount in USD (e.g. 0.50)"))?;
    if !v.is_finite() || v < 0.0 {
        return Err(format!("--max-cost must be a non-negative amount, not {s}"));
    }
    Ok(v)
}
