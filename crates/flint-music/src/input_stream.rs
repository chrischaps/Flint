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

/// Continuous sway state at a moment (right stick, deadzoned + normalized,
/// each axis in [-1, 1]). Same shape as lean; a distinct type so a match
/// can never confuse the two verbs.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SwaySample {
    pub sample: i64,
    pub x: f64,
    pub y: f64,
}

/// Which trigger a pressure sample speaks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PressureSide {
    Left,
    Right,
}

impl PressureSide {
    /// The chart channel this side judges against.
    pub fn channel(self) -> &'static str {
        match self {
            PressureSide::Left => "pressure_l",
            PressureSide::Right => "pressure_r",
        }
    }
}

/// Continuous trigger depth at a moment, in [0, 1].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PressureSample {
    pub sample: i64,
    pub side: PressureSide,
    pub value: f64,
}

/// A discrete pulse at a moment. `kind` speaks the chart's verb space
/// ("pulse", "press", "flick"). `direction` is set only for flicks (the
/// normalized gesture direction at detection); press depth is *not* carried
/// here — the judge reads it from the pressure stream inside the consumed
/// window, keeping the event stream monotonic (a retro-stamped peak would
/// violate the capture thread's nondecreasing guarantee).
#[derive(Debug, Clone, PartialEq)]
pub struct PulseEvent {
    pub sample: i64,
    pub kind: String,
    pub direction: Option<[f64; 2]>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum InputEvent {
    Lean(LeanSample),
    Pulse(PulseEvent),
    Sway(SwaySample),
    Pressure(PressureSample),
}

impl InputEvent {
    pub fn sample(&self) -> i64 {
        match self {
            InputEvent::Lean(l) => l.sample,
            InputEvent::Pulse(p) => p.sample,
            InputEvent::Sway(s) => s.sample,
            InputEvent::Pressure(p) => p.sample,
        }
    }

    /// The same event re-stamped (used to map raw clock samples to suite
    /// samples across a reintegration seam).
    pub fn with_sample(&self, sample: i64) -> InputEvent {
        match self {
            InputEvent::Lean(l) => InputEvent::Lean(LeanSample { sample, ..*l }),
            InputEvent::Pulse(p) => InputEvent::Pulse(PulseEvent {
                sample,
                kind: p.kind.clone(),
                direction: p.direction,
            }),
            InputEvent::Sway(s) => InputEvent::Sway(SwaySample { sample, ..*s }),
            InputEvent::Pressure(p) => InputEvent::Pressure(PressureSample { sample, ..*p }),
        }
    }
}
