//! Integration tests: scene loading + script execution cross-crate

use flint_core::EntityId;
use flint_ecs::FlintWorld;
use flint_scene::{load_scene_string, reload_scene_string, save_scene_string};
use flint_schema::SchemaRegistry;
use flint_script::engine::ScriptEngine;

/// Helper: load a scene and attach a script to a named entity
fn load_and_script(
    scene_toml: &str,
    entity_name: &str,
    script_src: &str,
) -> (FlintWorld, ScriptEngine) {
    let registry = SchemaRegistry::new();
    let (mut world, _) = load_scene_string(scene_toml, &registry).unwrap();

    let mut engine = ScriptEngine::new();
    let id: EntityId = world.get_id(entity_name).unwrap();
    let ast = engine.compile(script_src).unwrap();
    engine.add_script(id, ast, "test.rhai".into());
    engine.call_inits(&mut world);

    (world, engine)
}

#[test]
fn test_scene_load_then_script_reads_transform() {
    let scene = r#"
[scene]
name = "Read Test"

[entities.box1]
[entities.box1.transform]
position = [7.0, 8.0, 9.0]

[entities.box1.result]
x = 0.0
"#;

    let script = r#"
fn on_init() {
    let me = self_entity();
    let pos = get_position(me);
    set_field(me, "result", "x", pos.x);
}
"#;

    let (world, _) = load_and_script(scene, "box1", script);

    let id: EntityId = world.get_id("box1").unwrap();
    let x = world
        .get_components(id)
        .unwrap()
        .get_field("result", "x")
        .unwrap()
        .as_float()
        .unwrap();
    assert!(
        (x - 7.0).abs() < 0.01,
        "script should read x=7 from scene, got {}",
        x
    );
}

#[test]
fn test_scene_load_archetype_then_script_modifies() {
    let mut registry = SchemaRegistry::new();
    registry
        .load_archetype_string(
            r#"
[archetype.crate_box]
description = "A crate"
components = ["transform", "health"]

[archetype.crate_box.defaults.health]
current = 100
max = 100
"#,
        )
        .unwrap();

    let scene = r#"
[scene]
name = "Archetype Script Test"

[entities.box1]
archetype = "crate_box"

[entities.box1.transform]
position = [0.0, 0.0, 0.0]

[entities.box1.health]
current = 80
"#;

    let (mut world, _scene_file) = load_scene_string(scene, &registry).unwrap();

    let mut engine = ScriptEngine::new();
    let id: EntityId = world.get_id("box1").unwrap();

    // Verify archetype defaults merged with overrides before script runs
    let hp_before = world
        .get_components(id)
        .unwrap()
        .get_field("health", "current")
        .unwrap()
        .as_integer()
        .unwrap();
    assert_eq!(
        hp_before, 80,
        "entity override should win over archetype default"
    );

    let max_hp = world
        .get_components(id)
        .unwrap()
        .get_field("health", "max")
        .unwrap()
        .as_integer()
        .unwrap();
    assert_eq!(max_hp, 100, "archetype default 'max' should be preserved");

    // Now script modifies health
    let ast = engine
        .compile(
            r#"
fn on_init() {
    let me = self_entity();
    let hp = get_field(me, "health", "current");
    set_field(me, "health", "current", hp - 30);
}
"#,
        )
        .unwrap();

    engine.add_script(id, ast, "test.rhai".into());
    engine.call_inits(&mut world);

    let hp_after = world
        .get_components(id)
        .unwrap()
        .get_field("health", "current")
        .unwrap()
        .as_integer()
        .unwrap();
    assert_eq!(hp_after, 50, "script should reduce health from 80 to 50");

    // max should still be intact
    let max_after = world
        .get_components(id)
        .unwrap()
        .get_field("health", "max")
        .unwrap()
        .as_integer()
        .unwrap();
    assert_eq!(
        max_after, 100,
        "archetype default 'max' should survive script mutation"
    );
}

#[test]
fn test_scene_roundtrip_after_script_mutation() {
    let scene = r#"
[scene]
name = "Roundtrip Mutation"

[entities.mover]
[entities.mover.transform]
position = [0.0, 0.0, 0.0]
"#;

    let script = r#"
fn on_init() {
    let me = self_entity();
    set_position(me, 42.0, 0.0, 0.0);
}
"#;

    let (world, _) = load_and_script(scene, "mover", script);

    // Verify mutation happened
    let id: EntityId = world.get_id("mover").unwrap();
    let t = world.get_transform(id).unwrap();
    assert!((t.position.x - 42.0).abs() < 0.01);

    // Save and reload
    let saved = save_scene_string(&world, "Roundtrip Mutation").unwrap();
    let registry = SchemaRegistry::new();
    let (world2, _) = load_scene_string(&saved, &registry).unwrap();

    let id2: EntityId = world2.get_id("mover").unwrap();
    let t2 = world2.get_transform(id2).unwrap();
    assert!(
        (t2.position.x - 42.0).abs() < 0.01,
        "position should survive save/load after script mutation, got {}",
        t2.position.x
    );
}

#[test]
fn test_multi_entity_script_interaction() {
    let scene = r#"
[scene]
name = "Multi Entity"

[entities.reader]
[entities.reader.result]
target_x = 0.0

[entities.target]
[entities.target.transform]
position = [99.0, 0.0, 0.0]
"#;

    let registry = SchemaRegistry::new();
    let (mut world, _) = load_scene_string(scene, &registry).unwrap();

    let reader_id: EntityId = world.get_id("reader").unwrap();
    let target_id: EntityId = world.get_id("target").unwrap();

    let mut engine = ScriptEngine::new();
    let ast = engine
        .compile(&format!(
            r#"
fn on_init() {{
    let me = self_entity();
    let pos = get_position({target_id});
    set_field(me, "result", "target_x", pos.x);
}}
"#,
            target_id = target_id.raw() as i64,
        ))
        .unwrap();

    engine.add_script(reader_id, ast, "test.rhai".into());
    engine.call_inits(&mut world);

    let x = world
        .get_components(reader_id)
        .unwrap()
        .get_field("result", "target_x")
        .unwrap()
        .as_float()
        .unwrap();
    assert!(
        (x - 99.0).abs() < 0.01,
        "script on entity A should read entity B's position (99), got {}",
        x
    );
}

#[test]
fn test_reload_preserves_engine_functionality() {
    let scene_v1 = r#"
[scene]
name = "V1"

[entities.actor]
[entities.actor.transform]
position = [1.0, 0.0, 0.0]

[entities.actor.state]
value = 0.0
"#;

    let scene_v2 = r#"
[scene]
name = "V2"

[entities.actor]
[entities.actor.transform]
position = [50.0, 0.0, 0.0]

[entities.actor.state]
value = 0.0
"#;

    let registry = SchemaRegistry::new();
    let (mut world, _) = load_scene_string(scene_v1, &registry).unwrap();

    let mut engine = ScriptEngine::new();
    let id: EntityId = world.get_id("actor").unwrap();
    let ast = engine
        .compile(
            r#"
fn on_init() {
    let me = self_entity();
    let pos = get_position(me);
    set_field(me, "state", "value", pos.x);
}
"#,
        )
        .unwrap();

    engine.add_script(id, ast.clone(), "test.rhai".into());
    engine.call_inits(&mut world);

    let v1 = world
        .get_components(id)
        .unwrap()
        .get_field("state", "value")
        .unwrap()
        .as_float()
        .unwrap();
    assert!((v1 - 1.0).abs() < 0.01, "v1 should read x=1");

    // Reload scene — clears world
    reload_scene_string(scene_v2, &mut world, &registry).unwrap();

    // Re-attach script to new entity
    engine.clear();
    let new_id: EntityId = world.get_id("actor").unwrap();
    engine.add_script(new_id, ast, "test.rhai".into());
    engine.call_inits(&mut world);

    let v2 = world
        .get_components(new_id)
        .unwrap()
        .get_field("state", "value")
        .unwrap()
        .as_float()
        .unwrap();
    assert!(
        (v2 - 50.0).abs() < 0.01,
        "after reload, script should read x=50, got {}",
        v2
    );
}

#[test]
fn test_parent_chain_world_position_from_script() {
    let scene = r#"
[scene]
name = "Hierarchy Script"

[entities.root]
[entities.root.transform]
position = [10.0, 0.0, 0.0]

[entities.mid]
parent = "root"
[entities.mid.transform]
position = [5.0, 0.0, 0.0]

[entities.leaf]
parent = "mid"
[entities.leaf.transform]
position = [3.0, 0.0, 0.0]

[entities.leaf.result]
wx = 0.0
"#;

    let script = r#"
fn on_init() {
    let me = self_entity();
    let wp = get_world_position(me);
    set_field(me, "result", "wx", wp.x);
}
"#;

    let (world, _) = load_and_script(scene, "leaf", script);

    let leaf: EntityId = world.get_id("leaf").unwrap();
    let wx = world
        .get_components(leaf)
        .unwrap()
        .get_field("result", "wx")
        .unwrap()
        .as_float()
        .unwrap();
    assert!(
        (wx - 18.0).abs() < 0.01,
        "leaf world x should be 10+5+3=18, got {}",
        wx
    );
}
