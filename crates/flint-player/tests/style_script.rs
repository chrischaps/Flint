//! Scripts setting and clearing an entity's style (#269): the Rhai calls
//! queue commands, the player applies them to the world, and a set or a
//! clear fires `style_changed` to scripts' `on_event`.

use flint_ecs::FlintWorld;
use flint_player::{apply_style_command, StyleApplied, STYLE_CHANGED};
use flint_runtime::GameEvent;
use flint_script::engine::ScriptEngine;

/// One frame as the player runs it: scripts' updates, then their commands.
/// Returns the events the commands fired and the failures they logged.
fn frame(engine: &mut ScriptEngine, world: &mut FlintWorld) -> (Vec<GameEvent>, Vec<String>) {
    engine.call_updates(world);
    let mut events = Vec::new();
    let mut failures = Vec::new();
    for cmd in engine.drain_commands() {
        match apply_style_command(world, &cmd) {
            StyleApplied::Done(Some(e)) => events.push(e),
            StyleApplied::Failed(f) => failures.push(f),
            _ => {}
        }
    }
    (events, failures)
}

/// The one `style_changed` event in `events`, as (entity, style).
fn style_changed(events: &[GameEvent]) -> (i64, String) {
    match events {
        [GameEvent::Custom { name, data }] if name == STYLE_CHANGED => (
            data.get("entity").and_then(|v| v.as_integer()).unwrap(),
            data.get("style")
                .and_then(|v| v.as_str())
                .unwrap()
                .to_string(),
        ),
        other => panic!("one style_changed event expected, got {other:?}"),
    }
}

fn director(engine: &mut ScriptEngine, world: &mut FlintWorld, src: &str) {
    let id = world.spawn("director").unwrap();
    let mut story = toml::Table::new();
    story.insert("frame".into(), toml::Value::Integer(0));
    world.set_component(id, "story", story.into()).unwrap();
    let ast = engine.compile(src).unwrap();
    engine.add_script(id, ast, "director.rhai".into());
    engine.call_inits(world);
}

// Frame 0 styles the barrel, 1 to 3 grow its cells, 4 clears it. Integer
// literals are what a web script (i64 only) hands over.
const STORY: &str = r#"
fn on_update() {
    let me = self_entity();
    let n = get_field(me, "story", "frame");
    let barrel = get_entity("barrel");
    if n == 0 {
        set_entity_style(barrel, "ascii");
        set_style_param(barrel, "ramp", "blocks");
        set_style_param(barrel, "tint", [1.0, 0.6, 0.2]);
    } else if n < 4 {
        set_style_param(barrel, "cell", 6 + 2 * n);
        set_style_param(barrel, "levels", 4);
    } else if n == 4 {
        clear_entity_style(barrel);
    }
    set_field(me, "story", "frame", n + 1);
}
"#;

#[test]
fn a_script_sets_animates_and_clears_a_style() {
    let mut world = FlintWorld::new();
    let barrel = world.spawn("barrel").unwrap();
    let mut engine = ScriptEngine::new();
    director(&mut engine, &mut world, STORY);

    let (events, failures) = frame(&mut engine, &mut world);
    assert!(failures.is_empty(), "{failures:?}");
    let style = world.get_component(barrel, "ascii_style").expect("styled");
    assert_eq!(style.get("ramp").and_then(|v| v.as_str()), Some("blocks"));
    assert_eq!(
        style
            .get("tint")
            .and_then(|v| v.as_array())
            .map(|a| a.len()),
        Some(3)
    );
    assert_eq!(
        style_changed(&events),
        (barrel.0 as i64, "ascii".to_string())
    );

    for expect in [8.0, 10.0, 12.0] {
        let (events, failures) = frame(&mut engine, &mut world);
        assert!(failures.is_empty(), "{failures:?}");
        assert!(events.is_empty(), "a param change fires nothing");
        let style = world.get_component(barrel, "ascii_style").unwrap();
        assert_eq!(style.get("cell").and_then(|v| v.as_float()), Some(expect));
        assert_eq!(style.get("levels").and_then(|v| v.as_float()), Some(4.0));
    }

    let (events, _) = frame(&mut engine, &mut world);
    assert!(world.get_component(barrel, "ascii_style").is_none());
    assert!(world.entities_with_component("ascii_style").is_empty());
    assert_eq!(style_changed(&events), (barrel.0 as i64, String::new()));
}

#[test]
fn mistakes_reach_the_log_and_change_nothing() {
    let mut world = FlintWorld::new();
    let barrel = world.spawn("barrel").unwrap();
    let mut engine = ScriptEngine::new();
    director(
        &mut engine,
        &mut world,
        r#"
fn on_update() {
    let barrel = get_entity("barrel");
    set_style_param(barrel, "cell", 12.0);
    set_entity_style(barrel, "kuwahara");
}
"#,
    );
    let (events, failures) = frame(&mut engine, &mut world);
    assert!(events.is_empty());
    assert_eq!(failures.len(), 2, "{failures:?}");
    assert!(
        failures[0].contains("set_entity_style first"),
        "{failures:?}"
    );
    assert!(failures[1].contains("whole-frame"), "{failures:?}");
    assert!(world.get_component(barrel, "ascii_style").is_none());
}
