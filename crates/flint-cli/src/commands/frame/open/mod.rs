//! `flint frame open`: the frame debugger's window (#1088, ADR 0126
//! decision 7).
//!
//! It reads a capture the way `flint frame show` does (`read.rs`: the
//! folder and nothing else, no scene, no schemas) and shows it: the passes
//! as a tree, each pass's image with zoom, a pixel readout, channel and
//! exposure, an A/B wipe; the pass's draws as a table; the CPU and GPU
//! times and the frame history along the bottom. It shows what was
//! captured until R asks for a replay (#1089, `replay.rs`): then the scene
//! loads on a worker thread and each change of the replay's controls (the
//! draw limit of the scene pass or the one selected, hidden entities,
//! passes off) draws the frame again and shows it in the capture's place.
//!
//! It also opens a scope trace (#1099): a player's `--trace` file alone, or
//! the `trace.json` a capture taken while recording holds. A trace is the
//! profiler tab (`profile.rs`): the frame strip, a flame chart of the frames
//! chosen, and each scope's statistics; a capture with one has both tabs.
//!
//! - `model.rs`: the passes, draws and images as rows; stepping, finding
//!   an entity, the table's filter and sort. No egui.
//! - `timing.rs`: the bottom panel's bars and history, as data.
//! - `loader.rs`: images read on worker threads, kept a while.
//! - `viewer.rs` + `viewer.wgsl`: the image pane, drawn on the GPU through
//!   an egui-wgpu paint callback.
//! - `replay.rs`: the replay's worker thread and what it has drawn.
//! - `ui.rs`: the frame tab laid out.
//! - `profile.rs`: the profiler tab, over `frame::trace`'s reader.
//! - `window.rs`: the two tabs.
//! - `host.rs`: a winit window, or one frame into a PNG (`--screenshot`).

mod host;
mod loader;
mod model;
mod profile;
mod replay;
pub(crate) mod timing;
mod ui;
mod viewer;
mod window;

use super::read::Capture;
use crate::commands::trace::Trace;
use crate::output;
use anyhow::{bail, Context, Result};
use clap::{Args, ValueEnum};
use std::path::{Path, PathBuf};

#[derive(Args)]
pub struct OpenArgs {
    /// The capture folder (or its manifest.json, or a --frame-dump file),
    /// or a scope trace (the .json `flint play --trace` writes)
    pub capture: PathBuf,
    /// Open on this pass (`Scene`, `Bloom (2)`); else the finished frame
    #[arg(long, value_name = "NAME")]
    pub pass: Option<String>,
    /// Open on this of the pass's images, by target id (`depth`)
    #[arg(long, value_name = "ID")]
    pub target: Option<String>,
    /// Select this entity by name: its draws marked in every pass
    #[arg(long, value_name = "NAME")]
    pub entity: Option<String>,
    /// Open with an A/B wipe against the previous pass's image or the
    /// finished frame
    #[arg(long, value_enum, value_name = "WITH")]
    pub wipe: Option<WipeWith>,
    /// Draw one frame of the window into this PNG and exit, with no window
    /// (for an agent, a test or a page)
    #[arg(long, value_name = "PNG")]
    pub screenshot: Option<PathBuf>,
    /// The window's size, points (a screenshot's, pixels)
    #[arg(long, default_value_t = 1600, value_name = "W")]
    pub width: u32,
    #[arg(long, default_value_t = 1000, value_name = "H")]
    pub height: u32,
    /// A screenshot's pixels per point (2 for a high-DPI picture)
    #[arg(long, default_value_t = 1.0, value_name = "X")]
    pub scale: f32,
    /// Draw the frame again from its replay inputs as it opens (#1089;
    /// R in the window)
    #[arg(long)]
    pub replay: bool,
    /// Replay it without this entity (repeatable; implies --replay)
    #[arg(long, value_name = "NAME")]
    pub hide: Vec<String>,
    /// Replay it with a pass stopped after N draws: the --pass shown, when
    /// it is a shadow layer or a style mask or albedo, else the scene pass
    /// (implies --replay)
    #[arg(long, value_name = "N")]
    pub draws: Option<usize>,
    /// Replay it without this pass (repeatable; implies --replay)
    #[arg(long, value_name = "PASS")]
    pub skip: Vec<String>,
    /// Replay it with this entity's pixels outlined (implies --replay)
    #[arg(long, value_name = "NAME")]
    pub outline: Option<String>,
    /// The replay's scene, when it isn't where the capture says
    #[arg(long, value_name = "FILE")]
    pub scene: Option<PathBuf>,
    /// Schema directories for the replay's load
    #[arg(long, value_name = "DIR")]
    pub schemas: Vec<String>,
    /// A screenshot's pointer, in points (`900,420`): what the readout
    /// reads
    #[arg(long, value_name = "X,Y", value_parser = parse_point, hide = true)]
    pub hover: Option<[f32; 2]>,
    #[command(flatten)]
    pub trace: TraceOpenArgs,
}

/// The profiler tab's flags (#1099), a heading of their own.
#[derive(Args, Default)]
#[command(next_help_heading = "Profiler (a trace)")]
pub struct TraceOpenArgs {
    /// Open on the profiler tab (a trace alone always does)
    #[arg(long)]
    pub profile: bool,
    /// The trace's frames to choose: a frame number, `A..B`, `hitch` (the
    /// capture's hitch, else the worst; the default) or `worst` (the
    /// longest frame)
    #[arg(long, value_name = "FRAME", value_parser = parse_frame_choice)]
    pub trace_frame: Option<profile::FrameChoice>,
    /// Choose a scope by name (or part of it): every call marked, its time
    /// plotted across the trace
    #[arg(long, value_name = "NAME")]
    pub scope: Option<String>,
    /// Tint each bar by how much longer its path took than in the trace's
    /// median frame (C in the window)
    #[arg(long)]
    pub compare: bool,
}

impl TraceOpenArgs {
    fn any(&self) -> bool {
        self.profile || self.trace_frame.is_some() || self.scope.is_some() || self.compare
    }
}

fn parse_frame_choice(s: &str) -> Result<profile::FrameChoice, String> {
    profile::FrameChoice::parse(s)
}

fn parse_point(s: &str) -> Result<[f32; 2], String> {
    let (x, y) = s
        .split_once(',')
        .ok_or_else(|| format!("{s:?} is not X,Y"))?;
    let n = |v: &str| v.trim().parse::<f32>().map_err(|e| format!("{v:?}: {e}"));
    Ok([n(x)?, n(y)?])
}

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum WipeWith {
    /// The image of the pass before
    Previous,
    /// The finished frame
    Final,
}

impl OpenArgs {
    /// Whether this opens a window (a screenshot doesn't).
    pub fn interactive(&self) -> bool {
        self.screenshot.is_none()
    }
}

/// What `path` is: a trace file, or a capture and the trace it holds.
fn open_path(path: &Path) -> Result<window::Opened> {
    let shown = Capture::shown(path);
    let named_json = path.is_file()
        && path.file_name().is_some_and(|n| n != "manifest.json")
        && path.extension().is_some_and(|e| e == "json");
    if named_json {
        let text = std::fs::read_to_string(path).with_context(|| format!("reading {shown}"))?;
        if Trace::looks_like(&text) {
            let trace = Trace::parse(text.as_bytes(), &shown)?;
            return Ok(window::Opened {
                capture: None,
                trace: Some((trace, shown)),
                trace_problem: None,
            });
        }
    }
    let capture = Capture::open(path)?;
    let (mut trace, mut trace_problem) = (None, None);
    if let Some(file) = capture.description.trace.clone() {
        let at = capture.dir.join(&file);
        let at_shown = Capture::shown(&at);
        match Trace::open(&at) {
            Ok(t) => trace = Some((t, at_shown)),
            Err(e) => {
                tracing::warn!("the capture's trace: {e:#}");
                trace_problem = Some(format!("its trace {file} didn't read: {e:#}"));
            }
        }
    }
    Ok(window::Opened {
        capture: Some((capture, shown)),
        trace,
        trace_problem,
    })
}

pub fn run(args: OpenArgs) -> Result<()> {
    let opened = open_path(&args.capture)?;
    let shown = Capture::shown(&args.capture);
    let start = ui::Start {
        pass: args.pass.clone(),
        target: args.target.clone(),
        entity: args.entity.clone(),
        wipe: args.wipe.map(|w| match w {
            WipeWith::Previous => viewer::Wipe::Previous,
            WipeWith::Final => viewer::Wipe::Final,
        }),
        replay: (args.replay
            || !args.hide.is_empty()
            || args.draws.is_some()
            || !args.skip.is_empty()
            || args.outline.is_some())
        .then(|| crate::commands::render::replay_host::Controls {
            hidden: args.hide.clone(),
            // `--draws` steps the `--pass` shown, when it can be stepped
            pass: args
                .draws
                .and(args.pass.as_deref())
                .filter(|p| flint_render::SteppedPass::from_name(p).is_some())
                .map(String::from),
            draws: args.draws,
            skip: args.skip.clone(),
            outline: args.outline.clone(),
            // The window shows a capture's draw times (#1203) but doesn't
            // take them: `flint frame replay --draw-times`, then open that
            draw_times: false,
            overdraw: false,
        }),
        scene: args.scene.clone(),
        schemas: args.schemas.clone(),
    };
    let frame_flags = args.pass.is_some()
        || args.target.is_some()
        || args.entity.is_some()
        || args.wipe.is_some()
        || start.replay.is_some();
    if opened.capture.is_none() && frame_flags {
        bail!(
            "{shown} is a scope trace, not a frame capture: --pass, --target, --entity, \
             --wipe and the replay flags are a capture's"
        );
    }
    if opened.trace.is_none() && args.trace.any() {
        let why = if opened.trace_problem.is_some() {
            "its trace didn't read"
        } else {
            "it holds no trace (a player capture taken while recording does: \
             `--capture-on-hitch --trace-seconds 5`)"
        };
        bail!("{shown} has no profiler tab: {why}");
    }
    if let (Some(name), Some((capture, _))) = (&args.pass, &opened.capture) {
        let model = model::Model::new(&capture.description);
        if model.find_pass(name).is_none() {
            let names: Vec<String> = model.passes.iter().map(|p| p.key.clone()).collect();
            let hint = flint_core::diagnostic::did_you_mean(name, names.iter().map(String::as_str))
                .map_or(String::new(), |h| format!(": {h}"));
            bail!("{shown} has no pass named {name:?}{hint}");
        }
    }
    if !(0.25..=8.0).contains(&args.scale) {
        bail!(
            "--scale {} is not a size a picture is drawn at (0.25 to 8)",
            args.scale
        );
    }
    let launch = host::Launch {
        opened,
        start,
        profile: profile::ProfileStart {
            frame: args.trace.trace_frame.clone(),
            scope: args.trace.scope.clone(),
            compare: args.trace.compare,
            capture_hitch: None,
            capture_frame: None,
        },
        on_profile: args.trace.profile,
    };
    match &args.screenshot {
        Some(out) => {
            let size = [args.width.max(320), args.height.max(240)];
            host::screenshot(launch, size, args.scale, args.hover, out)?;
            let written = out.display().to_string().replace('\\', "/");
            output::say!("wrote {written}");
            output::when_json(|| serde_json::json!({ "written": [written] }));
            Ok(())
        }
        None => host::run_window(launch, [args.width, args.height]),
    }
}
