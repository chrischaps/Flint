//! Load reports (#1092): where one load's time went. A load is a scene's
//! start-up (process to first frame), a scene transition (the frame that
//! asked for it to the new scene's first frame), a chunk streamed in, or a
//! headless render's scene to its first capture.
//!
//! While a load is recorded on a thread, every timed frame note made there
//! ([`crate::frame_notes::begin`] / [`timed`](crate::frame_notes::timed))
//! is heard as a span of the [`Stage`] its kind maps to, whether or not
//! anyone listens for frame notes, and a host adds the stages no note
//! names ([`span`]: the window, the GPU device, the renderer). Spans nest
//! as notes do, and each stage gets the time of its spans **less the spans
//! inside them** (a texture decoded inside a model load is the texture's),
//! so the rows add up to the load's measured time, with what no span
//! named as one row of its own. Each span with a name (a file, a label) is
//! an asset; the report keeps the slowest by their whole time.
//!
//! The recorder is per thread, as frame notes are: a span on a worker
//! thread is not heard. While nothing is recorded, a frame note checks one
//! more thread-local flag and nothing else.
//!
//! An untimed note ([`crate::frame_notes::note`] with a time worked out by
//! hand) is not heard: its time may already hold the spans inside it. Code
//! that times by hand wraps the work in a [`span`] too.

use crate::frame_notes::NoteKind;
use serde::Serialize;
use std::cell::{Cell, RefCell};
use web_time::Instant;

/// How many slowest assets a report names.
pub const SLOWEST: usize = 5;

/// At most this many assets are kept while recording; the slowest are
/// picked from them.
const MAX_ASSETS: usize = 4096;

/// What kind of load.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum LoadKind {
    /// The process's first scene, up to its first frame on screen.
    Start,
    /// A scene transition, from the frame that asked to the new scene's
    /// first frame.
    Transition,
    /// A chunk's entities spawned, up to the next frame on screen.
    Chunk,
    /// A headless render's scene, up to its first capture.
    Render,
}

impl LoadKind {
    pub fn label(self) -> &'static str {
        match self {
            LoadKind::Start => "start",
            LoadKind::Transition => "transition",
            LoadKind::Chunk => "chunk",
            LoadKind::Render => "render",
        }
    }
}

/// A row of the report.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Stage {
    /// The scene file read, parsed, its prefabs expanded and its schemas
    /// merged.
    SceneRead,
    /// The window (and on the desktop the event loop).
    Window,
    /// The GPU adapter and device.
    Device,
    /// The scene renderer made, less the pipelines it built.
    Renderer,
    /// egui, its fonts and the text measurer.
    Hud,
    /// The sound device opened (and its mixer started).
    AudioDevice,
    /// The last scene torn down (a transition).
    Teardown,
    /// Procedural assets made or found in the cache, less the models and
    /// textures they loaded.
    Procgen,
    /// glTF models read, decoded and uploaded, less their textures.
    Models,
    /// Texture files decoded and uploaded.
    Textures,
    /// Render and compute pipelines built.
    Pipelines,
    /// Post-process styles' passes built, less their pipelines.
    Styles,
    /// The physics world built: bodies, colliders, the character.
    Physics,
    /// Sounds found, read and decoded.
    Audio,
    /// Animation clips, sprite clips and sequences loaded.
    Animation,
    /// Particle effects and their textures loaded.
    Particles,
    /// Scripts read and compiled.
    Scripts,
    /// The scripts' `on_init` (and `on_scene_enter` at a transition).
    ScriptInit,
    /// The first frame after the bring-up, up to its present, less what it
    /// loaded on first sight (a pipeline on its first draw is a pipeline).
    FirstFrame,
}

impl Stage {
    /// Every stage, in the order a start-up meets them.
    pub const ALL: [Stage; 19] = [
        Stage::SceneRead,
        Stage::Window,
        Stage::Device,
        Stage::Renderer,
        Stage::Hud,
        Stage::AudioDevice,
        Stage::Teardown,
        Stage::Procgen,
        Stage::Models,
        Stage::Textures,
        Stage::Pipelines,
        Stage::Styles,
        Stage::Physics,
        Stage::Audio,
        Stage::Animation,
        Stage::Particles,
        Stage::Scripts,
        Stage::ScriptInit,
        Stage::FirstFrame,
    ];

    /// The words a row is named by (and its JSON `stage`).
    pub fn label(self) -> &'static str {
        match self {
            Stage::SceneRead => "scene read",
            Stage::Window => "window",
            Stage::Device => "gpu device",
            Stage::Renderer => "renderer",
            Stage::Hud => "hud",
            Stage::AudioDevice => "audio device",
            Stage::Teardown => "teardown",
            Stage::Procgen => "procgen",
            Stage::Models => "models",
            Stage::Textures => "textures",
            Stage::Pipelines => "pipelines",
            Stage::Styles => "styles",
            Stage::Physics => "physics",
            Stage::Audio => "audio",
            Stage::Animation => "animation",
            Stage::Particles => "particles",
            Stage::Scripts => "scripts",
            Stage::ScriptInit => "on_init",
            Stage::FirstFrame => "first frame",
        }
    }

    /// The words an asset of this stage is named by: `model`, `texture`.
    pub fn asset_word(self) -> &'static str {
        match self {
            Stage::Models => "model",
            Stage::Textures => "texture",
            Stage::Pipelines => "pipeline",
            Stage::Styles => "style",
            Stage::Scripts => "script",
            Stage::Audio => "sound",
            Stage::Procgen => "procgen asset",
            other => other.label(),
        }
    }

    /// The stage a timed note of `kind` counts toward, or `None` for a
    /// note that only holds others (a transition, a chunk) or isn't load
    /// work.
    pub fn of_note(kind: NoteKind) -> Option<Stage> {
        Some(match kind {
            NoteKind::ModelLoad => Stage::Models,
            NoteKind::TextureLoad => Stage::Textures,
            NoteKind::PipelineBuild => Stage::Pipelines,
            NoteKind::StyleBuild => Stage::Styles,
            NoteKind::ScriptLoad | NoteKind::ScriptReload => Stage::Scripts,
            NoteKind::Procgen => Stage::Procgen,
            NoteKind::AudioDecode | NoteKind::AudioLoad => Stage::Audio,
            NoteKind::SceneRead => Stage::SceneRead,
            NoteKind::SceneTeardown => Stage::Teardown,
            NoteKind::PhysicsBuild => Stage::Physics,
            NoteKind::AnimationLoad => Stage::Animation,
            NoteKind::ParticleLoad => Stage::Particles,
            NoteKind::ScriptInit => Stage::ScriptInit,
            NoteKind::SceneTransition
            | NoteKind::ChunkLoad
            | NoteKind::ReloadPoll
            | NoteKind::SurfaceReconfigure
            | NoteKind::PhysicsSteps
            | NoteKind::ClockStall
            | NoteKind::Save
            | NoteKind::Screenshot
            | NoteKind::StatsOverlay
            | NoteKind::TraceRing
            // An untimed count of GPU objects (#1091), not load work
            | NoteKind::GpuObjects => return None,
        })
    }

    fn index(self) -> usize {
        self as usize
    }
}

/// One stage's share of a load.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LoadRow {
    pub stage: &'static str,
    /// Its spans' time less the spans inside them.
    pub ms: f64,
    /// How many spans: models loaded, pipelines built.
    pub count: u32,
}

/// One named piece of load work, by its whole time.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LoadAsset {
    /// `model`, `texture`, `pipeline`...
    pub kind: &'static str,
    pub name: String,
    pub ms: f64,
}

/// Where one load's time went.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LoadReport {
    pub kind: LoadKind,
    /// The scene, chunk or file loaded.
    pub name: String,
    /// The load's measured time, begin to finish.
    pub total_ms: f64,
    /// Each stage that ran, longest first.
    pub stages: Vec<LoadRow>,
    /// What no span named: `total_ms` less every row.
    pub unnamed_ms: f64,
    /// The slowest [`SLOWEST`] named spans, each by its whole time (a model
    /// with its textures).
    pub slowest: Vec<LoadAsset>,
}

impl LoadReport {
    /// The rows' sum: `total_ms - unnamed_ms`.
    pub fn named_ms(&self) -> f64 {
        self.stages.iter().map(|r| r.ms).sum()
    }

    /// The row of `stage`, if it ran.
    pub fn row(&self, stage: Stage) -> Option<&LoadRow> {
        self.stages.iter().find(|r| r.stage == stage.label())
    }

    /// The table `--load-report` prints:
    ///
    /// ```text
    /// load (start) tavern.scene.toml: 1843.2 ms to the first frame
    ///   stage           ms   share  count
    ///   gpu device   612.0   33.2%      1
    ///   ...
    ///   not named     31.0    1.7%
    ///   slowest: model models/tavern.glb 211.0 ms
    /// ```
    pub fn table(&self) -> String {
        use std::fmt::Write;
        let until = match self.kind {
            LoadKind::Render => "to the first capture",
            _ => "to the first frame",
        };
        let mut out = format!(
            "load ({}) {}: {:.1} ms {until}\n",
            self.kind.label(),
            self.name,
            self.total_ms
        );
        let share = |ms: f64| {
            if self.total_ms > 0.0 {
                100.0 * ms / self.total_ms
            } else {
                0.0
            }
        };
        let width = self
            .stages
            .iter()
            .map(|r| r.stage.len())
            .chain(["not named".len(), "stage".len()])
            .max()
            .unwrap_or(5);
        let _ = writeln!(
            out,
            "  {:<width$} {:>9} {:>7} {:>6}",
            "stage", "ms", "share", "count"
        );
        for row in &self.stages {
            let _ = writeln!(
                out,
                "  {:<width$} {:>9.1} {:>6.1}% {:>6}",
                row.stage,
                row.ms,
                share(row.ms),
                row.count
            );
        }
        let _ = writeln!(
            out,
            "  {:<width$} {:>9.1} {:>6.1}%",
            "not named",
            self.unnamed_ms,
            share(self.unnamed_ms)
        );
        if self.slowest.is_empty() {
            out.push_str("  slowest: nothing named\n");
        }
        for (i, asset) in self.slowest.iter().enumerate() {
            let lead = if i == 0 { "slowest:" } else { "        " };
            let _ = writeln!(
                out,
                "  {lead} {:>8.1} ms  {} {}",
                asset.ms, asset.kind, asset.name
            );
        }
        out.pop();
        out
    }
}

struct Open {
    stage: Stage,
    detail: String,
    began: Instant,
    /// The whole time of the spans that closed inside this one.
    inner_ms: f64,
}

#[derive(Default, Clone, Copy)]
struct Tally {
    ms: f64,
    count: u32,
}

struct Recorder {
    generation: u64,
    kind: LoadKind,
    name: String,
    began: Instant,
    stack: Vec<Open>,
    rows: [Tally; Stage::ALL.len()],
    assets: Vec<LoadAsset>,
    /// Since when the first frame has run, and the whole time of the spans
    /// closed at the top since.
    first_frame: Option<(Instant, f64)>,
}

thread_local! {
    static RECORDING: Cell<bool> = const { Cell::new(false) };
    static GENERATION: Cell<u64> = const { Cell::new(0) };
    static RECORDER: RefCell<Option<Recorder>> = const { RefCell::new(None) };
}

/// Whether a load is being recorded on this thread.
#[inline]
pub fn recording() -> bool {
    RECORDING.with(Cell::get)
}

/// Start recording a load on this thread. A load already recorded is
/// finished first and returned, so its table can still be said.
pub fn begin(kind: LoadKind, name: impl Into<String>) -> Option<LoadReport> {
    let last = finish();
    let generation = GENERATION.with(|g| {
        g.set(g.get() + 1);
        g.get()
    });
    RECORDER.with(|r| {
        *r.borrow_mut() = Some(Recorder {
            generation,
            kind,
            name: name.into(),
            began: Instant::now(),
            stack: Vec::new(),
            rows: [Tally::default(); Stage::ALL.len()],
            assets: Vec::new(),
            first_frame: None,
        })
    });
    RECORDING.with(|r| r.set(true));
    last
}

/// [`begin`] with the start moved back to `began` (a process's start,
/// taken before anything was known to want a report).
pub fn begin_at(kind: LoadKind, name: impl Into<String>, began: Instant) -> Option<LoadReport> {
    let last = begin(kind, name);
    RECORDER.with(|r| {
        if let Some(rec) = r.borrow_mut().as_mut() {
            rec.began = began;
        }
    });
    last
}

/// Name the load being recorded (a start-up begins before its scene is
/// known).
pub fn set_name(name: impl Into<String>) {
    if !recording() {
        return;
    }
    RECORDER.with(|r| {
        if let Some(rec) = r.borrow_mut().as_mut() {
            rec.name = name.into();
        }
    });
}

/// The kind of load being recorded, if any.
pub fn current() -> Option<LoadKind> {
    if !recording() {
        return None;
    }
    RECORDER.with(|r| r.borrow().as_ref().map(|rec| rec.kind))
}

/// The bring-up is done: what runs from here to [`finish`] is the first
/// frame's, less the spans it makes.
pub fn bring_up_done() {
    if !recording() {
        return;
    }
    RECORDER.with(|r| {
        if let Some(rec) = r.borrow_mut().as_mut() {
            rec.first_frame.get_or_insert((Instant::now(), 0.0));
        }
    });
}

/// Whether the load being recorded has had its [`bring_up_done`].
pub fn waiting_for_first_frame() -> bool {
    recording()
        && RECORDER.with(|r| {
            r.borrow()
                .as_ref()
                .is_some_and(|rec| rec.first_frame.is_some())
        })
}

/// Stop recording and return the report. `None` when nothing was.
pub fn finish() -> Option<LoadReport> {
    if !recording() {
        return None;
    }
    RECORDING.with(|r| r.set(false));
    let rec = RECORDER.with(|r| r.borrow_mut().take())?;
    Some(rec.report())
}

/// Stop recording and forget it.
pub fn abandon() {
    let _ = finish();
}

impl Recorder {
    fn report(mut self) -> LoadReport {
        let now = Instant::now();
        let total_ms = ms_between(self.began, now);
        if let Some((from, inner)) = self.first_frame {
            let row = &mut self.rows[Stage::FirstFrame.index()];
            row.ms += (ms_between(from, now) - inner).max(0.0);
            row.count += 1;
        }
        let mut stages: Vec<LoadRow> = Stage::ALL
            .iter()
            .map(|&s| (s, self.rows[s.index()]))
            .filter(|(_, t)| t.count > 0)
            .map(|(s, t)| LoadRow {
                stage: s.label(),
                ms: round(t.ms),
                count: t.count,
            })
            .collect();
        stages.sort_by(|a, b| b.ms.total_cmp(&a.ms));
        let named: f64 = Stage::ALL.iter().map(|s| self.rows[s.index()].ms).sum();
        self.assets.sort_by(|a, b| b.ms.total_cmp(&a.ms));
        self.assets.truncate(SLOWEST);
        for a in &mut self.assets {
            a.ms = round(a.ms);
        }
        LoadReport {
            kind: self.kind,
            name: self.name,
            total_ms: round(total_ms),
            stages,
            unnamed_ms: round((total_ms - named).max(0.0)),
            slowest: self.assets,
        }
    }

    fn close(&mut self, open: Open, counted: bool) {
        let ms = open.began.elapsed().as_secs_f64() * 1000.0;
        if !counted {
            return;
        }
        let row = &mut self.rows[open.stage.index()];
        row.ms += (ms - open.inner_ms).max(0.0);
        row.count += 1;
        if !open.detail.is_empty() && self.assets.len() < MAX_ASSETS {
            self.assets.push(LoadAsset {
                kind: open.stage.asset_word(),
                name: open.detail,
                ms,
            });
        }
        match self.stack.last_mut() {
            Some(parent) => parent.inner_ms += ms,
            None => {
                if let Some((_, inner)) = self.first_frame.as_mut() {
                    *inner += ms;
                }
            }
        }
    }
}

fn ms_between(from: Instant, to: Instant) -> f64 {
    to.saturating_duration_since(from).as_secs_f64() * 1000.0
}

fn round(ms: f64) -> f64 {
    (ms * 100.0).round() / 100.0
}

/// A stage being timed until it drops; see [`span`].
#[must_use = "the stage is timed until this drops"]
pub struct Span(Option<Token>);

/// Where an open span sits: its load and its depth there.
pub(crate) struct Token {
    generation: u64,
    depth: usize,
}

/// Time `stage` until the guard drops, named `detail` when it is one
/// asset (empty for a stage of many). While no load is recorded it holds
/// nothing and the detail is never made.
#[inline]
pub fn span(stage: Stage, detail: impl FnOnce() -> String) -> Span {
    if !recording() {
        return Span(None);
    }
    Span(open(stage, detail()))
}

/// Time `work` as `stage`.
#[inline]
pub fn timed<R>(stage: Stage, detail: impl FnOnce() -> String, work: impl FnOnce() -> R) -> R {
    let _span = span(stage, detail);
    work()
}

impl Span {
    /// Close it without counting it (the work turned out to be nothing):
    /// its time stays with whatever it ran inside.
    pub fn discard(mut self) {
        if let Some(token) = self.0.take() {
            close(token, false);
        }
    }
}

impl Drop for Span {
    fn drop(&mut self) {
        if let Some(token) = self.0.take() {
            close(token, true);
        }
    }
}

/// Open a span of `stage` (from a frame note or [`span`]).
pub(crate) fn open(stage: Stage, detail: String) -> Option<Token> {
    RECORDER.with(|r| {
        let mut r = r.borrow_mut();
        let rec = r.as_mut()?;
        rec.stack.push(Open {
            stage,
            detail,
            began: Instant::now(),
            inner_ms: 0.0,
        });
        Some(Token {
            generation: rec.generation,
            depth: rec.stack.len() - 1,
        })
    })
}

/// Close the span `token` opened, and any left open inside it.
pub(crate) fn close(token: Token, counted: bool) {
    RECORDER.with(|r| {
        let mut r = r.borrow_mut();
        let Some(rec) = r.as_mut().filter(|rec| rec.generation == token.generation) else {
            return;
        };
        while rec.stack.len() > token.depth {
            let open = rec.stack.pop().expect("a span is open");
            let this = rec.stack.len() == token.depth;
            rec.close(open, counted || !this);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame_notes;
    use std::time::Duration;

    fn busy(ms: u64) {
        // A sleep can overshoot by a whole timer tick on Windows; the
        // tests compare shares, not exact times
        std::thread::sleep(Duration::from_millis(ms));
    }

    #[test]
    fn nothing_is_kept_while_no_load_is_recorded() {
        abandon();
        let _span = span(Stage::Physics, || unreachable!("no detail is made"));
        frame_notes::timed(NoteKind::ModelLoad, || unreachable!(), || ());
        assert!(finish().is_none());
    }

    #[test]
    fn each_stage_gets_its_own_time_less_the_spans_inside_it() {
        begin(LoadKind::Start, "a.scene.toml");
        frame_notes::timed(
            NoteKind::ModelLoad,
            || "models/crate.glb".into(),
            || {
                busy(10);
                frame_notes::timed(NoteKind::TextureLoad, || "crate.png".into(), || busy(20));
            },
        );
        timed(Stage::Physics, String::new, || busy(5));
        busy(5);
        let report = finish().expect("recorded");
        let models = report.row(Stage::Models).unwrap();
        let textures = report.row(Stage::Textures).unwrap();
        assert_eq!((models.count, textures.count), (1, 1));
        assert!(textures.ms >= 20.0, "{report:#?}");
        assert!(models.ms >= 10.0 && models.ms < textures.ms, "{report:#?}");
        assert!(report.row(Stage::Physics).unwrap().ms >= 5.0);
        // The rows and what nothing named add up to the total
        let sum = report.named_ms() + report.unnamed_ms;
        assert!((sum - report.total_ms).abs() < 0.05, "{report:#?}");
        assert!(report.unnamed_ms >= 5.0, "{report:#?}");
        // The model is named by its whole time, textures included; a stage
        // span with no name is no asset
        assert_eq!(report.slowest[0].name, "models/crate.glb");
        assert!(report.slowest[0].ms >= 30.0);
        assert_eq!(report.slowest.len(), 2);
        assert!(report.table().contains("slowest:"), "{}", report.table());
    }

    #[test]
    fn the_first_frame_is_what_runs_after_the_bring_up_less_what_it_loads() {
        begin(LoadKind::Transition, "b.scene.toml");
        busy(2);
        bring_up_done();
        assert!(waiting_for_first_frame());
        busy(10);
        frame_notes::timed(NoteKind::PipelineBuild, || "pbr".into(), || busy(15));
        let report = finish().unwrap();
        let first = report.row(Stage::FirstFrame).unwrap().ms;
        let pipelines = report.row(Stage::Pipelines).unwrap().ms;
        assert!(first >= 10.0 && first < pipelines, "{report:#?}");
        assert!(!recording());
    }

    #[test]
    fn a_container_note_is_not_a_stage_but_what_it_holds_is() {
        begin(LoadKind::Transition, "c.scene.toml");
        frame_notes::timed(
            NoteKind::SceneTransition,
            || "c.scene.toml".into(),
            || frame_notes::timed(NoteKind::ScriptInit, String::new, || busy(5)),
        );
        let report = finish().unwrap();
        assert_eq!(report.stages.len(), 1, "{report:#?}");
        assert_eq!(report.stages[0].stage, "on_init");
    }

    #[test]
    fn a_discarded_span_leaves_its_time_to_its_parent() {
        begin(LoadKind::Chunk, "cave");
        timed(Stage::Procgen, String::new, || {
            let nothing = span(Stage::Models, || "x.glb".into());
            busy(5);
            nothing.discard();
        });
        let report = finish().unwrap();
        assert!(report.row(Stage::Models).is_none());
        assert!(report.row(Stage::Procgen).unwrap().ms >= 5.0);
    }

    #[test]
    fn a_new_load_finishes_the_last_and_a_span_of_the_last_is_forgotten() {
        begin(LoadKind::Start, "a");
        let late = span(Stage::Audio, || "late.ogg".into());
        let last = begin(LoadKind::Transition, "b").expect("the start's report");
        assert_eq!(last.name, "a");
        drop(late);
        let report = finish().unwrap();
        assert_eq!(report.kind, LoadKind::Transition);
        assert!(report.stages.is_empty(), "{report:#?}");
    }

    #[test]
    fn every_stage_has_words() {
        let labels: std::collections::HashSet<_> = Stage::ALL.iter().map(|s| s.label()).collect();
        assert_eq!(labels.len(), Stage::ALL.len());
        for (i, s) in Stage::ALL.iter().enumerate() {
            assert_eq!(s.index(), i);
        }
    }
}
