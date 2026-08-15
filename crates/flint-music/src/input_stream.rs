//! The timestamped input event stream judgment consumes.
//!
//! These types are the boundary between input production and everything
//! downstream: the realtime capture thread (flint-input-capture, gilrs)
//! produces them from hardware, and the replay path produces the identical
//! stream from a session file. Judgment, coherence, and logging never know
//! which one they are fed — that is the headless-testability guarantee.
//!
//! `sample` is the **compensated suite sample position**: the bridged clock
//! sample pulled back by measured output latency plus the player's
//! calibration offset, i.e. the musical moment the player was responding to.
//! Producers apply the offset; consumers never re-compensate.

/// Continuous lean state at a moment (left stick, deadzoned + normalized,
/// each axis in [-1, 1]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LeanSample {
    pub sample: i64,
    pub x: f64,
    pub y: f64,
}

/// A discrete pulse at a moment. `kind` speaks the chart's verb space
/// ("pulse", "press", "flick"); the prototype emits only "pulse".
#[derive(Debug, Clone, PartialEq)]
pub struct PulseEvent {
    pub sample: i64,
    pub kind: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum InputEvent {
    Lean(LeanSample),
    Pulse(PulseEvent),
}

impl InputEvent {
    pub fn sample(&self) -> i64 {
        match self {
            InputEvent::Lean(l) => l.sample,
            InputEvent::Pulse(p) => p.sample,
        }
    }
}
