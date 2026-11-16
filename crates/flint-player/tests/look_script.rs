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
    set_look("watercolor", 2);
    clear_look(0.5);
    set_look("ink_wash", -1.0);
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
        matches!(
            init.as_slice(),
            [ScriptCommand::SetLook { name: Some(n), seconds }] if n == "graphite" && *seconds == 0.0
        ),
        "{init:?}"
    );

    engine.call_updates(&mut world);
    let update = engine.drain_commands();
    assert!(
        matches!(
            update.as_slice(),
            [
                ScriptCommand::SetLook { name: None, .. },
                ScriptCommand::SetLook { name: None, .. },
                ScriptCommand::SetLook { name: Some(fade), seconds: fade_secs },
                ScriptCommand::SetLook { name: None, seconds: clear_secs },
                ScriptCommand::SetLook { name: Some(_), seconds: cut },
            ] if fade == "watercolor" && *fade_secs == 2.0 && *clear_secs == 0.5 && *cut == 0.0
        ),
        "a blank name is no look, an integer is seconds, and a negative fade is a cut: {update:?}"
    );
}
