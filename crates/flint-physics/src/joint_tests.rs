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

#[test]
fn jointed_child_follows_kinematic_parent_after_resting() {
    let mut world = FlintWorld::new();
    let mut sys = PhysicsSystem::new();

    // A scripted (kinematic) base with a sprung hinge child that has nothing to
    // do: after a few seconds Rapier would put the child to sleep, and moving
    // the base would then leave it behind. Jointed bodies may not sleep.
    let base = body(
        &mut world,
        "base",
        [0.0, 2.0, 0.0],
        "kinematic",
        "box",
        [0.2, 0.2, 0.2],
        0.0,
    );
    let arm = body(
        &mut world,
        "arm",
        [0.0, 0.0, 0.5],
        "dynamic",
        "box",
        [0.1, 0.1, 0.1],
        0.0,
    );
    world.set_parent(arm, base).unwrap();
    joint(
        &mut world,
        arm,
        vec![
            ("type", s("hinge")),
            ("anchor", floats(&[0.0, 0.0, -0.5])),
            ("axis", floats(&[1.0, 0.0, 0.0])),
            ("motor_stiffness", f(400.0)),
            ("motor_damping", f(25.0)),
        ],
    );

    sys.initialize(&mut world).unwrap();
    for _ in 0..300 {
        sys.fixed_update(&mut world, DT).unwrap();
    }
    let before = world_pos(&world, arm);
    assert!((before[2] - 0.5).abs() < 0.05, "arm at rest behind base: {before:?}");

    // Script drives the base 3 m along +X.
    world
        .set_field(base, "transform", "position", floats(&[3.0, 2.0, 0.0]))
        .unwrap();
    for _ in 0..60 {
        sys.fixed_update(&mut world, DT).unwrap();
    }
    let after = world_pos(&world, arm);
    assert!(
        (after[0] - 3.0).abs() < 0.05 && (after[2] - 0.5).abs() < 0.05,
        "arm should follow the kinematic base: {after:?}"
    );
}

#[test]
fn hinged_child_turns_with_yawing_kinematic_parent() {
    let mut world = FlintWorld::new();
    let mut sys = PhysicsSystem::new();

    let base = body(
        &mut world,
        "base",
        [0.0, 2.0, 0.0],
        "kinematic",
        "box",
        [0.2, 0.2, 0.2],
        0.0,
    );
    let arm = body(
        &mut world,
        "arm",
        [0.0, 0.0, 1.0],
        "dynamic",
        "box",
        [0.1, 0.1, 0.1],
        0.0,
    );
    world.set_parent(arm, base).unwrap();
    joint(
        &mut world,
        arm,
        vec![
            ("type", s("hinge")),
            ("anchor", floats(&[0.0, 0.0, -1.0])),
            ("axis", floats(&[1.0, 0.0, 0.0])),
            ("motor_stiffness", f(400.0)),
            ("motor_damping", f(25.0)),
        ],
    );

    sys.initialize(&mut world).unwrap();
    for _ in 0..30 {
        sys.fixed_update(&mut world, DT).unwrap();
    }

    // Yaw the base 90 degrees over a second, 1.5 deg per step.
    for i in 1..=60 {
        world
            .set_field(base, "transform", "rotation", floats(&[0.0, 1.5 * i as f64, 0.0]))
            .unwrap();
        sys.fixed_update(&mut world, DT).unwrap();
    }
    for _ in 0..30 {
        sys.fixed_update(&mut world, DT).unwrap();
    }
    let p = world_pos(&world, arm);
    // Yaw +90 about Y takes local +Z to world +X.
    assert!(
        (p[0] - 1.0).abs() < 0.1 && p[2].abs() < 0.1,
        "arm should have swung round with the base: {p:?}"
    );
}

#[test]
fn hinged_child_turns_when_the_kinematic_parents_own_parent_yaws() {
    let mut world = FlintWorld::new();
    let mut sys = PhysicsSystem::new();

    // root (no body, script-yawed) -> base (kinematic) -> arm (hinge)
    let root = world.spawn("root").unwrap();
    world
        .set_component(root, "transform", table(vec![("position", floats(&[0.0, 2.0, 0.0]))]))
        .unwrap();
    let base = body(&mut world, "base", [0.0, 0.0, 0.0], "kinematic", "box", [0.2, 0.2, 0.2], 0.0);
    world.set_parent(base, root).unwrap();
    let arm = body(&mut world, "arm", [0.0, 0.0, 1.0], "dynamic", "box", [0.1, 0.1, 0.1], 0.0);
    world.set_parent(arm, base).unwrap();
    joint(
        &mut world,
        arm,
        vec![
            ("type", s("hinge")),
            ("anchor", floats(&[0.0, 0.0, -1.0])),
            ("axis", floats(&[1.0, 0.0, 0.0])),
            ("motor_stiffness", f(400.0)),
            ("motor_damping", f(25.0)),
        ],
    );

    sys.initialize(&mut world).unwrap();
    for _ in 0..30 {
        sys.fixed_update(&mut world, DT).unwrap();
    }
    for i in 1..=60 {
        world
            .set_field(root, "transform", "rotation", floats(&[0.0, 1.5 * i as f64, 0.0]))
            .unwrap();
        sys.fixed_update(&mut world, DT).unwrap();
    }
    for _ in 0..30 {
        sys.fixed_update(&mut world, DT).unwrap();
    }
    let p = world_pos(&world, arm);
    assert!(
        (p[0] - 1.0).abs() < 0.1 && p[2].abs() < 0.1,
        "arm should have swung round with the root: {p:?}"
    );
    // And the arm's own orientation should have yawed too: its local +Z now points world +X.
    let m = world.get_world_matrix(arm).unwrap();
    let fwd = [m[2][0], m[2][1], m[2][2]]; // column-major: local +Z in world
    assert!(
        fwd[0] > 0.95 && fwd[2].abs() < 0.2,
        "arm should face world +X after the yaw: {fwd:?}"
    );
}

#[test]
fn collider_less_hinged_body_rotates_with_parent_and_motor() {
    let mut world = FlintWorld::new();
    let mut sys = PhysicsSystem::new();

    // A game may give a jointed node a rigidbody for mass alone (a collider on
    // the chain would be hit by the vehicle's own ground ray). Without angular
    // inertia such a body could never turn.
    let base = body(&mut world, "base", [0.0, 2.0, 0.0], "kinematic", "box", [0.2, 0.2, 0.2], 0.0);
    // Hinge anchored at the arm's own origin, as a vehicle articulation is.
    let arm = world.spawn("arm").unwrap();
    world
        .set_component(arm, "transform", table(vec![("position", floats(&[0.0, 0.0, 0.3]))]))
        .unwrap();
    world
        .set_component(
            arm,
            "rigidbody",
            table(vec![("body_type", s("dynamic")), ("gravity_scale", f(0.0))]),
        )
        .unwrap();
    world.set_parent(arm, base).unwrap();
    joint(
        &mut world,
        arm,
        vec![
            ("type", s("hinge")),
            ("anchor", floats(&[0.0, 0.0, 0.0])),
            ("axis", floats(&[1.0, 0.0, 0.0])),
            ("motor_stiffness", f(400.0)),
            ("motor_damping", f(25.0)),
        ],
    );

    sys.initialize(&mut world).unwrap();
    for i in 1..=60 {
        world
            .set_field(base, "transform", "rotation", floats(&[0.0, 1.5 * i as f64, 0.0]))
            .unwrap();
        sys.fixed_update(&mut world, DT).unwrap();
    }
    for _ in 0..30 {
        sys.fixed_update(&mut world, DT).unwrap();
    }
    let m = world.get_world_matrix(arm).unwrap();
    let fwd = [m[2][0], m[2][1], m[2][2]];
    assert!(
        fwd[0] > 0.95 && fwd[2].abs() < 0.2,
        "collider-less arm should yaw with its parent: {fwd:?}"
    );

    // The motor must be able to turn it about the hinge too.
    world
        .set_field(arm, "joint", "motor_target", f(-60.0))
        .unwrap();
    for _ in 0..60 {
        sys.fixed_update(&mut world, DT).unwrap();
    }
    let angle = sys.joint_position(arm).expect("hinge angle");
    assert!(
        (angle + 60.0).abs() < 5.0,
        "motor should drive the collider-less arm to -60 deg: {angle}"
    );
}

#[test]
fn jointed_child_keeps_up_with_a_fast_kinematic_parent() {
    let mut world = FlintWorld::new();
    let mut sys = PhysicsSystem::new();

    // Rapier's TGS solver substeps the dynamic body but solves each substep
    // against the kinematic body's start-of-step pose, so with n substeps a
    // jointed child trails a scripted parent by (n-1)/n of the parent's step
    // travel: 0.22 m at 18 m/s. Flint runs one substep so the child keeps up.
    let base = body(&mut world, "base", [0.0, 2.0, 0.0], "kinematic", "box", [0.2, 0.2, 0.2], 0.0);
    let arm = world.spawn("arm").unwrap();
    world
        .set_component(arm, "transform", table(vec![("position", floats(&[0.0, 0.0, 0.3]))]))
        .unwrap();
    world
        .set_component(
            arm,
            "rigidbody",
            table(vec![("body_type", s("dynamic")), ("gravity_scale", f(0.0))]),
        )
        .unwrap();
    world.set_parent(arm, base).unwrap();
    joint(
        &mut world,
        arm,
        vec![
            ("type", s("hinge")),
            ("anchor", floats(&[0.0, 0.0, 0.0])),
            ("axis", floats(&[1.0, 0.0, 0.0])),
            ("motor_stiffness", f(60.0)),
            ("motor_damping", f(6.0)),
        ],
    );

    sys.initialize(&mut world).unwrap();
    sys.fixed_update(&mut world, DT).unwrap();
    for i in 1..=12 {
        // 0.3 m per step along +Z: 18 m/s, a vehicle at speed.
        world
            .set_field(base, "transform", "position", floats(&[0.0, 2.0, 0.3 * i as f64]))
            .unwrap();
        sys.fixed_update(&mut world, DT).unwrap();
        let lz = world.get_transform(arm).unwrap().position.z;
        assert!(
            (lz - 0.3).abs() < 0.01,
            "arm should stay at its rest offset behind a moving parent, local z {lz} at step {i}"
        );
    }
}

#[test]
fn jointed_child_stays_anchored_on_a_curved_kinematic_path() {
    let mut world = FlintWorld::new();
    let mut sys = PhysicsSystem::new();

    // root (scripted) -> base (kinematic) -> pod hinged at the base origin.
    // Drive straight at 18 m/s, then turn at 60 deg/s. Rapier matches the
    // pod's velocity to the base's start-of-step tangent, so on the arc the
    // anchors separate a few mm per step and the joint never sees it; the
    // post-step projection keeps the pod on the base.
    let root = world.spawn("root").unwrap();
    world
        .set_component(root, "transform", table(vec![("position", floats(&[0.0, 2.0, 0.0]))]))
        .unwrap();
    let base = body(&mut world, "base", [0.0, 0.0, 0.0], "kinematic", "box", [0.2, 0.2, 0.2], 0.0);
    world.set_parent(base, root).unwrap();
    let pod = world.spawn("pod").unwrap();
    world
        .set_component(pod, "transform", table(vec![("position", floats(&[0.0, 0.0, 0.0]))]))
        .unwrap();
    world
        .set_component(
            pod,
            "rigidbody",
            table(vec![("body_type", s("dynamic")), ("gravity_scale", f(0.0))]),
        )
        .unwrap();
    world.set_parent(pod, base).unwrap();
    joint(
        &mut world,
        pod,
        vec![
            ("type", s("hinge")),
            ("anchor", floats(&[0.0, 0.0, 0.0])),
            ("axis", floats(&[1.0, 0.0, 0.0])),
            ("motor_stiffness", f(60.0)),
            ("motor_damping", f(6.0)),
        ],
    );
    // A steering head (kinematic, yawed under the child) with a sprung
    // prismatic piston hanging forward and down, like the trike's front wheel.
    let head = body(&mut world, "head", [0.0, 0.4, -1.0], "kinematic", "box", [0.1, 0.1, 0.1], 0.0);
    world.set_parent(head, base).unwrap();
    let piston = world.spawn("piston").unwrap();
    world
        .set_component(piston, "transform", table(vec![("position", floats(&[0.0, -0.12, -0.45]))]))
        .unwrap();
    world
        .set_component(
            piston,
            "rigidbody",
            table(vec![("body_type", s("dynamic")), ("gravity_scale", f(0.0))]),
        )
        .unwrap();
    world.set_parent(piston, head).unwrap();
    joint(
        &mut world,
        piston,
        vec![
            ("type", s("prismatic")),
            ("anchor", floats(&[0.0, 0.0, 0.0])),
            ("axis", floats(&[0.0, -0.26, -0.97])),
            ("limits", floats(&[-0.15, 0.15])),
            ("motor_stiffness", f(200.0)),
            ("motor_damping", f(10.0)),
        ],
    );
    sys.initialize(&mut world).unwrap();

    let (mut x, mut z, mut yaw) = (0.0f64, 0.0f64, 0.0f64);
    for i in 0..150 {
        // Accelerate over the first second (to 18 m/s), then turn.
        let step = 0.3 * ((i as f64 + 1.0) / 60.0).min(1.0);
        if i >= 60 {
            // Turn rate eases in over ten steps to 1 deg per step.
            yaw -= ((i as f64 - 59.0) / 10.0).min(1.0);
        }
        // Steering head swings 20 deg each way while turning, from zero.
        let steer = if i >= 60 { 20.0 * ((i as f64 - 60.0) * 0.1).sin() } else { 0.0 };
        world
            .set_field(head, "transform", "rotation", floats(&[0.0, steer, 0.0]))
            .unwrap();
        let (sy, cy) = (yaw.to_radians().sin(), yaw.to_radians().cos());
        x += step * sy;
        z -= step * cy;
        world
            .set_field(root, "transform", "position", floats(&[x, 2.0, z]))
            .unwrap();
        world
            .set_field(root, "transform", "rotation", floats(&[0.0, yaw, 0.0]))
            .unwrap();
        sys.fixed_update(&mut world, DT).unwrap();
        let lp = world.get_transform(pod).unwrap().position;
        let off = (lp.x * lp.x + lp.z * lp.z).sqrt();
        assert!(
            off < 0.02,
            "pod should stay on the base through the turn, local offset {off} at step {i}"
        );
        // The piston may only move along its axis within the stroke, and must
        // keep the head's orientation (a prismatic joint locks all rotation).
        let pt = world.get_transform(piston).unwrap();
        let d = [pt.position.x, pt.position.y + 0.12, pt.position.z + 0.45];
        let along = d[1] * -0.26 + d[2] * -0.97;
        let perp = (d[0].powi(2) + (d[1] - along * -0.26).powi(2) + (d[2] - along * -0.97).powi(2)).sqrt();
        // The spring holds the piston near rest; the turn must not pump it
        // along its axis (the drift used to walk it to the stroke limit).
        // Within the stroke always; once accelerating stops (step 60) the
        // spring must not be pumped to a stop by the turn.
        let max_along = if i >= 60 { 0.14 } else { 0.16 };
        assert!(
            perp < 0.02 && along.abs() < max_along,
            "piston left its slide or was pumped along it: perp {perp} along {along} at step {i}"
        );
        let q = pt.rotation_quat.unwrap_or([0.0, 0.0, 0.0, 1.0]);
        let tilt = 2.0 * (q[0] * q[0] + q[1] * q[1] + q[2] * q[2]).sqrt().asin();
        assert!(
            tilt < 0.05,
            "piston should keep the head's orientation, tilt {tilt} rad at step {i}"
        );
    }
}

#[test]
fn prismatic_child_stays_put_when_frames_and_steps_disagree() {
    // Scripts pose a kinematic parent once per frame while physics runs on a
    // fixed accumulator, so a frame may run several steps (120/240 Hz) or a
    // step may span several frames (61/90/144 Hz). If a step takes whatever
    // motion accumulated since the last one, the parent's step velocity
    // swings frame to frame and a prismatic child whose free axis lies along
    // the travel direction rings on its spring (the trike's front-wheel
    // piston). With the pending fixed time announced, every step carries an
    // equal share and the stroke stays near zero. The child's written-back
    // local pose must also stay at rest: the parent body trails the scripted
    // pose by the accumulator remainder, and the writeback accounts for it.
    for &hz in &[60.0f64, 61.0, 90.0, 120.0, 144.0, 240.0] {
        let mut world = FlintWorld::new();
        let mut sys = PhysicsSystem::new();

        let base = body(
            &mut world,
            "base",
            [0.0, 2.0, 0.0],
            "kinematic",
            "box",
            [0.2, 0.2, 0.2],
            0.0,
        );
        let piston = body(
            &mut world,
            "piston",
            [0.0, 0.0, -0.5],
            "dynamic",
            "box",
            [0.1, 0.1, 0.1],
            0.0,
        );
        world.set_parent(piston, base).unwrap();
        joint(
            &mut world,
            piston,
            vec![
                ("type", s("prismatic")),
                ("axis", floats(&[0.0, 0.0, -1.0])),
                ("limits", floats(&[-0.15, 0.15])),
                ("motor_stiffness", f(200.0)),
                ("motor_damping", f(10.0)),
            ],
        );
        sys.initialize(&mut world).unwrap();
        for _ in 0..60 {
            sys.fixed_update(&mut world, DT).unwrap();
        }

        // Drive the base at 20 m/s along -Z, one scripted pose per frame,
        // with the frame loop's accumulator deciding the steps.
        let speed = 20.0;
        let frame_dt = 1.0 / hz;
        let mut z = 0.0;
        let mut acc = 0.0f64;
        let mut worst_stroke: f32 = 0.0;
        let mut worst_local: f32 = 0.0;
        let frames = (hz * 4.0) as usize;
        for frame in 0..frames {
            z -= speed * frame_dt;
            world
                .set_field(base, "transform", "position", floats(&[0.0, 2.0, z]))
                .unwrap();
            acc += frame_dt;
            sys.begin_frame(acc / DT);
            while acc >= DT {
                sys.fixed_update(&mut world, DT).unwrap();
                acc -= DT;
            }
            if frame > frames / 4 {
                let stroke = sys.joint_position(piston).expect("prismatic coordinate");
                worst_stroke = worst_stroke.max(stroke.abs());
                let local = world.get_transform(piston).unwrap().position;
                worst_local = worst_local.max((local.z + 0.5).abs()).max(local.x.abs());
            }
        }
        assert!(
            worst_stroke < 0.01,
            "{hz} Hz: piston stroke should stay near zero under a steady parent, worst {worst_stroke} m"
        );
        assert!(
            worst_local < 0.01,
            "{hz} Hz: piston local pose should stay at rest, worst {worst_local} m off"
        );
    }
}

#[test]
fn prismatic_child_stays_put_under_ragged_frame_times() {
    // Same rig as above with frame times that alternate between two rates,
    // as a maximized GPU-bound window delivers. The player writes scripted
    // poses BEFORE its fixed physics loop, so the motion a frame's steps
    // spread matches the accumulator they spread it over. (Physics-first
    // order spreads a pose written with the previous delta over the current
    // accumulator; with these patterns the stroke hits its 0.15 m limit.)
    for &(hz_a, hz_b) in &[(60.0f64, 60.0f64), (45.0, 75.0), (40.0, 90.0), (55.0, 65.0)] {
        let mut world = FlintWorld::new();
        let mut sys = PhysicsSystem::new();
        let base = body(&mut world, "base", [0.0, 2.0, 0.0], "kinematic", "box", [0.2, 0.2, 0.2], 0.0);
        let piston = body(&mut world, "piston", [0.0, 0.0, -0.5], "dynamic", "box", [0.1, 0.1, 0.1], 0.0);
        world.set_parent(piston, base).unwrap();
        joint(&mut world, piston, vec![
            ("type", s("prismatic")),
            ("axis", floats(&[0.0, 0.0, -1.0])),
            ("limits", floats(&[-0.15, 0.15])),
            ("motor_stiffness", f(200.0)),
            ("motor_damping", f(10.0)),
        ]);
        sys.initialize(&mut world).unwrap();
        for _ in 0..60 {
            sys.fixed_update(&mut world, DT).unwrap();
        }
        let speed = 20.0;
        let mut z = 0.0;
        let mut acc = 0.0f64;
        let mut worst_stroke: f32 = 0.0;
        let frames = 240;
        for frame in 0..frames {
            let frame_dt = if frame % 2 == 0 { 1.0 / hz_a } else { 1.0 / hz_b };
            // scripts-first order: clock tick, scripts, then physics
            z -= speed * frame_dt;
            world.set_field(base, "transform", "position", floats(&[0.0, 2.0, z])).unwrap();
            acc += frame_dt;
            sys.begin_frame(acc / DT);
            while acc >= DT {
                sys.fixed_update(&mut world, DT).unwrap();
                acc -= DT;
            }
            if frame > frames / 4 {
                let stroke = sys.joint_position(piston).expect("prismatic coordinate");
                worst_stroke = worst_stroke.max(stroke.abs());
            }
        }
        assert!(
            worst_stroke < 0.01,
            "{hz_a}/{hz_b} Hz alternating: piston stroke should stay near zero, worst {worst_stroke} m"
        );
    }
}

/// Bench-shaped stress: a kinematic TrikeBody (no collider) under a scripted
/// root, with a collider-less hinge pod under a body-less GimbalYaw, driven
/// round a circle while its pitch sweeps through 180 deg with a wrapped motor
/// target. Every step the pod's pose must stay finite and near the body.
///
/// Reproduced the trick bench's vanishing pod before the guard in
/// `project_kinematic_joints`: Rapier's angular motor takes
/// `asin(rel_quat.imag[axis])` with no clamp, and once the hinge twist is
/// near 180 deg a relative quaternion whose norm has drifted a few 1e-6 over
/// 1.0 (kinematic re-seating + turning) gives asin(1.00001) = NaN, which the
/// solver writes into the child body's pose and velocity for good.
#[test]
fn pod_hinge_stays_finite_under_turning_pitching_kinematic_parent() {
    let mut world = FlintWorld::new();
    let mut sys = PhysicsSystem::new();

    let root = world.spawn("root").unwrap();
    world
        .set_component(root, "transform", table(vec![("position", floats(&[0.0, 0.0, 0.0]))]))
        .unwrap();
    let body_id = world.spawn("TrikeBody").unwrap();
    world
        .set_component(body_id, "transform", table(vec![("position", floats(&[0.0, 0.0, 0.0]))]))
        .unwrap();
    world
        .set_component(body_id, "rigidbody", table(vec![("body_type", s("kinematic"))]))
        .unwrap();
    world.set_parent(body_id, root).unwrap();
    let yaw = world.spawn("GimbalYaw").unwrap();
    world
        .set_component(yaw, "transform", table(vec![("position", floats(&[0.0, 0.6, -0.3]))]))
        .unwrap();
    world.set_parent(yaw, body_id).unwrap();
    let pod = world.spawn("GimbalPitch").unwrap();
    world
        .set_component(pod, "transform", table(vec![("position", floats(&[0.0, 0.0, 0.0]))]))
        .unwrap();
    world
        .set_component(
            pod,
            "rigidbody",
            table(vec![("body_type", s("dynamic")), ("gravity_scale", f(0.0))]),
        )
        .unwrap();
    world.set_parent(pod, yaw).unwrap();
    joint(
        &mut world,
        pod,
        vec![
            ("type", s("hinge")),
            ("parent", s("TrikeBody")),
            ("axis", floats(&[1.0, 0.0, 0.0])),
            ("limits", floats(&[0.0, 0.0])),
            ("motor_stiffness", f(60.0)),
            ("motor_damping", f(6.0)),
        ],
    );

    sys.initialize(&mut world).unwrap();
    let mut t = 0.0f64;
    let mut frame = 0usize;
    // Variable frame pacing: whole, fractional and multi-step frames.
    let spans = [1.0, 2.0, 1.5, 1.0, 0.7, 2.3];
    while t < 60.0 {
        let span = spans[frame % spans.len()];
        frame += 1;
        sys.begin_frame(span);
        let steps = (span as f64).floor().max(1.0) as usize;
        for _ in 0..steps {
            t += DT;
            // Root drives a 6 m circle at 12 m/s and bobs; the body pitches
            // +-20 deg on top of a slow sweep up through 180 deg.
            let w = 12.0 / 6.0;
            let (px, pz, yaw_deg) = (6.0 * (w * t).cos(), 6.0 * (w * t).sin(), -(w * t).to_degrees());
            world
                .set_field(root, "transform", "position", floats(&[px, 0.3 * (3.0 * t).sin(), pz]))
                .unwrap();
            world
                .set_field(root, "transform", "rotation", floats(&[0.0, yaw_deg, 0.0]))
                .unwrap();
            let pitch = 20.0 * (1.7 * t).sin() + 170.0 * (0.05 * t).sin().max(0.0);
            world
                .set_field(body_id, "transform", "rotation", floats(&[pitch, 0.0, 0.0]))
                .unwrap();
            // The bench wraps the counter-rotation target to (-180, 180].
            let mut target = -pitch % 360.0;
            if target > 180.0 {
                target -= 360.0;
            }
            if target <= -180.0 {
                target += 360.0;
            }
            world
                .set_field(pod, "joint", "motor_target", f(target))
                .unwrap();
            sys.fixed_update(&mut world, DT).unwrap();

            let p = world.get_world_position(pod).unwrap();
            let q = world
                .get_transform(pod)
                .unwrap()
                .rotation_quat
                .unwrap_or([0.0, 0.0, 0.0, 1.0]);
            assert!(
                p.x.is_finite() && p.y.is_finite() && p.z.is_finite() && q.iter().all(|c| c.is_finite()),
                "pod pose went non-finite at t={t:.3}: pos={p:?} quat={q:?}"
            );
            let b = world.get_world_position(body_id).unwrap();
            let d = ((p.x - b.x).powi(2) + (p.y - b.y).powi(2) + (p.z - b.z).powi(2)).sqrt();
            assert!(d < 2.0, "pod left the body at t={t:.3}: d={d} pod={p:?} body={b:?}");
        }
    }
}

/// A jointed child whose pose has gone non-finite (Rapier motor asin overflow)
/// is re-seated on its parent at the motor target with zero velocity instead
/// of staying NaN for the rest of the session.
#[test]
fn non_finite_jointed_child_is_reseated_at_motor_target() {
    let mut world = FlintWorld::new();
    let mut sys = PhysicsSystem::new();

    let base = body(&mut world, "base", [0.0, 2.0, 0.0], "kinematic", "box", [0.2, 0.2, 0.2], 0.0);
    let arm = world.spawn("arm").unwrap();
    world
        .set_component(arm, "transform", table(vec![("position", floats(&[0.0, 0.0, 0.5]))]))
        .unwrap();
    world
        .set_component(
            arm,
            "rigidbody",
            table(vec![("body_type", s("dynamic")), ("gravity_scale", f(0.0))]),
        )
        .unwrap();
    world.set_parent(arm, base).unwrap();
    joint(
        &mut world,
        arm,
        vec![
            ("type", s("hinge")),
            ("anchor", floats(&[0.0, 0.0, -0.5])),
            ("axis", floats(&[1.0, 0.0, 0.0])),
            ("motor_target", f(30.0)),
            ("motor_stiffness", f(400.0)),
            ("motor_damping", f(25.0)),
        ],
    );
    sys.initialize(&mut world).unwrap();
    for _ in 0..120 {
        sys.fixed_update(&mut world, DT).unwrap();
    }

    let h = sys.sync.body_map[&arm];
    let nan = f32::NAN;
    let b = sys.physics_world.get_rigid_body_mut(h).unwrap();
    b.set_position(
        rapier3d::na::Isometry3::translation(nan, nan, nan),
        true,
    );
    b.set_linvel(rapier3d::na::Vector3::new(nan, nan, nan), true);
    sys.fixed_update(&mut world, DT).unwrap();

    let p = world.get_world_position(arm).unwrap();
    assert!(p.x.is_finite() && p.y.is_finite() && p.z.is_finite(), "arm still non-finite: {p:?}");
    let b = sys.physics_world.get_rigid_body(h).unwrap();
    assert!(b.linvel().iter().all(|c| c.is_finite()) && b.angvel().iter().all(|c| c.is_finite()));
    // Seated at the anchor and near the 30 deg motor target.
    let angle = sys.sync.joint_position(arm, &sys.physics_world).unwrap();
    assert!((angle - 30.0).abs() < 5.0, "re-seated angle {angle} should be near the target");
    assert!((p.y - 2.0).abs() < 0.6 && (p.x).abs() < 0.05, "arm should hang off the base: {p:?}");
}
