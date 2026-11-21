//! A cross-fade from one look to the next (#496): `set_look(name, seconds)`
//! and F4's Look picker with a fade time ease the drawn post-processing from
//! the config it had to the one the new look resolves to, instead of cutting.
//!
//! A [`LookFade`] holds both ends and its progress, and lays the frame in
//! between over the renderer's config with [`LookFade::apply`]. It writes
//! only what the two ends set differently, so whatever else moves the config
//! during the fade (a script's per-frame overrides, laid after it; the boil's
//! clock; a style a script tweens that both looks leave alone) keeps moving.
//! At the end it has written the new look's values exactly, and a fade of 0
//! seconds is the cut `set_look` always made.
//!
//! How each part crosses, at the eased progress `s` (smoothstep of time):
//!
//! - **Numbers and colours** lerp: every `f32` and `rgb` key of the built-in
//!   effects ([`flint_schema::post_effects`]), the ground fog's.
//! - **An effect switched on or off** (bloom, SSAO, fog, vignette...) stays on
//!   through the fade and its strength ramps from or to 0
//!   ([`TOGGLE_STRENGTH`]), so the vignette of a look that has one darkens in
//!   rather than appearing at the midpoint.
//! - **Whole numbers and other switches** (sample counts, FXAA, `boil_on`)
//!   change at the midpoint, where they matter least.
//! - **A style in both looks**: its parameters lerp (one the table doesn't set
//!   reads as the style's default), its `blend` lerps, and its options (a
//!   ramp, a palette, a font), place in the chain and reveal change at the
//!   midpoint: there is one of each style, so they can't be drawn both ways.
//! - **A style in one look only** fades by its whole-frame blend (#399): the
//!   outgoing look's from its blend to 0, the incoming one's from 0 to its
//!   blend. At 0 a style leaves its chain, so both ends of the fade are the
//!   frame without it, byte for byte. Mid-fade the two chain as the stage
//!   orders them, the later reading the earlier's half-faded frame: the
//!   outgoing look's style fades out under the incoming one.

use crate::stylize::{self, StyleSetting, StyleSettings};
use crate::PostProcessConfig;
use flint_schema::post_effects::{self, PostParamMut, PostValue};

/// The effects whose switch turning fades their strength instead (an effect
/// toggled on or off between two looks): the switch key and the `f32` key at
/// whose 0 the effect draws nothing. While one end has the effect off, the
/// fade keeps it on and runs that end's strength as 0.
pub const TOGGLE_STRENGTH: &[(&str, &str)] = &[
    ("vignette_enabled", "vignette_intensity"),
    ("bloom_enabled", "bloom_intensity"),
    ("ssao_enabled", "ssao_intensity"),
    ("fog_enabled", "fog_density"),
    ("fog_height_enabled", "fog_height_density"),
    ("volumetric_enabled", "volumetric_density"),
    ("dither_enabled", "dither_intensity"),
];

/// A fade in progress from one post config to another.
#[derive(Debug, Clone)]
pub struct LookFade {
    from: PostProcessConfig,
    to: PostProcessConfig,
    /// The registry the two ends were read against: an unset parameter
    /// fades from or to its style's default.
    styles: stylize::StyleRegistry,
    seconds: f32,
    elapsed: f32,
}

impl LookFade {
    /// A fade from `from` (what the frame drew when it was asked) to `to`
    /// (what the new look resolves to) over `seconds`. Nothing has moved yet.
    pub fn new(
        from: PostProcessConfig,
        to: PostProcessConfig,
        seconds: f32,
        styles: &stylize::StyleRegistry,
    ) -> Self {
        Self {
            from,
            to,
            styles: styles.clone(),
            seconds: if seconds.is_finite() {
                seconds.max(0.0)
            } else {
                0.0
            },
            elapsed: 0.0,
        }
    }

    /// Move the fade on by `dt` seconds.
    pub fn step(&mut self, dt: f32) {
        if dt.is_finite() && dt > 0.0 {
            self.elapsed = (self.elapsed + dt).min(self.seconds);
        }
    }

    /// Put the fade `elapsed` seconds after it began (held to 0 and its
    /// length). A host that keeps the game time a fade was asked at lays it
    /// from that, so the player and a headless capture reach the same moment
    /// of it at the same game time (#1157), whatever steps came between.
    pub fn set_elapsed(&mut self, elapsed: f32) {
        if elapsed.is_finite() {
            self.elapsed = elapsed.clamp(0.0, self.seconds);
        }
    }

    /// The fade as it was (or will be) `elapsed` seconds after it began.
    pub fn at(&self, elapsed: f32) -> Self {
        let mut fade = self.clone();
        fade.set_elapsed(elapsed);
        fade
    }

    /// How far it has gone in time, 0 to 1 (1 for a fade of no length).
    pub fn progress(&self) -> f32 {
        if self.seconds <= 0.0 {
            1.0
        } else {
            (self.elapsed / self.seconds).clamp(0.0, 1.0)
        }
    }

    /// The eased share of the way from `from` to `to`: smoothstep of
    /// [`progress`](Self::progress), slow out of the old look and into the new.
    pub fn eased(&self) -> f32 {
        flint_core::tween::Easing::Smooth.apply_f32(self.progress())
    }

    /// Whether the fade has reached the new look.
    pub fn done(&self) -> bool {
        self.progress() >= 1.0
    }

    /// The fade's length in seconds.
    pub fn seconds(&self) -> f32 {
        self.seconds
    }

    /// The config the fade ends on.
    pub fn target(&self) -> &PostProcessConfig {
        &self.to
    }

    /// The config the fade started from.
    pub fn origin(&self) -> &PostProcessConfig {
        &self.from
    }

    /// Lay the fade at its progress over `config`: every field the two ends
    /// set differently takes its in-between value, and nothing else changes.
    pub fn apply(&self, config: &mut PostProcessConfig) {
        lay_between(&self.from, &self.to, self.eased(), config, &self.styles);
    }
}

/// Lay the in-between of `from` and `to` at `s` (0 is `from`, 1 is `to`) over
/// `config`, field by field where the two differ ([`LookFade`] says how each
/// part crosses).
pub fn lay_between(
    from: &PostProcessConfig,
    to: &PostProcessConfig,
    s: f32,
    config: &mut PostProcessConfig,
    styles: &stylize::StyleRegistry,
) {
    let s = s.clamp(0.0, 1.0);
    let past_half = s >= 0.5;
    for param in post_effects::params() {
        if TOGGLE_STRENGTH
            .iter()
            .any(|(switch, strength)| param.key == *switch || param.key == *strength)
        {
            continue; // laid together, below
        }
        let (Some(a), Some(b)) = (from.param(param.key), to.param(param.key)) else {
            continue;
        };
        if a == b {
            continue;
        }
        if let Some(field) = config.param_mut(param.key) {
            set_between(field, a, b, s, past_half);
        }
    }
    for (switch, strength) in TOGGLE_STRENGTH {
        lay_toggled(from, to, switch, strength, s, config);
    }
    if from.enabled != to.enabled {
        config.enabled = if past_half { to.enabled } else { from.enabled };
    }
    if from.ground_fog_density != to.ground_fog_density {
        config.ground_fog_density = lerp(from.ground_fog_density, to.ground_fog_density, s);
    }
    if from.ground_fog_falloff != to.ground_fog_falloff {
        config.ground_fog_falloff = lerp(from.ground_fog_falloff, to.ground_fog_falloff, s);
    }
    if from.ground_fog_color != to.ground_fog_color {
        config.ground_fog_color = lerp3(from.ground_fog_color, to.ground_fog_color, s);
    }
    if from.ground_fog_steps != to.ground_fog_steps {
        config.ground_fog_steps = if past_half {
            to.ground_fog_steps
        } else {
            from.ground_fog_steps
        };
    }
    lay_styles(&from.styles, &to.styles, s, &mut config.styles, styles);
}

/// One effect's switch and strength: on at both ends, the strength lerps; on
/// at one, the effect stays on and the strength runs from or to 0; off at
/// both, the strength switches at the midpoint (it draws nothing either way).
fn lay_toggled(
    from: &PostProcessConfig,
    to: &PostProcessConfig,
    switch: &str,
    strength: &str,
    s: f32,
    config: &mut PostProcessConfig,
) {
    let on = |c: &PostProcessConfig| matches!(c.param(switch), Some(PostValue::Bool(true)));
    let amount = |c: &PostProcessConfig| match c.param(strength) {
        Some(PostValue::F32(v)) => v,
        _ => 0.0,
    };
    let (on_a, on_b) = (on(from), on(to));
    let (a, b) = (amount(from), amount(to));
    if on_a == on_b && a == b {
        return;
    }
    let (enabled, value) = match (on_a, on_b) {
        (true, true) => (true, lerp(a, b, s)),
        (true, false) if s < 1.0 => (true, lerp(a, 0.0, s)),
        (false, true) if s > 0.0 => (true, lerp(0.0, b, s)),
        _ => (
            if s >= 1.0 { on_b } else { on_a },
            if s >= 0.5 { b } else { a },
        ),
    };
    if let Some(PostParamMut::Bool(field)) = config.param_mut(switch) {
        *field = enabled;
    }
    if let Some(PostParamMut::F32(field)) = config.param_mut(strength) {
        *field = value;
    }
}

/// Every style the two ends set differently.
fn lay_styles(
    from: &StyleSettings,
    to: &StyleSettings,
    s: f32,
    config: &mut StyleSettings,
    styles: &stylize::StyleRegistry,
) {
    let mut names: Vec<&str> = from.iter().map(|(n, _)| n).collect();
    for (name, _) in to.iter() {
        if !names.contains(&name) {
            names.push(name);
        }
    }
    for name in names {
        if from.same_style(name, to) {
            continue;
        }
        config.put(
            name,
            style_between(styles, name, from.get(name), to.get(name), s),
        );
    }
}

/// Style `name`'s setting at `s` between `a` and `b` (either may be none).
fn style_between(
    styles: &stylize::StyleRegistry,
    name: &str,
    a: Option<&StyleSetting>,
    b: Option<&StyleSetting>,
    s: f32,
) -> Option<StyleSetting> {
    let shows = |x: Option<&StyleSetting>| x.is_some_and(|x| x.enabled);
    let blend = |x: &StyleSetting| x.blend.map_or(1.0, |b| b.clamp(0.0, 1.0));
    let with_blend = |x: &StyleSetting, v: f32| {
        let mut x = x.clone();
        x.blend = (v < 1.0).then_some(v);
        Some(x)
    };
    match (a, b) {
        (Some(a), Some(b)) if shows(Some(a)) && shows(Some(b)) => {
            if s <= 0.0 {
                return Some(a.clone());
            }
            if s >= 1.0 {
                return Some(b.clone());
            }
            let mut mid = if s >= 0.5 { b.clone() } else { a.clone() };
            let descriptor = styles.find(name);
            let default = |key: &str| {
                descriptor
                    .and_then(|d| d.params.iter().find(|p| p.name == key))
                    .map(|p| p.default)
            };
            let mut keys: Vec<&String> = a.params.keys().collect();
            keys.extend(b.params.keys().filter(|k| !a.params.contains_key(*k)));
            for key in keys {
                let end = |x: &StyleSetting| x.params.get(key).copied().or_else(|| default(key));
                if let (Some(va), Some(vb)) = (end(a), end(b)) {
                    mid.params.insert(key.clone(), lerp(va, vb, s));
                }
            }
            mid.blend = Some(lerp(blend(a), blend(b), s)).filter(|v| *v < 1.0);
            Some(mid)
        }
        (Some(a), _) if shows(Some(a)) && s < 1.0 => with_blend(a, blend(a) * (1.0 - s)),
        (_, Some(b)) if shows(Some(b)) && s > 0.0 => with_blend(b, blend(b) * s),
        _ => {
            if s >= 0.5 {
                b.cloned()
            } else {
                a.cloned()
            }
        }
    }
}

fn set_between(field: PostParamMut<'_>, a: PostValue, b: PostValue, s: f32, past_half: bool) {
    match (field, a, b) {
        (PostParamMut::F32(f), PostValue::F32(a), PostValue::F32(b)) => *f = lerp(a, b, s),
        (PostParamMut::Rgb(f), PostValue::Rgb(a), PostValue::Rgb(b)) => *f = lerp3(a, b, s),
        (PostParamMut::Bool(f), PostValue::Bool(a), PostValue::Bool(b)) => {
            *f = if past_half { b } else { a }
        }
        (PostParamMut::U32(f), PostValue::U32(a), PostValue::U32(b)) => {
            *f = if past_half { b } else { a }
        }
        _ => {}
    }
}

fn lerp(a: f32, b: f32, s: f32) -> f32 {
    if s <= 0.0 {
        a
    } else if s >= 1.0 {
        b
    } else {
        a + (b - a) * s
    }
}

fn lerp3(a: [f32; 3], b: [f32; 3], s: f32) -> [f32; 3] {
    [
        lerp(a[0], b[0], s),
        lerp(a[1], b[1], s),
        lerp(a[2], b[2], s),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The config has no `PartialEq` (its boil frame); its `Debug` is every field.
    fn same(a: &PostProcessConfig, b: &PostProcessConfig) -> bool {
        format!("{a:?}") == format!("{b:?}")
    }

    fn looks() -> (PostProcessConfig, PostProcessConfig) {
        let mut a = PostProcessConfig {
            exposure: 1.0,
            vignette_enabled: false,
            ssao_samples: 16,
            ..Default::default()
        };
        a.styles.set_enabled("pencil", true);
        a.styles.set_param("pencil", "spacing", 4.0);
        a.styles.set_enabled("watercolor", false);
        let mut b = PostProcessConfig {
            exposure: 2.0,
            vignette_enabled: true,
            vignette_intensity: 0.4,
            ssao_samples: 32,
            ..Default::default()
        };
        b.styles.set_enabled("watercolor", true);
        b.styles.set_param("watercolor", "layers", 5.0);
        (a, b)
    }

    #[test]
    fn the_ends_are_the_two_looks() {
        let (a, b) = looks();
        let mut at_start = a.clone();
        lay_between(
            &a,
            &b,
            0.0,
            &mut at_start,
            &crate::stylize::StyleRegistry::new(),
        );
        assert!(same(&at_start, &a), "nothing moves at 0");
        let mut at_end = a.clone();
        lay_between(
            &a,
            &b,
            1.0,
            &mut at_end,
            &crate::stylize::StyleRegistry::new(),
        );
        assert!(same(&at_end, &b), "the fade ends on the new look exactly");
    }

    #[test]
    fn numbers_lerp_and_whole_numbers_switch_at_the_midpoint() {
        let (a, b) = looks();
        let mut mid = a.clone();
        lay_between(
            &a,
            &b,
            0.25,
            &mut mid,
            &crate::stylize::StyleRegistry::new(),
        );
        assert!((mid.exposure - 1.25).abs() < 1e-6);
        assert_eq!(mid.ssao_samples, 16);
        lay_between(
            &a,
            &b,
            0.75,
            &mut mid,
            &crate::stylize::StyleRegistry::new(),
        );
        assert!((mid.exposure - 1.75).abs() < 1e-6);
        assert_eq!(mid.ssao_samples, 32);
    }

    #[test]
    fn an_effect_switched_on_ramps_its_strength_from_zero() {
        let (a, b) = looks();
        let mut mid = a.clone();
        lay_between(&a, &b, 0.5, &mut mid, &crate::stylize::StyleRegistry::new());
        assert!(mid.vignette_enabled, "on through the fade");
        assert!((mid.vignette_intensity - 0.2).abs() < 1e-6);
        let mut back = b.clone();
        lay_between(
            &b,
            &a,
            0.5,
            &mut back,
            &crate::stylize::StyleRegistry::new(),
        );
        assert!(back.vignette_enabled);
        assert!((back.vignette_intensity - 0.2).abs() < 1e-6);
        lay_between(
            &b,
            &a,
            1.0,
            &mut back,
            &crate::stylize::StyleRegistry::new(),
        );
        assert!(!back.vignette_enabled, "and off once it has faded out");
        assert_eq!(back.vignette_intensity, a.vignette_intensity);
    }

    #[test]
    fn a_style_in_one_look_fades_by_its_blend() {
        let (a, b) = looks();
        let mut mid = a.clone();
        lay_between(
            &a,
            &b,
            0.25,
            &mut mid,
            &crate::stylize::StyleRegistry::new(),
        );
        assert!(mid.styles.is_enabled("pencil"));
        assert!((mid.styles.blend("pencil") - 0.75).abs() < 1e-6);
        assert!(mid.styles.is_enabled("watercolor"));
        assert!((mid.styles.blend("watercolor") - 0.25).abs() < 1e-6);
        assert_eq!(mid.styles.param("watercolor", "layers"), Some(5.0));
        assert_eq!(mid.styles.param("pencil", "spacing"), Some(4.0));
    }

    #[test]
    fn a_style_in_both_lerps_its_params_from_the_default_and_switches_options() {
        let mut a = PostProcessConfig::default();
        a.styles.set_enabled("pencil", true);
        a.styles.set_option("pencil", "direction", "screen");
        let mut b = a.clone();
        b.styles.set_param("pencil", "spacing", 9.0);
        b.styles.set_option("pencil", "direction", "tensor");
        b.styles.set_blend("pencil", Some(0.5));
        let default = stylize::builtin("pencil")
            .and_then(|d| d.params.iter().find(|p| p.name == "spacing"))
            .map(|p| p.default)
            .unwrap();
        let mut mid = a.clone();
        lay_between(
            &a,
            &b,
            0.25,
            &mut mid,
            &crate::stylize::StyleRegistry::new(),
        );
        let want = default + (9.0 - default) * 0.25;
        assert!((mid.styles.param("pencil", "spacing").unwrap() - want).abs() < 1e-5);
        assert_eq!(mid.styles.option("pencil", "direction"), Some("screen"));
        assert!((mid.styles.blend("pencil") - 0.875).abs() < 1e-6);
        lay_between(
            &a,
            &b,
            0.75,
            &mut mid,
            &crate::stylize::StyleRegistry::new(),
        );
        assert_eq!(mid.styles.option("pencil", "direction"), Some("tensor"));
    }

    #[test]
    fn only_what_the_looks_differ_in_is_written() {
        let (a, b) = looks();
        let mut live = a.clone();
        // A script's own value on a key both looks leave alone
        live.chromatic_aberration = 0.02;
        live.styles.set_enabled("invert", true);
        lay_between(
            &a,
            &b,
            0.5,
            &mut live,
            &crate::stylize::StyleRegistry::new(),
        );
        assert_eq!(live.chromatic_aberration, 0.02);
        assert!(live.styles.is_enabled("invert"));
    }

    #[test]
    fn a_fade_steps_on_its_clock_and_eases() {
        let (a, b) = looks();
        let mut fade = LookFade::new(
            a.clone(),
            b.clone(),
            2.0,
            &crate::stylize::StyleRegistry::new(),
        );
        assert_eq!(fade.eased(), 0.0);
        fade.step(1.0);
        assert!((fade.progress() - 0.5).abs() < 1e-6);
        assert!((fade.eased() - 0.5).abs() < 1e-6);
        fade.step(0.5);
        assert!(fade.eased() > 0.75 && fade.eased() < 0.9, "smoothstep");
        assert!(!fade.done());
        fade.step(5.0);
        assert!(fade.done());
        let mut live = a.clone();
        fade.apply(&mut live);
        assert!(same(&live, &b));
        assert!(
            LookFade::new(a, b, 0.0, &crate::stylize::StyleRegistry::new()).done(),
            "no length is a cut"
        );
    }

    #[test]
    fn every_toggle_names_a_switch_and_a_strength() {
        for (switch, strength) in TOGGLE_STRENGTH {
            let s = post_effects::param(switch).expect(switch);
            let k = post_effects::param(strength).expect(strength);
            assert_eq!(s.kind, post_effects::PostKind::Bool, "{switch}");
            assert_eq!(k.kind, post_effects::PostKind::F32, "{strength}");
            assert_eq!(s.effect, k.effect, "{switch} and {strength}");
            assert_eq!(k.min, 0.0, "{strength} draws nothing at 0");
        }
    }
}
