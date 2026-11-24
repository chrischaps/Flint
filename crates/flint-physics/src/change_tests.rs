//! A collider's layer, mask and scale changed after its body was made are
//! heard at the next step, and a step where nothing changed only compares
//! (#1221).

use super::*;
use crate::sync::ColliderRereads;
use flint_runtime::EventBus;

fn spawn(world: &mut FlintWorld, name: &str, text: &str) -> EntityId {
    let id = world.spawn(name).unwrap();
    let table: toml::Table = toml::from_str(text).unwrap();
    for (component, data) in table {
        world.set_component(id, &component, data).unwrap();
    }
    id
}

fn y_of(world: &FlintWorld, id: EntityId) -> f32 {
    world.get_transform(id).unwrap().position.y
}

fn steps(sys: &mut PhysicsSystem, world: &mut FlintWorld, n: usize) {
    let mut bus = EventBus::new();
    for _ in 0..n {
        sys.fixed_update(world, 1.0 / 60.0, &mut bus).unwrap();
    }
}

fn set(world: &mut FlintWorld, id: EntityId, component: &str, field: &str, value: &str) {
    let value: toml::Value =
        toml::from_str::<toml::Table>(&format!("v = {value}")).unwrap()["v"].clone();
    world.set_field(id, component, field, value).unwrap();
}

/// A floor that meets `default` and `world`, and so not `ghost`.
const FLOOR: &str = "[transform]\nposition = [0, 0, 0]\n[rigidbody]\nbody_type = \"static\"\n\
                     [collider]\nshape = \"box\"\nsize = [20, 1, 20]\n\
                     layer = \"world\"\nmask = [\"default\", \"world\"]\n";

fn ball(x: f32) -> String {
    format!(
        "[transform]\nposition = [{x}, 4, 0]\n[rigidbody]\nbody_type = \"dynamic\"\n\
         [collider]\nshape = \"sphere\"\nsize = [1, 1, 1]\n"
    )
}

fn world_with_layers() -> (FlintWorld, PhysicsSystem) {
    let mut sys = PhysicsSystem::new();
    sys.set_layers(CollisionLayers::new(&["world", "ghost"]).unwrap());
    (FlintWorld::new(), sys)
}

// ----------------------------------------------------------------------
// Done when: a script that moves a ball to the `ghost` layer mid-fall
// sees it pass through what it would have hit
// ----------------------------------------------------------------------

#[test]
fn a_ball_moved_to_ghost_mid_fall_passes_through_the_floor() {
    let (mut world, mut sys) = world_with_layers();
    spawn(&mut world, "floor", FLOOR);
    let stays = spawn(&mut world, "stays", &ball(-2.0));
    let ghost = spawn(&mut world, "ghost", &ball(2.0));
    sys.initialize(&mut world).unwrap();

    // A third of a second in, both falling and still above the floor
    steps(&mut sys, &mut world, 20);
    assert!(
        y_of(&world, ghost) > 2.0,
        "still falling: y = {}",
        y_of(&world, ghost)
    );
    set(&mut world, ghost, "collider", "layer", "\"ghost\"");
    steps(&mut sys, &mut world, 100);

    assert!(
        (y_of(&world, stays) - 1.0).abs() < 0.05,
        "the ball left alone rests on the floor: y = {}",
        y_of(&world, stays)
    );
    assert!(
        y_of(&world, ghost) < -3.0,
        "the ghost fell through: y = {}",
        y_of(&world, ghost)
    );
}

#[test]
fn a_ghost_that_comes_back_lands_again() {
    // The other way: a ball written on `ghost` falls through until a
    // script puts it back on `default`, then the next floor (which would
    // let a ghost through too) holds it
    let (mut world, mut sys) = world_with_layers();
    spawn(&mut world, "floor", FLOOR);
    spawn(
        &mut world,
        "low",
        &FLOOR.replace("position = [0, 0, 0]", "position = [0, -6, 0]"),
    );
    let id = spawn(
        &mut world,
        "ball",
        &format!("{}layer = \"ghost\"\n", ball(0.0)),
    );
    sys.initialize(&mut world).unwrap();
    steps(&mut sys, &mut world, 60);
    let y = y_of(&world, id);
    assert!(
        y < 0.0 && y > -5.0,
        "through the first floor, over the second: y = {y}"
    );
    set(&mut world, id, "collider", "layer", "\"default\"");
    steps(&mut sys, &mut world, 120);
    assert!(
        (y_of(&world, id) - -5.0).abs() < 0.05,
        "rests on the low floor: y = {}",
        y_of(&world, id)
    );
}

#[test]
fn a_mask_written_later_is_heard_and_wakes_a_body_at_rest() {
    let (mut world, mut sys) = world_with_layers();
    spawn(&mut world, "floor", FLOOR);
    let id = spawn(&mut world, "ball", &ball(0.0));
    sys.initialize(&mut world).unwrap();
    // Asleep on the floor after three seconds
    steps(&mut sys, &mut world, 180);
    assert!((y_of(&world, id) - 1.0).abs() < 0.05);
    // Its mask now leaves the floor's layer out: it falls
    set(&mut world, id, "collider", "mask", "[\"ghost\"]");
    steps(&mut sys, &mut world, 60);
    assert!(y_of(&world, id) < -1.0, "fell: y = {}", y_of(&world, id));
}

#[test]
fn an_unknown_layer_written_later_is_reported_and_reads_default() {
    let (mut world, mut sys) = world_with_layers();
    spawn(&mut world, "floor", FLOOR);
    // Starts on `ghost`, which would fall through
    let id = spawn(
        &mut world,
        "ball",
        &format!("{}layer = \"ghost\"\n", ball(0.0)),
    );
    sys.initialize(&mut world).unwrap();
    steps(&mut sys, &mut world, 10);
    set(&mut world, id, "collider", "layer", "\"gohst\"");
    steps(&mut sys, &mut world, 120);
    // `default` meets the floor: it lands
    assert!(
        (y_of(&world, id) - 1.0).abs() < 0.05,
        "y = {}",
        y_of(&world, id)
    );
}

// ----------------------------------------------------------------------
// Done when: a script that doubles a crate's scale makes its collider
// twice the size the next step
// ----------------------------------------------------------------------

const CRATE: &str = "[transform]\nposition = [0, 0, 0]\n[rigidbody]\nbody_type = \"static\"\n\
                     [collider]\nshape = \"box\"\nsize = [2, 2, 2]\n";

fn half_extents(sys: &PhysicsSystem, id: EntityId) -> [f32; 3] {
    match sys.get_entity_collider_extents(id) {
        Some(ColliderExtents::Box { half_extents }) => half_extents,
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_crate_scaled_twice_is_twice_the_size_the_next_step() {
    let mut world = FlintWorld::new();
    let mut sys = PhysicsSystem::new();
    let crate_ = spawn(&mut world, "crate", CRATE);
    // A ball resting on its top (y = 1), so the step after is seen too
    let ball = spawn(
        &mut world,
        "ball",
        "[transform]\nposition = [0, 3, 0]\n[rigidbody]\nbody_type = \"dynamic\"\n\
         [collider]\nshape = \"sphere\"\nsize = [1, 1, 1]\n",
    );
    sys.initialize(&mut world).unwrap();
    steps(&mut sys, &mut world, 180);
    assert_eq!(half_extents(&sys, crate_), [1.0, 1.0, 1.0]);
    assert!((y_of(&world, ball) - 1.5).abs() < 0.05);

    set(&mut world, crate_, "transform", "scale", "[2.0, 2.0, 2.0]");
    steps(&mut sys, &mut world, 1);
    assert_eq!(half_extents(&sys, crate_), [2.0, 2.0, 2.0]);

    // The ball, woken inside the grown crate, is pushed out onto its new
    // top (y = 2, plus its radius)
    steps(&mut sys, &mut world, 120);
    assert!(
        (y_of(&world, ball) - 2.5).abs() < 0.05,
        "on the grown crate: y = {}",
        y_of(&world, ball)
    );
}

#[test]
fn a_parent_scaled_later_rescales_its_childs_collider_and_its_offset() {
    let mut world = FlintWorld::new();
    let mut sys = PhysicsSystem::new();
    let group = spawn(&mut world, "group", "[transform]\nscale = [1, 1, 1]\n");
    let pebble = spawn(
        &mut world,
        "pebble",
        "[transform]\nposition = [0, 0, 0]\n[rigidbody]\nbody_type = \"static\"\n\
         [collider]\nshape = \"sphere\"\nsize = [1, 1, 1]\noffset = [0, 1, 0]\n",
    );
    world.set_parent(pebble, group).unwrap();
    sys.initialize(&mut world).unwrap();
    steps(&mut sys, &mut world, 1);
    set(&mut world, group, "transform", "scale", "[3, 3, 3]");
    steps(&mut sys, &mut world, 1);
    match sys.get_entity_collider_extents(pebble) {
        Some(ColliderExtents::Sphere { radius }) => assert_eq!(radius, 1.5),
        other => panic!("{other:?}"),
    }
    // The offset is scaled too: the sphere's centre is 3 m up
    let handle = sys.sync.collider_map[&pebble];
    let c = sys.physics_world.collider_set.get(handle).unwrap();
    let y = c.position_wrt_parent().unwrap().translation.y;
    assert!((y - 3.0).abs() < 1e-5, "offset y = {y}");
}

#[test]
fn a_scale_moved_within_a_part_in_a_thousand_keeps_its_shape() {
    let mut world = FlintWorld::new();
    let mut sys = PhysicsSystem::new();
    let crate_ = spawn(&mut world, "crate", CRATE);
    sys.initialize(&mut world).unwrap();
    steps(&mut sys, &mut world, 1);
    set(
        &mut world,
        crate_,
        "transform",
        "scale",
        "[1.0004, 1.0004, 1.0004]",
    );
    steps(&mut sys, &mut world, 1);
    assert_eq!(half_extents(&sys, crate_), [1.0, 1.0, 1.0]);
    assert_eq!(sys.sync.rereads.shapes, 0);
    // Creeping on, it is compared against the scale it was made at, not
    // the last one seen: past the tolerance it is remade
    set(
        &mut world,
        crate_,
        "transform",
        "scale",
        "[1.002, 1.002, 1.002]",
    );
    steps(&mut sys, &mut world, 1);
    assert_eq!(sys.sync.rereads.shapes, 1);
    assert!((half_extents(&sys, crate_)[0] - 1.002).abs() < 1e-6);
}

#[test]
fn a_scaled_mesh_collider_is_remade_from_the_cache() {
    let mut world = FlintWorld::new();
    let mut sys = PhysicsSystem::new();
    let a = spawn(
        &mut world,
        "a",
        "[transform]\nposition = [0, 0, 0]\n[rigidbody]\nbody_type = \"static\"\n\
         [collider]\nshape = \"convex\"\n",
    );
    let b = spawn(
        &mut world,
        "b",
        "[transform]\nposition = [5, 0, 0]\nscale = [2, 2, 2]\n[rigidbody]\nbody_type = \"static\"\n\
         [collider]\nshape = \"convex\"\n",
    );
    let cube = Arc::new(ColliderMesh {
        positions: vec![
            [-0.5, -0.5, -0.5],
            [0.5, -0.5, -0.5],
            [0.5, 0.5, -0.5],
            [-0.5, 0.5, -0.5],
            [-0.5, -0.5, 0.5],
            [0.5, -0.5, 0.5],
            [0.5, 0.5, 0.5],
            [-0.5, 0.5, 0.5],
        ],
        triangles: vec![],
    });
    sys.set_collider_mesh(a, cube.clone());
    sys.set_collider_mesh(b, cube);
    sys.initialize(&mut world).unwrap();
    steps(&mut sys, &mut world, 1);
    let shape_of = |sys: &PhysicsSystem, id: EntityId| {
        let h = sys.sync.collider_map[&id];
        sys.physics_world
            .collider_set
            .get(h)
            .unwrap()
            .shared_shape()
            .clone()
    };
    // `a` grown to `b`'s scale takes `b`'s hull, the same one
    set(&mut world, a, "transform", "scale", "[2, 2, 2]");
    steps(&mut sys, &mut world, 1);
    assert!(Arc::ptr_eq(&shape_of(&sys, a).0, &shape_of(&sys, b).0));
    let aabb = shape_of(&sys, a).compute_local_aabb();
    assert!((aabb.maxs.x - 1.0).abs() < 1e-5, "{aabb:?}");
}

#[test]
fn a_collider_made_again_from_late_triangles_still_hears_its_scale() {
    // #1222 makes a collider again when its triangles come after its body
    // (a new handle): the new one is watched as the first was
    let mut world = FlintWorld::new();
    let mut sys = PhysicsSystem::new();
    let rock = spawn(
        &mut world,
        "rock",
        "[transform]\nposition = [0, 0, 0]\n[rigidbody]\nbody_type = \"static\"\n\
         [collider]\nshape = \"convex\"\nsize = [0.2, 0.2, 0.2]\n",
    );
    sys.initialize(&mut world).unwrap();
    let first = sys.sync.collider_map[&rock];
    sys.set_collider_mesh(
        rock,
        Arc::new(ColliderMesh {
            positions: vec![
                [-0.5, -0.5, -0.5],
                [0.5, -0.5, -0.5],
                [0.5, 0.5, -0.5],
                [-0.5, 0.5, -0.5],
                [-0.5, -0.5, 0.5],
                [0.5, -0.5, 0.5],
                [0.5, 0.5, 0.5],
                [-0.5, 0.5, 0.5],
            ],
            triangles: vec![],
        }),
    );
    steps(&mut sys, &mut world, 1);
    let remade = sys.sync.collider_map[&rock];
    assert_ne!(remade, first, "made again on a new handle");
    set(&mut world, rock, "transform", "scale", "[3, 3, 3]");
    steps(&mut sys, &mut world, 1);
    let h = sys.sync.collider_map[&rock];
    let aabb = sys
        .physics_world
        .collider_set
        .get(h)
        .unwrap()
        .shared_shape()
        .compute_local_aabb();
    assert!(
        (aabb.maxs.x - 1.5).abs() < 1e-5,
        "the 1 m hull at 3x: {aabb:?}"
    );
}

// ----------------------------------------------------------------------
// Done when: a frame where nothing changed reads no more than it does now
// ----------------------------------------------------------------------

#[test]
fn a_step_where_nothing_changed_resolves_and_remakes_nothing() {
    let (mut world, mut sys) = world_with_layers();
    spawn(&mut world, "floor", FLOOR);
    let group = spawn(&mut world, "group", "[transform]\nscale = [1, 1, 1]\n");
    for i in 0..8 {
        let id = spawn(&mut world, &format!("ball{i}"), &ball(i as f32 * 1.5 - 6.0));
        if i % 2 == 0 {
            world.set_parent(id, group).unwrap();
        }
    }
    sys.initialize(&mut world).unwrap();
    // Falling and landing: dynamic bodies write their poses back every
    // step, which must not read as anything written: no collider is even
    // looked at
    steps(&mut sys, &mut world, 120);
    assert_eq!(sys.sync.rereads, ColliderRereads::default());

    // One change is one look and one resolve, and then quiet again
    let ball0 = world.get_id("ball0").unwrap();
    set(&mut world, ball0, "collider", "layer", "\"world\"");
    steps(&mut sys, &mut world, 30);
    assert_eq!(
        sys.sync.rereads,
        ColliderRereads {
            looks: 1,
            groups: 1,
            scales: 0,
            shapes: 0
        }
    );
    // A parent's scale asks each of its four children once
    set(&mut world, group, "transform", "scale", "[1.5, 1.5, 1.5]");
    steps(&mut sys, &mut world, 30);
    assert_eq!(
        sys.sync.rereads,
        ColliderRereads {
            looks: 5,
            groups: 1,
            scales: 4,
            shapes: 4
        }
    );
}

/// What the old pass did, `update_sensor_flags` before #1221: the
/// control the bench times the new one against.
fn sensor_flags_only(sync: &PhysicsSync, world: &FlintWorld, physics: &mut PhysicsWorld) {
    for (entity_id, col_handle) in &sync.collider_map {
        let col = world.view(*entity_id, comp::COLLIDER);
        if !col.exists() {
            continue;
        }
        let wants_sensor = col.bool("is_sensor", false);
        let fixed = physics.is_fixed_collider(*col_handle);
        if let Some(collider) = physics.collider_set.get_mut(*col_handle) {
            if collider.is_sensor() != wants_sensor {
                collider.set_sensor(wants_sensor);
                if fixed {
                    physics.touch_static();
                }
            }
        }
    }
}

/// The cost of the per-step pass over 2000 colliders where nothing
/// changed, the old pass against the new, alternated so a busy machine
/// slows both alike. `cargo test -p flint-physics --release
/// collider_pass_cost -- --ignored --nocapture`
#[test]
#[ignore]
fn collider_pass_cost() {
    let (mut world, mut sys) = world_with_layers();
    let group = spawn(&mut world, "group", "[transform]\nscale = [1, 1, 1]\n");
    for i in 0..2000 {
        let text = format!(
            "[transform]\nposition = [{}, 0, {}]\n[rigidbody]\nbody_type = \"static\"\n\
             [collider]\nshape = \"box\"\nsize = [1, 1, 1]\n{}",
            (i % 50) as f32 * 2.0,
            (i / 50) as f32 * 2.0,
            if i % 3 == 0 {
                "layer = \"world\"\nmask = [\"default\", \"world\"]\n"
            } else {
                ""
            }
        );
        let id = spawn(&mut world, &format!("c{i}"), &text);
        if i % 4 == 0 {
            world.set_parent(id, group).unwrap();
        }
    }
    sys.initialize(&mut world).unwrap();
    let rounds = 30;
    let reps = 50;
    let (mut old, mut new) = (Vec::new(), Vec::new());
    for _ in 0..rounds {
        let t = std::time::Instant::now();
        for _ in 0..reps {
            sensor_flags_only(&sys.sync, &world, &mut sys.physics_world);
        }
        old.push(t.elapsed().as_secs_f64() / reps as f64);
        let t = std::time::Instant::now();
        for _ in 0..reps {
            sys.sync.update_colliders(&world, &mut sys.physics_world);
        }
        new.push(t.elapsed().as_secs_f64() / reps as f64);
    }
    assert_eq!(sys.sync.rereads, ColliderRereads::default());
    let min = |v: &[f64]| v.iter().cloned().fold(f64::MAX, f64::min) * 1e6;
    let median = |v: &mut Vec<f64>| {
        v.sort_by(|a, b| a.partial_cmp(b).unwrap());
        v[v.len() / 2] * 1e6
    };
    println!(
        "collider pass, 2000 colliders, nothing changed: old min {:.1} us median {:.1} us; new min {:.1} us median {:.1} us",
        min(&old),
        median(&mut old.clone()),
        min(&new),
        median(&mut new.clone())
    );
}
