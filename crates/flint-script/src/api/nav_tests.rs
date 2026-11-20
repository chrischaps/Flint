//! The `nav` module (#969): a script asks for a way round a wall and walks
//! it, and asks wrongly and is told.

use crate::context::{InputSnapshot, StateScope};
use crate::engine::ScriptEngine;
use flint_core::EntityId;
use flint_ecs::FlintWorld;
use flint_physics::PhysicsSystem;
use flint_runtime::{EventBus, GameStateMachine, PersistentStore};

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
fn walled() -> (FlintWorld, PhysicsSystem) {
    let mut world = FlintWorld::new();
    block(&mut world, "floor", [0.0, -0.5, 0.0], [40.0, 1.0, 40.0]);
    block(&mut world, "wall", [0.0, 1.5, 0.0], [1.0, 3.0, 20.0]);
    let mut physics = PhysicsSystem::new();
    physics.initialize(&mut world).unwrap();
    physics
        .fixed_update(&mut world, 1.0 / 60.0, &mut EventBus::new())
        .unwrap();
    (world, physics)
}

fn num(world: &FlintWorld, id: EntityId, field: &str) -> f64 {
    let v = world
        .get_components(id)
        .unwrap()
        .get_field("probe", field)
        .cloned()
        .unwrap_or(toml::Value::Float(-1.0));
    v.as_float()
        .or(v.as_integer().map(|i| i as f64))
        .unwrap_or(-1.0)
}

#[test]
fn a_script_finds_a_way_round_a_wall_and_walks_it() {
    let (mut world, physics) = walled();
    // Its middle 0.9 m above the floor, west of the wall
    let walker = spawn(
        &mut world,
        "walker",
        "[transform]\nposition = [-5, 0.9, 0]\n[probe]\npoints = 0.0\n",
    );
    let mut engine = ScriptEngine::new();
    let ast = engine
        .compile(
            r#"
            let path = [];
            fn on_update() {
                let me = self_entity();
                if path.is_empty() {
                    let p = get_position(me);
                    path = find_path(p.x, p.y, p.z, 5.0, 0.0, 0.0);
                    set_field(me, "probe", "points", path.len());
                }
                let step = move_along_path(me, path, 4.0);
                set_field(me, "probe", "arrived", if step.arrived { 1 } else { 0 });
                set_field(me, "probe", "remaining", step.remaining);
            }
        "#,
        )
        .unwrap();
    engine.add_script(walker, ast, "walker.rhai".into());

    let mut sm = GameStateMachine::new();
    let mut store = PersistentStore::new();
    let ctx = engine.ctx.clone();
    let _scope = StateScope::new(&ctx, &mut sm, &mut store, &physics);
    let mut frames = 0;
    let mut first_remaining = None;
    for _ in 0..600 {
        engine.provide_context(InputSnapshot::default(), 1.0 / 60.0, 0.0);
        engine.call_updates(&mut world);
        frames += 1;
        let p = world.get_transform(walker).unwrap().position;
        // Never in the wall, nor nearer it than the agent's radius
        let in_wall = p.x.abs() < 0.5 + 0.39 && p.z.abs() < 10.0 + 0.39;
        assert!(!in_wall, "frame {frames}: at {p:?}, in the wall");
        assert!((p.y - 0.9).abs() < 1e-4, "it keeps its height: {p:?}");
        first_remaining.get_or_insert(num(&world, walker, "remaining"));
        if num(&world, walker, "arrived") == 1.0 {
            break;
        }
    }
    let p = world.get_transform(walker).unwrap().position;
    assert!(
        (p.x - 5.0).abs() < 1e-3 && p.z.abs() < 1e-3,
        "it arrived: {p:?}"
    );
    assert!(num(&world, walker, "points") >= 3.0, "the path bends");
    // 4 m/s over the ~24.5 m way round: about 6 s at 60 frames a second
    let length = first_remaining.unwrap() + 4.0 / 60.0;
    let expected = (length / 4.0 * 60.0).ceil() as i32;
    assert!(
        (frames - expected).abs() <= 1,
        "{frames} frames for {length} m at 4 m/s (expected {expected})"
    );
    assert_eq!(num(&world, walker, "remaining"), 0.0);
}

#[test]
fn find_path_with_nothing_to_stand_on_or_no_physics_is_empty() {
    let engine = ScriptEngine::new();
    // No physics lent
    let n: i64 = engine
        .rhai_engine()
        .eval("find_path(0, 0, 0, 1, 0, 1).len()")
        .unwrap();
    assert_eq!(n, 0);

    let (_world, physics) = walled();
    let mut sm = GameStateMachine::new();
    let mut store = PersistentStore::new();
    let _scope = StateScope::new(&engine.ctx, &mut sm, &mut store, &physics);
    // Far off the floor
    let n: i64 = engine
        .rhai_engine()
        .eval("find_path(100, 0, 100, 0, 0, 0).len()")
        .unwrap();
    assert_eq!(n, 0);
    // Integers, an opts map, and a small agent through the open floor
    let path: rhai::Array = engine
        .rhai_engine()
        .eval("find_path(-5, 0, 12, 5, 0, 12, #{ radius: 0.2, height: 1.0 })")
        .unwrap();
    assert_eq!(path.len(), 2);
}

#[test]
fn settings_that_make_no_grid_are_a_script_error() {
    let engine = ScriptEngine::new();
    let (_world, physics) = walled();
    let mut sm = GameStateMachine::new();
    let mut store = PersistentStore::new();
    let _scope = StateScope::new(&engine.ctx, &mut sm, &mut store, &physics);
    let err = engine
        .rhai_engine()
        .eval::<rhai::Dynamic>("find_path(0, 0, 0, 1, 0, 1, #{ radius: 0.3, cell: 0.7 })")
        .unwrap_err()
        .to_string();
    assert!(err.contains("diameter"), "{err}");
    // An unknown key is ignored with a warning, as every opts map's is
    let path: rhai::Array = engine
        .rhai_engine()
        .eval("find_path(-5, 0, 12, 5, 0, 12, #{ radious: 0.3 })")
        .unwrap();
    assert_eq!(path.len(), 2);
}

#[test]
fn a_path_that_is_not_points_is_a_script_error() {
    let engine = ScriptEngine::new();
    let mut world = FlintWorld::new();
    spawn(&mut world, "w", "[transform]\nposition = [0, 0, 0]\n");
    let _world = crate::context::WorldScope::new(&engine.ctx, &mut world);
    let err = engine
        .rhai_engine()
        .eval::<rhai::Dynamic>(r#"move_along_path(get_entity("w"), [1, 2], 1.0)"#)
        .unwrap_err()
        .to_string();
    assert!(err.contains("point 0"), "{err}");
    // An empty path: already there
    let arrived: bool = engine
        .rhai_engine()
        .eval(r#"move_along_path(get_entity("w"), [], 1.0).arrived"#)
        .unwrap();
    assert!(arrived);
}
