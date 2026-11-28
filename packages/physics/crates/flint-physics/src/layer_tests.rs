//! Collision layers, masked rays and moves, mesh and convex colliders, and
//! colliders that take their entity's scale (#729).

use super::*;
use flint_runtime::EventBus;

fn spawn(world: &mut FlintWorld, name: &str, text: &str) -> EntityId {
    let id = world.spawn(name).unwrap();
    let table: toml::Table = toml::from_str(text).unwrap();
    for (component, data) in table {
        world.set_component(id, &component, data).unwrap();
    }
    id
}

fn layers(names: &[&str]) -> CollisionLayers {
    CollisionLayers::new(names).unwrap()
}

fn y_of(world: &FlintWorld, id: EntityId) -> f32 {
    world.get_transform(id).unwrap().position.y
}

/// Step `seconds` at 60 Hz, gathering every event.
fn run(sys: &mut PhysicsSystem, world: &mut FlintWorld, seconds: f32) -> Vec<GameEvent> {
    let mut bus = EventBus::new();
    for _ in 0..(seconds * 60.0) as usize {
        sys.fixed_update(world, 1.0 / 60.0, &mut bus).unwrap();
    }
    bus.drain()
}

const FLOOR: &str = "[transform]\nposition = [0, 0, 0]\n[rigidbody]\nbody_type = \"static\"\n\
                     [collider]\nshape = \"box\"\nsize = [20, 1, 20]\n";

fn ball(x: f32, collider_extra: &str) -> String {
    format!(
        "[transform]\nposition = [{x}, 3, 0]\n[rigidbody]\nbody_type = \"dynamic\"\n\
         [collider]\nshape = \"sphere\"\nsize = [1, 1, 1]\n{collider_extra}"
    )
}

// ----------------------------------------------------------------------
// Done when: two bodies on non-matching layers pass through each other
// ----------------------------------------------------------------------

#[test]
fn bodies_on_layers_that_do_not_meet_pass_through_each_other() {
    let mut world = FlintWorld::new();
    let mut sys = PhysicsSystem::new();
    sys.set_layers(layers(&["world", "ghost"]));

    spawn(
        &mut world,
        "floor",
        &format!("{FLOOR}layer = \"world\"\nmask = [\"default\", \"world\"]\n"),
    );
    // Meets the world: lands on the floor (its top is y = 0.5)
    let landed = spawn(&mut world, "landed", &ball(-2.0, ""));
    // On `ghost`, which the floor's mask leaves out: falls through
    let ghost = spawn(&mut world, "ghost", &ball(2.0, "layer = \"ghost\"\n"));

    sys.initialize(&mut world).unwrap();
    let events = run(&mut sys, &mut world, 2.0);

    assert!(
        (y_of(&world, landed) - 1.0).abs() < 0.05,
        "the default-layer ball rests on the floor: y = {}",
        y_of(&world, landed)
    );
    assert!(
        y_of(&world, ghost) < -3.0,
        "the ghost fell through: y = {}",
        y_of(&world, ghost)
    );
    let ghost_touched = events.iter().any(|e| {
        matches!(e, GameEvent::CollisionStarted { entity_a, entity_b }
            if *entity_a == ghost || *entity_b == ghost)
    });
    assert!(!ghost_touched, "no contact is reported for the ghost");
}

#[test]
fn the_test_is_both_ways_so_one_mask_is_enough() {
    let mut world = FlintWorld::new();
    let mut sys = PhysicsSystem::new();
    sys.set_layers(layers(&["bullet"]));
    // The floor meets everything; the bullet's mask leaves the default
    // layer out, so the two never touch
    spawn(&mut world, "floor", FLOOR);
    let bullet = spawn(
        &mut world,
        "bullet",
        &ball(0.0, "layer = \"bullet\"\nmask = [\"bullet\"]\n"),
    );
    sys.initialize(&mut world).unwrap();
    run(&mut sys, &mut world, 2.0);
    assert!(y_of(&world, bullet) < -3.0);
}

#[test]
fn a_trigger_on_a_layer_a_mask_leaves_out_says_nothing() {
    let mut world = FlintWorld::new();
    let mut sys = PhysicsSystem::new();
    sys.set_layers(layers(&["trigger", "bullet"]));
    spawn(
        &mut world,
        "zone",
        "[transform]\nposition = [0, 0, 0]\n[rigidbody]\nbody_type = \"static\"\n\
         [collider]\nshape = \"box\"\nsize = [4, 4, 4]\nis_sensor = true\nlayer = \"trigger\"\n",
    );
    let bullet = spawn(
        &mut world,
        "bullet",
        &ball(
            0.0,
            "layer = \"bullet\"\nmask = [\"default\", \"bullet\"]\n",
        ),
    );
    let rock = spawn(&mut world, "rock", &ball(1.0, ""));
    sys.initialize(&mut world).unwrap();
    let events = run(&mut sys, &mut world, 1.5);
    let entered: Vec<EntityId> = events
        .iter()
        .filter_map(|e| match e {
            GameEvent::TriggerEntered { entity, .. } => Some(*entity),
            _ => None,
        })
        .collect();
    assert!(
        entered.contains(&rock),
        "the rock enters the zone: {entered:?}"
    );
    assert!(
        !entered.contains(&bullet),
        "the bullet ignores it: {entered:?}"
    );
}

#[test]
fn an_unknown_layer_is_reported_and_read_as_default() {
    let mut world = FlintWorld::new();
    let mut sys = PhysicsSystem::new();
    sys.set_layers(layers(&["player", "pickup"]));
    spawn(&mut world, "floor", FLOOR);
    let typo = spawn(
        &mut world,
        "typo",
        &ball(0.0, "layer = \"playr\"\nmask = [\"default\", \"pikcup\"]\n"),
    );
    sys.initialize(&mut world).unwrap();
    let reports = world.read_reports().snapshot();
    let text: Vec<String> = reports.iter().map(|d| d.one_line()).collect();
    assert!(
        text.iter()
            .any(|t| t.contains("collider.layer") && t.contains("player")),
        "{text:#?}"
    );
    assert!(
        text.iter()
            .any(|t| t.contains("collider.mask[1]") && t.contains("pickup")),
        "{text:#?}"
    );
    // On `default`, meeting `default`: it lands
    run(&mut sys, &mut world, 2.0);
    assert!((y_of(&world, typo) - 1.0).abs() < 0.05);
}

// ----------------------------------------------------------------------
// Done when: a script `raycast` can take a layer mask (the engine half;
// the script function is flint-script's)
// ----------------------------------------------------------------------

#[test]
fn a_masked_ray_passes_what_its_mask_leaves_out() {
    let mut world = FlintWorld::new();
    let mut sys = PhysicsSystem::new();
    sys.set_layers(layers(&["glass", "wall"]));
    let wall = spawn(
        &mut world,
        "wall",
        "[transform]\nposition = [10, 0, 0]\n[rigidbody]\nbody_type = \"static\"\n\
         [collider]\nshape = \"box\"\nsize = [1, 4, 4]\nlayer = \"wall\"\n",
    );
    let glass = spawn(
        &mut world,
        "glass",
        "[transform]\nposition = [5, 0, 0]\n[rigidbody]\nbody_type = \"static\"\n\
         [collider]\nshape = \"box\"\nsize = [0.1, 4, 4]\nlayer = \"glass\"\n",
    );
    sys.initialize(&mut world).unwrap();
    sys.step(&mut world, 1.0 / 60.0);

    let hit = sys.raycast([0.0; 3], [1.0, 0.0, 0.0], 50.0, None).unwrap();
    assert_eq!(hit.entity_id, glass, "unmasked, the glass is first");

    let walls = sys.layer_mask(&["wall"]).unwrap();
    let hit = sys
        .raycast_masked([0.0; 3], [1.0, 0.0, 0.0], 50.0, None, Some(walls))
        .unwrap();
    assert_eq!(hit.entity_id, wall);
    assert!((hit.distance - 9.5).abs() < 1e-3, "{}", hit.distance);

    let nothing = sys.layer_mask(&["default"]).unwrap();
    assert!(sys
        .raycast_masked([0.0; 3], [1.0, 0.0, 0.0], 50.0, None, Some(nothing))
        .is_none());
    assert!(sys.layer_mask(&["wal"]).is_err());
}

#[test]
fn a_character_walks_through_what_its_mask_leaves_out() {
    let mut world = FlintWorld::new();
    let mut sys = PhysicsSystem::new();
    sys.set_layers(layers(&["player", "pickup"]));
    let player = spawn(
        &mut world,
        "player",
        "[transform]\nposition = [0, 1, 0]\n[rigidbody]\nbody_type = \"kinematic\"\n\
         [collider]\nshape = \"capsule\"\nsize = [0.6, 1.8, 0.6]\n\
         layer = \"player\"\nmask = [\"default\", \"player\"]\n",
    );
    // A crate on the default layer, and a coin the player's mask leaves out
    spawn(
        &mut world,
        "crate",
        "[transform]\nposition = [0, 1, 3]\n[rigidbody]\nbody_type = \"static\"\n\
         [collider]\nshape = \"box\"\nsize = [1, 2, 1]\n",
    );
    spawn(
        &mut world,
        "coin",
        "[transform]\nposition = [3, 1, 0]\n[rigidbody]\nbody_type = \"static\"\n\
         [collider]\nshape = \"box\"\nsize = [1, 2, 1]\nlayer = \"pickup\"\n",
    );
    sys.initialize(&mut world).unwrap();
    sys.step(&mut world, 1.0 / 60.0);

    let toward_crate = sys
        .move_character_shape(player, [0.0, 1.0, 0.0], [0.0, 0.0, 5.0], 1.0 / 60.0)
        .unwrap();
    assert!(
        toward_crate.position[2] < 2.4,
        "stopped by the crate: {:?}",
        toward_crate.position
    );
    let through_coin = sys
        .move_character_shape(player, [0.0, 1.0, 0.0], [5.0, 0.0, 0.0], 1.0 / 60.0)
        .unwrap();
    assert!(
        (through_coin.position[0] - 5.0).abs() < 1e-3,
        "through the coin: {:?}",
        through_coin.position
    );
    // A mask given for the move replaces the collider's
    let all = sys.layer_mask(&["default", "player", "pickup"]).unwrap();
    let stopped = sys
        .move_character_masked(
            player,
            [0.0, 1.0, 0.0],
            [5.0, 0.0, 0.0],
            1.0 / 60.0,
            Some(all),
        )
        .unwrap();
    assert!(stopped.position[0] < 2.4, "{:?}", stopped.position);
}

// ----------------------------------------------------------------------
// Done when: a uniformly scaled collider matches its drawn size
// ----------------------------------------------------------------------

#[test]
fn a_uniformly_scaled_box_is_its_drawn_size() {
    let mut world = FlintWorld::new();
    let mut sys = PhysicsSystem::new();
    // A unit box drawn at 3x: a 3 m crate whose top is at y = 1.5
    let crate_ = spawn(
        &mut world,
        "crate",
        "[transform]\nposition = [0, 0, 0]\nscale = [3, 3, 3]\n[rigidbody]\nbody_type = \"static\"\n\
         [collider]\nshape = \"box\"\nsize = [1, 1, 1]\n",
    );
    let ball = spawn(&mut world, "ball", &ball(0.0, ""));
    sys.initialize(&mut world).unwrap();
    match sys.get_entity_collider_extents(crate_) {
        Some(ColliderExtents::Box { half_extents }) => assert_eq!(half_extents, [1.5, 1.5, 1.5]),
        other => panic!("{other:?}"),
    }
    run(&mut sys, &mut world, 2.0);
    assert!(
        (y_of(&world, ball) - 2.0).abs() < 0.05,
        "rests on the drawn top: y = {}",
        y_of(&world, ball)
    );
}

#[test]
fn a_scaled_sphere_capsule_and_offset_scale_with_it() {
    let mut world = FlintWorld::new();
    let mut sys = PhysicsSystem::new();
    let sphere = spawn(
        &mut world,
        "sphere",
        "[transform]\nscale = [2, 2, 2]\n[rigidbody]\nbody_type = \"static\"\n\
         [collider]\nshape = \"sphere\"\nsize = [1, 1, 1]\n",
    );
    // Non-uniform, but round in x and z and unrotated: exact
    let capsule = spawn(
        &mut world,
        "capsule",
        "[transform]\nposition = [10, 0, 0]\nscale = [2, 3, 2]\n[rigidbody]\nbody_type = \"static\"\n\
         [collider]\nshape = \"capsule\"\nsize = [1, 2, 1]\noffset = [0, 1, 0]\n",
    );
    sys.initialize(&mut world).unwrap();
    match sys.get_entity_collider_extents(sphere) {
        Some(ColliderExtents::Sphere { radius }) => assert_eq!(radius, 1.0),
        other => panic!("{other:?}"),
    }
    match sys.get_entity_collider_extents(capsule) {
        // Radius 0.5 * 2; total height 2 * 3 = 6, so half height 3 - 1
        Some(ColliderExtents::Capsule {
            radius,
            half_height,
        }) => {
            assert_eq!(radius, 1.0);
            assert_eq!(half_height, 2.0);
        }
        other => panic!("{other:?}"),
    }
    sys.step(&mut world, 1.0 / 60.0);
    // The offset is a point in the entity's frame: 1 up is 3 m up
    let hit = sys
        .raycast([10.0, 20.0, 0.0], [0.0, -1.0, 0.0], 50.0, None)
        .unwrap();
    assert!(
        (hit.point[1] - 6.0).abs() < 1e-3,
        "top at 3 + 3: {:?}",
        hit.point
    );
}

#[test]
fn a_box_takes_a_non_uniform_scale_exactly() {
    let mut world = FlintWorld::new();
    let mut sys = PhysicsSystem::new();
    let plank = spawn(
        &mut world,
        "plank",
        "[transform]\nscale = [4, 0.5, 1]\n[rigidbody]\nbody_type = \"static\"\n\
         [collider]\nshape = \"box\"\nsize = [1, 1, 1]\n",
    );
    sys.initialize(&mut world).unwrap();
    match sys.get_entity_collider_extents(plank) {
        Some(ColliderExtents::Box { half_extents }) => assert_eq!(half_extents, [2.0, 0.25, 0.5]),
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_scaled_parent_scales_its_childs_collider_and_write_back() {
    let mut world = FlintWorld::new();
    let mut sys = PhysicsSystem::new();
    let group = spawn(&mut world, "group", "[transform]\nscale = [2, 2, 2]\n");
    spawn(&mut world, "floor", FLOOR);
    let pebble = spawn(&mut world, "pebble", &ball(0.0, ""));
    world.set_parent(pebble, group).unwrap();
    // The pebble's local y 3 is 6 in the world; radius 0.5 is 1
    sys.initialize(&mut world).unwrap();
    run(&mut sys, &mut world, 3.0);
    // Resting on the floor's top (0.5) with radius 1: world y 1.5, local 0.75
    let local = world.get_transform(pebble).unwrap().position.y;
    assert!((local - 0.75).abs() < 0.03, "local y = {local}");
}

// ----------------------------------------------------------------------
// Done when: a model entity can get a mesh collider from TOML alone (the
// engine half: the triangles a host hands over become the shape; the
// player reads them from the model)
// ----------------------------------------------------------------------

/// A ramp's two triangles and a box's twelve, as a model would give them.
fn wedge() -> ColliderMesh {
    // A 2 x 1 x 2 block with its base at y = 0
    let p = [
        [-1.0, 0.0, -1.0],
        [1.0, 0.0, -1.0],
        [1.0, 0.0, 1.0],
        [-1.0, 0.0, 1.0],
        [-1.0, 1.0, -1.0],
        [1.0, 1.0, -1.0],
        [1.0, 1.0, 1.0],
        [-1.0, 1.0, 1.0],
    ];
    let t = [
        [0, 2, 1],
        [0, 3, 2],
        [4, 5, 6],
        [4, 6, 7],
        [0, 1, 5],
        [0, 5, 4],
        [1, 2, 6],
        [1, 6, 5],
        [2, 3, 7],
        [2, 7, 6],
        [3, 0, 4],
        [3, 4, 7],
    ];
    ColliderMesh {
        positions: p.to_vec(),
        triangles: t.to_vec(),
    }
}

#[test]
fn mesh_requests_name_the_model_or_the_colliders_own() {
    let mut world = FlintWorld::new();
    let rock = spawn(
        &mut world,
        "rock",
        "[model]\nasset = \"rock\"\n[rigidbody]\nbody_type = \"static\"\n[collider]\nshape = \"mesh\"\n",
    );
    let statue = spawn(
        &mut world,
        "statue",
        "[model]\nasset = \"statue\"\n[rigidbody]\nbody_type = \"static\"\n\
         [collider]\nshape = \"convex\"\nmodel = \"statue_hull\"\n",
    );
    spawn(
        &mut world,
        "crate",
        "[model]\nasset = \"crate\"\n[rigidbody]\nbody_type = \"static\"\n[collider]\nshape = \"box\"\n",
    );
    let mut wanted = PhysicsSystem::collider_mesh_requests(&world);
    wanted.sort();
    assert_eq!(
        wanted,
        vec![
            (rock, "rock".to_string()),
            (statue, "statue_hull".to_string())
        ]
    );
}

#[test]
fn a_mesh_collider_is_its_models_triangles_at_its_scale() {
    let mut world = FlintWorld::new();
    let mut sys = PhysicsSystem::new();
    // Drawn at 2x: 4 m wide, its top at y = 2
    let block = spawn(
        &mut world,
        "block",
        "[transform]\nscale = [2, 2, 2]\n[model]\nasset = \"wedge\"\n\
         [rigidbody]\nbody_type = \"static\"\n[collider]\nshape = \"mesh\"\n",
    );
    let ball = spawn(&mut world, "ball", &ball(1.5, ""));
    sys.set_collider_mesh(block, Arc::new(wedge()));
    sys.initialize(&mut world).unwrap();
    match sys.get_entity_collider_extents(block) {
        Some(ColliderExtents::Mesh {
            half_extents,
            centre,
        }) => {
            assert_eq!(half_extents, [2.0, 1.0, 2.0]);
            assert_eq!(centre, [0.0, 1.0, 0.0]);
        }
        other => panic!("{other:?}"),
    }
    run(&mut sys, &mut world, 2.0);
    assert!(
        (y_of(&world, ball) - 2.5).abs() < 0.05,
        "rests on the mesh's top: y = {}",
        y_of(&world, ball)
    );
}

#[test]
fn a_dynamic_mesh_is_made_convex_and_lands_on_a_mesh_floor() {
    let mut world = FlintWorld::new();
    let mut sys = PhysicsSystem::new();
    let floor = spawn(
        &mut world,
        "floor",
        "[transform]\nscale = [10, 1, 10]\n[rigidbody]\nbody_type = \"static\"\n[collider]\nshape = \"mesh\"\n",
    );
    let block = spawn(
        &mut world,
        "block",
        "[transform]\nposition = [0, 3, 0]\n[rigidbody]\nbody_type = \"dynamic\"\n[collider]\nshape = \"mesh\"\n",
    );
    let mesh = Arc::new(wedge());
    sys.set_collider_mesh(floor, mesh.clone());
    sys.set_collider_mesh(block, mesh);
    sys.initialize(&mut world).unwrap();
    assert!(matches!(
        sys.get_entity_collider_extents(block),
        Some(ColliderExtents::Convex { .. })
    ));
    let reports: Vec<String> = world
        .read_reports()
        .snapshot()
        .iter()
        .map(|d| d.one_line())
        .collect();
    assert!(
        reports.iter().any(|r| r.contains("made `convex`")),
        "{reports:#?}"
    );
    run(&mut sys, &mut world, 2.0);
    assert!(
        (y_of(&world, block) - 1.0).abs() < 0.05,
        "the block's base rests on the floor's top: y = {}",
        y_of(&world, block)
    );
}

#[test]
fn a_mesh_with_no_triangles_is_a_box_and_says_so() {
    let mut world = FlintWorld::new();
    let mut sys = PhysicsSystem::new();
    let lost = spawn(
        &mut world,
        "lost",
        "[rigidbody]\nbody_type = \"static\"\n[collider]\nshape = \"convex\"\nsize = [2, 2, 2]\n",
    );
    sys.initialize(&mut world).unwrap();
    assert!(matches!(
        sys.get_entity_collider_extents(lost),
        Some(ColliderExtents::Box {
            half_extents: [1.0, 1.0, 1.0]
        })
    ));
    let reports: Vec<String> = world
        .read_reports()
        .snapshot()
        .iter()
        .map(|d| d.one_line())
        .collect();
    assert!(
        reports.iter().any(|r| r.contains("has no triangles")),
        "{reports:#?}"
    );
}

#[test]
fn crates_of_one_model_and_scale_share_one_shape() {
    let mut world = FlintWorld::new();
    let mut sys = PhysicsSystem::new();
    let mesh = Arc::new(wedge());
    let mut ids = Vec::new();
    for i in 0..3 {
        let id = spawn(
            &mut world,
            &format!("crate{i}"),
            &format!(
                "[transform]\nposition = [{}, 0, 0]\n[rigidbody]\nbody_type = \"static\"\n\
                 [collider]\nshape = \"convex\"\n",
                i * 5
            ),
        );
        sys.set_collider_mesh(id, mesh.clone());
        ids.push(id);
    }
    sys.initialize(&mut world).unwrap();
    let shape_ptr = |id: &EntityId| {
        let h = sys.sync.collider_map[id];
        let shape = sys.physics_world.collider_set[h].shared_shape();
        Arc::as_ptr(&shape.0) as *const () as usize
    };
    assert_eq!(shape_ptr(&ids[0]), shape_ptr(&ids[1]));
    assert_eq!(shape_ptr(&ids[1]), shape_ptr(&ids[2]));
}

// ----------------------------------------------------------------------
// Triangles that arrive after the body (#1222): a procgen model finished
// late, a mesh a game uploads at runtime
// ----------------------------------------------------------------------

const RUNTIME_BLOCK: &str = "[transform]\nscale = [2, 2, 2]\n[model]\nasset = \"built\"\n\
                             runtime = true\n[rigidbody]\nbody_type = \"static\"\n\
                             [collider]\nshape = \"mesh\"\n";

fn reports(world: &FlintWorld) -> Vec<String> {
    world
        .read_reports()
        .snapshot()
        .iter()
        .map(|d| d.one_line())
        .collect()
}

#[test]
fn a_runtime_models_collider_waits_quietly_then_takes_its_triangles() {
    let mut world = FlintWorld::new();
    let mut sys = PhysicsSystem::new();
    let block = spawn(&mut world, "block", RUNTIME_BLOCK);
    // No request: the mesh has no file to be read from
    assert!(PhysicsSystem::collider_mesh_requests(&world).is_empty());
    sys.initialize(&mut world).unwrap();
    assert!(matches!(
        sys.get_entity_collider_extents(block),
        Some(ColliderExtents::Box { .. })
    ));
    assert!(
        !reports(&world).iter().any(|r| r.contains("no triangles")),
        "a runtime mesh isn't missing, only not uploaded yet: {:#?}",
        reports(&world)
    );

    // The game uploads it: the next step makes the collider again
    sys.set_collider_mesh(block, Arc::new(wedge()));
    let before = sys.sync.collider_map[&block];
    run(&mut sys, &mut world, 1.0 / 60.0);
    match sys.get_entity_collider_extents(block) {
        Some(ColliderExtents::Mesh {
            half_extents,
            centre,
        }) => {
            assert_eq!(half_extents, [2.0, 1.0, 2.0]);
            assert_eq!(centre, [0.0, 1.0, 0.0]);
        }
        other => panic!("{other:?}"),
    }
    let after = sys.sync.collider_map[&block];
    assert_ne!(before, after);
    assert_eq!(sys.entity_of_collider(after), Some(block));
    assert_eq!(sys.entity_of_collider(before), None);

    // A ray meets it where it's drawn: the 2x block's top is y = 2
    let hit = sys
        .raycast([1.5, 10.0, 1.5], [0.0, -1.0, 0.0], 50.0, None)
        .expect("the ray meets the mesh");
    assert_eq!(hit.entity_id, block);
    assert!((hit.point[1] - 2.0).abs() < 1e-3, "{:?}", hit.point);
    // ...and not where the 1 m placeholder box was not
    assert!(sys
        .raycast([3.5, 10.0, 0.0], [0.0, -1.0, 0.0], 50.0, None)
        .is_none());
}

#[test]
fn a_mesh_handed_over_again_replaces_the_shape_and_the_same_one_changes_nothing() {
    let mut world = FlintWorld::new();
    let mut sys = PhysicsSystem::new();
    let block = spawn(&mut world, "block", RUNTIME_BLOCK);
    let first = Arc::new(wedge());
    sys.set_collider_mesh(block, first.clone());
    sys.initialize(&mut world).unwrap();
    let made = sys.sync.collider_map[&block];

    // The same triangles: nothing is made again
    sys.set_collider_mesh(block, first.clone());
    run(&mut sys, &mut world, 1.0 / 60.0);
    assert_eq!(sys.sync.collider_map[&block], made);

    // A taller mesh (a game rebuilt it): the collider follows
    let mut taller = wedge();
    for p in &mut taller.positions {
        p[1] *= 3.0;
    }
    drop(first);
    sys.set_collider_mesh(block, Arc::new(taller));
    run(&mut sys, &mut world, 1.0 / 60.0);
    let hit = sys
        .raycast([0.0, 20.0, 0.0], [0.0, -1.0, 0.0], 50.0, None)
        .unwrap();
    assert!((hit.point[1] - 6.0).abs() < 1e-3, "{:?}", hit.point);
    // The old triangles' shape is no longer kept
    assert_eq!(sys.sync.mesh_shapes_len(), 1);
}

#[test]
fn a_ball_rests_on_a_convex_hull_handed_over_after_its_body() {
    // A procgen rock finished after the scene's bodies were made
    let mut world = FlintWorld::new();
    let mut sys = PhysicsSystem::new();
    let rock = spawn(
        &mut world,
        "rock",
        "[model]\nasset = \"rock\"\n[rigidbody]\nbody_type = \"static\"\n\
         [collider]\nshape = \"convex\"\nsize = [0.2, 0.2, 0.2]\n",
    );
    let b = spawn(&mut world, "ball", &ball(0.0, ""));
    sys.initialize(&mut world).unwrap();
    sys.set_collider_mesh(rock, Arc::new(wedge()));
    run(&mut sys, &mut world, 2.0);
    // On the hull's top (y = 1) with radius 0.5, not the 0.2 m box
    assert!(
        (y_of(&world, b) - 1.5).abs() < 0.05,
        "y = {}",
        y_of(&world, b)
    );
}

// ----------------------------------------------------------------------
// The reverse map
// ----------------------------------------------------------------------

#[test]
fn every_collider_finds_its_entity_and_forgets_it_when_removed() {
    let mut world = FlintWorld::new();
    let mut sys = PhysicsSystem::new();
    let floor = spawn(&mut world, "floor", FLOOR);
    let b = spawn(&mut world, "ball", &ball(0.0, ""));
    sys.initialize(&mut world).unwrap();
    for id in [floor, b] {
        let h = sys.sync.collider_map[&id];
        assert_eq!(sys.entity_of_collider(h), Some(id));
    }
    let h = sys.sync.collider_map[&b];
    sys.remove_entity(b);
    assert_eq!(sys.entity_of_collider(h), None);
    assert_eq!(sys.sync.collider_entity.len(), 1);
}
