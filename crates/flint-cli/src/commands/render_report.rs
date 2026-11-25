//! `flint render --report json` (#216): what the living props were doing when they
//! were captured, so an agent can explain what it sees. `flint query` reads scene
//! files and none of this is in one: a lineage is, but which pose a prop has reached
//! and what the field told it exist only once it's running.
//!
//! The render paths read a stage's props through [`of_game`] (#1333): in a
//! build without the living package the player grows none, and the report
//! says so with empty lists.

#[cfg(feature = "living")]
use flint_living::{KnobValue, LivingKnob, LivingPropId, LivingProps, Resolved};
use serde::Serialize;
use std::collections::BTreeMap;

/// The whole report: one JSON object on stdout.
#[derive(Debug, Serialize)]
pub struct RenderReport {
    pub scene: String,
    pub output: String,
    pub width: u32,
    pub height: u32,
    /// The seconds each tick of the clock moved, to compare with `--frame-step`.
    pub frame_step: f64,
    /// Whether a wooze field was deciding, or the specs were.
    pub wooze_field: bool,
    pub wooze_zones: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sweep: Option<String>,
    /// `--frame-dump`'s file (#1084): the described frame, the last capture
    /// of the first cell.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub frame_dump: Option<String>,
    /// `--frame-capture`'s folder (#1085): the captured frame's manifest
    /// and images.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub frame_capture: Option<String>,
    /// One per image in the file: a moment and, in a sweep, a column.
    pub captures: Vec<CaptureReport>,
    /// Entities asking to be living props that drew as still models, with the reason
    /// in the warnings on stderr.
    pub still_props: Vec<String>,
    pub warnings: Vec<String>,
    /// With --scripts: every script error of the run, each once with its
    /// count (#693). Empty when every script ran clean.
    pub script_errors: Vec<ScriptErrorReport>,
    /// What a style's cap left out (#740): the style, the cap, how many it
    /// holds, the first entity past it and how that entity drew instead.
    /// Left out of the JSON when nothing was.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub style_drops: Vec<flint_render::stylize::variant_table::Dropped>,
    /// `--load-report` (#1092): where the load's time went, from the scene
    /// read to the first capture.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub load: Option<flint_core::load_report::LoadReport>,
}

/// One way a script failed (#693), as `flint_script::ScriptError` counts it.
#[derive(Debug, Serialize)]
pub struct ScriptErrorReport {
    pub file: String,
    pub entity: String,
    /// Other entities whose scripts failed the same way.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub also_on: Vec<String>,
    /// `on_update`..., or `load`, `compile`, `top level`.
    pub callback: String,
    /// The helper the error was in, when not the callback itself.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub function: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub column: Option<usize>,
    pub message: String,
    pub count: u64,
    pub first_frame: u64,
    pub last_frame: u64,
}

impl From<&flint_script::ScriptError> for ScriptErrorReport {
    fn from(e: &flint_script::ScriptError) -> Self {
        Self {
            file: e.file.clone(),
            entity: e.entity_name.clone(),
            also_on: e.also_on.clone(),
            callback: e.callback.clone(),
            function: e.function.clone(),
            line: e.line,
            column: e.column,
            message: e.message.clone(),
            count: e.count,
            first_frame: e.first_frame,
            last_frame: e.last_frame,
        }
    }
}

/// Every living prop at one capture.
#[derive(Debug, Serialize)]
pub struct CaptureReport {
    pub time: f64,
    /// The value this column gave the swept knob.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub column: Option<String>,
    /// The value this row gave the second swept key, in a grid (#596).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub row: Option<String>,
    /// With `--reference`: the cell's distance to it, out of 100 (#596).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub distance: Option<f64>,
    /// With `--scripts`: the `state` of the expression scripts saw then (#8).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expression: Option<String>,
    pub props: Vec<PropReport>,
}

/// One living prop at one capture.
#[derive(Debug, Serialize)]
pub struct PropReport {
    pub entity: String,
    pub asset: String,
    pub lineage_seed: u64,
    pub position: [f32; 3],
    pub phase_offset: f32,
    /// Its clock, in steps it has run through.
    pub beats: f64,
    /// The seconds its words have drifted along their lines through (#576): 0
    /// until a form with `row_pace` runs, moving at `row_rate`.
    pub row_time: f64,
    /// `None` only if it hasn't posed, which `flint render` doesn't let happen.
    pub poses: Option<PoseReport>,
    pub wooze: WoozeReport,
    pub echoes: usize,
    /// Its spec's name, which is what `[living.species.<spec>]` is keyed by.
    pub species: String,
    /// Every value a species can set (#246), as the prop runs on it, and the layer
    /// that set it: `spec`, `entity`, `global`, `zone:<name>` or `species`.
    pub values: BTreeMap<&'static str, ValueReport>,
}

/// One resolved value and who said so.
#[derive(Debug, PartialEq, Serialize)]
pub struct ValueReport {
    pub value: ReportValue,
    pub layer: String,
}

/// A value as JSON writes it: an `f32` as briefly as it reads back, as the rest
/// of the report does.
#[derive(Debug, PartialEq, Serialize)]
#[serde(untagged)]
// Made from a living prop's values: never, in a build without the package
#[cfg_attr(not(feature = "living"), allow(dead_code))]
pub enum ReportValue {
    Number(f32),
    Count(u32),
    Flag(bool),
    Name(String),
}

#[cfg(feature = "living")]
impl From<&KnobValue> for ReportValue {
    fn from(value: &KnobValue) -> Self {
        match value {
            KnobValue::Number(value) => Self::Number(*value),
            KnobValue::Count(value) => Self::Count(*value),
            KnobValue::Flag(value) => Self::Flag(*value),
            KnobValue::Name(value) => Self::Name(value.clone()),
        }
    }
}

#[cfg(feature = "living")]
fn value(resolved: &Resolved<KnobValue>) -> ValueReport {
    ValueReport {
        value: (&resolved.value).into(),
        layer: resolved.layer.label(),
    }
}

/// Which poses a prop was drawn between, from the blend that drew it. A staggered
/// tree's parts aren't all on one: `lowest` and `highest` are how far they spread.
#[derive(Debug, PartialEq, Serialize)]
pub struct PoseReport {
    /// The trunk's pose: the one it's leaving.
    pub index: u64,
    /// How far the trunk has gone toward the next, 0 to 1.
    pub weight: f32,
    /// That pose's shape seed, which `pose()` would regrow it from.
    pub shape_seed: u64,
    pub lowest: u64,
    pub highest: u64,
    /// How many parts were between poses, not holding on one.
    pub gliding_parts: usize,
    pub parts: usize,
}

/// What the wooze field said about where a prop stands.
#[derive(Debug, PartialEq, Serialize)]
pub struct WoozeReport {
    pub amplitude: f32,
    pub step: f32,
    pub glide: f32,
    pub jitter: f32,
}

/// Every prop as it stands, by name, since a scene's entities load in no fixed
/// order and two runs ought to read the same.
/// `field` is the stage's, where its clock stands.
#[cfg(feature = "living")]
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
#[cfg(feature = "living")]
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
#[cfg(feature = "living")]
pub fn read_note(knob: &str, set: Option<f32>, props: &LivingProps) -> String {
    let read = |prop: &flint_living::LivingProp| {
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

/// A stage's living props and wooze field as the render paths read them.
#[cfg(feature = "living")]
pub mod of_game {
    use flint_player::HeadlessGame;

    /// Every prop as it stands (`--report json`'s `props`).
    pub fn props(game: &HeadlessGame) -> Vec<super::PropReport> {
        game.living_props()
            .map_or_else(Vec::new, |props| super::props_report(props, game.wooze()))
    }

    /// How many props the scene grew.
    pub fn count(game: &HeadlessGame) -> usize {
        game.living_props().map_or(0, |props| props.props().len())
    }

    /// The entities that grew into props.
    pub fn entities(game: &HeadlessGame) -> Vec<flint_core::EntityId> {
        game.living_props().map_or_else(Vec::new, |props| {
            props.props().iter().map(|prop| prop.entity()).collect()
        })
    }

    /// What a wooze column's props read, against what it set.
    pub fn read_note(game: &HeadlessGame, knob: &str, set: Option<f32>) -> String {
        game.living_props()
            .map_or_else(String::new, |props| super::read_note(knob, set, props))
    }

    /// The zones of the stage's wooze field, with one.
    pub fn wooze_zones(game: &HeadlessGame) -> Option<usize> {
        game.wooze().map(|(field, _)| field.zones().len())
    }
}

/// Built without the living package: the player grows no props and runs
/// no field, so there is nothing to read.
#[cfg(not(feature = "living"))]
pub mod of_game {
    use flint_player::HeadlessGame;

    pub fn props(_: &HeadlessGame) -> Vec<super::PropReport> {
        Vec::new()
    }

    pub fn count(_: &HeadlessGame) -> usize {
        0
    }

    pub fn entities(_: &HeadlessGame) -> Vec<flint_core::EntityId> {
        Vec::new()
    }

    pub fn read_note(_: &HeadlessGame, _: &str, _: Option<f32>) -> String {
        String::new()
    }

    pub fn wooze_zones(_: &HeadlessGame) -> Option<usize> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_report_with_no_sweep_leaves_the_key_out() {
        let report = RenderReport {
            scene: "s".into(),
            output: "o.png".into(),
            width: 4,
            height: 4,
            frame_step: 0.25,
            wooze_field: false,
            wooze_zones: 0,
            sweep: None,
            frame_dump: None,
            frame_capture: None,
            captures: vec![CaptureReport {
                time: 3.5,
                column: None,
                row: None,
                distance: None,
                expression: None,
                props: vec![],
            }],
            still_props: vec![],
            warnings: vec![],
            script_errors: vec![],
            style_drops: vec![],
            load: None,
        };
        let json: serde_json::Value =
            serde_json::from_str(&serde_json::to_string(&report).unwrap()).unwrap();
        assert!(json.get("sweep").is_none());
        assert!(json.get("frame_dump").is_none());
        // Present and empty when every script ran clean (#693)
        assert_eq!(json["script_errors"], serde_json::json!([]));
        assert_eq!(json["captures"][0]["time"], 3.5);
        assert!(json["captures"][0].get("column").is_none());
    }

    #[cfg(feature = "living")]
    #[test]
    fn no_props_means_nothing_to_note() {
        let props = LivingProps::new(1024);
        assert_eq!(read_note("glide", Some(0.5), &props), "");
    }
}
