//! Joint behaviour tests (ADR 0069): hinge, prismatic, spherical, lifecycle.

use crate::PhysicsSystem;
use flint_core::EntityId;
use flint_ecs::FlintWorld;
use flint_runtime::RuntimeSystem;
use rapier3d::prelude::JointAxis;

const DT: f64 = 1.0 / 60.0;

fn floats(v: &[f64]) -> toml::Value {
    toml::Value::Array(v.iter().map(|c| toml::Value::Float(*c)).collect())
}

fn table(pairs: Vec<(&str, toml::Value)>) -> toml::Value {
    let mut m = toml::map::Map::new();
    for (k, v) in pairs {
        m.insert(k.to_string(), v);
    }
    toml::Value::Table(m)
}

fn s(v: &str) -> toml::Value {
    toml::Value::String(v.to_string())
}

fn f(v: f64) -> toml::Value {
    toml::Value::Float(v)
}

/// Spawn an entity with transform, rigidbody and collider.
fn body(
    world: &mut FlintWorld,
    name: &str,
    pos: [f64; 3],
    body_type: &str,
    shape: &str,
    size: [f64; 3],
    gravity_scale: f64,
) -> EntityId {
    let id = world.spawn(name).unwrap();
    world
        .set_component(id, "transform", table(vec![("position", floats(&pos))]))
        .unwrap();
    world
        .set_component(
            id,
            "rigidbody",
            table(vec![
                ("body_type", s(body_type)),
                ("gravity_scale", f(gravity_scale)),
            ]),
        )
        .unwrap();
    world
        .set_component(
            id,
            "collider",
            table(vec![("shape", s(shape)), ("size", floats(&size))]),
        )
        .unwrap();
    id
}

fn joint(world: &mut FlintWorld, id: EntityId, pairs: Vec<(&str, toml::Value)>) {
    world.set_component(id, "joint", table(pairs)).unwrap();
}

fn world_pos(world: &FlintWorld, id: EntityId) -> [f32; 3] {
    let p = world.get_world_position(id).unwrap();
    [p.x, p.y, p.z]
}

#[test]
fn hinge_pendulum_keeps_anchor_distance_and_swings() {
    let mut world = FlintWorld::new();
    let mut sys = PhysicsSystem::new();

    // Anchor is a static body; the bob is its transform child 2 m along +X,
    // so the joint resolves the parent implicitly and the bob simulates in
    // world space (item 2 of the plan).
    let anchor = body(
        &mut world,
        "anchor",
        [0.0, 5.0, 0.0],
        "static",
        "box",
        [0.2, 0.2, 0.2],
        1.0,
    );
    let bob = body(
        &mut world,
        "bob",
        [2.0, 0.0, 0.0],
        "dynamic",
        "sphere",
        [0.5, 0.5, 0.5],
        1.0,
    );
    world.set_parent(bob, anchor).unwrap();
    joint(
        &mut world,
        bob,
        vec![
            ("type", s("hinge")),
            ("anchor", floats(&[-2.0, 0.0, 0.0])),
            ("axis", floats(&[0.0, 0.0, 1.0])),
        ],
    );

    sys.initialize(&mut world).unwrap();
    assert_eq!(sys.joint_count(), 1, "hinge created at init");

    let mut min_y = f32::MAX;
    for _ in 0..240 {
        sys.fixed_update(&mut world, DT).unwrap();
        let p = world_pos(&world, bob);
        let dist = ((p[0]).powi(2) + (p[1] - 5.0).powi(2) + p[2].powi(2)).sqrt();
        assert!(
            (dist - 2.0).abs() < 0.05,
            "bob left the arc: {p:?} dist {dist}"
        );
        assert!(p[2].abs() < 0.02, "bob left the hinge plane: {p:?}");
        min_y = min_y.min(p[1]);
    }
    assert!(
        min_y < 3.6,
        "bob should swing down under gravity, min y {min_y}"
    );

    let angle = sys.joint_position(bob).expect("hinge angle");
    assert!(
        angle.abs() > 5.0,
        "hinge angle should be non-trivial: {angle}"
    );
}

#[test]
fn prismatic_settles_at_target_respects_limits_and_updates_in_place() {
    let mut world = FlintWorld::new();
    let mut sys = PhysicsSystem::new();

    let _base = body(
        &mut world,
        "base",
        [0.0, 3.0, 0.0],
        "static",
        "box",
        [0.5, 0.5, 0.5],
        1.0,
    );
    let block = body(
        &mut world,
        "block",
        [0.0, 2.0, 0.0],
        "dynamic",
        "box",
        [0.5, 0.5, 0.5],
        0.0,
    );
    joint(
        &mut world,
        block,
        vec![
            ("type", s("prismatic")),
            ("parent", s("base")),
            ("axis", floats(&[0.0, 1.0, 0.0])),
            ("limits", floats(&[-0.5, 0.5])),
            ("motor_target", f(0.3)),
            ("motor_stiffness", f(500.0)),
            ("motor_damping", f(50.0)),
        ],
    );

    sys.initialize(&mut world).unwrap();
    assert_eq!(sys.joint_count(), 1);

    for _ in 0..300 {
        sys.fixed_update(&mut world, DT).unwrap();
    }
    let p = world_pos(&world, block);
    assert!(p[0].abs() < 1e-3 && p[2].abs() < 1e-3, "off axis: {p:?}");
    assert!(
        (p[1] - 2.3).abs() < 0.03,
        "should settle at target 0.3: {p:?}"
    );
    let d = sys.joint_position(block).expect("prismatic displacement");
    assert!((d - 0.3).abs() < 0.03, "joint_position {d}");

    // Script-style target change: written to the component, picked up in place.
    world.set_field(block, "joint", "motor_target", f(5.0));
    for _ in 0..300 {
        sys.fixed_update(&mut world, DT).unwrap();
    }
    assert_eq!(sys.joint_count(), 1, "joint updated, not recreated");
    let handle = sys.sync.get_joint_handle(block).unwrap();
    let motor = sys
        .physics_world
        .impulse_joint_set
        .get(handle)
        .unwrap()
        .data
        .motor(JointAxis::LinX)
        .unwrap();
    assert!(
        (motor.target_pos - 5.0).abs() < 1e-5,
        "target pushed: {}",
        motor.target_pos
    );
    let p = world_pos(&world, block);
    assert!(
        p[1] <= 2.5 + 0.03,
        "limit should stop the block at +0.5: {p:?}"
    );
    assert!(p[1] > 2.4, "block should reach the upper limit: {p:?}");
}

#[test]
fn spherical_motor_drives_pod_about_axis() {
    let mut world = FlintWorld::new();
    let mut sys = PhysicsSystem::new();

    let _base = body(
        &mut world,
        "base",
        [0.0, 4.0, 0.0],
        "static",
        "box",
        [0.2, 0.2, 0.2],
        1.0,
    );
    let pod = body(
        &mut world,
        "pod",
        [0.0, 3.0, 0.0],
        "dynamic",
        "capsule",
        [0.4, 1.0, 0.4],
        0.0,
    );
    joint(
        &mut world,
        pod,
        vec![
            ("type", s("spherical")),
            ("parent", s("base")),
            ("anchor", floats(&[0.0, 1.0, 0.0])),
            ("motor_axis", s("z")),
            ("motor_target", f(30.0)),
            ("motor_stiffness", f(100.0)),
            ("motor_damping", f(20.0)),
        ],
    );

    sys.initialize(&mut world).unwrap();
    for _ in 0..300 {
        sys.fixed_update(&mut world, DT).unwrap();
    }
    let p = world_pos(&world, pod);
    // The pod hangs 1 m under the anchor, swung 30 degrees about Z.
    let dist = (p[0].powi(2) + (p[1] - 4.0).powi(2) + p[2].powi(2)).sqrt();
    assert!((dist - 1.0).abs() < 0.03, "pod left the sphere: {p:?}");
    assert!(
        (p[0].abs() - 0.5).abs() < 0.05,
        "expected |x| = sin 30: {p:?}"
    );
    assert!(
        (p[1] - 3.134).abs() < 0.05,
        "expected y = 4 - cos 30: {p:?}"
    );
}

#[test]
fn joint_waits_for_bodies_then_attaches() {
    let mut world = FlintWorld::new();
    let mut sys = PhysicsSystem::new();

    let _base = body(
        &mut world,
        "base",
        [0.0, 3.0, 0.0],
        "static",
        "box",
        [0.5, 0.5, 0.5],
        1.0,
    );
    // Jointed entity without a rigidbody yet
    let late = world.spawn("late").unwrap();
    world
        .set_component(
            late,
            "transform",
            table(vec![("position", floats(&[0.0, 2.0, 0.0]))]),
        )
        .unwrap();
    joint(
        &mut world,
        late,
        vec![("type", s("hinge")), ("parent", s("base"))],
    );

    sys.initialize(&mut world).unwrap();
    assert_eq!(sys.joint_count(), 0, "no body yet, no joint");

    world
        .set_component(late, "rigidbody", table(vec![("body_type", s("dynamic"))]))
        .unwrap();
    world
        .set_component(
            late,
            "collider",
            table(vec![
                ("shape", s("sphere")),
                ("size", floats(&[0.5, 0.5, 0.5])),
            ]),
        )
        .unwrap();
    sys.fixed_update(&mut world, DT).unwrap();
    assert_eq!(sys.joint_count(), 1, "joint attaches once the body exists");
}

#[test]
fn remove_entity_drops_joint_from_either_end() {
    for remove_parent in [false, true] {
        let mut world = FlintWorld::new();
        let mut sys = PhysicsSystem::new();
        let base = body(
            &mut world,
            "base",
            [0.0, 3.0, 0.0],
            "static",
            "box",
            [0.5, 0.5, 0.5],
            1.0,
        );
        let block = body(
            &mut world,
            "block",
            [0.0, 2.0, 0.0],
            "dynamic",
            "box",
            [0.5, 0.5, 0.5],
            0.0,
        );
        joint(
            &mut world,
            block,
            vec![("type", s("fixed")), ("parent", s("base"))],
        );
        sys.initialize(&mut world).unwrap();
        assert_eq!(sys.joint_count(), 1);

        sys.remove_entity(if remove_parent { base } else { block });
        assert_eq!(sys.joint_count(), 0, "remove_parent={remove_parent}");
        assert!(sys.sync.joint_map.is_empty());
        assert!(sys.sync.joint_cache.is_empty());
        // Stepping afterwards must not panic on stale handles
        sys.fixed_update(&mut world, DT).unwrap();
    }
}
