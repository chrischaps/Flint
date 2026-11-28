//! The `nav` module (#969): a script asks for a way round a wall and walks
//! it, and asks wrongly and is told. (flint-script's until #1617.)

use super::module_tests::{lent, system};
use crate::PhysicsSystem;
use flint_core::resources::Resources;
use flint_core::EntityId;
use flint_ecs::FlintWorld;
use flint_runtime::{EventBus, GameStateMachine, InputState, PersistentStore};

fn spawn(world: &mut FlintWorld, name: &str, text: &str) -> EntityId {
    let id = world.spawn(name).unwrap();
    for (component, data) in text.parse::<toml::Table>().unwrap() {
        world.set_component(id, &component, data).unwrap();
    }
    id
}

fn block(world: &mut FlintWorld, name: &str, at: [f32; 3], size: [f32; 3]) {
    let text = format!(
        "[transform]\nposition = [{}, {}, {}]\n[rigidbody]\nbody_type = \"static\"\n\
         [collider]\nshape = \"box\"\nsize = [{}, {}, {}]\n",
        at[0], at[1], at[2], size[0], size[1], size[2]
    );
    spawn(world, name, &text);
}

/// A 40 m floor (top y = 0) with a wall across x = 0 from z = -10 to 10.
fn walled() -> (FlintWorld, Resources) {
    let mut world = FlintWorld::new();
    block(&mut world, "floor", [0.0, -0.5, 0.0], [40.0, 1.0, 40.0]);
    block(&mut world, "wall", [0.0, 1.5, 0.0], [1.0, 3.0, 20.0]);
    let mut physics = PhysicsSystem::new();
    physics.initialize(&mut world).unwrap();
    physics
        .fixed_update(&mut world, 1.0 / 60.0, &mut EventBus::new())
        .unwrap();
    (world, lent(physics))
}

#[test]
fn a_script_finds_a_way_round_a_wall_and_walks_it() {
    let (mut world, resources) = walled();
    // Its middle 0.9 m above the floor, west of the wall
    let walker = spawn(
        &mut world,
        "walker",
        "[transform]\nposition = [-5, 0.9, 0]\n",
    );
    let mut system = system(true);
    let (mut sm, mut store) = (GameStateMachine::new(), PersistentStore::new());
    let _scope = system.state_scope(&mut sm, &mut store, &resources);
    let mut scope = rhai::Scope::new();
    scope.push("me", walker.raw() as i64);
    scope.push("path", rhai::Array::new());
    let input = InputState::new();
    let mut frames = 0;
    let mut first_remaining = None;
    let mut points = 0;
    let (mut arrived, mut remaining) = (false, -1.0);
    for _ in 0..600 {
        system.provide_context(&input, &[], frames as f64 / 60.0, 1.0 / 60.0);
        let step: rhai::Map = system.with_world(&mut world, |rhai| {
            rhai.eval_with_scope(
                &mut scope,
                r#"
                if path.is_empty() {
                    let p = get_position(me);
                    path = find_path(p.x, p.y, p.z, 5.0, 0.0, 0.0);
                }
                let step = move_along_path(me, path, 4.0);
                step.points = path.len();
                step
            "#,
            )
            .unwrap()
        });
        frames += 1;
        points = step["points"].as_int().unwrap();
        arrived = step["arrived"].as_bool().unwrap();
        remaining = step["remaining"].as_float().unwrap();
        let p = world.get_transform(walker).unwrap().position;
        // Never in the wall, nor nearer it than the agent's radius
        let in_wall = p.x.abs() < 0.5 + 0.39 && p.z.abs() < 10.0 + 0.39;
        assert!(!in_wall, "frame {frames}: at {p:?}, in the wall");
        assert!((p.y - 0.9).abs() < 1e-4, "it keeps its height: {p:?}");
        first_remaining.get_or_insert(remaining);
        if arrived {
            break;
        }
    }
    assert!(arrived, "it arrived");
    let p = world.get_transform(walker).unwrap().position;
    assert!(
        (p.x - 5.0).abs() < 1e-3 && p.z.abs() < 1e-3,
        "it arrived: {p:?}"
    );
    assert!(points >= 3, "the path bends");
    // 4 m/s over the ~24.5 m way round: about 6 s at 60 frames a second
    let length = first_remaining.unwrap() + 4.0 / 60.0;
    let expected = (length / 4.0 * 60.0).ceil() as i32;
    assert!(
        (frames - expected).abs() <= 1,
        "{frames} frames for {length} m at 4 m/s (expected {expected})"
    );
    assert_eq!(remaining, 0.0);
}

#[test]
fn find_path_with_nothing_to_stand_on_or_no_physics_is_empty() {
    let mut system = system(true);
    // No physics lent
    let mut empty = FlintWorld::new();
    let n: i64 = system.with_world(&mut empty, |rhai| {
        rhai.eval("find_path(0, 0, 0, 1, 0, 1).len()").unwrap()
    });
    assert_eq!(n, 0);

    let (mut world, resources) = walled();
    let (mut sm, mut store) = (GameStateMachine::new(), PersistentStore::new());
    let _scope = system.state_scope(&mut sm, &mut store, &resources);
    system.with_world(&mut world, |rhai| {
        // Far off the floor
        let n: i64 = rhai.eval("find_path(100, 0, 100, 0, 0, 0).len()").unwrap();
        assert_eq!(n, 0);
        // Integers, an opts map, and a small agent through the open floor
        let path: rhai::Array = rhai
            .eval("find_path(-5, 0, 12, 5, 0, 12, #{ radius: 0.2, height: 1.0 })")
            .unwrap();
        assert_eq!(path.len(), 2);
    });
}

#[test]
fn settings_that_make_no_grid_are_a_script_error() {
    let mut system = system(true);
    let (mut world, resources) = walled();
    let (mut sm, mut store) = (GameStateMachine::new(), PersistentStore::new());
    let _scope = system.state_scope(&mut sm, &mut store, &resources);
    system.with_world(&mut world, |rhai| {
        let err = rhai
            .eval::<rhai::Dynamic>("find_path(0, 0, 0, 1, 0, 1, #{ radius: 0.3, cell: 0.7 })")
            .unwrap_err()
            .to_string();
        assert!(err.contains("diameter"), "{err}");
        // An unknown key is ignored with a warning, as every opts map's is
        let path: rhai::Array = rhai
            .eval("find_path(-5, 0, 12, 5, 0, 12, #{ radious: 0.3 })")
            .unwrap();
        assert_eq!(path.len(), 2);
    });
}

#[test]
fn a_path_that_is_not_points_is_a_script_error() {
    let mut system = system(true);
    let mut world = FlintWorld::new();
    spawn(&mut world, "w", "[transform]\nposition = [0, 0, 0]\n");
    system.with_world(&mut world, |rhai| {
        let err = rhai
            .eval::<rhai::Dynamic>(r#"move_along_path(get_entity("w"), [1, 2], 1.0)"#)
            .unwrap_err()
            .to_string();
        assert!(err.contains("point 0"), "{err}");
        // An empty path: already there
        let arrived: bool = rhai
            .eval(r#"move_along_path(get_entity("w"), [], 1.0).arrived"#)
            .unwrap();
        assert!(arrived);
    });
}
