//! What a hitch is (#1076): one frame that stood on screen much longer than
//! the frames around it. The detector reads the frame history
//! (`frame_history.rs`) as each frame closes, judges it, says whose time it
//! was, and keeps counts over the session and over the frames the history
//! holds. Pure logic: the F2 graph (#1078), the hitch log and the exit gate
//! (#1079) are what show it.
//!
//! A frame is a hitch when its time is over **both** `factor` times the
//! median of the frames before it (default 2x) **and** that median plus
//! `floor_ms` (default 8 ms). The factor alone cries wolf at high frame
//! rates (a 4 ms frame among 2 ms ones), the floor alone at low ones.
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

use super::frame_history::FrameHistory;
use super::perf::{script_name, Phase};
use std::collections::VecDeque;

/// The frames before a frame it is judged against: two seconds at 60 fps.
pub(crate) const MEDIAN_FRAMES: usize = 120;
/// Frames the detector waits for before judging any: the first frames of a
/// run (pipelines compiling, the first uploads) have no steady to stand out
/// from.
pub(crate) const WARM_FRAMES: usize = 30;
/// How many hitches are kept to look back at; the counts go on past them.
pub(crate) const HITCHES_KEPT: usize = 256;
/// The share of a hitch's excess the player's work must account for to be
/// the player's.
const PLAYER_SHARE: f32 = 0.5;

/// When a frame is a hitch; see the module docs.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct HitchThresholds {
    /// Over this many times the rolling median.
    pub factor: f32,
    /// And over the median by at least this, ms.
    pub floor_ms: f32,
}

impl Default for HitchThresholds {
    fn default() -> Self {
        Self {
            factor: 2.0,
            floor_ms: 8.0,
        }
    }
}

impl HitchThresholds {
    /// The defaults, with `FLINT_HITCH_FACTOR` / `FLINT_HITCH_FLOOR_MS`
    /// over them. A value that isn't a number (or a factor under 1, a
    /// negative floor) is said once and the default kept.
    pub(crate) fn from_env() -> Self {
        let var = |name: &str| std::env::var(name).ok();
        let (thresholds, warnings) =
            Self::parse(var("FLINT_HITCH_FACTOR"), var("FLINT_HITCH_FLOOR_MS"));
        for warning in warnings {
            tracing::warn!("{warning}");
        }
        thresholds
    }

    fn parse(factor: Option<String>, floor: Option<String>) -> (Self, Vec<String>) {
        let mut t = Self::default();
        let mut warnings = Vec::new();
        let mut read = |name: &str, value: Option<String>, min: f32, into: &mut f32| {
            let Some(value) = value else { return };
            match value.trim().parse::<f32>() {
                Ok(v) if v.is_finite() && v >= min => *into = v,
                _ => warnings.push(format!(
                    "{name}={value:?} is not a number of at least {min}: keeping {into}"
                )),
            }
        };
        read("FLINT_HITCH_FACTOR", factor, 1.0, &mut t.factor);
        read("FLINT_HITCH_FLOOR_MS", floor, 0.0, &mut t.floor_ms);
        (t, warnings)
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
    /// the driver, the OS. `present_ms` is the present phase's share, the
    /// wait for the swapchain.
    Elsewhere { present_ms: f32 },
}

// Read by the F2 graph and the hitch log (#1078, #1079); tested here.
#[cfg_attr(not(test), allow(dead_code))]
impl Cause {
    /// `player` or `elsewhere`, for a log line.
    pub(crate) fn label(&self) -> &'static str {
        match self {
            Cause::Player { .. } => "player",
            Cause::Elsewhere { .. } => "elsewhere",
        }
    }
}

/// One hitch. Times are ms.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Hitch {
    /// The frame's number in the history ([`Frame::index`]).
    pub index: u64,
    /// When it began, seconds since the history started.
    pub at: f64,
    /// Its wall time.
    pub ms: f32,
    /// The time it was judged by: paced where the clock paces, else wall.
    pub judged_ms: f32,
    /// The median of the frames before it.
    pub median_ms: f32,
    /// The player's work in it (every phase but `present`).
    pub player_ms: f32,
    /// Under vsync, how many refreshes it stood for (1 is on time).
    pub refreshes: Option<u32>,
    pub cause: Cause,
}

/// The counts over a span of frames.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub(crate) struct HitchCounts {
    /// Frames judged.
    pub frames: u64,
    /// The wall time they cover, seconds.
    pub secs: f64,
    pub hitches: u64,
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

#[cfg_attr(not(test), allow(dead_code))]
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
        if before.len() < WARM_FRAMES {
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
        let hitch = Hitch {
            index: frame.index,
            at: frame.at,
            ms: frame.ms,
            judged_ms: judged,
            median_ms: median,
            player_ms: frame.player_ms(),
            refreshes,
            cause,
        };
        let s = &mut self.session;
        s.hitches += 1;
        s.longest_clean_secs = s.longest_clean_secs.max(hitch.at - clean_from);
        self.clean_from = Some(frame.end());
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
        let typical = self.median(before.clone().map(|i| frames[i].player_ms()));
        if frame.player_ms() - typical < excess * PLAYER_SHARE {
            return Cause::Elsewhere {
                present_ms: frame.phases[Phase::Present as usize],
            };
        }
        let mut worst = (Phase::Input, f32::MIN);
        for phase in Phase::ALL.into_iter().filter(|&p| p != Phase::Present) {
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
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn hitches(&self) -> &VecDeque<Hitch> {
        &self.hitches
    }

    /// The newest hitch.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn last(&self) -> Option<&Hitch> {
        self.hitches.back()
    }

    /// The counts since the run began.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn session(&self) -> HitchCounts {
        let mut c = self.session;
        let clean_from = self.clean_from.unwrap_or(self.judged_to);
        c.longest_clean_secs = c.longest_clean_secs.max(self.judged_to - clean_from);
        c.since_last_secs = self
            .hitches
            .back()
            .map(|h| self.judged_to - (h.at + h.ms as f64 / 1000.0));
        c
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
        render: f64,
        present: f64,
    }

    fn steady(wall: f64) -> F {
        F {
            wall,
            input: 0.3,
            scripts: 1.0,
            render: 2.0,
            present: 0.2,
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
                (Phase::Render, f.render),
                (Phase::Present, f.present),
            ] {
                mark += ms(d);
                h.mark_at(phase, mark);
            }
            let times: Vec<(String, f32)> =
                scripts.iter().map(|(n, ms)| (n.to_string(), *ms)).collect();
            h.record_scripts(&times);
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
            matches!(h.cause, Cause::Elsewhere { present_ms } if (present_ms - 0.2).abs() < 0.01),
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
        // 2 ms frames: a 6 ms one is 3x, but only 4 ms over: no hitch.
        let mut run = Run::new(None);
        let quick = F {
            wall: 2.0,
            ..F::default()
        };
        run.frames(120, quick);
        assert_eq!(run.frame(F { wall: 6.0, ..quick }), 0);
        assert_eq!(
            run.frame(F {
                wall: 11.0,
                ..quick
            }),
            1
        );
        // 50 ms frames: 60 is 10 over but not 2x.
        let mut run = Run::new(None);
        run.frames(120, steady(50.0));
        assert_eq!(run.frame(steady(60.0)), 0);
        assert_eq!(run.frame(steady(101.0)), 1);
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
            history.gpu_ms(None);
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
            off.gpu_ms(None);
            std::hint::black_box(&mut off);
        }
        let off_us = began.elapsed().as_secs_f64() * 1e6 / frames as f64;
        assert!(off.frames().is_empty());
        let marks = Instant::now();
        for _ in 0..frames {
            std::hint::black_box(Instant::now());
        }
        let now_us = marks.elapsed().as_secs_f64() * 1e6 / frames as f64;
        println!(
            "always-on recording: {us:.2} us a frame ({:.4} ms); one Instant::now {:.0} ns; off: {:.0} ns a frame (the present's own Instant::now included)",
            us / 1000.0,
            now_us * 1000.0,
            off_us * 1000.0
        );
        assert!(history.frames().len() == HISTORY_FRAMES);
    }

    #[test]
    fn thresholds_come_from_the_environment_or_stay_default() {
        let (t, w) = HitchThresholds::parse(Some("3".into()), Some("12.5".into()));
        assert_eq!(t.factor, 3.0);
        assert_eq!(t.floor_ms, 12.5);
        assert!(w.is_empty());
        let (t, w) = HitchThresholds::parse(Some("fast".into()), Some("-1".into()));
        assert_eq!(t, HitchThresholds::default());
        assert_eq!(w.len(), 2, "{w:?}");
        assert!(w[0].contains("FLINT_HITCH_FACTOR"), "{w:?}");
        let (t, _) = HitchThresholds::parse(None, None);
        assert_eq!(t, HitchThresholds::default());
    }
}
