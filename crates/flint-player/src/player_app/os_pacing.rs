//! Frames the OS held back (#1200). Under vsync, Windows throttles the
//! presents of a window it doesn't show: one behind others (a background
//! job's, `--no-activate`'s) or minimised. Every frame then stands 250 to
//! 580 ms while the player's own work and the GPU's are a few ms, and the
//! hitch detector, judging each frame against the median of the frames
//! before it, can't see a 100 ms script stall against a 400 ms median.
//! `--capture-on-hitch`, the hitch log and `--max-hitches` went silently
//! blind in exactly the runs an agent makes.
//!
//! This doesn't fix the detector (the frames really did stand that long);
//! it says so. A frame is **held by the OS** when, under vsync, it stood at
//! least [`HELD_MS`] and [`HELD_REFRESHES`] refreshes while the player's
//! own work (every phase but the swapchain's waits, `Frame::player_ms`)
//! was under [`WORK_SHARE`] of it, and the GPU's time, where it was timed,
//! under half. A run is held when at least half of its judged frames after
//! the start were: said once on the console as it becomes so, and again
//! on `--exit-after`'s report, each with what to do about it.

use super::frame_history::Frame;
use crate::hitch_args::STARTUP_SECS;

/// A held frame stood at least this long: under 10 fps.
pub(crate) const HELD_MS: f32 = 100.0;
/// ...and at least this many refreshes, so a display that really refreshes
/// slowly (a virtual display paced at 7 Hz) isn't taken for a hidden
/// window.
pub(crate) const HELD_REFRESHES: f32 = 4.0;
/// The player's own work in a held frame is under this share of it.
const WORK_SHARE: f32 = 0.25;
/// Held frames before the run is called held, on the console or in the
/// report: about two seconds of them at the 400 ms frames #1200 measured.
const SAY_AFTER: u64 = 5;
/// The held frames kept for the report's median.
const KEPT: usize = 256;

/// Whether `f` was held by the OS; see the module docs.
pub(crate) fn held(f: &Frame) -> bool {
    let Some(refresh) = f.clock.and_then(|c| c.vsync_ms) else {
        return false; // no vsync: nothing paces the frames but the GPU
    };
    let ms = f.ms;
    ms >= HELD_MS
        && ms >= HELD_REFRESHES * refresh
        && f.player_ms() < WORK_SHARE * ms
        && f.gpu_ms.is_none_or(|g| g < 0.5 * ms)
}

/// The run's held frames, after the start.
#[derive(Debug, Default, Clone, PartialEq)]
pub(crate) struct OsPacing {
    /// Frames after the start that were looked at.
    pub frames: u64,
    /// Of them, held.
    pub held: u64,
    /// The latest held frames' wall ms and the player's work in them.
    kept: std::collections::VecDeque<(f32, f32)>,
    /// The console has been told.
    said: bool,
}

impl OsPacing {
    /// A frame closed: a line to say, the first time the run is held.
    pub(crate) fn add(&mut self, f: &Frame) -> Option<String> {
        if f.game_time < STARTUP_SECS || f.capturing {
            return None;
        }
        self.frames += 1;
        if held(f) {
            self.held += 1;
            if self.kept.len() == KEPT {
                self.kept.pop_front();
            }
            self.kept.push_back((f.ms, f.player_ms()));
        }
        if !self.said && self.is_held() {
            self.said = true;
            return Some(format!("[perf] frames held by the OS {}", self.what()));
        }
        None
    }

    /// At least half the frames after the start were held, and
    /// [`SAY_AFTER`] of them.
    pub(crate) fn is_held(&self) -> bool {
        self.held >= SAY_AFTER && self.held * 2 >= self.frames
    }

    /// The median of the kept held frames: wall ms, and the player's work.
    fn medians(&self) -> (f32, f32) {
        let median = |mut v: Vec<f32>| {
            v.sort_by(f32::total_cmp);
            v.get(v.len() / 2).copied().unwrap_or(0.0)
        };
        (
            median(self.kept.iter().map(|k| k.0).collect()),
            median(self.kept.iter().map(|k| k.1).collect()),
        )
    }

    /// `at ~412 ms (the player's work 3.1 ms): the window is hidden ...`
    fn what(&self) -> String {
        let (ms, work) = self.medians();
        format!(
            "at ~{ms:.0} ms (the player's work {work:.1} ms): the window is hidden or behind \
             others and its presents are throttled, so no hitch can stand out (the hitch log, \
             --capture-on-hitch and --max-hitches see none); FLINT_VSYNC=off measures real \
             frame times"
        )
    }

    /// `--exit-after`'s line, when the run was held.
    pub(crate) fn report(&self) -> Option<String> {
        self.is_held().then(|| {
            format!(
                "[perf] frames held by the OS: {} of {} after t={STARTUP_SECS:.0} s, {}",
                self.held,
                self.frames,
                self.what()
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use super::super::frame_history::ClockSample;
    use super::super::perf::Phase;
    use super::*;

    const HZ60: f32 = 1000.0 / 60.0;

    /// A frame of `ms` at game time `t`, `work` ms of it the player's
    /// render phase, under vsync at 60 Hz or not.
    fn frame(ms: f32, work: f32, t: f64, vsync: bool) -> Frame {
        let mut f = Frame {
            ms,
            game_time: t,
            clock: Some(ClockSample {
                raw_ms: ms,
                dt_ms: ms.min(250.0),
                paced_ms: vsync.then_some(ms),
                vsync_ms: vsync.then_some(HZ60),
            }),
            ..Frame::default()
        };
        f.phases[Phase::Render as usize] = work;
        f.phases[Phase::Acquire as usize] = ms - work;
        f
    }

    #[test]
    fn a_throttled_window_is_held_and_a_slow_game_is_not() {
        // #1200's frames: 400 ms standing, 3 ms of the player's
        assert!(held(&frame(400.0, 3.0, 3.0, true)));
        // The same frame without vsync: the GPU's, not the OS's
        assert!(!held(&frame(400.0, 3.0, 3.0, false)));
        // A game that really takes 150 ms a frame
        assert!(!held(&frame(150.0, 140.0, 3.0, true)));
        // A 60 Hz frame, and a 50 ms one: a hitch's size, not a throttle
        assert!(!held(&frame(HZ60, 3.0, 3.0, true)));
        assert!(!held(&frame(50.0, 3.0, 3.0, true)));
        // A display refreshing at 7 Hz: one refresh is no throttle
        let mut slow = frame(143.0, 3.0, 3.0, true);
        slow.clock.as_mut().unwrap().vsync_ms = Some(143.0);
        assert!(!held(&slow));
        // A GPU that was busy most of it
        let mut gpu = frame(400.0, 3.0, 3.0, true);
        gpu.gpu_ms = Some(380.0);
        assert!(!held(&gpu));
    }

    #[test]
    fn a_held_run_is_said_once_and_reported() {
        let mut p = OsPacing::default();
        // The start's frames don't count
        for i in 0..10 {
            assert_eq!(p.add(&frame(400.0, 3.0, 0.1 * i as f64, true)), None);
        }
        assert_eq!(p.frames, 0);
        let said: Vec<String> = (0..20)
            .filter_map(|i| p.add(&frame(400.0 + i as f32, 3.0, 2.0 + 0.4 * i as f64, true)))
            .collect();
        assert_eq!(said.len(), 1, "said once: {said:?}");
        assert!(
            said[0].starts_with("[perf] frames held by the OS at ~4"),
            "{}",
            said[0]
        );
        assert!(said[0].contains("FLINT_VSYNC=off"), "{}", said[0]);
        let report = p.report().expect("a held run is reported");
        assert!(
            report.starts_with(
                "[perf] frames held by the OS: 20 of 20 after t=2 s, at ~410 ms (the player's work 3.0 ms)"
            ),
            "{report}"
        );
    }

    #[test]
    fn a_run_that_shows_is_never_said() {
        let mut p = OsPacing::default();
        for i in 0..600 {
            // a steady 60 Hz with a 100 ms stall every 3 s
            let ms = if i % 180 == 0 { 116.7 } else { HZ60 };
            let work = if i % 180 == 0 { 103.0 } else { 3.0 };
            assert_eq!(p.add(&frame(ms, work, 2.0 + i as f64 / 60.0, true)), None);
        }
        assert_eq!(p.report(), None);
        // A few held frames (a window dragged behind another for a moment)
        // among many shown aren't the run's
        for i in 0..10 {
            p.add(&frame(400.0, 3.0, 12.0 + 0.4 * i as f64, true));
        }
        assert_eq!(p.report(), None);
    }
}
