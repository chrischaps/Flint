//! The recorder's look-back, saved (#1097): `--trace FILE` at exit, the
//! `save_trace` debug key (Shift+F3) mid-play, `--trace-on-hitch[=N]` a few
//! frames after each of the run's first N hitches, and beside every frame
//! capture taken while it records.
//!
//! `flint_core::profile` keeps each thread's scopes in a ring (ADR 0128);
//! this module turns recording on, sizes the rings by seconds
//! (`--trace-seconds`, [`ring_records`]), keeps the player's own tracks
//! beside them ([`Tracks`]: each frame from begin to begin, each phase
//! between its marks, the GPU's scopes by the frame that drew them), and at
//! a save copies the last seconds of all of it on the frame thread
//! ([`TraceJob`]), then writes Chrome trace-event JSON on a thread of its
//! own. Hitches and frame notes come from the frame history and the hitch
//! detector, which keep them anyway.
//!
//! A save is taken at the frame's end, after `finish_frame`: no scope is
//! open on the frame thread then, so the frame just ended is whole. A
//! worker thread's scope still open at that moment isn't in the file
//! (records are written when a scope closes).
//!
//! Without the recorder in the build (a release build without the `trace`
//! feature) `profile::COMPILED_IN` is false, every path here starts with
//! it, and the optimizer leaves none of it in the binary.

use super::hitch::Hitch;
use super::perf::Phase;
use super::PlayerApp;
use crate::hitch_args::STARTUP_SECS;
use flint_core::profile::{
    self,
    chrome::{Arg, ChromeTrace},
    ThreadScopes,
};
use flint_render::{TimestampHealth, TracedGpuFrame};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use web_time::Instant;

/// Seconds the recorder looks back unless `--trace-seconds` says.
pub const DEFAULT_SECONDS: f64 = 5.0;

/// Scopes a second a thread's ring is sized for: 512 a frame at 64 fps, or
/// 136 a frame at 240 Hz. A frame with more scopes than that shortens what
/// the ring reaches back to: the file's header says where every frame is
/// whole (`covered_from_ms`) and the run's line how much that is (#1288).
/// A script's engine calls are summed, one record a function a scope, so
/// it is the scopes a frame and the script functions called that count.
pub const RECORDS_PER_SECOND: usize = 32_768;

/// Bytes a record takes in a ring (ADR 0128).
pub const RECORD_BYTES: usize = 40;

/// How many times its first size a ring may grow to (#1288), doubling
/// whenever a lap of it spans less than the look-back (by [`LAP_MARGIN`]):
/// 1 310 720 records, 52 MB, for the default 5 s. Float uncapped at 405
/// fps records about 150 000 scopes a second, which is 4.6 times
/// [`RECORDS_PER_SECOND`]; a game past even this says so in the run's
/// line, with the `--trace-seconds` that would hold its window.
pub const RING_GROWTH: usize = 8;

/// A lap must span this many times the look-back before a ring stops
/// growing, so a frame rate that wavers still fills the window.
pub const LAP_MARGIN: f64 = 1.25;

/// Frames a hitch's trace waits for after the hitch, so the file shows
/// what followed it too (the hitch log's eight).
const FRAMES_AFTER_HITCH: u32 = 8;

/// The records a thread's ring holds for `seconds` of look-back; 0 (every
/// record) for a whole session.
pub fn ring_records(seconds: f64) -> usize {
    if seconds <= 0.0 {
        0
    } else {
        (seconds * RECORDS_PER_SECOND as f64).ceil() as usize
    }
}

/// What the run asked the recorder for (`TraceArgs`).
#[derive(Clone, Debug, PartialEq)]
pub struct TraceSetup {
    /// Written when the run ends (`--trace FILE`).
    pub exit_file: Option<PathBuf>,
    /// Look-back; 0 keeps the whole session.
    pub seconds: f64,
    /// Hitches past the start to write a trace after (`--trace-on-hitch`).
    pub on_hitch: u32,
}

impl Default for TraceSetup {
    fn default() -> Self {
        Self {
            exit_file: None,
            seconds: DEFAULT_SECONDS,
            on_hitch: 0,
        }
    }
}

/// A stretch of the frame thread, ns from the recorder's epoch, and the
/// frame index (`profile::frame`) it belongs to.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Span {
    pub start_ns: u64,
    pub end_ns: u64,
    pub frame: u32,
}

/// The player's own tracks while it records: kept for the look-back and
/// trimmed by time as they go, so once warm they allocate nothing more.
#[derive(Default)]
pub(crate) struct Tracks {
    /// How far back to keep; `None` keeps everything.
    keep_ns: Option<u64>,
    frames: VecDeque<Span>,
    phases: VecDeque<(Phase, Span)>,
    gpu: VecDeque<TracedGpuFrame>,
    /// Whether the GPU's timestamps read as times (#1316), as of the
    /// newest frames handed over.
    gpu_timestamps: TimestampHealth,
    /// The frame under way: when it began, and its index.
    began: Option<(u64, u32)>,
}

impl Tracks {
    fn new(seconds: f64) -> Self {
        Self {
            keep_ns: (seconds > 0.0).then_some((seconds * 1e9) as u64),
            ..Default::default()
        }
    }

    /// A frame begins at `now`: the one under way ends.
    pub(crate) fn frame_began(&mut self, now: Instant) {
        let ns = profile::clock::instant_ns(now);
        if let Some((start_ns, frame)) = self.began.replace((ns, profile::frame())) {
            self.frames.push_back(Span {
                start_ns,
                end_ns: ns,
                frame,
            });
            self.trim(ns);
        }
    }

    /// `phase` ran from `from` to `to`, in the frame under way.
    pub(crate) fn phase(&mut self, phase: Phase, from: Instant, to: Instant) {
        self.phases.push_back((
            phase,
            Span {
                start_ns: profile::clock::instant_ns(from),
                end_ns: profile::clock::instant_ns(to),
                frame: profile::frame(),
            },
        ));
    }

    /// The GPU's frames read back this frame, and whether the timer's
    /// timestamps can be read as pass times (#1316).
    pub(crate) fn gpu(&mut self, frames: Vec<TracedGpuFrame>, timestamps: TimestampHealth) {
        self.gpu.extend(frames);
        self.gpu_timestamps = timestamps;
    }

    fn trim(&mut self, now_ns: u64) {
        // A second over the look-back, so a save's window is always full
        let Some(keep) = self.keep_ns.map(|k| k + 1_000_000_000) else {
            return;
        };
        let cutoff = now_ns.saturating_sub(keep);
        while self.frames.front().is_some_and(|f| f.end_ns < cutoff) {
            self.frames.pop_front();
        }
        while self.phases.front().is_some_and(|(_, s)| s.end_ns < cutoff) {
            self.phases.pop_front();
        }
        let oldest = self.frames.front().map_or(0, |f| f.frame as u64);
        while self.gpu.front().is_some_and(|g| g.tag < oldest) {
            self.gpu.pop_front();
        }
    }
}

/// Why a trace is written.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Reason {
    /// The run ended (`--trace FILE`).
    Exit(PathBuf),
    /// The debug key, which says so on screen.
    Key(String),
    /// A hitch, with its cause line.
    Hitch {
        line: String,
        game_time: f64,
        ms: f32,
    },
    /// Into a frame capture's folder, as `trace.json` (desktop only).
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    Capture(PathBuf),
}

impl Reason {
    fn word(&self) -> &'static str {
        match self {
            Reason::Exit(_) => "exit",
            Reason::Key(_) => "key",
            Reason::Hitch { .. } => "hitch",
            Reason::Capture(_) => "capture",
        }
    }
}

/// A save waiting for the end of frame `due`.
struct Pending {
    reason: Reason,
    due: u32,
}

/// The recorder's state in the player: what was asked, the tracks, the
/// saves waiting for their frame and the writers on their way.
pub(crate) struct Tracer {
    pub(crate) setup: TraceSetup,
    pub(crate) tracks: Tracks,
    on_hitch_left: u32,
    passed_startup: u32,
    pending: Vec<Pending>,
    /// Paths written or being written this run, so none is written twice.
    names: Vec<PathBuf>,
    #[cfg(not(target_arch = "wasm32"))]
    writers: Vec<(Reason, std::thread::JoinHandle<Result<String, String>>)>,
    /// What to say: on the console, and on screen for the key's.
    said: Vec<(Option<String>, String)>,
    failures: Vec<String>,
}

impl Tracer {
    fn new(setup: TraceSetup) -> Self {
        Self {
            tracks: Tracks::new(setup.seconds),
            on_hitch_left: setup.on_hitch,
            passed_startup: 0,
            pending: Vec::new(),
            names: Vec::new(),
            #[cfg(not(target_arch = "wasm32"))]
            writers: Vec::new(),
            said: Vec::new(),
            failures: Vec::new(),
            setup,
        }
    }

    /// The window a save takes, ns from the epoch: the last `seconds`.
    fn since_ns(&self, now_ns: u64) -> Option<u64> {
        (self.setup.seconds > 0.0).then(|| now_ns.saturating_sub((self.setup.seconds * 1e9) as u64))
    }

    /// The file a mid-run trace goes to: beside `--trace`'s file when there
    /// is one (`<its stem>-<why>-t<game time>.json`), else `traces/` at
    /// the project's root (`<scene stem>-...`).
    fn path_for(&mut self, why: &str, game_time: f64, scene_path: &str, root: &Path) -> PathBuf {
        let (dir, stem) = match &self.setup.exit_file {
            Some(file) => (
                file.parent().map(Path::to_path_buf).unwrap_or_default(),
                file.file_stem()
                    .map_or("trace".into(), |s| s.to_string_lossy().into_owned()),
            ),
            None => (root.join("traces"), scene_stem(scene_path).to_string()),
        };
        let base = format!("{stem}-{why}-t{game_time:.3}");
        let mut path = dir.join(format!("{base}.json"));
        let mut n = 2;
        while self.names.contains(&path) {
            path = dir.join(format!("{base}-{n}.json"));
            n += 1;
        }
        self.names.push(path.clone());
        path
    }
}

/// `levels/tavern_hub.scene.toml` -> `tavern_hub`.
fn scene_stem(scene_path: &str) -> &str {
    let name = Path::new(scene_path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("scene");
    name.strip_suffix(".scene.toml")
        .or_else(|| name.strip_suffix(".toml"))
        .unwrap_or(name)
}

/// A hitch as the file marks it.
#[derive(Clone, Debug)]
pub(crate) struct HitchMark {
    start_ns: u64,
    dur_ns: u64,
    index: u64,
    game_time: f64,
    judged_ms: f32,
    median_ms: f32,
    cause: String,
}

/// A frame note as the file marks it: at its frame's start.
#[derive(Clone, Debug)]
pub(crate) struct NoteMark {
    at_ns: u64,
    kind: &'static str,
    detail: String,
    ms: Option<f32>,
    depth: u8,
}

/// The file's header: what was recorded, where and why.
#[derive(Clone, Debug, Default)]
pub(crate) struct Header {
    reason: String,
    scene: String,
    game_time: f64,
    adapter: Option<(String, String, String)>,
    seconds: f64,
    from_ns: Option<u64>,
    to_ns: u64,
    ring_capacity: usize,
    /// Where git is asked for the project's commit (desktop only).
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    project_root: PathBuf,
}

/// One save, copied on the frame thread: everything the file says, owned,
/// so it can be written anywhere.
pub(crate) struct TraceJob {
    pub(crate) path: PathBuf,
    threads: Vec<ThreadScopes>,
    frames: Vec<Span>,
    phases: Vec<(Phase, Span)>,
    gpu: Vec<TracedGpuFrame>,
    /// Why the GPU track holds no passes (#1316): the adapter's timestamps
    /// read zero. `None` when they read as times.
    gpu_unavailable: Option<String>,
    hitches: Vec<HitchMark>,
    notes: Vec<NoteMark>,
    header: Header,
}

/// Where a save's scopes begin, when a ring lost some of its window
/// (#1288): the latest of the cut threads' [`ThreadScopes::cut_at_ns`].
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Cut {
    pub at_ns: u64,
    /// The threads whose rings wrapped inside the window.
    pub threads: Vec<String>,
}

impl TraceJob {
    /// Scopes in the file, every thread's.
    pub(crate) fn scopes(&self) -> usize {
        self.threads.iter().map(|t| t.scopes.len()).sum()
    }

    /// Where the scopes begin, if a ring lost part of the window: a frame
    /// that began after it has every scope; one before it may lack some.
    pub(crate) fn cut(&self) -> Option<Cut> {
        let cut: Vec<(&str, u64)> = self
            .threads
            .iter()
            .filter_map(|t| t.cut_at_ns().map(|at| (t.thread.as_str(), at)))
            .collect();
        let at_ns = cut.iter().map(|(_, at)| *at).max()?;
        Some(Cut {
            at_ns,
            threads: cut.iter().map(|(t, _)| t.to_string()).collect(),
        })
    }

    /// What the run says of the cut: how much of the look-back the scopes
    /// cover, the frames, and the `--trace-seconds` that would hold it.
    pub(crate) fn coverage_line(&self) -> Option<String> {
        let cut = self.cut()?;
        let covered: Vec<&Span> = self
            .frames
            .iter()
            .filter(|f| f.start_ns >= cut.at_ns)
            .collect();
        let held = self.header.from_ns.map_or(0.0, |from| {
            self.header.to_ns.saturating_sub(from) as f64 / 1e9
        });
        let covered_s = self.header.to_ns.saturating_sub(cut.at_ns) as f64 / 1e9;
        let frames = match (
            covered.first(),
            covered.last(),
            self.frames.first(),
            self.frames.last(),
        ) {
            (Some(a), Some(b), Some(c), Some(d)) => {
                format!(
                    "frames {}..{} of {}..{}",
                    a.frame, b.frame, c.frame, d.frame
                )
            }
            _ => "no whole frame".to_string(),
        };
        // A ring's size, grown or not, is in proportion to `seconds`: this
        // many seconds of look-back would hold `held` of this game's scopes
        let enough = (held > 0.0 && covered_s > 0.0)
            .then(|| (self.header.seconds * held / covered_s).ceil());
        let mut line = format!(
            "the {} ring{} wrapped: scopes cover the last {covered_s:.2} s of {held:.0} s ({frames})",
            cut.threads.join(", "),
            if cut.threads.len() == 1 { "" } else { "s" },
        );
        if let Some(enough) = enough {
            line.push_str(&format!(
                "; --trace-seconds {enough:.0} would hold {held:.0} s"
            ));
        }
        Some(line)
    }

    /// The file's text. `found` is the engine the binary was built from and
    /// the project's checkout (#1290), `written_at` the time, both found off
    /// the frame thread.
    pub(crate) fn to_json(
        &self,
        found: &crate::provenance::Provenance,
        written_at: Option<String>,
    ) -> String {
        let mut trace = ChromeTrace::new();
        trace.process(&format!("flint-player: {}", scene_stem(&self.header.scene)));
        for thread in &self.threads {
            trace.scopes(thread);
        }
        let frames = trace.track("frames");
        for f in &self.frames {
            let ms = (f.end_ns - f.start_ns) as f64 / 1e6;
            trace.complete(
                frames,
                "frame",
                f.start_ns,
                f.end_ns - f.start_ns,
                &[("frame", Arg::Int(f.frame as i64)), ("ms", Arg::Num(ms))],
            );
        }
        let phases = trace.track("phases");
        for (phase, s) in &self.phases {
            trace.complete(
                phases,
                phase.label(),
                s.start_ns,
                s.end_ns - s.start_ns,
                &[("frame", Arg::Int(s.frame as i64))],
            );
        }
        let gpu = trace.track("gpu");
        if let Some(why) = &self.gpu_unavailable {
            // Spans of 0 ms would read as passes that cost nothing (#1316):
            // one mark says why the track is empty
            let at = self
                .frames
                .first()
                .map_or(self.header.from_ns.unwrap_or(0), |f| f.start_ns);
            trace.instant(
                gpu,
                flint_render::TIMESTAMPS_UNAVAILABLE,
                at,
                &[("why", Arg::Str(why))],
            );
        }
        for g in self.gpu.iter().filter(|_| self.gpu_unavailable.is_none()) {
            // The GPU's clock isn't the CPU's: each frame's scopes are laid
            // from its render phase's start, durations as the GPU measured
            let Some(base) = self.render_start(g.tag as u32) else {
                continue;
            };
            let total: f32 = g.passes.iter().map(|p| p.ms).sum();
            for p in &g.passes {
                trace.complete(
                    gpu,
                    p.label,
                    base + (p.start_ms as f64 * 1e6) as u64,
                    (p.ms as f64 * 1e6) as u64,
                    &[
                        ("frame", Arg::Int(g.tag as i64)),
                        ("gpu_frame_ms", Arg::Num(total as f64)),
                    ],
                );
            }
        }
        let hitches = trace.track("hitches");
        for h in &self.hitches {
            trace.complete(
                hitches,
                "hitch",
                h.start_ns,
                h.dur_ns,
                &[
                    ("history_frame", Arg::Int(h.index as i64)),
                    ("game_time", Arg::Num(h.game_time)),
                    ("judged_ms", Arg::Num(h.judged_ms as f64)),
                    ("median_ms", Arg::Num(h.median_ms as f64)),
                    ("cause", Arg::Str(&h.cause)),
                ],
            );
        }
        let notes = trace.track("notes");
        for n in &self.notes {
            let mut args = vec![
                ("detail", Arg::Str(&n.detail)),
                ("depth", Arg::Int(n.depth as i64)),
            ];
            if let Some(ms) = n.ms {
                args.push(("ms", Arg::Num(ms as f64)));
            }
            trace.instant(notes, n.kind, n.at_ns, &args);
        }

        let h = &self.header;
        let scopes = self.scopes() as i64;
        let earliest = self
            .threads
            .iter()
            .filter_map(|t| t.scopes.first().map(|s| s.start_ns))
            .min();
        let wrapped: Vec<&str> = self
            .threads
            .iter()
            .filter(|t| t.wrapped())
            .map(|t| t.thread.as_str())
            .collect();
        let wrapped = wrapped.join(", ");
        let version = env!("CARGO_PKG_VERSION");
        let written_by = format!("flint-player {version}");
        let (adapter, backend, driver) = h.adapter.clone().unwrap_or_default();
        let build = if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        };
        let mut features = Vec::new();
        if cfg!(feature = "trace") {
            features.push("trace");
        }
        if cfg!(feature = "profile") {
            features.push("profile");
        }
        let features = features.join(",");
        let ring_bytes = (h.ring_capacity * RECORD_BYTES) as i64;
        let written_at = written_at.unwrap_or_default();
        let ms = |ns: u64| ns as f64 / 1e6;
        // Only when the GPU track is empty for it (#1316)
        let gpu_pass_times = self
            .gpu_unavailable
            .as_ref()
            .map(|why| format!("{}: {why}", flint_render::TIMESTAMPS_UNAVAILABLE));
        let mut header = vec![
            ("format", Arg::Str("flint-trace")),
            ("version", Arg::Int(1)),
            ("written_by", Arg::Str(&written_by)),
            ("written_at", Arg::Str(&written_at)),
            ("reason", Arg::Str(&h.reason)),
            ("scene", Arg::Str(&h.scene)),
            ("game_time", Arg::Num(h.game_time)),
            ("adapter", Arg::Str(&adapter)),
            ("backend", Arg::Str(&backend)),
            ("driver", Arg::Str(&driver)),
            ("build", Arg::Str(build)),
            ("features", Arg::Str(&features)),
            ("seconds", Arg::Num(h.seconds)),
            ("window_to_ms", Arg::Num(ms(h.to_ns))),
            ("scopes", Arg::Int(scopes)),
            ("frames", Arg::Int(self.frames.len() as i64)),
            ("ring_records", Arg::Int(h.ring_capacity as i64)),
            ("ring_bytes_per_thread", Arg::Int(ring_bytes)),
            ("threads", Arg::Int(self.threads.len() as i64)),
            ("wrapped", Arg::Str(&wrapped)),
            (
                "gpu_placement",
                Arg::Str("each frame's GPU scopes from its render phase's start; durations the GPU's"),
            ),
            (
                "contents",
                Arg::Str("names only: scopes, scripts, assets and the scene by name, never their contents"),
            ),
        ];
        if let Some(said) = &gpu_pass_times {
            header.push(("gpu_pass_times", Arg::Str(said)));
        }
        // The commit the binary was built from; none when the build didn't
        // know, which readers say as "unknown" (#1290)
        if let Some(commit) = &found.engine.commit {
            header.push(("engine_commit", Arg::Str(commit)));
        }
        if let Some(dirty) = found.engine.dirty {
            header.push(("engine_dirty", Arg::Bool(dirty)));
        }
        if let Some(subdir) = &found.engine_subdir {
            header.push(("engine_subdir", Arg::Str(subdir)));
        }
        // The checkout the run's project is in: a game's own HEAD
        if let Some(commit) = &found.project.commit {
            header.push(("project_commit", Arg::Str(commit)));
        }
        if let Some(dirty) = found.project.dirty {
            header.push(("project_dirty", Arg::Bool(dirty)));
        }
        if let Some(from) = h.from_ns {
            header.push(("window_from_ms", Arg::Num(ms(from))));
        }
        if let Some(earliest) = earliest {
            header.push(("scopes_from_ms", Arg::Num(ms(earliest))));
        }
        // Where every frame is whole, when a ring lost part of the window
        // (#1288): `flint profile` and the flame view read from here.
        // `cut_threads` is written either way, empty for a whole window, so
        // a reader knows this writer said, and never guesses
        let cut = self.cut();
        let cut_threads = cut.as_ref().map_or(String::new(), |c| c.threads.join(", "));
        // The largest a ring grew to, when one did (#1288)
        let grown = self.threads.iter().filter_map(|t| t.capacity).max();
        if let Some(grown) = grown.filter(|g| *g > h.ring_capacity && h.ring_capacity > 0) {
            header.push(("ring_grown_to", Arg::Int(grown as i64)));
        }
        header.push(("cut_threads", Arg::Str(&cut_threads)));
        if let Some(cut) = &cut {
            header.push(("covered_from_ms", Arg::Num(ms(cut.at_ns))));
        }
        trace.finish(&header)
    }

    /// When frame `frame`'s render phase began: the first of its `Render`
    /// marks, or the frame's own start.
    fn render_start(&self, frame: u32) -> Option<u64> {
        self.phases
            .iter()
            .find(|(p, s)| *p == Phase::Render && s.frame == frame)
            .map(|(_, s)| s.start_ns)
            .or_else(|| {
                self.frames
                    .iter()
                    .find(|f| f.frame == frame)
                    .map(|f| f.start_ns)
            })
    }
}

/// Write `job` to its path; the line to say, or why not.
#[cfg(not(target_arch = "wasm32"))]
#[allow(clippy::disallowed_methods)] // a write path: the trace file the run asked for
fn write(job: TraceJob) -> Result<String, String> {
    let found = crate::provenance::Provenance::of(&job.header.project_root);
    let text = job.to_json(&found, Some(super::frame_capture::utc_now()));
    let shown = flint_core::project::shown(&job.path);
    if let Some(dir) = job.path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir).map_err(|e| format!("{shown}: {e}"))?;
    }
    std::fs::write(&job.path, &text).map_err(|e| format!("{shown}: {e}"))?;
    let mut line = format!(
        "wrote {shown}: {} scopes over {} frames, {:.1} MB",
        job.scopes(),
        job.frames.len(),
        text.len() as f64 / 1e6
    );
    if let Some(cut) = job.coverage_line() {
        line.push_str("; ");
        line.push_str(&cut);
    }
    Ok(line)
}

impl PlayerApp {
    /// Record the engine's scopes from now on, as `setup` says (#1097):
    /// size each thread's ring for its seconds, turn the recorder on and
    /// the frame history with it. A build without the recorder does
    /// nothing (`TraceArgs::apply` refused its flags already).
    pub fn trace_scopes(&mut self, setup: TraceSetup) {
        if !profile::COMPILED_IN {
            return;
        }
        // Before any ring is made: a thread's ring is sized at its first record
        if !profile::recording() {
            profile::set_ring_capacity(ring_records(setup.seconds));
            // ... and grows, to RING_GROWTH times that, when the game
            // records more than RECORDS_PER_SECOND (#1288)
            if setup.seconds > 0.0 {
                profile::set_ring_growth(
                    setup.seconds * LAP_MARGIN,
                    ring_records(setup.seconds) * RING_GROWTH,
                );
            }
        }
        profile::set_recording(true);
        self.stats.perf.always_on = true;
        if setup.on_hitch > 0 {
            // A hitch's cause names its script, as --capture-on-hitch does
            self.core.script.set_timing(true);
        }
        self.stats.perf.trace = Some(Box::new(Tracer::new(setup)));
        // Every frame capture from now on gets the trace beside it
        self.frame_captures.with_trace = true;
    }

    /// In `tick`, after the hitch detector: the hitches it just found
    /// (`found`, the newest) for `--trace-on-hitch`, past the start, each
    /// written once [`FRAMES_AFTER_HITCH`] frames have followed it.
    pub(super) fn trace_triggers(&mut self, found: usize) {
        if !profile::COMPILED_IN {
            return;
        }
        let Some(tracer) = self.stats.perf.trace.as_deref_mut() else {
            return;
        };
        if tracer.on_hitch_left == 0 || found == 0 {
            return;
        }
        let hitches = self.stats.hitches.hitches();
        for h in hitches.iter().skip(hitches.len().saturating_sub(found)) {
            if tracer.on_hitch_left == 0 {
                break;
            }
            if h.game_time < STARTUP_SECS {
                if tracer.passed_startup == 0 {
                    tracing::info!(
                        target: "player",
                        "[trace] passed over a startup hitch at t={:.2} s (before t={STARTUP_SECS:.0} s)",
                        h.game_time
                    );
                }
                tracer.passed_startup += 1;
                continue;
            }
            tracer.on_hitch_left -= 1;
            tracer.pending.push(Pending {
                reason: Reason::Hitch {
                    line: h.cause_line(),
                    game_time: h.game_time,
                    ms: h.judged_ms,
                },
                due: profile::frame() + FRAMES_AFTER_HITCH,
            });
        }
    }

    /// The `save_trace` debug key, on `key`: write the look-back now, or
    /// start recording if nothing records yet.
    pub(super) fn ask_trace(&mut self, key: &str) {
        if !profile::COMPILED_IN {
            self.hud.key_note.say(
                key,
                "this build has no scope recorder: a debug build has it, or build with --features trace",
            );
            return;
        }
        let Some(tracer) = self.stats.perf.trace.as_deref_mut() else {
            self.trace_scopes(TraceSetup::default());
            self.hud.key_note.say(
                key,
                &format!(
                    "recording scopes from now: press {key} again to save the last {DEFAULT_SECONDS:.0} s"
                ),
            );
            return;
        };
        tracer.pending.push(Pending {
            reason: Reason::Key(key.to_string()),
            due: profile::frame(),
        });
    }

    /// The frame captures just handed to their writers: each gets the
    /// trace in its folder as `trace.json`, at this frame's end (or now,
    /// when the run is ending). Desktop only, as captures are.
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    pub(crate) fn traces_into_captures(&mut self) {
        if !profile::COMPILED_IN {
            return;
        }
        let handed = std::mem::take(&mut self.frame_captures.handed);
        let Some(tracer) = self.stats.perf.trace.as_deref_mut() else {
            return;
        };
        for dir in handed {
            tracer.pending.push(Pending {
                reason: Reason::Capture(dir),
                due: profile::frame(),
            });
        }
    }

    /// After `finish_frame`: the saves due by now, and the writers done.
    pub(super) fn trace_frame_end(&mut self) {
        // A constant: without the recorder the optimizer drops all of this
        if !profile::COMPILED_IN {
            return;
        }
        let Some(tracer) = self.stats.perf.trace.as_deref_mut() else {
            return;
        };
        // `finish_frame` has counted the frame just ended
        let ended = profile::frame().saturating_sub(1);
        let mut due = Vec::new();
        tracer.pending.retain(|p| {
            if p.due <= ended {
                due.push(p.reason.clone());
                false
            } else {
                true
            }
        });
        for reason in due {
            self.save_trace(reason);
        }
        self.collect_trace_writers(false);
        self.say_traces();
    }

    /// Write the exit trace, if one was asked for, and wait for every
    /// writer: the run is ending.
    pub(super) fn finish_traces(&mut self) {
        if !profile::COMPILED_IN {
            return;
        }
        let Some(tracer) = self.stats.perf.trace.as_deref_mut() else {
            return;
        };
        let due: Vec<Reason> = tracer.pending.drain(..).map(|p| p.reason).collect();
        for reason in due {
            self.save_trace(reason);
        }
        if let Some(file) = self
            .stats
            .perf
            .trace
            .as_ref()
            .and_then(|t| t.setup.exit_file.clone())
        {
            self.save_trace(Reason::Exit(file));
        }
        self.collect_trace_writers(true);
        self.say_traces();
    }

    /// Why the run's traces failed, if any did: the run fails with it.
    pub(crate) fn trace_failure(&self) -> Option<String> {
        let tracer = self.stats.perf.trace.as_deref()?;
        let first = tracer.failures.first()?;
        Some(format!("trace not written: {first}"))
    }

    /// Copy the look-back now and hand it to a writer.
    fn save_trace(&mut self, reason: Reason) {
        if !profile::COMPILED_IN {
            return;
        }
        let job = self.trace_job(reason.clone());
        let Some(tracer) = self.stats.perf.trace.as_deref_mut() else {
            return;
        };
        #[cfg(not(target_arch = "wasm32"))]
        tracer
            .writers
            .push((reason, std::thread::spawn(move || write(job))));
        #[cfg(target_arch = "wasm32")]
        {
            let name = job
                .path
                .file_name()
                .map_or("trace.json".into(), |n| n.to_string_lossy().into_owned());
            let text = job.to_json(&crate::provenance::Provenance::built(), None);
            let line = match super::web_page::download_text(&name, &text) {
                Ok(()) => match job.coverage_line() {
                    Some(cut) => format!(
                        "offered {name} as a download: {} scopes; {cut}",
                        job.scopes()
                    ),
                    None => format!("offered {name} as a download: {} scopes", job.scopes()),
                },
                Err(e) => {
                    tracer.failures.push(e.clone());
                    format!("failed: {e}")
                }
            };
            let key = match &reason {
                Reason::Key(key) => Some(key.clone()),
                _ => None,
            };
            tracer.said.push((key, line));
        }
    }

    /// The look-back as one owned job, for `reason`.
    pub(crate) fn trace_job(&mut self, reason: Reason) -> TraceJob {
        let now_ns = profile::clock::now_ns();
        let game_time = self.core.clock.total_time;
        let scene_path = self.core.scene_path.clone();
        let scene_dir = Path::new(&scene_path)
            .parent()
            .unwrap_or(Path::new("."))
            .to_path_buf();
        let root = self.core.project.root_for(&scene_dir);
        let adapter = self.render_context.as_ref().map(|c| {
            let info = c.adapter.get_info();
            (info.name, format!("{:?}", info.backend), info.driver)
        });
        let history_start = self.stats.perf.start;
        let at_ns = |secs: f64| {
            profile::clock::instant_ns(
                history_start + std::time::Duration::from_secs_f64(secs.max(0.0)),
            )
        };
        let in_window = |since: Option<u64>, end_ns: u64| since.is_none_or(|s| end_ns >= s);

        let tracer = self.stats.perf.trace.as_deref_mut().expect("tracing");
        let since = tracer.since_ns(now_ns);
        let seconds = tracer.setup.seconds;
        let path = match &reason {
            Reason::Exit(file) => file.clone(),
            Reason::Capture(dir) => dir.join("trace.json"),
            Reason::Key(_) => tracer.path_for("key", game_time, &scene_path, &root),
            Reason::Hitch { game_time, .. } => {
                tracer.path_for("hitch", *game_time, &scene_path, &root)
            }
        };
        let reason_line = match &reason {
            Reason::Hitch {
                line,
                game_time,
                ms,
            } => {
                format!("hitch at t={game_time:.3} s, {ms:.1} ms: {line}")
            }
            other => other.word().to_string(),
        };
        let tracks = &tracer.tracks;
        let gpu_unavailable = tracks.gpu_timestamps.why();
        let frames: Vec<Span> = tracks
            .frames
            .iter()
            .filter(|f| in_window(since, f.end_ns))
            .copied()
            .collect();
        let phases: Vec<(Phase, Span)> = tracks
            .phases
            .iter()
            .filter(|(_, s)| in_window(since, s.end_ns))
            .copied()
            .collect();
        let oldest = frames.first().map_or(0, |f| f.frame as u64);
        let gpu: Vec<TracedGpuFrame> = tracks
            .gpu
            .iter()
            .filter(|g| g.tag >= oldest)
            .cloned()
            .collect();
        let hitches = self
            .stats
            .hitches
            .hitches()
            .iter()
            .map(|h: &Hitch| HitchMark {
                start_ns: at_ns(h.at),
                dur_ns: (h.ms as f64 * 1e6) as u64,
                index: h.index,
                game_time: h.game_time,
                judged_ms: h.judged_ms,
                median_ms: h.median_ms,
                cause: h.cause_line(),
            })
            .filter(|h| in_window(since, h.start_ns + h.dur_ns))
            .collect();
        let notes = self
            .stats
            .perf
            .frames()
            .iter()
            .filter(|f| in_window(since, at_ns(f.end())))
            .flat_map(|f| {
                let at = at_ns(f.at);
                f.notes.iter().map(move |n| NoteMark {
                    at_ns: at,
                    kind: n.kind.label(),
                    detail: n.detail.clone(),
                    ms: n.ms,
                    depth: n.depth,
                })
            })
            .collect();
        TraceJob {
            path,
            threads: profile::snapshot(since),
            frames,
            phases,
            gpu,
            gpu_unavailable,
            hitches,
            notes,
            header: Header {
                reason: reason_line,
                scene: scene_path,
                game_time,
                adapter,
                seconds,
                from_ns: since,
                to_ns: now_ns,
                ring_capacity: profile::ring_capacity(),
                project_root: root,
            },
        }
    }

    /// Join the writers that finished (all of them with `wait`).
    fn collect_trace_writers(&mut self, wait: bool) {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let Some(tracer) = self.stats.perf.trace.as_deref_mut() else {
                return;
            };
            let mut i = 0;
            while i < tracer.writers.len() {
                if !wait && !tracer.writers[i].1.is_finished() {
                    i += 1;
                    continue;
                }
                let (reason, writer) = tracer.writers.remove(i);
                let key = match &reason {
                    Reason::Key(key) => Some(key.clone()),
                    _ => None,
                };
                let line = match writer.join() {
                    Ok(Ok(line)) => line,
                    Ok(Err(e)) => {
                        tracer.failures.push(e.clone());
                        format!("failed: {e}")
                    }
                    Err(_) => {
                        let e = "the trace's writer thread panicked".to_string();
                        tracer.failures.push(e.clone());
                        format!("failed: {e}")
                    }
                };
                tracer.said.push((key, line));
            }
        }
        #[cfg(target_arch = "wasm32")]
        let _ = wait;
    }

    /// What the writers said: on the console, and the key's on screen.
    fn say_traces(&mut self) {
        let Some(tracer) = self.stats.perf.trace.as_deref_mut() else {
            return;
        };
        for (key, line) in std::mem::take(&mut tracer.said) {
            // `report`, which --quiet keeps: a whole session says its size
            tracing::info!(target: "report", "[trace] {line}");
            if let Some(key) = key {
                self.hud.key_note.say(&key, &line);
            }
        }
    }
}

/// One test at a time holds the recorder: recording on or off and the frame
/// index (`finish_frame`) are the process's, so a test that turns either
/// would move another's (`trace_tests`, `scope_coverage_tests`).
#[cfg(test)]
pub(crate) fn recorder_test_turn() -> std::sync::MutexGuard<'static, ()> {
    static TURN: std::sync::Mutex<()> = std::sync::Mutex::new(());
    TURN.lock().unwrap_or_else(|e| e.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_ring_is_sized_by_its_seconds() {
        assert_eq!(ring_records(5.0), 163_840);
        assert_eq!(
            ring_records(5.0) * RECORD_BYTES,
            6_553_600,
            "6.6 MB a thread"
        );
        assert_eq!(ring_records(0.0), 0, "0 keeps the whole session");
    }

    #[test]
    fn the_tracks_keep_their_seconds_and_a_second_more() {
        let mut tracks = Tracks::new(1.0);
        let t0 = Instant::now();
        let step = std::time::Duration::from_millis(100);
        for i in 0..40u32 {
            let at = t0 + step * i;
            tracks.phase(Phase::Input, at, at + step / 2);
            tracks.frame_began(at);
        }
        let span = |s: &VecDeque<Span>| s.back().unwrap().end_ns - s.front().unwrap().start_ns;
        let kept = span(&tracks.frames) as f64 / 1e9;
        assert!(
            (1.8..=2.1).contains(&kept),
            "1 s of look-back and a second of slack, not 3.9 s: {kept}"
        );
        assert!(tracks.phases.len() <= 22, "{}", tracks.phases.len());
    }

    /// The Pixel 8 Pro's zeros (#1316): a GPU track of 0 ms spans reads as
    /// passes that cost nothing, so with unusable timestamps the track
    /// holds one mark saying why and the header says it too. With real
    /// times the track is as it was.
    #[test]
    fn a_gpu_track_of_unusable_timestamps_says_so_instead_of_spans() {
        let frame = TracedGpuFrame {
            tag: 7,
            passes: vec![
                flint_render::TracedPass {
                    label: "Scene",
                    start_ms: 0.0,
                    ms: 6.0,
                },
                flint_render::TracedPass {
                    label: "Composite",
                    start_ms: 6.0,
                    ms: 0.0,
                },
            ],
        };
        let job = |gpu_unavailable: Option<String>| TraceJob {
            path: PathBuf::new(),
            threads: Vec::new(),
            frames: vec![Span {
                start_ns: 1_000_000,
                end_ns: 17_000_000,
                frame: 7,
            }],
            phases: Vec::new(),
            gpu: vec![frame.clone()],
            gpu_unavailable,
            hitches: Vec::new(),
            notes: Vec::new(),
            header: Header::default(),
        };
        let gpu_events = |doc: &serde_json::Value| -> Vec<serde_json::Value> {
            let events = doc["traceEvents"].as_array().expect("traceEvents");
            let tid = events
                .iter()
                .find(|e| {
                    e["ph"] == "M" && e["name"] == "thread_name" && e["args"]["name"] == "gpu"
                })
                .and_then(|e| e["tid"].as_i64())
                .expect("a gpu track");
            events
                .iter()
                .filter(|e| e["tid"].as_i64() == Some(tid) && e["ph"] != "M")
                .cloned()
                .collect()
        };
        let read = |j: &TraceJob| -> serde_json::Value {
            serde_json::from_str(&j.to_json(&Default::default(), None)).expect("the trace is JSON")
        };

        let real = read(&job(None));
        let names: Vec<String> = gpu_events(&real)
            .iter()
            .map(|e| e["name"].as_str().unwrap().to_owned())
            .collect();
        assert_eq!(names, ["Scene", "Composite"]);
        assert!(real["otherData"].get("gpu_pass_times").is_none());

        let why = flint_render::TimestampHealth::Unusable { zero: 5, scopes: 7 }.why();
        let zeros = read(&job(why));
        let events = gpu_events(&zeros);
        assert_eq!(events.len(), 1, "{events:?}");
        assert_eq!(events[0]["ph"], "i");
        assert_eq!(events[0]["name"], flint_render::TIMESTAMPS_UNAVAILABLE);
        assert_eq!(
            events[0]["args"]["why"],
            "5 of 7 scopes read 0 ticks for 60 frames"
        );
        assert_eq!(
            zeros["otherData"]["gpu_pass_times"],
            "GPU pass times unavailable on this adapter: 5 of 7 scopes read 0 ticks for 60 frames"
        );
    }

    /// The header names the engine the binary was built from and the
    /// project's checkout apart (#1290), and says nothing of a commit it
    /// doesn't know rather than an empty one.
    #[test]
    fn the_header_names_the_engine_built_and_the_project_apart() {
        let job = TraceJob {
            path: PathBuf::new(),
            threads: Vec::new(),
            frames: Vec::new(),
            phases: Vec::new(),
            gpu: Vec::new(),
            gpu_unavailable: None,
            hitches: Vec::new(),
            notes: Vec::new(),
            header: Header::default(),
        };
        let header = |found: &crate::provenance::Provenance| {
            let text = job.to_json(found, None);
            let doc: serde_json::Value = serde_json::from_str(&text).expect("the trace is JSON");
            doc["otherData"].clone()
        };
        let in_a_game = crate::provenance::Provenance {
            engine: crate::provenance::Checkout {
                commit: Some("e".repeat(40)),
                dirty: Some(false),
            },
            engine_subdir: None,
            project: crate::provenance::Checkout {
                commit: Some("9".repeat(40)),
                dirty: Some(true),
            },
        };
        let h = header(&in_a_game);
        assert_eq!(h["engine_commit"], "e".repeat(40), "{h}");
        assert_eq!(h["engine_dirty"], false, "{h}");
        assert_eq!(h["project_commit"], "9".repeat(40), "{h}");
        assert_eq!(h["project_dirty"], true, "{h}");
        assert!(h.get("engine_subdir").is_none(), "{h}");

        let subtree = crate::provenance::Provenance {
            engine_subdir: Some("engine/".into()),
            ..in_a_game
        };
        assert_eq!(header(&subtree)["engine_subdir"], "engine/");

        let unknown = header(&Default::default());
        for key in ["engine_commit", "engine_dirty", "project_commit"] {
            assert!(unknown.get(key).is_none(), "{key}: {unknown}");
        }
    }

    /// What recording costs the player each frame beyond its scopes: the
    /// tracks' frame and ten phase spans. And what a save costs the frame
    /// thread (the copy) and its writer (the JSON), for five seconds of a
    /// 240 Hz frame of 20 scopes. `cargo test --release -p flint-player --features trace
    /// --lib trace_cost -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn trace_cost() {
        let mut tracks = Tracks::new(5.0);
        let frames = 20_000u32;
        let t0 = Instant::now();
        for _ in 0..frames {
            let now = Instant::now();
            tracks.frame_began(now);
            for p in Phase::ALL {
                tracks.phase(p, now, Instant::now());
            }
        }
        let per_frame = t0.elapsed().as_nanos() as f64 / frames as f64;
        println!(
            "tracks: {per_frame:.0} ns a frame (a frame and ten phases, Instant reads included)"
        );

        profile::set_recording(true);
        static SITE: flint_core::profile::Site = flint_core::profile::Site {
            name: "bench",
            file: file!(),
            line: line!(),
        };
        for _ in 0..(5 * 240 * 20) {
            let _s = profile::Scope::enter(&SITE);
        }
        let t0 = Instant::now();
        let threads = profile::snapshot(None);
        let copy = t0.elapsed();
        let job = TraceJob {
            path: PathBuf::new(),
            threads,
            frames: tracks.frames.iter().copied().collect(),
            phases: tracks.phases.iter().copied().collect(),
            gpu: Vec::new(),
            gpu_unavailable: None,
            hitches: Vec::new(),
            notes: Vec::new(),
            header: Header::default(),
        };
        let t0 = Instant::now();
        let text = job.to_json(&Default::default(), None);
        let json = t0.elapsed();
        let events = job.scopes() + job.frames.len() + job.phases.len();
        println!(
            "save: copy {:.2} ms for {} scopes (frame thread); JSON of {events} events \
             (scopes, frames, phases) {:.1} ms, {:.1} MB, {:.0} bytes an event (writer)",
            copy.as_secs_f64() * 1e3,
            job.scopes(),
            json.as_secs_f64() * 1e3,
            text.len() as f64 / 1e6,
            text.len() as f64 / events as f64
        );
    }

    /// A save of 5 s whose main ring lost all but its last second: the
    /// header and the run's line say where the scopes begin (#1288).
    #[test]
    fn a_ring_that_wrapped_inside_the_window_is_said() {
        let s = 1_000_000_000u64;
        let scope = |start_ns: u64| profile::TakenScope {
            name: "tick",
            file: "",
            line: 0,
            start_ns,
            dur_ns: 10_000_000,
            frame: 0,
            depth: 0,
            detail: None,
        };
        let frames: Vec<Span> = (0..50u32)
            .map(|i| Span {
                start_ns: 5 * s + i as u64 * 100_000_000,
                end_ns: 5 * s + (i as u64 + 1) * 100_000_000,
                frame: 100 + i,
            })
            .collect();
        let main = ThreadScopes {
            thread: "main".into(),
            // The ring holds only what began at 9 s
            scopes: (0..10).map(|i| scope(9 * s + i * 100_000_000)).collect(),
            held: 10,
            written: 400,
            capacity: Some(10),
        };
        let worker = ThreadScopes {
            thread: "loader".into(),
            scopes: vec![scope(6 * s)],
            held: 1,
            written: 1,
            capacity: Some(10),
        };
        let mut job = TraceJob {
            path: PathBuf::new(),
            threads: vec![main, worker],
            frames,
            phases: Vec::new(),
            gpu: Vec::new(),
            gpu_unavailable: None,
            hitches: Vec::new(),
            notes: Vec::new(),
            header: Header {
                seconds: 5.0,
                from_ns: Some(5 * s),
                to_ns: 10 * s,
                ..Header::default()
            },
        };
        let cut = job.cut().expect("the main ring was cut");
        assert_eq!(
            cut.threads,
            vec!["main".to_string()],
            "the loader's copy is whole"
        );
        assert_eq!(cut.at_ns, 9 * s + 10_000_000, "its oldest record's close");
        let line = job.coverage_line().unwrap();
        assert!(
            line.starts_with("the main ring wrapped: scopes cover the last 0.99 s of 5 s"),
            "{line}"
        );
        assert!(line.contains("frames 141..149 of 100..149"), "{line}");
        assert!(
            line.ends_with("; --trace-seconds 26 would hold 5 s"),
            "{line}"
        );
        let text = job.to_json(&Default::default(), None);
        let header: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(header["otherData"]["covered_from_ms"], 9010.0);
        assert_eq!(header["otherData"]["cut_threads"], "main");

        // A ring that still held records before the window is whole
        job.threads[0].held = 20;
        assert_eq!(job.cut(), None);
        assert_eq!(job.coverage_line(), None);
    }

    #[test]
    fn a_mid_run_trace_is_named_beside_the_exit_file_or_under_traces() {
        let mut beside = Tracer::new(TraceSetup {
            exit_file: Some(PathBuf::from("out/run.json")),
            ..TraceSetup::default()
        });
        let root = Path::new("game");
        assert_eq!(
            beside.path_for("hitch", 3.25, "levels/tavern.scene.toml", root),
            Path::new("out").join("run-hitch-t3.250.json")
        );
        assert_eq!(
            beside.path_for("hitch", 3.25, "levels/tavern.scene.toml", root),
            Path::new("out").join("run-hitch-t3.250-2.json"),
            "a second at one time gets a number"
        );
        let mut under = Tracer::new(TraceSetup::default());
        assert_eq!(
            under.path_for("key", 1.0, "levels/tavern.scene.toml", root),
            root.join("traces").join("tavern-key-t1.000.json")
        );
    }
}
