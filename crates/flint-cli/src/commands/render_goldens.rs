//! `flint render-suite <file>.goldens.toml` (#271): render a list of
//! scenes with `flint render`, each with its own flags, and compare every
//! image with a checked-in golden PNG. The suite fails when any case moves
//! past its tolerance, so a change to the stylize framework (or anything
//! under it) can't silently shift a look.
//!
//! ```toml
//! [suite]
//! name = "stylize"
//! root = "../../../.."            # cases run from here (scene paths are relative to it)
//! width = 640
//! height = 360
//! args = ["--schemas", "schemas"] # before every case's own
//!
//! [tolerance]                     # the defaults; a case may override any
//! level = 48                      # a pixel "differs" past this many levels (0-255, max channel)
//! share = 0.02                    # ...and the case fails when more than this share of pixels do
//! mean = 3.0                      # ...or when the mean difference passes this
//!
//! [[case]]
//! name = "ascii-full-pbr"
//! scene = "demo/pbr_test.scene.toml"
//! golden = "ascii_pbr_test.png"   # beside the manifest; default "<name>.png"
//! args = ["--style", "ascii"]
//! text = "ascii_pbr_test.txt"     # optional (#317): the ASCII grid, compared exactly
//! text_cells = 0                  # ...or with at most this many cells different
//! ```
//!
//! A case with `matches` has no golden file (#417): it renders the scene a
//! second time with those flags in place of its own and must equal that
//! render, exactly unless it gives a tolerance. One adapter, one run, so
//! it holds on any GPU. With `error`, a user style's error must hold that
//! text afterwards (in this process, `stylize::custom::errors()`, which is
//! what `flint render` prints; with `--isolate`, the render's log):
//!
//! ```toml
//! [[case]]
//! name = "user-style-broken"
//! scene = "crates/flint-cli/tests/golden/fixtures/broken_style.scene.toml"
//! args = ["--no-grid", "--style", "broken"]
//! matches = ["--no-grid"]          # the same frame without the style
//! error = "broken.wgsl:9:9"
//! ```
//!
//! A case with `text` is a structural golden too: it renders with
//! `--ascii-text` and its character grid must match the checked-in one
//! cell for cell (the PNG stays the visual check). A miss writes
//! `<name>.text.diff` beside the render, each differing row shown as the
//! golden's line over the render's with a caret under each cell that moved.
//!
//! Each case's render, and a diff image when any pixel moved, land in
//! `--out-dir` (default: the system temp dir), with a `report.json` of
//! every case's numbers and an `index.html` contact sheet (#329): golden,
//! render and diff side by side for each case, failures first, with its
//! numbers against its tolerance. `--bless` writes the renders over the
//! goldens after a deliberate change, and the sheet then shows each one
//! against the golden it replaced: look at it before committing. Without
//! a GPU adapter every case is skipped and the suite passes, unless
//! `--require-gpu` or `FLINT_REQUIRE_GPU=1` (#314; CI's software-adapter
//! job sets the variable).
//!
//! Adapters differ (#314): a golden may have a variant per adapter beside
//! it, `<stem>.<key>.png`, where the key is [`adapter_key`]'s (`warp`,
//! `lavapipe`, else the backend: `vulkan`, `dx12`, `metal`, `gl`). A case
//! compares with its adapter's variant when there is one, else with the
//! plain golden. `--bless` rewrites whichever of the two the case compares
//! with; `--bless-variant` writes this adapter's variant. Each report
//! names the adapter, the golden compared and where a variant would go.
//!
//! Drift (#387): a case that passes but isn't byte-identical with its
//! golden (a pixel moved, or a grid cell) prints a `DRIFT` line, carries
//! `"drift": true` in `report.json`, and the run ends naming every such
//! case, or saying there are none. Drift inside the tolerance is how
//! stale goldens pile up: the next real change then has to be told apart
//! from it. `--bless-drifted` re-blesses only those cases (passing ones
//! that moved), for a deliberate catch-up commit; failures still fail.
//!
//! Every case runs in this process, on one GPU device (#329): it parses
//! the case's flags as `flint render` would and calls the same function.
//! `--isolate` spawns a `flint render` per case instead.

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Command;

/// The flags `render-suite` takes for a `.goldens.toml`.
#[derive(clap::Args, Debug, Default)]
pub struct GoldenArgs {
    /// Goldens: write each render over its golden instead of comparing
    /// (this adapter's variant, when the case has one)
    #[arg(long)]
    pub bless: bool,

    /// Goldens: write each render as this adapter's variant of its golden,
    /// `<stem>.<adapter>.png`, for a case another adapter draws differently
    #[arg(long)]
    pub bless_variant: bool,

    /// Goldens: re-bless only the cases that pass but aren't byte-identical
    /// with their golden (#387), for a deliberate catch-up commit. Failing
    /// cases are left alone and still fail the run
    #[arg(long, conflicts_with_all = ["bless", "bless_variant"])]
    pub bless_drifted: bool,

    /// Goldens: run only this case. A name that is exactly a case's picks
    /// that case alone (`ascii-silhouette-cell` skips
    /// `ascii-silhouette-cell-object`); one that names no case picks every
    /// case whose name contains it (repeatable)
    #[arg(long = "case")]
    pub cases: Vec<String>,

    /// Goldens: where renders, diffs and report.json go (default: a
    /// folder in the system temp dir)
    #[arg(long)]
    pub out_dir: Option<String>,

    /// Goldens: fail, rather than skip, when there is no GPU adapter
    #[arg(long)]
    pub require_gpu: bool,

    /// Goldens: run each case as its own `flint render` process. Slower
    /// (a GPU device, schemas and shaders a case) but a crash takes down
    /// only its case; by default every case runs in this process on one
    /// device, and a case that panics is run again in its own (#329)
    #[arg(long)]
    pub isolate: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    suite: Suite,
    #[serde(default)]
    tolerance: Tolerance,
    #[serde(rename = "case", default)]
    cases: Vec<Case>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Suite {
    name: String,
    /// Relative to the manifest's folder.
    #[serde(default)]
    root: Option<String>,
    #[serde(default = "default_width")]
    width: u32,
    #[serde(default = "default_height")]
    height: u32,
    #[serde(default)]
    args: Vec<String>,
}

fn default_width() -> u32 {
    640
}

fn default_height() -> u32 {
    360
}

#[derive(Deserialize, Serialize, Clone, Copy, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Tolerance {
    pub level: u8,
    pub share: f64,
    pub mean: f64,
}

impl Default for Tolerance {
    fn default() -> Self {
        // The ASCII golden's (#265): GPUs shade a little differently and a
        // cell on a ramp step's border can flip glyph; a broken grid, ramp
        // or colour mode moves far more than this.
        Self {
            level: 48,
            share: 0.02,
            mean: 3.0,
        }
    }
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct ToleranceOverride {
    level: Option<u8>,
    share: Option<f64>,
    mean: Option<f64>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Case {
    name: String,
    scene: String,
    #[serde(default)]
    golden: Option<String>,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    width: Option<u32>,
    #[serde(default)]
    height: Option<u32>,
    #[serde(default)]
    tolerance: ToleranceOverride,
    /// The ASCII grid's golden (#317), beside the manifest.
    #[serde(default)]
    text: Option<String>,
    /// How many of the grid's cells may differ from it (default 0).
    #[serde(default)]
    text_cells: usize,
    /// Compare with a second render of the scene made with these flags in
    /// place of `args`, not with a golden file (#417): "this frame must be
    /// the frame without X". Exact unless the case gives a tolerance.
    #[serde(default)]
    matches: Option<Vec<String>>,
    /// Text a user style's error must hold after the render (#417): what
    /// `flint render` prints as a warning, e.g. `"broken.wgsl:9:9"`.
    #[serde(default)]
    error: Option<String>,
}

/// How a render's ASCII grid compares with its golden text (#317).
#[derive(Serialize, Clone, Debug, Default, PartialEq)]
pub struct TextComparison {
    /// Cells whose character differs (a missing or extra cell counts).
    pub cells: usize,
    /// Rows holding any of them.
    pub rows: usize,
    pub size: [usize; 2],
    pub golden_size: [usize; 2],
}

/// Compare a grid with its golden, cell for cell, and write the rows that
/// differ: the golden's, the render's, and a caret under each moved cell.
pub fn compare_text(want: &str, got: &str) -> (TextComparison, String) {
    let w: Vec<Vec<char>> = want.lines().map(|l| l.chars().collect()).collect();
    let g: Vec<Vec<char>> = got.lines().map(|l| l.chars().collect()).collect();
    let size = |rows: &[Vec<char>]| [rows.first().map_or(0, Vec::len), rows.len()];
    let mut cmp = TextComparison {
        size: size(&g),
        golden_size: size(&w),
        ..Default::default()
    };
    let mut diff = String::new();
    for y in 0..w.len().max(g.len()) {
        let (a, b) = (
            w.get(y).map_or(&[][..], |r| &r[..]),
            g.get(y).map_or(&[][..], |r| &r[..]),
        );
        let marks: String = (0..a.len().max(b.len()))
            .map(|x| if a.get(x) == b.get(x) { ' ' } else { '^' })
            .collect();
        let moved = marks.matches('^').count();
        if moved > 0 {
            cmp.cells += moved;
            cmp.rows += 1;
            let line = |r: &[char]| r.iter().collect::<String>();
            diff.push_str(&format!(
                "row {y}: {moved} cell(s)\n  golden |{}|\n  render |{}|\n          {}\n",
                line(a),
                line(b),
                marks.trim_end()
            ));
        }
    }
    (cmp, diff)
}

/// How one render compares with its golden.
#[derive(Serialize, Clone, Copy, Debug, Default, PartialEq)]
pub struct Comparison {
    /// Share of pixels whose largest channel difference passes the level.
    pub far_share: f64,
    /// Mean of every pixel's largest channel difference.
    pub mean: f64,
    /// The largest difference anywhere.
    pub max: u8,
    /// Pixels that differ at all.
    pub changed: usize,
}

impl Comparison {
    pub fn passes(&self, t: &Tolerance) -> bool {
        self.far_share <= t.share && self.mean <= t.mean
    }
}

/// Compare `got` with `want` (same size) and draw a diff: the golden
/// dimmed to a quarter, amber where a pixel moved, red past the level.
pub fn compare(
    want: &image::RgbaImage,
    got: &image::RgbaImage,
    level: u8,
) -> (Comparison, image::RgbaImage) {
    let (w, h) = want.dimensions();
    let mut diff = image::RgbaImage::new(w, h);
    let mut c = Comparison::default();
    let mut total = 0u64;
    let mut far = 0usize;
    for ((a, b), out) in want.pixels().zip(got.pixels()).zip(diff.pixels_mut()) {
        let d = (0..3).map(|i| a[i].abs_diff(b[i])).max().unwrap();
        total += d as u64;
        c.max = c.max.max(d);
        let grey = ((a[0] as u32 * 54 + a[1] as u32 * 183 + a[2] as u32 * 19) >> 10) as u8;
        *out = if d > level {
            far += 1;
            image::Rgba([255, 48, 48, 255])
        } else if d > 0 {
            let k = 96 + (d as u32 * 159 / level.max(1) as u32).min(159);
            image::Rgba([k as u8, (k * 3 / 4) as u8, 32, 255])
        } else {
            image::Rgba([grey, grey, grey, 255])
        };
        c.changed += (d > 0) as usize;
    }
    let pixels = (w * h).max(1) as f64;
    c.far_share = far as f64 / pixels;
    c.mean = total as f64 / pixels;
    (c, diff)
}

#[derive(Serialize)]
pub(crate) struct CaseReport {
    pub name: String,
    pub scene: String,
    pub args: Vec<String>,
    /// pass, fail, error, missing, blessed or skip.
    pub status: &'static str,
    /// Passed, but not byte-identical with its golden (#387): some pixel
    /// or grid cell moved inside the tolerance. A `--bless-drifted` blesses
    /// exactly these, and keeps the flag on the `blessed` report.
    pub drift: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub comparison: Option<Comparison>,
    pub tolerance: Tolerance,
    pub render: String,
    /// The golden this case compared with: the plain one or a variant.
    pub golden: String,
    /// Where this adapter's variant of the golden goes (#314), whether or
    /// not there is one: the path to copy a render to when blessing a
    /// variant by hand from another machine (a CI artifact).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub variant: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub adapter: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diff: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<TextComparison>,
    /// The text golden's limit (`text_cells`).
    #[serde(skip)]
    pub text_cells: usize,
    /// The rows of `<name>.text.diff`, for the contact sheet.
    #[serde(skip)]
    pub text_diff: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// How long the render took.
    pub ms: u128,
    /// The golden as it was before this run (a `--bless` writes over it),
    /// copied into the out dir for the contact sheet.
    #[serde(skip)]
    pub had_golden: bool,
}

impl CaseReport {
    /// Whether any pixel, or any cell of the grid, differs from the golden.
    pub fn moved(&self) -> bool {
        self.comparison.is_some_and(|c| c.changed > 0)
            || self.text.as_ref().is_some_and(|t| t.cells > 0)
    }
}

/// The key an adapter's golden variants carry (#314): `warp` for the
/// software DX12 adapter, `lavapipe` for Mesa's software Vulkan, else the
/// backend's name. Keyed by backend rather than by GPU: one card under
/// DX12 and Vulkan differs more than two cards under one backend.
pub fn adapter_key(info: &wgpu::AdapterInfo) -> String {
    let software = info.device_type == wgpu::DeviceType::Cpu
        || info.name.contains("Basic Render Driver")
        || info.name.to_lowercase().contains("llvmpipe");
    match info.backend {
        wgpu::Backend::Dx12 if software => "warp".into(),
        wgpu::Backend::Vulkan if software => "lavapipe".into(),
        b => format!("{b:?}").to_lowercase(),
    }
}

/// `<dir>/<stem>.<key>.<ext>`: the variant of `golden` (an image, or a
/// text golden's `.txt`) for adapter `key`.
fn variant_path(golden: &Path, key: &str) -> PathBuf {
    let stem = golden
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let ext = golden
        .extension()
        .map(|e| e.to_string_lossy().into_owned())
        .unwrap_or_else(|| "png".into());
    golden.with_file_name(format!("{stem}.{key}.{ext}"))
}

/// The parser `flint render`'s flags go through when a case runs in this
/// process: the same `RenderArgs` the subcommand takes.
#[derive(clap::Parser)]
#[command(name = "flint render")]
struct RenderCli {
    #[command(flatten)]
    args: super::render::RenderArgs,
}

/// How a case's render went.
enum Rendered {
    /// With the user styles' errors it raised: a spawned render's whole
    /// log, or in this process [`user_style_errors`] (the messages
    /// `flint render` prints as warnings).
    Done(String),
    NoGpu,
    Failed(String),
}

/// Every loaded user style's error, a line each, as `flint render` warns
/// of them: what an in-process case's `error` is checked against.
fn user_style_errors() -> String {
    flint_render::stylize::custom::errors()
        .into_iter()
        .map(|(_, e)| e + "\n")
        .collect()
}

/// Sets the working directory back when dropped.
struct CwdGuard(Option<PathBuf>);

impl Drop for CwdGuard {
    fn drop(&mut self) {
        if let Some(dir) = self.0.take() {
            let _ = std::env::set_current_dir(dir);
        }
    }
}

/// Whether a failed render's log is `RenderError::AdapterNotFound`. Only
/// that message: wgpu logs warnings that name adapters on every run here
/// ("Adapter is not Vulkan compliant, hiding adapter"), and matching the
/// word made any failing case read as "no GPU" and skip the rest (#329).
fn looks_like_no_gpu(log: &str) -> bool {
    log.contains("Failed to get adapter")
}

/// One case's `flint render` in a process of its own, run from `root`.
fn render_spawned(flint: &Path, root: &Path, argv: &[std::ffi::OsString]) -> Result<Rendered> {
    let output = Command::new(flint)
        .current_dir(root)
        .args(argv)
        .output()
        .with_context(|| format!("running {}", flint.display()))?;
    let log = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    if output.status.success() {
        return Ok(Rendered::Done(log));
    }
    Ok(if looks_like_no_gpu(&log) {
        Rendered::NoGpu
    } else {
        Rendered::Failed(log)
    })
}

/// One case's `flint render` in this process, on `host`'s GPU (#329). The
/// working directory is already the suite's root. A panic drops the GPU
/// (a device that panicked mid-frame isn't one to trust) and reruns the
/// case in its own process, whose log says what went wrong.
fn render_in_process(
    host: &mut super::render::RenderHost,
    flint: &Path,
    root: &Path,
    argv: &[std::ffi::OsString],
) -> Result<Rendered> {
    use clap::Parser;
    let cli = match RenderCli::try_parse_from(
        std::iter::once(std::ffi::OsString::from("flint render")).chain(argv[1..].iter().cloned()),
    ) {
        Ok(cli) => cli,
        Err(e) => return Ok(Rendered::Failed(e.to_string())),
    };
    let ran = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        super::render::run_in(cli.args, host)
    }));
    match ran {
        Ok(Ok(())) => Ok(Rendered::Done(user_style_errors())),
        Ok(Err(e)) => {
            let log = format!("{e:#}");
            Ok(if looks_like_no_gpu(&log) {
                Rendered::NoGpu
            } else {
                Rendered::Failed(log)
            })
        }
        Err(_) => {
            host.drop_device();
            println!("  (panicked in this process; running the case again in its own)");
            match render_spawned(flint, root, argv)? {
                Rendered::Failed(log) => Ok(Rendered::Failed(format!(
                    "panicked in-process; in its own process:\n{log}"
                ))),
                other => Ok(other),
            }
        }
    }
}

pub fn run(manifest_path: &str, args: &GoldenArgs) -> Result<()> {
    let started_suite = std::time::Instant::now();
    let manifest_path = Path::new(manifest_path);
    let text = std::fs::read_to_string(manifest_path)
        .with_context(|| format!("reading {}", manifest_path.display()))?;
    let manifest: Manifest =
        toml::from_str(&text).with_context(|| format!("parsing {}", manifest_path.display()))?;
    let here = manifest_path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    // Absolute, as in-process cases run with the suite's root as the
    // working directory
    let here = std::path::absolute(here).unwrap_or_else(|_| here.to_path_buf());
    let root = match &manifest.suite.root {
        Some(r) => here.join(r),
        None => PathBuf::from("."),
    };
    let root = std::path::absolute(&root).unwrap_or(root);
    let out_dir = match &args.out_dir {
        Some(d) => PathBuf::from(d),
        None => std::env::temp_dir()
            .join("flint-goldens")
            .join(&manifest.suite.name),
    };
    let golden_copies = out_dir.join("golden");
    std::fs::create_dir_all(&golden_copies)
        .with_context(|| format!("creating {}", golden_copies.display()))?;
    // `flint render` runs from the suite's root; give it absolute outputs
    let out_dir = std::path::absolute(&out_dir).unwrap_or(out_dir);
    let golden_copies = out_dir.join("golden");

    let mut names = std::collections::HashSet::new();
    for case in &manifest.cases {
        if !names.insert(case.name.as_str()) {
            bail!(
                "case '{}' appears twice in {}",
                case.name,
                manifest_path.display()
            );
        }
    }
    // A filter that names a case exactly picks that one; otherwise it
    // picks every case whose name contains it
    let picks = |c: &Case| {
        args.cases.iter().any(|f| {
            let exact = manifest.cases.iter().any(|k| k.name == *f);
            if exact {
                c.name == *f
            } else {
                c.name.contains(f.as_str())
            }
        })
    };
    let chosen: Vec<&Case> = manifest
        .cases
        .iter()
        .filter(|c| args.cases.is_empty() || picks(c))
        .collect();
    if chosen.is_empty() {
        bail!("no cases to run in {}", manifest_path.display());
    }

    let flint = std::env::current_exe().context("finding the flint binary")?;
    println!(
        "render-suite '{}': {} case(s){}, {}, from {}",
        manifest.suite.name,
        chosen.len(),
        if args.bless || args.bless_variant {
            ", blessing"
        } else {
            ""
        },
        if args.isolate {
            "a process each"
        } else {
            "one process"
        },
        root.display()
    );

    // The adapter the cases will draw on (in this process, or children that
    // inherit WGPU_BACKEND / WGPU_ADAPTER_NAME), so a report says what drew
    // it: the tolerance holds between adapters, not pixels (#314)
    let info = pollster::block_on(flint_render::gpu_select::request_adapter(
        &flint_render::gpu_select::instance(),
        None,
    ))
    .map(|a| a.get_info());
    let adapter = info
        .as_ref()
        .map(|i| format!("{} ({:?}, {:?})", i.name, i.device_type, i.backend));
    let key = info.as_ref().map(adapter_key);
    println!(
        "adapter: {}{}",
        adapter.as_deref().unwrap_or("none found"),
        key.as_deref()
            .map(|k| format!(", golden variants '*.{k}.png'"))
            .unwrap_or_default()
    );
    if args.bless_variant && key.is_none() {
        bail!("--bless-variant needs an adapter to name the variant after");
    }
    let blessing = args.bless || args.bless_variant;

    let mut host = super::render::RenderHost::default();
    // In one process the cases share their models' decodes and textures'
    // mip chains too: the files don't change during a run, and in a debug
    // build the two were most of a props case's time
    flint_import::memoize_imports(!args.isolate);
    flint_render::memoize_mip_chains(!args.isolate);
    let _cwd = if args.isolate {
        CwdGuard(None)
    } else {
        let back = std::env::current_dir().ok();
        std::env::set_current_dir(&root).with_context(|| format!("entering {}", root.display()))?;
        CwdGuard(back)
    };

    let mut reports = Vec::new();
    let mut failed = 0usize;
    let mut no_gpu = false;
    for case in chosen {
        let width = case.width.unwrap_or(manifest.suite.width);
        let height = case.height.unwrap_or(manifest.suite.height);
        // A case that matches a second render of its own wants it exact:
        // one device, one process, the same scene (#417)
        let defaults = if case.matches.is_some() {
            Tolerance {
                level: 0,
                share: 0.0,
                mean: 0.0,
            }
        } else {
            manifest.tolerance
        };
        let tolerance = Tolerance {
            level: case.tolerance.level.unwrap_or(defaults.level),
            share: case.tolerance.share.unwrap_or(defaults.share),
            mean: case.tolerance.mean.unwrap_or(defaults.mean),
        };
        let reference = out_dir.join(format!("{}.reference.png", case.name));
        let plain = match &case.matches {
            Some(_) => reference.clone(),
            None => here.join(
                case.golden
                    .clone()
                    .unwrap_or_else(|| format!("{}.png", case.name)),
            ),
        };
        // This adapter's variant, when it has one, is the golden (#314).
        // A reference render is this adapter's already
        let variant = key
            .as_deref()
            .filter(|_| case.matches.is_none())
            .map(|k| variant_path(&plain, k));
        let golden = match &variant {
            Some(v) if args.bless_variant || v.exists() => v.clone(),
            _ => plain.clone(),
        };
        let render = out_dir.join(format!("{}.png", case.name));
        let mut report = CaseReport {
            name: case.name.clone(),
            scene: case.scene.clone(),
            args: case.args.clone(),
            status: "skip",
            drift: false,
            comparison: None,
            tolerance,
            render: render.display().to_string(),
            golden: golden.display().to_string(),
            variant: variant.as_ref().map(|v| v.display().to_string()),
            adapter: adapter.clone(),
            diff: None,
            text: None,
            text_cells: case.text_cells,
            text_diff: None,
            message: None,
            ms: 0,
            had_golden: false,
        };
        if no_gpu {
            report.message = Some("no GPU adapter".into());
            reports.push(report);
            continue;
        }

        let _ = std::fs::remove_file(&render);
        let diff_path = out_dir.join(format!("{}.diff.png", case.name));
        let _ = std::fs::remove_file(&diff_path);
        let text_diff_path = out_dir.join(format!("{}.text.diff", case.name));
        let _ = std::fs::remove_file(&text_diff_path);
        let golden_copy = golden_copies.join(format!("{}.png", case.name));
        let _ = std::fs::remove_file(&golden_copy);
        // A text golden has adapter variants too (#314): a glyph that flips
        // on WARP moves its cell, as it moves pixels
        let text_golden = case.text.as_ref().map(|t| {
            let plain = here.join(t);
            match key.as_deref().map(|k| variant_path(&plain, k)) {
                Some(v) if args.bless_variant || v.exists() => v,
                _ => plain,
            }
        });
        let text_render = out_dir.join(format!("{}.txt", case.name));
        let _ = std::fs::remove_file(&text_render);

        let _ = std::fs::remove_file(&reference);
        let argv_for = |flags: &[String], output: &Path, text: bool| {
            let mut argv: Vec<std::ffi::OsString> =
                vec!["render".into(), case.scene.clone().into()];
            argv.extend(
                [
                    "--width".to_string(),
                    width.to_string(),
                    "--height".to_string(),
                    height.to_string(),
                ]
                .into_iter()
                .chain(manifest.suite.args.iter().cloned())
                .chain(flags.iter().cloned())
                .map(Into::into),
            );
            if text {
                argv.push("--ascii-text".into());
                argv.push(text_render.clone().into_os_string());
            }
            argv.push("--output".into());
            argv.push(output.as_os_str().to_owned());
            argv
        };
        let argv = argv_for(&case.args, &render, text_golden.is_some());

        let started = std::time::Instant::now();
        let mut draw = |argv: &[std::ffi::OsString]| {
            if args.isolate {
                render_spawned(&flint, &root, argv)
            } else {
                render_in_process(&mut host, &flint, &root, argv)
            }
        };
        let mut rendered = draw(&argv)?;
        // The reference the case must match: its scene again, with the
        // case's `matches` flags in place of its own
        if let (Rendered::Done(_), Some(flags)) = (&rendered, &case.matches) {
            if let bad @ (Rendered::NoGpu | Rendered::Failed(_)) =
                draw(&argv_for(flags, &reference, false))?
            {
                rendered = match bad {
                    Rendered::Failed(log) => {
                        Rendered::Failed(format!("the reference render failed:\n{log}"))
                    }
                    other => other,
                };
            }
        }
        let ms = started.elapsed().as_millis();
        report.ms = ms;
        match rendered {
            Rendered::Done(log) => {
                // A user style's error the case expects (#417)
                if let Some(want) = &case.error {
                    if !log.contains(want.as_str()) {
                        failed += 1;
                        println!(
                            "  FAIL  {:<28} no user style error holding '{want}'; errors:\n{log}",
                            case.name
                        );
                        report.status = "fail";
                        report.message = Some(format!(
                            "expected a user style error holding '{want}', got:\n{log}"
                        ));
                        reports.push(report);
                        continue;
                    }
                }
            }
            Rendered::NoGpu => {
                no_gpu = true;
                println!("  SKIP  {:<28} no GPU adapter", case.name);
                report.message = Some("no GPU adapter".into());
                reports.push(report);
                continue;
            }
            Rendered::Failed(log) => {
                failed += 1;
                println!("  FAIL  {:<28} flint render failed:\n{log}", case.name);
                report.status = "error";
                report.message = Some(log);
                reports.push(report);
                continue;
            }
        }
        let got = image::open(&render)
            .with_context(|| format!("reading {}", render.display()))?
            .to_rgba8();

        let got_text = match &text_golden {
            Some(_) => Some(
                std::fs::read_to_string(&text_render)
                    .with_context(|| format!("reading {}", text_render.display()))?,
            ),
            None => None,
        };

        // The golden as it stands, compared before any --bless writes over
        // it: what a bless changes is what its reviewer has to look at
        // (A --bless-variant with no variant yet shows the plain golden)
        let shown = if golden.exists() { &golden } else { &plain };
        let want = image::open(shown).map(|img| img.to_rgba8());
        if let Ok(want) = &want {
            report.had_golden = want.save(&golden_copy).is_ok();
        }
        let mut pass;
        match &want {
            Err(e) => {
                pass = false;
                report.status = "missing";
                report.message = Some(format!("no golden: {e}"));
            }
            Ok(want) if want.dimensions() != got.dimensions() => {
                pass = false;
                report.status = "fail";
                report.message = Some(format!(
                    "size {:?} differs from the golden's {:?}",
                    got.dimensions(),
                    want.dimensions()
                ));
            }
            Ok(want) => {
                let (cmp, diff) = compare(want, &got, tolerance.level);
                pass = cmp.passes(&tolerance);
                if !pass || cmp.changed > 0 {
                    diff.save(&diff_path).ok();
                    report.diff = Some(diff_path.display().to_string());
                }
                report.comparison = Some(cmp);
            }
        }
        // The structural golden (#317): the grid, cell for cell
        if let (Some(path), Some(got_text)) = (&text_golden, &got_text) {
            let want_text = std::fs::read_to_string(path)
                .or_else(|_| std::fs::read_to_string(here.join(case.text.as_deref().unwrap_or(""))))
                .unwrap_or_default();
            let (tc, text_diff) = compare_text(&want_text, got_text);
            let text_pass = tc.size == tc.golden_size && tc.cells <= case.text_cells;
            if !blessing {
                println!(
                    "  {}  {:<28} text {}x{}: {} cell(s) in {} row(s) differ (limit {})",
                    if text_pass { "PASS" } else { "FAIL" },
                    case.name,
                    tc.size[0],
                    tc.size[1],
                    tc.cells,
                    tc.rows,
                    case.text_cells
                );
            }
            if tc.cells > 0 {
                std::fs::write(&text_diff_path, &text_diff).ok();
                report.text_diff = Some(text_diff);
            }
            if !text_pass && report.message.is_none() {
                report.message = Some(format!(
                    "text: {} cell(s) differ from {}",
                    tc.cells,
                    path.display()
                ));
            }
            pass &= text_pass;
            report.text = Some(tc);
        }

        // Inside the tolerance but not the golden's bytes (#387). A case
        // matching its own reference has no golden to go stale, so no drift
        report.drift = pass && report.moved() && case.matches.is_none();
        // A case matching its own reference has no golden to bless: it is
        // compared as ever, and a bless doesn't fail on it
        if (blessing || (args.bless_drifted && report.drift)) && case.matches.is_none() {
            if let Some(parent) = golden.parent() {
                std::fs::create_dir_all(parent).ok();
            }
            got.save(&golden)
                .with_context(|| format!("writing {}", golden.display()))?;
            if let (Some(path), Some(text)) = (&text_golden, &got_text) {
                std::fs::write(path, text)
                    .with_context(|| format!("writing {}", path.display()))?;
                println!("  BLESS {:<28} -> {}", case.name, path.display());
            }
            let moved = match &report.comparison {
                Some(c) if c.changed == 0 => "unchanged".to_string(),
                Some(c) => format!(
                    "{} pixel(s) moved, {:.2}% past {}",
                    c.changed,
                    c.far_share * 100.0,
                    tolerance.level
                ),
                None => "new".to_string(),
            };
            println!(
                "  BLESS {:<28} -> {} ({moved}, {ms} ms)",
                case.name,
                golden.display()
            );
            report.status = "blessed";
            reports.push(report);
            continue;
        }

        match (&report.comparison, report.status) {
            (Some(cmp), _) => println!(
                "  {}  {:<28} {:>6.2}% past {} (limit {:.2}%), mean {:.2} (limit {:.2}), max {}, {} ms{}",
                if pass { "PASS" } else { "FAIL" },
                case.name,
                cmp.far_share * 100.0,
                tolerance.level,
                tolerance.share * 100.0,
                cmp.mean,
                tolerance.mean,
                cmp.max,
                ms,
                if case.matches.is_some() {
                    ", against its reference render"
                } else if golden != plain {
                    ", against its variant"
                } else {
                    ""
                }
            ),
            (None, "missing") => println!(
                "  FAIL  {:<28} no golden at {}; --bless makes it",
                case.name,
                golden.display()
            ),
            (None, _) => println!(
                "  FAIL  {:<28} {}",
                case.name,
                report.message.as_deref().unwrap_or("")
            ),
        }
        if report.drift {
            let text = report
                .text
                .as_ref()
                .filter(|t| t.cells > 0)
                .map(|t| format!(", {} grid cell(s)", t.cells))
                .unwrap_or_default();
            let pixels = report
                .comparison
                .map(|c| format!("{} pixel(s) moved (max {})", c.changed, c.max))
                .unwrap_or_default();
            println!(
                "  DRIFT {:<28} {pixels}{text}: inside the tolerance, not byte-identical",
                case.name
            );
        }
        if report.status == "skip" {
            report.status = if pass { "pass" } else { "fail" };
        }
        failed += (!pass) as usize;
        reports.push(report);
    }
    drop(_cwd);
    flint_import::memoize_imports(false);
    flint_render::memoize_mip_chains(false);

    // The drift tally (#387): zero, or the cases that have it
    let drifted: Vec<&str> = reports
        .iter()
        .filter(|r| r.drift && r.status == "pass")
        .map(|r| r.name.as_str())
        .collect();
    let rebless: Vec<&str> = reports
        .iter()
        .filter(|r| r.drift && r.status == "blessed")
        .map(|r| r.name.as_str())
        .collect();
    let compared = reports.iter().filter(|r| r.comparison.is_some()).count();
    if args.bless_drifted {
        println!(
            "re-blessed {} drifted case(s){}",
            rebless.len(),
            if rebless.is_empty() {
                String::new()
            } else {
                format!(": {}", rebless.join(", "))
            }
        );
    } else if !blessing && compared > 0 {
        if drifted.is_empty() {
            println!("drift: none, every passing case is byte-identical with its golden");
        } else {
            println!(
                "drift: {} passing case(s) not byte-identical with their golden: {}
                   (--bless-drifted re-blesses only those, for a catch-up commit)",
                drifted.len(),
                drifted.join(", ")
            );
        }
    }

    let seconds = started_suite.elapsed().as_secs_f64();
    let report_path = out_dir.join("report.json");
    std::fs::write(&report_path, serde_json::to_string_pretty(&reports)?)
        .with_context(|| format!("writing {}", report_path.display()))?;
    let sheet = super::golden_sheet::Sheet {
        suite: &manifest.suite.name,
        manifest: &manifest_path.display().to_string(),
        bless: blessing || args.bless_drifted,
        isolate: args.isolate,
        seconds,
        no_gpu,
    };
    let sheet_path = out_dir.join("index.html");
    std::fs::write(&sheet_path, super::golden_sheet::html(&sheet, &reports))
        .with_context(|| format!("writing {}", sheet_path.display()))?;
    println!(
        "{} case(s) in {seconds:.1} s ({}); contact sheet: {}",
        reports.len(),
        if args.isolate {
            "a process each".to_string()
        } else {
            format!("one process, {} GPU device(s)", host.devices_made)
        },
        sheet_path.display()
    );

    if failed > 0 && !blessing {
        bail!("{failed} case(s) failed against their goldens");
    }
    if no_gpu {
        if args.require_gpu || flint_render::gpu_select::gpu_required() {
            bail!(
                "no GPU adapter, and --require-gpu (or FLINT_REQUIRE_GPU); adapters: {:?}",
                flint_render::list_adapters()
            );
        }
        println!("no GPU adapter: skipped (--require-gpu or FLINT_REQUIRE_GPU=1 fails instead)");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(v: u8) -> image::RgbaImage {
        image::RgbaImage::from_pixel(10, 10, image::Rgba([v, v, v, 255]))
    }

    #[test]
    fn identical_images_compare_clean() {
        let (c, _) = compare(&solid(100), &solid(100), 48);
        assert_eq!(c, Comparison::default());
        assert!(c.passes(&Tolerance::default()));
    }

    #[test]
    fn a_few_far_pixels_pass_and_many_fail() {
        let want = solid(0);
        let mut got = solid(0);
        got.put_pixel(0, 0, image::Rgba([255, 0, 0, 255]));
        let (c, diff) = compare(&want, &got, 48);
        assert_eq!((c.max, c.changed), (255, 1));
        assert!((c.far_share - 0.01).abs() < 1e-9);
        assert!(c.passes(&Tolerance::default()));
        assert_eq!(diff.get_pixel(0, 0)[0], 255);

        let (c, _) = compare(&want, &solid(200), 48);
        assert!(!c.passes(&Tolerance::default()));
    }

    #[test]
    fn variants_are_keyed_by_backend_and_software_adapters_by_name() {
        let info = |name: &str, device_type, backend| wgpu::AdapterInfo {
            name: name.into(),
            vendor: 0,
            device: 0,
            device_type,
            driver: String::new(),
            driver_info: String::new(),
            backend,
        };
        use wgpu::{Backend, DeviceType};
        let warp = info(
            "Microsoft Basic Render Driver",
            DeviceType::Cpu,
            Backend::Dx12,
        );
        assert_eq!(adapter_key(&warp), "warp");
        // Headless Windows lists a WARP that claims to be integrated
        let warp2 = info(
            "Microsoft Basic Render Driver",
            DeviceType::IntegratedGpu,
            Backend::Dx12,
        );
        assert_eq!(adapter_key(&warp2), "warp");
        let lava = info(
            "llvmpipe (LLVM 15.0.7, 256 bits)",
            DeviceType::Cpu,
            Backend::Vulkan,
        );
        assert_eq!(adapter_key(&lava), "lavapipe");
        let rtx = info(
            "NVIDIA GeForce RTX 4070",
            DeviceType::DiscreteGpu,
            Backend::Vulkan,
        );
        assert_eq!(adapter_key(&rtx), "vulkan");
        let rtx = info(
            "NVIDIA GeForce RTX 4070",
            DeviceType::DiscreteGpu,
            Backend::Dx12,
        );
        assert_eq!(adapter_key(&rtx), "dx12");
        assert_eq!(
            variant_path(Path::new("g/kuwahara.png"), "warp"),
            Path::new("g/kuwahara.warp.png")
        );
        assert_eq!(
            variant_path(Path::new("g/ascii_pbr_test.txt"), "warp"),
            Path::new("g/ascii_pbr_test.warp.txt")
        );
    }

    fn stylize_manifest() -> (PathBuf, Manifest) {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden/stylize.goldens.toml");
        let manifest = toml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        (path, manifest)
    }

    #[test]
    fn only_a_missing_adapter_reads_as_no_gpu() {
        assert!(looks_like_no_gpu(
            "Failed to create headless render context: Failed to get adapter"
        ));
        assert!(!looks_like_no_gpu(
            "WARN wgpu_hal::vulkan::adapter: Adapter is not Vulkan compliant, hiding adapter\n\
             thread 'main' panicked: wgpu error: Validation Error"
        ));
    }

    /// Every case's flags parse as `flint render`'s (#329: in one process
    /// they go through `RenderCli`, so a bad flag would fail its case, not
    /// the parse of a subcommand).
    #[test]
    fn every_case_parses_as_flint_render() {
        use clap::Parser;
        let (_, manifest) = stylize_manifest();
        for case in &manifest.cases {
            let argv = ["flint render".to_string(), case.scene.clone()]
                .into_iter()
                .chain(manifest.suite.args.iter().cloned())
                .chain(case.args.iter().cloned());
            if let Err(e) = RenderCli::try_parse_from(argv) {
                panic!("case '{}': {e}", case.name);
            }
            // ...and so do the flags of the render it must match (#417)
            if let Some(flags) = &case.matches {
                let argv = ["flint render".to_string(), case.scene.clone()]
                    .into_iter()
                    .chain(manifest.suite.args.iter().cloned())
                    .chain(flags.iter().cloned());
                if let Err(e) = RenderCli::try_parse_from(argv) {
                    panic!("case '{}' matches: {e}", case.name);
                }
            }
        }
    }

    #[test]
    fn text_goldens_count_each_moved_cell() {
        let (same, diff) = compare_text("ab\ncd\n", "ab\ncd\n");
        assert_eq!((same.cells, diff.as_str()), (0, ""));
        let (moved, diff) = compare_text("##..\n....\n", "#=..\n...:\n");
        assert_eq!((moved.cells, moved.rows), (2, 2));
        assert!(diff.contains("golden |##..|") && diff.contains("render |#=..|"));
        assert!(diff.contains("           ^\n"), "{diff}");
        let (short, _) = compare_text("ab\ncd\n", "ab\n");
        assert_eq!(
            (short.cells, short.golden_size, short.size),
            (2, [2, 2], [2, 1])
        );
    }

    #[test]
    fn every_case_has_a_golden_and_every_golden_a_case() {
        let (path, manifest) = stylize_manifest();
        let dir = path.parent().unwrap();
        let mut used = std::collections::HashSet::new();
        for case in &manifest.cases {
            // A case that matches its own reference render has no golden
            // file, and a golden it named would never be read
            if case.matches.is_some() {
                assert!(
                    case.golden.is_none() && case.text.is_none(),
                    "{} matches a reference render: it takes no golden",
                    case.name
                );
                continue;
            }
            let name = case
                .golden
                .clone()
                .unwrap_or_else(|| format!("{}.png", case.name));
            assert!(dir.join(&name).exists(), "{} has no golden", case.name);
            used.insert(name);
            if let Some(text) = &case.text {
                assert!(dir.join(text).exists(), "{} has no text golden", case.name);
                used.insert(text.clone());
            }
        }
        // An adapter's variant (#314) belongs to the golden it varies
        let keys = ["warp", "lavapipe", "vulkan", "dx12", "metal", "gl"];
        for entry in std::fs::read_dir(dir).unwrap() {
            let name = entry.unwrap().file_name().to_string_lossy().into_owned();
            let Some((stem, ext)) = name
                .strip_suffix(".png")
                .map(|s| (s, "png"))
                .or_else(|| name.strip_suffix(".txt").map(|s| (s, "txt")))
            else {
                continue;
            };
            let owner = match stem.rsplit_once('.') {
                Some((base, key)) => {
                    assert!(keys.contains(&key), "{name}: unknown adapter key '{key}'");
                    format!("{base}.{ext}")
                }
                None => name.clone(),
            };
            assert!(
                used.contains(&owner),
                "{name} is no case's golden: remove it, or add its case"
            );
        }
    }

    /// A new style adds a golden case (#271): every registered style is
    /// reached by some case's `--style <name>` (`mask_view` by the style
    /// debug view that turns it on).
    #[test]
    fn every_registered_style_has_a_golden_case() {
        let (_, manifest) = stylize_manifest();
        // Built-ins only: user styles (ADR 0112) that other tests in this
        // process registered aren't the engine's to golden (#417)
        for d in flint_render::stylize::registry()
            .iter()
            .filter(|d| flint_render::stylize::is_builtin(d.name))
        {
            let reached = manifest.cases.iter().any(|c| {
                c.args.windows(2).any(|w| {
                    (w[0] == "--style"
                        && (w[1] == d.name || w[1].starts_with(&format!("{}:", d.name))))
                        || (d.name == "mask_view" && w[0] == "--debug-mode" && w[1] == "style")
                })
            });
            assert!(
                reached,
                "style '{}' has no case in stylize.goldens.toml: add one and --bless it",
                d.name
            );
        }
    }

    /// Each style has a props bench of its own (#386): a case on the mixed
    /// bench, which carries ASCII and pencil props both, moves with either
    /// style, so only the mask views and the one deliberately mixed case
    /// may use it. Anything else picks `stylize_props_<style>` or `_plain`.
    #[test]
    fn only_the_mixed_cases_use_the_mixed_props_bench() {
        let (_, manifest) = stylize_manifest();
        let mixed = ["style-mask", "style-mask-msaa4", "styles-mixed"];
        for case in &manifest.cases {
            if case.scene == "demo/stylize_props.scene.toml" {
                assert!(
                    mixed.contains(&case.name.as_str()),
                    "case '{}' is on the mixed props bench: use the bench of its own \
                     style (demo/stylize_props_ascii, _pencil or _plain), so another \
                     style's change doesn't move it",
                    case.name
                );
            }
        }
        for name in mixed {
            assert!(
                manifest.cases.iter().any(|c| c.name == name),
                "the mixed case '{name}' is gone"
            );
        }
    }

    /// A new look adds a golden case too (#279).
    #[test]
    fn every_shipped_look_has_a_golden_case() {
        let (_, manifest) = stylize_manifest();
        for look in flint_render::look::builtin_names() {
            let reached = manifest
                .cases
                .iter()
                .any(|c| c.args.windows(2).any(|w| w[0] == "--look" && w[1] == look));
            assert!(
                reached,
                "look '{look}' has no case in stylize.goldens.toml: add one and --bless it"
            );
        }
    }
}
