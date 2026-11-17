//! The stepped clock (#747): time counted in beats, each held and then eased
//! into the next, the way hand-drawn animation holds a drawing and the way a
//! living prop holds a pose and glides to the next one.
//!
//! One beat is `step` seconds. [`warp`] says where a count of beats stands:
//! which target is held, and how far toward the next it has eased. A glide of
//! 0 never eases, which is a plain hold on ticks: [`tick`], what a pose held
//! on twos, a pencil's boil and the film's grain step on. A glide of 1 never
//! rests.
//!
//! Everything that steps reads this one clock, whatever it is called where it
//! is used: the living props and the wooze field (`flint_wooze`, which
//! re-exports [`warp`] and [`Cycle`]), the shading boil ([`StepClock`] under
//! `flint_wooze::BoilClock`), motion held on twos and a style's `boil` (both
//! through [`tick`], re-exported from `flint_ecs::motion_hold`). So a held pose
//! and the line drawn around it turn on the same frame.
//!
//! # Changing the step without a pop
//!
//! A clock is counted in beats and advanced by `dt / step`, never worked out
//! as `time / step`, so a new step bends it from where it stands instead of
//! moving it. [`Cycle`] holds the glide (and an amount) a cycle began with
//! until it ends, because neither can change mid-cycle continuously.

/// The shortest step, in seconds. A step under a frame can't be seen anyway,
/// and it asks for targets far faster than anything can make them.
pub const STEP_MIN: f32 = 0.01;

/// A tick lands this far (in ticks) early, so a time that sums to a whole tick
/// a hair short in floating point (5 steps of 1/60 against 1/12) still counts
/// as on it. A thousandth of a tick is 83 µs at 12 a second: far below a frame
/// at any rate a film uses.
pub const TICK_EARLY: f32 = 1e-3;

/// The clock at `beats`, counted in steps: `(index, weight)` means `weight` of
/// the way from target `index` to the next. It holds for the first
/// `1 - glide` of a step and eases over the rest (the living-props design's
/// t'). A glide of 0 never leaves the target and moves to the next on the
/// beat. Before the start reads as the start.
#[inline]
pub fn warp(beats: f64, glide: f32) -> (u64, f32) {
    let beats = beats.max(0.0);
    let index = beats.floor();
    (index as u64, eased(beats - index, glide))
}

/// How far toward the next target the clock is, `frac` of the way through a
/// step: 0 through the hold, then a smoothstep to 1 over the glide.
#[inline]
pub fn eased(frac: f64, glide: f32) -> f32 {
    let glide = f64::from(glide);
    if glide <= 1e-6 {
        return 0.0;
    }
    let x = ((frac - (1.0 - glide)) / glide).clamp(0.0, 1.0);
    crate::tween::Easing::Smooth.apply(x) as f32
}

/// Which tick `time` falls in at `rate` ticks a second: the clock with no
/// glide, a step of `1 / rate`, counted from 0 (and [`TICK_EARLY`] early).
/// `None` for a rate of 0 (or below, or not finite): nothing is held. Time
/// before 0 is tick 0.
///
/// The count is made in `f32`, as the post time it reads is, and then handed
/// to [`warp`]; widening an `f32` is exact, so this is the same floor it was
/// before the clock had a name.
#[inline]
pub fn tick(time: f32, rate: f32) -> Option<i64> {
    if !(rate > 0.0 && rate.is_finite() && time.is_finite()) {
        return None;
    }
    let beats = time.max(0.0) * rate + TICK_EARLY;
    Some(warp(f64::from(beats), 0.0).0 as i64)
}

/// A running count of beats, advanced by `dt / step` so that a change of step
/// bends it from where it stands. Whole beats are kept apart from the part of
/// one, because a single running float loses precision as it grows, and after
/// a couple of hours a beat would start to land a frame late.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct StepClock {
    whole: u64,
    part: f64,
}

impl StepClock {
    /// `dt` seconds later, at `step` seconds a beat (no shorter than
    /// [`STEP_MIN`]).
    #[inline]
    pub fn advance(&mut self, dt: f64, step: f32) {
        self.part += dt / f64::from(step.max(STEP_MIN));
        let carried = self.part.floor();
        self.part -= carried;
        self.whole += carried as u64;
    }

    /// Whole beats counted.
    #[inline]
    pub fn whole(&self) -> u64 {
        self.whole
    }

    /// How far into the current beat, 0 up to 1.
    #[inline]
    pub fn part(&self) -> f64 {
        self.part
    }

    /// The count as one number: exact while it is small, and only for reading.
    #[inline]
    pub fn beats(&self) -> f64 {
        self.whole as f64 + self.part
    }

    /// The target now and how far toward the next, at `glide`: [`warp`] of
    /// this count, with the whole beats kept exact.
    #[inline]
    pub fn now(&self, glide: f32) -> (u64, f32) {
        (self.whole, eased(self.part, glide))
    }
}

/// One part's way through its current cycle, as of when the cycle began.
///
/// The step bends the clock and can change at any moment, but a part must
/// finish a cycle on the glide and the amount it started it with, or its eased
/// weight, and what it shows, would jump. So a part takes them once a cycle, on
/// its own beat, which is also what lets a whole staggered tree take a change
/// one part at a time and never all at once.
///
/// The amount is whatever the cycle carries from target to target (the wooze
/// field's `amplitude`, for a living prop): [`Cycle::shown`] eases it with the
/// weight, so a new amount arrives with the next target and the beat itself
/// moves nothing.
///
/// A grove reads tens of thousands of these a frame, a prop's parts and each
/// of its echoes', so the read that finds the cycle still running has to be as
/// cheap as working out the clock with nothing to hold.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Cycle {
    /// The target being left, or [`Cycle::UNREAD`] before the first read.
    index: u64,
    glide: f32,
    /// The amount the target being left is shown at.
    from: f32,
    /// The amount the next target will arrive at.
    to: f32,
}

impl Default for Cycle {
    fn default() -> Self {
        Self {
            index: Self::UNREAD,
            glide: 0.0,
            from: 0.0,
            to: 0.0,
        }
    }
}

/// What [`Cycle::read`] says.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CycleNow {
    /// The target being held or left.
    pub index: u64,
    /// How far toward the next target, 0 to 1.
    pub weight: f32,
}

impl Cycle {
    /// No step ever reaches this: at a step every frame, it's far past the sun.
    const UNREAD: u64 = u64::MAX;

    /// Where this part stands at `beats`: its target, and how far toward the
    /// next. `glide` and `amount` are only taken when a cycle begins, so what a
    /// part shows here can only move as it would have anyway.
    #[inline]
    pub fn read(&mut self, beats: f64, glide: f32, amount: f32) -> CycleNow {
        let beats = beats.max(0.0);
        let floor = beats.floor();
        let index = floor as u64;
        if self.index != index {
            self.begin(index, glide, amount);
        }
        CycleNow {
            index,
            weight: eased(beats - floor, self.glide),
        }
    }

    /// How much of its amount this part shows, at a `weight` this cycle has
    /// read.
    #[inline]
    pub fn shown(&self, weight: f32) -> f32 {
        self.from + (self.to - self.from) * weight
    }

    /// A new cycle. What's arriving keeps the amount it was on its way in with,
    /// so the beat itself moves nothing. A part that has skipped a target, or
    /// just started, has nothing to keep.
    #[cold]
    fn begin(&mut self, index: u64, glide: f32, amount: f32) {
        self.from = if self.index != Self::UNREAD && self.index + 1 == index {
            self.to
        } else {
            amount
        };
        self.to = amount;
        self.glide = glide;
        self.index = index;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn warp_holds_then_glides() {
        assert_eq!(warp(2.0, 0.5), (2, 0.0));
        assert_eq!(warp(2.4, 0.5), (2, 0.0), "the first half is a hold");
        assert_eq!(warp(2.75, 0.5), (2, 0.5), "the middle of the glide");
        assert_eq!(warp(-1.0, 0.5), (0, 0.0), "before the start is the start");
        assert_eq!(warp(2.99, 0.0), (2, 0.0), "no glide never leaves");
    }

    #[test]
    fn a_tick_is_the_clock_with_no_glide() {
        assert_eq!(tick(0.0, 12.0), Some(0));
        assert_eq!(tick(0.08, 12.0), Some(0));
        assert_eq!(tick(1.0 / 12.0, 12.0), Some(1));
        assert_eq!(tick(-3.0, 12.0), Some(0));
        assert_eq!(tick(1.0, 0.0), None);
        assert_eq!(tick(f32::INFINITY, 12.0), None);
        // The same as the floor it was before (#405), at every frame of a
        // minute summed in sixtieths.
        let mut t = 0.0f32;
        for _ in 0..3600 {
            t += 1.0 / 60.0;
            for rate in [8.0, 12.0, 24.0] {
                let old = (t.max(0.0) * rate + TICK_EARLY).floor() as i64;
                assert_eq!(tick(t, rate), Some(old), "t {t} rate {rate}");
            }
        }
    }

    #[test]
    fn a_step_clock_is_warp_of_its_count() {
        let mut clock = StepClock::default();
        for _ in 0..100 {
            clock.advance(1.0 / 60.0, 0.25);
            // The same target; the weight to rounding, since `whole + part`
            // and the split count round differently.
            let (index, weight) = clock.now(0.6);
            let (want_index, want_weight) = warp(clock.beats(), 0.6);
            assert_eq!(index, want_index);
            assert!((weight - want_weight).abs() < 1e-6);
        }
        assert_eq!(clock.whole(), 6);
    }

    #[test]
    fn a_new_step_bends_the_clock_and_does_not_move_it() {
        let mut clock = StepClock::default();
        clock.advance(1.0, 0.25);
        let before = clock;
        clock.advance(0.0, 1.5);
        assert_eq!(clock, before);
        clock.advance(0.75, 1.5);
        assert_eq!(clock.beats(), before.beats() + 0.5);
    }

    #[test]
    fn a_cycle_keeps_its_glide_and_amount_until_it_ends() {
        let mut cycle = Cycle::default();
        assert_eq!(cycle.read(0.9, 0.5, 1.0).weight, warp(0.9, 0.5).1);
        // A new glide and amount mid-cycle change nothing yet.
        let now = cycle.read(0.95, 1.0, 0.0);
        assert_eq!(now.weight, warp(0.95, 0.5).1);
        assert_eq!(cycle.shown(now.weight), 1.0);
        // The next cycle takes them; what arrives keeps what it came in with.
        let next = cycle.read(1.0, 1.0, 0.0);
        assert_eq!(next.index, 1);
        assert_eq!(cycle.shown(0.0), 1.0);
        assert_eq!(cycle.shown(1.0), 0.0);
    }
}
