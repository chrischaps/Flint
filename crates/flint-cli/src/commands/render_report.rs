//! `flint render --report json` (#216): what the living props were doing when they
//! were captured, so an agent can explain what it sees. `flint query` reads scene
//! files and none of this is in one: a lineage is, but which pose a prop has reached
//! and what the field told it exist only once it's running.

use flint_procgen::{LivingPropId, LivingProps};
use serde::Serialize;

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
    /// One per image in the file: a moment and, in a sweep, a column.
    pub captures: Vec<CaptureReport>,
    /// Entities asking to be living props that drew as still models, with the reason
    /// in the warnings on stderr.
    pub still_props: Vec<String>,
    pub warnings: Vec<String>,
}

/// Every living prop at one capture.
#[derive(Debug, Serialize)]
pub struct CaptureReport {
    pub time: f64,
    /// The value this column gave the swept knob.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub column: Option<String>,
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
    /// `None` only if it hasn't posed, which `flint render` doesn't let happen.
    pub poses: Option<PoseReport>,
    pub wooze: WoozeReport,
    pub echoes: usize,
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
pub fn props_report(props: &LivingProps) -> Vec<PropReport> {
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
                poses,
                wooze: WoozeReport {
                    amplitude: wooze.amplitude,
                    step: wooze.step,
                    glide: wooze.glide,
                    jitter: wooze.jitter,
                },
                echoes: prop.echo_count(),
            }
        })
        .collect();
    report.sort_by(|a, b| a.entity.cmp(&b.entity));
    report
}

/// How the values a sweep's column set compare with what its props actually read:
/// nothing to say if they agree, or the spread if zones kept some off it.
pub fn read_note(knob: &str, set: Option<f32>, props: &LivingProps) -> String {
    let read = |prop: &flint_procgen::LivingProp| {
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
            captures: vec![CaptureReport {
                time: 3.5,
                column: None,
                props: vec![],
            }],
            still_props: vec![],
            warnings: vec![],
        };
        let json: serde_json::Value =
            serde_json::from_str(&serde_json::to_string(&report).unwrap()).unwrap();
        assert!(json.get("sweep").is_none());
        assert_eq!(json["captures"][0]["time"], 3.5);
        assert!(json["captures"][0].get("column").is_none());
    }

    #[test]
    fn no_props_means_nothing_to_note() {
        let props = LivingProps::new(1024);
        assert_eq!(read_note("glide", Some(0.5), &props), "");
    }
}
