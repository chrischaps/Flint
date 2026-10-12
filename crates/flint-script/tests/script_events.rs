//! Integration tests: custom game events reaching scripts' `on_event(name, data)`

use flint_core::EntityId;
use flint_ecs::FlintWorld;
use flint_runtime::GameEvent;
use flint_script::context::ScriptCommand;
use flint_script::engine::ScriptEngine;
use std::sync::{Arc, Mutex};

type Log = Arc<Mutex<Vec<String>>>;

/// A script engine with `record(s)` registered, which appends to the log.
fn engine_with_log() -> (ScriptEngine, Log) {
    let log = Log::default();
    let mut engine = ScriptEngine::new();
    let sink = log.clone();
    engine
        .rhai_engine_mut()
        .register_fn("record", move |s: &str| sink.lock().unwrap().push(s.into()));
    (engine, log)
}

fn attach(engine: &mut ScriptEngine, world: &mut FlintWorld, name: &str, src: &str) -> EntityId {
    let id = world.spawn(name).unwrap();
    let ast = engine.compile(src).unwrap();
    engine.add_script(id, ast, format!("{name}.rhai"));
    id
}

fn custom(name: &str, data: toml::Value) -> GameEvent {
    GameEvent::Custom {
        name: name.into(),
        data,
    }
}

/// The player turns each script `FireEvent` command into a `GameEvent::Custom`
/// for the next frame; do the same here.
fn fired_events(engine: &ScriptEngine) -> Vec<GameEvent> {
    engine
        .drain_commands()
        .into_iter()
        .filter_map(|cmd| match cmd {
            ScriptCommand::FireEvent { name, data } => Some(custom(&name, data)),
            _ => None,
        })
        .collect()
}

const LISTENER: &str = r#"
fn on_event(name, data) {
    let keys = data.keys();
    keys.sort();
    let parts = [];
    for k in keys { parts.push(`${k}=${data[k]}`); }
    record(`${entity_name(self_entity())}:${name}{${parts}}`);
}
"#;

#[test]
fn on_event_receives_engine_events_with_map_data() {
    let (mut engine, log) = engine_with_log();
    let mut world = FlintWorld::new();
    attach(&mut engine, &mut world, "listener", LISTENER);
    engine.call_inits(&mut world);

    let data: toml::Table = toml::from_str("plot = 12\nowner = \"ada\"").unwrap();
    engine.process_events(
        &[
            custom("plot_claimed", toml::Value::Table(data)),
            custom("tick", toml::Value::Table(toml::Table::new())),
        ],
        &mut world,
    );

    assert_eq!(
        *log.lock().unwrap(),
        [
            r#"listener:plot_claimed{["owner=ada", "plot=12"]}"#,
            "listener:tick{[]}",
        ]
    );
}

#[test]
fn non_table_payload_arrives_wrapped_as_value() {
    let (mut engine, log) = engine_with_log();
    let mut world = FlintWorld::new();
    attach(&mut engine, &mut world, "listener", LISTENER);
    engine.call_inits(&mut world);

    engine.process_events(&[custom("strands", toml::Value::Integer(250))], &mut world);

    assert_eq!(*log.lock().unwrap(), [r#"listener:strands{["value=250"]}"#]);
}

#[test]
fn script_fired_events_reach_every_listener_including_the_sender() {
    let (mut engine, log) = engine_with_log();
    let mut world = FlintWorld::new();
    attach(
        &mut engine,
        &mut world,
        "bell",
        r#"
fn on_update() {
    fire_event_data("rang", #{ times: 3, loud: true });
    fire_event("quiet");
}
fn on_event(name, data) { record(`bell heard ${name}`); }
"#,
    );
    attach(&mut engine, &mut world, "listener", LISTENER);
    engine.call_inits(&mut world);

    engine.call_updates(&mut world);
    let events = fired_events(&engine);
    assert_eq!(events.len(), 2);
    engine.process_events(&events, &mut world);

    // Entity-id order: bell was spawned first.
    assert_eq!(
        *log.lock().unwrap(),
        [
            "bell heard rang",
            r#"listener:rang{["loud=true", "times=3"]}"#,
            "bell heard quiet",
            "listener:quiet{[]}",
        ]
    );
}

#[test]
fn scripts_without_on_event_are_unaffected() {
    let (mut engine, log) = engine_with_log();
    let mut world = FlintWorld::new();
    attach(
        &mut engine,
        &mut world,
        "plain",
        r#"
fn on_action(a) { record(`plain action ${a}`); }
fn on_update() { record("plain update"); }
"#,
    );
    engine.call_inits(&mut world);

    engine.process_events(
        &[
            custom("anything", toml::Value::Table(toml::Table::new())),
            GameEvent::ActionPressed("jump".into()),
        ],
        &mut world,
    );
    engine.call_updates(&mut world);

    assert_eq!(*log.lock().unwrap(), ["plain action jump", "plain update"]);
}

#[test]
fn on_event_errors_do_not_stop_other_listeners() {
    let (mut engine, log) = engine_with_log();
    let mut world = FlintWorld::new();
    attach(
        &mut engine,
        &mut world,
        "broken",
        "fn on_event(name, data) { throw `broken on ${name}`; }",
    );
    attach(&mut engine, &mut world, "listener", LISTENER);
    engine.call_inits(&mut world);

    engine.process_events(
        &[custom("ping", toml::Value::Table(toml::Table::new()))],
        &mut world,
    );

    assert_eq!(*log.lock().unwrap(), ["listener:ping{[]}"]);
}
