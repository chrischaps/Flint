//! A script's callbacks against the one table (#700): a misspelt callback
//! is a load warning naming the one it was nearest, and a callback at a
//! count the engine doesn't call with is left out, never called.

use super::*;
use flint_core::callbacks::Callback;
use flint_ecs::FlintWorld;
use std::sync::{Arc, Mutex};
use test_support::{probe_float, scripted};

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

/// Load `source` as `cb.rhai` on an entity of `world`, returning its id and
/// what the load logged.
fn load(engine: &mut ScriptEngine, world: &mut FlintWorld, source: &str) -> (EntityId, String) {
    let id = scripted(world, "subject", "cb.rhai");
    let ast = engine.compile(source).unwrap();
    let log = Captured::default();
    let writer = log.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(move || writer.clone())
        .with_ansi(false)
        .finish();
    tracing::subscriber::with_default(subscriber, || {
        engine.add_script(id, ast, "cb.rhai".into());
    });
    let text = String::from_utf8(log.0.lock().unwrap().clone()).unwrap();
    (id, text)
}

#[test]
fn a_misspelt_callback_is_a_load_warning_naming_the_nearest() {
    let mut engine = ScriptEngine::new();
    let mut world = FlintWorld::new();
    let (id, log) = load(
        &mut engine,
        &mut world,
        "fn on_updte() {}\nfn on_colision(other) {}\nfn onInit() {}",
    );
    assert!(
        log.contains("cb.rhai): `fn on_updte` is not a callback, and nothing in this script calls it (did you mean `on_update`?)"),
        "{log}"
    );
    assert!(log.contains("(did you mean `on_collision`?)"), "{log}");
    assert!(
        log.contains("`fn onInit`") && log.contains("(did you mean `on_init`?)"),
        "{log}"
    );
    assert!(Callback::ALL
        .into_iter()
        .all(|c| !engine.scripts[&id].defines(c)));
}

#[test]
fn a_helper_named_like_a_callback_is_left_alone() {
    let mut engine = ScriptEngine::new();
    let mut world = FlintWorld::new();
    // Called, called by name, a function pointer: each a helper, not a typo.
    // `on_spawn` is no callback's typo at all.
    let (_, log) = load(
        &mut engine,
        &mut world,
        r#"
        fn on_inits() {}
        fn on_updat() {}
        fn on_actions(a) {}
        fn on_spawn() {}
        fn on_init() { on_inits(); call("on_updat"); let f = Fn("on_actions"); f.call("x"); }
        "#,
    );
    assert!(!log.contains("not a callback"), "{log}");
}

#[test]
fn a_cue_callback_at_the_wrong_count_warns_and_is_never_called() {
    let mut engine = ScriptEngine::new();
    let mut world = FlintWorld::new();
    let (id, log) = load(
        &mut engine,
        &mut world,
        r#"fn on_sequence_cue(seq) { set_field(self_entity(), "probe", "lx", 1.0); }"#,
    );
    assert!(
        log.contains("`fn on_sequence_cue` has 1 parameter; the engine calls it with 2, so it never runs (`fn on_sequence_cue(sequence, cue)`)"),
        "{log}"
    );
    assert!(!engine.scripts[&id].defines(Callback::SequenceCue));
    let cue = flint_animation::SequenceCueEvent {
        entity_id: id,
        sequence: "door".into(),
        cue: "open".into(),
        time: 0.0,
    };
    engine.call_sequence_cues(&mut world, std::slice::from_ref(&cue));
    assert_eq!(probe_float(&world, id, "lx"), -1.0, "not called");

    // At the right count it is called, with both arguments
    let mut engine = ScriptEngine::new();
    let mut world = FlintWorld::new();
    let (id, log) = load(
        &mut engine,
        &mut world,
        r#"fn on_sequence_cue(seq, cue) {
            if seq == "door" && cue == "open" { set_field(self_entity(), "probe", "lx", 2.0); }
        }"#,
    );
    assert!(!log.contains("warning"), "{log}");
    let cue = flint_animation::SequenceCueEvent {
        entity_id: id,
        ..cue
    };
    engine.call_sequence_cues(&mut world, std::slice::from_ref(&cue));
    assert_eq!(probe_float(&world, id, "lx"), 2.0);
}

#[test]
fn an_update_with_a_parameter_is_not_called_every_frame() {
    let mut engine = ScriptEngine::new();
    let mut world = FlintWorld::new();
    let (id, log) = load(
        &mut engine,
        &mut world,
        r#"fn on_update(dt) { set_field(self_entity(), "probe", "ty", 1.0); }"#,
    );
    assert!(log.contains("delta_time() is the frame's step"), "{log}");
    engine.call_updates(&mut world);
    assert_eq!(probe_float(&world, id, "ty"), -1.0);
}

#[test]
fn an_overload_at_the_right_count_is_the_callback() {
    let mut engine = ScriptEngine::new();
    let mut world = FlintWorld::new();
    let (id, log) = load(
        &mut engine,
        &mut world,
        r#"
        fn on_update(dt) { set_field(self_entity(), "probe", "ty", dt); }
        fn on_update() { on_update(3.0); }
        "#,
    );
    assert!(!log.contains("warning"), "{log}");
    engine.call_updates(&mut world);
    assert_eq!(probe_float(&world, id, "ty"), 3.0);
}

#[test]
fn a_reload_fixes_or_breaks_a_callback() {
    let mut engine = ScriptEngine::new();
    let mut world = FlintWorld::new();
    let (id, _) = load(&mut engine, &mut world, "fn on_updte() {}");
    assert!(!engine.scripts[&id].defines(Callback::Update));
    let fixed = engine.compile("fn on_update() {}").unwrap();
    engine.scripts.get_mut(&id).unwrap().hot_reload(fixed);
    assert!(engine.scripts[&id].defines(Callback::Update));
    let broken = engine.compile("fn on_update(dt) {}").unwrap();
    engine.scripts.get_mut(&id).unwrap().hot_reload(broken);
    assert!(!engine.scripts[&id].defines(Callback::Update));
}
