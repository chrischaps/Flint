//! The built-in post effects' parameters (#741): every flat key of a
//! scene's `[post_process]` table, written once.
//!
//! [`post_effects!`] is the one list. It hands each effect (bloom, SSAO,
//! fog, ...) and each parameter (its key, kind, default, the range it is
//! tuned for, its F4 widget, label and help) to a macro of the caller's, so
//! the crates that need the keys as Rust fields build them from it:
//!
//! - here, [`EFFECTS`]: the table `flint render --list-effects`, the range
//!   checks and the `[table.post_process]` schema's fields read;
//! - `flint_scene::PostProcessDef`, which a scene's table parses into;
//! - `flint_render::PostProcessConfig`, which the renderer draws with, and
//!   the copy from the one to the other;
//! - the F4 panel's sliders, through [`PostParamMut`].
//!
//! Adding a parameter is a line here, a field of the renderer's uniform
//! struct and the WGSL that reads it. A key's range is what the effect is
//! tuned for, as a style parameter's is (#355): a value past it still
//! draws, and the scene's load, `flint check` and a look say so.

use crate::{FieldSchema, FieldType};

/// What a parameter holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PostKind {
    Bool,
    F32,
    /// A whole number (a sample or step count, a hold).
    U32,
    /// Three numbers: a linear colour or a per-channel grade.
    Rgb,
}

impl PostKind {
    /// The kind's name in a listing: `bool`, `f32`, `u32`, `rgb`.
    pub fn name(self) -> &'static str {
        match self {
            PostKind::Bool => "bool",
            PostKind::F32 => "f32",
            PostKind::U32 => "u32",
            PostKind::Rgb => "rgb",
        }
    }
}

/// A parameter's value.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize)]
#[serde(untagged)]
pub enum PostValue {
    Bool(bool),
    F32(f32),
    U32(u32),
    Rgb([f32; 3]),
}

/// An f32 as TOML: its shortest decimal, `0.04`, not the
/// `0.03999999910593033` a widening cast gives. It reads back as the same
/// f32. The scene's JSON Schema hands a default to an editor, whose
/// completion inserts it as written (#1062).
fn float(v: f32) -> toml::Value {
    toml::Value::Float(v.to_string().parse().unwrap_or(v as f64))
}

impl PostValue {
    /// The value as TOML, as a scene writes it.
    pub fn to_toml(self) -> toml::Value {
        match self {
            PostValue::Bool(b) => toml::Value::Boolean(b),
            PostValue::F32(v) => float(v),
            PostValue::U32(v) => toml::Value::Integer(v as i64),
            PostValue::Rgb(rgb) => toml::Value::Array(rgb.iter().map(|&c| float(c)).collect()),
        }
    }

    /// The numbers in it: one, three for a colour, none for a bool.
    pub fn numbers(self) -> Vec<f32> {
        match self {
            PostValue::Bool(_) => Vec::new(),
            PostValue::F32(v) => vec![v],
            PostValue::U32(v) => vec![v as f32],
            PostValue::Rgb(rgb) => rgb.to_vec(),
        }
    }
}

impl std::fmt::Display for PostValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PostValue::Bool(b) => write!(f, "{b}"),
            PostValue::F32(v) => write!(f, "{v}"),
            PostValue::U32(v) => write!(f, "{v}"),
            PostValue::Rgb([r, g, b]) => write!(f, "[{r}, {g}, {b}]"),
        }
    }
}

/// The F4 widget a parameter is edited with.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PostWidget {
    /// A checkbox (a bool).
    Toggle,
    /// A slider over the range.
    Slider,
    /// A slider over the range, logarithmic: for a range spanning decades.
    Log,
    /// A whole-number slider over the range.
    Count,
    /// A drag value moving this much a pixel: for a distance with no
    /// natural top.
    Drag(f32),
    /// A colour picker.
    Color,
    /// Three drag values, one a channel, moving this much a pixel.
    Channels(f32),
}

/// A mutable borrow of one parameter's field, by kind: what the F4 panel
/// edits through.
#[derive(Debug)]
pub enum PostParamMut<'a> {
    Bool(&'a mut bool),
    F32(&'a mut f32),
    U32(&'a mut u32),
    Rgb(&'a mut [f32; 3]),
}

impl PostParamMut<'_> {
    /// Set the field from `value`, refused when it is not the field's kind
    /// (an integer is taken as a number, a number as a whole one only if it
    /// is whole and not negative).
    pub fn set(self, value: &toml::Value) -> Result<(), String> {
        let number = |v: &toml::Value| flint_core::toml_util::toml_f64(v);
        match self {
            PostParamMut::Bool(b) => *b = value.as_bool().ok_or("is a bool: true or false")?,
            PostParamMut::F32(f) => *f = number(value).ok_or("is a number")? as f32,
            PostParamMut::U32(u) => {
                let n = number(value).ok_or("is a whole number")?;
                if n < 0.0 || n.fract() != 0.0 || n > u32::MAX as f64 {
                    return Err("is a whole number, 0 or more".into());
                }
                *u = n as u32;
            }
            PostParamMut::Rgb(rgb) => {
                *rgb =
                    flint_core::toml_util::toml_vec3(value).ok_or("is three numbers: [r, g, b]")?;
            }
        }
        Ok(())
    }

    /// Set the field to `value`, exactly, when it is the field's kind
    /// (what a script's `set_post` checked when it was called, #853).
    /// `false`, and the field untouched, when it is another kind.
    pub fn put(self, value: PostValue) -> bool {
        match (self, value) {
            (PostParamMut::Bool(b), PostValue::Bool(v)) => *b = v,
            (PostParamMut::F32(f), PostValue::F32(v)) => *f = v,
            (PostParamMut::U32(u), PostValue::U32(v)) => *u = v,
            (PostParamMut::Rgb(rgb), PostValue::Rgb(v)) => *rgb = v,
            _ => return false,
        }
        true
    }
}

/// One `[post_process]` key.
#[derive(Clone, Copy, Debug, serde::Serialize)]
#[non_exhaustive]
pub struct PostParam {
    /// The key, as the scene, `--sweep post.<key>` and the renderer's
    /// config field all spell it.
    pub key: &'static str,
    /// The [`EffectDescriptor::name`] it belongs to.
    pub effect: &'static str,
    pub kind: PostKind,
    pub default: PostValue,
    /// What the effect is tuned for (each channel, for a colour). A bool's
    /// is 0..1 and means nothing.
    pub min: f32,
    pub max: f32,
    pub widget: PostWidget,
    /// The F4 label, short: the section names the effect.
    pub label: &'static str,
    pub help: &'static str,
}

impl PostParam {
    /// `value` as this key's kind, or why it isn't one ("is a number"),
    /// by [`PostParamMut::set`]'s rules: an integer is a number, a whole
    /// number 0 or more is a count.
    pub fn value_of(&self, value: &toml::Value) -> Result<PostValue, String> {
        let mut out = self.default;
        let slot = match &mut out {
            PostValue::Bool(b) => PostParamMut::Bool(b),
            PostValue::F32(f) => PostParamMut::F32(f),
            PostValue::U32(u) => PostParamMut::U32(u),
            PostValue::Rgb(rgb) => PostParamMut::Rgb(rgb),
        };
        slot.set(value)?;
        Ok(out)
    }

    /// Why `value` won't draw as the effect is tuned, when a number of it
    /// lies outside `min..=max`: "bloom_intensity=3 is outside its range
    /// 0..1", as a style parameter says it (#355). `None` inside, and for a
    /// value that is no number (the type check says that).
    pub fn range_error(&self, value: &toml::Value) -> Option<String> {
        if self.kind == PostKind::Bool {
            return None;
        }
        let numbers: Vec<f64> = match value {
            toml::Value::Array(items) => items
                .iter()
                .filter_map(flint_core::toml_util::toml_f64)
                .collect(),
            v => flint_core::toml_util::toml_f64(v).into_iter().collect(),
        };
        // As the f32 the reader stores: `0.7` written is 0.7f32, not past a
        // max of 0.7 that widens to 0.699999988
        let outside = numbers
            .iter()
            .any(|&n| (n as f32) < self.min || (n as f32) > self.max);
        outside.then(|| {
            format!(
                "{}={value} is outside its range {}..{}",
                self.key, self.min, self.max
            )
        })
    }

    /// The key as a `[table.post_process]` field: its type, default and
    /// help, the range said in the help. The range is no `min`/`max` there,
    /// which would make a value past it an error; a whole number's `min` is
    /// 0, as a negative one doesn't parse.
    pub fn field_schema(&self) -> FieldSchema {
        let mut field = FieldSchema::of(match self.kind {
            PostKind::Bool => FieldType::Bool,
            PostKind::F32 => FieldType::F32,
            PostKind::U32 => FieldType::I32,
            PostKind::Rgb => FieldType::Vec3,
        });
        field.default = Some(self.default.to_toml());
        field.description = Some(match self.kind {
            PostKind::Bool => self.help.to_string(),
            _ => format!("{} (tuned for {} to {})", self.help, self.min, self.max),
        });
        if self.kind == PostKind::U32 {
            field.min = Some(0.0);
        }
        field
    }
}

/// One built-in effect and its keys.
#[derive(Clone, Copy, Debug, serde::Serialize)]
#[non_exhaustive]
pub struct EffectDescriptor {
    /// Short and stable: `bloom`, `ssao`, `fog`.
    pub name: &'static str,
    /// The F4 section's title.
    pub label: &'static str,
    pub help: &'static str,
    pub params: &'static [PostParam],
}

/// Every parameter of every effect, in order.
pub fn params() -> impl Iterator<Item = &'static PostParam> {
    EFFECTS.iter().flat_map(|e| e.params.iter())
}

/// The parameter `key`, if a built-in effect has one.
pub fn param(key: &str) -> Option<&'static PostParam> {
    params().find(|p| p.key == key)
}

/// The effect `name`.
pub fn effect(name: &str) -> Option<&'static EffectDescriptor> {
    EFFECTS.iter().find(|e| e.name == name)
}

/// Why `key` is no parameter, and what it might have been: the effect's
/// own keys when it names an effect (`vignette`), else the closest key, then
/// every key. What a script's `set_post` and `flint script check` say
/// (#853).
pub fn unknown_key(key: &str) -> String {
    let first = match effect(key) {
        Some(e) => format!(
            "`{key}` is an effect, not a [post_process] key: its keys are {}",
            e.params
                .iter()
                .map(|p| p.key)
                .collect::<Vec<_>>()
                .join(", ")
        ),
        None => match flint_core::diagnostic::closest(key, params().map(|p| p.key)) {
            Some(near) => format!("`{key}` is no [post_process] key; did you mean `{near}`?"),
            None => format!("`{key}` is no [post_process] key"),
        },
    };
    let stop = if first.ends_with('?') { "" } else { "." };
    format!(
        "{first}{stop} The keys: {} (`flint render --list-effects` says what each does)",
        params().map(|p| p.key).collect::<Vec<_>>().join(", ")
    )
}

/// The range warnings of a raw `[post_process]` table: one a key whose
/// value lies outside what its effect is tuned for. Keys that are no
/// parameter are not this check's (the unknown-field check names them).
pub fn range_warnings(table: &toml::Table) -> Vec<String> {
    table
        .iter()
        .filter_map(|(key, value)| param(key)?.range_error(value))
        .collect()
}

/// Every parameter, for the `[table.post_process]` schema
/// ([`PostParam::field_schema`]). The schema registry adds the ones a
/// loaded `scene/post_process.toml` doesn't declare itself.
pub fn field_schemas() -> impl Iterator<Item = (&'static str, FieldSchema)> {
    params().map(|p| (p.key, p.field_schema()))
}

/// The effects as `flint render --list-effects` prints them: each effect
/// and what it does, then each key with its kind, default, range and help.
pub fn catalog_text() -> String {
    let mut out = String::new();
    for effect in EFFECTS {
        out.push_str(&format!(
            "{}  ({}): {}\n",
            effect.name, effect.label, effect.help
        ));
        for p in effect.params {
            let range = match p.kind {
                PostKind::Bool => String::new(),
                _ => format!("{}..{}", p.min, p.max),
            };
            out.push_str(&format!(
                "  {:<24} {:<4} {:<18} {:<14} {}\n",
                p.key,
                p.kind.name(),
                p.default.to_string(),
                range,
                p.help
            ));
        }
    }
    out.push_str(
        "\nEach is a [post_process] key, a `--sweep post.<key>=...` axis and an F4 widget. \
         A value past its range still draws, and warns (post-range).\n",
    );
    out
}

/// The list itself. `post_effects!(my_macro { anything })` expands to
/// `my_macro! { { anything } <effect> <effect> ... }`, each effect
///
/// ```text
/// name "Label" "Help" {
///     key: kind = default, [min, max] widget; "Label", "Help"
///     ...
/// }
/// ```
///
/// where `kind` is `bool`, `f32`, `u32` or `rgb` ([`post_type!`] makes it
/// a Rust type), the range is left out for a bool, and `widget` is
/// `toggle`, `slider`, `log`, `count`, `color`, `drag(speed)` or
/// `channels(speed)` ([`PostWidget`]). The tokens in braces pass through
/// untouched, for the caller's own fields.
///
/// A key's default is the renderer's and the scene's both; a key written
/// here is a `[post_process]` key, a `PostProcessConfig` field and an F4
/// widget at once.
#[macro_export]
macro_rules! post_effects {
    ($callback:ident { $($pass:tt)* }) => {
        $callback! {
            { $($pass)* }

            exposure "Exposure" "The picture's brightness before tone mapping" {
                exposure: f32 = 1.0, [0.05, 8.0] log;
                    "Exposure", "Exposure multiplier before tone mapping"
            }

            vignette "Vignette" "Corners darkened toward the frame's edge" {
                vignette_enabled: bool = false, toggle;
                    "Enabled", "Whether the corners darken"
                vignette_intensity: f32 = 0.3, [0.0, 1.0] slider;
                    "Intensity", "How dark the corners get"
                vignette_smoothness: f32 = 2.0, [0.25, 5.0] slider;
                    "Smoothness", "How gradually the vignette falls off"
            }

            lens "Lens" "Colour fringing and blur outward from the centre" {
                chromatic_aberration: f32 = 0.0, [0.0, 0.05] slider;
                    "Chromatic aberration", "Colour fringing toward the edges; 0 is off"
                radial_blur: f32 = 0.0, [0.0, 1.0] slider;
                    "Radial blur", "Blur outward from the centre; 0 is off"
            }

            bloom "Bloom" "Bright areas bleed light" {
                bloom_enabled: bool = true, toggle;
                    "Enabled", "Whether bright areas bleed light"
                bloom_intensity: f32 = 0.04, [0.0, 1.0] slider;
                    "Intensity", "How much bloom is added"
                bloom_threshold: f32 = 1.0, [0.0, 4.0] slider;
                    "Threshold", "The brightness above which a pixel blooms"
                bloom_soft_threshold: f32 = 0.5, [0.0, 1.0] slider;
                    "Soft threshold", "How softly blooming starts below the threshold: 0 is a hard cut"            }

            ssao "SSAO" "Screen-space ambient occlusion: creases and corners darken" {
                ssao_enabled: bool = true, toggle;
                    "Enabled", "Screen-space ambient occlusion"
                ssao_radius: f32 = 0.5, [0.05, 4.0] slider;
                    "Radius", "The reach of the occlusion in world units"
                ssao_intensity: f32 = 1.0, [0.0, 4.0] slider;
                    "Intensity", "How dark occluded corners get"
                ssao_bias: f32 = 0.025, [0.0, 0.1] slider;
                    "Bias", "Depth a sample must clear to occlude, against self-shadowing acne"
                ssao_samples: u32 = 64, [1.0, 64.0] count;
                    "Samples", "Hemisphere samples per pixel, 1 to 64 (64 is full quality). The heaviest per-pixel cost in the post stack; 16 is usually indistinguishable on soft or matte scenes and about 4x cheaper"
            }

            dof "Depth of field" "Blur away from a focus distance" {
                dof_strength: f32 = 0.0, [0.0, 2.0] slider;
                    "Strength", "Depth of field blur; 0 is off"
                dof_focus_distance: f32 = 10.0, [0.05, 1000.0] log;
                    "Focus distance", "Metres from the eye that are sharp"
                dof_focus_range: f32 = 5.0, [0.05, 200.0] log;
                    "Focus range", "Metres either side of the focus that stay sharp"
            }

            fog "Fog" "Distance fog, height fog and the aerial tint" {
                fog_enabled: bool = false, toggle;
                    "Enabled", "Distance fog"
                fog_color: rgb = [0.7, 0.75, 0.82], [0.0, 1.0] color;
                    "Color", "The fog's colour [r, g, b]"
                fog_density: f32 = 0.02, [0.0, 1.0] log;
                    "Density", "How thick the fog is"
                fog_start: f32 = 5.0, [0.0, 100000.0] drag(0.5);
                    "Start", "Metres from the eye where fog begins"
                fog_end: f32 = 100.0, [0.0, 100000.0] drag(0.5);
                    "End", "Metres from the eye where fog is full"
                fog_height_enabled: bool = false, toggle;
                    "Height fog", "Fog that thins with height"
                fog_height_falloff: f32 = 0.1, [0.0, 2.0] slider;
                    "Height falloff", "How fast height fog thins going up"
                fog_height_origin: f32 = 0.0, [-100000.0, 100000.0] drag(0.25);
                    "Height origin", "The height the height fog is densest at"
                fog_height_density: f32 = 0.0, [0.0, 1.0] log;
                    "Height density", "Height fog's density at fog_height_origin; 0 adds none"
                fog_tint_color: rgb = [0.42, 0.50, 0.65], [0.0, 1.0] color;
                    "Tint color", "Aerial tint: the colour far things take"
                fog_tint_strength: f32 = 0.0, [0.0, 1.0] slider;
                    "Tint strength", "How much of the aerial tint far things take; 0 is off"
                fog_tint_start: f32 = 100.0, [0.0, 100000.0] drag(1.0);
                    "Tint start", "Metres where the aerial tint begins"
                fog_tint_end: f32 = 1000.0, [0.0, 100000.0] drag(1.0);
                    "Tint end", "Metres where the aerial tint is full"
            }

            volumetric "Volumetric light" "Light shafts marched through lit air" {
                volumetric_enabled: bool = false, toggle;
                    "Enabled", "Volumetric light shafts"
                volumetric_samples: u32 = 32, [1.0, 128.0] count;
                    "Samples", "Ray-march samples per pixel"
                volumetric_density: f32 = 1.0, [0.0, 1000.0] log;
                    "Density", "How thick the lit air is"
                volumetric_max_distance: f32 = 100.0, [1.0, 1000.0] log;
                    "Max distance", "How far the shafts are marched, in metres"
                volumetric_decay: f32 = 0.98, [0.8, 1.0] slider;
                    "Decay", "How fast a shaft fades along its length"
            }

            grade "Grade" "Desaturation, an ASC CDL colour grade after tone mapping, and film grain" {
                desaturate: f32 = 0.0, [0.0, 1.0] slider;
                    "Desaturate", "How much colour is drained, 0 to 1"
                grade_lift: rgb = [0.0, 0.0, 0.0], [-0.25, 0.25] channels(0.002);
                    "Lift", "Colour grade lift: per-channel add after tone mapping. Neutral is [0, 0, 0]"
                grade_gamma: rgb = [1.0, 1.0, 1.0], [0.25, 4.0] channels(0.01);
                    "Gamma", "Colour grade gamma: per-channel midtone curve. Neutral is [1, 1, 1]"
                grade_gain: rgb = [1.0, 1.0, 1.0], [0.0, 2.0] channels(0.01);
                    "Gain", "Colour grade gain: per-channel multiply. Neutral is [1, 1, 1]"
                film_grain: f32 = 0.0, [0.0, 0.2] slider;
                    "Film grain", "Animated film grain; 0 is off, about 0.02 to 0.05 is subtle"
            }

            dither "Dither" "Noise against banding in the final image" {
                dither_enabled: bool = false, toggle;
                    "Enabled", "Dither the final image against banding"
                dither_intensity: f32 = 0.03, [0.0, 0.1] slider;
                    "Intensity", "How strong the dither is. In style 0 it is linear light, so the same value is far stronger in the dark; in styles 1 and 2 it is display levels, and 0.004 (one 8-bit step) hides banding without showing a pattern"
                dither_style: u32 = 0, [0.0, 2.0] count;
                    "Style", "Where the dither is added: 0 = linear light (the original, a crosshatch that shows most near black), 1 = 8x8 Bayer in display levels (the same size at every brightness), 2 = triangular noise in display levels (no tile; the one to hide banding with)"
            }

            fxaa "FXAA" "Fast approximate anti-aliasing on the final composite" {
                fxaa: bool = false, toggle;
                    "Enabled", "FXAA anti-aliasing on the final composite"
            }

            edges "Shared edges" "The geometry edges every style that draws lines reads" {
                edge_depth: f32 = 0.6, [0.05, 4.0] slider;
                    "Silhouette step", "The relative view-distance step that is a silhouette, for every style that reads the shared geometry edges"
                edge_normal: f32 = 2.5, [0.2, 8.0] slider;
                    "Crease", "The normal change that is a crease, for every style that reads the shared geometry edges"
            }

            timing "Held drawings" "How long the shading boil and the whole frame hold each drawing" {
                boil_on: u32 = 2, [1.0, 3.0] count;
                    "Boil held on", "Film frames each drawing of the shading boil is held: 1, 2 or 3 (on twos). Shared by both styles' boils"
                motion_hold: f32 = 0.0, [0.0, 60.0] slider;
                    "Motion hold", "Motion on twos for the whole frame: entities with no motion_hold of their own are drawn on ticks, this many a second, and the post time styles read holds to them (12 = on twos at 24 fps). 0 = off"
            }

            tear "Tear" "Holes the frame tears open in: a whole-frame style or a styled prop with blend_source = \"tear\" shows only inside them (#746)" {
                tear: f32 = 0.0, [0.0, 1.0] slider;
                    "Amount", "How much of the frame is torn: 0 none, scattered holes growing to the whole frame at 1"
                tear_scale: f32 = 3.0, [0.5, 12.0] slider;
                    "Scale", "How many patches span the frame's height; 0 or less draws as 3"
                tear_iris: bool = false, toggle;
                    "Iris", "Tear open from the frame's centre in a wobbling circle instead of in patches"
            }
        }
    };
}

/// A parameter kind as its Rust type: `bool`, `f32`, `u32`, `[f32; 3]`.
#[macro_export]
macro_rules! post_type {
    (bool) => {
        bool
    };
    (f32) => {
        f32
    };
    (u32) => {
        u32
    };
    (rgb) => {
        [f32; 3]
    };
}

/// A parameter's field as a [`PostParamMut`], by kind.
#[macro_export]
macro_rules! post_param_mut {
    (bool, $field:expr) => {
        $crate::post_effects::PostParamMut::Bool($field)
    };
    (f32, $field:expr) => {
        $crate::post_effects::PostParamMut::F32($field)
    };
    (u32, $field:expr) => {
        $crate::post_effects::PostParamMut::U32($field)
    };
    (rgb, $field:expr) => {
        $crate::post_effects::PostParamMut::Rgb($field)
    };
}

/// A parameter's value as a [`PostValue`], by kind.
#[macro_export]
macro_rules! post_value {
    (bool, $v:expr) => {
        $crate::post_effects::PostValue::Bool($v)
    };
    (f32, $v:expr) => {
        $crate::post_effects::PostValue::F32($v)
    };
    (u32, $v:expr) => {
        $crate::post_effects::PostValue::U32($v)
    };
    (rgb, $v:expr) => {
        $crate::post_effects::PostValue::Rgb($v)
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! __post_kind {
    (bool) => {
        $crate::post_effects::PostKind::Bool
    };
    (f32) => {
        $crate::post_effects::PostKind::F32
    };
    (u32) => {
        $crate::post_effects::PostKind::U32
    };
    (rgb) => {
        $crate::post_effects::PostKind::Rgb
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! __post_widget {
    (toggle) => {
        $crate::post_effects::PostWidget::Toggle
    };
    (slider) => {
        $crate::post_effects::PostWidget::Slider
    };
    (log) => {
        $crate::post_effects::PostWidget::Log
    };
    (count) => {
        $crate::post_effects::PostWidget::Count
    };
    (color) => {
        $crate::post_effects::PostWidget::Color
    };
    (drag($s:literal)) => {
        $crate::post_effects::PostWidget::Drag($s)
    };
    (channels($s:literal)) => {
        $crate::post_effects::PostWidget::Channels($s)
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! __post_range {
    () => {
        (0.0, 1.0)
    };
    ($min:literal, $max:literal) => {
        ($min, $max)
    };
}

macro_rules! descriptors {
    (
        { }
        $(
            $effect:ident $elabel:literal $ehelp:literal {
                $(
                    $key:ident : $kind:ident = $default:expr,
                    $([$min:literal, $max:literal])? $widget:ident $(($speed:literal))?;
                    $label:literal, $help:literal
                )*
            }
        )*
    ) => {
        /// Every built-in effect, in the order F4 shows them.
        pub const EFFECTS: &[EffectDescriptor] = &[
            $(
                EffectDescriptor {
                    name: stringify!($effect),
                    label: $elabel,
                    help: $ehelp,
                    params: &[
                        $(
                            PostParam {
                                key: stringify!($key),
                                effect: stringify!($effect),
                                kind: $crate::__post_kind!($kind),
                                default: $crate::post_value!($kind, $default),
                                min: $crate::__post_range!($($min, $max)?).0,
                                max: $crate::__post_range!($($min, $max)?).1,
                                widget: $crate::__post_widget!($widget $(($speed))?),
                                label: $label,
                                help: $help,
                            },
                        )*
                    ],
                },
            )*
        ];
    };
}

post_effects!(descriptors {});

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_are_unique_and_each_default_lies_in_its_range() {
        let mut seen = std::collections::HashSet::new();
        for p in params() {
            assert!(seen.insert(p.key), "{} is listed twice", p.key);
            assert!(p.min < p.max, "{}: {}..{}", p.key, p.min, p.max);
            assert!(
                p.range_error(&p.default.to_toml()).is_none(),
                "{}: its default {} is outside {}..{}",
                p.key,
                p.default,
                p.min,
                p.max
            );
            let toggle = p.widget == PostWidget::Toggle;
            assert_eq!(toggle, p.kind == PostKind::Bool, "{}: widget", p.key);
            if p.kind == PostKind::U32 {
                assert_eq!(p.widget, PostWidget::Count, "{}: widget", p.key);
            }
            if p.kind == PostKind::Rgb {
                assert!(
                    matches!(p.widget, PostWidget::Color | PostWidget::Channels(_)),
                    "{}: widget",
                    p.key
                );
            }
        }
        assert!(EFFECTS.iter().all(|e| !e.params.is_empty()));
    }

    #[test]
    fn the_catalog_names_every_key_with_its_range() {
        let text = catalog_text();
        for p in params() {
            assert!(text.contains(p.key), "{}", p.key);
        }
        assert!(
            text.contains("bloom_intensity          f32  0.04"),
            "{text}"
        );
        assert!(text.contains("0..1"), "{text}");
        let json = serde_json::to_value(EFFECTS).unwrap();
        assert_eq!(json[0]["params"][0]["key"], "exposure");
        assert_eq!(json[0]["params"][0]["max"], 8.0);
    }

    #[test]
    fn a_value_past_its_range_says_so_as_a_style_value_does() {
        let bloom = param("bloom_intensity").unwrap();
        assert_eq!(
            bloom.range_error(&toml::Value::Integer(3)).as_deref(),
            Some("bloom_intensity=3 is outside its range 0..1")
        );
        assert!(bloom.range_error(&toml::Value::Float(0.2)).is_none());
        let fog = param("fog_color").unwrap();
        let red: toml::Value = toml::Value::Array(vec![2.0.into(), 0.0.into(), 0.0.into()]);
        assert!(fog.range_error(&red).unwrap().contains("fog_color="));
        assert!(param("fxaa")
            .unwrap()
            .range_error(&toml::Value::Boolean(true))
            .is_none());
        let table: toml::Table =
            toml::from_str("exposure = 50\nbloom_typo = 9\nfog_end = 300").unwrap();
        assert_eq!(
            range_warnings(&table),
            ["exposure=50 is outside its range 0.05..8"]
        );
    }

    /// #1062: a default is the decimal the descriptor wrote, which an
    /// editor's completion inserts from the JSON Schema, and still the same
    /// f32; a value written at a bound is inside it.
    #[test]
    fn a_default_is_its_shortest_decimal_and_a_bound_is_inside() {
        let bloom = param("bloom_intensity").unwrap();
        assert_eq!(bloom.default.to_toml(), toml::Value::Float(0.04));
        assert_eq!(
            param("fog_color").unwrap().default.to_toml(),
            toml::Value::Array(vec![
                toml::Value::Float(0.7),
                toml::Value::Float(0.75),
                toml::Value::Float(0.82),
            ])
        );
        for p in params() {
            let back = p.value_of(&p.default.to_toml()).unwrap();
            assert_eq!(back, p.default, "{}: its default reads back", p.key);
            for bound in [p.min, p.max] {
                let written: f64 = bound.to_string().parse().unwrap();
                let at = match p.kind {
                    PostKind::Bool => continue,
                    PostKind::Rgb => toml::Value::Array(vec![toml::Value::Float(written); 3]),
                    _ => toml::Value::Float(written),
                };
                assert!(p.range_error(&at).is_none(), "{} = {at}", p.key);
            }
        }
    }

    #[test]
    fn a_field_is_set_through_its_kind() {
        let mut n = 0u32;
        assert!(PostParamMut::U32(&mut n)
            .set(&toml::Value::Integer(16))
            .is_ok());
        assert_eq!(n, 16);
        assert!(PostParamMut::U32(&mut n)
            .set(&toml::Value::Float(-1.0))
            .is_err());
        let mut f = 0.0f32;
        PostParamMut::F32(&mut f)
            .set(&toml::Value::Integer(2))
            .unwrap();
        assert_eq!(f, 2.0);
        let mut rgb = [0.0f32; 3];
        PostParamMut::Rgb(&mut rgb)
            .set(&toml::from_str::<toml::Table>("c = [0.1, 0.2, 0.3]").unwrap()["c"])
            .unwrap();
        assert_eq!(rgb, [0.1, 0.2, 0.3]);
        let mut b = false;
        assert!(PostParamMut::Bool(&mut b)
            .set(&toml::Value::Integer(1))
            .is_err());
    }

    #[test]
    fn a_value_is_read_as_its_key_kind_and_put_only_into_that_kind() {
        let samples = param("ssao_samples").unwrap();
        assert_eq!(
            samples.value_of(&toml::Value::Float(16.0)),
            Ok(PostValue::U32(16))
        );
        assert!(samples.value_of(&toml::Value::Float(1.5)).is_err());
        let mut f = 0.0f32;
        assert!(PostParamMut::F32(&mut f).put(PostValue::F32(0.25)));
        assert_eq!(f, 0.25);
        assert!(!PostParamMut::F32(&mut f).put(PostValue::Bool(true)));
        assert_eq!(f, 0.25, "untouched");
    }

    #[test]
    fn an_unknown_key_says_what_it_might_have_been_and_lists_every_key() {
        let e = unknown_key("fog_desnity");
        assert!(
            e.starts_with("`fog_desnity` is no [post_process] key; did you mean `fog_density`?")
        );
        for p in params() {
            assert!(e.contains(p.key), "{}", p.key);
        }
        assert!(unknown_key("bloom").starts_with(
            "`bloom` is an effect, not a [post_process] key: its keys are bloom_enabled, \
             bloom_intensity, bloom_threshold, bloom_soft_threshold."
        ));
    }
}
