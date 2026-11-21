//! What a hitch is (#1076): one frame that stood on screen much longer than
//! the frames around it. The detector reads the frame history
//! (`frame_history.rs`) as each frame closes, judges it, says whose time it
//! was, and keeps counts over the session and over the frames the history
//! holds. Pure logic: the F2 graph (#1078), the hitch log and the exit gate
//! (#1079) are what show it.
//!
//! A frame is a hitch when its time is over **both** `factor` times the
//! median of the frames before it (default 2x) **and** that median plus
//! `floor_ms` (default 24 ms). The factor alone cries wolf at high frame
//! rates (a 4 ms frame among 2 ms ones), the floor alone at low ones.
//!
//! The floor is a time, not a count of refreshes (#1109), because what a
//! player sees is how long the picture held. On a 60 Hz display one missed
//! refresh (16.7 ms late) is no hitch and two (33.3 ms) are, and 24 ms sits
//! between them, so there the rule is what the factor made it. A faster
//! display needs the same delay, not the same count: at 240 Hz, three
//! refreshes are 12.5 ms against a 4.2 ms median, two missed refreshes of
//! 4.2 ms that nobody sees, and the shortest hitch is seven (29.2 ms). The
//! floor was 8 ms until then, and at 240 Hz half of what it counted was
//! frames under 16.7 ms (`docs/design/hitching-1080.md`).
//!
//! The time judged is how long the frame stood on screen: where the clock
//! paces to vsync, the paced time, so Fifo letting frames through 6, 6, 6,
//! then 45 ms apart on a hybrid-GPU laptop (`pacing.rs`, `perf.rs`) reads
//! a refresh a frame and is no hitch, while a real 100 ms stall paces to six
//! refreshes and is one. Unpaced, the wall time.
//!
//! A hitch is the player's when its own work (every phase but `present`)
//! grew by at least half of the frame's excess over the median; the worst
//! phase and script callback are named. Otherwise the time went elsewhere:
//! the compositor, the driver, the present wait, the OS.
//!
//! Before either, a hitch is **tooling** when the player's own tools noted
//! at least half its excess on the frame (#1129, `NoteKind::is_tooling`): a
//! screenshot read back and written, the F2 overlay opening. Those are
//! kept, logged and named like any hitch, but counted apart from the game's
//! (`HitchCounts::tooling`), so a tool never reports its own cost as the
//! game's and no gate fails a run for its screenshots.
//!
//! A hitch is **loading** when a scene was loaded behind a script's cover
//! on its frame or the frame before (#1125, `Frame::behind_cover`): the
//! Exiting fade ended with `complete_transition()`, so the load's long
//! frame stood under a black screen. It keeps its cause (the player's
//! work, mostly the transition's notes) and is logged as any hitch, but is
//! counted apart (`HitchCounts::loading`) and no gate counts it unless
//! asked (`--max-hitches-of loading`). A transition nothing covers goes
//! straight through and stays the game's: the player sees it.
//!
//! Whose time says nothing of why. A hitch also keeps the frame notes
//! (#1077, `flint_core::frame_notes`) of its frame and the frame before,
//! since GPU work lands a frame late: a model loaded, a pipeline built, a
//! transition. [`Hitch::likely_cause`] names them, or says
//! [`NO_ENGINE_EVENT`]. [`Hitch::cause_line`] names them as the cause only
//! when their times cover half the frame's excess, the rule a script is
//! blamed by; otherwise whose time it was comes first and the notes follow
//! as `(also: ..)` (#1124).

use super::frame_history::FrameHistory;
use super::perf::{script_name, Phase};
use flint_core::frame_notes::FrameNote;
use std::collections::VecDeque;

/// The frames before a frame it is judged against: two seconds at 60 fps.
pub(crate) const MEDIAN_FRAMES: usize = 120;
/// Frames the detector waits for before judging any: the first frames of a
/// run (pipelines compiling, the first uploads) have no steady to stand out
/// from.
pub(crate) const WARM_FRAMES: usize = 30;
/// How many hitches are kept to look back at; the counts go on past them.
pub(crate) const HITCHES_KEPT: usize = 256;
/// How many frames after a hitch its GPU time is still looked for on its
/// record (#1104): a read-back is a frame or two late, and the timer has
/// three in flight at most.
pub(crate) const GPU_LATE_FRAMES: u64 = 8;
/// The share of a hitch's excess the player's work must account for to be
/// the player's.
const PLAYER_SHARE: f32 = 0.5;

/// When a frame is a hitch; see the module docs.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct HitchThresholds {
    /// Over this many times the rolling median.
    pub factor: f32,
    /// And over the median by at least this, ms: a delay on screen, the
    /// same at every refresh rate.
    pub floor_ms: f32,
}

impl Default for HitchThresholds {
    fn default() -> Self {
        Self {
            factor: 2.0,
            floor_ms: 24.0,
        }
    }
}

impl HitchThresholds {
    /// The defaults, with `FLINT_HITCH_FACTOR` / `FLINT_HITCH_FLOOR_MS`
    /// over them. A value that isn't a number (or a factor under 1, a
    /// negative floor) is said once and the default kept (`flint_core::env`).
    pub(crate) fn from_env() -> Self {
        let d = Self::default();
        Self {
            factor: flint_core::env::HITCH_FACTOR.f32().unwrap_or(d.factor),
            floor_ms: flint_core::env::HITCH_FLOOR_MS.f32().unwrap_or(d.floor_ms),
        }
    }
}

/// Whose time a hitch was.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Cause {
    /// The player's own work accounts for the excess. `phase` grew most
    /// over its own median, by `phase_ms`; `script` is the callback that
    /// took longest in the frame, when scripts were timed.
    Player {
        phase: Phase,
        phase_ms: f32,
        script: Option<(String, f32)>,
    },
    /// The frame was long but the player's work was not: the compositor,
    /// the driver, the OS. `acquire_ms` is the wait for a swapchain image
    /// (#1123), `present_ms` the wait handing it back.
    Elsewhere { acquire_ms: f32, present_ms: f32 },
    /// The player's own tools looking at the game account for the excess
    /// (#1129): a screenshot read back and written, the F2 overlay's first
    /// layout. `tool_ms` is what the frame's tooling notes timed. It is
    /// the tools' cost, not the game's: counted apart, never gated.
    Tooling { tool_ms: f32 },
}

// Read by the F2 graph and the hitch log (#1078, #1079); tested here.
impl Cause {
    /// `player`, `elsewhere` or `tooling`, for a log line.
    pub(crate) fn label(&self) -> &'static str {
        match self {
            Cause::Player { .. } => "player",
            Cause::Elsewhere { .. } => "elsewhere",
            Cause::Tooling { .. } => "tooling",
        }
    }

    /// The longer of the swapchain's two waits in an `elsewhere` hitch,
    /// by name (`acquire`, `present`), and its ms; `None` for the
    /// player's own, or its tools'. Read by the cause line and the F2 graph.
    pub(crate) fn longer_wait(&self) -> Option<(&'static str, f32)> {
        match *self {
            Cause::Elsewhere {
                acquire_ms,
                present_ms,
            } if acquire_ms > present_ms => Some(("acquire", acquire_ms)),
            Cause::Elsewhere { present_ms, .. } => Some(("present", present_ms)),
            Cause::Player { .. } | Cause::Tooling { .. } => None,
        }
    }

    /// Whether the hitch was its tools' (#1129) rather than the game's
    /// (the player's work, or elsewhere).
    pub(crate) fn is_tooling(&self) -> bool {
        matches!(self, Cause::Tooling { .. })
    }
}

/// One hitch. Times are ms.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Hitch {
    /// The frame's number in the history ([`Frame::index`]).
    pub index: u64,
    /// When it began, seconds since the history started.
    pub at: f64,
    /// The game time it ran at ([`Frame::game_time`]).
    pub game_time: f64,
    /// Its wall time.
    pub ms: f32,
    /// The time it was judged by: paced where the clock paces, else wall.
    pub judged_ms: f32,
    /// The median of the frames before it.
    pub median_ms: f32,
    /// The player's work in it (every phase but `acquire` and `present`).
    pub player_ms: f32,
    /// The GPU's busy time on its own work ([`Frame::gpu_ms`]), so a reader
    /// can tell a wait on the GPU from one on the compositor; `None` while
    /// the GPU isn't timed. It comes back a frame or two after the hitch is
    /// judged, and [`HitchDetector::observe`] fills it in then (#1104).
    pub gpu_ms: Option<f32>,
    /// Under vsync, how many refreshes it stood for (1 is on time).
    pub refreshes: Option<u32>,
    pub cause: Cause,
    /// On a covered scene load's frame or the frame after (#1125): drawn
    /// under the script's fade, so counted apart as `loading`. Never set
    /// on a tooling hitch, which is the tools' first.
    pub loading: bool,
    /// What the engine did in this frame and the one before it (#1077),
    /// this frame's first: the GPU's share of a frame's work lands a
    /// frame late, so a pipeline built or a texture uploaded the frame
    /// before is as likely a cause.
    pub notes: Vec<HitchNote>,
}

/// One of a hitch's notes, and whose frame it came from.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct HitchNote {
    pub note: FrameNote,
    /// From the frame before the hitch, not the hitch's own.
    pub frame_before: bool,
}

/// What a hitch with no engine note says it was.
pub(crate) const NO_ENGINE_EVENT: &str = "no engine event";

/// How many top-level notes [`Hitch::likely_cause`] names before "N more".
const CAUSES_NAMED: usize = 3;

// Read by the hitch log, the report and the readouts (#1079)
impl Hitch {
    /// Whether the player saw it as the game's: not its tools' (#1129),
    /// not a load behind a cover (#1125). What the counts, rates, clean
    /// stretches and the gates' default classes hold.
    pub(crate) fn is_game(&self) -> bool {
        !self.cause.is_tooling() && !self.loading
    }

    /// `player`, `elsewhere`, `tooling` or `loading`: the class a log line
    /// and a tally file it under.
    pub(crate) fn class(&self) -> &'static str {
        if self.loading {
            "loading"
        } else {
            self.cause.label()
        }
    }

    /// The notes that say what the engine was doing: not a clock stall or
    /// the physics steps, which are the long frame itself
    /// ([`NoteKind::is_symptom`](flint_core::frame_notes::NoteKind::is_symptom)).
    pub(crate) fn engine_events(&self) -> impl Iterator<Item = &HitchNote> {
        self.notes.iter().filter(|n| !n.note.kind.is_symptom())
    }

    /// The outermost engine events (a transition, not each model it
    /// loaded), this frame's first, then by time, longest first.
    fn top_events(&self) -> Vec<&HitchNote> {
        let mut top: Vec<&HitchNote> = self.engine_events().filter(|n| n.note.depth == 0).collect();
        top.sort_by(|a, b| {
            a.frame_before.cmp(&b.frame_before).then(
                b.note
                    .ms
                    .unwrap_or(-1.0)
                    .total_cmp(&a.note.ms.unwrap_or(-1.0)),
            )
        });
        top
    }

    /// How far the frame ran past the median of the frames before it, ms.
    fn excess_ms(&self) -> f32 {
        (self.judged_ms - self.median_ms).max(0.0)
    }

    /// Whether the engine's events account for the hitch (#1124): together
    /// they took at least half of its excess, the share a script must take
    /// to be blamed. A 1.2 ms reload poll in a frame 10 ms over doesn't.
    /// A note that wasn't timed can't be weighed, so it is named, as before.
    fn events_explain(&self, top: &[&HitchNote]) -> bool {
        if top.is_empty() {
            return false;
        }
        // An untimed load explains the hitch; a count (#1091) never does
        if top
            .iter()
            .any(|n| n.note.ms.is_none() && !n.note.kind.is_count())
        {
            return true;
        }
        let ms: f32 = top.iter().filter_map(|n| n.note.ms).sum();
        ms >= self.excess_ms() * PLAYER_SHARE
    }

    /// The engine's events in a line, whether or not they account for the
    /// hitch ([`cause_line`](Self::cause_line) decides that): the
    /// outermost notes, this frame's first, then longest first; `no engine
    /// event` when there are none.
    /// `model load models/crate.glb, 41.2 ms; pipeline build pbr, 9.0 ms
    /// (frame before)`
    pub(crate) fn likely_cause(&self) -> String {
        let top = self.top_events();
        if top.is_empty() {
            return NO_ENGINE_EVENT.to_string();
        }
        named(&top, |n| {
            if n.frame_before {
                format!("{} (frame before)", n.note)
            } else {
                n.note.to_string()
            }
        })
    }

    /// What the hitch was, in a line: the engine's events when they
    /// account for it, else whose time it was (`script
    /// stall.rhai:on_update 80.1 ms`, `physics +41.0 ms`, `elsewhere,
    /// acquire 515.6 ms` or `elsewhere, present 30.2 ms`, the longer of the
    /// swapchain's waits, #1123) with the events that didn't after it: `sync +9.1
    /// ms (also: reload poll 1.2 ms)` (#1079, #1124: the exit report,
    /// `--perf`'s line, `?perf=1`, the hitch log).
    pub(crate) fn cause_line(&self) -> String {
        // The tools' own cost names the tool alone: `screenshot
        // shots/look_0003.png, 92.5 ms` (#1129)
        if self.cause.is_tooling() {
            return self.tooling_line();
        }
        let top = self.top_events();
        if self.events_explain(&top) {
            return self.likely_cause();
        }
        let mut line = self.whose_line();
        if !top.is_empty() {
            // Short: the kind and its time, not the file or the label
            let also = named(&top, |n| {
                let mut s = n.note.kind.label().to_string();
                if let Some(ms) = n.note.ms {
                    s.push_str(&format!(" {ms:.1} ms"));
                }
                if n.frame_before {
                    s.push_str(" the frame before");
                }
                s
            });
            line.push_str(&format!(" (also: {also})"));
        }
        line
    }

    /// Whose time the hitch was, in a line: the blamed script, else the
    /// phase that grew most, else `elsewhere` and the longer of the
    /// swapchain's waits (#1123).
    fn whose_line(&self) -> String {
        if let Some((name, ms)) = self.blamed_script() {
            return format!("script {name} {ms:.1} ms");
        }
        match &self.cause {
            Cause::Player {
                phase, phase_ms, ..
            } => format!("{} +{phase_ms:.1} ms", phase.label()),
            Cause::Elsewhere { .. } => {
                let (wait, ms) = self.cause.longer_wait().unwrap_or(("present", 0.0));
                format!("elsewhere, {wait} {ms:.1} ms")
            }
            // `cause_line` names a tooling hitch's notes before asking this
            Cause::Tooling { tool_ms } => format!("tooling {tool_ms:.1} ms"),
        }
    }

    /// The tools' notes on the hitch's own frame, as its cause: `screenshot
    /// shots/look_0003.png, 92.5 ms`, `stats overlay opened, 104.0 ms`.
    fn tooling_notes(&self) -> impl Iterator<Item = &FrameNote> {
        self.notes
            .iter()
            .filter(|n| !n.frame_before && n.note.depth == 0 && n.note.kind.is_tooling())
            .map(|n| &n.note)
    }

    fn tooling_line(&self) -> String {
        let line = self
            .tooling_notes()
            .map(|n| n.to_string())
            .collect::<Vec<_>>()
            .join("; ");
        if line.is_empty() {
            "tooling".to_string()
        } else {
            line
        }
    }

    /// The script callback to blame: the frame's longest, when it took at
    /// least half of what its phase grew by. A render stall in a frame
    /// whose longest callback took 0.3 ms is the render's, not the script's.
    fn blamed_script(&self) -> Option<(&str, f32)> {
        match &self.cause {
            Cause::Player {
                phase_ms,
                script: Some((name, ms)),
                ..
            } if *ms >= phase_ms * PLAYER_SHARE => Some((name, *ms)),
            _ => None,
        }
    }

    /// The cause with no times in it, so hitches of one cause count
    /// together, by [`cause_line`](Self::cause_line)'s rule: the kind of
    /// the engine's first event (`model load`) when the events account for
    /// the hitch, else the script (`script stall.rhai:on_update`), the
    /// phase (`physics`) or `elsewhere`.
    pub(crate) fn cause_key(&self) -> String {
        if self.cause.is_tooling() {
            return self
                .tooling_notes()
                .next()
                .map_or("tooling", |n| n.kind.label())
                .to_string();
        }
        let top = self.top_events();
        if self.events_explain(&top) {
            return top[0].note.kind.label().to_string();
        }
        if let Some((name, _)) = self.blamed_script() {
            return format!("script {name}");
        }
        match &self.cause {
            Cause::Player { phase, .. } => phase.label().to_string(),
            Cause::Elsewhere { .. } => "elsewhere".to_string(),
            Cause::Tooling { .. } => "tooling".to_string(),
        }
    }
}

/// The first [`CAUSES_NAMED`] notes, each as `name` says, then "N more".
fn named(top: &[&HitchNote], name: impl Fn(&HitchNote) -> String) -> String {
    let mut line = top
        .iter()
        .take(CAUSES_NAMED)
        .map(|n| name(n))
        .collect::<Vec<_>>()
        .join("; ");
    if top.len() > CAUSES_NAMED {
        line.push_str(&format!("; {} more", top.len() - CAUSES_NAMED));
    }
    line
}

/// The counts over a span of frames.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub(crate) struct HitchCounts {
    /// Frames judged.
    pub frames: u64,
    /// The wall time they cover, seconds.
    pub secs: f64,
    /// The game's hitches: the player's and elsewhere, not the tools' nor
    /// a covered load's.
    pub hitches: u64,
    /// Hitches the player's own tools made ([`Cause::Tooling`]): counted
    /// here, and in nothing else (rates, clean stretches, since last).
    pub tooling: u64,
    /// Hitches of a scene loaded behind a script's cover ([`Hitch::loading`],
    /// #1125): counted here, and in nothing else, as the tools' are.
    pub loading: u64,
    /// The longest judged frame's wall time, ms.
    pub worst_ms: f32,
    /// Seconds from the end of the last hitch to the end of the newest
    /// frame; `None` with no hitch in the span.
    pub since_last_secs: Option<f64>,
    /// The longest stretch with no hitch, seconds.
    pub longest_clean_secs: f64,
    /// Under vsync, refreshes the frames ran past their one.
    pub missed_refreshes: u64,
}

#[cfg_attr(not(any(test, feature = "debug-hud")), allow(dead_code))]
impl HitchCounts {
    /// Hitches a minute over the span.
    pub(crate) fn per_minute(&self) -> f64 {
        if self.secs <= 0.0 {
            return 0.0;
        }
        self.hitches as f64 * 60.0 / self.secs
    }
}

/// Watches the frame history; see the module docs.
pub(crate) struct HitchDetector {
    pub thresholds: HitchThresholds,
    /// The next frame index to judge, and the first that was judged
    /// rather than passed over while warming up.
    next: u64,
    first_judged: Option<u64>,
    hitches: VecDeque<Hitch>,
    session: HitchCounts,
    /// Where the session's current stretch with no hitch began, and where
    /// the newest frame judged ended, seconds.
    clean_from: Option<f64>,
    judged_to: f64,
    /// Room for a median, kept so judging allocates nothing.
    scratch: Vec<f32>,
}

impl Default for HitchDetector {
    fn default() -> Self {
        Self::new(HitchThresholds::default())
    }
}

impl HitchDetector {
    pub(crate) fn new(thresholds: HitchThresholds) -> Self {
        Self {
            thresholds,
            next: 0,
            first_judged: None,
            hitches: VecDeque::with_capacity(HITCHES_KEPT),
            session: HitchCounts::default(),
            clean_from: None,
            judged_to: 0.0,
            scratch: Vec::with_capacity(MEDIAN_FRAMES),
        }
    }

    /// Judge the frames the history closed since the last look (once a
    /// frame, one frame: after the clock has ticked, so it has its paced
    /// time). Returns how many of them were hitches; the newest is
    /// [`last`](Self::last).
    pub(crate) fn observe(&mut self, history: &FrameHistory) -> usize {
        let frames = history.frames();
        let Some(newest) = frames.back() else {
            return 0;
        };
        // A hitch is judged before its GPU time is back: picked up from its
        // record once it is (#1104)
        for h in self
            .hitches
            .iter_mut()
            .rev()
            .take_while(|h| newest.index.saturating_sub(h.index) <= GPU_LATE_FRAMES)
        {
            if h.gpu_ms.is_none() {
                h.gpu_ms = history.frame(h.index).and_then(|f| f.gpu_ms);
            }
        }
        if newest.index < self.next {
            return 0;
        }
        // Frames are numbered without gaps, so the first unjudged one's
        // place is its distance from the newest (or the oldest kept, when
        // more closed than the ring holds).
        let unjudged = (newest.index - self.next + 1) as usize;
        let first = frames.len().saturating_sub(unjudged);
        let mut found = 0;
        for at in first..frames.len() {
            if self.judge(history, at) {
                found += 1;
            }
        }
        self.next = newest.index + 1;
        found
    }

    /// Judge the frame at `at` in the history's ring.
    fn judge(&mut self, history: &FrameHistory, at: usize) -> bool {
        let frames = history.frames();
        let frame = &frames[at];
        let before = at.saturating_sub(MEDIAN_FRAMES)..at;
        // A frame a capture was recorded or read back in (#1086) isn't
        // judged: the tool that looks for hitches mustn't report its own
        if before.len() < WARM_FRAMES || frame.capturing {
            return false;
        }
        let median = self.median(before.clone().map(|i| frames[i].judged_ms()));
        let judged = frame.judged_ms();
        let refreshes = frame
            .clock
            .and_then(|c| c.vsync_ms)
            .filter(|&v| v > 0.0)
            .map(|v| (judged / v).round().max(1.0) as u32);

        self.first_judged.get_or_insert(frame.index);
        let s = &mut self.session;
        s.frames += 1;
        s.secs += frame.ms as f64 / 1000.0;
        s.worst_ms = s.worst_ms.max(frame.ms);
        s.missed_refreshes += refreshes.map_or(0, |r| r as u64 - 1);
        let clean_from = *self.clean_from.get_or_insert(frame.at);
        self.judged_to = frame.end();

        let t = self.thresholds;
        if !(judged > median * t.factor && judged > median + t.floor_ms) {
            return false;
        }
        let cause = self.cause(history, at, before, judged - median);
        // A load behind a cover lands on its frame, and what its first
        // frames cost (the GPU's share) on the next, still under the fade
        let covered =
            frame.behind_cover || at.checked_sub(1).is_some_and(|b| frames[b].behind_cover);
        let loading = covered && !cause.is_tooling();
        let notes = at
            .checked_sub(1)
            .map(|b| (&frames[b], true))
            .into_iter()
            .chain([(frame, false)])
            .rev()
            .flat_map(|(f, frame_before)| {
                f.notes.iter().map(move |note| HitchNote {
                    note: note.clone(),
                    frame_before,
                })
            })
            .collect();
        let hitch = Hitch {
            notes,
            index: frame.index,
            at: frame.at,
            game_time: frame.game_time,
            ms: frame.ms,
            judged_ms: judged,
            median_ms: median,
            player_ms: frame.player_ms(),
            gpu_ms: frame.gpu_ms,
            refreshes,
            cause,
            loading,
        };
        let s = &mut self.session;
        if hitch.cause.is_tooling() {
            // The tools' cost: kept for the log and the graph's line, but
            // the game's stretch with no hitch goes on through it
            s.tooling += 1;
        } else if hitch.loading {
            // Under the script's fade: the same, the player saw none of it
            s.loading += 1;
        } else {
            s.hitches += 1;
            s.longest_clean_secs = s.longest_clean_secs.max(hitch.at - clean_from);
            self.clean_from = Some(frame.end());
        }
        if self.hitches.len() == HITCHES_KEPT {
            self.hitches.pop_front();
        }
        self.hitches.push_back(hitch);
        true
    }

    /// Whose time the frame at `at` was, `excess` ms over the median of
    /// the frames `before` it. Only for a hitch, so it may take its time.
    fn cause(
        &mut self,
        history: &FrameHistory,
        at: usize,
        before: std::ops::Range<usize>,
        excess: f32,
    ) -> Cause {
        let frames = history.frames();
        let frame = &frames[at];
        // The player's own tools first (#1129): a screenshot's read-back
        // lands in `present` and would read as elsewhere, the F2 overlay's
        // first layout in `hud`. Only the frame's own notes: the tools
        // wait for their work, so none of it lands a frame late.
        let tool_ms: f32 = frame
            .notes
            .iter()
            .filter(|n| n.depth == 0 && n.kind.is_tooling())
            .filter_map(|n| n.ms)
            .sum();
        if tool_ms >= excess * PLAYER_SHARE {
            return Cause::Tooling { tool_ms };
        }
        let typical = self.median(before.clone().map(|i| frames[i].player_ms()));
        if frame.player_ms() - typical < excess * PLAYER_SHARE {
            return Cause::Elsewhere {
                acquire_ms: frame.phases[Phase::Acquire as usize],
                present_ms: frame.phases[Phase::Present as usize],
            };
        }
        let mut worst = (Phase::Input, f32::MIN);
        for phase in Phase::ALL.into_iter().filter(|p| !p.is_wait()) {
            let i = phase as usize;
            let grew = frame.phases[i] - self.median(before.clone().map(|j| frames[j].phases[i]));
            if grew > worst.1 {
                worst = (phase, grew);
            }
        }
        let script = frame
            .scripts
            .iter()
            .enumerate()
            .filter(|(_, &ms)| ms > 0.0)
            .max_by(|a, b| a.1.total_cmp(b.1))
            .and_then(|(i, &ms)| {
                let name = history.script_names().get(i)?;
                Some((script_name(name), ms))
            });
        Cause::Player {
            phase: worst.0,
            phase_ms: worst.1,
            script,
        }
    }

    /// The median of `values`, in the detector's own room.
    fn median(&mut self, values: impl Iterator<Item = f32>) -> f32 {
        self.scratch.clear();
        self.scratch.extend(values);
        if self.scratch.is_empty() {
            return 0.0;
        }
        let mid = self.scratch.len() / 2;
        *self.scratch.select_nth_unstable_by(mid, f32::total_cmp).1
    }

    /// The hitches kept, oldest first (the last [`HITCHES_KEPT`]).
    pub(crate) fn hitches(&self) -> &VecDeque<Hitch> {
        &self.hitches
    }

    /// The newest hitch.
    #[cfg_attr(not(any(test, feature = "debug-hud")), allow(dead_code))]
    pub(crate) fn last(&self) -> Option<&Hitch> {
        self.hitches.back()
    }

    /// The counts since the run began.
    pub(crate) fn session(&self) -> HitchCounts {
        let mut c = self.session;
        let clean_from = self.clean_from.unwrap_or(self.judged_to);
        c.longest_clean_secs = c.longest_clean_secs.max(self.judged_to - clean_from);
        c.since_last_secs = self
            .last_of_game()
            .map(|h| self.judged_to - (h.at + h.ms as f64 / 1000.0));
        c
    }

    /// The newest hitch that was the game's, not its tools' nor a covered
    /// load's.
    pub(crate) fn last_of_game(&self) -> Option<&Hitch> {
        self.hitches.iter().rev().find(|h| h.is_game())
    }

    /// The counts over the frames `history` holds now (about the last ten
    /// seconds), for a graph or a readout to show beside the session's.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn window(&self, history: &FrameHistory) -> HitchCounts {
        let mut c = HitchCounts::default();
        let Some(first) = self.first_judged else {
            return c;
        };
        // Only what was judged: not the warm-up, nor a frame not yet looked at.
        let judged = history
            .frames()
            .iter()
            .filter(|f| f.index >= first && f.index < self.next);
        let mut from = None;
        let mut to = 0.0;
        for f in judged {
            from.get_or_insert(f.at);
            to = f.end();
            c.frames += 1;
            c.secs += f.ms as f64 / 1000.0;
            c.worst_ms = c.worst_ms.max(f.ms);
            if let Some(v) = f.clock.and_then(|k| k.vsync_ms).filter(|&v| v > 0.0) {
                c.missed_refreshes += ((f.judged_ms() / v).round().max(1.0) as u64) - 1;
            }
        }
        let Some(from) = from else {
            return c;
        };
        let mut clean_from = from;
        for h in self.hitches.iter().filter(|h| h.at >= from) {
            if h.cause.is_tooling() {
                c.tooling += 1;
                continue;
            }
            if h.loading {
                c.loading += 1;
                continue;
            }
            c.hitches += 1;
            c.longest_clean_secs = c.longest_clean_secs.max(h.at - clean_from);
            clean_from = h.at + h.ms as f64 / 1000.0;
        }
        c.longest_clean_secs = c.longest_clean_secs.max(to - clean_from);
        if c.hitches > 0 {
            c.since_last_secs = Some(to - clean_from);
        }
        c
    }
}

#[cfg(test)]
mod tests {
    use super::super::frame_history::{ClockSample, HISTORY_FRAMES};
    use super::*;
    use std::time::Duration;
    use web_time::Instant;

    const HZ60_MS: f64 = 1000.0 / 60.0;
    /// Three quick frames, then a long one: a 60 Hz mean.
    const BURST: [f64; 4] = [6.0, 6.0, 6.0, 4.0 * HZ60_MS - 18.0];

    fn ms(n: f64) -> Duration {
        Duration::from_secs_f64(n / 1000.0)
    }

    /// Plays frames into a history and the detector the way the player
    /// does: each frame begins, marks its phases, and the clock's tick at
    /// the next frame's start measures it.
    struct Run {
        history: FrameHistory,
        detector: HitchDetector,
        t: Instant,
        /// Vsync pacing: the refresh period, ms.
        paced: Option<f64>,
        debt: f64,
    }

    /// One frame: its wall time, and how long its phases took.
    #[derive(Clone, Copy, Default)]
    struct F {
        wall: f64,
        input: f64,
        scripts: f64,
        physics: f64,
        sync: f64,
        render: f64,
        present: f64,
        /// Fixed physics steps run (0: none said).
        steps: u32,
    }

    fn steady(wall: f64) -> F {
        F {
            wall,
            input: 0.3,
            scripts: 1.0,
            render: 2.0,
            present: 0.2,
            ..F::default()
        }
    }

    impl Run {
        fn new(paced: Option<f64>) -> Self {
            let mut history = FrameHistory::default();
            let t = history.start;
            history.begin_frame_at(t);
            Self {
                history,
                detector: HitchDetector::default(),
                t,
                paced,
                debt: 0.0,
            }
        }

        /// The frame, then the next frame's start and tick, which close
        /// and measure it, then the detector's look.
        fn frame(&mut self, f: F) -> usize {
            self.frame_scripts(f, &[])
        }

        fn frame_scripts(&mut self, f: F, scripts: &[(&str, f32)]) -> usize {
            let h = &mut self.history;
            let mut mark = self.t;
            for (phase, d) in [
                (Phase::Input, f.input),
                (Phase::Scripts, f.scripts),
                (Phase::Physics, f.physics),
                (Phase::Sync, f.sync),
                (Phase::Render, f.render),
                (Phase::Present, f.present),
            ] {
                mark += ms(d);
                h.mark_at(phase, mark);
            }
            let times: Vec<(String, f32)> =
                scripts.iter().map(|(n, ms)| (n.to_string(), *ms)).collect();
            h.record_scripts(&times);
            h.fixed_steps(f.steps);
            self.t += ms(f.wall);
            // The next frame's start closes this one; its tick measures it.
            h.begin_frame_at(self.t);
            let paced_ms = self.paced.map(|period| {
                // GameClock::pace, as far as these runs need it: whole
                // refreshes that keep game time on the wall clock.
                self.debt += f.wall;
                let n = (self.debt / period).round().max(1.0);
                self.debt = (self.debt - n * period).max(-100.0);
                (n * period) as f32
            });
            h.clock_ticked(ClockSample {
                raw_ms: f.wall as f32,
                dt_ms: paced_ms.unwrap_or(f.wall as f32),
                paced_ms,
                vsync_ms: self.paced.map(|p| p as f32),
            });
            self.detector.observe(&self.history)
        }

        fn frames(&mut self, n: usize, f: F) -> usize {
            (0..n).map(|_| self.frame(f)).sum()
        }
    }

    #[test]
    fn steady_frames_have_no_hitches() {
        let mut run = Run::new(None);
        assert_eq!(run.frames(600, steady(HZ60_MS)), 0);
        let s = run.detector.session();
        assert_eq!(s.hitches, 0);
        assert_eq!(s.frames as usize, 600 - WARM_FRAMES);
        assert!((s.worst_ms - HZ60_MS as f32).abs() < 0.01);
        assert_eq!(s.since_last_secs, None);
        assert!((s.longest_clean_secs - s.secs).abs() < 0.05, "{s:?}");
        assert_eq!(run.detector.window(&run.history).hitches, 0);
    }

    #[test]
    fn a_long_frame_with_little_work_was_time_spent_elsewhere() {
        let mut run = Run::new(None);
        run.frames(120, steady(HZ60_MS));
        assert_eq!(run.frame(steady(100.0)), 1);
        run.frames(60, steady(HZ60_MS));
        let hitches = run.detector.hitches();
        assert_eq!(hitches.len(), 1);
        let h = &hitches[0];
        assert!((h.ms - 100.0).abs() < 0.01);
        assert!((h.median_ms - HZ60_MS as f32).abs() < 0.01);
        assert_eq!(h.refreshes, None, "no vsync, no refreshes");
        assert!(
            matches!(h.cause, Cause::Elsewhere { present_ms, .. } if (present_ms - 0.2).abs() < 0.01),
            "{:?}",
            h.cause
        );
        let s = run.detector.session();
        assert_eq!(s.hitches, 1);
        assert!((s.worst_ms - 100.0).abs() < 0.01);
        // A second of clean frames since, and the longer stretch before it.
        assert!((s.since_last_secs.unwrap() - 1.0).abs() < 0.01, "{s:?}");
        // 90 judged frames before it (the first 30 warm up): 1.5 s.
        assert!((s.longest_clean_secs - 1.5).abs() < 0.02, "{s:?}");
        let w = run.detector.window(&run.history);
        assert_eq!(w.hitches, 1);
        assert!((w.since_last_secs.unwrap() - 1.0).abs() < 0.01, "{w:?}");
    }

    #[test]
    fn a_long_frame_a_script_took_is_the_players_and_names_it() {
        let mut run = Run::new(None);
        for _ in 0..120 {
            run.frame_scripts(
                steady(HZ60_MS),
                &[
                    ("scripts/weather.rhai:on_update", 0.6),
                    ("scripts/whale.rhai:on_update", 0.4),
                ],
            );
        }
        let slow = F {
            scripts: 81.0,
            ..steady(100.0)
        };
        let found = run.frame_scripts(
            slow,
            &[
                ("scripts/weather.rhai:on_update", 0.6),
                ("scripts/whale.rhai:on_update", 80.0),
            ],
        );
        assert_eq!(found, 1);
        let h = run.detector.last().unwrap();
        match &h.cause {
            Cause::Player {
                phase,
                phase_ms,
                script,
            } => {
                assert_eq!(*phase, Phase::Scripts);
                assert!((phase_ms - 80.0).abs() < 0.01, "{phase_ms}");
                let (name, ms) = script.as_ref().unwrap();
                assert_eq!(name, "whale.rhai:on_update");
                assert!((ms - 80.0).abs() < 0.01);
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(h.cause.label(), "player");
    }

    /// #1079, found on a real run: a frame whose render grew 21 ms, and
    /// whose longest script callback took 0.3 ms, is the render's.
    #[test]
    fn a_render_stall_is_not_blamed_on_a_quick_script() {
        let mut run = Run::new(None);
        let quick = [("scripts/hud.rhai:on_update", 0.3)];
        for _ in 0..120 {
            run.frame_scripts(steady(HZ60_MS), &quick);
        }
        let slow = F {
            render: 40.0,
            ..steady(60.0)
        };
        assert_eq!(run.frame_scripts(slow, &quick), 1);
        let h = run.detector.last().unwrap();
        assert_eq!(h.cause_line(), "render +38.0 ms");
        assert_eq!(h.cause_key(), "render");
        // The script that did take the time is still named
        let slow = F {
            scripts: 41.0,
            ..steady(60.0)
        };
        run.frames(60, steady(HZ60_MS));
        assert_eq!(
            run.frame_scripts(slow, &[("scripts/whale.rhai:on_update", 40.0)]),
            1
        );
        let h = run.detector.last().unwrap();
        assert_eq!(h.cause_line(), "script whale.rhai:on_update 40.0 ms");
        assert_eq!(h.cause_key(), "script whale.rhai:on_update");
    }

    /// Fifo on a hybrid-GPU laptop lets frames through 6, 6, 6, then about
    /// 45 ms apart (48.7 here, so the mean is 60 Hz, as `clock.rs`'s
    /// `bursts`). Paced, each stood a refresh on screen: no hitch storm, no
    /// missed refreshes. A real stall among them still is one.
    #[test]
    fn vsync_bursts_under_pacing_are_not_a_hitch_storm() {
        let mut run = Run::new(Some(HZ60_MS));
        for _ in 0..150 {
            for wall in BURST {
                assert_eq!(run.frame(steady(wall)), 0, "{wall} ms");
            }
        }
        let s = run.detector.session();
        assert_eq!(s.hitches, 0);
        assert_eq!(s.missed_refreshes, 0);
        // The wall clock still knows how long the long ones were.
        assert!((s.worst_ms - BURST[3] as f32).abs() < 0.01);

        assert_eq!(run.frame(steady(100.0)), 1);
        let h = run.detector.last().unwrap();
        assert!((h.judged_ms - 100.0).abs() < 0.5, "{h:?}");
        assert_eq!(h.refreshes, Some(6));
        assert_eq!(run.detector.session().missed_refreshes, 5);
    }

    /// The same bursts judged on the wall clock would be a hitch every
    /// fourth frame: why pacing's time is the one judged.
    #[test]
    fn unpaced_the_same_bursts_read_as_hitches() {
        let mut run = Run::new(None);
        let mut found = 0;
        for _ in 0..150 {
            for wall in BURST {
                found += run.frame(steady(wall));
            }
        }
        assert!(found > 100, "{found}");
    }

    #[test]
    fn a_frame_two_refreshes_long_is_a_missed_refresh_not_a_hitch() {
        let mut run = Run::new(Some(HZ60_MS));
        run.frames(120, steady(HZ60_MS));
        assert_eq!(run.frame(steady(2.0 * HZ60_MS)), 0);
        assert_eq!(run.detector.session().missed_refreshes, 1);
        assert_eq!(run.detector.window(&run.history).missed_refreshes, 1);
    }

    #[test]
    fn both_the_factor_and_the_floor_must_be_passed() {
        // 2 ms frames: a 20 ms one is 10x, but only 18 ms over: no hitch.
        let mut run = Run::new(None);
        let quick = F {
            wall: 2.0,
            ..F::default()
        };
        run.frames(120, quick);
        assert_eq!(
            run.frame(F {
                wall: 20.0,
                ..quick
            }),
            0
        );
        assert_eq!(
            run.frame(F {
                wall: 27.0,
                ..quick
            }),
            1
        );
        // 50 ms frames: 80 is 30 over but not 2x.
        let mut run = Run::new(None);
        run.frames(120, steady(50.0));
        assert_eq!(run.frame(steady(80.0)), 0);
        assert_eq!(run.frame(steady(101.0)), 1);
    }

    const HZ240_MS: f64 = 1000.0 / 240.0;

    /// #1109: a 240 Hz panel under a busy machine. Every so often a frame
    /// stands three refreshes (12.5 ms), now and then four or five (16.7,
    /// 20.8): a delay of two to four refreshes of 4.2 ms that no one sees,
    /// and no hitch. The rule with the old 8 ms floor counted every one.
    /// A 100 ms stall among them still is one, and says so in refreshes.
    #[test]
    fn at_240_hz_three_refresh_frames_are_not_hitches() {
        let late = [3.0, 3.0, 4.0, 3.0, 5.0];
        let series = |run: &mut Run| {
            let mut found = 0;
            for i in 0..2400 {
                // One late frame in about every 23, the others on time
                let wall = if i % 23 == 11 {
                    late[(i / 23) % late.len()] * HZ240_MS
                } else {
                    HZ240_MS
                };
                found += run.frame(steady(wall));
            }
            found
        };
        let mut run = Run::new(Some(HZ240_MS));
        assert_eq!(series(&mut run), 0);
        let s = run.detector.session();
        assert_eq!(s.hitches, 0);
        // The late frames are still counted, as the refreshes they missed
        assert!(s.missed_refreshes > 200, "{s:?}");

        assert_eq!(run.frame(steady(100.0)), 1);
        let h = run.detector.last().unwrap();
        assert_eq!(h.refreshes, Some(24));
        assert!((h.median_ms - HZ240_MS as f32).abs() < 0.01, "{h:?}");

        // What #1109 saw: the same series judged with the 8 ms floor
        let mut old = Run::new(Some(HZ240_MS));
        old.detector.thresholds.floor_ms = 8.0;
        assert!(series(&mut old) > 80);
    }

    /// The floor is a delay, so the shortest hitch is about the same length
    /// on every display, and on a 60 Hz one it is what it always was: two
    /// missed refreshes, where the factor puts it.
    #[test]
    fn the_shortest_hitch_is_about_as_long_at_every_refresh_rate() {
        let shortest = |hz: f64| {
            let period = 1000.0 / hz;
            (2..40)
                .find(|&n| {
                    let mut run = Run::new(Some(period));
                    run.frames(120, steady(period));
                    run.frame(steady(n as f64 * period)) == 1
                })
                .map(|n| (n, n as f64 * period))
                .unwrap()
        };
        assert_eq!(shortest(60.0).0, 3, "two missed refreshes at 60 Hz");
        for hz in [60.0, 75.0, 90.0, 120.0, 144.0, 165.0, 240.0, 360.0] {
            let (n, ms) = shortest(hz);
            assert!(
                (27.0..=50.0).contains(&ms),
                "{hz} Hz: the shortest hitch is {n} refreshes, {ms:.1} ms"
            );
        }
        assert_eq!(shortest(240.0).0, 7);
    }

    #[test]
    fn the_warm_up_is_not_judged() {
        let mut run = Run::new(None);
        // A 2 s first frame (pipelines compiling) is no hitch: nothing
        // came before it to stand out from.
        assert_eq!(run.frame(steady(2000.0)), 0);
        run.frames(200, steady(HZ60_MS));
        let s = run.detector.session();
        assert_eq!(s.hitches, 0);
        assert!(s.worst_ms < 17.0, "{s:?}");
    }

    #[test]
    fn counts_go_on_past_the_hitches_kept() {
        let mut run = Run::new(None);
        run.frames(120, steady(HZ60_MS));
        for _ in 0..HITCHES_KEPT + 10 {
            run.frames(3, steady(HZ60_MS));
            run.frame(steady(100.0));
        }
        assert_eq!(run.detector.hitches().len(), HITCHES_KEPT);
        let s = run.detector.session();
        assert_eq!(s.hitches as usize, HITCHES_KEPT + 10);
        assert!(s.per_minute() > 100.0, "{}", s.per_minute());
        // The window holds the history's frames, so fewer of them.
        let w = run.detector.window(&run.history);
        assert!(w.hitches > 0 && w.hitches < s.hitches, "{w:?}");
        assert!(w.secs < s.secs);
    }

    /// What the always-on recording costs a frame: the player's own calls
    /// in its order (a begin, nine marks, the present, steps, GPU, the
    /// clock, the detector's look), on the real clock. `--ignored`, and in
    /// a release build for the figure the CLAUDE.md quotes:
    /// `cargo test -p flint-player --release --lib always_on_cost -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn always_on_cost() {
        let mut history = FrameHistory::default();
        let mut detector = HitchDetector::default();
        let sample = ClockSample {
            raw_ms: 16.7,
            dt_ms: 16.7,
            paced_ms: Some(16.7),
            vsync_ms: Some(16.7),
        };
        let frame = |history: &mut FrameHistory, detector: &mut HitchDetector| {
            history.begin_frame();
            history.clock_ticked(sample);
            detector.observe(history);
            for phase in Phase::ALL {
                if phase == Phase::Present {
                    history.presented_at(Instant::now());
                }
                history.mark(phase);
            }
            history.fixed_steps(1);
            // The read-back of two frames ago, onto its record
            let late = history.under_way_index().checked_sub(2);
            history.gpu_time(late, 1.5);
        };
        // Warm: the ring full and the detector judging.
        for _ in 0..2 * HISTORY_FRAMES {
            frame(&mut history, &mut detector);
        }
        let frames = 200_000;
        let began = Instant::now();
        for _ in 0..frames {
            frame(&mut history, &mut detector);
        }
        let us = began.elapsed().as_secs_f64() * 1e6 / frames as f64;
        // Off (a build without `frame-history`, or FLINT_FRAME_HISTORY=0):
        // the same calls through the recorder, each a branch.
        let mut off = super::super::perf::PerfRecorder::recording_always(false);
        let began = Instant::now();
        for _ in 0..frames {
            off.begin_frame();
            if off.recording() {
                off.clock_ticked(sample);
                detector.observe(&off);
            }
            for phase in Phase::ALL {
                if phase == Phase::Present {
                    off.presented_at(Instant::now());
                }
                off.mark(phase);
            }
            off.fixed_steps(1);
            off.gpu_time(off.gpu_frame_index(), 1.5);
            std::hint::black_box(&mut off);
        }
        let off_us = began.elapsed().as_secs_f64() * 1e6 / frames as f64;
        assert!(off.frames().is_empty());
        let marks = Instant::now();
        for _ in 0..frames {
            std::hint::black_box(Instant::now());
        }
        let now_us = marks.elapsed().as_secs_f64() * 1e6 / frames as f64;
        // The frame notes' share (#1077): the frame's take of an empty
        // channel, and a note made where no one listens (every call site's
        // cost in a build that records nothing)
        let mut taken = Vec::new();
        flint_core::frame_notes::listen(true);
        let takes = Instant::now();
        for _ in 0..frames {
            std::hint::black_box(flint_core::frame_notes::take(&mut taken));
        }
        let take_ns = takes.elapsed().as_secs_f64() * 1e9 / frames as f64;
        flint_core::frame_notes::listen(false);
        let unheard = Instant::now();
        for _ in 0..frames {
            let _note = flint_core::frame_notes::begin(
                flint_core::frame_notes::NoteKind::ModelLoad,
                || unreachable!(),
            );
            std::hint::black_box(&_note);
        }
        let unheard_ns = unheard.elapsed().as_secs_f64() * 1e9 / frames as f64;
        println!(
            "always-on recording: {us:.2} us a frame ({:.4} ms); one Instant::now {:.0} ns; off: {:.0} ns a frame (the present's own Instant::now included); notes: a take {take_ns:.1} ns, a note no one hears {unheard_ns:.1} ns",
            us / 1000.0,
            now_us * 1000.0,
            off_us * 1000.0
        );
        assert!(history.frames().len() == HISTORY_FRAMES);
    }

    /// #1077: a hitch carries its frame's notes and the frame before's, and
    /// names the outermost as its likely cause.
    #[test]
    fn a_hitch_names_what_the_engine_did_in_it_and_the_frame_before() {
        use flint_core::frame_notes::{self, NoteKind};
        frame_notes::listen(true);
        let mut run = Run::new(None);
        run.frames(120, steady(HZ60_MS));
        // The frame before: a pipeline built, its GPU work landing late
        frame_notes::note(NoteKind::PipelineBuild, || "bloom".into(), Some(6.0));
        run.frame(steady(HZ60_MS));
        // The hitch: a transition that loaded a model inside it
        frame_notes::timed(
            NoteKind::SceneTransition,
            || "b.scene.toml".into(),
            || frame_notes::note(NoteKind::ModelLoad, || "models/box.glb".into(), Some(40.0)),
        );
        assert_eq!(run.frame(steady(100.0)), 1);
        let h = run.detector.last().unwrap();
        let kinds: Vec<_> = h
            .notes
            .iter()
            .map(|n| (n.note.kind, n.frame_before))
            .collect();
        assert_eq!(
            kinds,
            [
                (NoteKind::SceneTransition, false),
                (NoteKind::ModelLoad, false),
                (NoteKind::PipelineBuild, true)
            ]
        );
        let cause = h.likely_cause();
        assert!(
            cause.starts_with("scene transition b.scene.toml, "),
            "{cause}"
        );
        assert!(
            cause.ends_with("; pipeline build bloom, 6.0 ms (frame before)"),
            "{cause}"
        );
        assert!(!cause.contains("box.glb"), "inside the transition: {cause}");
        frame_notes::listen(false);
    }

    #[test]
    fn a_hitch_with_no_notes_says_no_engine_event() {
        use flint_core::frame_notes::{self, NoteKind};
        frame_notes::listen(true);
        let mut run = Run::new(None);
        run.frames(120, steady(HZ60_MS));
        assert_eq!(run.frame(steady(100.0)), 1);
        assert!(run.detector.last().unwrap().notes.is_empty());
        assert_eq!(run.detector.last().unwrap().likely_cause(), NO_ENGINE_EVENT);

        // A stall the clock passed through is the long frame itself, not
        // what the engine was doing: kept, but no cause
        run.frames(60, steady(HZ60_MS));
        assert_eq!(run.frame(steady(400.0)), 1);
        let h = run.detector.last().unwrap();
        assert_eq!(h.notes.len(), 1);
        assert_eq!(h.notes[0].note.kind, NoteKind::ClockStall);
        assert_eq!(h.likely_cause(), NO_ENGINE_EVENT);
        frame_notes::listen(false);
    }

    #[test]
    fn a_frame_of_two_physics_steps_says_so() {
        use flint_core::frame_notes::NoteKind;
        let mut h = FrameHistory::new(4);
        let t = h.start;
        h.begin_frame_at(t);
        h.fixed_steps(1);
        h.begin_frame_at(t + ms(16.0));
        assert!(h.newest().unwrap().notes.is_empty(), "one step is no news");
        h.fixed_steps(2);
        h.begin_frame_at(t + ms(32.0));
        let notes = &h.newest().unwrap().notes;
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].kind, NoteKind::PhysicsSteps);
        assert_eq!(notes[0].to_string(), "physics steps 2 fixed steps");
    }

    /// #1124, from #1080's log: debug Woozy Grove at 36 fps, a 446 ms frame
    /// whose render waited 410.7 ms on the swapchain. The frame before ran
    /// two fixed steps, as every frame at 36 fps does, and the clock saw
    /// the stall: both are the long frame, not its cause.
    /// `{"frame":43,"ms":445.97,"median_ms":33.33,"phase":"render",
    /// "phase_ms":410.74,"cause":"physics steps 2 fixed steps (frame
    /// before)"}`
    #[test]
    fn a_render_stall_after_two_physics_steps_is_the_renders() {
        use flint_core::frame_notes::NoteKind;
        let mut run = Run::new(None);
        let grove = F {
            wall: 33.33,
            input: 0.3,
            scripts: 1.0,
            physics: 2.0,
            sync: 2.0,
            render: 25.0,
            present: 2.0,
            steps: 2,
        };
        run.frames(120, grove);
        let stall = F {
            wall: 445.97,
            render: 25.0 + 410.74,
            steps: 1,
            ..grove
        };
        assert_eq!(run.frame(stall), 1);
        let h = run.detector.last().unwrap();
        let kinds: Vec<_> = h
            .notes
            .iter()
            .map(|n| (n.note.kind, n.frame_before))
            .collect();
        assert_eq!(
            kinds,
            [
                (NoteKind::ClockStall, false),
                (NoteKind::PhysicsSteps, true)
            ],
            "both kept on the hitch"
        );
        assert_eq!(h.cause_line(), "render +410.7 ms");
        assert_eq!(h.cause_key(), "render");
        assert_eq!(h.likely_cause(), NO_ENGINE_EVENT);
    }

    /// #1124, from #1080's log: release Woozy Grove, a 14.9 ms frame among
    /// 4.17 ms ones whose sync grew 9.1 ms, with a 1.2 ms reload poll in
    /// it. The poll is a seventh of the excess: named after, not as, the
    /// cause.
    /// `{"frame":645,"ms":14.9,"median_ms":4.17,"phase":"sync",
    /// "phase_ms":9.07,"cause":"reload poll 0 scripts watched, 1.2 ms"}`
    #[test]
    fn a_sync_stall_is_not_blamed_on_a_quick_reload_poll() {
        use flint_core::frame_notes::{self, NoteKind};
        frame_notes::listen(true);
        let mut run = Run::new(None);
        // The line was logged on the 8 ms floor; at 24 (#1109) a frame
        // 10.7 ms over is no hitch. The cause line is what's tested here.
        run.detector.thresholds.floor_ms = 8.0;
        let grove = F {
            wall: 4.17,
            input: 0.1,
            scripts: 0.3,
            physics: 0.2,
            sync: 0.5,
            render: 1.5,
            present: 1.0,
            steps: 1,
        };
        run.frames(120, grove);
        frame_notes::note(
            NoteKind::ReloadPoll,
            || "0 scripts watched".into(),
            Some(1.25),
        );
        let slow = F {
            wall: 14.9,
            sync: 0.5 + 9.07,
            ..grove
        };
        assert_eq!(run.frame(slow), 1);
        let h = run.detector.last().unwrap();
        assert_eq!(h.cause_line(), "sync +9.1 ms (also: reload poll 1.2 ms)");
        assert_eq!(h.cause_key(), "sync");
        // The notes in a line are still there for whoever asks
        assert_eq!(h.likely_cause(), "reload poll 0 scripts watched, 1.2 ms");
        frame_notes::listen(false);
    }

    /// A note that does account for its hitch is still the cause: a model
    /// load of 70 ms in a frame 83 ms over, a transition of 90 ms. A note
    /// of the frame before that doesn't is named after, marked so.
    #[test]
    fn a_frame_that_made_gpu_objects_is_named_beside_the_cause_never_as_it() {
        // #1091: an untimed load is named as the cause, but a count of GPU
        // objects is evidence, not a measured cost
        use flint_core::frame_notes::{self, NoteKind};
        frame_notes::listen(true);
        let mut run = Run::new(None);
        run.frames(120, steady(HZ60_MS));
        frame_notes::note(NoteKind::GpuObjects, || "40 bind groups".into(), None);
        let stall = F {
            render: 62.0,
            ..steady(HZ60_MS + 60.0)
        };
        assert_eq!(run.frame(stall), 1);
        let h = run.detector.last().unwrap();
        assert_eq!(h.cause_line(), "render +60.0 ms (also: gpu objects)");
        assert_eq!(h.cause_key(), "render");
        assert_eq!(h.likely_cause(), "gpu objects 40 bind groups");
        frame_notes::listen(false);
    }

    #[test]
    fn a_note_that_covers_half_the_hitch_is_its_cause() {
        use flint_core::frame_notes::{self, NoteKind};
        frame_notes::listen(true);
        let mut run = Run::new(None);
        run.frames(120, steady(HZ60_MS));
        frame_notes::note(
            NoteKind::ModelLoad,
            || "models/crate.glb".into(),
            Some(70.0),
        );
        let load = F {
            sync: 70.0,
            ..steady(100.0)
        };
        assert_eq!(run.frame(load), 1);
        let h = run.detector.last().unwrap();
        assert_eq!(h.cause_line(), "model load models/crate.glb, 70.0 ms");
        assert_eq!(h.cause_key(), "model load");

        // Several notes count together: two pipelines of 25 ms in a 60 ms
        // render stall
        run.frames(60, steady(HZ60_MS));
        for label in ["bloom", "ssao"] {
            frame_notes::note(NoteKind::PipelineBuild, || label.into(), Some(25.0));
        }
        let build = F {
            render: 62.0,
            ..steady(HZ60_MS + 60.0)
        };
        assert_eq!(run.frame(build), 1);
        let h = run.detector.last().unwrap();
        assert_eq!(
            h.cause_line(),
            "pipeline build bloom, 25.0 ms; pipeline build ssao, 25.0 ms"
        );

        // A 3 ms pipeline the frame before, under a 40 ms render stall
        run.frames(60, steady(HZ60_MS));
        frame_notes::note(NoteKind::PipelineBuild, || "pbr".into(), Some(3.0));
        run.frame(steady(HZ60_MS));
        let stall = F {
            render: 42.0,
            ..steady(60.0)
        };
        assert_eq!(run.frame(stall), 1);
        let h = run.detector.last().unwrap();
        assert_eq!(
            h.cause_line(),
            "render +40.0 ms (also: pipeline build 3.0 ms the frame before)"
        );
        assert_eq!(h.cause_key(), "render");
        frame_notes::listen(false);
    }

    #[test]
    fn the_tables_defaults_are_the_thresholds() {
        // `flint env` says what an unset switch means; it must be true
        let d = HitchThresholds::default();
        assert_eq!(flint_core::env::HITCH_FACTOR.default, d.factor.to_string());
        assert_eq!(
            flint_core::env::HITCH_FLOOR_MS.default,
            d.floor_ms.to_string()
        );
        assert!(flint_core::env::HITCH_FACTOR.parse("0.5").is_err());
        assert!(flint_core::env::HITCH_FLOOR_MS.parse("-1").is_err());
    }

    /// A frame a capture was recorded or read back in (#1086) is passed
    /// over: the tool that looks for hitches doesn't report its own.
    #[test]
    fn a_frame_a_capture_was_in_is_not_judged() {
        let mut run = Run::new(None);
        for _ in 0..60 {
            run.frame(steady(HZ60_MS));
        }
        let slow = F {
            render: 190.0,
            ..steady(200.0)
        };
        run.history.capturing();
        assert_eq!(run.frame(slow), 0, "the capture's own frame");
        assert_eq!(run.frame(slow), 1, "the same frame uncaptured is a hitch");
        assert_eq!(run.detector.session().hitches, 1);
    }

    /// #1125: a scene loaded behind a script's fade is judged and kept as
    /// a hitch (its cause still the player's), but counted as `loading`,
    /// and so is the frame after it; the next stall is the game's again.
    #[test]
    fn a_load_behind_a_cover_is_loading_and_the_frame_after_it_too() {
        let mut run = Run::new(None);
        for _ in 0..60 {
            run.frame(steady(HZ60_MS));
        }
        let load = F {
            input: 150.0,
            ..steady(160.0)
        };
        run.history.loading_behind_cover();
        assert_eq!(run.frame(load), 1);
        let h = run.detector.last().unwrap().clone();
        assert!(h.loading);
        assert!(!h.is_game());
        assert_eq!(h.class(), "loading");
        assert_eq!(h.cause.label(), "player", "whose work it was is kept");
        // Its first frame of drawing, still under the fade
        let first_draw = F {
            render: 90.0,
            ..steady(100.0)
        };
        assert_eq!(run.frame(first_draw), 1);
        assert!(run.detector.last().unwrap().loading);
        for _ in 0..30 {
            run.frame(steady(HZ60_MS));
        }
        // The same stall with no cover: the player sees it
        assert_eq!(run.frame(load), 1);
        let h = run.detector.last().unwrap();
        assert!(!h.loading && h.is_game());
        assert_eq!(h.class(), "player");

        let s = run.detector.session();
        assert_eq!((s.hitches, s.loading, s.tooling), (1, 2, 0));
        let w = run.detector.window(&run.history);
        assert_eq!((w.hitches, w.loading), (1, 2));
        assert_eq!(
            run.detector.last_of_game().map(|h| h.index),
            run.detector.last().map(|h| h.index)
        );
    }

    /// A screenshot's read-back lands in `present`, so its frame would read
    /// as time spent elsewhere (#1129): its note makes it the tools'.
    fn screenshot_frame() -> F {
        F {
            present: 90.0,
            ..steady(100.0)
        }
    }

    #[test]
    fn a_screenshot_hitch_is_the_tools_and_names_its_file() {
        use flint_core::frame_notes::{self, NoteKind};
        frame_notes::listen(true);
        let mut run = Run::new(None);
        run.frames(120, steady(HZ60_MS));
        frame_notes::note(
            NoteKind::Screenshot,
            || "shots/look_0003.png".into(),
            Some(88.0),
        );
        assert_eq!(run.frame(screenshot_frame()), 1, "still a long frame");
        let h = run.detector.last().unwrap().clone();
        assert_eq!(h.cause, Cause::Tooling { tool_ms: 88.0 });
        assert_eq!(h.cause.label(), "tooling");
        assert_eq!(h.cause_line(), "screenshot shots/look_0003.png, 88.0 ms");
        assert_eq!(h.cause_key(), "screenshot");

        // Counted apart: the game's count, rate and clean stretch go on
        run.frames(60, steady(HZ60_MS));
        let s = run.detector.session();
        assert_eq!((s.hitches, s.tooling), (0, 1));
        assert_eq!(s.since_last_secs, None, "no hitch of the game's yet");
        assert!((s.longest_clean_secs - s.secs).abs() < 0.05, "{s:?}");
        let w = run.detector.window(&run.history);
        assert_eq!((w.hitches, w.tooling), (0, 1));
        assert!(run.detector.last_of_game().is_none());

        // The same frame with no note is a hitch elsewhere, the game's
        assert_eq!(run.frame(screenshot_frame()), 1);
        let h = run.detector.last().unwrap();
        assert!(matches!(h.cause, Cause::Elsewhere { .. }), "{:?}", h.cause);
        assert_eq!(run.detector.session().hitches, 1);
        frame_notes::listen(false);
    }

    #[test]
    fn a_small_tool_note_on_a_real_stall_leaves_the_stall_the_games() {
        use flint_core::frame_notes::{self, NoteKind};
        frame_notes::listen(true);
        let mut run = Run::new(None);
        run.frames(120, steady(HZ60_MS));
        // A 2 ms screenshot in a frame a script held for 150 ms
        frame_notes::note(NoteKind::Screenshot, || "shot.png".into(), Some(2.0));
        let stall = F {
            scripts: 150.0,
            ..steady(160.0)
        };
        assert_eq!(run.frame(stall), 1);
        let h = run.detector.last().unwrap();
        assert!(
            matches!(
                h.cause,
                Cause::Player {
                    phase: Phase::Scripts,
                    ..
                }
            ),
            "{:?}",
            h.cause
        );
        assert_eq!(run.detector.session().hitches, 1);
        // The F2 overlay's first frame, timed as the HUD pass it was
        run.frames(60, steady(HZ60_MS));
        frame_notes::note(NoteKind::StatsOverlay, || "opened".into(), Some(95.0));
        let hud = F {
            render: 97.0,
            ..steady(100.0)
        };
        assert_eq!(run.frame(hud), 1);
        let h = run.detector.last().unwrap();
        assert_eq!(h.cause_line(), "stats overlay opened, 95.0 ms");
        assert_eq!(run.detector.session().hitches, 1, "not the game's");
        assert_eq!(run.detector.session().tooling, 1);
        frame_notes::listen(false);
    }
}
