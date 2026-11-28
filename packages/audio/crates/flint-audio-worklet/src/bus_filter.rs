//! A bus's own low-pass (#1218), on every bus in both mixers.
//!
//! kira's `Filter` would do, but it works out its coefficients (a `tan`)
//! for every sample, open or not: about 2-6 us a 128-frame quantum here,
//! as much as several voices, on every bus of every game whether or not it
//! is ever muffled. This one is the same two-pole state-variable low-pass
//! (resonance 0) with three differences:
//!
//! - open ([`OPEN`]) and not moving, it isn't run: the bus passes as it is;
//! - a cutoff that holds still has its coefficients worked out once;
//! - it fades in over one quantum when it engages, and out when it opens,
//!   so neither its fresh state nor its leaving clicks.
//!
//! This file depends only on kira: `flint-audio` includes it with
//! `#[path]` for the kira mixer on the main thread, as it does
//! `protocol.rs`, so both mixers filter alike.

use std::f64::consts::PI;

use kira::command::{command_writer_and_reader, CommandReader, CommandWriter, ValueChangeCommand};
use kira::effect::{Effect, EffectBuilder};
use kira::info::Info;
use kira::{Frame, Parameter, Tween, Value};

/// A cutoff at which the filter is open, Hz: from here up it isn't run.
pub const OPEN: f64 = 20_000.0;

/// A bus low-pass starting at `cutoff` Hz.
pub struct BusFilterBuilder {
    pub cutoff: f64,
}

/// Moves a bus low-pass's cutoff from another thread.
pub struct BusFilterHandle {
    cutoff: CommandWriter<ValueChangeCommand<f64>>,
}

impl BusFilterHandle {
    /// Move the cutoff to `hz` over `tween` ([`OPEN`] or more opens it).
    pub fn set_cutoff(&mut self, hz: f64, tween: Tween) {
        self.cutoff.write(ValueChangeCommand {
            target: Value::Fixed(hz),
            tween,
        });
    }
}

impl EffectBuilder for BusFilterBuilder {
    type Handle = BusFilterHandle;

    fn build(self) -> (Box<dyn Effect>, BusFilterHandle) {
        let (writer, reader) = command_writer_and_reader();
        let filter = BusFilter {
            reader,
            cutoff: Parameter::new(Value::Fixed(self.cutoff), OPEN),
            ic1eq: Frame::ZERO,
            ic2eq: Frame::ZERO,
            engaged: false,
            fixed: None,
        };
        (Box::new(filter), BusFilterHandle { cutoff: writer })
    }
}

struct BusFilter {
    reader: CommandReader<ValueChangeCommand<f64>>,
    cutoff: Parameter,
    ic1eq: Frame,
    ic2eq: Frame,
    /// Whether it ran last quantum.
    engaged: bool,
    /// The coefficients of a cutoff holding still, and that cutoff and
    /// sample rate.
    fixed: Option<(f64, f64, Coefficients)>,
}

#[derive(Clone, Copy)]
struct Coefficients {
    a1: f32,
    a2: f32,
    a3: f32,
}

impl Coefficients {
    /// As kira's `Filter` works them out, at resonance 0.
    fn new(cutoff: f64, sample_rate: f64) -> Self {
        let g = (PI * (cutoff / sample_rate).clamp(0.0001, 0.5)).tan();
        let k = 2.0;
        let a1 = 1.0 / (1.0 + g * (g + k));
        let a2 = g * a1;
        let a3 = g * a2;
        Self {
            a1: a1 as f32,
            a2: a2 as f32,
            a3: a3 as f32,
        }
    }
}

impl Effect for BusFilter {
    fn on_start_processing(&mut self) {
        self.cutoff.read_command(&mut self.reader);
    }

    fn process(&mut self, input: &mut [Frame], dt: f64, info: &Info) {
        let frames = input.len();
        if frames == 0 {
            return;
        }
        self.cutoff.update(dt * frames as f64, info);
        let (from, to) = (self.cutoff.previous_value(), self.cutoff.value());
        let wanted = from < OPEN || to < OPEN;
        if !wanted && !self.engaged {
            return;
        }
        if !self.engaged {
            // Engaging: a fresh state, faded in over this quantum.
            self.ic1eq = Frame::ZERO;
            self.ic2eq = Frame::ZERO;
        }
        let (wet_from, wet_to) = match (self.engaged, wanted) {
            (false, _) => (0.0f32, 1.0f32),
            (true, false) => (1.0, 0.0),
            (true, true) => (1.0, 1.0),
        };
        let sample_rate = 1.0 / dt;
        let moving = from != to;
        let still = if moving {
            None
        } else {
            Some(match self.fixed {
                Some((c, r, k)) if c == to && r == sample_rate => k,
                _ => {
                    let k = Coefficients::new(to, sample_rate);
                    self.fixed = Some((to, sample_rate, k));
                    k
                }
            })
        };
        for (i, frame) in input.iter_mut().enumerate() {
            let t = (i + 1) as f64 / frames as f64;
            let c = still.unwrap_or_else(|| Coefficients::new(from + (to - from) * t, sample_rate));
            let v3 = *frame - self.ic2eq;
            let v1 = self.ic1eq * c.a1 + v3 * c.a2;
            let v2 = self.ic2eq + self.ic1eq * c.a2 + v3 * c.a3;
            self.ic1eq = v1 * 2.0 - self.ic1eq;
            self.ic2eq = v2 * 2.0 - self.ic2eq;
            let wet = wet_from + (wet_to - wet_from) * t as f32;
            *frame = v2 * wet + *frame * (1.0 - wet);
        }
        self.engaged = wanted;
    }
}
