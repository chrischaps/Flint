//! What the script error card shows (#693): each error of the scripts
//! running now, most recent first, as `flint_debug_ui::paint_script_errors`
//! paints it.

use flint_debug_ui::{grouped, ScriptErrorRow};
use flint_script::errors::{COMPILE, LOAD, TOP_LEVEL};
use flint_script::{ScriptError, ScriptErrors};

/// The card's rows, and how many errors the store counted but didn't keep.
pub(super) fn rows(errors: &ScriptErrors) -> (Vec<ScriptErrorRow>, usize) {
    if errors.is_empty() {
        return (Vec::new(), 0);
    }
    let rows = errors.on_screen().iter().map(row).collect();
    let unkept = errors.distinct() as usize - errors.all().len();
    (rows, unkept)
}

fn row(err: &ScriptError) -> ScriptErrorRow {
    let dot = " \u{b7} ";
    let mut heading = match err.callback.as_str() {
        LOAD => format!("script not found{dot}{}", err.entity_name),
        COMPILE => format!("script doesn't compile{dot}{}", err.entity_name),
        TOP_LEVEL => format!("script error{dot}{}{dot}top level", err.entity_name),
        callback => format!("script error{dot}{}{dot}{callback}", err.entity_name),
    };
    if let Some(function) = &err.function {
        heading.push_str(&format!(" \u{203a} {function}"));
    }
    if !err.also_on.is_empty() {
        heading.push_str(&format!(" (and {})", err.also_on.join(", ")));
    }
    let count = if err.is_load_error() {
        "at load: the entity has no script until the file is fixed".to_string()
    } else if err.count == 1 {
        format!("once, frame {}", err.first_frame)
    } else {
        format!(
            "{} times since frame {}",
            grouped(err.count),
            err.first_frame
        )
    };
    ScriptErrorRow {
        heading,
        location: err.location(),
        message: err.message.clone(),
        count,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flint_script::errors::Failure;

    #[test]
    fn a_row_names_the_entity_callback_place_and_count() {
        let mut errors = ScriptErrors::new();
        errors.set_quiet(true);
        let failure = Failure {
            file: "scripts/hud.rhai".into(),
            entity_name: "hud_controller".into(),
            callback: "on_update".into(),
            function: Some("draw_bar".into()),
            line: Some(42),
            column: Some(5),
            message: "Function not found: drw_text (i64)".into(),
            once_a_scene: false,
        };
        for _ in 0..1240 {
            errors.next_frame();
            errors.record(failure.clone());
        }
        let (rows, unkept) = rows(&errors);
        assert_eq!(unkept, 0);
        assert_eq!(
            rows[0],
            ScriptErrorRow {
                heading: "script error \u{b7} hud_controller \u{b7} on_update \u{203a} draw_bar"
                    .into(),
                location: "scripts/hud.rhai:42:5".into(),
                message: "Function not found: drw_text (i64)".into(),
                count: "1,240 times since frame 1".into(),
            }
        );
    }
}
