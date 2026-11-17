//! What `flint render`'s post-processing flags write (#708): `--set`, and the
//! per-effect flags it replaced, kept as hidden aliases for a release.
//!
//! `--set` takes what `--sweep` takes, with one value instead of a list:
//! `post.<key>=v` is a `[post_process]` key (the effects' descriptors, #741,
//! are its list and its kinds), and `<style>.<key>=v` a key of a style's
//! table, as the scene's `[post_process.styles.<style>]` writes it, without
//! turning the style on (`--style` does that).
//!
//! Each old flag is a [`LegacyFlag`]: the flag as given and the `--set` /
//! `--style` words it stands for. Those words are what it applies, so the flag
//! and its new spelling are one config by construction, and the deprecation
//! note prints them.
//!
//! A render's post-processing is built in this order: the scene's block, the
//! look and a sweep column's `post.` value (as a `[post_process]` table); then
//! `--no-postprocess`, the old flags in their old order (the deprecated
//! `--render-mode` among them), `--set` in the order given, and `--style` last,
//! so an explicit word wins.

use super::render::RenderArgs;
use crate::commands::common_args::{parse_vec3, parse_vec4};
use anyhow::Result;
use flint_render::stylize::{StyleRegistry, StyleSpec};
use flint_render::PostProcessConfig;
use flint_schema::post_effects::{self, PostParam};

/// One `--set`: a key and the one value it takes.
#[derive(Debug, Clone, PartialEq)]
pub struct Set {
    pub key: SetKey,
    /// As written, after the `=`.
    pub value: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SetKey {
    /// `post.<key>`: a built-in effect's `[post_process]` key.
    Post(&'static str),
    /// `<style>.<key>`: a key of a style's table. Checked against the
    /// registry once the scene's user styles are in ([`Set::check`]), as
    /// `--style` is.
    Style { style: String, key: String },
}

impl Set {
    /// The key as written: `post.exposure`, `ascii.cell`. A `--sweep` of
    /// the same key has the same name.
    pub fn name(&self) -> String {
        match &self.key {
            SetKey::Post(key) => format!("post.{key}"),
            SetKey::Style { style, key } => format!("{style}.{key}"),
        }
    }

    /// A style key as the `--style` word that would also turn the style on:
    /// what [`StyleSpec`] checks and parses it with.
    fn style_word(&self) -> Option<String> {
        match &self.key {
            SetKey::Style { style, key } => Some(format!("{style}:{key}={}", self.value)),
            SetKey::Post(_) => None,
        }
    }

    fn at(&self) -> String {
        format!("--set {}={}", self.name(), self.value)
    }

    /// Check a style key against the registry, user styles included: call
    /// it once the scene's folder has registered them.
    pub fn check(&self, styles: &StyleRegistry) -> Result<(), String> {
        match self.style_word() {
            Some(word) => StyleSpec::parse(&word, styles)
                .map(drop)
                .map_err(|e| format!("{}: {e}", self.at())),
            None => Ok(()),
        }
    }

    /// Write the value into `config`. A post value past what its effect is
    /// tuned for still draws, and says so, as the scene's would.
    pub fn apply(&self, config: &mut PostProcessConfig, styles: &StyleRegistry) -> Result<()> {
        match &self.key {
            SetKey::Post(key) => {
                let value = super::sweep::parse_value(&self.value);
                if let Some(range) = post_effects::param(key).and_then(|p| p.range_error(&value)) {
                    eprintln!("warning: {}: {range} (it still draws)", self.at());
                }
                config
                    .param_mut(key)
                    .expect("every descriptor key is a config field")
                    .set(&value)
                    .map_err(|e| anyhow::anyhow!("{}: {key} {e}", self.at()))?;
            }
            SetKey::Style { .. } => {
                let word = self.style_word().expect("a style key");
                StyleSpec::parse(&word, styles)
                    .map_err(|e| anyhow::anyhow!("{}: {e}", self.at()))?
                    .apply_settings(config, styles);
            }
        }
        Ok(())
    }
}

/// `--set`'s value parser: `post.<key>=v` or `<style>.<key>=v`. A post key
/// and its value's kind are checked here; a style key once the scene's user
/// styles are registered ([`Set::check`]).
pub fn parse_set(text: &str) -> Result<Set, String> {
    let (name, value) = text
        .split_once('=')
        .map(|(k, v)| (k.trim(), v.trim()))
        .filter(|(k, v)| !k.is_empty() && !v.is_empty())
        .ok_or_else(|| {
            format!(
                "'{text}' is not key=value: post.<key>=v (`flint render --list-effects`) or \
                 <style>.<key>=v (`--list-styles`)"
            )
        })?;
    let (head, key) = name.split_once('.').ok_or_else(|| {
        format!("'{name}' is not post.<key> or <style>.<key> (`--list-effects`, `--list-styles`)")
    })?;
    let key = if head == "post" {
        let param = post_param(key)?;
        // The kind now, so a typo'd colour fails before any GPU work
        PostProcessConfig::default()
            .param_mut(param.key)
            .expect("every descriptor key is a config field")
            .set(&super::sweep::parse_value(value))
            .map_err(|e| format!("--set {name}={value}: {} {e}", param.key))?;
        SetKey::Post(param.key)
    } else {
        if matches!(key, "enabled" | "on" | "off") {
            return Err(format!(
                "--set {name}: --set tunes a style and leaves it on or off as it was; \
                 `--style {head}` turns it on and `--style {head}:off` off"
            ));
        }
        SetKey::Style {
            style: head.to_string(),
            key: key.to_string(),
        }
    };
    Ok(Set {
        key,
        value: value.to_string(),
    })
}

/// The descriptor of `post.<key>`, or the key it might have been.
fn post_param(key: &str) -> Result<&'static PostParam, String> {
    post_effects::param(key).ok_or_else(|| {
        let near = flint_core::diagnostic::closest(key, post_effects::params().map(|p| p.key));
        format!(
            "post.{key} is no [post_process] key{} (`flint render --list-effects` lists them)",
            near.map(|n| format!("; did you mean post.{n}?"))
                .unwrap_or_default()
        )
    })
}

/// The flags the effects' descriptors and the styles took over (#708): each a
/// `--set` or `--style` now, hidden, and gone in the release after. Each
/// one's words are in [`legacy_flags`].
#[derive(clap::Args, Debug, Clone, Default)]
pub struct LegacyPostArgs {
    #[command(flatten)]
    pub effects: LegacyEffectArgs,
    #[command(flatten)]
    pub grade: LegacyGradeArgs,
    #[command(flatten)]
    pub styles: LegacyStyleArgs,
    #[command(flatten)]
    pub ascii_look: LegacyAsciiArgs,
    #[command(flatten)]
    pub ascii_tune: LegacyAsciiTuneArgs,
}

/// Hidden: see [`LegacyPostArgs`].
#[derive(clap::Args, Debug, Clone, Default)]
pub struct LegacyEffectArgs {
    #[arg(long, hide = true)]
    pub bloom_intensity: Option<f32>,
    #[arg(long, hide = true)]
    pub bloom_threshold: Option<f32>,
    #[arg(long, hide = true, allow_hyphen_values = true)]
    pub exposure: Option<f32>,
    #[arg(long, hide = true)]
    pub ssao_radius: Option<f32>,
    #[arg(long, hide = true)]
    pub ssao_intensity: Option<f32>,
    #[arg(long, hide = true)]
    pub ssao_samples: Option<u32>,
    #[arg(long, hide = true)]
    pub fog_density: Option<f32>,
    #[arg(long, hide = true, value_parser = parse_vec3)]
    pub fog_color: Option<[f32; 3]>,
    #[arg(long, hide = true)]
    pub fog_height_falloff: Option<f32>,
    #[arg(long, hide = true)]
    pub dither_intensity: Option<f32>,
    #[arg(long, hide = true)]
    pub desaturate: Option<f32>,
    #[arg(long, hide = true)]
    pub dof: Option<f32>,
    #[arg(long, hide = true)]
    pub dof_focus: Option<f32>,
    #[arg(long, hide = true)]
    pub dof_range: Option<f32>,
    #[arg(long, hide = true)]
    pub tear: Option<f32>,
    #[arg(long, hide = true)]
    pub tear_scale: Option<f32>,
    #[arg(long, hide = true)]
    pub tear_iris: bool,
    /// Deprecated since #746: a render mode is a style. 1 = `--style
    /// matrix:blend_source=tear --set post.tear=M`, 2 = blood, 3 = `--style
    /// drunk:blend=M`, 4 = tron, 5 = `--style underwater:blend=M,depth=..`, 6 =
    /// `--set post.tear=M` alone
    #[arg(long, hide = true)]
    pub render_mode: Option<u32>,
    /// With --render-mode: its mix 0..1 (default 1)
    #[arg(long, hide = true)]
    pub mode_mix: Option<f32>,
    /// With --render-mode: X,Y,Z,W
    #[arg(long, hide = true, value_parser = parse_vec4)]
    pub mode_params: Option<[f32; 4]>,
}

/// Hidden: see [`LegacyPostArgs`].
#[derive(clap::Args, Debug, Clone, Default)]
pub struct LegacyGradeArgs {
    #[arg(long, hide = true)]
    pub volumetric_density: Option<f32>,
    #[arg(long, hide = true)]
    pub volumetric_samples: Option<u32>,
    #[arg(long, hide = true)]
    pub edge_depth: Option<f32>,
    #[arg(long, hide = true)]
    pub edge_normal: Option<f32>,
    #[arg(long, hide = true)]
    pub boil_on: Option<u32>,
    #[arg(long, hide = true, value_name = "RATE")]
    pub motion_hold: Option<f32>,
    #[arg(long, hide = true)]
    pub film_grain: Option<f32>,
    #[arg(long, hide = true, value_parser = parse_vec3, allow_hyphen_values = true)]
    pub grade_lift: Option<[f32; 3]>,
    #[arg(long, hide = true, value_parser = parse_vec3)]
    pub grade_gamma: Option<[f32; 3]>,
    #[arg(long, hide = true, value_parser = parse_vec3)]
    pub grade_gain: Option<[f32; 3]>,
    #[arg(long, hide = true)]
    pub fxaa: bool,
}

/// Hidden: see [`LegacyPostArgs`].
#[derive(clap::Args, Debug, Clone, Default)]
pub struct LegacyStyleArgs {
    #[arg(long, hide = true)]
    pub kuwahara_radius: Option<u32>,
    #[arg(long, hide = true)]
    pub kuwahara_sharpness: Option<f32>,
    #[arg(long, hide = true)]
    pub kuwahara_hardness: Option<f32>,
    #[arg(long, hide = true)]
    pub kuwahara_anisotropy: Option<f32>,
    #[arg(long, hide = true)]
    pub kuwahara_boil: Option<f32>,
    #[arg(long, hide = true)]
    pub contour: bool,
    #[arg(long, hide = true)]
    pub contour_width: Option<f32>,
    #[arg(long, hide = true)]
    pub contour_threshold: Option<f32>,
    #[arg(long, hide = true)]
    pub contour_strength: Option<f32>,
    #[arg(long, hide = true)]
    pub contour_luma: Option<f32>,
    #[arg(long, hide = true)]
    pub contour_fade: Option<f32>,
    #[arg(long, hide = true)]
    pub contour_boil: Option<f32>,
    #[arg(long, hide = true, value_parser = parse_vec3)]
    pub contour_color: Option<[f32; 3]>,
}

/// Hidden: see [`LegacyPostArgs`].
#[derive(clap::Args, Debug, Clone, Default)]
pub struct LegacyAsciiArgs {
    #[arg(long, hide = true)]
    pub ascii: bool,
    #[arg(long, hide = true)]
    pub ascii_font: Option<String>,
    #[arg(long, hide = true)]
    pub ascii_ramp: Option<String>,
    #[arg(long, hide = true, value_parser = ["screen", "entities"])]
    pub ascii_scope: Option<String>,
    #[arg(long, hide = true, value_parser = ["screen", "object"])]
    pub ascii_grid: Option<String>,
    #[arg(long, hide = true)]
    pub ascii_color: Option<String>,
    #[arg(long, hide = true)]
    pub ascii_ground: Option<String>,
    #[arg(long, hide = true)]
    pub ascii_palette: Option<String>,
    #[arg(long, hide = true, value_parser = parse_vec3)]
    pub ascii_tint: Option<[f32; 3]>,
}

/// Hidden: see [`LegacyPostArgs`].
#[derive(clap::Args, Debug, Clone, Default)]
pub struct LegacyAsciiTuneArgs {
    #[arg(long, hide = true)]
    pub ascii_cell: Option<f32>,
    #[arg(long, hide = true)]
    pub ascii_levels: Option<f32>,
    #[arg(long, hide = true)]
    pub ascii_footprint: Option<f32>,
    #[arg(long, hide = true)]
    pub ascii_background: Option<f32>,
    #[arg(long, hide = true, allow_hyphen_values = true)]
    pub ascii_brightness: Option<f32>,
    #[arg(long, hide = true)]
    pub ascii_contrast: Option<f32>,
    #[arg(long, hide = true)]
    pub ascii_ink: Option<f32>,
    #[arg(long, hide = true)]
    pub ascii_glow: Option<f32>,
    #[arg(long, hide = true)]
    pub ascii_edges: Option<f32>,
    #[arg(long, hide = true)]
    pub ascii_edge_threshold: Option<f32>,
    #[arg(long, hide = true)]
    pub ascii_edge_max: Option<f32>,
    #[arg(long, hide = true)]
    pub ascii_edge_depth: Option<f32>,
    #[arg(long, hide = true)]
    pub ascii_edge_normal: Option<f32>,
    #[arg(long, hide = true)]
    pub ascii_dither: Option<f32>,
    #[arg(long, hide = true, allow_hyphen_values = true)]
    pub ascii_exposure: Option<f32>,
    #[arg(long, hide = true)]
    pub ascii_normalize: Option<f32>,
    #[arg(long, hide = true)]
    pub ascii_no_edges: bool,
}

/// A word an old flag stands for.
#[derive(Debug, Clone, PartialEq)]
pub enum Word {
    /// `--set <key>=<value>`.
    Set(String),
    /// `--style <word>`.
    Style(String),
}

impl Word {
    /// The word as a command line writes it.
    pub fn text(&self) -> String {
        match self {
            Word::Set(set) => format!("--set {set}"),
            Word::Style(style) => format!("--style {style}"),
        }
    }

    fn apply(&self, config: &mut PostProcessConfig, styles: &StyleRegistry) -> Result<()> {
        match self {
            Word::Set(text) => parse_set(text)
                .map_err(anyhow::Error::msg)?
                .apply(config, styles),
            Word::Style(word) => flint_render::stylize::apply_style_spec(config, word, styles)
                .map_err(|e| anyhow::anyhow!("--style {word}: {e}")),
        }
    }
}

/// One old flag as given, and the words it is now.
#[derive(Debug, Clone, PartialEq)]
pub struct LegacyFlag {
    /// The flag and its value as written: `--bloom-intensity 0.1`.
    pub given: String,
    pub words: Vec<Word>,
}

impl LegacyFlag {
    /// The words as one command line.
    pub fn spelling(&self) -> String {
        self.words
            .iter()
            .map(Word::text)
            .collect::<Vec<_>>()
            .join(" ")
    }
}

/// Three numbers as `--set post.` and the old flags take them.
struct Rgb([f32; 3]);

impl std::fmt::Display for Rgb {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let [r, g, b] = self.0;
        write!(f, "{r},{g},{b}")
    }
}

impl Rgb {
    /// As a style's colour setting takes it: `r/g/b`.
    fn slashed(&self) -> String {
        let [r, g, b] = self.0;
        format!("{r}/{g}/{b}")
    }
}

/// The old flags given, built up in the order they used to apply.
#[derive(Default)]
struct Flags(Vec<LegacyFlag>);

impl Flags {
    /// `--<flag> v`, when given, as `words(v)`.
    fn with<V: std::fmt::Display>(
        &mut self,
        flag: &str,
        value: Option<V>,
        words: impl FnOnce(&V) -> Vec<Word>,
    ) {
        if let Some(v) = value {
            let words = words(&v);
            self.0.push(LegacyFlag {
                given: format!("--{flag} {v}"),
                words,
            });
        }
    }

    /// `--<flag> v` as `--set post.<key>=v`.
    fn post<V: std::fmt::Display>(&mut self, flag: &str, key: &str, value: Option<V>) {
        self.with(flag, value, |v| vec![post(key, v)]);
    }

    /// The switch `--<flag>`, when given, as `words`.
    fn switch(&mut self, flag: &str, on: bool, words: Vec<Word>) {
        if on {
            self.0.push(LegacyFlag {
                given: format!("--{flag}"),
                words,
            });
        }
    }
}

fn post(key: &str, value: impl std::fmt::Display) -> Word {
    Word::Set(format!("post.{key}={value}"))
}

fn style(word: String) -> Word {
    Word::Style(word)
}

fn set(key: &str, value: impl std::fmt::Display) -> Word {
    Word::Set(format!("{key}={value}"))
}

/// The old flags given, in the order they used to apply, as the words each
/// stands for. The first list applied before the deprecated `--render-mode`
/// and the second after it (the mode writes the tear, so a `--tear` loses to
/// it, as it always did).
pub fn legacy_flags(l: &LegacyPostArgs) -> (Vec<LegacyFlag>, Vec<LegacyFlag>) {
    let mut early = Flags::default();
    early.with("bloom-intensity", l.effects.bloom_intensity, |v| {
        vec![post("bloom_intensity", v), post("bloom_enabled", true)]
    });
    early.post(
        "bloom-threshold",
        "bloom_threshold",
        l.effects.bloom_threshold,
    );
    early.post("exposure", "exposure", l.effects.exposure);
    early.post("ssao-radius", "ssao_radius", l.effects.ssao_radius);
    early.with("ssao-intensity", l.effects.ssao_intensity, |v| {
        let mut words = vec![post("ssao_intensity", v)];
        if *v <= 0.0 {
            words.push(post("ssao_enabled", false));
        }
        words
    });
    early.with("ssao-samples", l.effects.ssao_samples, |v| {
        vec![post("ssao_samples", (*v).clamp(1, 64))]
    });
    early.with("fog-density", l.effects.fog_density, |v| {
        vec![post("fog_density", v), post("fog_enabled", *v > 0.0)]
    });
    early.post("fog-color", "fog_color", l.effects.fog_color.map(Rgb));
    early.with("fog-height-falloff", l.effects.fog_height_falloff, |v| {
        vec![
            post("fog_height_falloff", v),
            post("fog_height_enabled", true),
        ]
    });
    early.with("dither-intensity", l.effects.dither_intensity, |v| {
        vec![
            post("dither_intensity", v),
            post("dither_enabled", *v > 0.0),
        ]
    });
    early.post("desaturate", "desaturate", l.effects.desaturate);
    early.post("dof", "dof_strength", l.effects.dof);
    early.post("dof-focus", "dof_focus_distance", l.effects.dof_focus);
    early.post("dof-range", "dof_focus_range", l.effects.dof_range);
    early.post("tear", "tear", l.effects.tear);
    early.post("tear-scale", "tear_scale", l.effects.tear_scale);
    early.switch(
        "tear-iris",
        l.effects.tear_iris,
        vec![post("tear_iris", true)],
    );

    let mut late = Flags::default();
    late.with("volumetric-density", l.grade.volumetric_density, |v| {
        vec![
            post("volumetric_density", v),
            post("volumetric_enabled", *v > 0.0),
        ]
    });
    late.post(
        "volumetric-samples",
        "volumetric_samples",
        l.grade.volumetric_samples,
    );
    // Kuwahara's table (#298): each turns it on
    for (flag, key, value) in [
        (
            "kuwahara-radius",
            "radius",
            l.styles.kuwahara_radius.map(|r| r as f32),
        ),
        (
            "kuwahara-sharpness",
            "sharpness",
            l.styles.kuwahara_sharpness,
        ),
        ("kuwahara-hardness", "hardness", l.styles.kuwahara_hardness),
        (
            "kuwahara-anisotropy",
            "anisotropy",
            l.styles.kuwahara_anisotropy,
        ),
    ] {
        late.with(flag, value, |v| vec![style(format!("kuwahara:{key}={v}"))]);
    }
    late.post("edge-depth", "edge_depth", l.grade.edge_depth);
    late.post("edge-normal", "edge_normal", l.grade.edge_normal);
    // The shading boil tunes Kuwahara without turning it on
    late.with("kuwahara-boil", l.styles.kuwahara_boil, |v| {
        vec![set("kuwahara.boil", v)]
    });
    // The contour style (#599): any of its flags turns it on
    late.switch("contour", l.styles.contour, vec![style("contour".into())]);
    for (flag, key, value) in [
        ("contour-width", "width", l.styles.contour_width),
        ("contour-threshold", "threshold", l.styles.contour_threshold),
        ("contour-strength", "strength", l.styles.contour_strength),
        ("contour-luma", "luma", l.styles.contour_luma),
        ("contour-fade", "fade", l.styles.contour_fade),
        ("contour-boil", "boil", l.styles.contour_boil),
    ] {
        late.with(flag, value, |v| vec![style(format!("contour:{key}={v}"))]);
    }
    late.with("contour-color", l.styles.contour_color.map(Rgb), |v| {
        vec![style(format!("contour:color={}", v.slashed()))]
    });
    late.post("boil-on", "boil_on", l.grade.boil_on);
    late.with("motion-hold", l.grade.motion_hold, |v| {
        vec![post("motion_hold", v.max(0.0))]
    });
    late.post("film-grain", "film_grain", l.grade.film_grain);
    late.post("grade-lift", "grade_lift", l.grade.grade_lift.map(Rgb));
    late.post("grade-gamma", "grade_gamma", l.grade.grade_gamma.map(Rgb));
    late.post("grade-gain", "grade_gain", l.grade.grade_gain.map(Rgb));
    late.switch("fxaa", l.grade.fxaa, vec![post("fxaa", true)]);
    ascii_flags(l, &mut late);
    (early.0, late.0)
}

/// The ASCII style's flags (#265): `--ascii`, and the flags that set its look
/// (font, ramp, colour, scope, edges...), turn it on, as `--style ascii:..`;
/// the tuning flags (`--ascii-cell`, `--ascii-levels`, `--ascii-footprint`,
/// `--ascii-edge-depth`, `--ascii-edge-normal`, `--ascii-no-edges`) tune it
/// and leave it as it was, as `--set ascii.<key>`.
fn ascii_flags(l: &LegacyPostArgs, flags: &mut Flags) {
    let on = |key: &str, v: &dyn std::fmt::Display| style(format!("ascii:{key}={v}"));
    flags.switch("ascii", l.ascii_look.ascii, vec![style("ascii".into())]);
    for (flag, key, value) in [
        ("ascii-font", "font", &l.ascii_look.ascii_font),
        ("ascii-ramp", "ramp", &l.ascii_look.ascii_ramp),
        ("ascii-scope", "scope", &l.ascii_look.ascii_scope),
        ("ascii-grid", "grid", &l.ascii_look.ascii_grid),
        ("ascii-color", "color", &l.ascii_look.ascii_color),
        ("ascii-ground", "ground", &l.ascii_look.ascii_ground),
    ] {
        flags.with(flag, value.as_ref(), |v| vec![on(key, v)]);
    }
    // A palette or a tint picks its colour mode unless --ascii-color does
    let mode = |key: &str, mode: &str, v: &dyn std::fmt::Display| match l.ascii_look.ascii_color {
        Some(_) => style(format!("ascii:{key}={v}")),
        None => style(format!("ascii:{key}={v},color={mode}")),
    };
    flags.with("ascii-palette", l.ascii_look.ascii_palette.as_ref(), |v| {
        vec![mode("palette", "palette", v)]
    });
    flags.with("ascii-tint", l.ascii_look.ascii_tint.map(Rgb), |v| {
        vec![mode("tint", "tint", &v.slashed())]
    });
    // In the order they used to apply: tuning keys first
    for (flag, key, value, turns_on) in [
        ("ascii-cell", "cell", l.ascii_tune.ascii_cell, false),
        ("ascii-levels", "levels", l.ascii_tune.ascii_levels, false),
        (
            "ascii-footprint",
            "footprint",
            l.ascii_tune.ascii_footprint,
            false,
        ),
        (
            "ascii-background",
            "background",
            l.ascii_tune.ascii_background,
            true,
        ),
        (
            "ascii-brightness",
            "brightness",
            l.ascii_tune.ascii_brightness,
            true,
        ),
        (
            "ascii-contrast",
            "contrast",
            l.ascii_tune.ascii_contrast,
            true,
        ),
        ("ascii-ink", "ink", l.ascii_tune.ascii_ink, true),
        ("ascii-glow", "glow", l.ascii_tune.ascii_glow, true),
        ("ascii-edges", "edges", l.ascii_tune.ascii_edges, true),
        (
            "ascii-edge-threshold",
            "edge_threshold",
            l.ascii_tune.ascii_edge_threshold,
            true,
        ),
        (
            "ascii-edge-max",
            "edge_max",
            l.ascii_tune.ascii_edge_max,
            true,
        ),
        (
            "ascii-edge-depth",
            "edge_depth",
            l.ascii_tune.ascii_edge_depth,
            false,
        ),
        (
            "ascii-edge-normal",
            "edge_normal",
            l.ascii_tune.ascii_edge_normal,
            false,
        ),
        ("ascii-dither", "dither", l.ascii_tune.ascii_dither, true),
        (
            "ascii-exposure",
            "exposure",
            l.ascii_tune.ascii_exposure,
            true,
        ),
        (
            "ascii-normalize",
            "normalize",
            l.ascii_tune.ascii_normalize,
            true,
        ),
    ] {
        flags.with(flag, value, |v| match turns_on {
            true => vec![on(key, v)],
            false => vec![set(&format!("ascii.{key}"), v)],
        });
    }
    flags.switch(
        "ascii-no-edges",
        l.ascii_tune.ascii_no_edges,
        vec![set("ascii.edges", 0)],
    );
}

/// Every old flag given, in order.
pub fn legacy_flags_given(l: &LegacyPostArgs) -> Vec<LegacyFlag> {
    let (mut early, late) = legacy_flags(l);
    early.extend(late);
    early
}

/// Say once a process, for each old flag given, what it is now.
pub fn warn_legacy(args: &RenderArgs) {
    use std::collections::BTreeSet;
    use std::sync::{Mutex, OnceLock};
    static SAID: OnceLock<Mutex<BTreeSet<String>>> = OnceLock::new();
    let said = SAID.get_or_init(Default::default);
    let Ok(mut said) = said.lock() else { return };
    for flag in legacy_flags_given(&args.legacy) {
        if said.insert(flag.given.clone()) {
            eprintln!(
                "warning: {} is `{}` now (#708); the old flag goes in the next release",
                flag.given,
                flag.spelling()
            );
        }
    }
}

/// Check every `--set` style key (after the scene's user styles are
/// registered), and refuse a `--set` of a key a `--sweep` turns: one of the
/// two would be lost.
pub fn check(args: &RenderArgs, styles: &StyleRegistry) -> Result<()> {
    for s in &args.set {
        s.check(styles).map_err(anyhow::Error::msg)?;
        if let Some(sweep) = args.sweeps.sweep.iter().find(|w| w.name() == s.name()) {
            anyhow::bail!(
                "--set {} and --sweep {}: the sweep turns that key, so drop the --set",
                s.name(),
                sweep.name()
            );
        }
    }
    Ok(())
}

/// The deprecated `--render-mode` (#746), with its mix (1 when not given)
/// and params, as the shim maps it onto the styles that replaced it; warned
/// about once a process.
pub fn legacy_render_mode(args: &RenderArgs) -> Option<flint_render::render_mode::RenderMode> {
    let l = &args.legacy;
    let mode = l.effects.render_mode?;
    static WARNED: std::sync::Once = std::sync::Once::new();
    WARNED.call_once(|| {
        tracing::warn!(
            "--render-mode is deprecated (#746) and goes in the next release: \
             the modes are styles now (`flint render --list-styles`: matrix, blood, drunk, \
             tron, underwater), torn in by --set post.tear and `blend_source=tear`"
        );
    });
    Some(flint_render::render_mode::RenderMode {
        mode,
        // A mode with no explicit mix defaults to fully torn through
        mix: l.effects.mode_mix.unwrap_or(1.0),
        params: l.effects.mode_params.unwrap_or_default(),
    })
}

/// The command line's post words over `config`: the old flags (and the
/// render mode between them), then `--set`. The stage path lays them again
/// over a sweep column's preset, whose `[boil]` they win over.
pub fn apply_flags(
    args: &RenderArgs,
    config: &mut PostProcessConfig,
    styles: &StyleRegistry,
) -> Result<()> {
    let (early, late) = legacy_flags(&args.legacy);
    for word in early.iter().flat_map(|f| &f.words) {
        word.apply(config, styles)?;
    }
    if let Some(mode) = legacy_render_mode(args) {
        mode.apply(config);
    }
    for word in late.iter().flat_map(|f| &f.words) {
        word.apply(config, styles)?;
    }
    for s in &args.set {
        s.apply(config, styles)?;
    }
    Ok(())
}

/// The post-processing a render draws with: the scene's block (under the
/// look and a sweep column, already laid into `def`), then the command line.
pub fn post_process_config(
    args: &RenderArgs,
    def: Option<&flint_scene::PostProcessDef>,
    styles: &StyleRegistry,
) -> Result<PostProcessConfig> {
    let mut config = match def {
        Some(def) => flint_player::post_process_config_from_def(def, styles),
        None => PostProcessConfig::default(),
    };
    if args.no_postprocess {
        config.enabled = false;
    }
    apply_flags(args, &mut config, styles)?;
    // `--style name[:k=v,...]` (#271) last: an explicit word wins
    args.style.apply(&mut config, styles);
    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[derive(Parser)]
    struct Cli {
        #[command(flatten)]
        args: RenderArgs,
    }

    fn args(line: &str) -> RenderArgs {
        let words = ["render", "s.scene.toml"]
            .into_iter()
            .chain(line.split_whitespace());
        match Cli::try_parse_from(words) {
            Ok(cli) => cli.args,
            Err(e) => panic!("{line}: {e}"),
        }
    }

    fn config(line: &str) -> String {
        format!(
            "{:?}",
            post_process_config(
                &args(line),
                None,
                &flint_render::stylize::StyleRegistry::new()
            )
            .unwrap()
        )
    }

    /// Every old flag beside its new spelling (#708's done-when): each pair
    /// must give the same config. A tuning flag of the ASCII style is
    /// written with the style on, as it only ever tuned it then.
    const SPELLINGS: &[(&str, &str)] = &[
        (
            "--bloom-intensity 0.1",
            "--set post.bloom_intensity=0.1 --set post.bloom_enabled=true",
        ),
        ("--bloom-threshold 2.5", "--set post.bloom_threshold=2.5"),
        ("--exposure 1.5", "--set post.exposure=1.5"),
        ("--ssao-radius 1.25", "--set post.ssao_radius=1.25"),
        ("--ssao-intensity 2", "--set post.ssao_intensity=2"),
        (
            "--ssao-intensity 0",
            "--set post.ssao_intensity=0 --set post.ssao_enabled=false",
        ),
        ("--ssao-samples 16", "--set post.ssao_samples=16"),
        ("--ssao-samples 0", "--set post.ssao_samples=1"),
        (
            "--fog-density 0.05",
            "--set post.fog_density=0.05 --set post.fog_enabled=true",
        ),
        (
            "--fog-density 0",
            "--set post.fog_density=0 --set post.fog_enabled=false",
        ),
        (
            "--fog-color 0.1,0.2,0.3",
            "--set post.fog_color=0.1,0.2,0.3",
        ),
        (
            "--fog-height-falloff 0.4",
            "--set post.fog_height_falloff=0.4 --set post.fog_height_enabled=true",
        ),
        (
            "--dither-intensity 0.004",
            "--set post.dither_intensity=0.004 --set post.dither_enabled=true",
        ),
        ("--desaturate 0.85", "--set post.desaturate=0.85"),
        ("--dof 0.6", "--set post.dof_strength=0.6"),
        ("--dof-focus 12", "--set post.dof_focus_distance=12"),
        ("--dof-range 3", "--set post.dof_focus_range=3"),
        ("--tear 0.5", "--set post.tear=0.5"),
        ("--tear-scale 4", "--set post.tear_scale=4"),
        ("--tear-iris", "--set post.tear_iris=true"),
        (
            "--volumetric-density 2",
            "--set post.volumetric_density=2 --set post.volumetric_enabled=true",
        ),
        (
            "--volumetric-samples 48",
            "--set post.volumetric_samples=48",
        ),
        ("--kuwahara-radius 6", "--style kuwahara:radius=6"),
        ("--kuwahara-sharpness 4", "--style kuwahara:sharpness=4"),
        ("--kuwahara-hardness 12", "--style kuwahara:hardness=12"),
        (
            "--kuwahara-anisotropy 0.5",
            "--style kuwahara:anisotropy=0.5",
        ),
        ("--edge-depth 0.3", "--set post.edge_depth=0.3"),
        ("--edge-normal 1.5", "--set post.edge_normal=1.5"),
        ("--kuwahara-boil 0.5", "--set kuwahara.boil=0.5"),
        ("--contour", "--style contour"),
        ("--contour-width 1.5", "--style contour:width=1.5"),
        ("--contour-threshold 0.2", "--style contour:threshold=0.2"),
        ("--contour-strength 0.5", "--style contour:strength=0.5"),
        ("--contour-luma 0.25", "--style contour:luma=0.25"),
        ("--contour-fade 12", "--style contour:fade=12"),
        ("--contour-boil 1", "--style contour:boil=1"),
        (
            "--contour-color 0.2,0.05,0.02",
            "--style contour:color=0.2/0.05/0.02",
        ),
        ("--boil-on 3", "--set post.boil_on=3"),
        ("--motion-hold 12", "--set post.motion_hold=12"),
        ("--film-grain 0.03", "--set post.film_grain=0.03"),
        (
            "--grade-lift 0.03,0.02,0.015",
            "--set post.grade_lift=0.03,0.02,0.015",
        ),
        (
            "--grade-gamma 1.1,1,0.9",
            "--set post.grade_gamma=1.1,1,0.9",
        ),
        (
            "--grade-gain 1.04,1,0.94",
            "--set post.grade_gain=1.04,1,0.94",
        ),
        ("--fxaa", "--set post.fxaa=true"),
        ("--ascii", "--style ascii"),
        ("--ascii-font builtin", "--style ascii:font=builtin"),
        ("--ascii-ramp blocks", "--style ascii:ramp=blocks"),
        ("--ascii-scope entities", "--style ascii:scope=entities"),
        ("--ascii-grid object", "--style ascii:grid=object"),
        ("--ascii-color mono", "--style ascii:color=mono"),
        ("--ascii-ground inverse", "--style ascii:ground=inverse"),
        (
            "--ascii-palette phosphor",
            "--style ascii:palette=phosphor,color=palette",
        ),
        (
            "--ascii-color mono --ascii-palette amber",
            "--style ascii:color=mono --style ascii:palette=amber",
        ),
        (
            "--ascii-tint 0.35,1,0.45",
            "--style ascii:tint=0.35/1/0.45,color=tint",
        ),
        (
            "--ascii --ascii-cell 12",
            "--style ascii --set ascii.cell=12",
        ),
        (
            "--ascii --ascii-levels 4",
            "--style ascii --set ascii.levels=4",
        ),
        (
            "--ascii --ascii-footprint 2",
            "--style ascii --set ascii.footprint=2",
        ),
        ("--ascii-background 0.3", "--style ascii:background=0.3"),
        ("--ascii-brightness -0.2", "--style ascii:brightness=-0.2"),
        ("--ascii-contrast 1.5", "--style ascii:contrast=1.5"),
        ("--ascii-ink 0.8", "--style ascii:ink=0.8"),
        ("--ascii-glow 0.6", "--style ascii:glow=0.6"),
        ("--ascii-edges 0.5", "--style ascii:edges=0.5"),
        (
            "--ascii-edge-threshold 0.25",
            "--style ascii:edge_threshold=0.25",
        ),
        ("--ascii-edge-max 4", "--style ascii:edge_max=4"),
        (
            "--ascii --ascii-edge-depth 0.3",
            "--style ascii --set ascii.edge_depth=0.3",
        ),
        (
            "--ascii --ascii-edge-normal 1.5",
            "--style ascii --set ascii.edge_normal=1.5",
        ),
        ("--ascii-dither 0.5", "--style ascii:dither=0.5"),
        ("--ascii-exposure -1", "--style ascii:exposure=-1"),
        ("--ascii-normalize 1", "--style ascii:normalize=1"),
        (
            "--ascii --ascii-no-edges",
            "--style ascii --set ascii.edges=0",
        ),
    ];

    #[test]
    fn every_old_flag_is_its_new_spelling() {
        let plain = config("");
        for (old, new) in SPELLINGS {
            let was = config(old);
            assert_ne!(was, plain, "{old} changes nothing");
            assert_eq!(was, config(new), "{old} against {new}");
        }
    }

    #[test]
    fn the_deprecation_note_prints_the_new_spelling() {
        for (old, new) in SPELLINGS {
            let spelled: Vec<String> = legacy_flags_given(&args(old).legacy)
                .iter()
                .map(LegacyFlag::spelling)
                .collect();
            assert_eq!(spelled.join(" "), *new, "{old}");
        }
    }

    /// The old flags keep the order they applied in on main (#708 checked
    /// every row of [`SPELLINGS`] and these against main's mapping, copied
    /// in, before deleting it): the render mode after the tear, the edge
    /// switch after the edge strength, a tint's colour mode after a
    /// palette's, --style over all of them.
    #[test]
    fn the_old_flags_keep_their_order() {
        let c = |line| {
            post_process_config(
                &args(line),
                None,
                &flint_render::stylize::StyleRegistry::new(),
            )
            .unwrap()
        };
        assert_eq!(c("--tear 0.3 --render-mode 1 --mode-mix 0.5").tear, 0.5);
        assert_eq!(
            config("--ascii-edges 1 --ascii-no-edges"),
            config("--style ascii:edges=0")
        );
        assert_eq!(
            config("--ascii-palette cga --ascii-tint 1,0,0"),
            config("--style ascii:palette=cga,tint=1/0/0,color=tint")
        );
        assert_eq!(
            config("--ascii --ascii-cell 6 --style ascii:cell=9"),
            config("--style ascii:cell=9")
        );
    }

    /// Every hidden flag has a row in [`SPELLINGS`], but the render mode's.
    #[test]
    fn every_hidden_flag_has_a_spelling() {
        use clap::CommandFactory;
        let command = Cli::command();
        for arg in command.get_arguments().filter(|a| a.is_hide_set()) {
            let long = format!("--{}", arg.get_long().unwrap());
            if ["--render-mode", "--mode-mix", "--mode-params"].contains(&long.as_str()) {
                continue;
            }
            assert!(
                SPELLINGS
                    .iter()
                    .any(|(old, _)| old.split_whitespace().any(|w| w == long)),
                "{long} has no row in SPELLINGS"
            );
        }
    }

    #[test]
    fn set_checks_the_key_and_the_kind() {
        let s = parse_set("post.exposure=2").unwrap();
        assert_eq!(s.key, SetKey::Post("exposure"));
        let e = parse_set("post.exposur=2").unwrap_err();
        assert!(e.contains("did you mean post.exposure"), "{e}");
        let e = parse_set("post.fog_color=1,2").unwrap_err();
        assert!(e.contains("three numbers"), "{e}");
        let e = parse_set("post.ssao_samples=1.5").unwrap_err();
        assert!(e.contains("whole number"), "{e}");
        assert!(parse_set("exposure=2").is_err());
        assert!(parse_set("post.exposure").is_err());
        let e = parse_set("ascii.enabled=true").unwrap_err();
        assert!(e.contains("--style ascii"), "{e}");
        // A style key waits for the registry: a user style may be named
        let s = parse_set("my_style.glow=2").unwrap();
        assert!(
            s.check(&flint_render::stylize::StyleRegistry::new())
                .is_err(),
            "an unknown style fails its check"
        );
        assert!(parse_set("ascii.cel=3")
            .unwrap()
            .check(&flint_render::stylize::StyleRegistry::new())
            .is_err());
        assert!(parse_set("ascii.cell=12")
            .unwrap()
            .check(&flint_render::stylize::StyleRegistry::new())
            .is_ok());
    }

    #[test]
    fn a_style_key_tunes_without_turning_the_style_on() {
        let c = post_process_config(
            &args("--set ascii.cell=12"),
            None,
            &flint_render::stylize::StyleRegistry::new(),
        )
        .unwrap();
        assert!(!c.styles.is_enabled("ascii"));
        let on = post_process_config(
            &args("--set ascii.cell=12 --style ascii"),
            None,
            &flint_render::stylize::StyleRegistry::new(),
        )
        .unwrap();
        assert!(on.styles.is_enabled("ascii"));
        assert_eq!(
            format!("{on:?}"),
            config("--style ascii:cell=12"),
            "the key is kept when --style turns it on"
        );
    }

    #[test]
    fn set_applies_in_order_and_style_last() {
        assert_eq!(
            config("--set post.exposure=1 --set post.exposure=3"),
            config("--set post.exposure=3")
        );
        assert_eq!(
            config("--style ascii:cell=8 --set ascii.cell=12"),
            config("--style ascii:cell=8"),
            "--style applies after --set"
        );
        // An old flag is under --set
        assert_eq!(
            config("--exposure 2 --set post.exposure=3"),
            config("--set post.exposure=3")
        );
    }

    #[test]
    fn a_set_of_a_swept_key_is_refused() {
        let e = check(
            &args("--set post.exposure=2 --sweep post.exposure=1,2"),
            &flint_render::stylize::StyleRegistry::new(),
        )
        .unwrap_err();
        assert!(e.to_string().contains("drop the --set"), "{e}");
        assert!(check(
            &args("--set post.exposure=2 --sweep post.desaturate=0,1"),
            &flint_render::stylize::StyleRegistry::new()
        )
        .is_ok());
    }
}
