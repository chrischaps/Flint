//! `flint profile`: read a scope trace from the command line (#1098, epic
//! #1094).
//!
//! A trace is the Chrome trace-event JSON `flint-player --trace`,
//! `--trace-on-hitch` and Shift+F3 write (ADR 0128, #1097). This reads that
//! file and nothing else: no GPU, scene or schemas, so a trace from another
//! machine reads the same. It is a `flint` subcommand, as ADR 0126 decision
//! 7 made `flint frame` one: the questions an agent asks of a trace are
//! textual, and `flint --help` is where it looks.
//!
//! ```text
//! flint profile top <trace> [--thread T] [--frames a..b] [--sort total|self|worst|calls]
//! flint profile frame <trace> <n|worst|hitch> [--min-ms X]
//! flint profile explain <trace> [n|worst|hitch]
//! flint profile diff <a> <b>
//! ```
//!
//! Every view answers the global `--json` with one document carrying the
//! numbers its text shows. Frames are named by the trace's own frame index
//! (each scope's `frame`, what Perfetto shows in its args).

mod diff;
mod explain;
mod frame_tree;
pub(crate) mod read;
pub(crate) mod stats;
mod top;

use crate::commands::frame::trace::{meant_hitch, HitchMark};
use anyhow::{bail, Result};
use clap::{Args, Subcommand, ValueEnum};
use read::Trace;
use std::path::PathBuf;

#[derive(Args)]
pub struct ProfileArgs {
    #[command(subcommand)]
    pub command: ProfileCommands,
}

#[derive(Subcommand)]
pub enum ProfileCommands {
    /// Scopes by total time, self time, mean per frame and worst single
    /// call, with call counts
    Top(TopArgs),
    /// One frame's scope tree, indented, with ms and share of the frame
    Frame(FrameArgs),
    /// Why a frame was slow: each scope against its median over the
    /// trace's normal frames, sorted by excess, and what ran only there
    Explain(ExplainArgs),
    /// Two traces' per-scope mean and p90 per frame, and the deltas, with
    /// a note where the frames' own spread is larger than a delta
    Diff(DiffArgs),
}

#[derive(Args)]
pub struct TopArgs {
    /// The trace (`flint-player --trace out.json`)
    pub trace: PathBuf,
    /// Only the scopes of this thread (`main`)
    #[arg(long, value_name = "NAME")]
    pub thread: Option<String>,
    /// Only these frames, by the trace's frame index: `400..450`, `400..`,
    /// `..450` or one frame
    #[arg(long, value_name = "A..B")]
    pub frames: Option<String>,
    /// The order
    #[arg(long, value_enum, default_value_t = TopSort::Total)]
    pub sort: TopSort,
    /// How many scopes to list
    #[arg(long, default_value_t = 20, value_name = "N")]
    pub limit: usize,
    /// Count a scope with text detail by its detail too (`script::call
    /// (scripts/stall.rhai)`, `import_model (models/whale.glb)`)
    #[arg(long)]
    pub by_detail: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum TopSort {
    /// Time inside the scope, children included
    Total,
    /// Time in the scope less its children's
    #[value(name = "self")]
    SelfTime,
    /// The longest single call
    Worst,
    /// How many times it ran
    Calls,
}

#[derive(Args)]
pub struct FrameArgs {
    /// The trace
    pub trace: PathBuf,
    /// The frame: its index, `worst` (the longest) or `hitch` (the one a
    /// `--trace-on-hitch` file was saved for, else the worst hitch the
    /// detector found; the profiler tab's default too)
    pub frame: String,
    /// Leave out scopes shorter than this (each parent says how many it
    /// left out and their time)
    #[arg(long, default_value_t = 0.05, value_name = "MS")]
    pub min_ms: f64,
    /// Only this thread's tree
    #[arg(long, value_name = "NAME")]
    pub thread: Option<String>,
}

#[derive(Args)]
pub struct ExplainArgs {
    /// The trace
    pub trace: PathBuf,
    /// The frame to explain: its index, `worst` (the default) or `hitch`
    #[arg(default_value = "worst")]
    pub frame: String,
    /// How many scopes the excess table lists
    #[arg(long, default_value_t = 12, value_name = "N")]
    pub limit: usize,
}

#[derive(Args)]
pub struct DiffArgs {
    /// The trace before
    pub a: PathBuf,
    /// The trace after
    pub b: PathBuf,
    /// Only this thread's scopes, in both
    #[arg(long, value_name = "NAME")]
    pub thread: Option<String>,
    /// How many scopes to list
    #[arg(long, default_value_t = 20, value_name = "N")]
    pub limit: usize,
    /// List scopes whose delta is inside the frames' spread too (they are
    /// marked `~` either way)
    #[arg(long)]
    pub all: bool,
}

pub fn run(args: ProfileArgs) -> Result<()> {
    match args.command {
        ProfileCommands::Top(args) => top::run(args),
        ProfileCommands::Frame(args) => frame_tree::run(args),
        ProfileCommands::Explain(args) => explain::run(args),
        ProfileCommands::Diff(args) => diff::run(args),
    }
}

/// Which frame a view asked for, as a position in `trace.frames`.
pub(crate) fn pick_frame(trace: &Trace, which: &str) -> Result<usize> {
    let worst = || {
        (0..trace.frames.len())
            .max_by(|&a, &b| trace.frames[a].dur_ns.cmp(&trace.frames[b].dur_ns))
            .expect("a trace with scopes has a frame")
    };
    let held = || {
        let (a, b) = (
            trace.frames.first().map_or(0, |f| f.frame),
            trace.frames.last().map_or(0, |f| f.frame),
        );
        format!("frames {a}..{b}")
    };
    match which {
        "worst" => Ok(worst()),
        "hitch" => {
            // The profiler tab's rule (#1289): the hitch a `--trace-on-hitch`
            // file was saved for, else the worst
            let frames = trace.hitch_frames();
            let marks: Vec<HitchMark> = trace
                .hitches
                .iter()
                .zip(&frames)
                .map(|(h, f)| HitchMark {
                    game_time: h.game_time,
                    // A mark with no judgement: its frame's length
                    judged_ms: h
                        .judged_ms
                        .unwrap_or_else(|| f.map_or(0.0, |f| trace.frames[f].ms())),
                })
                .collect();
            let reason = trace.header.get("reason").and_then(|v| v.as_str());
            let meant = meant_hitch(reason, &marks).and_then(|h| frames[h]);
            // A hitch before the scopes begin has no frame here (#1288):
            // the worst one after them, as the tab chooses
            let worst_covered = || {
                (0..marks.len())
                    .filter(|&h| frames[h].is_some())
                    .max_by(|&a, &b| marks[a].judged_ms.total_cmp(&marks[b].judged_ms))
                    .and_then(|h| frames[h])
            };
            if let Some(pos) = meant.or_else(worst_covered) {
                return Ok(pos);
            }
            let w = worst();
            bail!(
                "{} holds no hitch the detector found; its worst frame is {} ({:.1} ms): \
                 ask for `worst`",
                trace.path,
                trace.frames[w].frame,
                trace.frames[w].ms()
            )
        }
        n => {
            let Ok(n) = n.parse::<u32>() else {
                bail!("`{n}` is not a frame: give a frame index, `worst` or `hitch`");
            };
            if let Some(c) = trace.coverage.as_ref().filter(|c| c.before(n)) {
                bail!(
                    "frame {n} is in {} but before its scopes begin: {}",
                    trace.path,
                    c.line(&trace.frames)
                );
            }
            trace.position(n).ok_or_else(|| {
                anyhow::anyhow!("frame {n} is not in {}: it holds {}", trace.path, held())
            })
        }
    }
}

/// `--frames`' range of frame indices, both ends included.
pub(crate) fn parse_range(text: &str) -> Result<(u32, u32)> {
    let bad = || {
        anyhow::anyhow!("`{text}` is not a range of frames: `400..450`, `400..`, `..450` or `412`")
    };
    let num = |s: &str| s.trim().parse::<u32>().map_err(|_| bad());
    match text.split_once("..") {
        Some((a, b)) => {
            let a = if a.trim().is_empty() { 0 } else { num(a)? };
            let b = b.trim_start_matches('=');
            let b = if b.trim().is_empty() {
                u32::MAX
            } else {
                num(b)?
            };
            if a > b {
                return Err(bad());
            }
            Ok((a, b))
        }
        None => {
            let n = num(text)?;
            Ok((n, n))
        }
    }
}

/// The view's first line: the file, the scene, why it was written.
pub(crate) fn about(trace: &Trace) -> String {
    let mut line = trace.path.clone();
    if let Some(scene) = trace.header_str("scene") {
        line.push_str(&format!(": {scene}"));
    }
    let mut bits = Vec::new();
    if let Some(build) = trace.header_str("build") {
        bits.push(build.to_string());
    }
    if let Some(adapter) = trace.header_str("adapter") {
        bits.push(adapter.to_string());
    }
    if !bits.is_empty() {
        line.push_str(&format!(" ({})", bits.join(", ")));
    }
    if let Some(reason) = trace.header_str("reason") {
        line.push_str(&format!("\n  written for {reason}"));
    }
    if let Some(c) = &trace.coverage {
        line.push_str(&format!("\n  {}", c.line(&trace.frames)));
    }
    line
}

/// What the scopes cover, for the JSON (`null` when they cover it all).
pub(crate) fn coverage_json(trace: &Trace) -> serde_json::Value {
    let Some(c) = &trace.coverage else {
        return serde_json::Value::Null;
    };
    let first = |f: &[read::FrameSpan]| f.first().map(|f| f.frame);
    let last = |f: &[read::FrameSpan]| f.last().map(|f| f.frame);
    serde_json::json!({
        "line": c.line(&trace.frames),
        "from_ms": stats::round(c.from_ns as f64 / 1e6),
        "threads": c.threads,
        "covered_s": stats::round(c.covered_s),
        "window_s": c.window_s,
        "frames": { "first": first(&trace.frames), "last": last(&trace.frames), "count": trace.frames.len() },
        "in_file": { "first": first(&c.all), "last": last(&c.all), "count": c.all.len() },
    })
}

/// The header fields every JSON document carries.
pub(crate) fn about_json(trace: &Trace) -> serde_json::Value {
    let pick = |k: &str| {
        trace
            .header
            .get(k)
            .cloned()
            .unwrap_or(serde_json::Value::Null)
    };
    serde_json::json!({
        "file": trace.path,
        "scene": pick("scene"),
        "reason": pick("reason"),
        "build": pick("build"),
        "adapter": pick("adapter"),
        "engine_commit": pick("engine_commit"),
        "project_commit": pick("project_commit"),
        "frames": trace.frames.len(),
        "frames_from_scopes": trace.frames_derived,
        "threads": trace.thread_names(),
        "coverage": coverage_json(trace),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_range_takes_open_ends_and_one_frame() {
        assert_eq!(parse_range("400..450").unwrap(), (400, 450));
        assert_eq!(parse_range("400..").unwrap(), (400, u32::MAX));
        assert_eq!(parse_range("..450").unwrap(), (0, 450));
        assert_eq!(parse_range("..=450").unwrap(), (0, 450));
        assert_eq!(parse_range("412").unwrap(), (412, 412));
        assert!(parse_range("9..3").is_err());
        assert!(parse_range("a..b").is_err());
    }
}
