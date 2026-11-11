//! The `entity` module from a script: transforms, rotation, shear, joints,
//! materials, scale and visibility, spawning, the hierarchy, queries, and
//! ids that name nothing.

use crate::engine::ScriptEngine;
use flint_core::EntityId;
use flint_ecs::FlintWorld;

#[test]
fn test_position_get_set() {
    let mut engine = ScriptEngine::new();
    let mut world = FlintWorld::new();
    let id = world.spawn("pos_test").unwrap();

    // Set up transform
    world
        .set_component(
            id,
            "transform",
            toml::Value::Table({
                let mut m = toml::map::Map::new();
                m.insert(
                    "position".into(),
                    toml::Value::Array(vec![
                        toml::Value::Float(1.0),
                        toml::Value::Float(2.0),
                        toml::Value::Float(3.0),
                    ]),
                );
                m
            }),
        )
        .unwrap();

    let ast = engine
        .compile(
            r#"
        fn on_init() {
            let me = self_entity();
            let pos = get_position(me);
            set_position(me, pos.x + 10.0, pos.y, pos.z);
        }
    "#,
        )
        .unwrap();

    engine.add_script(id, ast, "test.rhai".into());
    engine.call_inits(&mut world);

    let transform = world.get_transform(id).unwrap();
    assert!((transform.position.x - 11.0).abs() < 0.01);
    assert!((transform.position.y - 2.0).abs() < 0.01);
}

#[test]
fn test_spawn_entity_from_script() {
    let mut engine = ScriptEngine::new();
    let mut world = FlintWorld::new();
    let id = world.spawn("spawner").unwrap();

    let ast = engine
        .compile(
            r#"
        fn on_init() {
            spawn_entity("spawned_child");
        }
    "#,
        )
        .unwrap();

    engine.add_script(id, ast, "test.rhai".into());
    engine.call_inits(&mut world);

    assert!(
        world.contains_name("spawned_child"),
        "entity spawned from script should exist"
    );
}

#[test]
fn test_despawn_entity_from_script() {
    let mut engine = ScriptEngine::new();
    let mut world = FlintWorld::new();
    let id = world.spawn("controller").unwrap();
    let target = world.spawn("target_to_kill").unwrap();

    let ast = engine
        .compile(&format!(
            r#"
        fn on_init() {{
            despawn_entity({});
        }}
    "#,
            target.raw() as i64
        ))
        .unwrap();

    engine.add_script(id, ast, "test.rhai".into());
    engine.call_inits(&mut world);

    assert!(
        !world.contains_name("target_to_kill"),
        "despawned entity should be gone"
    );
}

#[test]
fn test_set_rotation_from_script() {
    let mut engine = ScriptEngine::new();
    let mut world = FlintWorld::new();
    let id = world.spawn("rotator").unwrap();

    world
        .set_component(
            id,
            "transform",
            toml::Value::Table({
                let mut m = toml::map::Map::new();
                m.insert(
                    "position".into(),
                    toml::Value::Array(vec![
                        toml::Value::Float(0.0),
                        toml::Value::Float(0.0),
                        toml::Value::Float(0.0),
                    ]),
                );
                m.insert(
                    "rotation".into(),
                    toml::Value::Array(vec![
                        toml::Value::Float(0.0),
                        toml::Value::Float(0.0),
                        toml::Value::Float(0.0),
                    ]),
                );
                m
            }),
        )
        .unwrap();

    let ast = engine
        .compile(
            r#"
        fn on_init() {
            let me = self_entity();
            set_rotation(me, 0.0, 90.0, 0.0);
        }
    "#,
        )
        .unwrap();

    engine.add_script(id, ast, "test.rhai".into());
    engine.call_inits(&mut world);

    let t = world.get_transform(id).unwrap();
    assert!(
        (t.rotation.y - 90.0).abs() < 0.01,
        "rotation Y should be 90, got {}",
        t.rotation.y
    );
}

fn quat_dot(a: [f32; 4], b: [f32; 4]) -> f32 {
    (0..4).map(|i| a[i] * b[i]).sum::<f32>().abs()
}

fn spawn_with_quat(world: &mut FlintWorld, name: &str, q: Option<[f32; 4]>) -> EntityId {
    let id = world.spawn(name).unwrap();
    let mut m = toml::map::Map::new();
    m.insert(
        "position".into(),
        toml::Value::Array(vec![
            toml::Value::Float(0.0),
            toml::Value::Float(0.0),
            toml::Value::Float(0.0),
        ]),
    );
    if let Some(q) = q {
        m.insert(
            "rotation_quat".into(),
            toml::Value::Array(q.iter().map(|c| toml::Value::Float(*c as f64)).collect()),
        );
    }
    world
        .set_component(id, "transform", toml::Value::Table(m))
        .unwrap();
    id
}

#[test]
fn rotate_local_composes_onto_identity() {
    let mut engine = ScriptEngine::new();
    let mut world = FlintWorld::new();
    let id = spawn_with_quat(&mut world, "spinner", None);
    let ast = engine
        .compile(
            r#"
        fn on_init() {
            let me = self_entity();
            rotate_local(me, 0.0, 45.0, 0.0);
            rotate_local(me, 0.0, 45.0, 0.0);
        }
    "#,
        )
        .unwrap();
    engine.add_script(id, ast, "test.rhai".into());
    engine.call_inits(&mut world);

    let t = world.get_transform(id).unwrap();
    let expected = flint_core::euler_deg_to_quat(0.0, 90.0, 0.0);
    assert!(
        quat_dot(t.effective_quat(), expected) > 0.9999,
        "two 45° yaws should be 90°: {:?}",
        t.rotation_quat
    );
}

#[test]
fn rotate_local_composes_onto_rest_quaternion() {
    let mut engine = ScriptEngine::new();
    let mut world = FlintWorld::new();
    // Rest pose leaned 15° about X (like a fork axis); drive 30° about its own X.
    let rest = flint_core::euler_deg_to_quat(15.0, 0.0, 0.0);
    let id = spawn_with_quat(&mut world, "fork", Some(rest));
    let ast = engine
        .compile(
            r#"
        fn on_init() {
            rotate_local(self_entity(), 30.0, 0.0, 0.0);
        }
    "#,
        )
        .unwrap();
    engine.add_script(id, ast, "test.rhai".into());
    engine.call_inits(&mut world);

    let t = world.get_transform(id).unwrap();
    let expected = flint_core::euler_deg_to_quat(45.0, 0.0, 0.0);
    assert!(
        quat_dot(t.effective_quat(), expected) > 0.9999,
        "rest 15° + 30° should be 45°: {:?}",
        t.rotation_quat
    );
}

/// #625: a script shears one entity, reads it back, and clears it.
#[test]
fn set_get_and_clear_shear() {
    let mut engine = ScriptEngine::new();
    let mut world = FlintWorld::new();
    let id = spawn_with_quat(&mut world, "tile", None);
    let ast = engine
        .compile(
            r#"
        fn on_init() {
            let me = self_entity();
            set_shear(me, #{ xz: 0.25, yz: 1 });
            let s = get_shear(me);
            set_field(me, "probe", "xz", s.xz);
            set_field(me, "probe", "xy", s.xy);
        }
    "#,
        )
        .unwrap();
    engine.add_script(id, ast, "test.rhai".into());
    engine.call_inits(&mut world);

    let t = world.get_transform(id).unwrap();
    assert_eq!(t.shear, Some([0.0, 0.25, 0.0, 1.0, 0.0, 0.0]));
    let m = t.to_matrix();
    assert_eq!([m[2][0], m[2][1], m[2][2]], [0.25, 1.0, 1.0]);
    let comps = world.get_components(id).unwrap();
    let probe = |k| comps.get_field("probe", k).and_then(|v| v.as_float());
    assert_eq!(probe("xz"), Some(0.25));
    assert_eq!(probe("xy"), Some(0.0));
    let stored = comps.get_field("transform", "shear").unwrap();
    assert_eq!(stored.as_table().unwrap().len(), 2, "only non-zero terms");

    let clear = engine
        .compile("fn on_update() { clear_shear(self_entity()); }")
        .unwrap();
    engine.add_script(id, clear, "clear.rhai".into());
    engine.call_updates(&mut world);
    let comps = world.get_components(id).unwrap();
    assert!(comps.get_field("transform", "shear").is_none());
    assert_eq!(world.get_transform(id).unwrap().shear, None);
}

#[test]
fn set_and_get_rotation_quat_round_trip() {
    let mut engine = ScriptEngine::new();
    let mut world = FlintWorld::new();
    let id = spawn_with_quat(&mut world, "quat", None);
    let ast = engine
        .compile(
            r#"
        fn on_init() {
            let me = self_entity();
            set_rotation_quat(me, 0.0, 0.7071, 0.0, 0.7071);
            let q = get_rotation_quat(me);
            set_field(me, "probe", "y", q.y);
            set_field(me, "probe", "w", q.w);
        }
    "#,
        )
        .unwrap();
    engine.add_script(id, ast, "test.rhai".into());
    engine.call_inits(&mut world);

    let comps = world.get_components(id).unwrap();
    let y = comps
        .get_field("probe", "y")
        .and_then(|v| v.as_float())
        .unwrap();
    let w = comps
        .get_field("probe", "w")
        .and_then(|v| v.as_float())
        .unwrap();
    assert!((y - 0.70710677).abs() < 1e-4 && (w - 0.70710677).abs() < 1e-4);
    let t = world.get_transform(id).unwrap();
    assert!(t.rotation_quat.is_some());
    assert_eq!(
        t.rotation.y, 0.0,
        "Euler zeroed when the quaternion is authoritative"
    );
}

#[test]
fn set_joint_target_writes_motor_target() {
    let mut engine = ScriptEngine::new();
    let mut world = FlintWorld::new();
    let id = spawn_with_quat(&mut world, "piston", None);
    let mut j = toml::map::Map::new();
    j.insert("type".into(), toml::Value::String("prismatic".into()));
    world
        .set_component(id, "joint", toml::Value::Table(j))
        .unwrap();
    let ast = engine
        .compile(
            r#"
        fn on_init() {
            set_joint_target(self_entity(), 0.25);
            set_field(self_entity(), "probe", "t", get_joint_target(self_entity()));
        }
    "#,
        )
        .unwrap();
    engine.add_script(id, ast, "test.rhai".into());
    engine.call_inits(&mut world);

    let comps = world.get_components(id).unwrap();
    let t = comps
        .get_field("joint", "motor_target")
        .and_then(|v| v.as_float())
        .unwrap();
    assert!((t - 0.25).abs() < 1e-9);
    let probe = comps
        .get_field("probe", "t")
        .and_then(|v| v.as_float())
        .unwrap();
    assert!((probe - 0.25).abs() < 1e-9);
}

#[test]
fn test_set_material_color_from_script() {
    let mut engine = ScriptEngine::new();
    let mut world = FlintWorld::new();
    let id = world.spawn("colored").unwrap();

    world
        .set_component(
            id,
            "material",
            toml::Value::Table({
                let mut m = toml::map::Map::new();
                m.insert(
                    "color".into(),
                    toml::Value::Array(vec![
                        toml::Value::Float(1.0),
                        toml::Value::Float(1.0),
                        toml::Value::Float(1.0),
                    ]),
                );
                m
            }),
        )
        .unwrap();

    let ast = engine
        .compile(
            r#"
        fn on_init() {
            let me = self_entity();
            set_material_color(me, 1.0, 0.0, 0.0, 1.0);
        }
    "#,
        )
        .unwrap();

    engine.add_script(id, ast, "test.rhai".into());
    engine.call_inits(&mut world);

    let mat = world.get_component(id, "material").unwrap();
    // set_material_color stores individual base_color_r/g/b/a fields
    assert!((mat.get("base_color_r").unwrap().as_float().unwrap() - 1.0).abs() < 0.01);
    assert!((mat.get("base_color_g").unwrap().as_float().unwrap() - 0.0).abs() < 0.01);
    assert!((mat.get("base_color_b").unwrap().as_float().unwrap() - 0.0).abs() < 0.01);
    assert!((mat.get("base_color_a").unwrap().as_float().unwrap() - 1.0).abs() < 0.01);
}

#[test]
fn test_set_material_override_from_script() {
    let mut engine = ScriptEngine::new();
    let mut world = FlintWorld::new();
    let id = world.spawn("liveried").unwrap();

    let ast = engine
        .compile(
            r#"
        fn on_init() {
            let me = self_entity();
            set_material_override(me, "Frame", 0.2, 0.4, 0.6, 1.0);
            set_material_override(me, "Capsule", 1.0, 0.5, 0.0, 0.4);
        }
    "#,
        )
        .unwrap();

    engine.add_script(id, ast, "test.rhai".into());
    engine.call_inits(&mut world);

    // The component is created on demand, keyed by glTF material name.
    let ov = world.get_component(id, "material_overrides").unwrap();
    let frame = ov.get("Frame").unwrap().as_array().unwrap();
    let f = |i: usize| frame[i].as_float().unwrap();
    assert!((f(0) - 0.2).abs() < 1e-6 && (f(1) - 0.4).abs() < 1e-6);
    assert!((f(2) - 0.6).abs() < 1e-6 && (f(3) - 1.0).abs() < 1e-6);
    let glass = ov.get("Capsule").unwrap().as_array().unwrap();
    assert!((glass[3].as_float().unwrap() - 0.4).abs() < 1e-6);
}

#[test]
fn test_set_material_emissive_from_script() {
    let mut engine = ScriptEngine::new();
    let mut world = FlintWorld::new();
    let id = world.spawn("road").unwrap();

    let ast = engine
        .compile(
            r#"
        fn on_init() {
            let me = self_entity();
            set_material_override(me, "Neon", 0.1, 0.1, 0.1, 1.0);
            set_material_emissive(me, "Neon", 0.0, 1.0, 0.8, 4.0);
            set_material_emissive(me, "Sign", 1.0, 0.2, 0.6);
            set_material_override(me, "Sign", 0.5, 0.5, 0.5, 1.0);
            set_material_emissive(me, "Gone", 1.0, 1.0, 1.0);
            clear_material_override(me, "Gone");
        }
    "#,
        )
        .unwrap();

    engine.add_script(id, ast, "test.rhai".into());
    engine.call_inits(&mut world);

    let ov = world.get_component(id, "material_overrides").unwrap();
    let num = |v: &toml::Value| v.as_float().unwrap();
    // The paint set first survives the light set after it.
    let neon = ov.get("Neon").unwrap().as_table().unwrap();
    assert!((num(&neon["base_color"][0]) - 0.1).abs() < 1e-6);
    assert!((num(&neon["emissive"][1]) - 1.0).abs() < 1e-6);
    assert!((num(&neon["emissive_strength"]) - 4.0).abs() < 1e-6);
    // And a light set first survives the paint set after it.
    let sign = ov.get("Sign").unwrap().as_table().unwrap();
    assert!((num(&sign["emissive"][2]) - 0.6).abs() < 1e-6);
    assert!((num(&sign["base_color"][0]) - 0.5).abs() < 1e-6);
    assert!(!sign.contains_key("emissive_strength"));
    assert!(ov.get("Gone").is_none());
}

#[test]
fn test_scale_and_visibility_from_script() {
    // #628: set_scale/get_scale, set_visible/is_visible
    let mut engine = ScriptEngine::new();
    let mut world = FlintWorld::new();
    let id = world.spawn("tile").unwrap();
    let shown = world.spawn("never_hidden").unwrap();
    world
        .set_component(id, "transform", toml::Value::Table(toml::map::Map::new()))
        .unwrap();

    let ast = engine
        .compile(
            r#"
        fn on_init() {
            let me = self_entity();
            let before = get_scale(me);
            set_scale(me, before.x * 2.0, 0.5, 3.0);
            set_visible(me, false);
            if is_visible(me) { throw "still visible"; }
            let s = get_scale(me);
            if s.x != 2.0 || s.y != 0.5 || s.z != 3.0 { throw "scale " + s; }
            set_visible(get_entity("never_hidden"), true);
        }
    "#,
        )
        .unwrap();

    engine.add_script(id, ast, "test.rhai".into());
    engine.call_inits(&mut world);

    let t = world.get_transform(id).unwrap();
    assert!((t.scale.x - 2.0).abs() < 1e-6 && (t.scale.z - 3.0).abs() < 1e-6);
    assert!(!world.is_visible(id));
    assert!(world.hidden_entities().contains(&id));
    // Showing what was never hidden adds nothing
    assert!(world.get_component(shown, "visibility").is_none());
}

#[test]
fn test_find_entities_with_from_script() {
    let mut engine = ScriptEngine::new();
    let mut world = FlintWorld::new();
    let controller = world.spawn("controller").unwrap();

    // Create 3 entities with "health" component
    for name in &["enemy_a", "enemy_b", "enemy_c"] {
        let eid = world.spawn(*name).unwrap();
        world
            .set_component(
                eid,
                "health",
                toml::Value::Table({
                    let mut m = toml::map::Map::new();
                    m.insert("current".into(), toml::Value::Integer(100));
                    m
                }),
            )
            .unwrap();
    }

    world
        .set_component(
            controller,
            "result",
            toml::Value::Table({
                let mut m = toml::map::Map::new();
                m.insert("count".into(), toml::Value::Integer(0));
                m
            }),
        )
        .unwrap();

    let ast = engine
        .compile(
            r#"
        fn on_init() {
            let me = self_entity();
            let enemies = find_entities_with("health");
            set_field(me, "result", "count", enemies.len());
        }
    "#,
        )
        .unwrap();

    engine.add_script(controller, ast, "test.rhai".into());
    engine.call_inits(&mut world);

    let count = world
        .get_components(controller)
        .unwrap()
        .get_field("result", "count")
        .unwrap()
        .as_integer()
        .unwrap();
    assert_eq!(count, 3, "should find 3 entities with 'health' component");
}

#[test]
fn test_negative_entity_id_guards() {
    // Negative entity IDs should not crash
    let mut engine = ScriptEngine::new();
    let mut world = FlintWorld::new();
    let id = world.spawn("guard_test").unwrap();

    let ast = engine
        .compile(
            r#"
        fn on_init() {
            let exists = entity_exists(-1);
            let pos = get_position(-1);
        }
    "#,
        )
        .unwrap();

    engine.add_script(id, ast, "test.rhai".into());
    // Should not panic
    engine.call_inits(&mut world);
}

#[test]
fn test_set_parent_from_script() {
    let mut engine = ScriptEngine::new();
    let mut world = FlintWorld::new();
    let controller = world.spawn("controller").unwrap();
    let parent = world.spawn("parent_e").unwrap();
    let child = world.spawn("child_e").unwrap();

    let ast = engine
        .compile(&format!(
            r#"
        fn on_init() {{
            set_parent({child_id}, {parent_id});
        }}
    "#,
            child_id = child.raw() as i64,
            parent_id = parent.raw() as i64,
        ))
        .unwrap();

    engine.add_script(controller, ast, "test.rhai".into());
    engine.call_inits(&mut world);

    assert_eq!(
        world.get_parent(child),
        Some(parent),
        "parent should be set from script"
    );
}

#[test]
fn test_get_children_from_script() {
    let mut engine = ScriptEngine::new();
    let mut world = FlintWorld::new();
    let parent = world.spawn("parent").unwrap();
    let child1 = world.spawn("child1").unwrap();
    let child2 = world.spawn("child2").unwrap();

    world.set_parent(child1, parent).unwrap();
    world.set_parent(child2, parent).unwrap();

    world
        .set_component(
            parent,
            "result",
            toml::Value::Table({
                let mut m = toml::map::Map::new();
                m.insert("child_count".into(), toml::Value::Integer(0));
                m
            }),
        )
        .unwrap();

    let ast = engine
        .compile(
            r#"
        fn on_init() {
            let me = self_entity();
            let kids = get_children(me);
            set_field(me, "result", "child_count", kids.len());
        }
    "#,
        )
        .unwrap();

    engine.add_script(parent, ast, "test.rhai".into());
    engine.call_inits(&mut world);

    let count = world
        .get_components(parent)
        .unwrap()
        .get_field("result", "child_count")
        .unwrap()
        .as_integer()
        .unwrap();
    assert_eq!(count, 2, "should have 2 children");
}

#[test]
fn test_get_world_position_from_script() {
    let mut engine = ScriptEngine::new();
    let mut world = FlintWorld::new();

    let parent = world.spawn("parent").unwrap();
    let child = world.spawn("child").unwrap();

    let make_pos = |x: f64, y: f64, z: f64| {
        toml::Value::Table({
            let mut m = toml::map::Map::new();
            m.insert(
                "position".into(),
                toml::Value::Array(vec![
                    toml::Value::Float(x),
                    toml::Value::Float(y),
                    toml::Value::Float(z),
                ]),
            );
            m
        })
    };

    world
        .set_component(parent, "transform", make_pos(10.0, 0.0, 0.0))
        .unwrap();
    world
        .set_component(child, "transform", make_pos(5.0, 0.0, 0.0))
        .unwrap();
    world.set_parent(child, parent).unwrap();

    world
        .set_component(
            child,
            "result",
            toml::Value::Table({
                let mut m = toml::map::Map::new();
                m.insert("wx".into(), toml::Value::Float(0.0));
                m
            }),
        )
        .unwrap();

    let ast = engine
        .compile(
            r#"
        fn on_init() {
            let me = self_entity();
            let wp = get_world_position(me);
            set_field(me, "result", "wx", wp.x);
        }
    "#,
        )
        .unwrap();

    engine.add_script(child, ast, "test.rhai".into());
    engine.call_inits(&mut world);

    let wx = world
        .get_components(child)
        .unwrap()
        .get_field("result", "wx")
        .unwrap()
        .as_float()
        .unwrap();
    assert!(
        (wx - 15.0).abs() < 0.01,
        "world x should be 15.0, got {}",
        wx
    );
}

#[test]
fn test_distance_between_entities() {
    let mut engine = ScriptEngine::new();
    let mut world = FlintWorld::new();

    let a = world.spawn("a").unwrap();
    let b = world.spawn("b").unwrap();

    let make_pos = |x: f64, y: f64, z: f64| {
        toml::Value::Table({
            let mut m = toml::map::Map::new();
            m.insert(
                "position".into(),
                toml::Value::Array(vec![
                    toml::Value::Float(x),
                    toml::Value::Float(y),
                    toml::Value::Float(z),
                ]),
            );
            m
        })
    };

    world
        .set_component(a, "transform", make_pos(0.0, 0.0, 0.0))
        .unwrap();
    world
        .set_component(b, "transform", make_pos(3.0, 4.0, 0.0))
        .unwrap();

    world
        .set_component(
            a,
            "result",
            toml::Value::Table({
                let mut m = toml::map::Map::new();
                m.insert("dist".into(), toml::Value::Float(0.0));
                m
            }),
        )
        .unwrap();

    let ast = engine
        .compile(&format!(
            r#"
        fn on_init() {{
            let me = self_entity();
            let d = distance(me, {b_id});
            set_field(me, "result", "dist", d);
        }}
    "#,
            b_id = b.raw() as i64,
        ))
        .unwrap();

    engine.add_script(a, ast, "test.rhai".into());
    engine.call_inits(&mut world);

    let dist = world
        .get_components(a)
        .unwrap()
        .get_field("result", "dist")
        .unwrap()
        .as_float()
        .unwrap();
    assert!(
        (dist - 5.0).abs() < 0.01,
        "distance should be 5.0 (3-4-5 triangle), got {}",
        dist
    );
}

#[test]
fn test_entity_count_with_component() {
    let mut engine = ScriptEngine::new();
    let mut world = FlintWorld::new();
    let controller = world.spawn("counter").unwrap();

    // Create entities, some with "light" component
    for i in 0..4 {
        let name = format!("light_{}", i);
        let eid = world.spawn(&name).unwrap();
        world
            .set_component(eid, "light", toml::Value::Table(Default::default()))
            .unwrap();
    }
    // Also create one without light
    world.spawn("no_light").unwrap();

    world
        .set_component(
            controller,
            "result",
            toml::Value::Table({
                let mut m = toml::map::Map::new();
                m.insert("count".into(), toml::Value::Integer(0));
                m
            }),
        )
        .unwrap();

    let ast = engine
        .compile(
            r#"
        fn on_init() {
            let me = self_entity();
            let lights = find_entities_with("light");
            set_field(me, "result", "count", lights.len());
        }
    "#,
        )
        .unwrap();

    engine.add_script(controller, ast, "test.rhai".into());
    engine.call_inits(&mut world);

    let count = world
        .get_components(controller)
        .unwrap()
        .get_field("result", "count")
        .unwrap()
        .as_integer()
        .unwrap();
    assert_eq!(count, 4, "should find exactly 4 entities with 'light'");
}
