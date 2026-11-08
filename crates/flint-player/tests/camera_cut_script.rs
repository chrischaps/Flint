//! A script resetting style history at its own cut (#343): the Rhai calls
//! queue the commands the player turns into `reset_style_history()` on the
//! renderer and a switch on its camera-cut detector.

use flint_ecs::FlintWorld;
use flint_script::context::ScriptCommand;
use flint_script::engine::ScriptEngine;

const DIRECTOR: &str = r#"
fn on_init() {
    set_camera_cut_detection(false);
}

fn on_update() {
    reset_style_history();
}
"#;

#[test]
fn a_script_resets_style_history_and_switches_detection() {
    let mut world = FlintWorld::new();
    let mut engine = ScriptEngine::new();
    let id = world.spawn("director").unwrap();
    let ast = engine.compile(DIRECTOR).unwrap();
    engine.add_script(id, ast, "director.rhai".into());

    engine.call_inits(&mut world);
    let init = engine.drain_commands();
    assert!(
        matches!(
            init.as_slice(),
            [ScriptCommand::SetCameraCutDetection { enabled: false }]
        ),
        "{init:?}"
    );

    engine.call_updates(&mut world);
    let update = engine.drain_commands();
    assert!(
        matches!(update.as_slice(), [ScriptCommand::ResetStyleHistory]),
        "{update:?}"
    );
}
