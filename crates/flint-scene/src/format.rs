//! Scene file format definitions

use flint_core::toml_util::edit_distance;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Root structure of a scene TOML file: `[scene]`, the scene-level tables,
/// `[prefabs.*]` and `[entities.*]`.
///
/// A scene-level table is any other top-level table. The ones flint-scene
/// reads itself are fields ([`SceneFile::TYPED_TABLES`]); **every other one
/// is kept as written in [`tables`](Self::tables)** (#686) and written back
/// by `toml::to_string`, so a game's own `[weather]` survives a load and
/// reaches whoever reads it (a `PlayerExtension`, through
/// `ExtensionCtx::scene_table`). The engine's own features read theirs the
/// same way: `[living]` and `[wooze]` are in `tables`, and the crate that
/// owns living props gives them their types (`flint_procgen::LivingTables`).
/// A `[table.<name>]` schema declares one;
/// [`diagnose`](crate::diagnose) warns about one no schema declares
/// (`unknown-table`) and checks the fields of the ones that are declared.
#[derive(Debug, Clone, Serialize)]
pub struct SceneFile {
    pub scene: SceneMetadata,
    pub camera: Option<CameraDef>,
    pub environment: Option<EnvironmentDef>,
    pub post_process: Option<PostProcessDef>,
    /// Every top-level table that is none of the above, by name, as the
    /// file wrote it: a feature's (`[living]`, `[wooze]`), a game's or an
    /// extension's own.
    #[serde(flatten)]
    pub tables: toml::Table,
    /// By name, as `entities` is
    pub prefabs: BTreeMap<String, PrefabInstance>,
    /// By name (#688): the loader spawns in this order, so a scene's ids
    /// are the same every load, and a saved file lists entities the same
    /// way every time
    pub entities: BTreeMap<String, EntityDef>,
    /// The typed tables ([`SceneFile::TYPED_TABLES`]) as the file wrote
    /// them, before their defaults were filled in: what
    /// [`diagnose`](crate::diagnose) checks against their `[table.*]`
    /// schemas, so a misspelled key serde dropped is still named.
    /// [`SceneFile::parse`] and the loaders record it; a scene built in code
    /// or read with `toml::from_str` has none, and its typed tables are not
    /// checked.
    #[serde(skip)]
    pub written: toml::Table,
}

impl<'de> Deserialize<'de> for SceneFile {
    /// Each known table straight into its type (so an error keeps its line
    /// and column, which `#[serde(flatten)]` would lose), and every other
    /// top-level key into [`tables`](SceneFile::tables).
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct SceneVisitor;

        impl<'de> serde::de::Visitor<'de> for SceneVisitor {
            type Value = SceneFile;

            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("a scene file")
            }

            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                mut map: A,
            ) -> Result<SceneFile, A::Error> {
                let mut scene: Option<SceneMetadata> = None;
                let mut file = SceneFile::new("");
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "scene" => scene = Some(map.next_value()?),
                        "camera" => file.camera = map.next_value()?,
                        "environment" => file.environment = map.next_value()?,
                        "post_process" => file.post_process = map.next_value()?,
                        "prefabs" => file.prefabs = map.next_value()?,
                        "entities" => file.entities = map.next_value()?,
                        _ => {
                            file.tables.insert(key, map.next_value()?);
                        }
                    }
                }
                file.scene = scene.ok_or_else(|| serde::de::Error::missing_field("scene"))?;
                Ok(file)
            }
        }

        deserializer.deserialize_map(SceneVisitor)
    }
}

/// A prefab template file (.prefab.toml)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrefabFile {
    pub prefab: PrefabMetadata,
    #[serde(default)]
    pub entities: BTreeMap<String, EntityDef>,
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
    pub overrides: BTreeMap<String, BTreeMap<String, toml::Value>>,
}

/// The macro [`flint_schema::post_effects!`] calls to make
/// [`PostProcessDef`]: one field a built-in effect's key (#741), then the
/// fields that are no effect's, passed through.
macro_rules! post_process_def {
    (
        {
            $(
                $(#[$xmeta:meta])*
                pub $xfield:ident : $xtype:ty
            ),* $(,)?
        }
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
        /// Post-processing settings for the scene: a scene's `[post_process]`.
        ///
        /// The built-in effects' keys (`bloom_intensity`, `fog_color`, ...)
        /// are fields made from their descriptors
        /// ([`flint_schema::post_effects`], #741), each defaulting to its
        /// descriptor's default; the rest (styles, the look, the old style
        /// keys) are this crate's.
        #[derive(Debug, Clone, Serialize, Deserialize)]
        #[serde(default)]
        pub struct PostProcessDef {
            $($(
                #[doc = $help]
                pub $key: flint_schema::post_type!($kind),
            )*)*
            $(
                $(#[$xmeta])*
                pub $xfield: $xtype,
            )*
        }

        impl Default for PostProcessDef {
            fn default() -> Self {
                Self {
                    $($( $key: $default, )*)*
                    $( $xfield: Default::default(), )*
                }
            }
        }

        impl PostProcessDef {
            /// The built-in effect key `key`'s field, to read or set by
            /// name. `None` for a key that is no effect's.
            pub fn param_mut(&mut self, key: &str) -> Option<flint_schema::post_effects::PostParamMut<'_>> {
                match key {
                    $($( stringify!($key) => Some(flint_schema::post_param_mut!($kind, &mut self.$key)), )*)*
                    _ => None,
                }
            }

            /// The built-in effect key `key`'s value. `None` for a key that
            /// is no effect's.
            pub fn param(&self, key: &str) -> Option<flint_schema::post_effects::PostValue> {
                match key {
                    $($( stringify!($key) => Some(flint_schema::post_value!($kind, self.$key)), )*)*
                    _ => None,
                }
            }
        }
    };
}

flint_schema::post_effects!(post_process_def {
    /// The old spellings of `[post_process.styles.kuwahara]` (#298), kept
    /// so scenes written before the style table still load. Read them
    /// through [`style_tables`](Self::style_tables), which folds them in;
    /// a key the table also sets is the table's.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kuwahara_enabled: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kuwahara_radius: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kuwahara_sharpness: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kuwahara_hardness: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kuwahara_anisotropy: Option<f32>,
    /// Stylize passes by name (#262): `[post_process.styles.invert]` with
    /// `enabled` (default true) and the style's numeric parameters. The
    /// renderer checks names and parameters against its registry.
    #[serde(skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub styles: std::collections::BTreeMap<String, toml::Table>,
    /// The old spellings of the shading boil's and the contour's settings
    /// (#214), which are `[post_process.styles.kuwahara] boil` and
    /// `[post_process.styles.contour]` since #599. Kept so older scenes
    /// still load, and read through [`style_tables`](Self::style_tables)
    /// like the `kuwahara_*` keys above.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kuwahara_boil: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub contour_enabled: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub contour_width: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub contour_threshold: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub contour_strength: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub contour_luma: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub contour_color: Option<[f32; 3]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub contour_fade: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub contour_boil: Option<f32>,
    /// A look (#356): a `styles/<name>.look.toml` beside the scene (or its
    /// parent's `styles/`), or a built-in (`ascii_noir`, `graphite`, ...),
    /// laid under this block's own keys. `flint_render::look::resolve`
    /// applies it; until then the struct holds the scene's keys alone.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub look: Option<String>,
    /// Looks the scene will cut or fade to later (#1159), named as
    /// `look` is: a host builds their styles at the scene's entry, on
    /// worker threads, so the frame a script's `set_look` shows one builds
    /// nothing. The look itself isn't shown.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub preload_looks: Vec<String>,
    /// Styles the scene will switch on later by name (#1159): a script's
    /// `set_post_style`, a render mode's reality styles (`matrix`,
    /// `tron`). Built at the scene's entry with every draw the stage gives
    /// a style (blend, reveal, tear), so neither switching one on nor
    /// tweening its reveal or tearing it builds on a frame.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub preload_styles: Vec<String>,
    /// The keys the scene's `[post_process]` wrote, as written (the loader
    /// records them), so a look goes under exactly those and a later
    /// `set_look` can re-lay a different one. `None` for a block built in
    /// code, which then counts every field as its own.
    #[serde(skip)]
    pub authored: Option<toml::Table>,
});

fn default_true() -> bool {
    true
}

/// The `[post_process]` keys that predate the style table, and the
/// `[post_process.styles.<style>]` key each one is now (#298, #599).
pub const LEGACY_STYLE_KEYS: &[(&str, &str, &str)] = &[
    ("kuwahara_enabled", "kuwahara", "enabled"),
    ("kuwahara_radius", "kuwahara", "radius"),
    ("kuwahara_sharpness", "kuwahara", "sharpness"),
    ("kuwahara_hardness", "kuwahara", "hardness"),
    ("kuwahara_anisotropy", "kuwahara", "anisotropy"),
    ("kuwahara_boil", "kuwahara", "boil"),
    ("contour_enabled", "contour", "enabled"),
    ("contour_width", "contour", "width"),
    ("contour_threshold", "contour", "threshold"),
    ("contour_strength", "contour", "strength"),
    ("contour_luma", "contour", "luma"),
    ("contour_color", "contour", "color"),
    ("contour_fade", "contour", "fade"),
    ("contour_boil", "contour", "boil"),
];

/// Move a `[post_process]` table's old style keys (`kuwahara_radius = 4`)
/// into its style tables (`[styles.kuwahara] radius = 4`), so the two
/// spellings mean the same thing. A key the style table already sets is
/// the table's, and the old key is dropped. A style table made here is on
/// only if the old keys say so (`kuwahara_enabled = true`): before the
/// table, `kuwahara_radius` alone left the filter off (and `contour_width`
/// alone the ink).
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

impl PostProcessDef {
    /// Every key a `[post_process]` block can name, sorted: the struct's
    /// own fields, read back from a serialized one (so a new field is
    /// known the moment it exists), with the ones skipped when empty
    /// (`kuwahara_*`, `contour_*`, `styles`) filled in so they serialize too.
    pub fn field_names() -> Vec<String> {
        let mut def: PostProcessDef = toml::Value::Table(toml::Table::new())
            .try_into()
            .expect("an empty [post_process] deserializes");
        def.kuwahara_enabled = Some(false);
        def.kuwahara_radius = Some(0);
        def.kuwahara_sharpness = Some(0.0);
        def.kuwahara_hardness = Some(0.0);
        def.kuwahara_anisotropy = Some(0.0);
        def.kuwahara_boil = Some(0.0);
        def.contour_enabled = Some(false);
        def.contour_width = Some(0.0);
        def.contour_threshold = Some(0.0);
        def.contour_strength = Some(0.0);
        def.contour_luma = Some(0.0);
        def.contour_color = Some([0.0; 3]);
        def.contour_fade = Some(0.0);
        def.contour_boil = Some(0.0);
        def.styles.insert("_".into(), toml::Table::new());
        def.look = Some(String::new());
        def.preload_looks = vec![String::new()];
        def.preload_styles = vec![String::new()];
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

    /// Style `style`'s shading boil (`kuwahara` or `contour`, #214) as the
    /// scene gives it: its table's `boil`, or the old flat key. `None` when
    /// neither says.
    pub fn boil(&self, style: &str) -> Option<f32> {
        let table = self.style_tables();
        let value = table.get(style)?.get("boil")?;
        flint_core::toml_util::toml_f64(value).map(|v| v as f32)
    }

    /// Set style `style`'s shading boil (a wooze preset's `[boil]`, which
    /// wins over the scene's): in its table when the scene has one, so
    /// the table's value can't outrank it, else in the old flat key, so no
    /// table is made and no style switched on.
    pub fn set_boil(&mut self, style: &str, amount: f32) {
        if let Some(table) = self.styles.get_mut(style) {
            table.insert("boil".into(), toml::Value::Float(amount as f64));
            return;
        }
        match style {
            "kuwahara" => self.kuwahara_boil = Some(amount),
            "contour" => self.contour_boil = Some(amount),
            _ => {}
        }
    }

    /// The style tables, with the old `kuwahara_*` and `contour_*` keys
    /// folded in ([`fold_legacy_style_keys`]). What the renderer reads.
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
            ("kuwahara_boil", float(self.kuwahara_boil)),
            (
                "contour_enabled",
                self.contour_enabled.map(toml::Value::Boolean),
            ),
            ("contour_width", float(self.contour_width)),
            ("contour_threshold", float(self.contour_threshold)),
            ("contour_strength", float(self.contour_strength)),
            ("contour_luma", float(self.contour_luma)),
            (
                "contour_color",
                self.contour_color.map(|rgb| {
                    toml::Value::Array(rgb.iter().map(|&c| toml::Value::Float(c as f64)).collect())
                }),
            ),
            ("contour_fade", float(self.contour_fade)),
            ("contour_boil", float(self.contour_boil)),
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
    /// False: the scene casts no shadows, whatever the quality tier says
    /// (#627). Absent = true.
    #[serde(default)]
    pub shadows: Option<bool>,
    /// False: a scene with no light entities is lit by its ambient (and
    /// its emission) alone, not the renderer's built-in key and fill
    /// (#627). Absent = true.
    #[serde(default)]
    pub default_lights: Option<bool>,
}

impl EnvironmentDef {
    /// `shadows`, true when absent (#627).
    pub fn casts_shadows(&self) -> bool {
        self.shadows.unwrap_or(true)
    }

    /// `default_lights`, true when absent (#627).
    pub fn uses_default_lights(&self) -> bool {
        self.default_lights.unwrap_or(true)
    }
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
    /// Component data - all other fields are treated as components. By
    /// name, so a load merges them in the same order every time
    #[serde(flatten)]
    pub components: BTreeMap<String, toml::Value>,
}

impl EntityDef {
    pub fn new() -> Self {
        Self {
            archetype: None,
            parent: None,
            components: BTreeMap::new(),
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
    /// The scene-level tables that are fields of this struct, by the name
    /// their header is written with. Any other top-level table is in
    /// [`tables`](Self::tables).
    pub const TYPED_TABLES: &'static [&'static str] = &["camera", "environment", "post_process"];

    /// A scene from its text, with what its typed tables wrote kept in
    /// [`written`](Self::written) (and in
    /// [`PostProcessDef::authored`]). What the loaders and
    /// [`diagnose`](crate::diagnose) parse with.
    pub fn parse(text: &str) -> Result<Self, toml::de::Error> {
        let mut scene: SceneFile = toml::from_str(text)?;
        if let Ok(doc) = toml::from_str::<toml::Table>(text) {
            scene.record_written(doc);
        }
        Ok(scene)
    }

    /// A scene [`parse`](Self::parse) refuses because a typed table
    /// ([`TYPED_TABLES`](Self::TYPED_TABLES)) doesn't fit its type (#1061):
    /// `[camera] fov = "wide"`. Each such table is left unset, and the rest
    /// of the scene parsed as usual, with every typed table as written in
    /// [`written`](Self::written), so [`diagnose`](crate::diagnose) can
    /// hold it to its `[table.<name>]` schema and name the field. Beside
    /// it, each refused table's name and what serde said of it.
    ///
    /// `None` when the text is not TOML, or when every typed table fits and
    /// the refusal is elsewhere (`[scene]`, an entity): there is nothing
    /// more to say than `parse`'s own error. Nothing loads a scene this way:
    /// what a check reads.
    pub fn parse_refusing(text: &str) -> Option<(Self, Vec<(&'static str, String)>)> {
        let doc: toml::Table = toml::from_str(text).ok()?;
        let mut rest = doc.clone();
        let mut refused = Vec::new();
        for name in Self::TYPED_TABLES {
            let Some(value) = doc.get(*name) else {
                continue;
            };
            let fits = match *name {
                "camera" => value.clone().try_into::<CameraDef>().err(),
                "environment" => value.clone().try_into::<EnvironmentDef>().err(),
                _ => value.clone().try_into::<PostProcessDef>().err(),
            };
            if let Some(e) = fits {
                rest.remove(*name);
                refused.push((*name, e.message().trim_end().replace('\n', " ")));
            }
        }
        if refused.is_empty() {
            return None;
        }
        let mut scene: SceneFile = toml::Value::Table(rest).try_into().ok()?;
        scene.record_written(doc);
        Some((scene, refused))
    }

    /// Keep what the typed tables of `doc`, the whole file, wrote
    /// ([`written`](Self::written), [`PostProcessDef::authored`]).
    fn record_written(&mut self, mut doc: toml::Table) {
        for name in Self::TYPED_TABLES {
            if let Some(value) = doc.remove(*name) {
                self.written.insert((*name).to_string(), value);
            }
        }
        if let Some(def) = &mut self.post_process {
            def.authored = match self.written.get("post_process") {
                Some(toml::Value::Table(post)) => Some(post.clone()),
                _ => None,
            };
        }
    }

    /// The scene-level table `name` as the file wrote it: one of
    /// [`tables`](Self::tables), or a typed one from
    /// [`written`](Self::written).
    pub fn table(&self, name: &str) -> Option<&toml::Value> {
        self.tables.get(name).or_else(|| self.written.get(name))
    }

    /// Every scene-level table as the file wrote it, by name: the typed
    /// ones ([`written`](Self::written)) and the rest
    /// ([`tables`](Self::tables)). What the player hands its extensions.
    pub fn scene_tables(&self) -> toml::Table {
        let mut all = self.written.clone();
        for (name, value) in &self.tables {
            all.insert(name.clone(), value.clone());
        }
        all
    }

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
            tables: toml::Table::new(),
            prefabs: BTreeMap::new(),
            entities: BTreeMap::new(),
            written: toml::Table::new(),
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

    const WEATHER: &str = r#"
[scene]
name = "Storm"

[camera]
fov = 40.0
fovv = 1

[weather]
wind = [1.0, 0.0, 0.0]
rain = 0.7

[[weather.front]]
at = 12.5

[entities.rock]
"#;

    /// #686: a top-level table that is no field is kept, as written, and
    /// written back.
    #[test]
    fn a_table_that_is_no_field_is_kept_and_written_back() {
        let scene: SceneFile = toml::from_str(WEATHER).unwrap();
        assert_eq!(
            scene.tables.keys().collect::<Vec<_>>(),
            ["weather"],
            "the typed tables, [scene] and [entities] are not in it"
        );
        let weather = &scene.tables["weather"];
        assert_eq!(weather["rain"].as_float(), Some(0.7));
        assert_eq!(weather["front"][0]["at"].as_float(), Some(12.5));
        assert_eq!(scene.camera.as_ref().unwrap().fov, Some(40.0));
        assert!(scene.written.is_empty(), "only `parse` records it");

        let saved = toml::to_string_pretty(&scene).unwrap();
        let reread: SceneFile = toml::from_str(&saved).unwrap();
        assert_eq!(reread.tables, scene.tables, "{saved}");
        assert!(reread.entities.contains_key("rock"));
    }

    /// `parse` also keeps what the typed tables wrote, a key serde dropped
    /// included, and hands every table out by name.
    #[test]
    fn parse_keeps_what_the_typed_tables_wrote() {
        let scene = SceneFile::parse(WEATHER).unwrap();
        assert_eq!(scene.written.keys().collect::<Vec<_>>(), ["camera"]);
        assert_eq!(scene.written["camera"]["fovv"].as_integer(), Some(1));
        assert_eq!(scene.table("camera").unwrap()["fov"].as_float(), Some(40.0));
        assert!(scene.table("weather").is_some());
        assert!(scene.table("environment").is_none());
        let all = scene.scene_tables();
        assert_eq!(all.keys().collect::<Vec<_>>(), ["camera", "weather"]);

        let post = SceneFile::parse("[scene]\nname = \"p\"\n[post_process]\nexposure = 2.0\n")
            .unwrap()
            .post_process
            .unwrap();
        assert_eq!(
            post.authored.unwrap().keys().collect::<Vec<_>>(),
            ["exposure"]
        );
    }

    /// A mistake in a typed table still names its line (the reason the
    /// tables are not a `#[serde(flatten)]` field on the way in), and a file
    /// with no `[scene]` says so.
    #[test]
    fn a_parse_error_keeps_its_line() {
        let err =
            toml::from_str::<SceneFile>("[scene]\nname = \"s\"\n\n[camera]\nfov = \"wide\"\n")
                .unwrap_err()
                .to_string();
        assert!(err.contains("line 5"), "{err}");
        let err = toml::from_str::<SceneFile>("[weather]\nrain = 1\n")
            .unwrap_err()
            .to_string();
        assert!(err.contains("missing field `scene`"), "{err}");
    }

    /// #1061: a typed table that doesn't fit its type is set aside, each of
    /// them, and the rest of the scene parsed, with the table as written
    /// kept for the schema pass. A refusal elsewhere is `parse`'s alone.
    #[test]
    fn parse_refusing_sets_aside_each_table_that_does_not_fit() {
        let text = "[scene]\nname = \"s\"\n\n[camera]\nfov = \"wide\"\n\n\
                    [environment]\ndiffuse_wrap = \"soft\"\n\n\
                    [post_process]\nexposure = 2.0\n\n[entities.rock]\n";
        assert!(SceneFile::parse(text).is_err());
        let (scene, refused) = SceneFile::parse_refusing(text).expect("the rest parses");
        let names: Vec<&str> = refused.iter().map(|(name, _)| *name).collect();
        assert_eq!(names, ["camera", "environment"]);
        assert!(refused[0].1.contains("expected f32"), "{refused:?}");
        assert!(scene.camera.is_none() && scene.environment.is_none());
        let post = scene.post_process.as_ref().expect("the table that fits");
        assert_eq!(post.exposure, 2.0);
        assert!(post.authored.is_some());
        assert_eq!(scene.written["camera"]["fov"].as_str(), Some("wide"));
        assert!(scene.entities.contains_key("rock"));

        // Not TOML, a refusal outside the typed tables, a scene that parses
        assert!(SceneFile::parse_refusing("[scene\n").is_none());
        assert!(SceneFile::parse_refusing("[camera]\nfov = 1.0\n").is_none());
        assert!(SceneFile::parse_refusing("[scene]\nname = \"s\"\n").is_none());
    }

    /// ADR 0115 (#1061): `scene.version` is informational. The same scene
    /// under any version, or none, reads the same: its tables, entities and
    /// the one migration, `fold_legacy_style_keys`, which keys on what the
    /// scene writes and never on the number. A migration list ordered by
    /// version is for the day a change can't be told from the data; when
    /// it comes, this test changes with it.
    #[test]
    fn the_version_changes_nothing_a_scene_reads_as() {
        const BODY: &str = "\n[post_process]\nkuwahara_enabled = true\nkuwahara_radius = 4\n\n\
                            [camera]\nfov = 50\n\n[weather]\nrain = 0.5\n\n\
                            [entities.rock.transform]\nposition = [1, 2, 3]\n";
        let read = |version: Option<&str>| {
            let head = match version {
                Some(v) => format!("[scene]\nname = \"v\"\nversion = \"{v}\"\n"),
                None => "[scene]\nname = \"v\"\n".to_string(),
            };
            let scene = SceneFile::parse(&(head + BODY)).unwrap();
            let styles = scene.post_process.as_ref().unwrap().style_tables();
            assert_eq!(
                styles["kuwahara"]["radius"].as_integer(),
                Some(4),
                "folded under {version:?}"
            );
            let mut unversioned = scene.clone();
            unversioned.scene.version = String::new();
            (
                toml::to_string(&unversioned).unwrap(),
                scene.scene_tables(),
                format!("{styles:?}"),
            )
        };
        let baseline = read(None);
        for version in ["1.0", "0.1", "2.0", "anything"] {
            assert_eq!(read(Some(version)), baseline, "version {version}");
        }
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

    /// The contour's and the paint boil's flat keys (#214) are its table's
    /// and Kuwahara's since #599, colour and all.
    #[test]
    fn old_contour_and_boil_keys_fold_into_their_tables() {
        let old = def(
            "kuwahara_boil = 0.5\ncontour_enabled = true\ncontour_width = 1.5\n\
             contour_color = [0.1, 0.2, 0.3]\ncontour_boil = 1.0\nboil_on = 2",
        );
        let tables = old.style_tables();
        let c = &tables["contour"];
        assert_eq!(c["enabled"].as_bool(), Some(true));
        assert_eq!(c["width"].as_float(), Some(1.5));
        assert_eq!(c["boil"].as_float(), Some(1.0));
        let rgb: Vec<f64> = c["color"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_float().unwrap())
            .collect();
        assert_eq!(rgb.len(), 3);
        assert!((rgb[1] - 0.2).abs() < 1e-6);
        // Boil alone never switched the filter on, and doesn't now
        let k = &tables["kuwahara"];
        assert_eq!(k["boil"].as_float(), Some(0.5));
        assert_eq!(k["enabled"].as_bool(), Some(false));
        assert_eq!(old.boil_on, 2);
        // The ink stays off without its old switch
        let width_only = def("contour_width = 2.0").style_tables();
        assert_eq!(width_only["contour"]["enabled"].as_bool(), Some(false));
        assert!(PostProcessDef::unknown_field_warnings(
            "[post_process]\ncontour_fade = 26.0\nkuwahara_boil = 0.5\n",
            None
        )
        .is_empty());
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
