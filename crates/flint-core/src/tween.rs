//! Easing a number over time (#734): the one [`Easing`] every "over N
//! seconds" in the engine names, and the two shapes a value on its way takes.
//!
//! - A [`Ramp`] is a pure function of time: it holds where it started, when,
//!   and for how long, so a sample at any time is the same however often (or
//!   seldom) it's asked. The parameter field's values ride on one (the wooze
//!   field among them, `flint_core::field`), as do Living Props' knobs.
//! - A [`Tween`] is a ramp stepped by frame times, for the things that move
//!   with a game clock that can pause: script tweens of a component field or
//!   a style setting, an animation layer's weight.
//!
//! Both start a new heading from wherever they stand, so a change of mind
//! never jumps. What they ease is anything [`Lerp`]: an `f32`, an `f64`, an
//! array of `f32`s, a list of `f64`s.

use std::fmt;

/// How a value moves from where it starts to where it's heading.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Easing {
    /// The same speed all the way.
    Linear,
    /// Smoothstep: slow out, slow in. The default.
    #[default]
    Smooth,
    /// Cubic, slow out of the start.
    EaseIn,
    /// Cubic, slow into the target.
    EaseOut,
    /// Cubic both ends.
    EaseInOut,
}

/// The eased fraction of `t`, written once for each float width so an `f32`
/// ease is computed in `f32` (the wooze field's ramps are, byte for byte).
macro_rules! eased_fraction {
    ($easing:expr, $t:expr) => {{
        let t = $t.clamp(0.0, 1.0);
        match $easing {
            Easing::Linear => t,
            Easing::Smooth => t * t * (3.0 - 2.0 * t),
            Easing::EaseIn => t * t * t,
            Easing::EaseOut => 1.0 - (1.0 - t).powi(3),
            Easing::EaseInOut => {
                if t < 0.5 {
                    4.0 * t * t * t
                } else {
                    1.0 - (-2.0 * t + 2.0).powi(3) / 2.0
                }
            }
        }
    }};
}

impl Easing {
    /// Every one, in the order [`Self::NAMES`] spells them.
    pub const ALL: [Easing; 5] = [
        Easing::Linear,
        Easing::Smooth,
        Easing::EaseIn,
        Easing::EaseOut,
        Easing::EaseInOut,
    ];

    /// What a script or a file calls each.
    pub const NAMES: &'static [&'static str] =
        &["linear", "smooth", "ease_in", "ease_out", "ease_in_out"];

    /// The easing called `name`; `""` is the default, smooth.
    pub fn from_name(name: &str) -> Option<Self> {
        Some(match name {
            "linear" => Self::Linear,
            "" | "smooth" => Self::Smooth,
            "ease_in" => Self::EaseIn,
            "ease_out" => Self::EaseOut,
            "ease_in_out" => Self::EaseInOut,
            _ => return None,
        })
    }

    /// The easing called `name`, or the message a caller (`function`, the
    /// script call or key that named it) gives for one that isn't.
    pub fn parse(name: &str, function: &str) -> Result<Self, String> {
        Self::from_name(name).ok_or_else(|| format!("{function}: {}", Self::refusal(name)))
    }

    /// What's wrong with `name`, which is no easing: `no easing `x`; one
    /// of linear, ...`.
    pub fn refusal(name: &str) -> String {
        format!("no easing `{name}`; one of {}", Self::NAMES.join(", "))
    }

    /// Its name, as [`Self::from_name`] takes it.
    pub fn name(self) -> &'static str {
        match self {
            Self::Linear => "linear",
            Self::Smooth => "smooth",
            Self::EaseIn => "ease_in",
            Self::EaseOut => "ease_out",
            Self::EaseInOut => "ease_in_out",
        }
    }

    /// The eased fraction of the way at `t` (0..1, clamped).
    pub fn apply(self, t: f64) -> f64 {
        eased_fraction!(self, t)
    }

    /// [`Self::apply`] in `f32`.
    pub fn apply_f32(self, t: f32) -> f32 {
        eased_fraction!(self, t)
    }

    /// The eased fraction of a span `seconds` long, `elapsed` into it: 1 for
    /// a span of no length, or past its end.
    pub fn over(self, elapsed: f32, seconds: f32) -> f32 {
        if seconds > 0.0 {
            self.apply_f32(elapsed / seconds)
        } else {
            1.0
        }
    }
}

impl fmt::Display for Easing {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// What can be eased: a value some fraction of the way between two others.
pub trait Lerp: Clone + PartialEq {
    /// `from` toward `to` by `easing` of `x`, the fraction of the time gone
    /// (0..1).
    fn ease(from: &Self, to: &Self, easing: Easing, x: f64) -> Self;
}

impl Lerp for f32 {
    fn ease(from: &Self, to: &Self, easing: Easing, x: f64) -> Self {
        from + (to - from) * easing.apply_f32(x as f32)
    }
}

impl Lerp for f64 {
    fn ease(from: &Self, to: &Self, easing: Easing, x: f64) -> Self {
        from + (to - from) * easing.apply(x)
    }
}

impl<const N: usize> Lerp for [f32; N] {
    fn ease(from: &Self, to: &Self, easing: Easing, x: f64) -> Self {
        let k = easing.apply_f32(x as f32);
        std::array::from_fn(|i| from[i] + (to[i] - from[i]) * k)
    }
}

/// Element by element; a list longer than the other eases only as far as
/// the shorter goes.
impl Lerp for Vec<f64> {
    fn ease(from: &Self, to: &Self, easing: Easing, x: f64) -> Self {
        let k = easing.apply(x);
        from.iter().zip(to).map(|(a, b)| a + (b - a) * k).collect()
    }
}

/// A value on its way from one place to another, as a pure function of time:
/// a sample at `time` is the same however the ramp got there. A new heading
/// starts from wherever it stands, so a change of mind never jumps.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ramp<T = f32> {
    from: T,
    to: T,
    start: f64,
    seconds: f64,
    easing: Easing,
}

impl<T: Lerp> Ramp<T> {
    /// A value that stays where it is (and eases smoothly when it's told to
    /// head somewhere).
    pub fn constant(value: T) -> Self {
        Self {
            from: value.clone(),
            to: value,
            start: 0.0,
            seconds: 0.0,
            easing: Easing::Smooth,
        }
    }

    /// Where it stands at `time`.
    pub fn at(&self, time: f64) -> T {
        if self.arrived(time) {
            return self.to.clone();
        }
        let x = ((time - self.start) / self.seconds).max(0.0);
        T::ease(&self.from, &self.to, self.easing, x)
    }

    /// Where it's heading: where it ends, or where it is once it's there.
    pub fn target(&self) -> &T {
        &self.to
    }

    /// How it moves.
    pub fn easing(&self) -> Easing {
        self.easing
    }

    /// Its length in seconds: 0 for one that stays where it is.
    pub fn seconds(&self) -> f64 {
        self.seconds
    }

    /// Whether it stands at its target by `time`, for good.
    pub fn arrived(&self, time: f64) -> bool {
        self.seconds <= 0.0 || time >= self.start + self.seconds
    }

    /// Head for `to` from wherever this stands at `time`, over `seconds`, as
    /// it eases now.
    pub fn head_for(&mut self, to: T, time: f64, seconds: f64) {
        self.head_for_eased(to, time, seconds, self.easing);
    }

    /// [`Self::head_for`], moving as `easing` says from here on.
    pub fn head_for_eased(&mut self, to: T, time: f64, seconds: f64, easing: Easing) {
        *self = Self {
            from: self.at(time),
            to,
            start: time,
            seconds: if seconds.is_finite() {
                seconds.max(0.0)
            } else {
                0.0
            },
            easing,
        };
    }
}

/// How far short of its length a [`Tween`]'s time may fall and still arrive,
/// as a share of it: sixty steps of 1/60 s add up to a hair under a second,
/// and a tween over a whole number of frames lands on its last one.
pub const ARRIVAL_SLACK: f64 = 1e-9;

/// A [`Ramp`] stepped by frame times: what moves on a game clock that can
/// stop (a script's tween, an animation layer's fade). It counts its own
/// time from 0, and on arriving says its target exactly.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tween<T = f32> {
    ramp: Ramp<T>,
    elapsed: f64,
}

impl<T: Lerp> Tween<T> {
    /// From `from` to `to` over `seconds` (none, not finite, or below 0: it
    /// is there at once), moving as `easing` says.
    pub fn new(from: T, to: T, seconds: f64, easing: Easing) -> Self {
        let mut ramp = Ramp::constant(from);
        ramp.head_for_eased(to, 0.0, seconds, easing);
        Self { ramp, elapsed: 0.0 }
    }

    /// Move on by `dt` seconds (one below 0 or not finite doesn't move it)
    /// and say where it stands.
    pub fn step(&mut self, dt: f64) -> T {
        if dt.is_finite() && dt > 0.0 {
            self.elapsed += dt;
        }
        self.value()
    }

    /// Where it stands: its target exactly once it's [`done`](Self::done).
    pub fn value(&self) -> T {
        if self.done() {
            self.ramp.target().clone()
        } else {
            self.ramp.at(self.elapsed)
        }
    }

    /// Whether it has arrived.
    pub fn done(&self) -> bool {
        let seconds = self.ramp.seconds();
        seconds <= 0.0 || self.elapsed >= seconds * (1.0 - ARRIVAL_SLACK)
    }

    /// Where it's heading.
    pub fn target(&self) -> &T {
        self.ramp.target()
    }

    /// Its length in seconds.
    pub fn seconds(&self) -> f64 {
        self.ramp.seconds()
    }

    /// How it moves.
    pub fn easing(&self) -> Easing {
        self.ramp.easing()
    }

    /// Seconds it has moved for.
    pub fn elapsed(&self) -> f64 {
        self.elapsed
    }

    /// Head for `to` over `seconds` from wherever it stands now, moving as
    /// `easing` says; its time starts again from 0.
    pub fn retarget(&mut self, to: T, seconds: f64, easing: Easing) {
        *self = Self::new(self.value(), to, seconds, easing);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_name_reads_back_as_its_easing() {
        for (easing, name) in Easing::ALL.iter().zip(Easing::NAMES) {
            assert_eq!(Easing::from_name(name), Some(*easing));
            assert_eq!(easing.name(), *name);
        }
        assert_eq!(Easing::from_name(""), Some(Easing::Smooth));
        assert_eq!(Easing::default(), Easing::Smooth);
        let refused = Easing::parse("bouncy", "tween").unwrap_err();
        assert!(refused.contains("tween: no easing `bouncy`"), "{refused}");
        assert!(refused.contains("ease_in_out"), "{refused}");
    }

    #[test]
    fn every_easing_starts_at_0_ends_at_1_and_never_leaves_the_range() {
        for easing in Easing::ALL {
            assert_eq!(easing.apply(0.0), 0.0, "{easing}");
            assert_eq!(easing.apply(1.0), 1.0, "{easing}");
            assert_eq!(easing.apply(-1.0), 0.0, "{easing} clamps below");
            assert_eq!(easing.apply(2.0), 1.0, "{easing} clamps above");
            let mut last = 0.0;
            for i in 0..=100 {
                let k = easing.apply(f64::from(i) / 100.0);
                assert!(k >= last, "{easing} never goes back");
                last = k;
            }
        }
        assert_eq!(Easing::Smooth.apply(0.5), 0.5);
        assert_eq!(Easing::EaseIn.apply(0.5), 0.125);
        assert_eq!(Easing::EaseOut.apply(0.5), 0.875);
        assert_eq!(Easing::Linear.over(1.0, 4.0), 0.25);
        assert_eq!(Easing::Linear.over(1.0, 0.0), 1.0, "no length: there");
    }

    /// The parameter field's ramp, as it was before it was generic: the
    /// wooze field samples it, and must not move by a bit.
    fn old_ramp_at(from: f32, to: f32, start: f64, seconds: f64, time: f64) -> f32 {
        if seconds <= 0.0 || time >= start + seconds {
            return to;
        }
        let x = ((time - start) / seconds).max(0.0) as f32;
        from + (to - from) * (x * x * (3.0 - 2.0 * x))
    }

    #[test]
    fn an_f32_ramp_is_the_old_field_ramp_bit_for_bit() {
        let mut ramp = Ramp::constant(0.37_f32);
        ramp.head_for(2.9, 10.0, 1.7);
        for i in 0..400 {
            let time = 9.5 + f64::from(i) * 0.00731;
            let old = old_ramp_at(0.37, 2.9, 10.0, 1.7, time);
            assert_eq!(ramp.at(time).to_bits(), old.to_bits(), "at {time}");
        }
    }

    #[test]
    fn a_new_heading_starts_where_the_ramp_stands() {
        let mut ramp = Ramp::constant(0.0_f64);
        ramp.head_for_eased(10.0, 0.0, 2.0, Easing::Linear);
        assert_eq!(ramp.at(1.0), 5.0);
        ramp.head_for(0.0, 1.0, 1.0);
        assert_eq!(ramp.at(1.0), 5.0, "no jump");
        assert_eq!(ramp.easing(), Easing::Linear, "kept");
        assert_eq!(ramp.at(1.5), 2.5);
        assert!(ramp.arrived(2.0));
        assert_eq!(*ramp.target(), 0.0);
        ramp.head_for(3.0, 2.0, f64::NAN);
        assert_eq!(ramp.at(2.0), 3.0, "no time to ease over: there");
    }

    #[test]
    fn a_tween_over_a_whole_number_of_frames_lands_on_its_last() {
        let mut tween = Tween::new(0.0_f64, 1.0, 1.0, Easing::Linear);
        for frame in 1..60 {
            tween.step(1.0 / 60.0);
            assert!(!tween.done(), "frame {frame}");
        }
        assert_eq!(tween.step(1.0 / 60.0), 1.0, "exactly its target");
        assert!(tween.done());
        assert_eq!(tween.step(-1.0), 1.0);
        assert_eq!(tween.step(f64::NAN), 1.0);
    }

    #[test]
    fn a_retargeted_tween_starts_from_where_it_stood() {
        let mut tween = Tween::new(vec![0.0, 10.0], vec![10.0, 0.0], 2.0, Easing::Linear);
        assert_eq!(tween.step(1.0), vec![5.0, 5.0]);
        tween.retarget(vec![5.0, 15.0], 1.0, Easing::Smooth);
        assert_eq!(tween.value(), vec![5.0, 5.0]);
        assert_eq!(tween.step(0.5), vec![5.0, 10.0]);
        assert_eq!(tween.easing(), Easing::Smooth);
        let none = Tween::new([0.0_f32; 3], [1.0, 2.0, 3.0], 0.0, Easing::Linear);
        assert!(none.done());
        assert_eq!(none.value(), [1.0, 2.0, 3.0]);
    }
}
