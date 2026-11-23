//! Frame notes (#1077): what the engine was doing on a frame, said by the
//! code that did it, so a slow frame can name its cause. A model decoded
//! on first sight, a pipeline built on first draw, a scene transition, a
//! script hot reload: each leaves a [`FrameNote`] (its kind, what it was,
//! and how long it took where the code knows), and the player's frame
//! history files the notes of a frame on that frame's record.
//!
//! The channel is **per thread and write-only from below**. A crate under
//! the player (`flint-render`, `flint-script`, `flint-audio`) calls
//! [`note`] or [`timed`]; nothing below the player reads a note. The host
//! that wants them calls [`listen`]`(true)` on its frame thread and
//! [`take`]s them once a frame. On a thread no one listens on, a note is
//! one thread-local flag read and nothing else: no clock is read, no
//! detail formatted, nothing kept. A note from a worker thread is never
//! seen (the frame thread notes what it waits for), and each test thread
//! has a channel of its own.
//!
//! At most [`MAX_NOTES`] are kept between takes; the rest are counted
//! ([`take`] returns how many were dropped), so a host that stops taking
//! holds a bounded list, never a growing one.
//!
//! A load being recorded ([`crate::load_report`], #1092) hears every timed
//! note too, listened for or not: a note's time is also a load stage's.
//!
//! Notes nest. A [`timed`] note made inside another (a texture decoded
//! inside a model load, a pipeline inside a style build, a model load
//! inside a scene transition) is kept with its `depth`, and its time is
//! also inside the outer note's: sum only the notes at depth 0. Each is
//! kept in the order its work began, so the outer note comes first.

use std::cell::{Cell, RefCell};
use web_time::Instant;

/// How many notes are kept between takes; more are counted, not kept.
pub const MAX_NOTES: usize = 64;

/// What the engine was doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NoteKind {
    /// A glTF model read, decoded and uploaded (`detail`: its file).
    ModelLoad,
    /// A texture file decoded and uploaded (`detail`: its file).
    TextureLoad,
    /// A render or compute pipeline built (`detail`: its label).
    PipelineBuild,
    /// A post-process style's passes built (`detail`: the style).
    StyleBuild,
    /// A scene left and the next one entered (`detail`: the target).
    SceneTransition,
    /// A chunk's entities spawned into the scene (`detail`: the chunk).
    ChunkLoad,
    /// A script read and compiled (`detail`: its file).
    ScriptLoad,
    /// A changed script compiled and swapped in (`detail`: its file).
    ScriptReload,
    /// The hot-reload poll itself, when it took a while.
    ReloadPoll,
    /// Procedural assets generated (`detail`: how many, which).
    Procgen,
    /// Sound decoded on the frame (`detail`: the sounds).
    AudioDecode,
    /// A scene file read, parsed and settled: prefabs, schemas (#1092;
    /// `detail`: the scene).
    SceneRead,
    /// The last scene torn down at a transition (#1092).
    SceneTeardown,
    /// The physics world built from the scene: bodies, colliders (#1092).
    PhysicsBuild,
    /// A scene's sounds found and read, the preload decoded (#1092).
    AudioLoad,
    /// Animation clips, sprite clips and sequences loaded (#1092).
    AnimationLoad,
    /// Particle effects and their textures loaded (#1092).
    ParticleLoad,
    /// The scripts' `on_init` (and `on_scene_enter` at a transition)
    /// (#1092).
    ScriptInit,
    /// The window's surface configured again (`detail`: its size).
    SurfaceReconfigure,
    /// A frame that ran more than one fixed physics step.
    PhysicsSteps,
    /// The clock saw a stall long enough that pacing passed it through.
    ClockStall,
    /// The `persist_*` store written to its save file (#1119; `detail`:
    /// why: `debounce`, `persist_save`, `scene transition`, `exit`).
    Save,
    /// A screenshot read back and written (`--screenshot`; `detail`: its
    /// file). Tooling: see [`NoteKind::is_tooling`].
    Screenshot,
    /// The F2 stats overlay opened (`detail`: what it shows). Tooling.
    StatsOverlay,
    /// The scope recorder's ring grew to hold its look-back (#1288;
    /// `detail`: the thread and its new size). Tooling.
    TraceRing,
    /// The renderer created GPU objects on the frame (#1091; `detail`:
    /// what, `3 buffers, 2 bind groups`). A count, not a time: see
    /// [`NoteKind::is_count`].
    GpuObjects,
    /// A frame capture's writer thread was encoding and writing while the
    /// frame ran (#1178; `detail`: the folder). Untimed: the frame thread
    /// never waits on it, it only shares the machine with it. See
    /// [`NoteKind::is_background`].
    CaptureWrite,
}

impl NoteKind {
    /// Every kind, in the order above.
    pub const ALL: [NoteKind; 27] = [
        NoteKind::ModelLoad,
        NoteKind::TextureLoad,
        NoteKind::PipelineBuild,
        NoteKind::StyleBuild,
        NoteKind::SceneTransition,
        NoteKind::ChunkLoad,
        NoteKind::ScriptLoad,
        NoteKind::ScriptReload,
        NoteKind::ReloadPoll,
        NoteKind::Procgen,
        NoteKind::AudioDecode,
        NoteKind::SceneRead,
        NoteKind::SceneTeardown,
        NoteKind::PhysicsBuild,
        NoteKind::AudioLoad,
        NoteKind::AnimationLoad,
        NoteKind::ParticleLoad,
        NoteKind::ScriptInit,
        NoteKind::SurfaceReconfigure,
        NoteKind::PhysicsSteps,
        NoteKind::ClockStall,
        NoteKind::Save,
        NoteKind::Screenshot,
        NoteKind::StatsOverlay,
        NoteKind::TraceRing,
        NoteKind::GpuObjects,
        NoteKind::CaptureWrite,
    ];

    /// The words a log line or an overlay names it by.
    pub fn label(self) -> &'static str {
        match self {
            NoteKind::ModelLoad => "model load",
            NoteKind::TextureLoad => "texture load",
            NoteKind::PipelineBuild => "pipeline build",
            NoteKind::StyleBuild => "style build",
            NoteKind::SceneTransition => "scene transition",
            NoteKind::ChunkLoad => "chunk load",
            NoteKind::ScriptLoad => "script load",
            NoteKind::ScriptReload => "script reload",
            NoteKind::ReloadPoll => "reload poll",
            NoteKind::Procgen => "procgen",
            NoteKind::AudioDecode => "audio decode",
            NoteKind::SceneRead => "scene read",
            NoteKind::SceneTeardown => "scene teardown",
            NoteKind::PhysicsBuild => "physics build",
            NoteKind::AudioLoad => "audio load",
            NoteKind::AnimationLoad => "animation load",
            NoteKind::ParticleLoad => "particle load",
            NoteKind::ScriptInit => "on_init",
            NoteKind::SurfaceReconfigure => "surface reconfigure",
            NoteKind::PhysicsSteps => "physics steps",
            NoteKind::ClockStall => "clock stall",
            NoteKind::Save => "save",
            NoteKind::Screenshot => "screenshot",
            NoteKind::StatsOverlay => "stats overlay",
            NoteKind::TraceRing => "trace ring",
            NoteKind::GpuObjects => "gpu objects",
            NoteKind::CaptureWrite => "capture write",
        }
    }

    /// Whether the work ran on another thread beside the frame rather than
    /// on it (#1178: a capture's writer). The frame thread didn't wait on
    /// it, so it has no `ms`, and it slows a frame only by sharing the
    /// cores and the disk: a hitch names it as its likely cause unless a
    /// script took the time, whose own stall it then stands beside
    /// (`also: capture write`).
    pub fn is_background(self) -> bool {
        matches!(self, NoteKind::CaptureWrite)
    }

    /// Whether a note of this kind counts what the frame did rather than
    /// timing it (#1091: the GPU objects the renderer created). It has no
    /// `ms`, and unlike an untimed load it never explains a hitch on its
    /// own: a hitch names it beside whatever it does blame (`also: gpu
    /// objects`), since a frame that allocates is evidence, not a cost
    /// measured.
    pub fn is_count(self) -> bool {
        matches!(self, NoteKind::GpuObjects)
    }

    /// Whether the work was the player's own tools looking at the game (a
    /// screenshot written, the F2 overlay opening, the trace recorder's
    /// ring growing) rather than the game: a hitch they account for is the
    /// tools' cost, which the hitch detector classes `tooling`, counts
    /// apart and no gate counts (#1129).
    pub fn is_tooling(self) -> bool {
        matches!(
            self,
            NoteKind::Screenshot | NoteKind::StatsOverlay | NoteKind::TraceRing
        )
    }

    /// Whether a note of this kind tells what a long frame *was* rather
    /// than what made it long: a clock stall is the long frame itself, seen
    /// by the clock, and extra physics steps are the clock paying back the
    /// game time a long frame before owed (#1124: named for 410 ms render
    /// stalls). The steps' own cost is the physics phase's, which a hitch
    /// names by phase. A hitch's cause leaves these out.
    pub fn is_symptom(self) -> bool {
        matches!(self, NoteKind::ClockStall | NoteKind::PhysicsSteps)
    }
}

/// One thing the engine did on a frame.
#[derive(Debug, Clone, PartialEq)]
pub struct FrameNote {
    pub kind: NoteKind,
    /// What it was: a file, a label, a scene.
    pub detail: String,
    /// How long it took, where the code timed it.
    pub ms: Option<f32>,
    /// How many [`timed`] notes it was made inside (0: none).
    pub depth: u8,
}

impl FrameNote {
    pub fn new(kind: NoteKind, detail: impl Into<String>, ms: Option<f32>) -> Self {
        Self {
            kind,
            detail: detail.into(),
            ms,
            depth: 0,
        }
    }
}

impl std::fmt::Display for FrameNote {
    /// `model load models/crate.glb, 41.2 ms`
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.kind.label())?;
        if !self.detail.is_empty() {
            write!(f, " {}", self.detail)?;
        }
        if let Some(ms) = self.ms {
            write!(f, ", {ms:.1} ms")?;
        }
        Ok(())
    }
}

#[derive(Default)]
struct Channel {
    notes: Vec<FrameNote>,
    dropped: u32,
    /// Takes so far: a [`timed`] note whose slot was taken while its work
    /// ran (a host that takes inside it) is pushed again, not lost.
    takes: u64,
}

thread_local! {
    static LISTENING: Cell<bool> = const { Cell::new(false) };
    /// How many [`timed`] notes are under way on this thread.
    static DEPTH: Cell<u8> = const { Cell::new(0) };
    static CHANNEL: RefCell<Channel> = RefCell::new(Channel::default());
}

/// Keep this thread's notes from now on (`true`), or stop and forget what
/// was kept (`false`).
pub fn listen(on: bool) {
    if LISTENING.with(|l| l.replace(on)) && !on {
        CHANNEL.with(|c| {
            let mut c = c.borrow_mut();
            c.notes.clear();
            c.dropped = 0;
        });
    }
}

/// Whether anyone takes this thread's notes: a caller with a costly detail
/// to work out may ask first.
#[inline]
pub fn listening() -> bool {
    LISTENING.with(Cell::get)
}

/// Note `kind`, with the detail `detail` gives (called only while someone
/// listens) and its time where known.
#[inline]
pub fn note(kind: NoteKind, detail: impl FnOnce() -> String, ms: Option<f32>) {
    if !listening() {
        return;
    }
    let depth = DEPTH.with(Cell::get);
    push(FrameNote {
        kind,
        detail: detail(),
        ms,
        depth,
    });
}

/// Run `work` and note how long it took as `kind`. While no one listens it
/// is `work()` and a flag read.
#[inline]
pub fn timed<R>(kind: NoteKind, detail: impl FnOnce() -> String, work: impl FnOnce() -> R) -> R {
    let _timing = begin(kind, detail);
    work()
}

/// Start a note of `kind` that is timed until the [`Timing`] drops: for
/// work that can't be one closure (it `continue`s a loop, returns early).
/// While no one listens and no load is recorded it holds nothing and its
/// drop does nothing.
#[inline]
#[must_use = "the note is timed until this drops"]
pub fn begin(kind: NoteKind, detail: impl FnOnce() -> String) -> Timing {
    let listening = listening();
    let loading = crate::load_report::recording();
    if !listening && !loading {
        return Timing(None, None);
    }
    let detail = detail();
    let load = crate::load_report::Stage::of_note(kind)
        .filter(|_| loading)
        .and_then(|stage| crate::load_report::open(stage, detail.clone()));
    Timing(listening.then(|| Open::new(kind, detail)), load)
}

/// A note being timed; see [`begin`]: its frame note and its load span.
pub struct Timing(Option<Open>, Option<crate::load_report::Token>);

impl Drop for Timing {
    fn drop(&mut self) {
        // The frame note is filled in first, then the load's span closes
        drop(self.0.take());
        if let Some(token) = self.1.take() {
            crate::load_report::close(token, true);
        }
    }
}

/// Fills the note in when the work ends, a panic included.
struct Open {
    slot: Option<(u64, usize)>,
    began: Instant,
    note: Option<FrameNote>,
}

impl Open {
    fn new(kind: NoteKind, detail: String) -> Self {
        let depth = DEPTH.with(|d| {
            let depth = d.get();
            d.set(depth.saturating_add(1));
            depth
        });
        let note = FrameNote {
            kind,
            detail,
            ms: None,
            depth,
        };
        // The slot is taken now, so the outer note precedes the notes inside it
        let slot = CHANNEL.with(|c| {
            let mut c = c.borrow_mut();
            if c.notes.len() < MAX_NOTES {
                c.notes.push(note.clone());
                Some((c.takes, c.notes.len() - 1))
            } else {
                c.dropped = c.dropped.saturating_add(1);
                None
            }
        });
        Open {
            slot,
            began: Instant::now(),
            note: Some(note),
        }
    }
}

impl Drop for Open {
    fn drop(&mut self) {
        DEPTH.with(|d| d.set(d.get().saturating_sub(1)));
        let ms = Some(self.began.elapsed().as_secs_f32() * 1000.0);
        let Some(mut note) = self.note.take() else {
            return;
        };
        note.ms = ms;
        CHANNEL.with(|c| {
            let mut c = c.borrow_mut();
            match self.slot {
                // Its place is still there: fill it in
                Some((takes, i)) if takes == c.takes => c.notes[i].ms = ms,
                // Taken meanwhile: it belongs to the frame it ended in
                Some(_) => {
                    drop(c);
                    push(note);
                }
                // No room was left when it began
                None => {}
            }
        });
    }
}

fn push(note: FrameNote) {
    CHANNEL.with(|c| {
        let mut c = c.borrow_mut();
        if c.notes.len() < MAX_NOTES {
            c.notes.push(note);
        } else {
            c.dropped = c.dropped.saturating_add(1);
        }
    });
}

/// Move this thread's notes onto the end of `into`, oldest first, and
/// return how many were dropped since the last take for want of room.
pub fn take(into: &mut Vec<FrameNote>) -> u32 {
    CHANNEL.with(|c| {
        let mut c = c.borrow_mut();
        c.takes += 1;
        into.append(&mut c.notes);
        std::mem::take(&mut c.dropped)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn taken() -> (Vec<FrameNote>, u32) {
        let mut notes = Vec::new();
        let dropped = take(&mut notes);
        (notes, dropped)
    }

    #[test]
    fn nothing_is_kept_or_worked_out_while_no_one_listens() {
        listen(false);
        let mut asked = false;
        note(
            NoteKind::ModelLoad,
            || {
                asked = true;
                "x.glb".into()
            },
            None,
        );
        assert_eq!(timed(NoteKind::ScriptLoad, || unreachable!(), || 7), 7);
        assert!(!asked, "the detail is never made");
        assert_eq!(taken(), (Vec::new(), 0));
    }

    #[test]
    fn a_note_says_what_and_how_long() {
        listen(true);
        note(NoteKind::PhysicsSteps, || "3 steps".into(), None);
        let built = timed(
            NoteKind::PipelineBuild,
            || "pbr".into(),
            || {
                std::thread::sleep(std::time::Duration::from_millis(2));
                "made"
            },
        );
        assert_eq!(built, "made");
        let (notes, dropped) = taken();
        assert_eq!(dropped, 0);
        assert_eq!(notes.len(), 2);
        assert_eq!(notes[0].to_string(), "physics steps 3 steps");
        assert_eq!(notes[1].kind, NoteKind::PipelineBuild);
        assert!(notes[1].ms.unwrap() >= 2.0, "{:?}", notes[1]);
        assert!(notes[1].to_string().starts_with("pipeline build pbr, "));
        assert!(taken().0.is_empty(), "a take empties the channel");
        listen(false);
    }

    #[test]
    fn notes_nest_outer_first_and_each_knows_its_depth() {
        listen(true);
        timed(
            NoteKind::SceneTransition,
            || "b.scene.toml".into(),
            || {
                timed(
                    NoteKind::ModelLoad,
                    || "box.glb".into(),
                    || note(NoteKind::TextureLoad, || "box.png".into(), Some(1.0)),
                );
            },
        );
        let (notes, _) = taken();
        let shape: Vec<_> = notes.iter().map(|n| (n.kind, n.depth)).collect();
        assert_eq!(
            shape,
            [
                (NoteKind::SceneTransition, 0),
                (NoteKind::ModelLoad, 1),
                (NoteKind::TextureLoad, 2)
            ]
        );
        assert!(notes[0].ms.unwrap() >= notes[1].ms.unwrap());
        listen(false);
    }

    #[test]
    fn the_channel_is_bounded_and_counts_what_it_drops() {
        listen(true);
        for i in 0..MAX_NOTES + 5 {
            note(NoteKind::TextureLoad, || format!("{i}.png"), None);
        }
        // No room for a timed one either: counted, and its work still runs
        assert_eq!(timed(NoteKind::ModelLoad, || "late.glb".into(), || 1), 1);
        let (notes, dropped) = taken();
        assert_eq!(notes.len(), MAX_NOTES);
        assert_eq!(dropped, 6);
        assert_eq!(taken(), (Vec::new(), 0));
        listen(false);
    }

    #[test]
    fn a_note_taken_while_its_work_ran_lands_in_the_next_take() {
        listen(true);
        let (first, _) = timed(NoteKind::ChunkLoad, || "cave".into(), taken);
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].ms, None, "taken before it ended");
        let (second, _) = taken();
        assert_eq!(second.len(), 1);
        assert!(second[0].ms.is_some());
        listen(false);
    }

    #[test]
    fn stopping_forgets_what_was_kept() {
        listen(true);
        note(NoteKind::Procgen, || "1 asset".into(), None);
        listen(false);
        listen(true);
        assert!(taken().0.is_empty());
        listen(false);
    }

    #[test]
    fn every_kind_has_words() {
        let labels: std::collections::HashSet<_> =
            NoteKind::ALL.iter().map(|k| k.label()).collect();
        assert_eq!(labels.len(), NoteKind::ALL.len());
        assert!(NoteKind::ClockStall.is_symptom());
        assert!(!NoteKind::ModelLoad.is_symptom());
        let tooling: Vec<_> = NoteKind::ALL.iter().filter(|k| k.is_tooling()).collect();
        assert_eq!(
            tooling,
            [
                &NoteKind::Screenshot,
                &NoteKind::StatsOverlay,
                &NoteKind::TraceRing
            ]
        );
        assert!(tooling.iter().all(|k| !k.is_symptom()));
    }
}
