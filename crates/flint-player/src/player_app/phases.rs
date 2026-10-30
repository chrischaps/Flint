//! Where a frame's CPU time goes, phase by phase (#104).
//!
//! `tick` and `render` mark the end of each phase with [`PhaseTimes::lap`],
//! which costs one clock read: `Instant::now` natively and
//! `performance.now()` on the web (through `web-time`). The sums are
//! reported beside `--exit-after`'s `[perf]` lines, and every `?perf=<s>`
//! seconds in the browser's console, so a phase can be compared across
//! targets: rapier's fixed steps, the scripts (FlintKart's AI racers are
//! Rhai), the renderer.
//!
//! In a browser `performance.now()` is coarsened (to 0.1 ms in Chrome, more
//! without cross-origin isolation). One frame's reading is noise, a sum over
//! thousands of frames is not.

use web_time::Instant;

/// The phases of a frame, in the order they run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Phase {
    /// Clock, input script, transitions, extensions, game events.
    Input,
    /// Every script's `on_update`.
    Scripts,
    /// The fixed-timestep loop: rapier and its sync, 0 or more steps.
    Physics,
    /// Camera overrides and every script's `on_draw_ui`.
    ScriptUi,
    /// Audio triggers and sync, animation, IK, particles.
    AvSystems,
    /// Procgen queue, renderer sync, post overrides, end of frame.
    Sync,
    /// Encoding and submitting the scene and the HUD, up to present.
    Render,
}

impl Phase {
    pub(crate) const ALL: [Phase; 7] = [
        Phase::Input,
        Phase::Scripts,
        Phase::Physics,
        Phase::ScriptUi,
        Phase::AvSystems,
        Phase::Sync,
        Phase::Render,
    ];

    fn name(self) -> &'static str {
        match self {
            Phase::Input => "input",
            Phase::Scripts => "scripts",
            Phase::Physics => "physics",
            Phase::ScriptUi => "script ui",
            Phase::AvSystems => "audio/anim/fx",
            Phase::Sync => "sync",
            Phase::Render => "render",
        }
    }
}

/// Milliseconds spent in each phase, summed since the last report.
#[derive(Debug, Clone, Default)]
pub(crate) struct PhaseTimes {
    ms: [f64; Phase::ALL.len()],
    frames: u64,
    physics_steps: u64,
    /// Game time of the last `?perf=` report.
    last_report: f64,
}

impl PhaseTimes {
    /// Add the time since `since` to `phase`; returns now, the next
    /// phase's start.
    pub(crate) fn lap(&mut self, phase: Phase, since: Instant) -> Instant {
        let now = Instant::now();
        self.ms[phase as usize] += now.duration_since(since).as_secs_f64() * 1000.0;
        now
    }

    /// One frame presented, which ran `physics_steps` fixed steps.
    pub(crate) fn end_frame(&mut self, physics_steps: u32) {
        self.frames += 1;
        self.physics_steps += physics_steps as u64;
    }

    #[cfg(test)]
    pub(crate) fn frames(&self) -> u64 {
        self.frames
    }

    /// `[perf] phases: ...` for the frames so far, mean ms per frame, and
    /// rapier's mean per fixed step.
    pub(crate) fn summary(&self) -> Option<String> {
        if self.frames == 0 {
            return None;
        }
        let per_frame = |ms: f64| ms / self.frames as f64;
        let total: f64 = self.ms.iter().sum();
        let mut line = format!(
            "{:.2} ms/frame CPU over {} frames:",
            per_frame(total),
            self.frames
        );
        for phase in Phase::ALL {
            line.push_str(&format!(
                " {} {:.2}",
                phase.name(),
                per_frame(self.ms[phase as usize])
            ));
        }
        if self.physics_steps > 0 {
            line.push_str(&format!(
                " · {:.2} fixed steps/frame, {:.3} ms/step",
                self.physics_steps as f64 / self.frames as f64,
                self.ms[Phase::Physics as usize] / self.physics_steps as f64
            ));
        }
        Some(line)
    }

    /// The `?perf=<every>` report: a summary every `every` game seconds,
    /// then a fresh sum.
    pub(crate) fn report_due(&mut self, every: f64, game_time: f64) -> Option<String> {
        if game_time - self.last_report < every {
            return None;
        }
        self.last_report = game_time;
        let line = self.summary();
        let last_report = self.last_report;
        *self = PhaseTimes {
            last_report,
            ..Default::default()
        };
        line
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summary_is_mean_per_frame_and_per_step() {
        let mut t = PhaseTimes::default();
        t.ms[Phase::Physics as usize] = 6.0;
        t.ms[Phase::Scripts as usize] = 4.0;
        t.end_frame(2);
        t.end_frame(1);
        let s = t.summary().unwrap();
        assert!(s.starts_with("5.00 ms/frame CPU over 2 frames:"), "{s}");
        assert!(s.contains("scripts 2.00"), "{s}");
        assert!(s.contains("physics 3.00"), "{s}");
        assert!(s.contains("1.50 fixed steps/frame, 2.000 ms/step"), "{s}");
    }

    #[test]
    fn report_due_resets_after_each_report() {
        let mut t = PhaseTimes::default();
        t.end_frame(1);
        assert!(t.report_due(10.0, 5.0).is_none());
        assert!(t.report_due(10.0, 10.0).is_some());
        assert_eq!(t.frames(), 0);
        assert!(t.report_due(10.0, 15.0).is_none());
        assert!(PhaseTimes::default().summary().is_none());
    }
}
