//! Scene file format definitions

use flint_core::toml_util::edit_distance;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};

/// Root structure of a scene TOML file
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SceneFile {
    pub scene: SceneMetadata,
    #[serde(default)]
    pub camera: Option<CameraDef>,
    #[serde(default)]
    pub environment: Option<EnvironmentDef>,
    #[serde(default)]
    pub post_process: Option<PostProcessDef>,
    #[serde(default)]
    pub living: Option<LivingDef>,
    #[serde(default)]
    pub wooze: Option<WoozeDef>,
    #[serde(default)]
    pub prefabs: HashMap<String, PrefabInstance>,
    #[serde(default)]
    pub entities: HashMap<String, EntityDef>,
}

/// A scene's `[wooze]` table (#213): how woozy it is everywhere, and the zones
/// that are more or less so. A scene without one has no field and pays nothing.
/// [`WoozeDef::field`](crate::WoozeDef::field) makes it into one, and
/// [`WoozeDef::check`](crate::WoozeDef::check) is what `flint validate` runs.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WoozeDef {
    /// A built-in set of knobs to start from. Unset: everything as authored.
    #[serde(default)]
    pub preset: Option<String>,
    /// How much of a consumer's slow drift shows, 0 to 1.
    #[serde(default)]
    pub amplitude: Option<f32>,
    /// Seconds between pose targets.
    #[serde(default)]
    pub step: Option<f32>,
    /// How much of a step is spent easing, 0 to 1.
    #[serde(default)]
    pub glide: Option<f32>,
    /// How much of a consumer's fast jitter shows.
    #[serde(default)]
    pub jitter: Option<f32>,
    /// `[[wooze.zone]]` entries. Raw, so that one that's wrong can be named
    /// rather than failing the whole scene's parse with a line number.
    #[serde(default)]
    pub zone: Vec<toml::Value>,
}

/// A scene's `[living]` table: what it says about all its living props, ahead
/// of any one spec. How woozy they are is `[wooze]`'s to say.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LivingDef {
    /// What the shadows follow: `blended` (what's drawn; the default) or
    /// `drift` (a slower pose with no stagger or jitter).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shadows: Option<String>,
    /// How parts fall out of phase, over each spec's and each prop's own: `none`,
    /// `per_level` or `per_branch`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stagger: Option<String>,
    /// Faint time-offset copies of every prop. Fields left unset are each spec's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub echoes: Option<LivingEchoesDef>,
    /// How much of the difference between poses is left. Fields left unset are
    /// each spec's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variance: Option<LivingVarianceDef>,
    /// `[living.species.<spec>]` (#246): one species' own values, over everything
    /// above, a field at a time. Keyed by the spec's `meta.name`, which is what a
    /// living prop's `model.asset` names.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub species: BTreeMap<String, LivingSpeciesDef>,
}

/// One species' values (#246): the clock knobs `[wooze]` has and the props'
/// knobs `[living]` has, each beating the global one, zones included. Unset is
/// whatever the layers below say.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LivingSpeciesDef {
    /// How much of its slow drift shows, 0 to 1. The panel calls it drift.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub amplitude: Option<f32>,
    /// Seconds between pose targets.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step: Option<f32>,
    /// How much of a step is spent easing, 0 to 1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub glide: Option<f32>,
    /// How much of its fast jitter shows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jitter: Option<f32>,
    /// `none`, `per_level` or `per_branch`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stagger: Option<String>,
    /// Its echoes, a field at a time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub echoes: Option<LivingEchoesDef>,
    /// Its variance, a field at a time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variance: Option<LivingVarianceDef>,
}

/// A scene's `[living] echoes`, over each spec's a field at a time.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LivingEchoesDef {
    /// How many copies; 0 turns them off.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub count: Option<u32>,
    /// How far the farthest copy is from now, in steps of the clock.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spread: Option<f32>,
    /// The opacity of a copy at now, which falls off with distance from it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alpha: Option<f32>,
}

/// A saved look (#218): a `<name>.wooze.toml` beside a scene, which its `[wooze]
/// preset` can name. The tables are the shapes a scene's own take, so one can be
/// pasted into the other. Zones aren't here, since they belong to a place.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WoozePreset {
    /// The field's global knobs.
    #[serde(default)]
    pub wooze: WoozeKnobs,
    /// What the scene says about its props: stagger, echoes, variance.
    #[serde(default)]
    pub living: LivingDef,
    /// The shading boil.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub boil: Option<BoilDef>,
}

/// A wooze field's global knobs: whichever a preset sets.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WoozeKnobs {
    /// How much of a consumer's slow drift shows, 0 to 1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub amplitude: Option<f32>,
    /// Seconds between pose targets.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step: Option<f32>,
    /// How much of a step is spent easing, 0 to 1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub glide: Option<f32>,
    /// How much of a consumer's fast jitter shows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jitter: Option<f32>,
}

/// The shading boil in a preset, which is `kuwahara_boil`, `contour_boil` and
/// `boil_on` from a scene's `[post_process]`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BoilDef {
    /// Whether it boils at all. Off keeps the amounts for when it's back on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    /// How far each drawing moves the paint.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kuwahara: Option<f32>,
    /// How far each drawing moves the lines.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contour: Option<f32>,
    /// A new drawing every this many film frames: 1, 2 or 3.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on: Option<u32>,
}

/// A scene's `[living] variance`, over each spec's a field at a time.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LivingVarianceDef {
    /// How much of each pose's difference from the base is left, 0 to 1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub amount: Option<f32>,
    /// How much of that is held back from the trunk side: 0 even, 1 all at the
    /// tips.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub steady: Option<f32>,
    /// Scale each pose so its box matches the base's, as near as one scale can.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keep_size: Option<bool>,
}

/// A prefab template file (.prefab.toml)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrefabFile {
    pub prefab: PrefabMetadata,
    #[serde(default)]
    pub entities: HashMap<String, EntityDef>,
}

/// Metadata for a prefab template
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrefabMetadata {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
}

/// An instance of a prefab in a scene
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrefabInstance {
    pub template: String,
    pub prefix: String,
    #[serde(default)]
    pub overrides: HashMap<String, HashMap<String, toml::Value>>,
}

/// Post-processing settings for the scene
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PostProcessDef {
    #[serde(default = "default_true")]
    pub bloom_enabled: bool,
    #[serde(default = "default_bloom_intensity")]
    pub bloom_intensity: f32,
    #[serde(default = "default_bloom_threshold")]
    pub bloom_threshold: f32,
    #[serde(default)]
    pub vignette_enabled: bool,
    #[serde(default = "default_vignette_intensity")]
    pub vignette_intensity: f32,
    #[serde(default = "default_vignette_smoothness")]
    pub vignette_smoothness: f32,
    #[serde(default = "default_exposure")]
    pub exposure: f32,
    #[serde(default = "default_true")]
    pub ssao_enabled: bool,
    #[serde(default = "default_ssao_radius")]
    pub ssao_radius: f32,
    #[serde(default = "default_ssao_intensity")]
    pub ssao_intensity: f32,
    /// SSAO hemisphere samples per pixel, 1-64 (default 64 = full quality).
    /// The heaviest per-pixel cost in the post stack; 16 is usually
    /// indistinguishable on soft/matte scenes and ~4x cheaper.
    #[serde(default = "default_ssao_samples")]
    pub ssao_samples: u32,
    #[serde(default)]
    pub fog_enabled: bool,
    #[serde(default = "default_fog_color")]
    pub fog_color: [f32; 3],
    #[serde(default = "default_fog_density")]
    pub fog_density: f32,
    #[serde(default = "default_fog_start")]
    pub fog_start: f32,
    #[serde(default = "default_fog_end")]
    pub fog_end: f32,
    #[serde(default)]
    pub fog_height_enabled: bool,
    #[serde(default = "default_fog_height_falloff")]
    pub fog_height_falloff: f32,
    #[serde(default)]
    pub fog_height_origin: f32,
    /// Height fog's density at `fog_height_origin` (ADR 0101); 0 adds none.
    #[serde(default)]
    pub fog_height_density: f32,
    /// Aerial tint (ADR 0101): far things take this colour, up to
    /// `fog_tint_strength` (0 = off), over `fog_tint_start..fog_tint_end` m.
    #[serde(default = "default_fog_tint_color")]
    pub fog_tint_color: [f32; 3],
    #[serde(default)]
    pub fog_tint_strength: f32,
    #[serde(default = "default_fog_tint_start")]
    pub fog_tint_start: f32,
    #[serde(default = "default_fog_tint_end")]
    pub fog_tint_end: f32,
    #[serde(default)]
    pub dither_enabled: bool,
    #[serde(default = "default_dither_intensity")]
    pub dither_intensity: f32,
    #[serde(default)]
    pub volumetric_enabled: bool,
    #[serde(default = "default_volumetric_samples")]
    pub volumetric_samples: u32,
    #[serde(default = "default_volumetric_density")]
    pub volumetric_density: f32,
    #[serde(default = "default_volumetric_max_distance")]
    pub volumetric_max_distance: f32,
    #[serde(default = "default_volumetric_decay")]
    pub volumetric_decay: f32,
    #[serde(default)]
    pub chromatic_aberration: f32,
    #[serde(default)]
    pub radial_blur: f32,
    #[serde(default)]
    pub desaturate: f32,
    #[serde(default)]
    pub dof_strength: f32,
    #[serde(default = "default_dof_focus_distance")]
    pub dof_focus_distance: f32,
    #[serde(default = "default_dof_focus_range")]
    pub dof_focus_range: f32,
    /// The old spellings of `[post_process.styles.kuwahara]` (#298), kept
    /// so scenes written before the style table still load. Read them
    /// through [`style_tables`](Self::style_tables), which folds them in;
    /// a key the table also sets is the table's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kuwahara_enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kuwahara_radius: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kuwahara_sharpness: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kuwahara_hardness: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kuwahara_anisotropy: Option<f32>,
    /// The shared geometry edges' thresholds (#301), which every style that
    /// reads them sees: the relative view-distance step that is a
    /// silhouette, and the normal change (Sobel) that is a crease.
    #[serde(default = "default_edge_depth")]
    pub edge_depth: f32,
    #[serde(default = "default_edge_normal")]
    pub edge_normal: f32,
    /// Stylize passes by name (#262): `[post_process.styles.invert]` with
    /// `enabled` (default true) and the style's numeric parameters. The
    /// renderer checks names and parameters against its registry.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub styles: std::collections::BTreeMap<String, toml::Table>,
    /// How far each drawing moves the paint in a scene with a `[wooze]` table: a
    /// share of the radius, turning the strokes too. 0 holds it still.
    #[serde(default)]
    pub kuwahara_boil: f32,
    /// Ink lines over the scene, from breaks in depth and luminance (#214).
    #[serde(default)]
    pub contour_enabled: bool,
    /// Half a line's width, in pixels at 1080p.
    #[serde(default = "default_contour_width")]
    pub contour_width: f32,
    /// How strong an edge must be before it starts to ink. Lower draws more.
    #[serde(default = "default_contour_threshold")]
    pub contour_threshold: f32,
    /// How dark the ink goes, 0 to 1.
    #[serde(default = "default_contour_strength")]
    pub contour_strength: f32,
    /// How much edges in luminance count, against edges in depth. 0 is silhouettes only.
    #[serde(default = "default_contour_luma")]
    pub contour_luma: f32,
    /// The ink's colour, linear.
    #[serde(default = "default_contour_color")]
    pub contour_color: [f32; 3],
    /// Metres at which a line has faded by half, so that distant foliage doesn't
    /// go solid with ink. 0 never fades.
    #[serde(default = "default_contour_fade")]
    pub contour_fade: f32,
    /// How far each drawing moves the lines in a scene with a `[wooze]` table,
    /// and how much it changes their width and pressure. 0 holds them still.
    #[serde(default)]
    pub contour_boil: f32,
    /// Film frames each drawing is held: 1, 2 or 3 ("on twos"). A beat of the
    /// field's `step` is six film frames.
    #[serde(default = "default_boil_on")]
    pub boil_on: u32,
    /// Animated film grain intensity (0 = off; ~0.02-0.05 is subtle).
    #[serde(default)]
    pub film_grain: f32,
    /// Color grade lift (per-channel add after ACES). Neutral = [0,0,0].
    #[serde(default)]
    pub grade_lift: [f32; 3],
    /// Color grade gamma (per-channel midtone curve). Neutral = [1,1,1].
    #[serde(default = "default_grade_one")]
    pub grade_gamma: [f32; 3],
    /// Color grade gain (per-channel multiply). Neutral = [1,1,1].
    #[serde(default = "default_grade_one")]
    pub grade_gain: [f32; 3],
    /// FXAA anti-aliasing pass on the final composite (default off — the
    /// headless pixel-diff gates run without it; ADR 0050).
    #[serde(default)]
    pub fxaa: bool,
    /// A look (#356): a `styles/<name>.look.toml` beside the scene (or its
    /// parent's `styles/`), or a built-in (`ascii_noir`, `graphite`, ...),
    /// laid under this block's own keys. `flint_render::look::resolve`
    /// applies it; until then the struct holds the scene's keys alone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub look: Option<String>,
    /// The keys the scene's `[post_process]` wrote, as written (the loader
    /// records them), so a look goes under exactly those and a later
    /// `set_look` can re-lay a different one. `None` for a block built in
    /// code, which then counts every field as its own.
    #[serde(skip)]
    pub authored: Option<toml::Table>,
}

fn default_grade_one() -> [f32; 3] {
    [1.0, 1.0, 1.0]
}

fn default_true() -> bool {
    true
}

fn default_bloom_intensity() -> f32 {
    0.04
}

fn default_bloom_threshold() -> f32 {
    1.0
}

fn default_vignette_intensity() -> f32 {
    0.3
}

fn default_vignette_smoothness() -> f32 {
    2.0
}

fn default_exposure() -> f32 {
    1.0
}

fn default_ssao_radius() -> f32 {
    0.5
}

fn default_ssao_intensity() -> f32 {
    1.0
}

fn default_ssao_samples() -> u32 {
    64
}

fn default_fog_color() -> [f32; 3] {
    [0.7, 0.75, 0.82]
}

fn default_fog_density() -> f32 {
    0.02
}

fn default_fog_start() -> f32 {
    5.0
}

fn default_fog_end() -> f32 {
    100.0
}

fn default_fog_height_falloff() -> f32 {
    0.1
}

fn default_fog_tint_color() -> [f32; 3] {
    [0.42, 0.50, 0.65]
}

fn default_fog_tint_start() -> f32 {
    100.0
}

fn default_fog_tint_end() -> f32 {
    1000.0
}

fn default_dof_focus_distance() -> f32 {
    10.0
}

fn default_dof_focus_range() -> f32 {
    5.0
}

fn default_dither_intensity() -> f32 {
    0.03
}

fn default_volumetric_samples() -> u32 {
    32
}

fn default_volumetric_density() -> f32 {
    1.0
}

fn default_volumetric_max_distance() -> f32 {
    100.0
}

fn default_volumetric_decay() -> f32 {
    0.98
}

/// The `[post_process]` keys that predate the style table, and the
/// `[post_process.styles.<style>]` key each one is now (#298).
pub const LEGACY_STYLE_KEYS: &[(&str, &str, &str)] = &[
    ("kuwahara_enabled", "kuwahara", "enabled"),
    ("kuwahara_radius", "kuwahara", "radius"),
    ("kuwahara_sharpness", "kuwahara", "sharpness"),
    ("kuwahara_hardness", "kuwahara", "hardness"),
    ("kuwahara_anisotropy", "kuwahara", "anisotropy"),
];

/// Move a `[post_process]` table's old style keys (`kuwahara_radius = 4`)
/// into its style tables (`[styles.kuwahara] radius = 4`), so the two
/// spellings mean the same thing. A key the style table already sets is
/// the table's, and the old key is dropped. A style table made here is on
/// only if the old keys say so (`kuwahara_enabled = true`): before the
/// table, `kuwahara_radius` alone left the filter off.
pub fn fold_legacy_style_keys(post_process: &mut toml::Table) {
    let mut made: Vec<&str> = Vec::new();
    for &(old, style, key) in LEGACY_STYLE_KEYS {
        let Some(value) = post_process.remove(old) else {
            continue;
        };
        let styles = post_process
            .entry("styles")
            .or_insert_with(|| toml::Value::Table(toml::Table::new()));
        let Some(styles) = styles.as_table_mut() else {
            continue;
        };
        if !styles.contains_key(style) {
            let mut fresh = toml::Table::new();
            fresh.insert("enabled".into(), toml::Value::Boolean(false));
            styles.insert(style.into(), toml::Value::Table(fresh));
            made.push(style);
        }
        let Some(table) = styles.get_mut(style).and_then(|t| t.as_table_mut()) else {
            continue;
        };
        // `enabled` in a table made here is the old key's to set; in an
        // authored one it is already the table's (absent means on)
        if key == "enabled" {
            if made.contains(&style) {
                table.insert(key.into(), value);
            }
        } else {
            table.entry(key).or_insert(value);
        }
    }
}

/// The name in `known` closest to `key` by edit distance, if within 3.
fn closest_name(key: &str, known: &[String]) -> Option<String> {
    known
        .iter()
        .map(|k| (edit_distance(key, k), k))
        .filter(|(d, _)| *d <= 3)
        .min_by_key(|(d, _)| *d)
        .map(|(_, k)| k.clone())
}

fn default_edge_depth() -> f32 {
    0.6
}

fn default_edge_normal() -> f32 {
    2.5
}

impl PostProcessDef {
    /// Every key a `[post_process]` block can name, sorted: the struct's
    /// own fields, read back from a serialized one (so a new field is
    /// known the moment it exists), with the ones skipped when empty
    /// (`kuwahara_*`, `styles`) filled in so they serialize too.
    pub fn field_names() -> Vec<String> {
        let mut def: PostProcessDef = toml::Value::Table(toml::Table::new())
            .try_into()
            .expect("an empty [post_process] deserializes");
        def.kuwahara_enabled = Some(false);
        def.kuwahara_radius = Some(0);
        def.kuwahara_sharpness = Some(0.0);
        def.kuwahara_hardness = Some(0.0);
        def.kuwahara_anisotropy = Some(0.0);
        def.styles.insert("_".into(), toml::Table::new());
        def.look = Some(String::new());
        let mut names: Vec<String> = toml::Table::try_from(&def)
            .map(|t| t.keys().cloned().collect())
            .unwrap_or_default();
        names.sort();
        names
    }

    /// The known field closest to `key`, when one is within three edits.
    pub fn closest_field(key: &str) -> Option<String> {
        closest_name(key, &Self::field_names())
    }

    /// The keys of a raw `[post_process]` table that are no field of
    /// this struct (#354), each with the closest known name. Serde drops
    /// them without a word, so a typo would leave the default in place.
    pub fn unknown_fields(raw: &toml::Table) -> Vec<(String, Option<String>)> {
        let known = Self::field_names();
        raw.keys()
            .filter(|key| !known.iter().any(|k| k == *key))
            .map(|key| (key.clone(), closest_name(key, &known)))
            .collect()
    }

    /// [`Self::unknown_fields`] of a whole scene file's text, as warning
    /// lines naming `source` (the file, when there is one). Empty when
    /// the text has no `[post_process]` or doesn't parse (the loader
    /// reports a parse error itself).
    pub fn unknown_field_warnings(scene_text: &str, source: Option<&str>) -> Vec<String> {
        let Ok(doc) = toml::from_str::<toml::Table>(scene_text) else {
            return Vec::new();
        };
        let Some(toml::Value::Table(post)) = doc.get("post_process") else {
            return Vec::new();
        };
        let at = source.map(|s| format!("{s}: ")).unwrap_or_default();
        Self::unknown_fields(post)
            .into_iter()
            .map(|(key, near)| {
                let hint = near
                    .map(|n| format!(", did you mean '{n}'?"))
                    .unwrap_or_default();
                format!("{at}[post_process] has no field '{key}'{hint} (ignored)")
            })
            .collect()
    }

    /// A scene file's `[post_process]` table as written, for
    /// [`Self::authored`]. `None` when there is none or the text doesn't
    /// parse.
    pub fn authored_table(scene_text: &str) -> Option<toml::Table> {
        let mut doc = toml::from_str::<toml::Table>(scene_text).ok()?;
        match doc.remove("post_process") {
            Some(toml::Value::Table(post)) => Some(post),
            _ => None,
        }
    }

    /// A `[post_process]` block from its table, remembering the table as
    /// what it authors (as the loader does), for blocks built in code that
    /// a look should go under (#356).
    pub fn from_authored(table: toml::Table) -> Result<Self, toml::de::Error> {
        let mut def: Self = toml::Value::Table(table.clone()).try_into()?;
        def.authored = Some(table);
        Ok(def)
    }

    /// The style tables, with the old `kuwahara_*` keys folded in
    /// ([`fold_legacy_style_keys`]). What the renderer reads.
    pub fn style_tables(&self) -> std::collections::BTreeMap<String, toml::Table> {
        let mut table = toml::Table::new();
        let float = |v: Option<f32>| v.map(|v| toml::Value::Float(v as f64));
        let legacy = [
            (
                "kuwahara_enabled",
                self.kuwahara_enabled.map(toml::Value::Boolean),
            ),
            (
                "kuwahara_radius",
                self.kuwahara_radius.map(|r| toml::Value::Integer(r as i64)),
            ),
            ("kuwahara_sharpness", float(self.kuwahara_sharpness)),
            ("kuwahara_hardness", float(self.kuwahara_hardness)),
            ("kuwahara_anisotropy", float(self.kuwahara_anisotropy)),
        ];
        for (key, value) in legacy {
            if let Some(value) = value {
                table.insert(key.into(), value);
            }
        }
        let mut styles: std::collections::BTreeMap<String, toml::Table> = self.styles.clone();
        if table.is_empty() {
            return styles;
        }
        table.insert(
            "styles".into(),
            toml::Value::Table(
                styles
                    .iter()
                    .map(|(k, v)| (k.clone(), toml::Value::Table(v.clone())))
                    .collect(),
            ),
        );
        fold_legacy_style_keys(&mut table);
        styles.clear();
        if let Some(toml::Value::Table(folded)) = table.remove("styles") {
            for (name, value) in folded {
                if let toml::Value::Table(t) = value {
                    styles.insert(name, t);
                }
            }
        }
        styles
    }
}

fn default_contour_width() -> f32 {
    1.25
}

fn default_contour_threshold() -> f32 {
    0.08
}

fn default_contour_strength() -> f32 {
    0.9
}

fn default_contour_luma() -> f32 {
    0.5
}

fn default_contour_color() -> [f32; 3] {
    [0.015, 0.012, 0.02]
}

fn default_contour_fade() -> f32 {
    30.0
}

fn default_boil_on() -> u32 {
    2
}

/// Camera configuration for the scene
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CameraDef {
    /// Projection type: "perspective" or "orthographic"
    #[serde(default = "default_projection")]
    pub projection: String,
    /// Orthographic half-height in world units (only used when projection = "orthographic")
    #[serde(default)]
    pub ortho_height: f32,
    /// Camera position [x, y, z]
    #[serde(default)]
    pub position: Option<[f32; 3]>,
    /// Camera look-at target [x, y, z]
    #[serde(default)]
    pub target: Option<[f32; 3]>,
    /// Field of view in degrees (perspective only)
    #[serde(default)]
    pub fov: Option<f32>,
    /// Near clipping plane
    #[serde(default)]
    pub near: Option<f32>,
    /// Far clipping plane
    #[serde(default)]
    pub far: Option<f32>,
}

fn default_projection() -> String {
    "perspective".to_string()
}

/// Environment settings for the scene (skybox, etc.)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvironmentDef {
    /// Path to equirectangular panorama image for the skybox
    #[serde(default)]
    pub skybox: Option<String>,
    /// Hemisphere ambient sky color [r, g, b] (linear). Absent keeps the
    /// renderer's built-in default.
    #[serde(default)]
    pub ambient_sky: Option<[f32; 3]>,
    /// Hemisphere ambient ground color [r, g, b] (linear). Absent keeps the
    /// renderer's built-in default.
    #[serde(default)]
    pub ambient_ground: Option<[f32; 3]>,
    /// Diffuse terminator wrap (0 = physically sharp, ~0.2-0.5 = soft matte /
    /// subsurface-ish). Absent = 0 = exact legacy shading.
    #[serde(default)]
    pub diffuse_wrap: Option<f32>,
    /// Blend from Lambert diffuse toward the Fujii Oren-Nayar approximation
    /// (0..1; sigma comes from material roughness — this only blends).
    /// Rough matte surfaces get a flatter, chalkier falloff. Absent = 0 =
    /// exact legacy shading (ADR 0048).
    #[serde(default)]
    pub oren_nayar: Option<f32>,
    /// Charlie-sheen rim tint [r, g, b] (linear). Only meaningful with a
    /// non-zero `sheen_strength`. Absent = white.
    #[serde(default)]
    pub sheen_color: Option<[f32; 3]>,
    /// Charlie-sheen rim strength (0..~0.3 — no energy compensation, high
    /// values over-brighten). Absent = 0 = exact legacy shading (ADR 0048).
    #[serde(default)]
    pub sheen_strength: Option<f32>,
}

/// Scene metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SceneMetadata {
    pub name: String,
    #[serde(default = "default_version")]
    pub version: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub input_config: Option<String>,
    /// When false, the player skips the blanket preload of every file under
    /// `audio/` at scene load (a silent scene starts instantly). Sounds named
    /// by `audio_source` components still load, and music-session stems load
    /// through their own resolver — this gates only the script-triggered-
    /// sound convenience preload. Default true = original behavior.
    #[serde(default = "default_true")]
    pub preload_audio: bool,
}

fn default_version() -> String {
    "1.0".to_string()
}

/// Definition of an entity in a scene file
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EntityDef {
    /// Optional archetype name
    #[serde(skip_serializing_if = "Option::is_none")]
    pub archetype: Option<String>,
    /// Optional parent entity name
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    /// Component data - all other fields are treated as components
    #[serde(flatten)]
    pub components: HashMap<String, toml::Value>,
}

impl EntityDef {
    pub fn new() -> Self {
        Self {
            archetype: None,
            parent: None,
            components: HashMap::new(),
        }
    }

    pub fn with_archetype(mut self, archetype: impl Into<String>) -> Self {
        self.archetype = Some(archetype.into());
        self
    }

    pub fn with_parent(mut self, parent: impl Into<String>) -> Self {
        self.parent = Some(parent.into());
        self
    }

    pub fn with_component(mut self, name: impl Into<String>, data: toml::Value) -> Self {
        self.components.insert(name.into(), data);
        self
    }
}

impl SceneFile {
    /// Create a new scene file
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            scene: SceneMetadata {
                name: name.into(),
                version: default_version(),
                description: None,
                input_config: None,
                preload_audio: true,
            },
            camera: None,
            environment: None,
            post_process: None,
            living: None,
            wooze: None,
            prefabs: HashMap::new(),
            entities: HashMap::new(),
        }
    }

    /// Add an entity to the scene
    pub fn add_entity(&mut self, name: impl Into<String>, entity: EntityDef) {
        self.entities.insert(name.into(), entity);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_scene_file_serialization() {
        let mut scene = SceneFile::new("Test Scene");
        scene.add_entity(
            "door1",
            EntityDef::new()
                .with_archetype("door")
                .with_component("door", toml::toml! { locked = false }.into()),
        );

        let toml_str = toml::to_string_pretty(&scene).unwrap();
        assert!(toml_str.contains("Test Scene"));
        assert!(toml_str.contains("door1"));
    }

    #[test]
    fn test_scene_file_deserialization() {
        let toml_str = r#"
[scene]
name = "Test Scene"
version = "1.0"

[entities.room1]
archetype = "room"

[entities.room1.bounds]
min = [0, 0, 0]
max = [10, 4, 8]
"#;

        let scene: SceneFile = toml::from_str(toml_str).unwrap();
        assert_eq!(scene.scene.name, "Test Scene");
        assert!(scene.entities.contains_key("room1"));
    }
}

#[cfg(test)]
mod legacy_style_key_tests {
    use super::*;

    fn def(text: &str) -> PostProcessDef {
        toml::from_str(text).unwrap()
    }

    #[test]
    fn old_kuwahara_keys_fold_into_its_table() {
        let old = def("kuwahara_enabled = true\nkuwahara_radius = 4\nkuwahara_sharpness = 6.0");
        let tables = old.style_tables();
        let k = &tables["kuwahara"];
        assert_eq!(k["enabled"].as_bool(), Some(true));
        assert_eq!(k["radius"].as_integer(), Some(4));
        assert_eq!(k["sharpness"].as_float(), Some(6.0));
    }

    #[test]
    fn old_keys_without_enabled_leave_it_off() {
        let tables = def("kuwahara_radius = 6").style_tables();
        assert_eq!(tables["kuwahara"]["enabled"].as_bool(), Some(false));
        assert!(def("bloom_intensity = 0.1").style_tables().is_empty());
    }

    #[test]
    fn the_table_wins_over_an_old_key() {
        let both =
            def("kuwahara_enabled = false\nkuwahara_radius = 2\n[styles.kuwahara]\nradius = 5");
        let k = &both.style_tables()["kuwahara"];
        assert_eq!(k["radius"].as_integer(), Some(5));
        // An authored table is on unless it says otherwise
        assert!(!k.contains_key("enabled"));
    }

    #[test]
    fn unknown_post_process_fields_warn_with_the_closest_name() {
        let scene = "[scene]\nname = \"t\"\n\n[post_process]\n\
                     bloom_intensty = 0.1\nexposure = 1.5\nzzzzzzzzzz = 1\n\
                     kuwahara_radius = 4\n[post_process.styles.ascii]\ncell = 8\n";
        let warnings = PostProcessDef::unknown_field_warnings(scene, Some("levels/a.scene.toml"));
        assert_eq!(
            warnings,
            [
                "levels/a.scene.toml: [post_process] has no field 'bloom_intensty', \
                 did you mean 'bloom_intensity'? (ignored)",
                "levels/a.scene.toml: [post_process] has no field 'zzzzzzzzzz' (ignored)",
            ]
        );
        // Known fields, the legacy kuwahara keys and `styles` pass
        assert!(PostProcessDef::unknown_field_warnings(
            "[post_process]\nfxaa = true\nkuwahara_enabled = true\n[post_process.styles.x]\n",
            None
        )
        .is_empty());
        assert!(PostProcessDef::unknown_field_warnings("[scene]\nname = \"t\"\n", None).is_empty());
    }

    #[test]
    fn field_names_cover_every_field() {
        let names = PostProcessDef::field_names();
        for f in [
            "bloom_intensity",
            "fxaa",
            "styles",
            "kuwahara_anisotropy",
            "grade_gain",
        ] {
            assert!(names.iter().any(|n| n == f), "{f} missing");
        }
        assert_eq!(
            PostProcessDef::closest_field("expsure").as_deref(),
            Some("exposure")
        );
    }

    #[test]
    fn folding_a_raw_table_drops_the_old_keys() {
        let mut t: toml::Table = toml::from_str("kuwahara_enabled = true\nexposure = 2.0").unwrap();
        fold_legacy_style_keys(&mut t);
        assert!(!t.contains_key("kuwahara_enabled"));
        assert_eq!(t["styles"]["kuwahara"]["enabled"].as_bool(), Some(true));
        assert_eq!(t["exposure"].as_float(), Some(2.0));
    }
}
