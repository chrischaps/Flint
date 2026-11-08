//! A script switching looks (#356): `set_look` and `clear_look` queue the
//! command the player turns into `PlayerApp::set_look`.

use flint_ecs::FlintWorld;
use flint_script::context::ScriptCommand;
use flint_script::engine::ScriptEngine;

const DIRECTOR: &str = r#"
fn on_init() {
    set_look("graphite");
}

fn on_update() {
    set_look("  ");
    clear_look();
}
"#;

#[test]
fn a_script_sets_and_clears_a_look() {
    let mut world = FlintWorld::new();
    let mut engine = ScriptEngine::new();
    let id = world.spawn("director").unwrap();
    let ast = engine.compile(DIRECTOR).unwrap();
    engine.add_script(id, ast, "director.rhai".into());

    engine.call_inits(&mut world);
    let init = engine.drain_commands();
    assert!(
        matches!(init.as_slice(), [ScriptCommand::SetLook { name: Some(n) }] if n == "graphite"),
        "{init:?}"
    );

    engine.call_updates(&mut world);
    let update = engine.drain_commands();
    assert!(
        matches!(
            update.as_slice(),
            [
                ScriptCommand::SetLook { name: None },
                ScriptCommand::SetLook { name: None }
            ]
        ),
        "a blank name is no look: {update:?}"
    );
}
