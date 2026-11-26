//! `flint render --report json` (#216): what the living props were doing when they
//! were captured, so an agent can explain what it sees. `flint query` reads scene
//! files and none of this is in one: a lineage is, but which pose a prop has reached
//! and what the field told it exist only once it's running.
//!
//! The render paths read a stage's props through [`of_game`] (#1333): the
//! living package's render part reads them (#1558,
//! `flint_package::cli::render::StagePart`), and in a build without it the
//! player grows none, and the report says so with empty lists. The props'
//! shape is the Package API's, so the document is one whoever fills it.

use serde::Serialize;

pub use flint_package::cli::render::PropReport;

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

/// A stage's living props and wooze field as the render paths read them:
/// the living package's, through the render's point, and nothing without it.
pub mod of_game {
    use flint_player::HeadlessGame;

    /// Every prop as it stands (`--report json`'s `props`).
    pub fn props(game: &HeadlessGame) -> Vec<super::PropReport> {
        super::super::render::stages()
            .map_or_else(Vec::new, |stages| stages.props(game.capture_game()))
    }

    /// How many props the scene grew.
    pub fn count(game: &HeadlessGame) -> usize {
        super::super::render::stages().map_or(0, |stages| stages.count(game.capture_game()))
    }

    /// The entities that grew into props.
    pub fn entities(game: &HeadlessGame) -> Vec<flint_core::EntityId> {
        super::super::render::stages()
            .map_or_else(Vec::new, |stages| stages.entities(game.capture_game()))
    }

    /// What a wooze column's props read, against what it set.
    pub fn read_note(game: &HeadlessGame, knob: &str, set: Option<f32>) -> String {
        super::super::render::stages().map_or_else(String::new, |stages| {
            stages.read_note(game.capture_game(), knob, set)
        })
    }

    /// The zones of the stage's wooze field, with one.
    pub fn wooze_zones(game: &HeadlessGame) -> Option<usize> {
        super::super::render::stages().and_then(|stages| stages.zones(game.capture_game()))
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
}
