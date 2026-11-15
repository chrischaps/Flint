//! The scene tables living props own: `[living]` and `[wooze]` (#686).
//!
//! flint-scene keeps a scene's top-level tables it has no field for as raw
//! TOML (`SceneFile::tables`), and the crate that owns a feature gives its
//! tables their types. These are living props' and the wooze field's: the
//! definitions a scene, a `<name>.wooze.toml` preset and the Living Props
//! panel share ([`LivingDef`], [`WoozeDef`], [`WoozePreset`] and their
//! parts), reading them out of a scene ([`LivingTables`]), filling in the
//! preset a scene names ([`apply_preset_file`], [`settle_scene`]), and
//! writing a tuned look back into a scene file ([`commit_look`]).
//!
//! They lived in flint-scene until #686, as fields of `SceneFile`; the move
//! changed where they are, not what they do. Their schemas are
//! `schemas/scene/living.toml` and `schemas/scene/wooze.toml`.

mod commit;
mod wooze;

pub use commit::{commit_look, edit_species, look_edits, CommitReport};
pub use wooze::{
    apply_preset_file, list_presets, list_presets_in, preset_save_dir, read_preset, read_preset_in,
    write_preset, write_preset_in, WoozeIssue, PRESET_SUFFIX,
};

use flint_scene::SceneFile;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

/// A scene's `[living]` and `[wooze]` tables, read into their types.
///
/// A table that doesn't fit its type (a key it doesn't know, a value of the
/// wrong type) is an error naming the table, as it was when these were
/// fields of `SceneFile` and the whole file failed to parse over it.
#[derive(Debug, Clone, Default)]
pub struct LivingTables {
    /// `[living]`, or `None` when the scene has none.
    pub living: Option<LivingDef>,
    /// `[wooze]`, or `None` when the scene has none: no field, no samples.
    pub wooze: Option<WoozeDef>,
}

impl LivingTables {
    /// The scene's two tables. `Err` is `[living]: <what serde said>`.
    pub fn of(scene: &SceneFile) -> Result<Self, String> {
        Ok(Self {
            living: read_table(scene, "living")?,
            wooze: read_table(scene, "wooze")?,
        })
    }

    /// [`of`](Self::of) for a scene that has been through [`settle_scene`],
    /// which is where a table that doesn't fit stops the load: here one is
    /// warned about and read as absent.
    pub fn or_warn(scene: &SceneFile) -> Self {
        fn read<T: serde::de::DeserializeOwned>(scene: &SceneFile, name: &str) -> Option<T> {
            read_table(scene, name).unwrap_or_else(|problem| {
                tracing::warn!("{problem}; going without it");
                None
            })
        }
        Self {
            living: read(scene, "living"),
            wooze: read(scene, "wooze"),
        }
    }

    /// Write the two back into the scene's tables: a `None` takes its table
    /// out.
    pub fn put(&self, scene: &mut SceneFile) {
        put_table(scene, "living", self.living.as_ref());
        put_table(scene, "wooze", self.wooze.as_ref());
    }
}

fn read_table<T: serde::de::DeserializeOwned>(
    scene: &SceneFile,
    name: &str,
) -> Result<Option<T>, String> {
    match scene.tables.get(name) {
        None => Ok(None),
        Some(value) => value
            .clone()
            .try_into()
            .map(Some)
            .map_err(|e: toml::de::Error| format!("[{name}]: {}", e.message())),
    }
}

fn put_table<T: Serialize>(scene: &mut SceneFile, name: &str, def: Option<&T>) {
    match def.and_then(|def| toml::Value::try_from(def).ok()) {
        Some(value) => {
            scene.tables.insert(name.to_string(), value);
        }
        None => {
            scene.tables.remove(name);
        }
    }
}

/// A scene's `[wooze]`, for a test: the table must fit.
#[cfg(test)]
pub(crate) fn wooze_of(scene: &SceneFile) -> Option<WoozeDef> {
    LivingTables::of(scene).unwrap().wooze
}

/// A scene's `[living]`, for a test: the table must fit.
#[cfg(test)]
pub(crate) fn living_of(scene: &SceneFile) -> Option<LivingDef> {
    LivingTables::of(scene).unwrap().living
}

/// What every host does to a scene it has loaded before reading its living
/// tables: check that `[living]` and `[wooze]` fit their types, then fill in
/// the preset `[wooze] preset` names ([`apply_preset_file`]), so everything
/// downstream only sees knobs. `flint_scene::load_scene` did this itself
/// while the tables were its own.
///
/// A table that doesn't fit is an `Err` naming the file and the table: the
/// scene is refused, as it was. What's wrong with a preset file is warned
/// about and the scene goes on, since a look lab shouldn't refuse to start
/// over one; `flint validate` fails on it.
pub fn settle_scene(scene: &mut SceneFile, scene_path: &Path) -> flint_core::Result<()> {
    LivingTables::of(scene).map_err(|problem| {
        flint_core::FlintError::TomlParseError(format!(
            "{}: {problem}",
            flint_core::project::shown(scene_path)
        ))
    })?;
    for issue in apply_preset_file(scene, scene_path) {
        tracing::warn!("{issue}");
    }
    Ok(())
}

/// A scene's `[wooze]` table (#213): how woozy it is everywhere, and the zones
/// that are more or less so. A scene without one has no field and pays nothing.
/// [`WoozeDef::field`] makes it into one, and [`WoozeDef::check`] is what
/// `flint validate` runs.
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
    /// How soft every prop's silhouette is, 0 to 1 (#582), over each spec's:
    /// the body fades where it turns away from the eye. 0 is a hard edge.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub softness: Option<f32>,
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
    /// How soft its silhouette is, 0 to 1 (#582).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub softness: Option<f32>,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tables_are_read_out_of_a_scene_and_put_back() {
        let mut scene: SceneFile = toml::from_str(
            r#"
[scene]
name = "grove"

[living]
shadows = "drift"

[wooze]
glide = 0.4

[[wooze.zone]]
name = "pocket"
shape = "sphere"
center = [0, 0, 12]
radius = 6
"#,
        )
        .unwrap();
        let tables = LivingTables::of(&scene).unwrap();
        let living = tables.living.clone().expect("[living] should be present");
        assert_eq!(living.shadows.as_deref(), Some("drift"));
        let wooze = tables.wooze.clone().expect("[wooze] should be present");
        assert_eq!(wooze.glide, Some(0.4));
        assert_eq!(wooze.zone.len(), 1);

        // Put back, they read the same (a number is written as the f32 it
        // was read into); a None takes its table out
        tables.put(&mut scene);
        let again = LivingTables::of(&scene).unwrap();
        assert_eq!(again.living, tables.living);
        let again = again.wooze.unwrap();
        assert_eq!((again.glide, &again.zone), (wooze.glide, &wooze.zone));
        LivingTables {
            living: None,
            wooze: tables.wooze,
        }
        .put(&mut scene);
        assert!(!scene.tables.contains_key("living"));
        assert!(LivingTables::of(&scene).unwrap().wooze.is_some());

        let none = LivingTables::of(&SceneFile::new("bare")).unwrap();
        assert!(none.living.is_none() && none.wooze.is_none());
    }

    /// A key a table doesn't know refuses the scene, naming the table and
    /// the file: `glide` is `[wooze]`'s, and shouldn't pass for nothing.
    #[test]
    fn a_table_that_does_not_fit_is_refused_by_name() {
        let mut scene: SceneFile =
            toml::from_str("[scene]\nname = \"grove\"\n\n[living]\nglide = 0.3\n").unwrap();
        let problem = LivingTables::of(&scene).unwrap_err();
        assert!(
            problem.starts_with("[living]: unknown field `glide`"),
            "{problem}"
        );
        let err = settle_scene(&mut scene, Path::new("levels/grove.scene.toml")).unwrap_err();
        assert!(
            err.to_string()
                .contains("levels/grove.scene.toml: [living]: unknown field `glide`"),
            "{err}"
        );
        // ...and after a settle that passed, a reader goes without it
        let tables = LivingTables::or_warn(&scene);
        assert!(tables.living.is_none());
    }
}
