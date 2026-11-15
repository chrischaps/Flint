//! What a script says and how long it takes (#703): `print` and `debug` go
//! to the log under `script`, naming the file and entity; every callback is
//! timed on its own.

use super::*;
use flint_ecs::FlintWorld;
use flint_runtime::GameEvent;
use std::sync::{Arc, Mutex};
use test_support::scripted;

#[derive(Clone, Default)]
struct Captured(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for Captured {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// What `run` logged, every level.
fn logged(run: impl FnOnce()) -> String {
    let log = Captured::default();
    let writer = log.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(move || writer.clone())
        .with_max_level(tracing::Level::TRACE)
        .with_ansi(false)
        .finish();
    tracing::subscriber::with_default(subscriber, run);
    let text = String::from_utf8(log.0.lock().unwrap().clone()).unwrap();
    text
}

const TALKER: &str = r#"
print("awake");
fn on_update() { print("tick " + 1); }
fn on_draw_ui() { debug("drawing"); }
fn on_event(name, data) { print("heard " + name); }
"#;

/// `TALKER` on an entity named `talker`, from `scripts/talker.rhai`.
fn talker(engine: &mut ScriptEngine, world: &mut FlintWorld) -> flint_core::EntityId {
    let id = scripted(world, "talker", "talker.rhai");
    let ast = engine.compile(TALKER).unwrap();
    engine.add_named_script(
        id,
        ast,
        "scripts/talker.rhai".into(),
        "talker",
        Some("scripts/talker.rhai"),
    );
    id
}

#[test]
fn print_and_debug_go_to_the_log_naming_the_script() {
    let mut engine = ScriptEngine::new();
    let mut world = FlintWorld::new();
    talker(&mut engine, &mut world);
    let log = logged(|| {
        engine.call_updates(&mut world);
        engine.call_draw_uis(&mut world);
    });
    let line = |text: &str| {
        log.lines()
            .find(|l| l.contains(text))
            .unwrap_or_else(|| panic!("no {text:?} in:\n{log}"))
            .to_string()
    };
    for text in ["awake", "tick 1"] {
        let l = line(text);
        assert!(l.contains("INFO script:"), "{l}");
        assert!(l.contains("file=scripts/talker.rhai"), "{l}");
        assert!(l.contains("entity=talker"), "{l}");
    }
    // debug quotes a string, as Rhai's own did, and says where
    let l = line("\"drawing\"");
    assert!(l.contains("line=4"), "{l}");
    assert!(l.contains("entity=talker"), "{l}");
}

#[test]
fn a_renamed_script_prints_its_new_name() {
    let mut engine = ScriptEngine::new();
    let mut world = FlintWorld::new();
    let id = talker(&mut engine, &mut world);
    engine.call_updates(&mut world);
    engine.name_script(id, "chatterbox");
    let log = logged(|| engine.call_updates(&mut world));
    assert!(log.contains("entity=chatterbox"), "{log}");
}

#[test]
fn every_callback_is_timed_under_its_file_and_name() {
    let mut engine = ScriptEngine::new();
    let mut world = FlintWorld::new();
    talker(&mut engine, &mut world);
    assert!(engine.take_times().is_empty(), "nothing until timing is on");
    engine.set_timing(true);
    logged(|| {
        engine.call_updates(&mut world);
        engine.call_draw_uis(&mut world);
        engine.process_events(
            &[GameEvent::Custom {
                name: "bell".into(),
                data: toml::Value::Boolean(true),
            }],
            &mut world,
        );
    });
    let labels: Vec<String> = engine.take_times().into_iter().map(|(l, _)| l).collect();
    assert_eq!(
        labels,
        [
            "scripts/talker.rhai:on_update",
            "scripts/talker.rhai:on_draw_ui",
            "scripts/talker.rhai:on_event",
        ]
    );
    assert!(engine.take_times().is_empty(), "a take empties them");
}
