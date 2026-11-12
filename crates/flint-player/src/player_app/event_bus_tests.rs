//! #727: the player owns the frame's `EventBus`. Physics pushes its contacts
//! into it at each fixed step; a script's `fire_event` goes to it directly,
//! so physics' `clear` (a scene transition) no longer loses one. Both reach
//! the scripts in the same frame's batch.
//!
//! The project is a `MemorySource`; no window, no GPU.

use super::scene_entry::SceneEntry;
use super::PlayerApp;
use flint_core::project::{SchemaDirs, SchemaSource};
use flint_core::vfs::{self, MemorySource};
use flint_runtime::GameEvent;
use flint_script::ScriptHost;
use std::sync::Arc;

/// On a ball that falls onto a floor: fires `ping` every frame, and notes a
/// frame whose batch held both its collision and a ping.
const BALL: &str = r#"
let frame = 0;
let hit = -1;
let ping = -1;
fn on_collision(other) { hit = frame; }
fn on_event(name, data) {
    if name == "ping" { ping = frame; }
    if name == "kept" { persist_set("kept", data.value); }
}
fn on_update() {
    if hit == frame && ping == frame && hit >= 0 { persist_set("both_at", frame); }
    persist_set("pings", ping);
    frame += 1;
    fire_event("ping");
}
"#;

/// Only hears events.
const LISTENER: &str = r#"
fn on_event(name, data) {
    if name == "kept" { persist_set("kept_in_b", data.value); }
}
"#;

const DROP: &str = r#"[scene]
name = "Drop"

[entities.floor.transform]
position = [0.0, -0.5, 0.0]
[entities.floor.rigidbody]
body_type = "static"
[entities.floor.collider]
shape = "box"
size = [20.0, 1.0, 20.0]

[entities.ball.transform]
position = [0.0, 1.5, 0.0]
[entities.ball.rigidbody]
body_type = "dynamic"
[entities.ball.collider]
shape = "sphere"
size = [1.0, 1.0, 1.0]
[entities.ball.script]
source = "ball.rhai"
"#;

const B: &str = r#"[scene]
name = "B"

[entities.listener.script]
source = "listener.rhai"
"#;

fn game() -> MemorySource {
    let mut files = MemorySource::new();
    files.insert("game/flint.toml", "");
    files.insert("game/scenes/drop.scene.toml", DROP);
    files.insert("game/scenes/b.scene.toml", B);
    files.insert("game/scripts/ball.rhai", BALL);
    files.insert("game/scripts/listener.rhai", LISTENER);
    files
}

/// Started in `path`, saving (the scripts' `persist_set`) to a folder of
/// the test's own under the temp dir, never into the crate.
fn started_in(path: &str, test: &str) -> PlayerApp {
    let registry = flint_schema::SchemaRegistry::new();
    let (world, file) = flint_scene::load_scene(path, &registry).expect("the scene loads");
    let mut app = PlayerApp::new(world, path.into(), false, None, None);
    app.apply_scene_settings(&file);
    app.set_schemas(
        registry,
        SchemaDirs {
            dirs: Vec::new(),
            source: SchemaSource::Flags,
            dropped: Vec::new(),
        },
    );
    app.core.clock.frame_step = Some(1.0 / 30.0);
    app.launch.save_dir =
        Some(std::env::temp_dir().join(format!("flint_727_{test}_{}", std::process::id())));
    app.enter_scene(SceneEntry::Start)
        .expect("the scene starts");
    app
}

fn stored(app: &PlayerApp, key: &str) -> Option<i64> {
    app.core.store.get(key).and_then(|v| v.as_integer())
}

#[test]
fn a_collision_and_a_scripts_event_reach_the_scripts_in_one_frame() {
    vfs::scoped(Arc::new(game()), || {
        let mut app = started_in("game/scenes/drop.scene.toml", "batch");
        for _ in 0..90 {
            app.tick();
        }
        assert!(
            stored(&app, "pings").is_some_and(|p| p > 0),
            "the script's own event came back to it"
        );
        assert!(
            stored(&app, "both_at").is_some(),
            "a frame's batch held the ball's collision and a ping"
        );
    });
}

#[test]
fn physics_clear_leaves_a_scripts_event_on_the_bus() {
    vfs::scoped(Arc::new(game()), || {
        let mut app = started_in("game/scenes/drop.scene.toml", "clear");
        app.tick();
        app.fire_event("kept".into(), toml::Value::Integer(7));
        // What a transition does to physics; it used to drop the event
        app.core.physics.clear();
        app.tick();
        assert_eq!(stored(&app, "kept"), Some(7));
    });
}

#[test]
fn a_scripts_event_crosses_a_transition_and_the_old_scenes_contacts_do_not() {
    vfs::scoped(Arc::new(game()), || {
        let mut app = started_in("game/scenes/drop.scene.toml", "transition");
        app.tick();
        let ball = app.core.world.get_id("ball").expect("the ball");
        let floor = app.core.world.get_id("floor").expect("the floor");
        app.core.events.push(GameEvent::CollisionStarted {
            entity_a: ball,
            entity_b: floor,
        });
        app.fire_event("kept".into(), toml::Value::Integer(9));

        assert!(app.execute_scene_transition("b.scene.toml"));
        let left: Vec<_> = app.core.events.drain();
        assert!(
            left.iter().all(|e| !e.is_contact()),
            "the old scene's contacts went with it: {left:?}"
        );
        // Put back what was kept, as the next frame would find it
        for event in left {
            app.core.events.push(event);
        }
        app.tick();
        assert_eq!(stored(&app, "kept_in_b"), Some(9));
    });
}
