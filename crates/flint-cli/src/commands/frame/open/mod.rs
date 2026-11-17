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
//! scene pass's draw limit, hidden entities, passes off) draws the frame
//! again and shows it in the capture's place.
//!
//! - `model.rs`: the passes, draws and images as rows; stepping, finding
//!   an entity, the table's filter and sort. No egui.
//! - `timing.rs`: the bottom panel's bars and history, as data.
//! - `loader.rs`: images read on worker threads, kept a while.
//! - `viewer.rs` + `viewer.wgsl`: the image pane, drawn on the GPU through
//!   an egui-wgpu paint callback.
//! - `replay.rs`: the replay's worker thread and what it has drawn.
//! - `ui.rs`: the window laid out.
//! - `host.rs`: a winit window, or one frame into a PNG (`--screenshot`).

mod host;
mod loader;
mod model;
mod replay;
mod timing;
mod ui;
mod viewer;

use super::read::Capture;
use crate::output;
use anyhow::{bail, Result};
use clap::{Args, ValueEnum};
use std::path::PathBuf;

#[derive(Args)]
pub struct OpenArgs {
    /// The capture folder (or its manifest.json, or a --frame-dump file)
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
    /// Replay it with the scene pass stopped after N draws (implies --replay)
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

pub fn run(args: OpenArgs) -> Result<()> {
    let capture = Capture::open(&args.capture)?;
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
            scene_draws: args.draws,
            skip: args.skip.clone(),
            outline: args.outline.clone(),
        }),
        scene: args.scene.clone(),
        schemas: args.schemas.clone(),
    };
    if let Some(name) = &args.pass {
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
    match &args.screenshot {
        Some(out) => {
            let size = [args.width.max(320), args.height.max(240)];
            host::screenshot(capture, shown, start, size, args.scale, args.hover, out)?;
            let written = out.display().to_string().replace('\\', "/");
            output::say!("wrote {written}");
            output::when_json(|| serde_json::json!({ "written": [written] }));
            Ok(())
        }
        None => host::run_window(capture, shown, start, [args.width, args.height]),
    }
}
