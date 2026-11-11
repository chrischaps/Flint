//! A scene's `[wooze]` table (#213) as the field every consumer samples.
//!
//! Zones are kept as raw TOML in [`WoozeDef`], so that a malformed one can be
//! told apart and named: read through serde, a bad zone would fail the whole
//! scene with a line number. Loading a scene never fails over a zone. The player
//! warns and goes without it, and `flint validate` fails on it, through
//! [`WoozeDef::check`].

use crate::format::{
    LivingDef, LivingEchoesDef, LivingSpeciesDef, LivingVarianceDef, SceneFile, WoozeDef,
    WoozePreset,
};
use flint_core::project::{AssetKind, Project};
use flint_core::vfs;
use flint_core::wooze::{check_knob_named, preset, WoozeField, WoozeSample};
use flint_schema::{validate_component_data, SchemaRegistry};
use std::fmt;
use std::path::{Path, PathBuf};

/// What a preset file's name ends in: `<name>.wooze.toml`.
pub const PRESET_SUFFIX: &str = ".wooze.toml";

/// The directory a scene file is in (`.` for a bare file name).
fn scene_dir_of(scene_path: &Path) -> &Path {
    scene_path
        .parent()
        .filter(|dir| !dir.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
}

/// The preset file named `name` that a scene can see, read. `None` if there's no
/// such file, and it may yet be a built-in. A file comes before a built-in of its
/// name, as a game's schemas come before the engine's.
///
/// The file is found by the project's one asset search (`AssetKind::WoozePresets`,
/// ADR 0114): `wooze/<name>.wooze.toml` beside the scene, in each directory above
/// it up to the project root, then in the engine. The project is the one the scene
/// file is found in; a host that holds it calls [`read_preset_in`].
pub fn read_preset(scene_path: &Path, name: &str) -> Option<Result<WoozePreset, String>> {
    read_preset_in(&Project::for_scene(scene_path), scene_path, name)
}

/// [`read_preset`] in a project the host already holds.
pub fn read_preset_in(
    project: &Project,
    scene_path: &Path,
    name: &str,
) -> Option<Result<WoozePreset, String>> {
    let path = project
        .resolve(AssetKind::WoozePresets, name, scene_dir_of(scene_path))
        .ok()?;
    let parsed = vfs::read_to_string(&path)
        .map_err(|e| e.to_string())
        .and_then(|text| WoozePreset::parse(&text));
    Some(parsed.map_err(|e| format!("{}: {e}", path.display())))
}

/// The preset files a scene can see, by name, in order.
pub fn list_presets(scene_path: &Path) -> Vec<String> {
    list_presets_in(&Project::for_scene(scene_path), scene_path)
}

/// [`list_presets`] in a project the host already holds.
pub fn list_presets_in(project: &Project, scene_path: &Path) -> Vec<String> {
    let mut names: Vec<String> = project
        .list(AssetKind::WoozePresets, scene_dir_of(scene_path))
        .into_iter()
        .map(|listed| listed.name)
        .collect();
    names.sort();
    names
}

/// Where [`write_preset`] puts a scene's presets: the nearest `wooze/` that
/// exists, from beside the scene up to the project root (never the mounted
/// engine's, which a game doesn't write into), or `wooze/` beside the scene when
/// there is none yet. A file saved there is the one the scene's search finds first
/// among those that exist.
pub fn preset_save_dir(project: &Project, scene_path: &Path) -> PathBuf {
    let scene_dir = scene_dir_of(scene_path);
    let engine = project.engine_mount();
    let dirs: Vec<PathBuf> = project
        .search_dirs(AssetKind::WoozePresets, scene_dir)
        .into_iter()
        .filter(|dir| !engine.as_ref().is_some_and(|e| dir.starts_with(e)))
        .collect();
    dirs.iter()
        .find(|dir| vfs::is_dir(dir))
        .or(dirs.first())
        .cloned()
        .unwrap_or_else(|| scene_dir.join("wooze"))
}

/// Save a preset where a scene can name it: in [`preset_save_dir`]. Returns the
/// file written. Not for the web, which has no folders to put it in.
pub fn write_preset(
    scene_path: &Path,
    name: &str,
    preset: &WoozePreset,
) -> Result<PathBuf, String> {
    write_preset_in(&Project::for_scene(scene_path), scene_path, name, preset)
}

/// [`write_preset`] in a project the host already holds.
pub fn write_preset_in(
    project: &Project,
    scene_path: &Path,
    name: &str,
    preset: &WoozePreset,
) -> Result<PathBuf, String> {
    let fine = |c: char| c.is_ascii_alphanumeric() || c == '_' || c == '-';
    if name.is_empty() || !name.chars().all(fine) {
        return Err(format!(
            "'{name}' can't name a preset: use letters, digits, '_' and '-'"
        ));
    }
    let dir = &preset_save_dir(project, scene_path);
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let path = dir.join(format!("{name}{PRESET_SUFFIX}"));
    std::fs::write(&path, preset.to_toml(name)).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(path)
}

/// If a scene's `[wooze] preset` names a file, lay it under the scene's own
/// tables: whatever the scene says itself stands, as it does over a built-in.
/// The exception is `[boil]`, since a scene's `[post_process]` has no way to say
/// it hasn't set a value, and nothing to fill in with no such table: the passes
/// that boil are off. What's wrong with the file comes back to be named.
pub fn apply_preset_file(scene: &mut SceneFile, scene_path: &Path) -> Vec<WoozeIssue> {
    let Some(def) = &mut scene.wooze else {
        return Vec::new();
    };
    let Some(name) = def.preset.clone() else {
        return Vec::new();
    };
    let found = match read_preset(scene_path, &name) {
        None => return Vec::new(),
        Some(found) => found,
    };
    // It's been dealt with, whichever way this goes, so nothing goes looking for
    // a built-in of that name.
    def.preset = None;
    let issue = |message: String| WoozeIssue {
        zone: None,
        message: format!("preset '{name}': {message}"),
    };
    let preset = match found {
        Ok(preset) => preset,
        Err(message) => return vec![issue(message)],
    };
    let issues: Vec<WoozeIssue> = preset.problems().into_iter().map(issue).collect();

    def.amplitude = def.amplitude.or(preset.wooze.amplitude);
    def.step = def.step.or(preset.wooze.step);
    def.glide = def.glide.or(preset.wooze.glide);
    def.jitter = def.jitter.or(preset.wooze.jitter);

    let living = scene.living.get_or_insert_with(Default::default);
    *living = living.clone().over(preset.living);

    if let (Some(boil), Some(post)) = (preset.boil, &mut scene.post_process) {
        let on = boil.enabled.unwrap_or(true);
        if let Some(paint) = boil.kuwahara {
            post.set_boil("kuwahara", if on { paint } else { 0.0 });
        }
        if let Some(ink) = boil.contour {
            post.set_boil("contour", if on { ink } else { 0.0 });
        }
        if let Some(cadence) = boil.on {
            post.boil_on = cadence;
        }
    }
    issues
}

fn echoes_over(a: Option<LivingEchoesDef>, b: Option<LivingEchoesDef>) -> Option<LivingEchoesDef> {
    match (a, b) {
        (Some(a), Some(b)) => Some(LivingEchoesDef {
            count: a.count.or(b.count),
            spread: a.spread.or(b.spread),
            alpha: a.alpha.or(b.alpha),
        }),
        (a, b) => a.or(b),
    }
}

fn variance_over(
    a: Option<LivingVarianceDef>,
    b: Option<LivingVarianceDef>,
) -> Option<LivingVarianceDef> {
    match (a, b) {
        (Some(a), Some(b)) => Some(LivingVarianceDef {
            amount: a.amount.or(b.amount),
            steady: a.steady.or(b.steady),
            keep_size: a.keep_size.or(b.keep_size),
        }),
        (a, b) => a.or(b),
    }
}

impl LivingDef {
    /// This table over `under`, a field at a time, species by species.
    pub fn over(self, under: LivingDef) -> LivingDef {
        let mut species = under.species;
        for (name, mine) in self.species {
            let merged = match species.remove(&name) {
                Some(theirs) => mine.over(theirs),
                None => mine,
            };
            species.insert(name, merged);
        }
        LivingDef {
            shadows: self.shadows.or(under.shadows),
            stagger: self.stagger.or(under.stagger),
            echoes: echoes_over(self.echoes, under.echoes),
            variance: variance_over(self.variance, under.variance),
            softness: self.softness.or(under.softness),
            species,
        }
    }
}

impl LivingSpeciesDef {
    /// This species' values over `under`'s, a field at a time.
    pub fn over(self, under: LivingSpeciesDef) -> LivingSpeciesDef {
        LivingSpeciesDef {
            amplitude: self.amplitude.or(under.amplitude),
            step: self.step.or(under.step),
            glide: self.glide.or(under.glide),
            jitter: self.jitter.or(under.jitter),
            stagger: self.stagger.or(under.stagger),
            echoes: echoes_over(self.echoes, under.echoes),
            variance: variance_over(self.variance, under.variance),
            softness: self.softness.or(under.softness),
        }
    }

    /// Whether it says nothing at all.
    pub fn is_empty(&self) -> bool {
        *self == LivingSpeciesDef::default()
    }

    /// What's wrong with its values that the types can't say, each named by its
    /// key. The player makes do; `flint validate` fails.
    pub fn problems(&self) -> Vec<String> {
        let mut problems = Vec::new();
        for (key, value) in [
            ("amplitude", self.amplitude),
            ("step", self.step),
            ("glide", self.glide),
            ("jitter", self.jitter),
        ] {
            if let Some(value) = value {
                if let Err(problem) = check_knob_named(key, value) {
                    problems.push(problem);
                }
            }
        }
        problems.extend(props_problems(
            self.stagger.as_deref(),
            self.echoes,
            self.variance,
            self.softness,
        ));
        problems
    }
}

/// Stagger, echoes, variance and softness, wherever a table holds them.
fn props_problems(
    stagger: Option<&str>,
    echoes: Option<LivingEchoesDef>,
    variance: Option<LivingVarianceDef>,
    softness: Option<f32>,
) -> Vec<String> {
    let mut problems = Vec::new();
    if let Some(name) = stagger {
        if !["none", "per_level", "per_branch"].contains(&name) {
            problems.push(format!(
                "stagger = \"{name}\" isn't \"none\", \"per_level\" or \"per_branch\""
            ));
        }
    }
    let mut unit = |key: &str, value: Option<f32>| {
        if let Some(value) = value {
            if !(0.0..=1.0).contains(&value) {
                problems.push(format!("{key} = {value} must be in [0.0, 1.0]"));
            }
        }
    };
    if let Some(echoes) = echoes {
        unit("echoes.alpha", echoes.alpha);
    }
    if let Some(variance) = variance {
        unit("variance.amount", variance.amount);
        unit("variance.steady", variance.steady);
    }
    unit("softness", softness);
    problems
}

impl WoozePreset {
    /// A preset file's text. A wrong key is an error, and says which.
    pub fn parse(text: &str) -> Result<Self, String> {
        toml::from_str(text).map_err(|e| e.message().to_string())
    }

    /// What's wrong with its values that the types can't say, for the knobs the
    /// field doesn't check itself. The player makes do; `flint validate` fails.
    pub fn problems(&self) -> Vec<String> {
        let mut problems = props_problems(
            self.living.stagger.as_deref(),
            self.living.echoes,
            self.living.variance,
            self.living.softness,
        );
        for (name, species) in &self.living.species {
            for problem in species.problems() {
                problems.push(format!("species '{name}': {problem}"));
            }
        }
        if let Some(on) = self.boil.and_then(|boil| boil.on) {
            if !(1..=3).contains(&on) {
                problems.push(format!("boil.on = {on} must be 1, 2 or 3"));
            }
        }
        problems
    }

    /// The file's text. Each number is written as briefly as reads back to the
    /// same bits, so a value that's saved and loaded looks exactly as it did.
    pub fn to_toml(&self, name: &str) -> String {
        fn line(out: &mut String, key: &str, value: Option<impl fmt::Debug>) {
            if let Some(value) = value {
                out.push_str(&format!("{key} = {value:?}\n"));
            }
        }
        /// An inline table of the fields set, or `None` if none are, so a file
        /// never says `echoes = {  }`.
        fn inline(fields: &[(&str, Option<String>)]) -> Option<String> {
            let set: Vec<String> = fields
                .iter()
                .filter_map(|(key, value)| value.as_ref().map(|value| format!("{key} = {value}")))
                .collect();
            (!set.is_empty()).then(|| format!("{{ {} }}", set.join(", ")))
        }
        let number = |value: Option<f32>| value.map(|value| format!("{value:?}"));

        let mut out = format!(
            "# A saved look for Living Props. In a scene: [wooze] preset = \"{name}\"\n\n[wooze]\n"
        );
        line(&mut out, "amplitude", self.wooze.amplitude);
        line(&mut out, "step", self.wooze.step);
        line(&mut out, "glide", self.wooze.glide);
        line(&mut out, "jitter", self.wooze.jitter);

        out.push_str("\n[living]\n");
        line(&mut out, "shadows", self.living.shadows.as_deref());
        line(&mut out, "stagger", self.living.stagger.as_deref());
        line(&mut out, "softness", self.living.softness);
        if let Some(echoes) = self.living.echoes {
            let table = inline(&[
                ("count", echoes.count.map(|count| count.to_string())),
                ("spread", number(echoes.spread)),
                ("alpha", number(echoes.alpha)),
            ]);
            if let Some(table) = table {
                out.push_str(&format!("echoes = {table}\n"));
            }
        }
        if let Some(variance) = self.living.variance {
            let table = inline(&[
                ("amount", number(variance.amount)),
                ("steady", number(variance.steady)),
                ("keep_size", variance.keep_size.map(|keep| keep.to_string())),
            ]);
            if let Some(table) = table {
                out.push_str(&format!("variance = {table}\n"));
            }
        }
        for (name, species) in &self.living.species {
            out.push_str(&format!("\n[living.species.{name}]\n"));
            if species.is_empty() {
                out.push_str("# No values of its own: loading this gives it the grove's.\n");
            }
            line(&mut out, "amplitude", species.amplitude);
            line(&mut out, "step", species.step);
            line(&mut out, "glide", species.glide);
            line(&mut out, "jitter", species.jitter);
            line(&mut out, "stagger", species.stagger.as_deref());
            line(&mut out, "softness", species.softness);
            if let Some(echoes) = species.echoes {
                let table = inline(&[
                    ("count", echoes.count.map(|count| count.to_string())),
                    ("spread", number(echoes.spread)),
                    ("alpha", number(echoes.alpha)),
                ]);
                if let Some(table) = table {
                    out.push_str(&format!("echoes = {table}\n"));
                }
            }
            if let Some(variance) = species.variance {
                let table = inline(&[
                    ("amount", number(variance.amount)),
                    ("steady", number(variance.steady)),
                    ("keep_size", variance.keep_size.map(|keep| keep.to_string())),
                ]);
                if let Some(table) = table {
                    out.push_str(&format!("variance = {table}\n"));
                }
            }
        }

        if let Some(boil) = self.boil {
            out.push_str("\n[boil]\n");
            line(&mut out, "enabled", boil.enabled);
            line(&mut out, "kuwahara", boil.kuwahara);
            line(&mut out, "contour", boil.contour);
            line(&mut out, "on", boil.on);
        }
        out
    }
}

/// Something wrong with a `[wooze]` table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WoozeIssue {
    /// The zone it's about, by name, or by its place in the list if it has none.
    /// `None` for the table's global values.
    pub zone: Option<String>,
    /// What's wrong with it.
    pub message: String,
}

impl fmt::Display for WoozeIssue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.zone {
            Some(zone) => write!(f, "wooze zone '{zone}': {}", self.message),
            None => write!(f, "[wooze]: {}", self.message),
        }
    }
}

impl WoozeDef {
    /// The field this table describes, made of whatever is good in it, and an
    /// issue for each thing that isn't.
    pub fn field(&self) -> (WoozeField, Vec<WoozeIssue>) {
        let mut issues = Vec::new();
        let global = |message: String| WoozeIssue {
            zone: None,
            message,
        };

        let base = match self.preset.as_deref() {
            None => WoozeSample::NEUTRAL,
            Some(name) => preset(name).unwrap_or_else(|| {
                let names: Vec<&str> = flint_core::wooze::PRESETS.iter().map(|p| p.0).collect();
                issues.push(global(format!(
                    "preset '{name}' isn't a file in wooze/ or one of {}",
                    names.join(", ")
                )));
                WoozeSample::NEUTRAL
            }),
        };
        let mut field = WoozeField::new(base);
        let knobs = [
            ("amplitude", self.amplitude),
            ("step", self.step),
            ("glide", self.glide),
            ("jitter", self.jitter),
        ];
        for (name, value) in knobs {
            let Some(value) = value else { continue };
            // One at a time, so one bad knob leaves the others standing.
            let mut table = toml::Table::new();
            table.insert(name.to_string(), toml::Value::Float(f64::from(value)));
            if let Err(message) = field.set_global(&table, 0.0, 0.0) {
                issues.push(global(message));
            }
        }

        for (i, zone) in self.zone.iter().enumerate() {
            let added = match zone.as_table() {
                Some(table) => field.add_zone(table),
                None => Err(format!("{zone} must be a table")),
            };
            if let Err(message) = added {
                issues.push(WoozeIssue {
                    zone: Some(zone_label(zone, i)),
                    message,
                });
            }
        }
        (field, issues)
    }

    /// Everything wrong with this table: first what the field itself can't use,
    /// which says the most, and then anything that passes that but not the `wooze`
    /// and `wooze_zone` schemas, which a game's own schemas can tighten. One issue
    /// a zone at most, so that one mistake isn't reported twice.
    pub fn check(&self, schemas: &SchemaRegistry) -> Vec<WoozeIssue> {
        let (_, mut issues) = self.field();

        let global_is_fine = issues.iter().all(|issue| issue.zone.is_some());
        if let (true, Some(schema)) = (global_is_fine, schemas.get_component("wooze")) {
            let mut table = toml::Table::new();
            if let Some(name) = &self.preset {
                table.insert("preset".into(), toml::Value::String(name.clone()));
            }
            let knobs = [
                ("amplitude", self.amplitude),
                ("step", self.step),
                ("glide", self.glide),
                ("jitter", self.jitter),
            ];
            for (name, value) in knobs {
                if let Some(value) = value {
                    table.insert(name.into(), toml::Value::Float(f64::from(value)));
                }
            }
            if let Err(e) = validate_component_data(schema, &toml::Value::Table(table)) {
                issues.push(WoozeIssue {
                    zone: None,
                    message: e.to_string(),
                });
            }
        }

        if let Some(schema) = schemas.get_component("wooze_zone") {
            for (i, zone) in self.zone.iter().enumerate() {
                let label = zone_label(zone, i);
                if issues
                    .iter()
                    .any(|issue| issue.zone.as_deref() == Some(&label))
                {
                    continue;
                }
                if let Err(e) = validate_component_data(schema, zone) {
                    issues.push(WoozeIssue {
                        zone: Some(label),
                        message: e.to_string(),
                    });
                }
            }
        }
        issues
    }
}

/// A zone's name, or `zone <n>`, counting from 1, if it hasn't got one.
fn zone_label(zone: &toml::Value, index: usize) -> String {
    match zone.get("name").and_then(|name| name.as_str()) {
        Some(name) if !name.is_empty() => name.to_string(),
        _ => format!("zone {}", index + 1),
    }
}

#[cfg(test)]
mod preset_tests {
    use super::*;
    use crate::format::{BoilDef, WoozeKnobs};

    fn tuned() -> WoozePreset {
        WoozePreset {
            wooze: WoozeKnobs {
                amplitude: Some(0.7),
                // Neither reads back from a rounded decimal.
                step: Some(1.0 / 3.0),
                glide: Some(0.1 + 0.2),
                jitter: Some(2.0),
            },
            living: LivingDef {
                shadows: None,
                stagger: Some("per_branch".into()),
                echoes: Some(LivingEchoesDef {
                    count: Some(3),
                    spread: Some(1.8),
                    alpha: Some(0.15),
                }),
                variance: Some(LivingVarianceDef {
                    amount: Some(1.0),
                    steady: Some(0.0),
                    keep_size: Some(false),
                }),
                softness: Some(0.35),
                species: [(
                    "grove_thorn".to_string(),
                    LivingSpeciesDef {
                        step: Some(0.1 + 0.15),
                        stagger: Some("none".into()),
                        softness: Some(0.7),
                        echoes: Some(LivingEchoesDef {
                            count: Some(1),
                            ..Default::default()
                        }),
                        ..Default::default()
                    },
                )]
                .into(),
            },
            boil: Some(BoilDef {
                enabled: Some(true),
                kuwahara: Some(0.5),
                contour: Some(1.25),
                on: Some(3),
            }),
        }
    }

    /// A folder of its own, whoever else is running, gone when dropped.
    struct Folder(PathBuf);

    impl Folder {
        fn new(name: &str) -> Self {
            let dir =
                std::env::temp_dir().join(format!("flint_preset_{name}_{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }

        fn scene(&self) -> PathBuf {
            self.0.join("grove.scene.toml")
        }
    }

    impl Drop for Folder {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn a_saved_preset_reads_back_to_the_same_bits() {
        let preset = tuned();
        let text = preset.to_toml("x");
        assert_eq!(WoozePreset::parse(&text).unwrap(), preset, "{text}");
        assert_eq!(preset.problems(), Vec::<String>::new());
    }

    #[test]
    fn whole_numbers_stay_floats_and_the_file_says_how_to_use_it() {
        let text = tuned().to_toml("misty");
        assert!(text.contains("jitter = 2.0"), "{text}");
        assert!(text.contains("preset = \"misty\""), "{text}");
        assert!(text.contains("keep_size = false"), "{text}");
    }

    #[test]
    fn a_scene_can_name_what_was_saved_beside_it() {
        let folder = Folder::new("beside");
        let scene_path = folder.scene();
        let path = write_preset(&scene_path, "misty", &tuned()).unwrap();
        assert!(path.ends_with("wooze/misty.wooze.toml"), "{path:?}");
        assert_eq!(list_presets(&scene_path), ["misty"]);
        assert_eq!(read_preset(&scene_path, "misty").unwrap().unwrap(), tuned());
        assert!(read_preset(&scene_path, "linklater").is_none());
    }

    #[test]
    fn a_scene_in_a_levels_folder_finds_the_games_presets() {
        let folder = Folder::new("levels");
        let levels = folder.0.join("levels");
        std::fs::create_dir_all(&levels).unwrap();
        std::fs::create_dir_all(folder.0.join("wooze")).unwrap();
        let scene_path = levels.join("grove.scene.toml");
        write_preset(&scene_path, "misty", &tuned()).unwrap();
        assert!(folder.0.join("wooze/misty.wooze.toml").is_file());
        assert!(!levels.join("wooze").exists());
        assert_eq!(list_presets(&scene_path), ["misty"]);
    }

    #[test]
    fn a_scene_two_folders_deep_finds_and_saves_to_the_projects_presets() {
        // ADR 0114: before the one resolver a preset was looked for beside
        // the scene and one level up, and no further.
        let folder = Folder::new("deep");
        let deep = folder.0.join("scenes/act1/cave");
        std::fs::create_dir_all(&deep).unwrap();
        std::fs::create_dir_all(folder.0.join("wooze")).unwrap();
        std::fs::write(folder.0.join("flint.toml"), "").unwrap();
        let scene_path = deep.join("grove.scene.toml");
        std::fs::write(&scene_path, "").unwrap();

        // Saved into the nearest wooze/ that exists: the project's.
        let saved = write_preset(&scene_path, "misty", &tuned()).unwrap();
        assert_eq!(saved, folder.0.join("wooze").join("misty.wooze.toml"));
        assert!(!deep.join("wooze").exists());
        assert_eq!(list_presets(&scene_path), ["misty"]);
        assert_eq!(read_preset(&scene_path, "misty").unwrap().unwrap(), tuned());

        // A nearer folder, once there is one, is where the next save goes,
        // and its file hides the project's of the same name.
        std::fs::create_dir_all(folder.0.join("scenes/wooze")).unwrap();
        let mut nearer = tuned();
        nearer.wooze.step = Some(0.125);
        let saved = write_preset(&scene_path, "misty", &nearer).unwrap();
        assert_eq!(
            saved,
            folder.0.join("scenes/wooze").join("misty.wooze.toml")
        );
        assert_eq!(read_preset(&scene_path, "misty").unwrap().unwrap(), nearer);
        assert_eq!(list_presets(&scene_path), ["misty"]);
    }

    #[test]
    fn a_preset_needs_a_name_that_can_be_a_file() {
        let folder = Folder::new("names");
        for bad in ["", "../up", "a b", "a.b"] {
            assert!(
                write_preset(&folder.scene(), bad, &tuned()).is_err(),
                "{bad}"
            );
        }
        assert!(list_presets(&folder.scene()).is_empty());
    }

    fn scene_naming(extra: &str) -> SceneFile {
        toml::from_str(&format!(
            "[scene]\nname = \"g\"\n[wooze]\npreset = \"misty\"\n{extra}"
        ))
        .unwrap()
    }

    #[test]
    fn a_file_fills_in_what_the_scene_left_unsaid() {
        let folder = Folder::new("fill");
        write_preset(&folder.scene(), "misty", &tuned()).unwrap();
        let mut scene = scene_naming(
            "glide = 0.9\n[living]\nechoes = { count = 1 }\n[post_process]\nbloom_enabled = true\n",
        );
        assert!(apply_preset_file(&mut scene, &folder.scene()).is_empty());

        let wooze = scene.wooze.as_ref().unwrap();
        assert_eq!(wooze.preset, None);
        assert_eq!(wooze.glide, Some(0.9), "the scene's own word stands");
        assert_eq!(wooze.step, Some(1.0 / 3.0));
        let (field, issues) = wooze.field();
        assert!(issues.is_empty(), "{issues:?}");
        assert_eq!(field.sample([0.0; 3], 0.0).jitter, 2.0);

        let living = scene.living.as_ref().unwrap();
        let echoes = living.echoes.unwrap();
        assert_eq!(echoes.count, Some(1), "a field at a time");
        assert_eq!(echoes.spread, Some(1.8));
        assert_eq!(living.stagger.as_deref(), Some("per_branch"));
        assert_eq!(living.variance.unwrap().amount, Some(1.0));

        let post = scene.post_process.as_ref().unwrap();
        assert_eq!(post.boil("kuwahara"), Some(0.5));
        assert_eq!(post.boil("contour"), Some(1.25));
        assert_eq!(post.boil_on, 3);
        // Made no table, so switched neither style on
        assert!(post.styles.is_empty());
    }

    #[test]
    fn a_file_beats_a_built_in_of_its_name() {
        let folder = Folder::new("shadow");
        write_preset(&folder.scene(), "calm", &tuned()).unwrap();
        let mut scene: SceneFile =
            toml::from_str("[scene]\nname = \"g\"\n[wooze]\npreset = \"calm\"\n").unwrap();
        assert!(apply_preset_file(&mut scene, &folder.scene()).is_empty());
        let (field, _) = scene.wooze.unwrap().field();
        assert_eq!(field.sample([0.0; 3], 0.0).amplitude, 0.7);
    }

    #[test]
    fn a_built_in_name_and_no_file_is_left_alone() {
        let folder = Folder::new("builtin");
        let mut scene: SceneFile =
            toml::from_str("[scene]\nname = \"g\"\n[wooze]\npreset = \"boil\"\n").unwrap();
        assert!(apply_preset_file(&mut scene, &folder.scene()).is_empty());
        assert_eq!(scene.wooze.unwrap().preset.as_deref(), Some("boil"));
    }

    #[test]
    fn a_bad_file_is_named_and_the_scene_still_loads() {
        let folder = Folder::new("bad");
        let dir = folder.0.join("wooze");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("misty.wooze.toml"), "[wooze]\nstepp = 1\n").unwrap();
        let mut scene = scene_naming("");
        let issues = apply_preset_file(&mut scene, &folder.scene());
        assert_eq!(issues.len(), 1, "{issues:?}");
        let said = issues[0].to_string();
        assert!(said.contains("misty") && said.contains("stepp"), "{said}");
        // And nothing goes on to say it isn't a built-in, too.
        assert!(scene.wooze.unwrap().field().1.is_empty());
    }

    #[test]
    fn values_out_of_range_are_named() {
        let mut preset = tuned();
        preset.living.stagger = Some("sideways".into());
        preset.living.variance.as_mut().unwrap().amount = Some(1.5);
        preset.boil.as_mut().unwrap().on = Some(7);
        let problems = preset.problems().join("\n");
        for word in ["sideways", "variance.amount", "boil.on"] {
            assert!(problems.contains(word), "{word} in {problems}");
        }
    }

    #[test]
    fn boil_off_loads_as_no_boil() {
        let folder = Folder::new("off");
        let mut preset = tuned();
        preset.boil.as_mut().unwrap().enabled = Some(false);
        write_preset(&folder.scene(), "misty", &preset).unwrap();
        let mut scene = scene_naming("[post_process]\nbloom_enabled = true\n");
        apply_preset_file(&mut scene, &folder.scene());
        let post = scene.post_process.unwrap();
        assert_eq!(
            (post.boil("kuwahara"), post.boil("contour")),
            (Some(0.0), Some(0.0))
        );
    }

    /// A preset's boil beats the scene's own, in either spelling (#599).
    #[test]
    fn a_preset_boil_beats_the_scenes_style_table() {
        let folder = Folder::new("table");
        write_preset(&folder.scene(), "misty", &tuned()).unwrap();
        let mut scene = scene_naming(
            "[post_process]\n[post_process.styles.contour]\nboil = 0.2\nwidth = 2.0\n",
        );
        apply_preset_file(&mut scene, &folder.scene());
        let post = scene.post_process.unwrap();
        assert_eq!(post.boil("contour"), Some(1.25));
        assert_eq!(post.styles["contour"]["width"].as_float(), Some(2.0));
        assert!(!post.styles.contains_key("kuwahara"));
    }
}

#[cfg(test)]
mod tests {
    use crate::SceneFile;
    use flint_core::wooze::{preset, WoozeSample, AMPLITUDE_MAX, JITTER_MAX, STEP_MIN};
    use flint_schema::SchemaRegistry;

    fn scene(table: &str) -> SceneFile {
        toml::from_str(&format!("[scene]\nname = \"grove\"\n{table}")).unwrap()
    }

    fn schemas() -> SchemaRegistry {
        SchemaRegistry::load_from_directory("../../schemas").unwrap()
    }

    const GROVE: &str = r#"
[wooze]
preset = "linklater"
glide = 0.4
step = 0.25

[[wooze.zone]]
name = "pocket"
shape = "sphere"
center = [0, 0, 12]
radius = 6
falloff = 4
glide = 0.8

[[wooze.zone]]
name = "hush"
shape = "box"
center = [20, 0, 0]
size = [10, 10, 10]
amplitude = 0.2
step = 2
jitter = 0
"#;

    #[test]
    fn the_table_becomes_a_field() {
        let def = scene(GROVE).wooze.unwrap();
        let (field, issues) = def.field();
        assert!(issues.is_empty(), "{issues:?}");
        assert_eq!(field.zones().len(), 2);

        let linklater = preset("linklater").unwrap();
        let far = field.sample([100.0, 0.0, 0.0], 0.0);
        assert_eq!(far.amplitude, linklater.amplitude, "from the preset");
        assert_eq!(far.glide, 0.4, "a global key beats its preset");
        assert_eq!(field.sample([0.0, 0.0, 12.0], 0.0).glide, 0.8);
        let hush = field.sample([20.0, 0.0, 0.0], 0.0);
        assert_eq!((hush.amplitude, hush.step, hush.jitter), (0.2, 2.0, 0.0));
    }

    #[test]
    fn a_bare_table_opts_in_at_neutral() {
        let (field, issues) = scene("[wooze]\n").wooze.unwrap().field();
        assert!(issues.is_empty());
        assert_eq!(field.sample([0.0; 3], 0.0), WoozeSample::NEUTRAL);
        assert!(scene("").wooze.is_none());
    }

    #[test]
    fn a_grove_passes_its_schema() {
        let issues = scene(GROVE).wooze.unwrap().check(&schemas());
        assert!(issues.is_empty(), "{issues:?}");
    }

    #[test]
    fn a_malformed_zone_is_named_and_the_good_ones_still_stand() {
        let def = scene(
            r#"
[wooze]
[[wooze.zone]]
name = "good"
shape = "sphere"
center = [0, 0, 0]
radius = 1
glide = 0.5

[[wooze.zone]]
name = "broken"
shape = "sphere"
center = [0, 0, 0]
radius = -1
glide = 0.5

[[wooze.zone]]
shape = "sphere"
center = [0, 0, 0]
glide = 0.5
"#,
        )
        .wooze
        .unwrap();
        let (field, issues) = def.field();
        assert_eq!(field.zones().len(), 1);
        assert_eq!(issues.len(), 2, "{issues:?}");
        assert!(issues[0].to_string().contains("'broken'"), "{}", issues[0]);
        assert!(issues[0].to_string().contains("radius"), "{}", issues[0]);
        assert!(issues[1].to_string().contains("'zone 3'"), "{}", issues[1]);

        let checked = def.check(&schemas());
        assert_eq!(checked.len(), 2, "one each: {checked:?}");
        assert!(checked.iter().any(|i| i.zone.as_deref() == Some("broken")));
    }

    #[test]
    fn a_wrong_type_is_named_with_its_key() {
        let def = scene(
            r#"
[wooze]
[[wooze.zone]]
name = "wrong"
shape = "sphere"
center = [0, 0, 0]
radius = "six"
glide = 0.5
"#,
        )
        .wooze
        .unwrap();
        let issues = def.check(&schemas());
        assert_eq!(issues.len(), 1, "{issues:?}");
        assert_eq!(issues[0].zone.as_deref(), Some("wrong"));
        assert!(issues[0].message.contains("radius"), "{issues:?}");
    }

    /// A game's schemas load after the engine's, so a game can keep its own world
    /// within narrower limits, and `flint validate` holds a scene to them.
    #[test]
    fn a_games_schema_can_tighten_what_a_zone_may_be() {
        let mut schemas = schemas();
        schemas
            .load_component_string(
                r#"
[component.wooze_zone.fields]
glide = { type = "f32", min = 0.0, max = 0.5 }
[component.wooze.fields]
step = { type = "f32", min = 1.0 }
"#,
            )
            .unwrap();
        let def = scene(GROVE).wooze.unwrap();
        let issues = def.check(&schemas);
        assert_eq!(issues.len(), 2, "{issues:?}");
        assert_eq!(issues[0].zone, None);
        assert!(issues[0].message.contains("step"), "{issues:?}");
        assert_eq!(issues[1].zone.as_deref(), Some("pocket"));
        assert!(issues[1].message.contains("glide"), "{issues:?}");
    }

    #[test]
    fn a_bad_global_knob_or_preset_is_an_issue_with_no_zone() {
        for bad in ["glide = 2.0", "step = 0.0", "preset = \"nope\""] {
            let def = scene(&format!("[wooze]\n{bad}\n")).wooze.unwrap();
            let issues = def.check(&schemas());
            assert_eq!(issues.len(), 1, "{bad}: {issues:?}");
            assert_eq!(issues[0].zone, None);
            assert_eq!(def.field().0.sample([0.0; 3], 0.0), WoozeSample::NEUTRAL);
        }
    }

    #[test]
    fn a_misspelt_global_knob_fails_the_table() {
        let bad = toml::from_str::<SceneFile>("[scene]\nname = \"g\"\n[wooze]\ngilde = 0.3\n");
        assert!(bad.is_err());
    }

    /// The schema is what agents read and games override, the code is what runs,
    /// so they have to say the same thing.
    #[test]
    fn the_schema_and_the_field_agree_on_every_knob() {
        let schemas = schemas();
        let wooze = schemas
            .get_component("wooze")
            .expect("schemas/components/wooze.toml");
        let zone = schemas
            .get_component("wooze_zone")
            .expect("the zone's schema");
        let ranges = [
            ("amplitude", Some(0.0), Some(f64::from(AMPLITUDE_MAX))),
            ("step", Some(f64::from(STEP_MIN)), None),
            ("glide", Some(0.0), Some(1.0)),
            ("jitter", Some(0.0), Some(f64::from(JITTER_MAX))),
        ];
        for schema in [wooze, zone] {
            for (name, min, max) in ranges {
                let field = schema.get_field(name).unwrap_or_else(|| panic!("{name}"));
                let close = |a: Option<f64>, b: Option<f64>| match (a, b) {
                    (Some(a), Some(b)) => (a - b).abs() < 1e-6,
                    (None, None) => true,
                    _ => false,
                };
                assert!(close(field.min, min), "{name} min {:?}", field.min);
                assert!(close(field.max, max), "{name} max {:?}", field.max);
            }
        }
        let mut names: Vec<&str> = zone.field_names();
        names.sort_unstable();
        let mut keys = flint_core::wooze::ZONE_KEYS.to_vec();
        keys.sort_unstable();
        assert_eq!(names, keys);
    }
}

#[cfg(test)]
mod species_tests {
    use crate::format::{LivingDef, SceneFile};

    #[test]
    fn a_scene_says_what_one_species_does_and_a_wrong_key_fails() {
        let scene: SceneFile = toml::from_str(
            r#"
            [scene]
            name = "s"
            [living]
            stagger = "per_level"
            [living.species.grove_thorn]
            step = 0.25
            stagger = "none"
            echoes = { count = 1 }
            "#,
        )
        .unwrap();
        let living = scene.living.unwrap();
        let thorn = &living.species["grove_thorn"];
        assert_eq!(thorn.step, Some(0.25));
        assert_eq!(thorn.echoes.unwrap().count, Some(1));
        assert_eq!(living.stagger.as_deref(), Some("per_level"));

        let wrong = toml::from_str::<LivingDef>("[species.grove_thorn]\nstepp = 1.0\n");
        assert!(wrong.unwrap_err().to_string().contains("stepp"));
    }

    #[test]
    fn a_scenes_species_beat_a_presets_a_field_at_a_time() {
        let scene: LivingDef =
            toml::from_str("[species.thorn]\nstep = 0.25\nechoes = { count = 1 }\n").unwrap();
        let preset: LivingDef = toml::from_str(
            "[species.thorn]\nstep = 2.0\nglide = 0.4\nechoes = { alpha = 0.3 }\n\
             [species.oak]\njitter = 0.5\n",
        )
        .unwrap();
        let both = scene.over(preset);
        let thorn = &both.species["thorn"];
        assert_eq!((thorn.step, thorn.glide), (Some(0.25), Some(0.4)));
        let echoes = thorn.echoes.unwrap();
        assert_eq!((echoes.count, echoes.alpha), (Some(1), Some(0.3)));
        assert_eq!(both.species["oak"].jitter, Some(0.5));
    }

    #[test]
    fn a_bad_species_value_is_named_with_its_species() {
        let preset = crate::WoozePreset::parse(
            "[living.species.thorn]\nglide = 2.0\nstagger = \"sideways\"\n",
        )
        .unwrap();
        let problems = preset.problems();
        assert_eq!(problems.len(), 2, "{problems:?}");
        assert!(problems.iter().all(|p| p.starts_with("species 'thorn': ")));
    }
}
