//! The wooze field (#213): how woozy any place in a scene is, as one function of
//! position and time that every consumer reads, so that the world agrees.
//!
//! A sample has four knobs. `step` and `glide` set the hold-and-glide clock in
//! absolute terms. `amplitude` and `jitter` are shares of what each consumer was
//! authored with: 1 means as authored, 0 means none of it.
//!
//! The field is the scene's global values with its zones laid over them in
//! order, each by how far inside it the point is. A script can move any of it,
//! at once or over some seconds, which is what makes it a function of time.
//!
//! Sampling is free of side effects and allocates nothing. A scene with no
//! `[wooze]` table has no field, so nothing samples and nothing pays.
//!
//! # What is wooze's own (#747, ADR 0123)
//!
//! Neither the clock nor the field is. The hold-and-glide clock is
//! [`flint_core::step_clock`] ([`warp`], [`Cycle`] and [`STEP_MIN`] are
//! re-exported here), the same one motion held on twos and a style's `boil`
//! tick on. The zones, falloff and ramps are [`flint_core::field::ParamField`];
//! a [`WoozeField`] is that field over [`WOOZE_KEYS`], the four knobs, with the
//! presets below. What stays here is what makes them wooze: the knobs' ranges,
//! the presets, the shading boil's film-frame cadence and the shallow clock.
//!
//! # Changing a knob without a pop
//!
//! Two things keep a change from reading as the very pop this system exists to
//! avoid. A consumer counts its clock in beats and advances it by `dt / step`,
//! so a new step bends the clock from wherever it stands instead of moving it.
//! And [`Cycle`] holds the glide and amplitude a part started its cycle with
//! until that cycle ends, because neither can change mid-cycle continuously:
//! a new glide would move the eased weight, and a new amplitude the whole pose.

use flint_core::field::{FieldKeys, ParamField, Zone};
use flint_core::step_clock::StepClock;

pub use flint_core::field::SHAPE_KEYS;
pub use flint_core::step_clock::{warp, Cycle, CycleNow, STEP_MIN};

pub mod components;

/// The largest `jitter`: enough to make a zone boil harder than its specs.
pub const JITTER_MAX: f32 = 4.0;
/// The largest `amplitude`: a consumer's whole authored motion.
pub const AMPLITUDE_MAX: f32 = 1.0;

/// How woozy one place is at one moment.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WoozeSample {
    /// How much of a consumer's slow drift shows, 0 to 1.
    pub amplitude: f32,
    /// Seconds between targets: one beat of the hold-and-glide clock.
    pub step: f32,
    /// How much of a step is spent easing to the next target, 0 to 1.
    pub glide: f32,
    /// How much of a consumer's fast jitter shows, 0 up to [`JITTER_MAX`].
    pub jitter: f32,
}

impl WoozeSample {
    /// A `[wooze]` table that says nothing else: everything as authored, on the
    /// same step and glide a `[living]` spec defaults to.
    pub const NEUTRAL: Self = Self {
        amplitude: 1.0,
        step: 0.5,
        glide: 0.3,
        jitter: 1.0,
    };

    /// The knobs in [`WOOZE_KEYS`]' order: what the general field holds.
    pub fn to_array(self) -> [f32; KNOBS] {
        [self.amplitude, self.step, self.glide, self.jitter]
    }

    /// A sample from the knobs in [`WOOZE_KEYS`]' order.
    pub fn from_array([amplitude, step, glide, jitter]: [f32; KNOBS]) -> Self {
        Self {
            amplitude,
            step,
            glide,
            jitter,
        }
    }
}

/// The built-in presets.
pub const PRESETS: &[(&str, WoozeSample)] = &[
    // The sweet spot (#219), found in Woozy Grove. Half the drift a spec asks for, on
    // a half second, is what takes a crown from swinging to wandering: the fastest 1%
    // of the picture goes from 15 pixels a frame to 6 at 1080p. A beat here is 12 film
    // frames a second, so with `boil_on = 1` the paint and ink redraw 12 times a second,
    // the rate of animation on twos. Glide matters far less once it's off the floor.
    // The book's Living Props chapter has the measurements and the strip.
    (
        "linklater",
        WoozeSample {
            amplitude: 0.5,
            step: 0.5,
            glide: 0.6,
            jitter: 1.0,
        },
    ),
    // Pure boil: small, fast and snapping, which the design calls jerky in 3D.
    (
        "boil",
        WoozeSample {
            amplitude: 0.6,
            step: 0.125,
            glide: 0.0,
            jitter: 1.5,
        },
    ),
    // Pure morph: slow and never resting, which it calls too smooth.
    (
        "morph",
        WoozeSample {
            amplitude: 1.0,
            step: 1.0,
            glide: 1.0,
            jitter: 0.0,
        },
    ),
    // Barely there: for the edge of a woozy world, or to come down from one.
    (
        "calm",
        WoozeSample {
            amplitude: 0.35,
            step: 1.5,
            glide: 0.6,
            jitter: 0.25,
        },
    ),
];

/// A built-in preset by name.
pub fn preset(name: &str) -> Option<WoozeSample> {
    PRESETS.iter().find(|(n, _)| *n == name).map(|(_, s)| *s)
}

fn preset_names() -> String {
    let names: Vec<&str> = PRESETS.iter().map(|(n, _)| *n).collect();
    names.join(", ")
}

const KNOBS: usize = 4;

/// The wooze field's key set: its four knobs, how each is checked, and the
/// presets. A [`WoozeField`] is the general field over these.
pub static WOOZE_KEYS: FieldKeys<KNOBS> = FieldKeys {
    field: "wooze",
    names: ["amplitude", "step", "glide", "jitter"],
    check: check_knob,
    preset: Some(preset_knobs),
};

fn preset_knobs(name: &str) -> Result<[f32; KNOBS], String> {
    preset(name)
        .map(WoozeSample::to_array)
        .ok_or_else(|| format!("preset '{name}' isn't one of {}", preset_names()))
}

/// Check a knob's value, the same for the scene table, a zone and a script.
fn check_knob(knob: usize, value: f32) -> Result<f32, String> {
    let name = WOOZE_KEYS.names[knob];
    let (ok, range) = match knob {
        0 => ((0.0..=AMPLITUDE_MAX).contains(&value), "0 to 1".to_string()),
        1 => (
            value.is_finite() && value >= STEP_MIN,
            format!("{STEP_MIN} seconds or more"),
        ),
        2 => ((0.0..=1.0).contains(&value), "0 to 1".to_string()),
        _ => (
            (0.0..=JITTER_MAX).contains(&value),
            format!("0 to {JITTER_MAX}"),
        ),
    };
    if ok {
        Ok(value)
    } else {
        Err(format!("{name} = {value} must be {range}"))
    }
}

/// Check a knob by name (`amplitude`, `step`, `glide` or `jitter`), as the field
/// checks its own: for anything else that holds one, such as a species (#246).
pub fn check_knob_named(name: &str, value: f32) -> Result<f32, String> {
    WOOZE_KEYS.check_named(name, value)
}

/// Film frames to one beat of the field. Six is what ones, twos and threes all
/// divide, so a drawing lands on every beat whichever a scene picks, and the
/// `linklater` preset's half-second step is exactly 12 frames a second.
pub const FILM_FRAMES_PER_BEAT: u32 = 6;

/// Which drawing the shading boil (#214) is on, and how hard it boils.
///
/// The default is no boil at all, which is what a scene with no field gets.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct BoilFrame {
    /// The drawing: the same for as long as it's held, and new when it changes.
    pub seed: u32,
    /// How much of the authored boil shows, the field's `jitter` here and now.
    pub amount: f32,
    /// The field's step here and now in seconds, or 0 with no field.
    pub step: f32,
}

/// The shading boil's clock: a [`StepClock`] on the field's step, like any
/// other consumer's, so a change of step bends it from where it stands. It runs
/// on game time, so a drawing lasts as long at 30 frames a second as at 144.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct BoilClock {
    clock: StepClock,
}

impl BoilClock {
    /// `dt` seconds later, at the tempo of `sample`.
    #[inline]
    pub fn tick(&mut self, dt: f64, sample: &WoozeSample) {
        self.clock.advance(dt, sample.step);
    }

    /// The drawing now, held for `on` film frames: 1, 2 or 3, and anything else
    /// is taken as the nearest of them. The nudge keeps a frame that lands exactly
    /// on a change from falling short of it by a rounding error, which would hold
    /// one drawing for a frame too long and the next for a frame too few.
    pub fn drawing(&self, on: u32) -> u64 {
        let per_beat = u64::from(FILM_FRAMES_PER_BEAT);
        let into_beat = (self.clock.part() * per_beat as f64 + 1e-9).floor() as u64;
        (self.clock.whole() * per_beat + into_beat) / u64::from(on.clamp(1, 3))
    }

    /// What the passes need to know this frame, from the field where the viewer
    /// stands. The boil's strength is the field's `jitter`, its layer of fast small
    /// motion, so a zone can hold its trees still and go on boiling their lines.
    pub fn frame(&self, sample: &WoozeSample, on: u32) -> BoilFrame {
        BoilFrame {
            seed: self.drawing(on) as u32,
            amount: sample.jitter,
            step: sample.step,
        }
    }
}

/// The hold-and-glide clock of something with no poses of its own (#215), such
/// as a hand-made mesh that shallow wooze displaces.
///
/// It counts beats the way a living prop does, from the same field and with the
/// same `dt`, so it keeps the same tempo as any prop standing beside it, through
/// every change of step. It has no phase offset of its own: two clocks in the same
/// place read the same target and weight, so the world-space noise they displace
/// by is one continuous field, and a lantern set on a bench stays set on it.
///
/// Its count is one `f64`, as a living prop's is, not a [`StepClock`]'s whole
/// and part: the two round differently, and these must keep the props' beat.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ShallowClock {
    beats: f64,
    cycle: Cycle,
}

impl ShallowClock {
    /// A clock starting at `time` where `sample` was read. Started late, as
    /// something a script spawned is, it lands on the beat the scene has reached
    /// if the step hasn't changed, and not on a phase of its own.
    pub fn starting(time: f64, sample: &WoozeSample) -> Self {
        Self {
            beats: time.max(0.0) / f64::from(sample.step.max(STEP_MIN)),
            cycle: Cycle::default(),
        }
    }

    /// Move on by `dt` seconds at the place `sample` was read.
    #[inline]
    pub fn advance(&mut self, dt: f64, sample: &WoozeSample) -> ShallowNow {
        self.beats += dt / f64::from(sample.step.max(STEP_MIN));
        let now = self.cycle.read(self.beats, sample.glide, sample.amplitude);
        ShallowNow {
            index: now.index,
            weight: now.weight,
            amplitude: self.cycle.shown(now.weight),
        }
    }

    /// How many beats it has counted.
    pub fn beats(&self) -> f64 {
        self.beats
    }
}

/// What [`ShallowClock::advance`] says.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ShallowNow {
    /// The target being held or left.
    pub index: u64,
    /// How far toward the next target, 0 to 1.
    pub weight: f32,
    /// How much of its authored drift shows, 0 to 1.
    pub amplitude: f32,
}

/// One part of the world woozier or calmer than the rest: a zone of the
/// general field over the four knobs.
pub type WoozeZone = Zone<KNOBS>;

/// The whole field: global values and zones, the general
/// [`ParamField`] over [`WOOZE_KEYS`] read back as [`WoozeSample`]s.
#[derive(Debug, Clone, PartialEq)]
pub struct WoozeField(ParamField<KNOBS>);

impl WoozeField {
    /// A field with these global values everywhere and no zones.
    pub fn new(base: WoozeSample) -> Self {
        Self(ParamField::new(&WOOZE_KEYS, base.to_array()))
    }

    /// The general field underneath, for reading a knob by name.
    pub fn params(&self) -> &ParamField<KNOBS> {
        &self.0
    }

    /// The zones, in the order they apply.
    pub fn zones(&self) -> &[WoozeZone] {
        self.0.zones()
    }

    /// The field's global values at `time`, before any zone: what `set_global`
    /// moves, and what a debug panel shows.
    pub fn global(&self, time: f64) -> WoozeSample {
        WoozeSample::from_array(self.0.global(time))
    }

    /// Where the global knobs are heading: the end of a ramp `set_global`
    /// started, or the values themselves when none is running.
    pub fn global_target(&self) -> WoozeSample {
        WoozeSample::from_array(self.0.global_target())
    }

    /// How woozy `position` is at `time`, in whichever clock the caller keeps
    /// this field on. A pure function: the same arguments give the same bits.
    #[inline]
    pub fn sample(&self, position: [f32; 3], time: f64) -> WoozeSample {
        WoozeSample::from_array(self.0.sample(position, time))
    }

    /// Which zone has the last word on each knob at `position` and `time`, in the
    /// order `amplitude`, `step`, `glide`, `jitter`: `None` where the global value
    /// stands. For saying where a value came from (#246), not for every frame.
    pub fn zones_at(&self, position: [f32; 3], time: f64) -> [Option<&str>; KNOBS] {
        self.0.zones_at(position, time)
    }

    /// The zones that reach `position` at `time`, in the order they apply, each
    /// with how much of itself it gives there, 0 to 1: what a tree picked in the
    /// world stands in (#248), whether or not one of them has the last word.
    pub fn zones_in(&self, position: [f32; 3], time: f64) -> Vec<(&str, f32)> {
        self.0.zones_in(position, time)
    }

    /// Change global knobs from a table of any of `amplitude`, `step`, `glide`,
    /// `jitter` and `preset`, heading for them over `seconds` from `time`. The
    /// preset applies first and the others on top. Nothing changes on an error.
    pub fn set_global(
        &mut self,
        table: &toml::Table,
        time: f64,
        seconds: f64,
    ) -> Result<(), String> {
        self.0.set_global(table, time, seconds)
    }

    /// Add a zone from a `[[wooze.zone]]` table, or an error saying what's wrong
    /// with it. An unnamed one takes a name from its place in the list.
    pub fn add_zone(&mut self, table: &toml::Table) -> Result<(), String> {
        self.0.add_zone(table)
    }

    /// Change the named zone from a table, or add it if there's none yet, either
    /// way over `seconds` from `time`. A new zone fades in whole; an existing one
    /// keeps what the table leaves out, and its shape and place move at once.
    /// Nothing changes on an error.
    pub fn set_zone(
        &mut self,
        name: &str,
        table: &toml::Table,
        time: f64,
        seconds: f64,
    ) -> Result<(), String> {
        self.0.set_zone(name, table, time, seconds)
    }

    /// Fade the named zone out over `seconds` and drop it. Returns whether there
    /// was one.
    pub fn remove_zone(&mut self, name: &str, time: f64, seconds: f64) -> bool {
        self.0.remove_zone(name, time, seconds)
    }
}

/// Every key a `[[wooze.zone]]` table takes: the field's [`SHAPE_KEYS`], then
/// the four knobs.
pub const ZONE_KEYS: [&str; 10] = [
    "name",
    "shape",
    "center",
    "radius",
    "size",
    "falloff",
    "amplitude",
    "step",
    "glide",
    "jitter",
];

#[cfg(test)]
mod boil_tests {
    use super::*;

    fn at(step: f32, jitter: f32) -> WoozeSample {
        WoozeSample {
            step,
            jitter,
            ..WoozeSample::NEUTRAL
        }
    }

    /// The drawing after each of `frames` film frames at 24 frames a second.
    fn drawings(frames: usize, step: f32, on: u32) -> Vec<u64> {
        let sample = at(step, 1.0);
        let mut clock = BoilClock::default();
        (0..frames)
            .map(|_| {
                clock.tick(1.0 / 24.0, &sample);
                clock.drawing(on)
            })
            .collect()
    }

    #[test]
    fn on_twos_a_drawing_lasts_two_film_frames_and_on_threes_three() {
        // A quarter-second step is six film frames at 24 a second.
        assert_eq!(drawings(8, 0.25, 1), [1, 2, 3, 4, 5, 6, 7, 8]);
        assert_eq!(drawings(8, 0.25, 2), [0, 1, 1, 2, 2, 3, 3, 4]);
        assert_eq!(drawings(8, 0.25, 3), [0, 0, 1, 1, 1, 2, 2, 2]);
    }

    #[test]
    fn hours_of_rounding_never_lengthen_or_shorten_a_drawing() {
        // Two hours of film.
        let all = drawings(24 * 60 * 60 * 2, 0.25, 2);
        for pair in all.windows(2) {
            assert!(pair[1] - pair[0] <= 1);
        }
        // Each drawing past the first is held for exactly two frames.
        for chunk in all[1..all.len() - 1].chunks(2) {
            assert_eq!(chunk[0], chunk[1]);
        }
    }

    #[test]
    fn a_drawing_lasts_the_same_time_at_any_frame_rate() {
        let sample = at(0.25, 1.0);
        for fps in [30.0, 60.0, 144.0, 240.0] {
            let mut clock = BoilClock::default();
            for _ in 0..(fps as usize) {
                clock.tick(1.0 / fps, &sample);
            }
            assert_eq!(clock.drawing(2), 12, "at {fps} frames a second");
        }
    }

    #[test]
    fn every_cadence_lands_on_the_beat() {
        let sample = at(0.5, 1.0);
        for on in 1..=3 {
            let mut clock = BoilClock::default();
            clock.tick(sample.step as f64 * 3.0, &sample);
            assert_eq!(clock.drawing(on), u64::from(3 * FILM_FRAMES_PER_BEAT / on));
        }
    }

    #[test]
    fn a_change_of_step_bends_the_clock_and_does_not_move_it() {
        let mut clock = BoilClock::default();
        clock.tick(1.0, &at(0.25, 1.0));
        let before = clock.drawing(2);
        clock.tick(0.0, &at(1.5, 1.0));
        assert_eq!(clock.drawing(2), before);
    }

    #[test]
    fn a_cadence_outside_the_three_is_taken_as_the_nearest() {
        let mut clock = BoilClock::default();
        clock.tick(1.0, &at(0.25, 1.0));
        assert_eq!(clock.drawing(0), clock.drawing(1));
        assert_eq!(clock.drawing(9), clock.drawing(3));
    }

    #[test]
    fn the_frame_takes_its_strength_from_jitter_alone() {
        // A zone can hold its trees still and go on boiling their lines.
        let still_trees = WoozeSample {
            amplitude: 0.0,
            jitter: 1.5,
            step: 0.25,
            glide: 0.3,
        };
        let frame = BoilClock::default().frame(&still_trees, 2);
        assert_eq!(frame.amount, 1.5);
        assert_eq!(frame.step, 0.25);
        assert_eq!(BoilFrame::default().amount, 0.0);
    }

    #[test]
    fn the_seed_wraps_rather_than_saturating() {
        let mut clock = BoilClock::default();
        // Past u32::MAX drawings: the pattern goes on changing.
        let beats = ((1u64 << 32) + 2) / u64::from(FILM_FRAMES_PER_BEAT);
        // A quarter-second step, so the count lands on the beat exactly.
        clock.tick(beats as f64 * 0.25, &at(0.25, 1.0));
        assert_eq!(clock.frame(&at(0.25, 1.0), 1).seed, 2);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table(src: &str) -> toml::Table {
        src.parse().unwrap()
    }

    #[test]
    fn a_shallow_clock_holds_then_glides_on_the_beat() {
        let sample = WoozeSample {
            step: 1.0,
            glide: 0.5,
            ..WoozeSample::NEUTRAL
        };
        let mut clock = ShallowClock::starting(0.0, &sample);
        let held = clock.advance(0.25, &sample);
        assert_eq!((held.index, held.weight), (0, 0.0));
        let gliding = clock.advance(0.5, &sample);
        assert_eq!(gliding.index, 0);
        assert_eq!(
            gliding.weight, 0.5,
            "halfway through a glide of half a step"
        );
        let next = clock.advance(0.5, &sample);
        assert_eq!((next.index, next.weight), (1, 0.0));
    }

    #[test]
    fn a_shallow_clocks_amplitude_changes_without_a_pop() {
        // As with a prop's part: a new amplitude arrives with the next target, so
        // the beat itself moves nothing.
        let mut sample = WoozeSample {
            amplitude: 1.0,
            step: 1.0,
            glide: 1.0,
            jitter: 0.0,
        };
        let mut clock = ShallowClock::starting(0.0, &sample);
        let dt = 1.0 / 120.0;
        let mut last = clock.advance(dt, &sample).amplitude;
        for frame in 0..480 {
            if frame == 60 {
                sample.amplitude = 0.25;
            }
            let shown = clock.advance(dt, &sample).amplitude;
            assert!(
                (shown - last).abs() < 0.03,
                "frame {frame}: {last} to {shown}"
            );
            last = shown;
        }
        assert_eq!(last, 0.25);
    }

    #[test]
    fn a_shallow_clock_started_late_is_where_one_started_early_would_be() {
        let sample = WoozeSample::NEUTRAL;
        let mut early = ShallowClock::starting(0.0, &sample);
        for _ in 0..600 {
            early.advance(0.01, &sample);
        }
        let late = ShallowClock::starting(6.0, &sample);
        assert!((early.beats() - late.beats()).abs() < 1e-9);
    }

    fn field_with(zone: &str) -> WoozeField {
        let mut field = WoozeField::new(WoozeSample::NEUTRAL);
        field.add_zone(&table(zone)).unwrap();
        field
    }

    const POCKET: &str = r#"
        name = "pocket"
        shape = "sphere"
        center = [0, 0, 12]
        radius = 6
        falloff = 4
        glide = 0.8
    "#;

    #[test]
    fn a_sphere_holds_inside_and_falls_off_smoothly_to_the_global_value() {
        let field = field_with(POCKET);
        let at = |z: f32| field.sample([0.0, 0.0, 12.0 + z], 0.0).glide;
        assert_eq!(at(0.0), 0.8);
        assert_eq!(at(6.0), 0.8, "the edge is still inside");
        assert_eq!(at(10.0), WoozeSample::NEUTRAL.glide, "the falloff's end");
        assert_eq!(at(30.0), WoozeSample::NEUTRAL.glide);

        // Halfway through the falloff is halfway between, and it never turns back.
        assert!((at(8.0) - 0.55).abs() < 1e-6, "{}", at(8.0));
        let mut last = at(6.0);
        for i in 1..=40 {
            let now = at(6.0 + i as f32 * 0.1);
            assert!(now <= last + 1e-6, "rose at {i}");
            assert!(last - now < 0.03, "a jump of {} at {i}", last - now);
            last = now;
        }
    }

    #[test]
    fn a_box_reaches_its_corners_by_distance() {
        let field = field_with(
            r#"shape = "box"
            center = [10, 0, 0]
            size = [4, 2, 4]
            falloff = 1
            step = 2.0"#,
        );
        let step = |p: [f32; 3]| field.sample(p, 0.0).step;
        assert_eq!(step([10.0, 0.0, 0.0]), 2.0);
        assert_eq!(step([12.0, 1.0, 2.0]), 2.0, "a corner is inside");
        assert_eq!(step([13.0, 0.0, 0.0]), 0.5, "one metre out is the end");
        let past_a_corner = step([12.5, 1.5, 2.5]);
        assert!(past_a_corner > 0.5 && past_a_corner < 2.0);
    }

    #[test]
    fn a_zone_only_touches_the_knobs_it_names() {
        let field = field_with(POCKET);
        let inside = field.sample([0.0, 0.0, 12.0], 0.0);
        assert_eq!(inside.step, WoozeSample::NEUTRAL.step);
        assert_eq!(inside.amplitude, WoozeSample::NEUTRAL.amplitude);
        assert_eq!(inside.jitter, WoozeSample::NEUTRAL.jitter);
    }

    #[test]
    fn later_zones_lie_over_earlier_ones() {
        let mut field = field_with(POCKET);
        field
            .add_zone(&table(
                r#"name = "core"
                shape = "sphere"
                center = [0, 0, 12]
                radius = 1
                glide = 0.1"#,
            ))
            .unwrap();
        assert_eq!(field.sample([0.0, 0.0, 12.0], 0.0).glide, 0.1);
        assert_eq!(field.sample([0.0, 0.0, 14.0], 0.0).glide, 0.8);

        // What a picked tree stands in (#248): both at the core, in order, and
        // only the pocket a little way out; nothing far off.
        let names = |p: [f32; 3]| -> Vec<String> {
            let zones = field.zones_in(p, 0.0);
            assert!(
                zones.iter().all(|(_, w)| *w > 0.0 && *w <= 1.0),
                "{zones:?}"
            );
            zones
                .into_iter()
                .map(|(name, _)| name.to_string())
                .collect()
        };
        let pocket = field.zones()[0].name().to_string();
        assert_eq!(
            names([0.0, 0.0, 12.0]),
            [pocket.clone(), "core".to_string()]
        );
        assert_eq!(names([0.0, 0.0, 14.0]), [pocket]);
        assert!(names([0.0, 0.0, 400.0]).is_empty());
    }

    #[test]
    fn an_unnamed_zone_is_named_by_its_place() {
        let mut field = WoozeField::new(WoozeSample::NEUTRAL);
        let zone = table("shape = \"sphere\"\ncenter = [0, 0, 0]\nradius = 1\nglide = 1");
        field.add_zone(&zone).unwrap();
        field.add_zone(&zone).unwrap();
        assert_eq!(field.zones()[1].name(), "zone 2");
    }

    #[test]
    fn a_malformed_zone_says_what_is_wrong() {
        let cases = [
            ("center = [0, 0, 0]\nradius = 1\nglide = 1", "shape"),
            ("shape = \"cone\"\ncenter = [0, 0, 0]\nglide = 1", "cone"),
            ("shape = \"sphere\"\nradius = 1\nglide = 1", "center"),
            (
                "shape = \"sphere\"\ncenter = [0, 0]\nradius = 1\nglide = 1",
                "center",
            ),
            (
                "shape = \"sphere\"\ncenter = [0, 0, 0]\nglide = 1",
                "radius",
            ),
            (
                "shape = \"sphere\"\ncenter = [0, 0, 0]\nradius = -1\nglide = 1",
                "radius",
            ),
            (
                "shape = \"sphere\"\ncenter = [0, 0, 0]\nsize = [1, 1, 1]\nglide = 1",
                "size",
            ),
            ("shape = \"box\"\ncenter = [0, 0, 0]\nglide = 1", "size"),
            (
                "shape = \"box\"\ncenter = [0, 0, 0]\nsize = [1, 0, 1]\nglide = 1",
                "size",
            ),
            (
                "shape = \"sphere\"\ncenter = [0, 0, 0]\nradius = 1",
                "changes nothing",
            ),
            (
                "shape = \"sphere\"\ncenter = [0, 0, 0]\nradius = 1\nglide = 1.5",
                "glide",
            ),
            (
                "shape = \"sphere\"\ncenter = [0, 0, 0]\nradius = 1\nstep = 0",
                "step",
            ),
            (
                "shape = \"sphere\"\ncenter = [0, 0, 0]\nradius = 1\nglide = \"a\"",
                "glide",
            ),
            (
                "shape = \"sphere\"\ncenter = [0, 0, 0]\nradius = 1\ngilde = 1",
                "gilde",
            ),
            (
                "shape = \"sphere\"\ncenter = [0, 0, 0]\nradius = 1\nglide = 1\nfalloff = -1",
                "falloff",
            ),
        ];
        for (zone, says) in cases {
            let mut field = WoozeField::new(WoozeSample::NEUTRAL);
            let reason = field.add_zone(&table(zone)).unwrap_err();
            assert!(reason.contains(says), "{zone:?} said {reason:?}");
            assert!(field.zones().is_empty());
        }
    }

    #[test]
    fn two_zones_cannot_share_a_name() {
        let mut field = field_with(POCKET);
        let reason = field.add_zone(&table(POCKET)).unwrap_err();
        assert!(reason.contains("taken"), "{reason}");
    }

    /// What the grove's judgement came to (#219). A change here changes the look the
    /// book's strip shows, so make it on purpose and make the strip again.
    #[test]
    fn linklater_is_the_look_the_grove_settled_on() {
        let look = preset("linklater").unwrap();
        assert_eq!(
            (look.amplitude, look.step, look.glide, look.jitter),
            (0.5, 0.5, 0.6, 1.0)
        );
        // On ones, that's a new drawing 12 times a second.
        assert_eq!(f64::from(FILM_FRAMES_PER_BEAT) / f64::from(look.step), 12.0);
    }

    #[test]
    fn every_preset_is_a_valid_set_of_knobs() {
        assert!(preset("linklater").is_some());
        assert!(preset("nope").is_none());
        for (name, sample) in PRESETS {
            for (i, value) in sample.to_array().into_iter().enumerate() {
                check_knob(i, value).unwrap_or_else(|e| panic!("{name}: {e}"));
            }
        }
    }

    #[test]
    fn a_ramp_moves_a_global_knob_over_time_and_is_a_function_of_it() {
        let mut field = WoozeField::new(WoozeSample::NEUTRAL);
        field.set_global(&table("step = 2.5"), 10.0, 2.0).unwrap();
        let step = |t: f64| field.sample([0.0; 3], t).step;
        assert_eq!(step(10.0), 0.5);
        assert_eq!(step(11.0), 1.5, "halfway through a smoothstep is halfway");
        assert_eq!(step(12.0), 2.5);
        assert_eq!(step(99.0), 2.5);
        // Asked twice, the same answer: nothing about sampling moves it.
        assert_eq!(step(10.7), step(10.7));

        // Turned around midway, it leaves from where it is.
        field.set_global(&table("step = 0.5"), 11.0, 2.0).unwrap();
        assert_eq!(field.sample([0.0; 3], 11.0).step, 1.5);
    }

    #[test]
    fn set_global_takes_a_preset_with_knobs_on_top_or_changes_nothing() {
        let mut field = WoozeField::new(WoozeSample::NEUTRAL);
        field
            .set_global(&table("preset = \"boil\"\nglide = 0.2"), 0.0, 0.0)
            .unwrap();
        let boil = preset("boil").unwrap();
        let now = field.sample([0.0; 3], 0.0);
        assert_eq!(now.step, boil.step);
        assert_eq!(now.glide, 0.2);

        let before = field.clone();
        for bad in [
            "step = 1.0\nglide = 7.0",
            "preset = \"nope\"",
            "gilde = 1.0",
        ] {
            assert!(field.set_global(&table(bad), 0.0, 0.0).is_err(), "{bad}");
            assert_eq!(field, before, "{bad}");
        }
    }

    #[test]
    fn a_script_edits_a_zone_and_keeps_what_it_left_out() {
        let mut field = field_with(POCKET);
        field
            .set_zone("pocket", &table("step = 3.0"), 0.0, 0.0)
            .unwrap();
        let inside = field.sample([0.0, 0.0, 12.0], 0.0);
        assert_eq!((inside.glide, inside.step), (0.8, 3.0));

        // It can walk the zone around without restating anything else.
        field
            .set_zone("pocket", &table("center = [100, 0, 0]"), 0.0, 0.0)
            .unwrap();
        assert_eq!(field.sample([100.0, 0.0, 0.0], 0.0).glide, 0.8);
        assert_eq!(field.sample([0.0, 0.0, 12.0], 0.0).glide, 0.3);
        assert_eq!(field.zones().len(), 1);
    }

    #[test]
    fn a_new_zone_fades_in_and_a_removed_one_fades_out() {
        let mut field = WoozeField::new(WoozeSample::NEUTRAL);
        let new = table("shape = \"sphere\"\ncenter = [0, 0, 0]\nradius = 5\nglide = 0.9");
        field.set_zone("late", &new, 10.0, 2.0).unwrap();
        let glide = |f: &WoozeField, t: f64| f.sample([0.0; 3], t).glide;
        assert_eq!(glide(&field, 10.0), 0.3);
        assert!((glide(&field, 11.0) - 0.6).abs() < 1e-5);
        assert_eq!(glide(&field, 12.0), 0.9);

        assert!(field.remove_zone("late", 20.0, 2.0));
        assert_eq!(glide(&field, 20.0), 0.9);
        assert!((glide(&field, 21.0) - 0.6).abs() < 1e-5);
        assert_eq!(glide(&field, 22.0), 0.3);

        // Gone for good once it has faded: a name can come back.
        assert!(!field.remove_zone("late", 23.0, 0.0));
        assert!(field.zones().is_empty());
    }

    #[test]
    fn a_bad_edit_leaves_the_zone_alone() {
        let mut field = field_with(POCKET);
        let before = field.clone();
        assert!(field
            .set_zone("pocket", &table("glide = 9.0"), 0.0, 0.0)
            .is_err());
        assert!(field
            .set_zone("new", &table("glide = 0.5"), 0.0, 0.0)
            .is_err());
        assert_eq!(field, before);
    }

    /// A part's read of the field, as a living prop makes it.
    fn read_cycle(part: &mut Cycle, beats: f64, s: &WoozeSample) -> CycleNow {
        part.read(beats, s.glide, s.amplitude)
    }

    #[test]
    fn the_zone_keys_are_the_fields_and_the_knobs() {
        let mut keys = SHAPE_KEYS.to_vec();
        keys.extend(WOOZE_KEYS.names);
        assert_eq!(keys, ZONE_KEYS);
    }

    fn sample(glide: f32, amplitude: f32) -> WoozeSample {
        WoozeSample {
            glide,
            amplitude,
            ..WoozeSample::NEUTRAL
        }
    }

    #[test]
    fn warp_is_the_design_formula() {
        assert_eq!(warp(3.2, 0.3), (3, 0.0), "holding");
        assert_eq!(warp(3.85, 0.3), (3, 0.5), "halfway through the glide");
        assert_eq!(warp(4.0, 0.3), (4, 0.0), "the next pose, at rest");
        assert_eq!(warp(3.99, 0.0), (3, 0.0), "glide 0 only moves on the beat");
        assert_eq!(warp(-1.0, 0.3), (0, 0.0));
        let (_, midway) = warp(0.5, 1.0);
        assert!((midway - 0.5).abs() < 1e-6);
    }

    /// Where a part's value is, as one number: this is the design's t'.
    fn t_prime(now: CycleNow) -> f64 {
        now.index as f64 + f64::from(now.weight)
    }

    #[test]
    fn a_new_glide_waits_for_the_next_cycle_so_t_prime_never_jumps() {
        let mut part = Cycle::default();
        let calm = sample(0.3, 1.0);
        read_cycle(&mut part, 0.0, &calm);

        // At frac 0.5 a glide of 0.3 is holding and one of 0.8 would be well
        // under way. Nothing moves until the cycle ends.
        let held = read_cycle(&mut part, 0.5, &calm);
        let dreamy = sample(0.8, 1.0);
        assert_eq!(read_cycle(&mut part, 0.5, &dreamy), held);
        assert_eq!(read_cycle(&mut part, 0.69, &dreamy).weight, 0.0);

        // And the cycle after it runs on the new glide.
        assert_eq!(read_cycle(&mut part, 1.0, &dreamy).index, 1);
        assert_eq!(read_cycle(&mut part, 1.5, &dreamy).weight, warp(1.5, 0.8).1);
    }

    #[test]
    fn t_prime_stays_continuous_through_any_change_of_glide() {
        let mut part = Cycle::default();
        let glides = [0.3, 0.8, 1.0, 0.5, 0.2, 0.9];
        let mut last = t_prime(read_cycle(&mut part, 0.0, &sample(0.3, 1.0)));
        for frame in 1..6000 {
            let beats = f64::from(frame) * 0.003;
            // A new value every 37 frames, whichever part of the cycle that is.
            let glide = glides[(frame / 37) as usize % glides.len()];
            let now = t_prime(read_cycle(&mut part, beats, &sample(glide, 1.0)));
            assert!(now >= last, "went back at frame {frame}: {last} to {now}");
            // A smoothstep's top speed at the shortest glide here, 0.2, a beat
            // and 0.003 beats a frame: about 0.0225. And a little for rounding.
            assert!(
                now - last < 0.0226,
                "jumped at frame {frame}: {last} to {now}"
            );
            last = now;
        }
    }

    #[test]
    fn a_new_amplitude_arrives_with_the_next_pose_and_the_beat_moves_nothing() {
        let mut part = Cycle::default();
        let shown = |part: &mut Cycle, beats: f64, s: &WoozeSample| {
            let now = read_cycle(part, beats, s);
            part.shown(now.weight)
        };
        assert_eq!(shown(&mut part, 0.0, &sample(0.5, 1.0)), 1.0);

        // This cycle was promised at the old value, all the way to its end.
        let low = sample(0.5, 0.2);
        assert_eq!(shown(&mut part, 0.25, &low), 1.0);
        assert_eq!(shown(&mut part, 0.75, &low), 1.0);

        // The next cycle leaves at what it had and heads for the new value.
        assert_eq!(shown(&mut part, 1.0, &low), 1.0);
        assert!((shown(&mut part, 1.75, &low) - 0.6).abs() < 1e-6);
        let end = shown(&mut part, 1.999_999, &low);
        assert!((end - 0.2).abs() < 1e-3, "{end}");
        // The beat itself: what arrived is at rest at the same amplitude.
        assert_eq!(shown(&mut part, 2.0, &low), 0.2);
    }

    #[test]
    fn a_part_that_skips_a_target_starts_clean() {
        let mut part = Cycle::default();
        read_cycle(&mut part, 0.0, &sample(0.3, 1.0));
        let after = read_cycle(&mut part, 5.0, &sample(0.3, 0.4));
        assert_eq!(part.shown(after.weight), 0.4);
    }

    #[test]
    fn sampling_allocates_no_state_and_is_repeatable() {
        let field = field_with(POCKET);
        let a = field.sample([1.0, 2.0, 3.0], 4.0);
        let b = field.clone().sample([1.0, 2.0, 3.0], 4.0);
        assert_eq!(a, b);
    }
}
