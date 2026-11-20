//! `flint capture <shots.toml>` (#722): deterministic footage from a shot
//! list, with no window to watch and no Python.
//!
//! A shot is a scene played for a number of seconds at a frame rate and
//! size, optionally under a look, a seed and a timed input script. Each of
//! a shot's takes (one, `main`, unless it lists several) is one run of
//! `flint play` with the clock stepped exactly 1/fps a frame
//! (`--frame-step`) and every frame written (`--screenshot-every`), so a
//! slow frame changes nothing on screen: the run is the same however long
//! it takes. Takes land in
//!
//! ```text
//! <out>/<shot>/<take>/frame_0001.png ...   frame 1 is t = 0
//! <out>/<shot>/<take>/take.json            how they were made
//! <out>/<shot>/<take>/player.log           what the player said
//! ```
//!
//! `--verify` captures each take a second time into `<take>.again` and
//! compares the two frame by frame (`capture_compare.rs`), naming each frame
//! that differs, in how many pixels and by how many levels. `--encode mp4`
//! makes `<out>/<shot>/<take>.mp4` with ffmpeg when it is on PATH.
//!
//! This was the showcase film's `tools/showcase/capture.py` and
//! `compare.py`; the film's list, in this format, lives with the film in
//! flint-examples since #984: `flint-examples/tools/showcase/film.toml`.

use super::capture_compare::{compare_folders, Comparison};
use crate::output::{self, progress, say};
use anyhow::{bail, Context, Result};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

#[derive(clap::Args)]
pub struct CaptureArgs {
    /// The shot list: `[capture]` (fps, width, height, points_wide, out,
    /// look, seed, args) and a `[[shot]]` per shot (id, scene, seconds,
    /// and optionally title, notes, fps, width, height, look, seed, input,
    /// args and `[[shot.take]]`s)
    #[arg(value_name = "SHOTS.toml")]
    pub list: PathBuf,

    /// Only this shot (by id); repeatable
    #[arg(long = "shot", value_name = "ID")]
    pub shots: Vec<String>,

    /// Only takes of this name; repeatable
    #[arg(long = "take", value_name = "NAME")]
    pub takes: Vec<String>,

    /// Where the takes go (default: the list's `[capture] out`, else
    /// `captures/<list name>/` beside the list)
    #[arg(long, value_name = "DIR")]
    pub out: Option<PathBuf>,

    /// A fraction of each shot's size (0.5 = a half-size preview); the
    /// point canvas stays the list's, so the layout is the same, smaller
    #[arg(long, default_value_t = 1.0, value_name = "FRACTION")]
    pub scale: f64,

    /// Frame width for every shot (overrides the list and --scale)
    #[arg(long)]
    pub width: Option<u32>,

    /// Frame height for every shot (overrides the list and --scale)
    #[arg(long)]
    pub height: Option<u32>,

    /// Capture each take twice and compare the runs frame by frame; exit 1
    /// when a frame differs by more than --tolerance
    #[arg(long)]
    pub verify: bool,

    /// With --verify: the largest difference, in 8-bit levels of any
    /// channel, that still counts as the same frame
    #[arg(long, default_value_t = 0, value_name = "LEVELS", requires = "verify")]
    pub tolerance: u8,

    /// Encode each take once captured (needs ffmpeg on PATH)
    #[arg(long, value_enum, value_name = "FORMAT")]
    pub encode: Option<Encode>,

    /// Print the takes, their frames and command lines, and capture nothing
    #[arg(long)]
    pub dry_run: bool,

    /// Schema directories for the player (the whole list; default: the
    /// scene's project's flint.toml)
    #[arg(long, action = clap::ArgAction::Append)]
    pub schemas: Vec<String>,

    /// Flags for `flint play` after `--`, added to every take after the
    /// list's own
    #[arg(last = true, value_name = "PLAY FLAGS")]
    pub player_args: Vec<String>,
}

#[derive(clap::ValueEnum, Clone, Copy, PartialEq, Eq, Debug)]
pub enum Encode {
    /// H.264 in MP4, BT.709 limited range, no sound
    Mp4,
}

// ── The list ─────────────────────────────────────────────────────────

/// A shot list as written.
#[derive(Deserialize, Debug, Default)]
#[serde(deny_unknown_fields)]
pub struct ShotList {
    #[serde(default)]
    pub capture: Settings,
    #[serde(default)]
    pub shot: Vec<Shot>,
}

/// `[capture]`: what every shot takes unless it says otherwise.
#[derive(Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    pub title: Option<String>,
    #[serde(default = "default_fps")]
    pub fps: u32,
    #[serde(default = "default_width")]
    pub width: u32,
    #[serde(default = "default_height")]
    pub height: u32,
    /// The width of the canvas point-sized things (ASCII cells, egui, the
    /// HUD) are laid out on: pixels per point is width / points_wide, and
    /// input scripts' pointer positions are in these points. Default: the
    /// list's width, one pixel a point at full size.
    pub points_wide: Option<f64>,
    pub out: Option<String>,
    pub look: Option<String>,
    pub seed: Option<u64>,
    #[serde(default)]
    pub args: Vec<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            title: None,
            fps: default_fps(),
            width: default_width(),
            height: default_height(),
            points_wide: None,
            out: None,
            look: None,
            seed: None,
            args: Vec::new(),
        }
    }
}

fn default_fps() -> u32 {
    30
}
fn default_width() -> u32 {
    1280
}
fn default_height() -> u32 {
    720
}

/// `[[shot]]`
#[derive(Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub struct Shot {
    pub id: String,
    /// The scene, from the list's folder
    pub scene: String,
    pub seconds: f64,
    pub title: Option<String>,
    #[serde(default)]
    pub notes: Vec<String>,
    pub fps: Option<u32>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub look: Option<String>,
    pub seed: Option<u64>,
    /// A `--input-script` file, from the list's folder, pointer positions
    /// in points
    pub input: Option<String>,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub take: Vec<Take>,
}

/// `[[shot.take]]`: one run of the shot, with what it changes.
#[derive(Deserialize, Debug, Default)]
#[serde(deny_unknown_fields)]
pub struct Take {
    pub name: String,
    /// Stop sooner than the shot (a cut that uses only its start)
    pub seconds: Option<f64>,
    pub look: Option<String>,
    pub seed: Option<u64>,
    pub input: Option<String>,
    #[serde(default)]
    pub args: Vec<String>,
}

impl ShotList {
    pub fn parse(text: &str) -> Result<Self> {
        let list: ShotList = toml::from_str(text)?;
        list.check()?;
        Ok(list)
    }

    fn check(&self) -> Result<()> {
        if self.shot.is_empty() {
            bail!("no [[shot]]: a shot list needs at least one");
        }
        let c = &self.capture;
        if c.fps == 0 || c.width == 0 || c.height == 0 {
            bail!("[capture] fps, width and height must be above 0");
        }
        if c.points_wide.is_some_and(|p| p <= 0.0) {
            bail!("[capture] points_wide must be above 0");
        }
        let mut ids = BTreeSet::new();
        for shot in &self.shot {
            folder_name(&shot.id).with_context(|| format!("shot id {:?}", shot.id))?;
            if !ids.insert(shot.id.as_str()) {
                bail!("two shots are named {:?}", shot.id);
            }
            if shot.seconds.is_nan() || shot.seconds <= 0.0 {
                bail!("shot {}: seconds must be above 0", shot.id);
            }
            if [shot.fps, shot.width, shot.height].contains(&Some(0)) {
                bail!("shot {}: fps, width and height must be above 0", shot.id);
            }
            let mut names = BTreeSet::new();
            for take in &shot.take {
                folder_name(&take.name)
                    .with_context(|| format!("shot {}: take name {:?}", shot.id, take.name))?;
                if !names.insert(take.name.as_str()) {
                    bail!("shot {}: two takes are named {:?}", shot.id, take.name);
                }
                if let Some(s) = take.seconds {
                    if s.is_nan() || s <= 0.0 || s > shot.seconds + 1e-6 {
                        bail!(
                            "shot {}/{}: seconds {s} must be above 0 and at most the shot's {}",
                            shot.id,
                            take.name,
                            shot.seconds
                        );
                    }
                }
            }
        }
        Ok(())
    }
}

/// A shot id or take name is a folder: letters, digits, `_`, `-`, `.`,
/// and not starting with a dot.
fn folder_name(name: &str) -> Result<()> {
    let ok = !name.is_empty()
        && !name.starts_with('.')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'));
    if !ok {
        bail!("must be letters, digits, `_`, `-` or `.`, not starting with `.`");
    }
    Ok(())
}

// ── The plan ─────────────────────────────────────────────────────────

/// One take, everything decided: what `flint play` is told and where its
/// frames go.
#[derive(Debug, Clone)]
pub struct TakePlan {
    pub shot: String,
    pub take: String,
    pub title: Option<String>,
    pub notes: Vec<String>,
    /// The scene as the list names it, and the path the player opens
    pub scene_named: String,
    pub scene: PathBuf,
    pub input: Option<PathBuf>,
    pub look: Option<String>,
    pub seed: Option<u64>,
    pub args: Vec<String>,
    pub fps: u32,
    pub width: u32,
    pub height: u32,
    pub pixels_per_point: f64,
    pub points_wide: f64,
    pub seconds: f64,
    pub folder: PathBuf,
}

impl TakePlan {
    /// Frames a take writes: one at t = 0, then one a step to `seconds`.
    pub fn frames(&self) -> usize {
        frame_count(self.seconds, self.fps)
    }

    pub fn label(&self) -> String {
        format!("{}/{}", self.shot, self.take)
    }

    fn step(&self) -> f64 {
        1.0 / self.fps as f64
    }

    /// The words after `flint`: `play <scene> ...`. `input_copy` is the
    /// scaled input script the take reads, when it has one.
    pub fn play_words(
        &self,
        folder: &Path,
        input_copy: Option<&Path>,
        schemas: &[String],
        extra: &[String],
    ) -> Vec<String> {
        let step = self.step();
        let mut words: Vec<String> = vec![
            "play".into(),
            self.scene.display().to_string(),
            "--width".into(),
            self.width.to_string(),
            "--height".into(),
            self.height.to_string(),
            "--pixels-per-point".into(),
            format!("{:.6}", self.pixels_per_point),
            "--frame-step".into(),
            format!("{step:.9}"),
            "--screenshot".into(),
            folder.join("frame.png").display().to_string(),
            "--screenshot-every".into(),
            format!("{step:.9}"),
            // Half a step short of `seconds`, so float sums of the step
            // neither drop the last frame nor add one past it
            "--exit-after".into(),
            format!("{:.9}", self.seconds - step * 0.5),
            "--music-volume".into(),
            "0".into(),
            "--sfx-volume".into(),
            "0".into(),
            // Unattended: a take never takes the keyboard from what you're
            // doing (a capture hears only its script anyway)
            "--no-activate".into(),
        ];
        for dir in schemas {
            words.push("--schemas".into());
            words.push(dir.clone());
        }
        if let Some(seed) = self.seed {
            words.push("--seed".into());
            words.push(seed.to_string());
        }
        if let Some(look) = &self.look {
            words.push("--look".into());
            words.push(look.clone());
        }
        if let Some(input) = input_copy {
            words.push("--input-script".into());
            words.push(input.display().to_string());
        }
        words.extend(self.args.iter().cloned());
        words.extend(extra.iter().cloned());
        words
    }
}

pub fn frame_count(seconds: f64, fps: u32) -> usize {
    (seconds * fps as f64).round() as usize + 1
}

/// What the command line changes about the list.
#[derive(Debug, Default, Clone)]
pub struct Overrides {
    pub out: Option<PathBuf>,
    pub scale: f64,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub shots: Vec<String>,
    pub takes: Vec<String>,
}

/// Every take the run makes, in list order.
pub fn plan(
    list: &ShotList,
    list_path: &Path,
    over: &Overrides,
) -> Result<(PathBuf, Vec<TakePlan>)> {
    let base = list_path.parent().unwrap_or(Path::new("."));
    let base = std::path::absolute(base).unwrap_or_else(|_| base.to_path_buf());
    let c = &list.capture;
    if over.scale.is_nan() || over.scale <= 0.0 {
        bail!("--scale must be above 0");
    }
    for id in &over.shots {
        if !list.shot.iter().any(|s| &s.id == id) {
            let have: Vec<&str> = list.shot.iter().map(|s| s.id.as_str()).collect();
            bail!(
                "no shot {id:?} in {} (it has {have:?})",
                list_path.display()
            );
        }
    }
    let out = match (&over.out, &c.out) {
        (Some(dir), _) => std::path::absolute(dir).unwrap_or_else(|_| dir.clone()),
        (None, Some(dir)) => {
            let dir = expand_home(dir);
            if dir.is_absolute() {
                dir
            } else {
                base.join(dir)
            }
        }
        (None, None) => base.join("captures").join(list_name(list_path)),
    };
    let list_points = c.points_wide.unwrap_or(c.width as f64);

    let mut plans = Vec::new();
    for shot in &list.shot {
        if !over.shots.is_empty() && !over.shots.contains(&shot.id) {
            continue;
        }
        let main = [Take {
            name: "main".into(),
            ..Take::default()
        }];
        let takes: &[Take] = if shot.take.is_empty() {
            &main
        } else {
            &shot.take
        };
        // The shot's own size, at --scale, unless the command line names one
        let full_w = shot.width.unwrap_or(c.width);
        let full_h = shot.height.unwrap_or(c.height);
        let width = over
            .width
            .unwrap_or_else(|| ((full_w as f64 * over.scale).round() as u32).max(1));
        let height = over
            .height
            .unwrap_or_else(|| ((full_h as f64 * over.scale).round() as u32).max(1));
        // A shot wider than the list keeps the list's points per pixel at
        // full size; --scale shrinks pixels, never the canvas
        let points_wide = list_points * full_w as f64 / c.width as f64;
        let fps = shot.fps.unwrap_or(c.fps);
        let scene = base.join(native(&shot.scene));
        for take in takes {
            if !over.takes.is_empty() && !over.takes.contains(&take.name) {
                continue;
            }
            let input = take
                .input
                .as_ref()
                .or(shot.input.as_ref())
                .map(|p| base.join(native(p)));
            let mut args = c.args.clone();
            args.extend(shot.args.iter().cloned());
            args.extend(take.args.iter().cloned());
            plans.push(TakePlan {
                shot: shot.id.clone(),
                take: take.name.clone(),
                title: shot.title.clone(),
                notes: shot.notes.clone(),
                scene_named: shot.scene.clone(),
                scene: scene.clone(),
                input,
                look: take
                    .look
                    .clone()
                    .or_else(|| shot.look.clone())
                    .or_else(|| c.look.clone()),
                seed: take.seed.or(shot.seed).or(c.seed),
                args,
                fps,
                width,
                height,
                pixels_per_point: width as f64 / points_wide,
                points_wide,
                seconds: take.seconds.unwrap_or(shot.seconds),
                folder: out.join(&shot.id).join(&take.name),
            });
        }
    }
    if plans.is_empty() {
        bail!("no take matches --shot / --take");
    }
    Ok((out, plans))
}

/// A path for a person to read: from the working directory when it is
/// under it.
fn shown(path: &Path) -> String {
    std::env::current_dir()
        .ok()
        .and_then(|here| path.strip_prefix(here).ok().map(Path::to_path_buf))
        .unwrap_or_else(|| path.to_path_buf())
        .display()
        .to_string()
}

/// A path as a list writes it (`/` between folders) in this system's
/// spelling.
fn native(path: &str) -> PathBuf {
    Path::new(path).components().collect()
}

/// `trailer.shots.toml` -> `trailer`, `film.toml` -> `film`.
fn list_name(path: &Path) -> String {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "shots".into());
    let name = name.strip_suffix(".toml").unwrap_or(&name);
    let name = name.strip_suffix(".shots").unwrap_or(name);
    name.to_string()
}

fn expand_home(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/").or(path.strip_prefix("~\\")) {
        let home = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME"));
        if let Some(home) = home {
            return PathBuf::from(home).join(rest);
        }
    }
    PathBuf::from(path)
}

/// An input script with its pointer positions scaled from points to the
/// take's pixels: `mouse x y`, `click x y`, `touch id x y` and `drag id x
/// y` move; `look` is raw motion and keeps its units.
pub fn scale_input(text: &str, pixels_per_point: f64) -> String {
    let mut out = String::with_capacity(text.len());
    for line in text.lines() {
        let (body, comment) = match line.find('#') {
            Some(i) => line.split_at(i),
            None => (line, ""),
        };
        let mut words: Vec<String> = body.split_whitespace().map(str::to_string).collect();
        let first = match words.get(1).map(String::as_str) {
            Some("mouse" | "click") => Some(2),
            Some("touch" | "drag") => Some(3),
            _ => None,
        };
        match first {
            Some(first) if words.len() > first + 1 && pixels_per_point != 1.0 => {
                for word in &mut words[first..first + 2] {
                    if let Ok(v) = word.parse::<f64>() {
                        *word = format!("{:.1}", v * pixels_per_point);
                    }
                }
                out.push_str(&words.join("  "));
                if !comment.is_empty() {
                    out.push_str("  ");
                }
            }
            _ => out.push_str(body),
        }
        out.push_str(comment);
        out.push('\n');
    }
    out
}

// ── Running it ───────────────────────────────────────────────────────

pub fn run(args: CaptureArgs) -> Result<()> {
    let text = std::fs::read_to_string(&args.list)
        .with_context(|| format!("reading {}", args.list.display()))?;
    let list = ShotList::parse(&text).with_context(|| format!("{}", args.list.display()))?;
    let over = Overrides {
        out: args.out.clone(),
        scale: args.scale,
        width: args.width,
        height: args.height,
        shots: args.shots.clone(),
        takes: args.takes.clone(),
    };
    let (out, plans) = plan(&list, &args.list, &over)?;
    let title = list
        .capture
        .title
        .clone()
        .unwrap_or_else(|| list_name(&args.list));

    // Everything a take names is there before the first one runs
    let mut missing = Vec::new();
    for p in &plans {
        if !p.scene.is_file() {
            missing.push(format!("{}: no scene {}", p.label(), p.scene.display()));
        }
        if let Some(input) = &p.input {
            if !input.is_file() {
                missing.push(format!(
                    "{}: no input script {}",
                    p.label(),
                    input.display()
                ));
            }
        }
    }
    missing.dedup();
    if !missing.is_empty() {
        bail!("{}", missing.join("\n"));
    }

    let runs = if args.verify { 2 } else { 1 };
    let frames: usize = plans.iter().map(TakePlan::frames).sum::<usize>() * runs;
    // A 4K PNG of the showcase film averaged 7 MB (#436); say 8 MB
    let bytes: f64 = plans
        .iter()
        .map(|p| p.frames() as f64 * 8e6 * (p.width as f64 * p.height as f64) / (3840.0 * 2160.0))
        .sum::<f64>()
        * runs as f64;
    let header = format!(
        "{title}: {} takes, {frames} frames{}, into {} (~{:.1} GB)",
        plans.len(),
        if args.verify { " (each twice)" } else { "" },
        shown(&out),
        bytes / 1e9
    );

    let flint = std::env::current_exe().context("finding the flint executable")?;
    if args.dry_run {
        return dry_run(&header, &out, &plans, &flint, &args);
    }
    if args.encode.is_some() && !ffmpeg_present() {
        bail!("--encode needs ffmpeg on PATH (https://ffmpeg.org/download.html)");
    }

    say!("{header}");
    let mut records = Vec::new();
    let mut failed = false;
    for p in &plans {
        let mut record = capture_take(p, &p.folder, &flint, &args)?;
        failed |= !record.ok;
        if args.verify {
            let again = p.folder.with_file_name(format!("{}.again", p.take));
            let second = capture_take(p, &again, &flint, &args)?;
            failed |= !second.ok;
            let cmp = compare_folders(&p.folder, &again)?;
            let same = cmp.same_within(args.tolerance);
            failed |= !same;
            for line in cmp.text_lines(args.tolerance) {
                say!("    {line}");
            }
            let report = cmp.json(args.tolerance);
            std::fs::write(
                p.folder.join("verify.json"),
                serde_json::to_string_pretty(&report)?,
            )?;
            // A second run that matched is the first again: give its disk
            // back. One that didn't stays, to look at.
            if same && cmp.identical() && second.ok {
                let _ = std::fs::remove_dir_all(&again);
            }
            record.verify = Some((cmp, report));
        }
        if let (Some(Encode::Mp4), true) = (args.encode, record.ok) {
            let file = p.folder.with_file_name(format!("{}.mp4", p.take));
            match encode_mp4(&p.folder, p.fps, &file) {
                Ok(()) => {
                    say!("    encoded {}", shown(&file));
                    record.encoded = Some(file);
                }
                Err(e) => {
                    say!("    encode FAILED: {e:#}");
                    failed = true;
                }
            }
        }
        records.push(record);
    }

    if args.verify {
        let all: Vec<Value> = records
            .iter()
            .filter_map(|r| r.verify.as_ref().map(|(_, v)| v.clone()))
            .collect();
        std::fs::write(out.join("verify.json"), serde_json::to_string_pretty(&all)?)?;
    }
    let bad = records.iter().filter(|r| !r.ok).count();
    let differ = records
        .iter()
        .filter(|r| {
            r.verify
                .as_ref()
                .is_some_and(|(c, _)| !c.same_within(args.tolerance))
        })
        .count();
    let summary = if !failed {
        format!(
            "captured {} takes{}",
            records.len(),
            if args.verify {
                ", each the same twice"
            } else {
                ""
            }
        )
    } else {
        let mut parts = Vec::new();
        if bad > 0 {
            parts.push(format!("{bad} of {} takes failed", records.len()));
        }
        if differ > 0 {
            parts.push(format!("{differ} differ between runs"));
        }
        if parts.is_empty() {
            parts.push("an encode failed".into());
        }
        parts.join("; ")
    };
    say!("{summary}");
    output::when_json(|| {
        json!({
            "list": args.list.display().to_string(),
            "title": title,
            "out": out.display().to_string(),
            "takes": records.iter().map(|r| r.json()).collect::<Vec<_>>(),
            "failed": failed,
            "summary": summary,
        })
    });
    if failed {
        Err(output::failed())
    } else {
        Ok(())
    }
}

fn dry_run(
    header: &str,
    out: &Path,
    plans: &[TakePlan],
    flint: &Path,
    args: &CaptureArgs,
) -> Result<()> {
    let exe = flint
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "flint".into());
    let mut text = format!("{header}\n");
    let mut takes = Vec::new();
    for p in plans {
        let input = p.input.as_ref().map(|_| p.folder.join("input.txt"));
        let words = p.play_words(
            &p.folder,
            input.as_deref(),
            &args.schemas,
            &args.player_args,
        );
        text.push_str(&format!(
            "  {:<24} {:>5} frames  {}x{} at {} fps, {:.2} s\n    {exe} {}\n",
            p.label(),
            p.frames(),
            p.width,
            p.height,
            p.fps,
            p.seconds,
            words.join(" ")
        ));
        takes.push(json!({
            "shot": p.shot,
            "take": p.take,
            "folder": p.folder.display().to_string(),
            "scene": p.scene_named,
            "width": p.width,
            "height": p.height,
            "fps": p.fps,
            "seconds": p.seconds,
            "frames": p.frames(),
            "pixels_per_point": p.pixels_per_point,
            "command": std::iter::once(exe.clone()).chain(words).collect::<Vec<_>>(),
        }));
    }
    output::emit(&output::Answer::new(
        text,
        json!({ "dry_run": true, "out": out.display().to_string(), "takes": takes }),
    ))
}

/// What one run of a take made.
struct Record {
    plan: TakePlan,
    frames: usize,
    exit_code: Option<i32>,
    wall_seconds: f64,
    ok: bool,
    command: Vec<String>,
    folder: PathBuf,
    verify: Option<(Comparison, Value)>,
    encoded: Option<PathBuf>,
}

impl Record {
    fn take_json(&self) -> Value {
        let p = &self.plan;
        json!({
            "shot": p.shot,
            "take": p.take,
            "title": p.title,
            "notes": p.notes,
            "scene": p.scene_named,
            "look": p.look,
            "seed": p.seed,
            "width": p.width,
            "height": p.height,
            "fps": p.fps,
            "seconds": p.seconds,
            "frames": self.frames,
            "expected_frames": p.frames(),
            "pixels_per_point": round(p.pixels_per_point, 6),
            "points_wide": round(p.points_wide, 3),
            "wall_seconds": round(self.wall_seconds, 1),
            "exit_code": self.exit_code,
            "engine": env!("CARGO_PKG_VERSION"),
            "command": self.command,
        })
    }

    fn json(&self) -> Value {
        let mut v = self.take_json();
        v["folder"] = json!(self.folder.display().to_string());
        v["ok"] = json!(self.ok);
        if let Some((_, report)) = &self.verify {
            v["verify"] = report.clone();
        }
        if let Some(file) = &self.encoded {
            v["encoded"] = json!(file.display().to_string());
        }
        v
    }
}

fn round(v: f64, places: i32) -> f64 {
    let k = 10f64.powi(places);
    (v * k).round() / k
}

/// Empties `folder` for a take, refusing one that holds anything but a
/// past take's files.
fn fresh_folder(folder: &Path) -> Result<()> {
    if folder.exists() {
        let entries: Vec<_> = std::fs::read_dir(folder)?
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        let a_take = entries.is_empty()
            || entries.iter().any(|n| n == "take.json")
            || entries
                .iter()
                .all(|n| n.starts_with("frame_") || n == "player.log");
        if !a_take {
            bail!(
                "{} holds files that aren't a take's; capture into another --out",
                folder.display()
            );
        }
        std::fs::remove_dir_all(folder)
            .with_context(|| format!("clearing {}", folder.display()))?;
    }
    std::fs::create_dir_all(folder).with_context(|| format!("making {}", folder.display()))
}

fn capture_take(p: &TakePlan, folder: &Path, flint: &Path, args: &CaptureArgs) -> Result<Record> {
    fresh_folder(folder)?;
    let input = match &p.input {
        Some(source) => {
            let text = std::fs::read_to_string(source)
                .with_context(|| format!("reading {}", source.display()))?;
            let copy = folder.join("input.txt");
            std::fs::write(&copy, scale_input(&text, p.pixels_per_point))?;
            Some(copy)
        }
        None => None,
    };
    let words = p.play_words(folder, input.as_deref(), &args.schemas, &args.player_args);
    let log_path = folder.join("player.log");
    let log = std::fs::File::create(&log_path)?;
    progress!("  {} ...", p.label());
    let started = std::time::Instant::now();
    // The player's own output goes to its log, never to this command's
    // stdout (which may be a JSON document)
    let status = Command::new(flint)
        .args(&words)
        .stdin(Stdio::null())
        .stdout(Stdio::from(log.try_clone()?))
        .stderr(Stdio::from(log))
        .status()
        .with_context(|| format!("running {}", flint.display()))?;
    let wall_seconds = started.elapsed().as_secs_f64();
    let frames = frame_files(folder)?.len();
    let ok = status.success() && frames == p.frames();
    let exe = flint
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "flint".into());
    let record = Record {
        plan: p.clone(),
        frames,
        exit_code: status.code(),
        wall_seconds,
        ok,
        command: std::iter::once(exe).chain(words).collect(),
        folder: folder.to_path_buf(),
        verify: None,
        encoded: None,
    };
    std::fs::write(
        folder.join("take.json"),
        serde_json::to_string_pretty(&record.take_json())?,
    )?;
    let status_word = if ok {
        "ok".to_string()
    } else if !status.success() {
        format!("FAILED (exit {:?})", status.code())
    } else {
        "FAILED (frame count)".to_string()
    };
    say!(
        "  {}: {frames} frames ({} expected) in {wall_seconds:.0} s  {status_word}",
        folder
            .strip_prefix(folder.parent().and_then(Path::parent).unwrap_or(folder))
            .unwrap_or(folder)
            .display()
            .to_string()
            .replace('\\', "/"),
        p.frames()
    );
    if !ok {
        say!("    see {}", shown(&log_path));
    }
    Ok(record)
}

/// `frame_*.png` in a take's folder, by name.
pub fn frame_files(folder: &Path) -> Result<Vec<String>> {
    let mut names: Vec<String> = std::fs::read_dir(folder)?
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with("frame_") && n.ends_with(".png"))
        .collect();
    names.sort();
    Ok(names)
}

fn ffmpeg_present() -> bool {
    Command::new("ffmpeg")
        .arg("-version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// The ffmpeg words that turn a take's frames into an MP4: sRGB frames
/// into BT.709 limited-range YUV 4:2:0, tagged as such on the frames (ffmpeg
/// 8 takes a stream's tags from them, #436), an even size, no sound.
pub fn mp4_words(folder: &Path, fps: u32, file: &Path) -> Vec<String> {
    let filter = "scale=w=trunc(iw/2)*2:h=trunc(ih/2)*2:out_color_matrix=bt709:out_range=tv:\
                  flags=lanczos+accurate_rnd+full_chroma_int,format=yuv420p,\
                  setparams=color_primaries=bt709:color_trc=bt709:colorspace=bt709:range=tv";
    [
        "-hide_banner",
        "-loglevel",
        "error",
        "-y",
        "-framerate",
        &fps.to_string(),
        "-i",
        &folder.join("frame_%04d.png").display().to_string(),
        "-vf",
        filter,
        "-c:v",
        "libx264",
        "-preset",
        "slow",
        "-crf",
        "16",
        "-movflags",
        "+faststart",
        "-an",
        &file.display().to_string(),
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

fn encode_mp4(folder: &Path, fps: u32, file: &Path) -> Result<()> {
    let status = Command::new("ffmpeg")
        .args(mp4_words(folder, fps, file))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .status()
        .context("running ffmpeg")?;
    if !status.success() {
        bail!("ffmpeg exited {:?}", status.code());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIST: &str = r#"
[capture]
title = "Three"
fps = 10
width = 320
height = 180
points_wide = 160
look = "graphite"

[[shot]]
id = "open"
scene = "scenes/main.scene.toml"
seconds = 1.0

[[shot]]
id = "looks"
scene = "scenes/main.scene.toml"
seconds = 2.0
seed = 7
take = [
    { name = "noir", look = "ascii_noir" },
    { name = "short", seconds = 0.5, args = ["--msaa", "4"] },
]

[[shot]]
id = "menu"
scene = "../other/menu.scene.toml"
seconds = 0.4
input = "menu.input.txt"
width = 640
height = 360
fps = 20
"#;

    fn over() -> Overrides {
        Overrides {
            scale: 1.0,
            ..Overrides::default()
        }
    }

    #[test]
    fn a_list_plans_a_take_per_shot_and_one_per_named_take() {
        let list = ShotList::parse(LIST).unwrap();
        let path = Path::new("/game/trailer.shots.toml");
        let (out, plans) = plan(&list, path, &over()).unwrap();
        assert!(out.ends_with("captures/trailer"), "{}", out.display());
        let labels: Vec<_> = plans.iter().map(TakePlan::label).collect();
        assert_eq!(
            labels,
            ["open/main", "looks/noir", "looks/short", "menu/main"]
        );
        assert_eq!(plans[0].frames(), 11);
        assert_eq!(plans[0].look.as_deref(), Some("graphite"));
        assert_eq!(plans[1].look.as_deref(), Some("ascii_noir"));
        assert_eq!(plans[1].seed, Some(7));
        assert_eq!(plans[2].frames(), 6);
        assert_eq!(plans[2].args, ["--msaa", "4"]);
        // 320 px over 160 points; the wider shot keeps the same pixels a
        // point, so its canvas is 320 points
        assert_eq!(plans[0].pixels_per_point, 2.0);
        assert_eq!(plans[3].pixels_per_point, 2.0);
        assert_eq!(plans[3].frames(), 9);
        assert!(plans[3].input.as_ref().unwrap().ends_with("menu.input.txt"));
        assert!(plans[1].folder.ends_with("looks/noir"));
    }

    #[test]
    fn scale_keeps_the_canvas_and_filters_pick_takes() {
        let list = ShotList::parse(LIST).unwrap();
        let o = Overrides {
            scale: 0.5,
            shots: vec!["looks".into()],
            takes: vec!["short".into()],
            out: Some(PathBuf::from("elsewhere")),
            ..Overrides::default()
        };
        let (out, plans) = plan(&list, Path::new("t.toml"), &o).unwrap();
        assert!(out.ends_with("elsewhere"));
        assert_eq!(plans.len(), 1);
        assert_eq!((plans[0].width, plans[0].height), (160, 90));
        assert_eq!(plans[0].pixels_per_point, 1.0);
        let none = Overrides {
            shots: vec!["nope".into()],
            ..over()
        };
        assert!(plan(&list, Path::new("t.toml"), &none).is_err());
    }

    #[test]
    fn the_play_command_steps_the_clock_and_writes_every_frame() {
        let list = ShotList::parse(LIST).unwrap();
        let (_, plans) = plan(&list, Path::new("t.toml"), &over()).unwrap();
        let p = &plans[1];
        let words = p.play_words(
            Path::new("o"),
            None,
            &["schemas".into()],
            &["--msaa".into(), "4".into()],
        );
        let after = |flag: &str| {
            let i = words.iter().position(|w| w == flag).unwrap();
            words[i + 1].clone()
        };
        assert_eq!(words[0], "play");
        assert_eq!(after("--frame-step"), "0.100000000");
        assert_eq!(after("--screenshot-every"), "0.100000000");
        assert_eq!(after("--exit-after"), "1.950000000");
        assert_eq!(after("--look"), "ascii_noir");
        assert_eq!(after("--seed"), "7");
        assert_eq!(after("--schemas"), "schemas");
        assert_eq!(after("--pixels-per-point"), "2.000000");
        assert!(words.contains(&"--no-activate".to_string()));
        assert_eq!(words.last().unwrap(), "4");
    }

    #[test]
    fn mistakes_in_a_list_are_refused() {
        for (text, says) in [
            ("[[shot]]\nid = \"a\"\nscene = \"s\"\nseconds = 1\nlenght = 2\n", "lenght"),
            ("[capture]\nfps = 0\n[[shot]]\nid = \"a\"\nscene = \"s\"\nseconds = 1\n", "above 0"),
            ("[[shot]]\nid = \"../a\"\nscene = \"s\"\nseconds = 1\n", "shot id"),
            (
                "[[shot]]\nid = \"a\"\nscene = \"s\"\nseconds = 1\n[[shot]]\nid = \"a\"\nscene = \"s\"\nseconds = 1\n",
                "two shots",
            ),
            ("[[shot]]\nid = \"a\"\nscene = \"s\"\nseconds = 0\n", "above 0"),
            (
                "[[shot]]\nid = \"a\"\nscene = \"s\"\nseconds = 1\ntake = [{ name = \"x\", seconds = 3 }]\n",
                "at most",
            ),
            ("[capture]\nfps = 30\n", "no [[shot]]"),
        ] {
            let err = format!("{:#}", ShotList::parse(text).unwrap_err());
            assert!(err.contains(says), "{text:?}: {err}");
        }
    }

    #[test]
    fn a_minimal_list_takes_the_defaults() {
        let list = ShotList::parse("[[shot]]\nid = \"a\"\nscene = \"s.scene.toml\"\nseconds = 2\n")
            .unwrap();
        let (_, plans) = plan(&list, Path::new("shots.toml"), &over()).unwrap();
        let p = &plans[0];
        assert_eq!((p.width, p.height, p.fps), (1280, 720, 30));
        assert_eq!(p.pixels_per_point, 1.0);
        assert_eq!(p.frames(), 61);
        assert_eq!(p.take, "main");
    }

    #[test]
    fn input_scripts_move_pointers_into_pixels() {
        let text = "0.5 mouse 100 50 0.2  # glide\n1 click 10 20\n2 touch 1 30 40\n\
                    3 drag 1 5 6 0.4\n4 look 40 -10\n5 tap KeyF\n# only a comment\n";
        let scaled = scale_input(text, 2.0);
        let lines: Vec<&str> = scaled.lines().collect();
        assert_eq!(lines[0], "0.5  mouse  200.0  100.0  0.2  # glide");
        assert_eq!(lines[1], "1  click  20.0  40.0");
        assert_eq!(lines[2], "2  touch  1  60.0  80.0");
        assert_eq!(lines[3], "3  drag  1  10.0  12.0  0.4");
        assert_eq!(lines[4], "4 look 40 -10");
        assert_eq!(lines[5], "5 tap KeyF");
        assert_eq!(lines[6], "# only a comment");
        // One pixel a point copies the script as it is
        assert_eq!(scale_input(text, 1.0), text);
    }

    #[test]
    fn the_mp4_is_bt709_and_even() {
        let words = mp4_words(Path::new("take"), 60, Path::new("take.mp4"));
        let vf = &words[words.iter().position(|w| w == "-vf").unwrap() + 1];
        assert!(vf.contains("trunc(iw/2)*2"));
        assert!(vf.contains("color_primaries=bt709"));
        assert!(words.contains(&"60".to_string()));
        assert!(words.iter().any(|w| w.ends_with("frame_%04d.png")));
    }

    /// A list shaped like the showcase film's (flint-examples'
    /// `tools/showcase/film.toml` since #984: a 4K master laid out on a
    /// 1920-point canvas, scenes named from the list's folder, a beat shot
    /// two ways), over the frozen fixture scenes: every take plans, each
    /// scene is found, and a point is two pixels.
    #[test]
    fn a_film_shaped_list_parses_and_finds_its_scenes() {
        // Where the list would lie; nothing is read from it
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tools/film/film.toml");
        let list = ShotList::parse(
            "[capture]\ntitle = \"Many Hands\"\nfps = 60\nwidth = 3840\nheight = 2160\n\
             points_wide = 1920\nout = \"~/flint-film\"\n\n\
             [[shot]]\nid = \"01_plain\"\nscene = \"../../tests/fixtures/scenes/pbr_test.scene.toml\"\n\
             seconds = 6.0\n\n\
             [[shot]]\nid = \"02_type\"\nscene = \"../../tests/fixtures/scenes/ascii_entities.scene.toml\"\n\
             seconds = 14.0\n\
             [[shot.take]]\nname = \"main\"\n\
             [[shot.take]]\nname = \"short\"\nseconds = 4.0\n",
        )
        .unwrap();
        let (_, plans) = plan(&list, &path, &over()).unwrap();
        assert_eq!(plans.len(), 3, "{}", plans.len());
        for p in &plans {
            assert!(p.scene.is_file(), "{}: {}", p.label(), p.scene.display());
        }
        assert_eq!(plans[0].pixels_per_point, 2.0);
    }
}
