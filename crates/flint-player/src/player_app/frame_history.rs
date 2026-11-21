//! The always-on frame history (#1076): the player's last few hundred
//! frames, kept whether or not `--perf` / `?perf=1` is on, so a stutter can
//! be asked about after it happened. The perf readout's ten-second summary
//! (`perf.rs`), the hitch detector (`hitch.rs`) and, after them, the F2
//! graph and the hitch log all read this one ring.
//!
//! A frame is the time from one `begin_frame` to the next (one
//! `RedrawRequested` to the next; on the web one `requestAnimationFrame` to
//! the next). Inside it the player marks where each [`Phase`] of its own
//! work ends, says when `present()` returned and how many physics steps
//! ran. The clock's view of a frame comes one tick later: the tick at the
//! next frame's start measures the interval just closed, so
//! [`FrameHistory::clock_ticked`] fills in the newest record rather than
//! the one under way. The GPU's comes later still, a frame or two and not
//! always in order: each read-back carries the number of the frame it
//! timed, and [`FrameHistory::gpu_time`] writes it onto that record
//! (#1104).
//!
//! Once the ring is full a closed frame takes the oldest record's place,
//! script buffer and all, so recording allocates nothing per frame once
//! warm (a script seen for the first time grows one buffer once).
//!
//! Each frame also carries its notes (#1077): what the engine did in it,
//! said by the code that did it through `flint_core::frame_notes` (a model
//! load, a pipeline build, a transition) and taken as the frame closes,
//! and two the history adds itself from what it already knows: a frame
//! that ran more than one physics step, and a clock stall that pacing
//! passed through.

use super::perf::Phase;
use flint_core::frame_notes::{self, FrameNote, NoteKind};
use std::collections::VecDeque;
use web_time::Instant;

/// How many frames the history keeps at least: ten seconds at 60 fps.
pub(crate) const HISTORY_FRAMES: usize = 600;

/// Whether this build records every frame: the `frame-history` feature
/// (default on; a shipped game can leave it out).
pub(crate) const BUILT_IN: bool = cfg!(feature = "frame-history");

/// A tick longer than this is a stall the clock passed through rather than
/// paced (`GameClock`'s clamp, 250 ms): noted on the frame it measured.
pub(crate) const STALL_MS: f32 = 250.0;

/// Whether to record every frame: `FLINT_FRAME_HISTORY=0|1` (also
/// `off`/`on`, `false`/`true`) over the build's default. Anything else is
/// said once and the default kept (`flint_core::env`).
pub(crate) fn always_on_from_env() -> bool {
    flint_core::env::FRAME_HISTORY.flag().unwrap_or(BUILT_IN)
}

/// The clock's view of a frame's interval, from the tick that ended it.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub(crate) struct ClockSample {
    /// The wall time between the ticks, ms (`raw_delta_time`).
    pub raw_ms: f32,
    /// The game step it became, ms (`delta_time`: paced, clamped at 250 ms
    /// and scaled by `time_scale`).
    pub dt_ms: f32,
    /// What vsync pacing made of the wall time, ms, before the clamp and
    /// the time scale: the refreshes the frame stood on screen for. `None`
    /// where the clock doesn't pace (no vsync period, `--frame-step`) or
    /// the tick stepped nothing.
    pub paced_ms: Option<f32>,
    /// The display's refresh period the clock paces to, ms.
    pub vsync_ms: Option<f32>,
}

impl ClockSample {
    /// What `clock` says of the tick it just took.
    pub(crate) fn of(clock: &flint_runtime::GameClock) -> Self {
        let ms = |s: f64| (s * 1000.0) as f32;
        Self {
            raw_ms: ms(clock.raw_delta_time),
            dt_ms: ms(clock.delta_time),
            paced_ms: clock.paced_delta_time().map(ms),
            vsync_ms: clock.vsync_period().map(ms),
        }
    }
}

/// One frame of the history. Times are ms unless named otherwise.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Frame {
    /// The frame's number: frames closed before it since the start.
    pub index: u64,
    /// When it began, seconds since the history started.
    pub at: f64,
    /// Wall time from its start to the next frame's.
    pub ms: f32,
    /// The game time it ran at: `GameClock::total_time` once its own tick
    /// had stepped, what its scripts read as `total_time()` (#1079).
    pub game_time: f64,
    /// Each phase of the player's work, indexed by `Phase as usize`.
    pub phases: [f32; Phase::COUNT],
    /// Each script callback's time, indexed as
    /// [`FrameHistory::script_names`]; empty unless script timing is on
    /// (`--perf`, `?perf=1`).
    pub scripts: Vec<f32>,
    /// The clock's view of it, once the next tick has said.
    pub clock: Option<ClockSample>,
    /// From the previous frame's `present()` returning to this one's.
    pub present_ms: Option<f32>,
    /// The fixed physics steps that ran in it.
    pub fixed_steps: u32,
    /// The GPU's busy time of this frame's own work, unsmoothed (#1104).
    /// Its read-back comes a frame or two after the frame closes, so the
    /// newest records are still `None`; so is every frame with GPU timing
    /// off, or one the timer had no free read-back for.
    pub gpu_ms: Option<f32>,
    /// What the engine did in it, in the order the work began
    /// (`flint_core::frame_notes`, #1077).
    pub notes: Vec<FrameNote>,
    /// Notes the channel had no room for in it.
    pub notes_dropped: u32,
    /// A frame capture (#1086) was being recorded or read back in it: the
    /// hitch detector passes over it, so a capture isn't counted as the
    /// hitch it may cause.
    pub capturing: bool,
    /// A scene was loaded in it behind a script's cover (#1125,
    /// [`FrameHistory::loading_behind_cover`]): its hitch, and the next
    /// frame's, are counted as `loading`.
    pub behind_cover: bool,
}

impl Frame {
    /// The player's own work in the frame: every phase but `Acquire` and
    /// `Present`, whose waits are the swapchain's, the driver's and the
    /// compositor's (#1123).
    pub(crate) fn player_ms(&self) -> f32 {
        Phase::ALL
            .iter()
            .filter(|p| !p.is_wait())
            .map(|&p| self.phases[p as usize])
            .sum()
    }

    /// How long the frame stood on screen: the paced time where the clock
    /// paces to vsync (Fifo's 6, 6, 6, 45 ms bursts are a refresh each),
    /// else the wall time.
    pub(crate) fn judged_ms(&self) -> f32 {
        self.clock.and_then(|c| c.paced_ms).unwrap_or(self.ms)
    }

    /// When it ended, seconds since the history started.
    pub(crate) fn end(&self) -> f64 {
        self.at + self.ms as f64 / 1000.0
    }

    /// Back to an empty frame, keeping the script and note buffers' room.
    fn reset(&mut self) {
        let mut scripts = std::mem::take(&mut self.scripts);
        scripts.clear();
        let mut notes = std::mem::take(&mut self.notes);
        notes.clear();
        *self = Frame {
            scripts,
            notes,
            ..Frame::default()
        };
    }
}

/// The ring of frames, and the frame under way; see the module docs.
pub(crate) struct FrameHistory {
    /// When recording started: what [`Frame::at`] counts from.
    pub start: Instant,
    frames: VecDeque<Frame>,
    /// Frames kept at least.
    min_frames: usize,
    /// Seconds kept at least, when something reads further back than
    /// `min_frames` reach (the perf readout's ten seconds at 144 Hz).
    keep_secs: Option<f64>,
    /// The frame under way: when it began, when its last mark fell, and
    /// what it has so far.
    frame_start: Option<Instant>,
    last_mark: Option<Instant>,
    under_way: Frame,
    last_present: Option<Instant>,
    /// Script callbacks seen so far, in the order first seen.
    script_names: Vec<String>,
    /// The newest frame still waits for the tick that measures it.
    awaiting_clock: bool,
    /// Frames begun since recording started: the web readout divides the
    /// page's WebGPU call counts by it (#139).
    pub frames_begun: u64,
}

impl Default for FrameHistory {
    fn default() -> Self {
        Self::new(HISTORY_FRAMES)
    }
}

impl FrameHistory {
    /// A history keeping at least `min_frames` frames.
    pub(crate) fn new(min_frames: usize) -> Self {
        let min_frames = min_frames.max(1);
        Self {
            start: Instant::now(),
            frames: VecDeque::with_capacity(min_frames),
            min_frames,
            keep_secs: None,
            frame_start: None,
            last_mark: None,
            under_way: Frame::default(),
            last_present: None,
            script_names: Vec::new(),
            awaiting_clock: false,
            frames_begun: 0,
        }
    }

    /// Keep at least `secs` of frames as well as the frame count: the ring
    /// grows (once, while it warms) until its oldest frame is that old.
    pub(crate) fn keep_secs(&mut self, secs: Option<f64>) {
        self.keep_secs = secs;
    }

    /// A new frame begins now: the one before it ends. (The player goes
    /// through `PerfRecorder`, which takes one `now` for the trace too.)
    #[cfg(test)]
    pub(crate) fn begin_frame(&mut self) {
        self.begin_frame_at(Instant::now());
    }

    pub(crate) fn begin_frame_at(&mut self, now: Instant) {
        self.frames_begun += 1;
        // What the engine said since the last frame began is this frame's
        self.under_way.notes_dropped += frame_notes::take(&mut self.under_way.notes);
        if let Some(began) = self.frame_start {
            self.under_way.at = began.duration_since(self.start).as_secs_f64();
            self.under_way.ms = now.duration_since(began).as_secs_f32() * 1000.0;
            self.under_way.index = self.frames.back().map_or(0, |f| f.index + 1);
            self.close();
        } else {
            // Anything said before the first frame began belongs to none.
            self.under_way.reset();
        }
        self.frame_start = Some(now);
        self.last_mark = Some(now);
    }

    /// The frame under way into the ring, in the oldest record's place
    /// once the ring is full, and a fresh one under way.
    fn close(&mut self) {
        let at = self.under_way.at;
        let old = |f: &Frame, keep: Option<f64>| keep.is_none_or(|k| f.at < at - k);
        let mut slot = None;
        while self.frames.len() >= self.min_frames
            && self.frames.front().is_some_and(|f| old(f, self.keep_secs))
        {
            slot = self.frames.pop_front();
        }
        let mut slot = slot.unwrap_or_default();
        slot.reset();
        std::mem::swap(&mut slot, &mut self.under_way);
        self.frames.push_back(slot);
        self.awaiting_clock = true;
    }

    /// The end of `phase`, now.
    #[cfg(test)]
    pub(crate) fn mark(&mut self, phase: Phase) {
        self.mark_at(phase, Instant::now());
    }

    /// The end of `phase` at `now`; returns when it began (the last mark),
    /// if a frame is under way.
    pub(crate) fn mark_at(&mut self, phase: Phase, now: Instant) -> Option<Instant> {
        let last = self.last_mark?;
        // `+=`: a phase marked twice in a frame (a second render path)
        // counts both.
        self.under_way.phases[phase as usize] += now.duration_since(last).as_secs_f32() * 1000.0;
        self.last_mark = Some(now);
        Some(last)
    }

    /// This frame's time in each script callback
    /// (`ScriptSystem::take_times`), added to what the frame already has.
    pub(crate) fn record_scripts(&mut self, times: &[(String, f32)]) {
        for (path, ms) in times {
            let i = match self.script_names.iter().position(|n| n == path) {
                Some(i) => i,
                None => {
                    self.script_names.push(path.clone());
                    self.script_names.len() - 1
                }
            };
            let scripts = &mut self.under_way.scripts;
            if scripts.len() <= i {
                scripts.resize(i + 1, 0.0);
            }
            scripts[i] += ms;
        }
    }

    /// `present()` returned at `now`, in the frame under way.
    pub(crate) fn presented_at(&mut self, now: Instant) {
        if let Some(last) = self.last_present {
            self.under_way.present_ms = Some(now.duration_since(last).as_secs_f32() * 1000.0);
        }
        self.last_present = Some(now);
    }

    /// The fixed physics steps that ran in the frame under way.
    pub(crate) fn fixed_steps(&mut self, steps: u32) {
        self.under_way.fixed_steps = steps;
        if steps > 1 {
            self.under_way.notes.push(FrameNote::new(
                NoteKind::PhysicsSteps,
                format!("{steps} fixed steps"),
                None,
            ));
        }
    }

    /// A GPU time read back for the frame numbered `index` (the
    /// [`under_way_index`](Self::under_way_index) it was resolved in,
    /// #1104): onto that frame's record, which is a frame or two back by
    /// the time it comes, or the frame under way's. A frame the ring no
    /// longer holds, one not begun yet, or no index at all (the frame was
    /// resolved while the history wasn't recording) drops it. Returns
    /// whether it landed.
    pub(crate) fn gpu_time(&mut self, index: Option<u64>, ms: f32) -> bool {
        let Some(index) = index else {
            return false;
        };
        let frame = if index == self.under_way_index() {
            Some(&mut self.under_way)
        } else {
            self.frame_mut(index)
        };
        match frame {
            Some(f) => {
                f.gpu_ms = Some(ms);
                true
            }
            None => false,
        }
    }

    /// The kept record numbered `index`: records are numbered without
    /// gaps, so it is found by its distance from the oldest.
    pub(crate) fn frame(&self, index: u64) -> Option<&Frame> {
        let oldest = self.frames.front()?.index;
        self.frames
            .get(usize::try_from(index.checked_sub(oldest)?).ok()?)
    }

    fn frame_mut(&mut self, index: u64) -> Option<&mut Frame> {
        let oldest = self.frames.front()?.index;
        self.frames
            .get_mut(usize::try_from(index.checked_sub(oldest)?).ok()?)
    }

    /// The clock has ticked for the frame under way, so it has measured
    /// the one before: that is the newest record's clock. Once per record;
    /// a tick with no record waiting (the first) is nobody's.
    pub(crate) fn clock_ticked(&mut self, sample: ClockSample) {
        if !std::mem::take(&mut self.awaiting_clock) {
            return;
        }
        if let Some(newest) = self.frames.back_mut() {
            newest.clock = Some(sample);
            if sample.raw_ms > STALL_MS {
                newest.notes.push(FrameNote::new(
                    NoteKind::ClockStall,
                    format!("{:.0} ms between ticks, past pacing", sample.raw_ms),
                    Some(sample.raw_ms),
                ));
            }
        }
    }

    /// The game time the frame under way runs at (its tick's `total_time`).
    pub(crate) fn game_time(&mut self, total_time: f64) {
        self.under_way.game_time = total_time;
    }

    /// The frame under way is captured, or a capture is read back in it
    /// (#1086): the hitch detector passes over it.
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))] // desktop only
    pub(crate) fn capturing(&mut self) {
        self.under_way.capturing = true;
    }

    /// The frame under way loads a scene behind a script's cover (#1125):
    /// the transition's Exiting fade ended with `complete_transition()`, so
    /// the load's long frame is drawn under it. A hitch on it, or on the
    /// frame after, is the detector's `loading`, not the game's.
    pub(crate) fn loading_behind_cover(&mut self) {
        self.under_way.behind_cover = true;
    }

    /// The number the frame under way will have once it closes.
    pub(crate) fn under_way_index(&self) -> u64 {
        self.frames.back().map_or(0, |f| f.index + 1)
    }

    /// The frames kept, oldest first.
    pub(crate) fn frames(&self) -> &VecDeque<Frame> {
        &self.frames
    }

    /// The newest whole frame.
    #[cfg_attr(not(test), allow(dead_code))] // nothing reads it yet
    pub(crate) fn newest(&self) -> Option<&Frame> {
        self.frames.back()
    }

    /// The script callbacks the frames' `scripts` are indexed by.
    pub(crate) fn script_names(&self) -> &[String] {
        &self.script_names
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn ms(n: f64) -> Duration {
        Duration::from_secs_f64(n / 1000.0)
    }

    #[test]
    fn a_frame_is_begin_to_begin_with_its_phases_and_present() {
        let mut h = FrameHistory::new(4);
        let t = h.start;
        h.begin_frame_at(t);
        h.mark_at(Phase::Input, t + ms(1.0));
        h.mark_at(Phase::Scripts, t + ms(4.0));
        h.fixed_steps(2);
        h.presented_at(t + ms(10.0));
        assert!(h.gpu_time(Some(h.under_way_index()), 3.5));
        h.begin_frame_at(t + ms(16.0));
        h.presented_at(t + ms(27.0));
        let f = h.newest().unwrap();
        assert_eq!(f.index, 0);
        assert!((f.ms - 16.0).abs() < 1e-3);
        assert!((f.phases[Phase::Scripts as usize] - 3.0).abs() < 1e-3);
        assert_eq!(f.fixed_steps, 2);
        assert_eq!(f.gpu_ms, Some(3.5));
        // The first present has nothing before it to be measured from.
        assert_eq!(f.present_ms, None);
        h.begin_frame_at(t + ms(32.0));
        let f = h.newest().unwrap();
        assert_eq!(f.index, 1);
        assert!((f.present_ms.unwrap() - 17.0).abs() < 1e-3);
        assert_eq!(f.fixed_steps, 0, "a new frame starts empty");
    }

    #[test]
    fn each_gpu_time_lands_on_the_frame_it_timed_whatever_order_it_comes_in() {
        let mut h = FrameHistory::new(4);
        let mut t = h.start;
        // Frame n's GPU work took n + 0.5 ms. Each frame is resolved under
        // its own number; the read-backs come back two or three frames
        // later, and out of order.
        let back_in: [(u64, &[u64]); 8] = [
            (0, &[]),
            (1, &[]),
            (2, &[1, 0]),
            (3, &[]),
            (4, &[2, 3]),
            (5, &[]),
            (6, &[5, 4]),
            (7, &[6]),
        ];
        for (frame, arrivals) in back_in {
            h.begin_frame_at(t);
            assert_eq!(h.under_way_index(), frame);
            for &n in arrivals {
                assert!(h.gpu_time(Some(n), n as f32 + 0.5), "frame {n} is kept");
            }
            t += ms(16.0);
        }
        h.begin_frame_at(t);
        // A ring of 4 holds frames 4..=7: each with its own time but the
        // newest, whose read-back hasn't come yet.
        let gpu: Vec<(u64, Option<f32>)> = h.frames().iter().map(|f| (f.index, f.gpu_ms)).collect();
        assert_eq!(
            gpu,
            [(4, Some(4.5)), (5, Some(5.5)), (6, Some(6.5)), (7, None)]
        );
        assert_eq!(h.frame(6).unwrap().gpu_ms, Some(6.5));
        // It comes now, a frame late, and lands on 7, not on the frame
        // under way (8).
        assert!(h.gpu_time(Some(7), 7.5));
        assert_eq!(h.newest().unwrap().gpu_ms, Some(7.5));
        h.begin_frame_at(t + ms(16.0));
        assert_eq!(h.newest().unwrap().index, 8);
        assert_eq!(h.newest().unwrap().gpu_ms, None);
        // A frame the ring let go of, one not begun, and no number at all
        // are dropped, and touch no record.
        assert!(!h.gpu_time(Some(1), 99.0));
        assert!(!h.gpu_time(Some(40), 99.0));
        assert!(!h.gpu_time(None, 99.0));
        assert!(h.frames().iter().all(|f| f.gpu_ms != Some(99.0)));
        assert!(h.frame(1).is_none() && h.frame(40).is_none());
    }

    #[test]
    fn the_tick_after_a_frame_is_its_clock() {
        let mut h = FrameHistory::new(4);
        let t = h.start;
        let tick = |paced| ClockSample {
            raw_ms: 16.0,
            dt_ms: 16.0,
            paced_ms: paced,
            vsync_ms: Some(16.7),
        };
        h.begin_frame_at(t);
        // The first frame's tick measured nothing of ours.
        h.clock_ticked(tick(None));
        assert!(h.newest().is_none());
        h.begin_frame_at(t + ms(6.0));
        h.clock_ticked(tick(Some(16.7)));
        // A second tick for the same record is ignored.
        h.clock_ticked(tick(Some(99.0)));
        let f = h.newest().unwrap();
        assert_eq!(f.clock.unwrap().paced_ms, Some(16.7));
        assert!((f.judged_ms() - 16.7).abs() < 1e-3);
        assert!((f.ms - 6.0).abs() < 1e-3);
    }

    #[test]
    fn the_ring_keeps_its_frames_and_reuses_their_room() {
        let mut h = FrameHistory::new(8);
        let mut t = h.start;
        for _ in 0..20 {
            h.begin_frame_at(t);
            h.record_scripts(&[("a.rhai:on_update".into(), 1.0)]);
            t += ms(16.0);
        }
        h.begin_frame_at(t);
        assert_eq!(h.frames().len(), 8);
        assert_eq!(h.frames().front().unwrap().index, 12);
        assert_eq!(h.newest().unwrap().index, 19);
        assert_eq!(h.newest().unwrap().scripts, vec![1.0]);
        // Warm: closing frames allocates nothing more.
        let room = h.frames().capacity();
        let buffers: Vec<*const f32> = h.frames().iter().map(|f| f.scripts.as_ptr()).collect();
        for _ in 0..16 {
            h.begin_frame_at(t);
            h.record_scripts(&[("a.rhai:on_update".into(), 1.0)]);
            t += ms(16.0);
        }
        assert_eq!(h.frames().capacity(), room);
        let reused = h
            .frames()
            .iter()
            .filter(|f| buffers.contains(&f.scripts.as_ptr()))
            .count();
        assert!(reused >= 7, "{reused} of 8 buffers reused");
    }

    #[test]
    fn keeping_seconds_grows_the_ring_past_its_frames() {
        let mut h = FrameHistory::new(10);
        h.keep_secs(Some(1.0));
        let mut t = h.start;
        // 200 frames of 10 ms: two seconds, of which the last one is kept.
        for _ in 0..200 {
            h.begin_frame_at(t);
            t += ms(10.0);
        }
        h.begin_frame_at(t);
        let n = h.frames().len();
        assert!((100..=102).contains(&n), "{n}");
    }
}
