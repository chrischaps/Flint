//! `flint gen`'s arguments (#1543): in every build, so `flint gen --help`
//! is the same whichever packages a flint was built with. The command
//! itself is `gen.rs`, the procgen package's; without it `gen_absent.rs`
//! stands in, running a glyph set (`glyph_preview.rs`, the kernel's) and
//! naming the package for anything else.

use clap::{ArgGroup, Args, ValueEnum};

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
    ArgGroup::new("ai_step")
        .args(["ai_assist", "ai_refine", "ai_create"])
        .multiple(true)
))]
pub struct GenArgs {
    /// Path to a .procgen.toml spec file (not needed with --ai-create,
    /// --cost-report, --list-generators or --describe), or a .glyphs.toml
    /// glyph set to preview: its atlas, ramps and a specimen as one sheet
    /// (-o x.png) or every piece (-o dir/), or a .terrain.toml, which is a
    /// terrain_v1 spec: its 16-bit heightmap and splat map, x_height.png and
    /// x_splat.png for -o x.png
    #[arg(required_unless_present_any = ["ai_create", "cost_report", "list_generators", "describe"])]
    pub spec: Option<String>,

    /// List every generator a spec's `generator` may name: its output, its
    /// revision, how many parameters it takes and whether it can live
    #[arg(
        long,
        conflicts_with_all = ["spec", "ai_create", "cost_report", "describe"],
        help_heading = "Generators"
    )]
    pub list_generators: bool,

    /// Print every parameter a generator reads: type, range, default and
    /// what it does, grouped by the mode that reads it (texture_v1: each
    /// pipeline op too, with its ports)
    #[arg(
        long,
        value_name = "GENERATOR",
        conflicts_with_all = ["spec", "ai_create", "cost_report"],
        help_heading = "Generators"
    )]
    pub describe: Option<String>,

    /// With --describe: the parameters as Markdown tables, as the
    /// flint-procgen README's catalog prints them. (The global --json gives
    /// either catalog as one JSON document: --describe's is the schema
    /// itself, and texture_v1's ops.)
    #[arg(long, requires = "describe", help_heading = "Generators")]
    pub markdown: bool,

    /// Ask the AI agent to review the spec (interpret) and then improve it
    /// (refine) before generating; suggestions are printed to stderr
    #[arg(long, help_heading = "AI assistance")]
    pub ai_assist: bool,

    /// Create a new spec from a description, then generate it
    #[arg(
        long,
        value_name = "DESCRIPTION",
        conflicts_with = "spec",
        help_heading = "AI assistance"
    )]
    pub ai_create: Option<String>,

    /// Refine the spec against the --reference image before generating
    #[arg(long, requires = "reference", help_heading = "AI assistance")]
    pub ai_refine: bool,

    /// Reference image (PNG, JPEG, GIF or WebP) for the refine step
    #[arg(
        long,
        value_name = "IMAGE",
        requires = "ai_step",
        help_heading = "AI assistance"
    )]
    pub reference: Option<String>,

    /// With --ai-create: the generator to write for (tree_v1, texture_v1,
    /// creature_v1). Guessed from the description when omitted
    #[arg(
        long,
        value_name = "NAME",
        requires = "ai_create",
        help_heading = "AI assistance"
    )]
    pub ai_generator: Option<String>,

    /// AI provider: anthropic or mock [default: anthropic, or
    /// .flint/procgen_ai.toml's provider]
    #[arg(long, value_name = "PROVIDER", help_heading = "AI assistance")]
    pub ai_provider: Option<String>,

    /// Model to ask [default: .flint/procgen_ai.toml's model, else the
    /// provider's default]
    #[arg(long, value_name = "MODEL", help_heading = "AI assistance")]
    pub ai_model: Option<String>,

    /// Most rounds per AI step before the agent gives up [default: 3]
    #[arg(
        long,
        value_name = "N",
        value_parser = clap::value_parser!(u32).range(1..=20),
        help_heading = "AI assistance"
    )]
    pub ai_iterations: Option<u32>,

    /// Most measured refine rounds against --reference. Claude sees a render
    /// of what the spec builds; each round's result is rendered and
    /// measured, and the refine asks again while the distance falls, keeping
    /// the closest spec [default: --ai-iterations]
    #[arg(
        long,
        value_name = "N",
        requires = "reference",
        value_parser = clap::value_parser!(u32).range(1..=8),
        help_heading = "AI assistance"
    )]
    pub refine_rounds: Option<u32>,

    /// Print the final spec (after any AI steps and --seed) to stdout as
    /// TOML; the generation summary moves to stderr
    #[arg(long, conflicts_with = "validate", help_heading = "AI assistance")]
    pub print_spec: bool,

    /// Write the final spec to this file
    #[arg(long, value_name = "PATH", help_heading = "AI assistance")]
    pub save_spec: Option<String>,

    /// Refuse any AI call that could take this run's spend past USD; checked
    /// before every call, with a warning past 80%
    #[arg(long, value_name = "USD", value_parser = parse_usd, help_heading = "Cost tracking")]
    pub max_cost: Option<f64>,

    /// Print what AI calls have cost, from the project's .flint/ai_costs.jsonl
    /// (procgen, looks and assets): total, per provider, per model, per spec
    /// or asset
    #[arg(long, help_heading = "Cost tracking")]
    pub cost_report: bool,

    /// With --cost-report: only calls on or after this date (YYYY-MM-DD)
    #[arg(
        long,
        value_name = "DATE",
        requires = "cost_report",
        help_heading = "Cost tracking"
    )]
    pub since: Option<String>,

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

    /// Style guide TOML path for additional validation constraints
    /// (triangle ceiling, roughness/metallic ranges, palette, require_uvs)
    #[arg(long, requires = "validate")]
    pub style_guide: Option<String>,

    /// Render a [living] spec over time as one labelled contact sheet, a PNG:
    /// hold-and-glide evaluated per frame from the spec's step, glide and
    /// stagger, the whole mesh rebuilt each frame, one fixed camera
    #[arg(
        long,
        help_heading = "Living props",
        conflicts_with_all = [
            "ai_step", "batch", "format", "print_spec", "register", "save_spec", "seed",
            "validate",
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

fn parse_usd(s: &str) -> std::result::Result<f64, String> {
    super::common_args::parse_usd(s)
}

/// The flags that need the ai package (#1333), refused with exit 2 in a
/// build without it.
#[cfg(not(feature = "ai"))]
pub(super) fn refuse_ai(args: &GenArgs) -> anyhow::Result<()> {
    let flag = [
        (args.cost_report, "--cost-report"),
        (args.ai_create.is_some(), "--ai-create"),
        (args.ai_assist, "--ai-assist"),
        (args.ai_refine, "--ai-refine"),
        (args.style_guide.is_some(), "--style-guide"),
    ]
    .into_iter()
    .find_map(|(given, flag)| given.then_some(flag));
    match flag {
        Some(flag) => Err(super::absent::missing(format!("flint gen {flag}"), "ai")),
        None => Ok(()),
    }
}
