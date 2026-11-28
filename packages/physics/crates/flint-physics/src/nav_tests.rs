//! Navigation over the fixed colliders (#969): paths around walls, up
//! ramps and steps, through gaps the agent fits, and none where it can't go.

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

/// A static box `size` (full extents) centred at `at`, turned `rot` degrees.
fn block(at: [f32; 3], size: [f32; 3], rot: [f32; 3]) -> String {
    format!(
        "[transform]\nposition = [{}, {}, {}]\nrotation = [{}, {}, {}]\n\
         [rigidbody]\nbody_type = \"static\"\n\
         [collider]\nshape = \"box\"\nsize = [{}, {}, {}]\n",
        at[0], at[1], at[2], rot[0], rot[1], rot[2], size[0], size[1], size[2]
    )
}

/// A floor 40 m square whose top is y = 0.
fn floor(world: &mut FlintWorld) {
    spawn(
        world,
        "floor",
        &block([0.0, -0.5, 0.0], [40.0, 1.0, 40.0], [0.0; 3]),
    );
}

fn wall(world: &mut FlintWorld, name: &str, at: [f32; 2], size: [f32; 2]) {
    let text = block([at[0], 1.5, at[1]], [size[0], 3.0, size[1]], [0.0; 3]);
    spawn(world, name, &text);
}

/// The physics of `world`, stepped once: a grid keeps its own query
/// pipeline, but the tests' own rays (`assert_clear`, `assert_follows`)
/// see only what a step has put in Rapier's.
fn ready(world: &mut FlintWorld) -> PhysicsSystem {
    let mut sys = PhysicsSystem::new();
    sys.initialize(world).unwrap();
    sys.fixed_update(world, 1.0 / 60.0, &mut EventBus::new())
        .unwrap();
    sys
}

fn length(path: &[[f32; 3]]) -> f32 {
    path.windows(2)
        .map(|w| {
            let d = [w[1][0] - w[0][0], w[1][1] - w[0][1], w[1][2] - w[0][2]];
            (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt()
        })
        .sum()
}

/// Every leg of the path is clear of every collider a metre above the
/// floor, by the physics world's own rays, not the grid's.
fn assert_clear(sys: &PhysicsSystem, path: &[[f32; 3]]) {
    for w in path.windows(2) {
        let (a, b) = (w[0], w[1]);
        let d = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
        let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
        if len < 1e-4 {
            continue;
        }
        let dir = [d[0] / len, d[1] / len, d[2] / len];
        let hit = sys.raycast([a[0], a[1] + 1.0, a[2]], dir, len, None);
        assert!(hit.is_none(), "the leg {a:?} -> {b:?} meets {hit:?}");
    }
}

/// Every leg keeps to the floor: under each point along it, a ray down
/// meets the floor within `FOLLOW` of the leg's height (and a little for
/// the samples between columns' middles).
fn assert_follows(sys: &PhysicsSystem, path: &[[f32; 3]]) {
    for w in path.windows(2) {
        let (a, b) = (w[0], w[1]);
        for i in 0..=40 {
            let t = i as f32 / 40.0;
            let p = [
                a[0] + (b[0] - a[0]) * t,
                a[1] + (b[1] - a[1]) * t,
                a[2] + (b[2] - a[2]) * t,
            ];
            let hit = sys
                .raycast([p[0], p[1] + 1.0, p[2]], [0.0, -1.0, 0.0], 3.0, None)
                .expect("a floor under the leg");
            let floor = hit.point[1];
            assert!(
                (floor - p[1]).abs() <= nav::FOLLOW + 0.08,
                "the leg {a:?} -> {b:?} is {} m off the floor at {p:?}",
                p[1] - floor
            );
        }
    }
}

#[test]
fn an_open_floor_is_one_straight_walk() {
    let mut world = FlintWorld::new();
    floor(&mut world);
    let sys = ready(&mut world);
    let mut grid = NavGrid::new(NavSettings::default());

    let path = grid
        .find_path(&sys, [-5.0, 0.9, 1.0], [6.0, 0.0, -2.0])
        .expect("a path over open floor");
    assert_eq!(path.len(), 2, "{path:?}");
    // On the floor, where the points stand
    assert_eq!([path[0][0], path[0][2]], [-5.0, 1.0]);
    assert_eq!([path[1][0], path[1][2]], [6.0, -2.0]);
    assert!(path.iter().all(|p| p[1].abs() < 1e-3), "{path:?}");
}

#[test]
fn a_wall_is_walked_around_and_never_through() {
    let mut world = FlintWorld::new();
    floor(&mut world);
    // A wall across x = 0 from z = -10 to 10
    wall(&mut world, "wall", [0.0, 0.0], [1.0, 20.0]);
    let sys = ready(&mut world);
    let mut grid = NavGrid::new(NavSettings::default());

    let path = grid
        .find_path(&sys, [-5.0, 0.0, 0.0], [5.0, 0.0, 0.0])
        .expect("a way around the wall's end");
    assert!(path.len() >= 3, "it bends: {path:?}");
    assert_clear(&sys, &path);
    // Round an end, keeping its radius off the corner
    assert!(
        path.iter().any(|p| p[2].abs() > 10.3),
        "it rounds an end: {path:?}"
    );
    let straight = 10.0;
    // To the wall's corner (its half thickness plus the radius off it),
    // along its end, and back: the shortest way an agent fits
    let around = 2.0 * (4.1f32 * 4.1 + 10.4 * 10.4).sqrt() + 1.8;
    let len = length(&path);
    assert!(
        len > straight && len < around * 1.05,
        "{len} m: about the way round ({around} m)"
    );
    // Pulled straight: the corner points, not a staircase of cells
    assert!(path.len() <= 4, "{path:?}");
}

#[test]
fn a_closed_room_has_no_way_in() {
    let mut world = FlintWorld::new();
    floor(&mut world);
    wall(&mut world, "n", [0.0, 3.0], [7.0, 1.0]);
    wall(&mut world, "s", [0.0, -3.0], [7.0, 1.0]);
    wall(&mut world, "e", [3.0, 0.0], [1.0, 7.0]);
    wall(&mut world, "w", [-3.0, 0.0], [1.0, 7.0]);
    let sys = ready(&mut world);
    let mut grid = NavGrid::new(NavSettings::default());

    assert!(grid
        .find_path(&sys, [10.0, 0.0, 0.0], [0.0, 0.0, 0.0])
        .is_none());
    // Inside, the room is open
    assert!(grid
        .find_path(&sys, [-1.5, 0.0, -1.5], [1.5, 0.0, 1.5])
        .is_some());
}

#[test]
fn a_gap_lets_through_only_an_agent_that_fits() {
    let mut world = FlintWorld::new();
    floor(&mut world);
    // A wall across x = 0, wall to wall, with a gap at z = 0 of `gap` metres
    let gap = 1.2;
    let half = 20.0;
    let side = half - gap / 2.0;
    wall(&mut world, "a", [0.0, gap / 2.0 + side / 2.0], [1.0, side]);
    wall(
        &mut world,
        "b",
        [0.0, -(gap / 2.0 + side / 2.0)],
        [1.0, side],
    );
    let sys = ready(&mut world);

    let narrow = NavSettings::default(); // 0.8 m across
    let path = NavGrid::new(narrow)
        .find_path(&sys, [-4.0, 0.0, 3.0], [4.0, 0.0, -3.0])
        .expect("an agent 0.8 m across fits a 1.2 m gap");
    assert_clear(&sys, &path);
    assert!(path.iter().any(|p| p[0].abs() < 0.6 && p[2].abs() < 0.6));

    let wide = NavSettings {
        radius: 0.7, // 1.4 m across
        cell: 0.25,
        ..NavSettings::default()
    };
    assert!(NavGrid::new(wide)
        .find_path(&sys, [-4.0, 0.0, 3.0], [4.0, 0.0, -3.0])
        .is_none());
}

#[test]
fn a_ramp_climbs_to_a_platform_that_has_no_other_way_up() {
    let build = |with_ramp: bool| {
        let mut world = FlintWorld::new();
        floor(&mut world);
        // A platform x 10..14, top at y = 2
        spawn(
            &mut world,
            "platform",
            &block([12.0, 1.0, 0.0], [4.0, 2.0, 6.0], [0.0; 3]),
        );
        if with_ramp {
            // 20 degrees up from x = 4.5 to the platform's edge at x = 10
            let a = 20f32.to_radians();
            let (mid_x, mid_y) = (7.25, 1.0);
            let len = (5.5f32 * 5.5 + 4.0).sqrt() + 0.6;
            let at = [mid_x + 0.1 * a.sin(), mid_y - 0.1 * a.cos(), 0.0];
            spawn(
                &mut world,
                "ramp",
                &block(at, [len, 0.2, 3.0], [0.0, 0.0, 20.0]),
            );
        }
        let sys = ready(&mut world);
        (world, sys)
    };

    let (_w, sys) = build(true);
    let mut grid = NavGrid::new(NavSettings::default());
    let path = grid
        .find_path(&sys, [0.0, 0.0, 2.0], [12.0, 2.0, 2.0])
        .expect("up the ramp");
    let end = path.last().unwrap();
    assert!(
        (end[1] - 2.0).abs() < 0.02,
        "it ends on the platform: {path:?}"
    );
    // It bends at the ramp's foot and top rather than climbing from the
    // start: an agent walking it never floats off the floor
    assert_follows(&sys, &path);
    assert!(path.len() >= 4, "{path:?}");
    // Every point stands on the ramp's width where it is on the slope
    for p in &path {
        if p[0] > 5.0 && p[0] < 9.8 {
            assert!(p[2].abs() < 1.5, "on the ramp: {p:?}");
        }
    }

    let (_w, sys) = build(false);
    let mut grid = NavGrid::new(NavSettings::default());
    assert!(
        grid.find_path(&sys, [0.0, 0.0, 2.0], [12.0, 2.0, 2.0])
            .is_none(),
        "2 m is no step"
    );
}

#[test]
fn a_low_step_is_climbed_and_a_tall_one_walked_round() {
    let mut world = FlintWorld::new();
    floor(&mut world);
    // A kerb 0.25 m high across the whole floor at x = 0..2
    spawn(
        &mut world,
        "kerb",
        &block([1.0, 0.125, 0.0], [2.0, 0.25, 40.0], [0.0; 3]),
    );
    // A crate 1 m high at x = 6
    spawn(
        &mut world,
        "crate",
        &block([6.0, 0.5, 0.0], [1.0, 1.0, 1.0], [0.0; 3]),
    );
    let sys = ready(&mut world);
    let mut grid = NavGrid::new(NavSettings::default());

    let over = grid
        .find_path(&sys, [-3.0, 0.0, 0.0], [1.0, 0.25, 0.0])
        .unwrap();
    // Straight to the kerb, one column's climb up it (a step is a leg of
    // its own, so a walk rises at the step and not before), and on
    assert!(over.len() <= 4, "straight up the kerb: {over:?}");
    assert!(over.iter().all(|p| p[2].abs() <= 0.125), "{over:?}");
    assert_follows(&sys, &over);
    assert!((over.last().unwrap()[1] - 0.25).abs() < 1e-3);

    let round = grid
        .find_path(&sys, [4.0, 0.0, 0.0], [8.0, 0.0, 0.0])
        .unwrap();
    assert!(round.len() >= 3, "round the crate: {round:?}");
    assert_clear(&sys, &round);
}

#[test]
fn bodies_that_move_are_not_on_the_grid() {
    let mut world = FlintWorld::new();
    floor(&mut world);
    // A door (kinematic) and a crate (dynamic) on the straight line
    let door = "[transform]\nposition = [0, 1.5, 0]\n[rigidbody]\nbody_type = \"kinematic\"\n\
                [collider]\nshape = \"box\"\nsize = [1, 3, 40]\n";
    spawn(&mut world, "door", door);
    let crate_ = "[transform]\nposition = [3, 0.5, 0]\n[rigidbody]\nbody_type = \"dynamic\"\n\
                  [collider]\nshape = \"box\"\nsize = [1, 1, 1]\n";
    spawn(&mut world, "crate", crate_);
    let sys = ready(&mut world);

    let path = NavGrid::new(NavSettings::default())
        .find_path(&sys, [-5.0, 0.0, 0.0], [5.0, 0.0, 0.0])
        .unwrap();
    assert_eq!(path.len(), 2, "straight through: {path:?}");
}

#[test]
fn a_wall_added_later_is_seen_by_the_next_search() {
    let mut world = FlintWorld::new();
    floor(&mut world);
    let mut sys = ready(&mut world);
    let mut grid = NavGrid::new(NavSettings::default());
    let before = grid
        .find_path(&sys, [-5.0, 0.0, 0.0], [5.0, 0.0, 0.0])
        .unwrap();
    assert_eq!(before.len(), 2);
    let generation = sys.static_generation();

    wall(&mut world, "wall", [0.0, 0.0], [1.0, 20.0]);
    sys.fixed_update(&mut world, 1.0 / 60.0, &mut EventBus::new())
        .unwrap();
    assert_ne!(sys.static_generation(), generation);

    let after = grid
        .find_path(&sys, [-5.0, 0.0, 0.0], [5.0, 0.0, 0.0])
        .unwrap();
    assert!(after.len() >= 3, "round the new wall: {after:?}");
    assert_clear(&sys, &after);

    // A dynamic body coming and going leaves the grid alone
    let generation = sys.static_generation();
    let ball = "[transform]\nposition = [8, 3, 8]\n[rigidbody]\nbody_type = \"dynamic\"\n\
                [collider]\nshape = \"sphere\"\nsize = [1, 1, 1]\n";
    let id = spawn(&mut world, "ball", ball);
    sys.fixed_update(&mut world, 1.0 / 60.0, &mut EventBus::new())
        .unwrap();
    sys.remove_entity(id);
    assert_eq!(sys.static_generation(), generation);
}

#[test]
fn a_floor_above_another_is_its_own_floor() {
    let mut world = FlintWorld::new();
    floor(&mut world);
    // A deck at y = 3 (top), over x -4..4, with nothing reaching it
    spawn(
        &mut world,
        "deck",
        &block([0.0, 2.9, 0.0], [8.0, 0.2, 8.0], [0.0; 3]),
    );
    let sys = ready(&mut world);
    let mut grid = NavGrid::new(NavSettings::default());

    // Under the deck the ground is a floor (3 m of room), and on it the deck is
    let under = grid.nearest_floor(&sys, [0.0, 0.9, 0.0]).unwrap();
    assert!(under[1].abs() < 1e-3, "{under:?}");
    let on = grid.nearest_floor(&sys, [0.0, 3.9, 0.0]).unwrap();
    assert!((on[1] - 3.0).abs() < 1e-3, "{on:?}");
    // Walking under it is one straight walk; up onto it there is no way
    assert_eq!(
        grid.find_path(&sys, [-6.0, 0.0, 0.0], [6.0, 0.0, 0.0])
            .unwrap()
            .len(),
        2
    );
    assert!(grid
        .find_path(&sys, [-6.0, 0.0, 0.0], [0.0, 3.0, 0.0])
        .is_none());
}

#[test]
fn a_search_is_the_same_every_time_and_bakes_only_what_it_reaches() {
    let mut world = FlintWorld::new();
    floor(&mut world);
    wall(&mut world, "wall", [0.0, 0.0], [1.0, 20.0]);
    let sys = ready(&mut world);

    let mut a = NavGrid::new(NavSettings::default());
    let mut b = NavGrid::new(NavSettings::default());
    let pa = a.find_path(&sys, [-5.0, 0.0, 0.0], [5.0, 0.0, 0.0]);
    let pb = b.find_path(&sys, [-5.0, 0.0, 0.0], [5.0, 0.0, 0.0]);
    assert_eq!(pa, pb);
    assert_eq!(pa, a.find_path(&sys, [-5.0, 0.0, 0.0], [5.0, 0.0, 0.0]));

    // The 40 m floor is 160 x 160 cells, 100 tiles; an open walk bakes few
    let mut open = NavGrid::new(NavSettings::default());
    open.find_path(&sys, [-15.0, 0.0, 15.0], [-12.0, 0.0, 15.0]);
    assert!(open.stats().tiles <= 4, "{:?}", open.stats());
}

#[test]
fn settings_that_cannot_make_a_grid_say_why() {
    let ok = NavSettings::default();
    assert!(ok.validate().is_ok());
    let wide_cells = NavSettings { cell: 0.8, ..ok };
    assert!(wide_cells.validate().unwrap_err().contains("diameter"));
    assert!(NavSettings { radius: -1.0, ..ok }.validate().is_err());
    assert!(NavSettings {
        max_slope_deg: 90.0,
        ..ok
    }
    .validate()
    .is_err());
    assert!(NavSettings {
        step: f32::NAN,
        ..ok
    }
    .validate()
    .is_err());
}

#[test]
fn no_floor_is_no_path() {
    let mut world = FlintWorld::new();
    let sys = ready(&mut world);
    let mut grid = NavGrid::new(NavSettings::default());
    assert!(grid.find_path(&sys, [0.0; 3], [1.0, 0.0, 0.0]).is_none());
}

/// What a search costs, cold (baking the tiles it reaches) and warm, on
/// the lamplighter's garden and on a 200 m field split by a 150 m wall.
/// `cargo test -p flint-physics --release --lib nav_cost -- --ignored --nocapture`
#[test]
#[ignore]
#[allow(clippy::disallowed_methods)] // a measurement: the wall clock is the point
fn nav_cost() {
    use std::time::Instant;
    let time = |label: &str, grid: &mut NavGrid, sys: &PhysicsSystem, a: [f32; 3], b: [f32; 3]| {
        let t = Instant::now();
        let path = grid.find_path(sys, a, b);
        let ms = t.elapsed().as_secs_f64() * 1e3;
        let len = path.as_deref().map(length).unwrap_or(0.0);
        println!(
            "{label:<28} {ms:8.2} ms  {:>2} points  {len:6.1} m  tiles {:>3}",
            path.map_or(0, |p| p.len()),
            grid.stats().tiles
        );
    };

    // The garden of flint-examples/nav_lamplighter.scene.toml
    let mut world = FlintWorld::new();
    spawn(
        &mut world,
        "ground",
        &block([0.0, -0.25, 0.0], [24.0, 0.5, 24.0], [0.0; 3]),
    );
    for (name, at, size) in [
        ("s", [0.0, -12.3], [25.2, 0.6]),
        ("n", [0.0, 12.3], [25.2, 0.6]),
        ("w", [-12.3, 0.0], [0.6, 24.0]),
        ("e", [12.3, 0.0], [0.6, 24.0]),
        ("a", [-4.0, -4.0], [16.0, 0.6]),
        ("b", [4.0, 3.0], [16.0, 0.6]),
    ] {
        spawn(
            &mut world,
            name,
            &block([at[0], 0.8, at[1]], [size[0], 1.6, size[1]], [0.0; 3]),
        );
    }
    spawn(
        &mut world,
        "terrace",
        &block([8.5, 0.75, 8.5], [5.0, 1.5, 5.0], [0.0; 3]),
    );
    let a = 18f32.to_radians();
    let run = 1.5 / a.tan();
    let len = (run * run + 2.25).sqrt() + 0.5;
    let at = [6.0 - run / 2.0 + 0.1 * a.sin(), 0.75 - 0.1 * a.cos(), 8.0];
    spawn(
        &mut world,
        "ramp",
        &block(at, [len, 0.2, 2.0], [0.0, 0.0, 18.0]),
    );
    let sys = ready(&mut world);
    let mut grid = NavGrid::new(NavSettings::default());
    let stops = [
        [0.0, 0.75, -9.0],
        [-9.0, 0.0, -9.0],
        [9.0, 0.0, -8.0],
        [-9.0, 0.0, 8.0],
        [9.6, 1.5, 9.6],
    ];
    for (i, w) in stops.windows(2).enumerate() {
        time(
            &format!("garden leg {} cold", i + 1),
            &mut grid,
            &sys,
            w[0],
            w[1],
        );
    }
    for (i, w) in stops.windows(2).enumerate() {
        time(
            &format!("garden leg {} warm", i + 1),
            &mut grid,
            &sys,
            w[0],
            w[1],
        );
    }

    // A 200 m field with a 150 m wall across it
    let mut world = FlintWorld::new();
    spawn(
        &mut world,
        "field",
        &block([0.0, -0.5, 0.0], [200.0, 1.0, 200.0], [0.0; 3]),
    );
    spawn(
        &mut world,
        "wall",
        &block([0.0, 1.5, -25.0], [1.0, 3.0, 150.0], [0.0; 3]),
    );
    let sys = ready(&mut world);
    let mut grid = NavGrid::new(NavSettings::default());
    time(
        "field open 120 m cold",
        &mut grid,
        &sys,
        [-60.0, 0.0, 80.0],
        [60.0, 0.0, 80.0],
    );
    time(
        "field round wall cold",
        &mut grid,
        &sys,
        [-30.0, 0.0, -40.0],
        [30.0, 0.0, -40.0],
    );
    time(
        "field round wall warm",
        &mut grid,
        &sys,
        [-30.0, 0.0, -40.0],
        [30.0, 0.0, -40.0],
    );
    let mut coarse = NavGrid::new(NavSettings {
        cell: 0.5,
        ..NavSettings::default()
    });
    time(
        "field round wall, 0.5 m cold",
        &mut coarse,
        &sys,
        [-30.0, 0.0, -40.0],
        [30.0, 0.0, -40.0],
    );
    let mut wide = NavGrid::new(NavSettings {
        cell: 0.5,
        max_nodes: 400_000,
        ..NavSettings::default()
    });
    time(
        "field round wall, 0.5 m, 400k",
        &mut wide,
        &sys,
        [-30.0, 0.0, -40.0],
        [30.0, 0.0, -40.0],
    );
    time(
        "  the same, warm",
        &mut wide,
        &sys,
        [-30.0, 0.0, -40.0],
        [30.0, 0.0, -40.0],
    );
}
