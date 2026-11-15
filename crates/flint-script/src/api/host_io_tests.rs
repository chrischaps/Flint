//! The modules that read from the host or ask it for something: input
//! (keys, the wheel, touch and the safe area), audio commands, animation,
//! the camera's requests, and persistence, states and physics with
//! nothing lent.

use crate::context::{InputSnapshot, ScriptCommand};
use crate::engine::test_support::*;
use crate::engine::ScriptEngine;
use flint_ecs::FlintWorld;

#[test]
fn test_mouse_wheel_reaches_scripts() {
    let mut engine = ScriptEngine::new();
    let mut world = FlintWorld::new();
    let id = world.spawn("wheel").unwrap();
    probe_component(&mut world, id);
    let ast = engine
        .compile(
            r#"
        fn on_update() {
            let me = self_entity();
            set_field(me, "probe", "wx", mouse_wheel_x());
            set_field(me, "probe", "wy", mouse_wheel_y());
        }
    "#,
        )
        .unwrap();
    engine.add_script(id, ast, "wheel.rhai".into());
    let input = InputSnapshot {
        mouse_wheel: (-0.5, 2.0),
        ..Default::default()
    };
    engine.provide_context(input, 0.016, 0.0);
    engine.call_updates(&mut world);
    assert_eq!(probe_float(&world, id, "wx"), -0.5);
    assert_eq!(probe_float(&world, id, "wy"), 2.0);
}

#[test]
fn safe_area_and_touch_device_reach_scripts() {
    // #660: a HUD keeps out of the notch, and knows a phone before the
    // first finger lands.
    let mut engine = ScriptEngine::new();
    let mut world = FlintWorld::new();
    let id = world.spawn("safe").unwrap();
    probe_component(&mut world, id);
    let ast = engine
        .compile(
            r#"
        fn on_update() {
            let me = self_entity();
            let s = safe_area();
            set_field(me, "probe", "l", s.left);
            set_field(me, "probe", "t", s.top);
            set_field(me, "probe", "r", s.right);
            set_field(me, "probe", "b", s.bottom);
            set_field(me, "probe", "touch", if is_touch_device() { 1.0 } else { 0.0 });
            set_field(me, "probe", "last", if last_input_device() == "touch" { 1.0 } else { 0.0 });
        }
    "#,
        )
        .unwrap();
    engine.add_script(id, ast, "safe.rhai".into());

    engine.provide_context(InputSnapshot::default(), 0.016, 0.0);
    engine.call_updates(&mut world);
    for k in ["l", "t", "r", "b", "touch"] {
        assert_eq!(probe_float(&world, id, k), 0.0, "{k} on a desktop");
    }

    let input = InputSnapshot {
        touch_device: true,
        safe_area: [47.0, 0.0, 47.0, 21.0],
        ..Default::default()
    };
    engine.provide_context(input, 0.016, 0.016);
    engine.call_updates(&mut world);
    assert_eq!(probe_float(&world, id, "l"), 47.0);
    assert_eq!(probe_float(&world, id, "t"), 0.0);
    assert_eq!(probe_float(&world, id, "r"), 47.0);
    assert_eq!(probe_float(&world, id, "b"), 21.0);
    assert_eq!(probe_float(&world, id, "touch"), 1.0);
    assert_eq!(probe_float(&world, id, "last"), 0.0, "nothing touched yet");
}

#[test]
fn test_camera_roll_override_round_trip() {
    let mut engine = ScriptEngine::new();
    let mut world = FlintWorld::new();
    let id = world.spawn("roller").unwrap();

    // Not set until a script calls the API.
    assert_eq!(engine.ctx.lock().unwrap().requests.camera.roll, None);

    let ast = engine
        .compile("fn on_update() { set_camera_roll(0.25); }")
        .unwrap();
    engine.add_script(id, ast, "roller.rhai".into());
    engine.provide_context(InputSnapshot::default(), 0.016, 0.0);
    engine.call_updates(&mut world);

    // Set after the frame, and take() clears it (one-frame override).
    assert_eq!(
        engine.ctx.lock().unwrap().requests.camera.roll.take(),
        Some(0.25)
    );
    assert_eq!(engine.ctx.lock().unwrap().requests.camera.roll, None);
}

#[test]
fn test_input_api() {
    let mut engine = ScriptEngine::new();
    let mut world = FlintWorld::new();
    let id = world.spawn("input_test").unwrap();

    world
        .set_component(
            id,
            "result",
            toml::Value::Table({
                let mut m = toml::map::Map::new();
                m.insert("jumped".into(), toml::Value::Boolean(false));
                m
            }),
        )
        .unwrap();

    let ast = engine
        .compile(
            r#"
        fn on_update() {
            let me = self_entity();
            if is_action_just_pressed("jump") {
                set_field(me, "result", "jumped", true);
            }
        }
    "#,
        )
        .unwrap();

    engine.add_script(id, ast, "test.rhai".into());

    let mut input = InputSnapshot::default();
    input.actions_just_pressed.insert("jump".into());
    engine.provide_context(input, 0.016, 0.0);
    engine.call_updates(&mut world);

    let jumped = world
        .get_components(id)
        .unwrap()
        .get_field("result", "jumped")
        .unwrap()
        .as_bool()
        .unwrap();
    assert!(jumped);
}

#[test]
fn test_play_sound_command() {
    let mut engine = ScriptEngine::new();
    let mut world = FlintWorld::new();
    let id = world.spawn("snd_test").unwrap();

    let ast = engine
        .compile(
            r#"
        fn on_init() {
            play_sound("bang.ogg");
        }
    "#,
        )
        .unwrap();

    engine.add_script(id, ast, "test.rhai".into());
    engine.call_inits(&mut world);

    let commands = engine.drain_commands();
    assert_eq!(commands.len(), 1);
    match &commands[0] {
        ScriptCommand::PlaySound {
            name,
            volume,
            pitch,
        } => {
            assert_eq!(name, "bang.ogg");
            assert!((volume - 1.0).abs() < 1e-10);
            assert!((pitch - 1.0).abs() < 1e-10);
        }
        _ => panic!("Expected PlaySound command"),
    }
}

/// `rumble()` asks the host for a buzz, with no music session anywhere
/// (#748): ints or floats, every pad or one, and a zero that stops.
#[test]
fn rumble_queues_a_command_for_the_host() {
    let mut engine = ScriptEngine::new();
    let mut world = FlintWorld::new();
    let id = world.spawn("buzz").unwrap();
    let ast = engine
        .compile(
            r#"
        fn on_init() {
            rumble(-1, 0.6, 0.25);
            rumble(0, 1, 2);
            rumble(-1, 0, 0);
        }
    "#,
        )
        .unwrap();
    engine.add_script(id, ast, "buzz.rhai".into());
    engine.call_inits(&mut world);

    let rumbles: Vec<_> = engine
        .drain_commands()
        .into_iter()
        .filter_map(|c| match c {
            ScriptCommand::Rumble {
                pad,
                strength,
                seconds,
            } => Some((pad, strength, seconds)),
            _ => None,
        })
        .collect();
    assert_eq!(rumbles, [(-1, 0.6, 0.25), (0, 1.0, 2.0), (-1, 0.0, 0.0)]);
}

#[test]
fn test_play_sound_pitch_and_falloff() {
    let mut engine = ScriptEngine::new();
    let mut world = FlintWorld::new();
    let id = world.spawn("snd_opts").unwrap();

    let ast = engine
        .compile(
            r#"
        fn on_init() {
            play_sound("click", 0.5, 1.2);
            play_sound_at("bell", 1.0, 2.0, 3.0, #{ volume: 0.8, max_distance: 400, rolloff: 0.7 });
            play_sound_at("tap", 0.0, 0.0, 0.0, #{});
        }
    "#,
        )
        .unwrap();

    engine.add_script(id, ast, "test.rhai".into());
    engine.call_inits(&mut world);

    let commands = engine.drain_commands();
    assert_eq!(commands.len(), 3);
    match &commands[0] {
        ScriptCommand::PlaySound { volume, pitch, .. } => {
            assert!((volume - 0.5).abs() < 1e-10);
            assert!((pitch - 1.2).abs() < 1e-6);
        }
        other => panic!("Expected PlaySound, got {other:?}"),
    }
    match &commands[1] {
        ScriptCommand::PlaySoundAt {
            name,
            position,
            volume,
            pitch,
            distances,
            rolloff,
        } => {
            assert_eq!(name, "bell");
            assert_eq!(*position, (1.0, 2.0, 3.0));
            assert!((volume - 0.8).abs() < 1e-6);
            assert!((pitch - 1.0).abs() < 1e-10);
            assert_eq!(*distances, Some((1.0, 400.0)));
            assert_eq!(*rolloff, Some(0.7));
        }
        other => panic!("Expected PlaySoundAt, got {other:?}"),
    }
    match &commands[2] {
        ScriptCommand::PlaySoundAt {
            volume,
            distances,
            rolloff,
            ..
        } => {
            assert!((volume - 1.0).abs() < 1e-10);
            assert_eq!(*distances, None);
            assert_eq!(*rolloff, None);
        }
        other => panic!("Expected PlaySoundAt, got {other:?}"),
    }
}

#[test]
fn test_animation_api_play_clip() {
    let mut engine = ScriptEngine::new();
    let mut world = FlintWorld::new();
    let id = world.spawn("anim_test").unwrap();

    // Set up an animator component
    world
        .set_component(
            id,
            "animator",
            toml::Value::Table({
                let mut m = toml::map::Map::new();
                m.insert("clip".into(), toml::Value::String("idle".into()));
                m.insert("playing".into(), toml::Value::Boolean(false));
                m
            }),
        )
        .unwrap();

    let ast = engine
        .compile(
            r#"
        fn on_init() {
            let me = self_entity();
            play_clip(me, "run");
        }
    "#,
        )
        .unwrap();

    engine.add_script(id, ast, "test.rhai".into());
    engine.call_inits(&mut world);

    let clip = world
        .get_components(id)
        .unwrap()
        .get_field("animator", "clip")
        .unwrap()
        .as_str()
        .unwrap()
        .to_string();
    let playing = world
        .get_components(id)
        .unwrap()
        .get_field("animator", "playing")
        .unwrap()
        .as_bool()
        .unwrap();
    assert_eq!(clip, "run");
    assert!(playing);
}

#[test]
fn test_persist_api_without_store() {
    // persist_set with null store should not crash, just silently fail
    let mut engine = ScriptEngine::new();
    let mut world = FlintWorld::new();
    let id = world.spawn("persist_test").unwrap();

    let ast = engine
        .compile(
            r#"
        fn on_init() {
            // persistent_store pointer is null — these should not crash
            persist_set("key", 42);
            let val = persist_get("key");
            let has = persist_has("key");
            // #726: slots, version and the corrupt report have answers
            // with no store too
            if persist_slot() != "default" { throw "slot"; }
            if persist_slot("b") { throw "switched with no store"; }
            if persist_slots().len() != 0 { throw "slots"; }
            if persist_version() != 0 { throw "version"; }
            persist_set_version(2);
            if persist_corrupt() != () { throw "corrupt"; }
            set_field(self_entity(), "probe", "age", 1.0);
        }
        fn on_persist_migrate(from) { throw "no store, nothing to migrate"; }
    "#,
        )
        .unwrap();

    probe_component(&mut world, id);
    engine.add_script(id, ast, "test.rhai".into());
    assert!(engine.scripts[&id].has_on_persist_migrate);
    // Should not panic, and with no store in scope migrates nothing
    engine.migrate_persistent_store(&mut world);
    engine.call_inits(&mut world);
    assert_eq!(probe_float(&world, id, "age"), 1.0, "on_init ran through");
}

#[test]
fn test_state_api_without_state_machine() {
    // current_state with null state_machine should return "playing" fallback
    let mut engine = ScriptEngine::new();
    let mut world = FlintWorld::new();
    let id = world.spawn("state_test").unwrap();

    world
        .set_component(
            id,
            "result",
            toml::Value::Table({
                let mut m = toml::map::Map::new();
                m.insert("state".into(), toml::Value::String(String::new()));
                m
            }),
        )
        .unwrap();

    let ast = engine
        .compile(
            r#"
        fn on_init() {
            let me = self_entity();
            let state = current_state();
            set_field(me, "result", "state", state);
        }
    "#,
        )
        .unwrap();

    engine.add_script(id, ast, "test.rhai".into());
    engine.call_inits(&mut world);

    let state = world
        .get_components(id)
        .unwrap()
        .get_field("result", "state")
        .unwrap()
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(state, "playing", "fallback state should be 'playing'");
}

#[test]
fn test_physics_api_without_physics() {
    // raycast with null physics should return unit, not crash
    let mut engine = ScriptEngine::new();
    let mut world = FlintWorld::new();
    let id = world.spawn("physics_test").unwrap();

    let ast = engine
        .compile(
            r#"
        fn on_init() {
            // physics pointer is null — should return () without crashing
            let hit = raycast(0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 100.0);
            if type_of(hit) != "()" { throw "a hit with no physics"; }
            set_field(self_entity(), "probe", "age", 1.0);
        }
    "#,
        )
        .unwrap();

    probe_component(&mut world, id);
    engine.add_script(id, ast, "test.rhai".into());
    // Should not panic, and the call is found and answers ()
    engine.call_inits(&mut world);
    assert_eq!(probe_float(&world, id, "age"), 1.0, "on_init ran through");
}
