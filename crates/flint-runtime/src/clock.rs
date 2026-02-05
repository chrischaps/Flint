//! Game clock with fixed-timestep accumulator

use std::time::Instant;

/// Tracks game time and provides a fixed-timestep accumulator for physics updates
pub struct GameClock {
    /// Total elapsed game time in seconds
    pub total_time: f64,
    /// Time since last frame in seconds
    pub delta_time: f64,
    /// Fixed timestep interval (default: 1/60 second)
    pub fixed_timestep: f64,
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

        let elapsed = now.duration_since(self.last_instant).as_secs_f64();
        self.last_instant = now;

        // Clamp to avoid spiral of death (max 250ms frame time)
        self.delta_time = elapsed.min(0.25);
        self.total_time += self.delta_time;
        self.accumulator += self.delta_time;
    }

    /// Returns true if there's enough accumulated time for a fixed update step
    pub fn should_fixed_update(&self) -> bool {
        self.accumulator >= self.fixed_timestep
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

    #[test]
    fn test_interpolation_alpha() {
        let mut clock = GameClock::new();
        clock.fixed_timestep = 1.0 / 60.0;
        clock.accumulator = clock.fixed_timestep * 0.5;
        let alpha = clock.interpolation_alpha();
        assert!((alpha - 0.5).abs() < 1e-10);
    }
}
