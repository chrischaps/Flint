//! Living's part of a `flint render` stage (#216, #1558): the props grown
//! and posed at `--time`, a sweep's wooze and echoes columns, the shading
//! boil read off the field, and `--report json`'s props. The render
//! declares the point (`flint_package::cli::render::StagePart`); this is
//! what its `living` feature did inline until the package had a crate.

use anyhow::{anyhow, Result};
use flint_core::field_frame::BoilFrame;
use flint_core::EntityId;
use flint_ecs::FlintWorld;
use flint_package::cli::render::{
    PoseReport, PropReport, ReportValue, StageClock, StagePart, TableAxes, ValueReport, WoozeAxes,
    WoozeReport,
};
use flint_package::{CaptureGame, CaptureGameMut, Resources};
use flint_render::{PostProcessConfig, SceneRenderer};
use flint_scene::SceneFile;
use std::collections::BTreeMap;
use std::path::Path;

use crate::player::{props, subsystem};
use crate::{KnobValue, LivingKnob, LivingPropId, LivingProps, Resolved};

/// The render's living part.
pub struct Stages;

impl StagePart for Stages {
    fn grows(&self, world: &FlintWorld) -> bool {
        props::wanted_by(world)
    }

    fn check_cell(&self, scene: &Path, sweep: &str, axes: WoozeAxes) -> Result<()> {
        let (_, Some(table)) = cell_knobs(scene, axes)? else {
            return Ok(());
        };
        let mut field = flint_wooze::WoozeField::new(flint_wooze::WoozeSample::NEUTRAL);
        field
            .set_global(&table, 0.0, 0.0)
            .map_err(|reason| anyhow!("--sweep {sweep}: {reason}"))
    }

    fn check_echo(&self, knob: &str, value: &str) -> Result<(), String> {
        echo_value(knob, value)
    }

    fn lay_tables(&self, tables: &mut toml::Table, axes: TableAxes) {
        lay_tables(tables, axes)
    }

    fn stage(
        &self,
        scene: &SceneFile,
        path: &Path,
        axes: WoozeAxes,
        grows: bool,
        resources: &mut Resources,
    ) -> Result<Box<dyn StageClock>> {
        // A preset column names a preset file beside the scene before a
        // built-in, as the player's Load does (#249), and the file's
        // [living] tables, species and all, go over the scene's as they do
        // there.
        let (preset, knobs) = cell_knobs(path, axes)?;
        if let (true, Some(preset)) = (grows, &preset) {
            for problem in props::apply_preset(subsystem::props_mut(resources), &preset.living) {
                eprintln!("Warning: --sweep preset: {problem}");
            }
        }
        // The stage's one wooze field (#581), with the column's knobs over
        // the scene's: the living props read it, the shading boil always,
        // and shallow wooze in a capture of a moment. A plain render shows
        // those meshes at rest (#215).
        let field = props::wooze_of_scene(
            flint_procgen::LivingTables::or_warn(scene).wooze.as_ref(),
            knobs.as_ref(),
        )
        .map_err(|reason| anyhow!("--sweep: {reason}"))?;
        // The capture's living subsystem takes the field as it is built
        resources.insert(subsystem::CaptureWooze(field.clone()));
        Ok(Box::new(Clock {
            boil: preset.and_then(|preset| preset.boil),
            field,
        }))
    }

    fn upload(
        &self,
        game: CaptureGameMut,
        renderer: &mut SceneRenderer,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
    ) {
        subsystem::upload_living(game, renderer, device, queue);
    }

    fn props(&self, game: CaptureGame) -> Vec<PropReport> {
        let field = subsystem::wooze_of(&game);
        grown(&game).map_or_else(Vec::new, |props| props_report(props, field))
    }

    fn count(&self, game: CaptureGame) -> usize {
        grown(&game).map_or(0, |props| props.props().len())
    }

    fn entities(&self, game: CaptureGame) -> Vec<EntityId> {
        grown(&game).map_or_else(Vec::new, |props| {
            props.props().iter().map(|prop| prop.entity()).collect()
        })
    }

    fn read_note(&self, game: CaptureGame, knob: &str, set: Option<f32>) -> String {
        grown(&game).map_or_else(String::new, |props| read_note(knob, set, props))
    }

    fn zones(&self, game: CaptureGame) -> Option<usize> {
        subsystem::wooze_of(&game).map(|(field, _)| field.zones().len())
    }
}

/// The stage's props, if it grew any.
fn grown<'g>(game: &CaptureGame<'g>) -> Option<&'g LivingProps> {
    let props = game.resources.get::<LivingProps>()?;
    (!props.is_empty()).then_some(props)
}

/// One stage's field and a preset column's boil.
struct Clock {
    boil: Option<flint_procgen::BoilDef>,
    field: Option<flint_wooze::WoozeField>,
}

impl StageClock for Clock {
    fn lay_boil(&self, config: &mut PostProcessConfig) -> bool {
        let Some(boil) = self.boil else {
            return false;
        };
        let on = boil.enabled.unwrap_or(true);
        let (paint, ink) = config.boil_amounts();
        let amount = |v: Option<f32>, was: f32| v.map_or(was, |v| if on { v } else { 0.0 });
        config.set_boil_amounts(amount(boil.kuwahara, paint), amount(boil.contour, ink));
        if let Some(cadence) = boil.on {
            config.boil_on = cadence.clamp(1, 3);
        }
        true
    }

    fn has_field(&self) -> bool {
        self.field.is_some()
    }

    fn boil_at(&self, eye: [f32; 3], time: f64, boil_on: u32) -> Option<BoilFrame> {
        let field = self.field.as_ref()?;
        Some(props::boil_at(field, eye, time, boil_on))
    }
}

/// The preset file a `wooze.preset=<value>` names, if it's a file the scene
/// can see rather than a built-in (#249).
fn preset_file_of(
    scene: &Path,
    knob: &str,
    value: &str,
) -> Result<Option<flint_procgen::WoozePreset>> {
    if knob != "preset" {
        return Ok(None);
    }
    match flint_procgen::read_preset(scene, value) {
        None => Ok(None),
        Some(Ok(preset)) => Ok(Some(preset)),
        Some(Err(reason)) => Err(anyhow!("--sweep: preset '{value}': {reason}")),
    }
}

/// What a sweep cell lays over the scene's `[wooze]` table: a preset file one
/// of its axes names (its knobs first), then every other wooze value it takes,
/// so a `wooze.glide` row turns the glide a preset column set (#596). `None`
/// for a cell no axis of which is a wooze knob.
fn cell_knobs(
    scene: &Path,
    axes: WoozeAxes,
) -> Result<(Option<flint_procgen::WoozePreset>, Option<toml::Table>)> {
    let mut preset = None;
    for &(knob, value) in axes {
        if let Some(file) = preset_file_of(scene, knob, value)? {
            preset = Some(file);
        }
    }
    let mut table = preset.as_ref().map(|p| preset_wooze_table(&p.wooze));
    for &(knob, value) in axes {
        if preset.is_some() && knob == "preset" {
            continue;
        }
        table
            .get_or_insert_with(toml::Table::new)
            .insert(knob.to_string(), wooze_value(value));
    }
    Ok((preset, table))
}

/// A `wooze.<knob>` column's value as the field's table takes it: a
/// number, else its name (a preset's).
fn wooze_value(text: &str) -> toml::Value {
    match text.parse::<f64>() {
        Ok(number) => toml::Value::Float(number),
        Err(_) => toml::Value::String(text.to_string()),
    }
}

/// A preset's `[wooze]` knobs as the table the field takes.
fn preset_wooze_table(knobs: &flint_procgen::WoozeKnobs) -> toml::Table {
    let mut table = toml::Table::new();
    for (key, value) in [
        ("amplitude", knobs.amplitude),
        ("step", knobs.step),
        ("glide", knobs.glide),
        ("jitter", knobs.jitter),
    ] {
        if let Some(value) = value {
            table.insert(key.to_string(), toml::Value::Float(f64::from(value)));
        }
    }
    table
}

/// A cell's `living.echoes.<knob>` values written into its scene's
/// `[living] echoes` (#531, the kernel's until #1595), as a scene would
/// write them: `count` an integer, the others floats. The table and its
/// `echoes` are made when the scene has none; a `[living]` that isn't a
/// table was refused when the scene settled, and is passed over.
fn lay_tables(tables: &mut toml::Table, axes: TableAxes) {
    'axes: for &(key, text) in axes {
        let mut path: Vec<&str> = key.split('.').collect();
        let Some(field) = path.pop() else {
            continue;
        };
        let value = match field {
            "count" => text.parse().map(toml::Value::Integer).ok(),
            _ => text.parse().map(toml::Value::Float).ok(),
        };
        let Some(value) = value else {
            continue;
        };
        let mut at: &mut toml::Table = tables;
        for part in path {
            let entry = at
                .entry(part)
                .or_insert_with(|| toml::Value::Table(toml::Table::new()));
            match entry.as_table_mut() {
                Some(table) => at = table,
                None => continue 'axes,
            }
        }
        at.insert(field.to_string(), value);
    }
}

/// One `living.echoes.<knob>` value, held to what a spec's echoes take
/// (`EchoConfig::check`).
fn echo_value(knob: &str, value: &str) -> Result<(), String> {
    let mut echoes = flint_procgen::EchoConfig::default();
    if knob == "count" {
        echoes.count = value.parse().map_err(|_| {
            format!("--sweep: living.echoes.count takes whole numbers, and '{value}' isn't")
        })?;
    } else {
        let number = value
            .parse::<f32>()
            .ok()
            .filter(|n| n.is_finite())
            .ok_or_else(|| {
                format!("--sweep: living.echoes.{knob} takes numbers, and '{value}' isn't")
            })?;
        if knob == "spread" {
            echoes.spread = number;
        } else {
            echoes.alpha = number;
        }
    }
    // `why` starts with the value: `3: must be from 0 to 1.82 steps, ...`
    echoes
        .check()
        .map_err(|(_, why)| format!("--sweep living.echoes.{knob}={why}"))
}

/// A knob's value as the report writes it.
fn report_value(value: &KnobValue) -> ReportValue {
    match value {
        KnobValue::Number(value) => ReportValue::Number(*value),
        KnobValue::Count(value) => ReportValue::Count(*value),
        KnobValue::Flag(value) => ReportValue::Flag(*value),
        KnobValue::Name(value) => ReportValue::Name(value.clone()),
    }
}

fn value(resolved: &Resolved<KnobValue>) -> ValueReport {
    ValueReport {
        value: report_value(&resolved.value),
        layer: resolved.layer.label(),
    }
}

/// Every prop as it stands, by name, since a scene's entities load in no fixed
/// order and two runs ought to read the same.
/// `field` is the stage's, where its clock stands.
pub fn props_report(
    props: &LivingProps,
    field: Option<(&flint_wooze::WoozeField, f64)>,
) -> Vec<PropReport> {
    let mut report: Vec<PropReport> = props
        .props()
        .iter()
        .enumerate()
        .map(|(i, prop)| {
            let id = LivingPropId(i);
            let wooze = prop.wooze();
            let times = prop.drawn_times();
            let poses = times.first().map(|trunk| PoseReport {
                index: trunk.index,
                weight: trunk.weight,
                shape_seed: prop.shape_seed(trunk.index),
                lowest: times.iter().map(|t| t.index).min().unwrap_or(trunk.index),
                highest: times.iter().map(|t| t.index).max().unwrap_or(trunk.index),
                gliding_parts: times
                    .iter()
                    .filter(|t| t.weight > 0.0 && t.weight < 1.0)
                    .count(),
                parts: times.len(),
            });
            PropReport {
                entity: prop.entity_name().to_string(),
                asset: prop.asset_name().to_string(),
                lineage_seed: props.lineage_seed(id),
                position: prop.position(),
                phase_offset: prop.phase_offset(),
                beats: prop.beats(),
                row_time: prop.row_time(),
                poses,
                wooze: WoozeReport {
                    amplitude: wooze.amplitude,
                    step: wooze.step,
                    glide: wooze.glide,
                    jitter: wooze.jitter,
                },
                echoes: prop.echo_count(),
                species: props.species_of(id).to_string(),
                values: values_of(props, id, field),
            }
        })
        .collect();
    report.sort_by(|a, b| a.entity.cmp(&b.entity));
    report
}

/// Each value a species can set on a prop, by the key a scene gives it.
fn values_of(
    props: &LivingProps,
    id: LivingPropId,
    field: Option<(&flint_wooze::WoozeField, f64)>,
) -> BTreeMap<&'static str, ValueReport> {
    let at = props.resolved(id, field);
    (at.iter())
        .map(|(knob, resolved): (LivingKnob, _)| (knob.key(), value(resolved)))
        .collect()
}

/// How the values a sweep's column set compare with what its props actually read:
/// nothing to say if they agree, or the spread if zones kept some off it.
pub fn read_note(knob: &str, set: Option<f32>, props: &LivingProps) -> String {
    let read = |prop: &crate::LivingProp| {
        let wooze = prop.wooze();
        match knob {
            "amplitude" => Some(wooze.amplitude),
            "step" => Some(wooze.step),
            "glide" => Some(wooze.glide),
            "jitter" => Some(wooze.jitter),
            _ => None,
        }
    };
    let values: Vec<f32> = props.props().iter().filter_map(read).collect();
    let (Some(set), false) = (set, values.is_empty()) else {
        return String::new();
    };
    let low = values.iter().copied().fold(f32::INFINITY, f32::min);
    let high = values.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    if low == set && high == set {
        return String::new();
    }
    if (high - low).abs() < 0.005 {
        format!("PROPS READ {low:.2}")
    } else {
        format!("PROPS READ {low:.2}-{high:.2}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_props_means_nothing_to_note() {
        let props = LivingProps::new(1024);
        assert_eq!(read_note("glide", Some(0.5), &props), "");
    }

    /// A cell's preset file is laid first and its other wooze values over
    /// it; a built-in preset's name stays a value of the table.
    #[test]
    fn a_cells_wooze_values_make_its_table() {
        let (preset, table) = cell_knobs(
            Path::new("nowhere.scene.toml"),
            &[("glide", "0.5"), ("preset", "calm")],
        )
        .unwrap();
        assert!(preset.is_none());
        let table = table.unwrap();
        assert_eq!(table["glide"], toml::Value::Float(0.5));
        assert_eq!(table["preset"].as_str(), Some("calm"));
        assert_eq!(cell_knobs(Path::new("x"), &[]).unwrap().1, None);
    }

    /// #531: an echoes cell is written into the scene's `[living] echoes`
    /// as a scene would write it, over what the scene wrote and beside it.
    #[test]
    fn a_cells_echoes_are_written_into_the_living_table() {
        let mut tables: toml::Table = toml::from_str(
            "[living]\nstagger = \"none\"\nsoftness = 0.3\nechoes = { count = 2, spread = 0.2 }",
        )
        .unwrap();
        lay_tables(&mut tables, &[("living.echoes.spread", "0.9")]);
        let expected: toml::Table = toml::from_str(
            "[living]\nstagger = \"none\"\nsoftness = 0.3\nechoes = { count = 2, spread = 0.9 }",
        )
        .unwrap();
        assert_eq!(tables, expected);

        // A scene with no [living] gets one holding only the field
        let mut bare = toml::Table::new();
        lay_tables(&mut bare, &[("living.echoes.spread", "1.8")]);
        let expected: toml::Table = toml::from_str("[living]\nechoes = { spread = 1.8 }").unwrap();
        assert_eq!(bare, expected);

        // Both axes of a grid, count a whole number
        let mut both = toml::Table::new();
        lay_tables(
            &mut both,
            &[("living.echoes.alpha", "0.4"), ("living.echoes.count", "6")],
        );
        let expected: toml::Table =
            toml::from_str("[living]\nechoes = { alpha = 0.4, count = 6 }").unwrap();
        assert_eq!(both, expected);

        // A [living] that isn't a table is left as it is
        let mut odd: toml::Table = toml::from_str("living = 3").unwrap();
        lay_tables(&mut odd, &[("living.echoes.spread", "0.9")]);
        assert_eq!(odd["living"], toml::Value::Integer(3));
    }

    #[test]
    fn an_echo_value_is_held_to_what_echoes_take() {
        assert!(echo_value("count", "3").is_ok());
        assert!(echo_value("count", "1.5")
            .unwrap_err()
            .contains("whole numbers"));
        assert!(echo_value("spread", "x")
            .unwrap_err()
            .contains("takes numbers"));
    }
}
