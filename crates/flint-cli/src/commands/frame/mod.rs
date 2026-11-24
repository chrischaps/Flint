//! `flint frame`: read a frame capture from the command line (#1087, ADR
//! 0126 decision 7).
//!
//! A capture is the folder `flint render --frame-capture` writes (and the
//! player will, #1086): `manifest.json`, a [`FrameDescription`], and every
//! pass's output under `targets/`. This reads only that folder: no GPU, no
//! scene, no schemas, so a capture from another machine or an older engine
//! reads the same, and one from a newer engine is refused naming both
//! versions.
//!
//! ```text
//! flint frame show <capture> [--passes] [--draws ...] [--targets] [--hitch]
//! flint frame diff <a> <b> [--out dir]
//! flint frame image <capture> <pass> -o out.png [--target id] [--raw] | --all -o dir
//! flint frame open <capture> [--pass name] [--entity name] [--screenshot out.png]
//! flint frame replay <capture> [--hide name] [--draws N] [--skip pass] [--out dir]
//! ```
//!
//! Every view answers the global `--json` with one document carrying the
//! numbers its text shows. `open` is a window (#1088, `open/`), and answers
//! `--json` only with `--screenshot`. `replay` (#1089) is the one view that
//! needs more than the folder: the scene, its assets and a GPU, to draw the
//! frame again.

mod diff;
pub(crate) mod display;
mod image_out;
pub mod open;
pub(crate) mod read;
pub(crate) mod replay;
mod show;
pub(crate) mod table;

use anyhow::Result;
use clap::{Args, Subcommand, ValueEnum};
use flint_render::frame_capture::{FrameDescription, PassDescription};
use std::path::PathBuf;

#[derive(Args)]
pub struct FrameArgs {
    #[command(subcommand)]
    pub command: FrameCommands,
}

#[derive(Subcommand)]
pub enum FrameCommands {
    /// A capture's summary (scene, adapter, size, GPU time by pass, draws,
    /// memory), or one of its views
    Show(ShowArgs),
    /// What changed between two captures: passes, GPU time, each entity's
    /// draws, target memory, and each pass image's pixels
    Diff(DiffArgs),
    /// Copy a pass's image out as a PNG you can look at: HDR tonemapped at
    /// the frame's exposure, depth stretched to grey, ids coloured
    Image(ImageArgs),
    /// The frame debugger: a window with the pass tree, each pass's image
    /// (zoom, pixel readout, channels, exposure, A/B wipe), its draws, and
    /// the frame's CPU and GPU times
    Open(open::OpenArgs),
    /// Draw the captured frame again from its replay inputs, whole or with
    /// entities hidden, the scene pass stopped after N draws, or passes
    /// off; written as a capture, and compared with the captured image
    Replay(replay::ReplayArgs),
}

#[derive(Args)]
pub struct ShowArgs {
    /// The capture folder (or its manifest.json, or a --frame-dump file)
    pub capture: PathBuf,
    /// Every pass in the order recorded: ran or why not, GPU ms, draws,
    /// triangles, what it wrote
    #[arg(long)]
    pub passes: bool,
    /// Every draw, named by its entity (implied by --pass, --entity,
    /// --sort and --culled)
    #[arg(long)]
    pub draws: bool,
    /// Only the draws of passes with this name (`Scene`, `Shadow cascade 0`)
    #[arg(long, value_name = "NAME")]
    pub pass: Option<String>,
    /// Only the draws of this entity, by name (`*` matches any run of
    /// letters: `tree_*`)
    #[arg(long, value_name = "NAME")]
    pub entity: Option<String>,
    /// The draws' order: as the frame drew them (by pass), most triangles
    /// first, or most GPU time first (a replay's `--draw-times`)
    #[arg(long, value_enum)]
    pub sort: Option<DrawSort>,
    /// With the draws, the ones a cull set aside, and why
    #[arg(long)]
    pub culled: bool,
    /// Every texture the frame drew into or read: format, size, memory,
    /// the passes that wrote and read it, its images
    #[arg(long)]
    pub targets: bool,
    /// The frame history around the captured frame and the hitch that
    /// asked for it, when the capture holds one (the player's)
    #[arg(long)]
    pub hitch: bool,
    /// How many of the most expensive passes the summary lists
    #[arg(long, default_value_t = 10, value_name = "N")]
    pub top: usize,
}

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum DrawSort {
    /// In the order drawn, pass by pass
    Pass,
    /// Most triangles first
    Tris,
    /// Most GPU time first, untimed draws last (#1203: `flint frame replay
    /// --draw-times` times them)
    Gpu,
}

#[derive(Args)]
pub struct DiffArgs {
    /// The capture before
    pub a: PathBuf,
    /// The capture after
    pub b: PathBuf,
    /// Write a diff PNG for each pass image that differs into this folder
    #[arg(long, value_name = "DIR")]
    pub out: Option<PathBuf>,
    /// For float targets, how far two values may lie apart and still be
    /// the same (0: exact)
    #[arg(long, default_value_t = 0.0, value_name = "X")]
    pub tolerance: f32,
}

#[derive(Args)]
pub struct ImageArgs {
    /// The capture folder
    pub capture: PathBuf,
    /// The pass whose image to copy (`Scene`, `Bloom`, `Shadow cascade 1`),
    /// or an image's file name (`03-hdr_scene.exr`). A pass that writes
    /// more than one image gives its first unless --target says
    pub pass: Option<String>,
    /// Where to write: a .png file, or a folder with --all
    #[arg(short, long, value_name = "PATH")]
    pub output: PathBuf,
    /// Which of the pass's images: its target id (`depth`, `hdr_scene`)
    #[arg(long, value_name = "ID")]
    pub target: Option<String>,
    /// Which step of a pass that writes one target more than once
    /// (bloom's `threshold`, `down 2`, `up 1`)
    #[arg(long, value_name = "STEP")]
    pub step: Option<String>,
    /// Copy the stored file as it is (an EXR stays an EXR)
    #[arg(long)]
    pub raw: bool,
    /// Every image of the capture, viewable, into the folder -o names
    #[arg(long, conflicts_with_all = ["pass", "target", "step"])]
    pub all: bool,
}

pub fn run(args: FrameArgs) -> Result<()> {
    match args.command {
        FrameCommands::Show(args) => show::run(args),
        FrameCommands::Diff(args) => diff::run(args),
        FrameCommands::Image(args) => image_out::run(args),
        FrameCommands::Open(args) => open::run(args),
        FrameCommands::Replay(args) => replay::run(args),
    }
}

/// A pass as the views name it across captures: its place in
/// [`FrameDescription::all_passes`] (the `NN` of its image files), its
/// name, and which of the passes of that name it is (bloom runs several),
/// with the depth it sits at (a shadow layer under `Shadows`).
pub(crate) struct PassAt<'a> {
    pub index: usize,
    pub pass: &'a PassDescription,
    pub occurrence: usize,
    pub depth: usize,
}

impl PassAt<'_> {
    /// `Bloom`, or `Bloom (2)` for the second pass of that name: the key
    /// a diff pairs passes by.
    pub fn key(&self) -> String {
        if self.occurrence == 0 {
            self.pass.name.clone()
        } else {
            format!("{} ({})", self.pass.name, self.occurrence + 1)
        }
    }
}

/// Every pass of `d` in recorded order, with where it is.
pub(crate) fn passes(d: &FrameDescription) -> Vec<PassAt<'_>> {
    fn walk<'a>(
        passes: &'a [PassDescription],
        depth: usize,
        out: &mut Vec<(&'a PassDescription, usize)>,
    ) {
        for pass in passes {
            out.push((pass, depth));
            walk(&pass.passes, depth + 1, out);
        }
    }
    let mut flat = Vec::new();
    for panel in &d.panels {
        walk(&panel.passes, 0, &mut flat);
    }
    walk(&d.passes, 0, &mut flat);
    let mut seen: std::collections::HashMap<&str, usize> = Default::default();
    flat.into_iter()
        .enumerate()
        .map(|(index, (pass, depth))| {
            let n = seen.entry(pass.name.as_str()).or_default();
            let occurrence = *n;
            *n += 1;
            PassAt {
                index,
                pass,
                occurrence,
                depth,
            }
        })
        .collect()
}

/// Whether `name` matches `pattern`: equal, or with `*` standing for any
/// run of characters.
pub(crate) fn name_matches(pattern: &str, name: &str) -> bool {
    if !pattern.contains('*') {
        return pattern == name;
    }
    let parts: Vec<&str> = pattern.split('*').collect();
    let mut rest = name;
    for (i, part) in parts.iter().enumerate() {
        if i == 0 {
            match rest.strip_prefix(part) {
                Some(r) => rest = r,
                None => return false,
            }
        } else if i == parts.len() - 1 {
            return rest.ends_with(part);
        } else {
            match rest.find(part) {
                Some(at) => rest = &rest[at + part.len()..],
                None => return false,
            }
        }
    }
    true
}

/// A byte count as a person reads it.
pub(crate) fn bytes(n: u64) -> String {
    const KB: f64 = 1024.0;
    let f = n as f64;
    if f >= KB * KB * KB {
        format!("{:.2} GB", f / (KB * KB * KB))
    } else if f >= KB * KB {
        format!("{:.1} MB", f / (KB * KB))
    } else if f >= KB {
        format!("{:.1} KB", f / KB)
    } else {
        format!("{n} B")
    }
}

/// A signed byte delta.
pub(crate) fn bytes_delta(d: i64) -> String {
    let sign = if d < 0 { "-" } else { "+" };
    format!("{sign}{}", bytes(d.unsigned_abs()))
}

/// Now, as RFC 3339 UTC to the second (`2026-11-16T22:30:00Z`).
/// The engine's commit as the views say it (#1290): ` at <10 chars>`, with
/// ` (in engine/)` when the commit is a game's carrying the engine as a
/// subtree and ` (uncommitted changes)` when the build's tree had them, or
/// ` at an unknown commit` when the writer didn't know which.
pub(crate) fn commit_words(
    commit: Option<&str>,
    subdir: Option<&str>,
    dirty: Option<bool>,
) -> String {
    let Some(c) = commit.filter(|c| !c.is_empty()) else {
        return " at an unknown commit".into();
    };
    let short: String = c.chars().take(10).collect();
    let mut s = format!(" at {short}");
    if let Some(dir) = subdir.filter(|d| !d.is_empty()) {
        s.push_str(&format!(" (in {dir})"));
    }
    if dirty == Some(true) {
        s.push_str(" (uncommitted changes)");
    }
    s
}

pub(crate) fn utc_now() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    rfc3339(secs)
}

fn rfc3339(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    // Howard Hinnant's civil_from_days
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc3339_counts_days_from_the_epoch() {
        assert_eq!(rfc3339(0), "1970-01-01T00:00:00Z");
        assert_eq!(rfc3339(951_782_400), "2000-02-29T00:00:00Z");
        assert_eq!(rfc3339(1_794_868_200), "2026-11-16T22:30:00Z");
    }

    #[test]
    fn a_star_matches_any_run() {
        assert!(name_matches("crate", "crate"));
        assert!(!name_matches("crate", "crates"));
        assert!(name_matches("tree_*", "tree_oak"));
        assert!(name_matches("*_oak", "tree_oak"));
        assert!(name_matches("t*e*k", "tree_oak"));
        assert!(!name_matches("tree_*", "bush"));
    }

    #[test]
    fn repeated_passes_are_told_apart() {
        let mut d = FrameDescription::default();
        let pass = |name: &str| PassDescription {
            name: name.into(),
            ..Default::default()
        };
        d.passes = vec![pass("Bloom"), pass("Scene"), pass("Bloom")];
        d.passes[1].passes.push(pass("Shadow cascade 0"));
        let keys: Vec<(usize, String, usize)> = passes(&d)
            .iter()
            .map(|p| (p.index, p.key(), p.depth))
            .collect();
        assert_eq!(
            keys,
            [
                (0, "Bloom".to_string(), 0),
                (1, "Scene".to_string(), 0),
                (2, "Shadow cascade 0".to_string(), 1),
                (3, "Bloom (2)".to_string(), 0),
            ]
        );
    }
}
