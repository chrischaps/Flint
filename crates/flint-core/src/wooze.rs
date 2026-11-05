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
//! # Changing a knob without a pop
//!
//! Two things keep a change from reading as the very pop this system exists to
//! avoid. A consumer counts its clock in beats and advances it by `dt / step`,
//! so a new step bends the clock from wherever it stands instead of moving it.
//! And [`Cycle`] holds the glide and amplitude a part started its cycle with
//! until that cycle ends, because neither can change mid-cycle continuously:
//! a new glide would move the eased weight, and a new amplitude the whole pose.

use crate::toml_util::{toml_f32, toml_vec3};

/// The shortest step, in seconds. A step under a frame can't be seen anyway, and
/// it asks for poses far faster than any budget can make them.
pub const STEP_MIN: f32 = 0.01;
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

    fn to_array(self) -> [f32; KNOBS] {
        [self.amplitude, self.step, self.glide, self.jitter]
    }

    fn from_array([amplitude, step, glide, jitter]: [f32; KNOBS]) -> Self {
        Self {
            amplitude,
            step,
            glide,
            jitter,
        }
    }
}

/// The built-in presets. `linklater` is a named placeholder, the design's first
/// guess, until the grove finds the real sweet spot (#219).
pub const PRESETS: &[(&str, WoozeSample)] = &[
    (
        "linklater",
        WoozeSample {
            amplitude: 1.0,
            step: 0.25,
            glide: 0.3,
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
const KNOB_NAMES: [&str; KNOBS] = ["amplitude", "step", "glide", "jitter"];

/// Check a knob's value, the same for the scene table, a zone and a script.
fn check_knob(knob: usize, value: f32) -> Result<f32, String> {
    let name = KNOB_NAMES[knob];
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

/// The hold-and-glide clock at `beats`, counted in steps: `(index, weight)` means
/// `weight` of the way from target `index` to the next. The design's t', which
/// holds for the first `1 - glide` of a step and eases over the rest. A glide of
/// 0 never leaves the target and moves to the next on the beat.
pub fn warp(beats: f64, glide: f32) -> (u64, f32) {
    let beats = beats.max(0.0);
    let index = beats.floor();
    let glide = f64::from(glide);
    let eased = if glide <= 1e-6 {
        0.0
    } else {
        let x = ((beats - index - (1.0 - glide)) / glide).clamp(0.0, 1.0);
        x * x * (3.0 - 2.0 * x)
    };
    (index as u64, eased as f32)
}

/// One part's way through its current cycle, as of when the cycle began.
///
/// `step` bends the clock and can change at any moment, but a part must finish a
/// cycle on the glide and amplitude it started it with, or its eased weight and
/// its pose would jump. So a part reads the field once a cycle, on its own beat,
/// which is also what lets a whole staggered tree take a change one part at a
/// time and never all at once.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Cycle {
    /// The target being left, or `None` before the first read.
    index: Option<u64>,
    glide: f32,
    /// The amplitude the target being left is shown at.
    from: f32,
    /// The amplitude the next target will arrive at.
    to: f32,
}

impl Cycle {
    /// Where this part stands at `beats`: its target, how far toward the next,
    /// and how much of the pose to show. `sample` is only read when a cycle
    /// begins, so a part's value here can only move as it would have anyway.
    pub fn read(&mut self, beats: f64, sample: &WoozeSample) -> CycleNow {
        let index = beats.max(0.0).floor() as u64;
        if self.index != Some(index) {
            // What's arriving keeps the amplitude it was on its way in with, so
            // the beat itself moves nothing. A part that has skipped a target,
            // or just started, has nothing to keep.
            self.from = if self.index.map(|last| last + 1) == Some(index) {
                self.to
            } else {
                sample.amplitude
            };
            self.to = sample.amplitude;
            self.glide = sample.glide;
            self.index = Some(index);
        }
        let (index, weight) = warp(beats, self.glide);
        CycleNow {
            index,
            weight,
            amplitude: self.from + (self.to - self.from) * weight,
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
    /// How much of the pose to show, 0 to 1.
    pub amplitude: f32,
}

/// A value on its way from one number to another, so that a sample is a pure
/// function of time however a script has moved things.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Ramp {
    from: f32,
    to: f32,
    start: f64,
    seconds: f64,
}

impl Ramp {
    fn constant(value: f32) -> Self {
        Self {
            from: value,
            to: value,
            start: 0.0,
            seconds: 0.0,
        }
    }

    fn at(&self, time: f64) -> f32 {
        if self.seconds <= 0.0 || time >= self.start + self.seconds {
            return self.to;
        }
        let x = ((time - self.start) / self.seconds).max(0.0) as f32;
        self.from + (self.to - self.from) * (x * x * (3.0 - 2.0 * x))
    }

    /// Head for `to` from wherever this stands at `time`, over `seconds`.
    fn head_for(&mut self, to: f32, time: f64, seconds: f64) {
        *self = Self {
            from: self.at(time),
            to,
            start: time,
            seconds: if seconds.is_finite() {
                seconds.max(0.0)
            } else {
                0.0
            },
        };
    }
}

/// What a zone covers.
#[derive(Debug, Clone, Copy, PartialEq)]
enum WoozeShape {
    /// A ball of `radius` metres around the centre.
    Sphere {
        /// Metres from the centre to the edge.
        radius: f32,
    },
    /// An axis-aligned box, `size` metres across on each axis.
    Box {
        /// Full width on each axis, in metres.
        size: [f32; 3],
    },
}

/// One part of the world woozier or calmer than the rest.
#[derive(Debug, Clone, PartialEq)]
pub struct WoozeZone {
    name: String,
    shape: WoozeShape,
    center: [f32; 3],
    /// Metres past the edge over which the zone fades out. 0 is a hard edge.
    falloff: f32,
    /// The knobs it sets; a knob left `None` passes through what's below.
    knobs: [Option<Ramp>; KNOBS],
    /// How much of itself it applies, so it can fade in and out whole.
    strength: Ramp,
}

impl WoozeZone {
    /// Its name, which is how a script finds it. Unnamed ones get `zone <n>`.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// How far inside it `position` is: 1 inside, 0 past the falloff.
    fn weight(&self, position: [f32; 3]) -> f32 {
        let d = |axis: usize| position[axis] - self.center[axis];
        let past_edge = match self.shape {
            WoozeShape::Sphere { radius } => {
                (d(0) * d(0) + d(1) * d(1) + d(2) * d(2)).sqrt() - radius
            }
            WoozeShape::Box { size } => {
                let out = |axis: usize| (d(axis).abs() - size[axis] * 0.5).max(0.0);
                (out(0) * out(0) + out(1) * out(1) + out(2) * out(2)).sqrt()
            }
        };
        if past_edge <= 0.0 {
            1.0
        } else if past_edge >= self.falloff {
            0.0
        } else {
            let x = 1.0 - past_edge / self.falloff;
            x * x * (3.0 - 2.0 * x)
        }
    }
}

/// The whole field: global values and zones.
#[derive(Debug, Clone, PartialEq)]
pub struct WoozeField {
    base: [Ramp; KNOBS],
    zones: Vec<WoozeZone>,
}

impl WoozeField {
    /// A field with these global values everywhere and no zones.
    pub fn new(base: WoozeSample) -> Self {
        Self {
            base: base.to_array().map(Ramp::constant),
            zones: Vec::new(),
        }
    }

    /// The zones, in the order they apply.
    pub fn zones(&self) -> &[WoozeZone] {
        &self.zones
    }

    /// How woozy `position` is at `time`, in whichever clock the caller keeps
    /// this field on. A pure function: the same arguments give the same bits.
    pub fn sample(&self, position: [f32; 3], time: f64) -> WoozeSample {
        let mut value = self.base.map(|ramp| ramp.at(time));
        for zone in &self.zones {
            let weight = zone.weight(position) * zone.strength.at(time);
            if weight <= 0.0 {
                continue;
            }
            for (out, knob) in value.iter_mut().zip(&zone.knobs) {
                if let Some(knob) = knob {
                    // Exact inside, so a zone's value is that value to the bit.
                    let target = knob.at(time);
                    *out = if weight >= 1.0 {
                        target
                    } else {
                        *out + (target - *out) * weight
                    };
                }
            }
        }
        WoozeSample::from_array(value)
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
        let mut targets: [Option<f32>; KNOBS] = [None; KNOBS];
        for (key, value) in table {
            if key == "preset" {
                continue;
            }
            let Some(knob) = KNOB_NAMES.iter().position(|k| k == key) else {
                return Err(format!(
                    "'{key}' isn't a wooze knob; it takes preset, {}",
                    KNOB_NAMES.join(", ")
                ));
            };
            targets[knob] = Some(read_knob(knob, value)?);
        }
        if let Some(value) = table.get("preset") {
            let name = value
                .as_str()
                .ok_or_else(|| format!("preset = {value} must be a name"))?;
            let found = preset(name).ok_or_else(|| unknown_preset(name))?;
            for (target, value) in targets.iter_mut().zip(found.to_array()) {
                target.get_or_insert(value);
            }
        }
        for (ramp, target) in self.base.iter_mut().zip(targets) {
            if let Some(target) = target {
                ramp.head_for(target, time, seconds);
            }
        }
        Ok(())
    }

    /// Add a zone from a `[[wooze.zone]]` table, or an error saying what's wrong
    /// with it. An unnamed one takes a name from its place in the list.
    pub fn add_zone(&mut self, table: &toml::Table) -> Result<(), String> {
        let name = match table.get("name") {
            None => format!("zone {}", self.zones.len() + 1),
            Some(toml::Value::String(name)) if !name.is_empty() => name.clone(),
            Some(other) => return Err(format!("name = {other} must be a name")),
        };
        if self.zones.iter().any(|zone| zone.name == name) {
            return Err("its name is already taken by another zone".to_string());
        }
        let zone = read_zone(name, table, None, &self.base, 0.0, 0.0)?;
        self.zones.push(zone);
        Ok(())
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
        if name.is_empty() {
            return Err("a zone needs a name".to_string());
        }
        self.settle(time);
        let found = self.zones.iter().position(|zone| zone.name == name);
        let old = found.map(|i| &self.zones[i]);
        // A new zone is all there and fades in whole, so only an edit ramps knobs.
        let knob_seconds = if old.is_some() { seconds } else { 0.0 };
        let mut zone = read_zone(name.to_string(), table, old, &self.base, time, knob_seconds)?;
        match found {
            Some(i) => {
                zone.strength.head_for(1.0, time, seconds);
                self.zones[i] = zone;
            }
            None => {
                zone.strength = Ramp::constant(0.0);
                zone.strength.head_for(1.0, time, seconds);
                self.zones.push(zone);
            }
        }
        Ok(())
    }

    /// Fade the named zone out over `seconds` and drop it. Returns whether there
    /// was one.
    pub fn remove_zone(&mut self, name: &str, time: f64, seconds: f64) -> bool {
        self.settle(time);
        let Some(i) = self.zones.iter().position(|zone| zone.name == name) else {
            return false;
        };
        if seconds > 0.0 {
            self.zones[i].strength.head_for(0.0, time, seconds);
        } else {
            self.zones.remove(i);
        }
        true
    }

    /// Drop the zones that have finished fading out.
    fn settle(&mut self, time: f64) {
        self.zones.retain(|zone| {
            zone.strength.to > 0.0 || time < zone.strength.start + zone.strength.seconds
        });
    }
}

fn unknown_preset(name: &str) -> String {
    format!("preset '{name}' isn't one of {}", preset_names())
}

fn read_knob(knob: usize, value: &toml::Value) -> Result<f32, String> {
    let number = toml_f32(value)
        .ok_or_else(|| format!("{} = {value} must be a number", KNOB_NAMES[knob]))?;
    check_knob(knob, number)
}

/// Every key a `[[wooze.zone]]` table takes.
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

/// A zone from its table, with `old` filling in anything left out. A knob that's
/// new starts from the global value, so a zone's new knob eases in from what
/// that place was, save where zones overlap.
fn read_zone(
    name: String,
    table: &toml::Table,
    old: Option<&WoozeZone>,
    base: &[Ramp; KNOBS],
    time: f64,
    seconds: f64,
) -> Result<WoozeZone, String> {
    for key in table.keys() {
        if !ZONE_KEYS.contains(&key.as_str()) {
            return Err(format!(
                "'{key}' isn't a zone key; it takes {}",
                ZONE_KEYS.join(", ")
            ));
        }
    }

    let center = match table.get("center") {
        Some(value) => {
            read_point(value).ok_or_else(|| format!("center = {value} must be [x, y, z]"))?
        }
        None => old
            .map(|z| z.center)
            .ok_or("it needs a center: [x, y, z]")?,
    };
    let falloff = match table.get("falloff") {
        Some(value) => toml_f32(value)
            .filter(|f| f.is_finite() && *f >= 0.0)
            .ok_or_else(|| format!("falloff = {value} must be 0 metres or more"))?,
        None => old.map_or(0.0, |z| z.falloff),
    };
    let shape = read_shape(table, old)?;

    let mut knobs = old.map_or([None; KNOBS], |z| z.knobs);
    for (i, slot) in knobs.iter_mut().enumerate() {
        if let Some(value) = table.get(KNOB_NAMES[i]) {
            let target = read_knob(i, value)?;
            let mut ramp = slot.unwrap_or_else(|| Ramp::constant(base[i].at(time)));
            ramp.head_for(target, time, seconds);
            *slot = Some(ramp);
        }
    }
    if knobs.iter().all(Option::is_none) {
        return Err(format!(
            "it changes nothing: it needs at least one of {}",
            KNOB_NAMES.join(", ")
        ));
    }

    Ok(WoozeZone {
        name,
        shape,
        center,
        falloff,
        knobs,
        strength: old.map_or(Ramp::constant(1.0), |z| z.strength),
    })
}

fn read_shape(table: &toml::Table, old: Option<&WoozeZone>) -> Result<WoozeShape, String> {
    let name = match table.get("shape") {
        Some(toml::Value::String(name)) => name.as_str(),
        Some(other) => return Err(format!("shape = {other} must be \"sphere\" or \"box\"")),
        None => match old.map(|z| z.shape) {
            Some(WoozeShape::Sphere { .. }) => "sphere",
            Some(WoozeShape::Box { .. }) => "box",
            None => return Err("it needs a shape: \"sphere\" or \"box\"".to_string()),
        },
    };
    match name {
        "sphere" => {
            if table.contains_key("size") {
                return Err("a sphere takes radius, not size".to_string());
            }
            let radius = match (table.get("radius"), old.map(|z| z.shape)) {
                (Some(value), _) => toml_f32(value)
                    .filter(|r| r.is_finite() && *r > 0.0)
                    .ok_or_else(|| format!("radius = {value} must be more than 0 metres"))?,
                (None, Some(WoozeShape::Sphere { radius })) => radius,
                (None, _) => return Err("a sphere needs a radius".to_string()),
            };
            Ok(WoozeShape::Sphere { radius })
        }
        "box" => {
            if table.contains_key("radius") {
                return Err("a box takes size, not radius".to_string());
            }
            let size = match (table.get("size"), old.map(|z| z.shape)) {
                (Some(value), _) => read_point(value)
                    .filter(|s| s.iter().all(|a| a.is_finite() && *a > 0.0))
                    .ok_or_else(|| {
                        format!("size = {value} must be [x, y, z], each more than 0 metres")
                    })?,
                (None, Some(WoozeShape::Box { size })) => size,
                (None, _) => return Err("a box needs a size: [x, y, z]".to_string()),
            };
            Ok(WoozeShape::Box { size })
        }
        other => Err(format!("shape = \"{other}\" must be \"sphere\" or \"box\"")),
    }
}

/// Exactly three finite numbers, unlike [`toml_vec3`] alone, which takes more.
fn read_point(value: &toml::Value) -> Option<[f32; 3]> {
    if value.as_array()?.len() != 3 {
        return None;
    }
    toml_vec3(value).filter(|p| p.iter().all(|a| a.is_finite()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table(src: &str) -> toml::Table {
        src.parse().unwrap()
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
        field.settle(23.0);
        assert!(field.zones().is_empty());
        assert!(!field.remove_zone("late", 23.0, 0.0));
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
        part.read(0.0, &calm);

        // At frac 0.5 a glide of 0.3 is holding and one of 0.8 would be well
        // under way. Nothing moves until the cycle ends.
        let held = part.read(0.5, &calm);
        let dreamy = sample(0.8, 1.0);
        assert_eq!(part.read(0.5, &dreamy), held);
        assert_eq!(part.read(0.69, &dreamy).weight, 0.0);

        // And the cycle after it runs on the new glide.
        assert_eq!(part.read(1.0, &dreamy).index, 1);
        assert_eq!(part.read(1.5, &dreamy).weight, warp(1.5, 0.8).1);
    }

    #[test]
    fn t_prime_stays_continuous_through_any_change_of_glide() {
        let mut part = Cycle::default();
        let glides = [0.3, 0.8, 1.0, 0.5, 0.2, 0.9];
        let mut last = t_prime(part.read(0.0, &sample(0.3, 1.0)));
        for frame in 1..6000 {
            let beats = f64::from(frame) * 0.003;
            // A new value every 37 frames, whichever part of the cycle that is.
            let glide = glides[(frame / 37) as usize % glides.len()];
            let now = t_prime(part.read(beats, &sample(glide, 1.0)));
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
        assert_eq!(part.read(0.0, &sample(0.5, 1.0)).amplitude, 1.0);

        // This cycle was promised at the old value, all the way to its end.
        let low = sample(0.5, 0.2);
        assert_eq!(part.read(0.25, &low).amplitude, 1.0);
        assert_eq!(part.read(0.75, &low).amplitude, 1.0);

        // The next cycle leaves at what it had and heads for the new value.
        assert_eq!(part.read(1.0, &low).amplitude, 1.0);
        assert!((part.read(1.75, &low).amplitude - 0.6).abs() < 1e-6);
        let end = part.read(1.999_999, &low).amplitude;
        assert!((end - 0.2).abs() < 1e-3, "{end}");
        // The beat itself: what arrived is at rest at the same amplitude.
        assert_eq!(part.read(2.0, &low).amplitude, 0.2);
    }

    #[test]
    fn a_part_that_skips_a_target_starts_clean() {
        let mut part = Cycle::default();
        part.read(0.0, &sample(0.3, 1.0));
        let after = part.read(5.0, &sample(0.3, 0.4));
        assert_eq!(after.amplitude, 0.4);
    }

    #[test]
    fn sampling_allocates_no_state_and_is_repeatable() {
        let field = field_with(POCKET);
        let a = field.sample([1.0, 2.0, 3.0], 4.0);
        let b = field.clone().sample([1.0, 2.0, 3.0], 4.0);
        assert_eq!(a, b);
    }
}
