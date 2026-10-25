//! Game clock with fixed-timestep accumulator

use std::time::Instant;

/// The longest step a frame takes, in seconds.
const MAX_DELTA: f64 = 0.25;
/// How far paced game time may run ahead of the wall clock, in seconds.
const MAX_LEAD: f64 = 0.1;
/// Weight of each frame in the running mean of the raw frame time.
const MEAN_WEIGHT: f64 = 1.0 / 32.0;

/// Tracks game time and provides a fixed-timestep accumulator for physics updates
pub struct GameClock {
    /// Total elapsed game time in seconds
    pub total_time: f64,
    /// Time since last frame in seconds
    pub delta_time: f64,
    /// Fixed timestep interval (default: 1/60 second)
    pub fixed_timestep: f64,
    /// When set, every frame advances by exactly this many seconds instead
    /// of the wall time since the last tick (offline capture: a frame that
    /// takes 80 ms to render and save still moves the game 1/30 s).
    pub frame_step: Option<f64>,
    /// Wall time since the last tick, before pacing and the clamp. Equal
    /// to the wall time even when `delta_time` is paced or `frame_step`ped.
    pub raw_delta_time: f64,
    /// Vsync pacing: the display's refresh period, when frames are
    /// presented one a vblank (Fifo). See [`GameClock::set_vsync_period`].
    vsync_period: Option<f64>,
    /// Wall time not yet handed out as `delta_time` (negative: handed out
    /// ahead of the wall clock).
    pace_debt: f64,
    /// Running mean of the raw frame time, about half a second deep.
    raw_mean: f64,
    /// Accumulated time for fixed-step consumption
    accumulator: f64,
    /// Last tick instant
    last_instant: Instant,
    /// Whether this is the first tick
    first_tick: bool,
}

impl Default for GameClock {
    fn default() -> Self {
        Self {
            total_time: 0.0,
            delta_time: 0.0,
            fixed_timestep: 1.0 / 60.0,
            frame_step: None,
            raw_delta_time: 0.0,
            vsync_period: None,
            pace_debt: 0.0,
            raw_mean: 0.0,
            accumulator: 0.0,
            last_instant: Instant::now(),
            first_tick: true,
        }
    }
}

impl GameClock {
    /// Create a new game clock with default 60Hz fixed timestep
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a game clock with a custom fixed timestep
    pub fn with_fixed_timestep(hz: f64) -> Self {
        Self {
            fixed_timestep: 1.0 / hz,
            ..Self::default()
        }
    }

    /// Advance the clock. Call once per frame.
    pub fn tick(&mut self) {
        let now = Instant::now();

        if self.first_tick {
            self.first_tick = false;
            self.last_instant = now;
            self.delta_time = 0.0;
            return;
        }

        let raw = now.duration_since(self.last_instant).as_secs_f64();
        self.last_instant = now;
        self.advance(raw);
    }

    /// One frame of `raw` wall seconds: pace it, clamp it, accumulate it.
    fn advance(&mut self, raw: f64) {
        self.raw_delta_time = raw;
        let elapsed = match self.frame_step {
            Some(step) => step,
            None => self.pace(raw),
        };

        // Clamp to avoid spiral of death (max 250ms frame time)
        self.delta_time = elapsed.min(MAX_DELTA);
        self.total_time += self.delta_time;
        self.accumulator += self.delta_time;
    }

    /// Pace frames to the display's refresh. `None` (the default) steps by
    /// the wall time between ticks.
    ///
    /// With vsync (Fifo) every presented frame is on screen for a whole
    /// number of refreshes, but the CPU can be let through in bursts: on a
    /// hybrid-GPU laptop, frames came 6 ms, 6 ms, 6 ms, then 45 ms apart at a
    /// 60 Hz mean. Stepping the game by those makes each shown frame move
    /// 6, 6, 6, then 45 ms of game time, which is judder. Paced, each frame
    /// steps a whole number of refresh periods, as many as keep game time
    /// on the wall clock, and at least one. Game time may run up to
    /// `MAX_LEAD` ahead of the wall clock inside a burst; a stall longer
    /// than `MAX_DELTA` passes through as it would unpaced. If frames come
    /// faster on average than this refresh (the wrong monitor, or not
    /// vsynced after all), the raw time is used.
    pub fn set_vsync_period(&mut self, period: Option<f64>) {
        self.vsync_period = period.filter(|p| p.is_finite() && *p > 0.0);
        self.pace_debt = 0.0;
        // Paced until the frames show they come faster than this.
        if let Some(period) = self.vsync_period {
            self.raw_mean = period;
        }
    }

    /// The refresh period frames are paced to, if any.
    pub fn vsync_period(&self) -> Option<f64> {
        self.vsync_period
    }

    fn pace(&mut self, raw: f64) -> f64 {
        self.raw_mean = if self.raw_mean > 0.0 {
            self.raw_mean + (raw - self.raw_mean) * MEAN_WEIGHT
        } else {
            raw
        };
        let Some(period) = self.vsync_period else {
            return raw;
        };
        self.pace_debt += raw;
        if self.raw_mean < period * 0.9 || self.pace_debt > MAX_DELTA {
            self.pace_debt = 0.0;
            return raw;
        }
        let refreshes = (self.pace_debt / period).round().max(1.0);
        let dt = refreshes * period;
        self.pace_debt = (self.pace_debt - dt).max(-MAX_LEAD);
        dt
    }

    /// Returns true if there's enough accumulated time for a fixed update step
    pub fn should_fixed_update(&self) -> bool {
        self.accumulator >= self.fixed_timestep
    }

    /// Number of fixed steps the accumulator currently holds — how many
    /// `should_fixed_update` / `consume_fixed_step` rounds this frame will run.
    pub fn pending_fixed_steps(&self) -> usize {
        if self.fixed_timestep <= 0.0 {
            return 0;
        }
        (self.accumulator / self.fixed_timestep).floor().max(0.0) as usize
    }

    /// Accumulated time in units of the fixed step, before this frame's
    /// steps are consumed: `pending_fixed_steps()` plus the remainder that
    /// will be carried into the next frame. Scripted kinematic motion is
    /// spread over this span so its step velocity stays even when one step
    /// covers several frames (see `PhysicsSystem::begin_frame`).
    pub fn pending_fixed_ratio(&self) -> f64 {
        if self.fixed_timestep <= 0.0 {
            return 0.0;
        }
        (self.accumulator / self.fixed_timestep).max(0.0)
    }

    /// Consume one fixed timestep from the accumulator
    pub fn consume_fixed_step(&mut self) {
        self.accumulator -= self.fixed_timestep;
    }

    /// Get the interpolation alpha for rendering between fixed steps
    pub fn interpolation_alpha(&self) -> f64 {
        self.accumulator / self.fixed_timestep
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_clock_defaults() {
        let clock = GameClock::new();
        assert!((clock.fixed_timestep - 1.0 / 60.0).abs() < 1e-10);
        assert_eq!(clock.total_time, 0.0);
        assert_eq!(clock.delta_time, 0.0);
    }

    #[test]
    fn test_custom_timestep() {
        let clock = GameClock::with_fixed_timestep(30.0);
        assert!((clock.fixed_timestep - 1.0 / 30.0).abs() < 1e-10);
    }

    #[test]
    fn test_first_tick_zero_delta() {
        let mut clock = GameClock::new();
        clock.tick();
        assert_eq!(clock.delta_time, 0.0);
    }

    #[test]
    fn test_accumulator_logic() {
        let mut clock = GameClock::new();
        clock.fixed_timestep = 1.0 / 60.0;
        // Simulate adding time directly
        clock.accumulator = 1.0 / 30.0; // Two fixed steps worth

        assert!(clock.should_fixed_update());
        clock.consume_fixed_step();
        assert!(clock.should_fixed_update());
        clock.consume_fixed_step();
        assert!(!clock.should_fixed_update());
    }

    const HZ60: f64 = 1.0 / 60.0;

    fn paced(period: Option<f64>) -> GameClock {
        let mut clock = GameClock::new();
        clock.set_vsync_period(period);
        clock
    }

    /// Feed `frames` raw times and return the deltas handed out.
    fn run(clock: &mut GameClock, frames: impl IntoIterator<Item = f64>) -> Vec<f64> {
        frames
            .into_iter()
            .map(|raw| {
                clock.advance(raw);
                clock.delta_time
            })
            .collect()
    }

    /// Three quick frames, then a long one: a 60 Hz mean (#406).
    fn bursts(n: usize) -> impl Iterator<Item = f64> {
        (0..n).map(|i| if i % 4 == 3 { 4.0 * HZ60 - 0.018 } else { 0.006 })
    }

    #[test]
    fn unpaced_steps_by_the_raw_time() {
        let mut clock = paced(None);
        let dts = run(&mut clock, bursts(8));
        assert!((dts[0] - 0.006).abs() < 1e-12);
        assert!(dts[3] > 0.04);
    }

    #[test]
    fn paced_bursts_step_one_refresh_a_frame() {
        let mut clock = paced(Some(HZ60));
        let dts = run(&mut clock, bursts(240));
        for dt in &dts {
            assert!((dt - HZ60).abs() < 1e-9, "dt {dt}");
        }
        // Game time stays on the wall clock: 240 frames of a 60 Hz mean.
        assert!((clock.total_time - 4.0).abs() < MAX_LEAD);
        assert!((clock.raw_delta_time - (4.0 * HZ60 - 0.018)).abs() < 1e-12);
    }

    #[test]
    fn a_missed_refresh_steps_two() {
        let mut clock = paced(Some(HZ60));
        let jittered = (0..60).map(|i| 2.0 * HZ60 + if i % 2 == 0 { 0.004 } else { -0.004 });
        let dts = run(&mut clock, jittered);
        assert!(dts.iter().all(|dt| (dt - 2.0 * HZ60).abs() < 1e-9));
    }

    #[test]
    fn frames_faster_than_the_refresh_step_by_the_raw_time() {
        // Paced to 60 Hz, but presented at 120: the period is wrong.
        let mut clock = paced(Some(HZ60));
        let dts = run(&mut clock, std::iter::repeat_n(HZ60 / 2.0, 240));
        assert!(dts[120..].iter().all(|dt| (dt - HZ60 / 2.0).abs() < 1e-9));
        assert!((clock.total_time - 2.0).abs() < MAX_LEAD);
    }

    #[test]
    fn a_stall_passes_through_clamped() {
        let mut clock = paced(Some(HZ60));
        run(&mut clock, std::iter::repeat_n(HZ60, 30));
        let dts = run(&mut clock, [0.4, HZ60, HZ60]);
        assert_eq!(dts[0], MAX_DELTA);
        assert!((dts[1] - HZ60).abs() < 1e-9);
        assert!((dts[2] - HZ60).abs() < 1e-9);
    }

    #[test]
    fn frame_step_wins_over_pacing() {
        let mut clock = paced(Some(HZ60));
        clock.frame_step = Some(0.25);
        let dts = run(&mut clock, bursts(8));
        assert!(dts.iter().all(|&dt| dt == 0.25));
    }

    #[test]
    fn test_interpolation_alpha() {
        let mut clock = GameClock::new();
        clock.fixed_timestep = 1.0 / 60.0;
        clock.accumulator = clock.fixed_timestep * 0.5;
        let alpha = clock.interpolation_alpha();
        assert!((alpha - 0.5).abs() < 1e-10);
    }
}
