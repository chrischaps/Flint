//! The `physics` and `physics_2d` modules through a script (flint-script's
//! tests until #1617): rays with and without physics lent, layer masks, a
//! collider moved to another layer, and the gate a run without the module
//! enabled meets.

use super::module::{PHYSICS, PHYSICS_2D};
use super::nav_module::NAV;
use crate::{CollisionLayers, PhysicsSystem};
use flint_core::resources::Resources;
use flint_ecs::FlintWorld;
use flint_runtime::{EventBus, GameStateMachine, PersistentStore};
use flint_script::ScriptSystem;

/// A script system with the package's modules declared, enabled or not.
pub(super) fn system(enabled: bool) -> ScriptSystem {
    let mut system = ScriptSystem::new();
    system.declare_module(&super::module::PHYSICS_MODULE);
    system.declare_module(&super::module::PHYSICS_2D_MODULE);
    system.declare_module(&super::nav_module::NAV_MODULE);
    if enabled {
        system.set_enabled_modules(vec![PHYSICS, PHYSICS_2D, NAV]);
    }
    system
}

/// `physics` as the run's resource.
pub(super) fn lent(physics: PhysicsSystem) -> Resources {
    let mut resources = Resources::new();
    resources.insert(physics);
    resources
}

fn spawn(world: &mut FlintWorld, name: &str, text: &str) -> flint_core::EntityId {
    let id = world.spawn(name).unwrap();
    for (component, data) in text.parse::<toml::Table>().unwrap() {
        world.set_component(id, &component, data).unwrap();
    }
    id
}

#[test]
fn a_ray_with_no_physics_lent_answers_unit() {
    let mut system = system(true);
    let mut world = FlintWorld::new();
    let resources = Resources::new();
    let (mut sm, mut store) = (GameStateMachine::new(), PersistentStore::new());
    let _scope = system.state_scope(&mut sm, &mut store, &resources);
    let kind: String = system.with_world(&mut world, |rhai| {
        rhai.eval("type_of(raycast(0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 100.0))")
            .unwrap()
    });
    assert_eq!(kind, "()");
}

/// A run that hasn't enabled the module (its scene was entered without the
/// package's subsystem) gets the gate's error, naming when it is enabled.
#[test]
fn a_ray_where_the_module_is_not_enabled_says_so() {
    let mut system = system(false);
    let mut world = FlintWorld::new();
    let err = system.with_world(&mut world, |rhai| {
        rhai.eval::<rhai::Dynamic>("raycast(0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 100.0)")
            .unwrap_err()
            .to_string()
    });
    assert!(err.contains("physics module is not enabled"), "{err}");
    assert!(err.contains("the run has the physics package"), "{err}");
}

/// A wall behind a pane of glass, each on its own layer (#729): a script's
/// `raycast` meets the glass, or with a mask the wall, and an unknown
/// layer is a script error naming the nearest.
#[test]
fn a_script_raycast_takes_a_layer_mask() {
    let mut system = system(true);
    let mut world = FlintWorld::new();
    let mut physics = PhysicsSystem::new();
    physics.set_layers(CollisionLayers::new(&["glass", "wall"]).unwrap());
    for (name, x, w, layer) in [("glass", 5.0, 0.1, "glass"), ("wall", 10.0, 1.0, "wall")] {
        let text = format!(
            "[transform]\nposition = [{x}, 0, 0]\n[rigidbody]\nbody_type = \"static\"\n\
             [collider]\nshape = \"box\"\nsize = [{w}, 4, 4]\nlayer = \"{layer}\"\n"
        );
        spawn(&mut world, name, &text);
    }
    physics.initialize(&mut world).unwrap();
    physics
        .fixed_update(&mut world, 1.0 / 60.0, &mut EventBus::new())
        .unwrap();
    let resources = lent(physics);
    let (mut sm, mut store) = (GameStateMachine::new(), PersistentStore::new());
    let _scope = system.state_scope(&mut sm, &mut store, &resources);
    let (text, err) = system.with_world(&mut world, |rhai| {
        let names: rhai::Array = rhai
            .eval(
                r#"
                let plain = raycast(0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 50.0);
                let walls = raycast(0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 50.0, "wall");
                let both = raycast(0, 0, 0, 1, 0, 0, 50, ["wall", "glass"]);
                let none = raycast(0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 50.0, []);
                [entity_name(plain.entity), entity_name(walls.entity), entity_name(both.entity),
                 type_of(none), collision_layers()]
            "#,
            )
            .unwrap();
        let text: Vec<String> = names.iter().map(|v| v.to_string()).collect();
        let err = rhai
            .eval::<rhai::Dynamic>(r#"raycast(0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 50.0, "wal")"#)
            .unwrap_err()
            .to_string();
        (text, err)
    });
    assert_eq!(
        text,
        [
            "glass",
            "wall",
            "glass",
            "()",
            "[\"default\", \"glass\", \"wall\"]"
        ]
    );
    assert!(err.contains("`wal` is not a collision layer"), "{err}");
    assert!(err.contains("did you mean `wall`?"), "{err}");
}

/// A ball over a floor that leaves `ghost` out (#1221): a script that puts
/// the ball on `ghost` mid-fall sees it fall through, and one that names a
/// layer that isn't, or an entity with no collider, is told at the call.
#[test]
fn a_script_moves_a_collider_to_another_layer() {
    let mut system = system(true);
    let mut world = FlintWorld::new();
    let mut physics = PhysicsSystem::new();
    physics.set_layers(CollisionLayers::new(&["world", "ghost"]).unwrap());
    spawn(
        &mut world,
        "floor",
        "[transform]\nposition = [0, 0, 0]\n[rigidbody]\nbody_type = \"static\"\n\
         [collider]\nshape = \"box\"\nsize = [20, 1, 20]\nlayer = \"world\"\n\
         mask = [\"default\", \"world\"]\n",
    );
    let ball = spawn(
        &mut world,
        "ball",
        "[transform]\nposition = [0, 4, 0]\n[rigidbody]\nbody_type = \"dynamic\"\n\
         [collider]\nshape = \"sphere\"\nsize = [1, 1, 1]\n",
    );
    let marker = spawn(&mut world, "marker", "[transform]\nposition = [0, 0, 0]\n");
    physics.initialize(&mut world).unwrap();
    let mut bus = EventBus::new();
    for _ in 0..20 {
        physics
            .fixed_update(&mut world, 1.0 / 60.0, &mut bus)
            .unwrap();
    }
    let mut resources = lent(physics);
    {
        let (mut sm, mut store) = (GameStateMachine::new(), PersistentStore::new());
        let _scope = system.state_scope(&mut sm, &mut store, &resources);
        system.with_world(&mut world, |rhai| {
            rhai.run(r#"set_collision_layer(get_entity("ball"), "ghost")"#)
                .unwrap();
            let err = rhai
                .eval::<rhai::Dynamic>(r#"set_collision_layer(get_entity("ball"), "gohst")"#)
                .unwrap_err()
                .to_string();
            assert!(err.contains("did you mean `ghost`?"), "{err}");
            let err = rhai
                .eval::<rhai::Dynamic>(r#"set_collision_mask(get_entity("ball"), ["world", 3])"#)
                .unwrap_err()
                .to_string();
            assert!(err.contains("a mask is layer names"), "{err}");
            let err = rhai
                .eval::<rhai::Dynamic>(r#"set_collision_layer(get_entity("marker"), "ghost")"#)
                .unwrap_err()
                .to_string();
            assert!(err.contains("`marker` has no collider"), "{err}");
            rhai.run(r#"set_collision_mask(get_entity("ball"), "ghost")"#)
                .unwrap();
        });
    }
    let col = world.get_component(ball, "collider").unwrap();
    assert_eq!(col["layer"].as_str(), Some("ghost"));
    assert_eq!(col["mask"].as_array().unwrap().len(), 1);
    assert!(world.get_component(marker, "collider").is_none());
    let physics = crate::of_mut(&mut resources).unwrap();
    for _ in 0..100 {
        physics
            .fixed_update(&mut world, 1.0 / 60.0, &mut bus)
            .unwrap();
    }
    let y = world.get_transform(ball).unwrap().position.y;
    assert!(y < -3.0, "the ghost fell through the floor: y = {y}");
}

/// `set_velocity_2d` queues the package's command, for its subsystem to
/// take at the drain; `get_velocity_2d` of a body with none is `()`.
#[test]
fn set_velocity_2d_is_the_packages_command() {
    let mut system = system(true);
    let mut world = FlintWorld::new();
    spawn(&mut world, "puck", "[transform]\nposition = [0, 0, 0]\n");
    let resources = lent(PhysicsSystem::new());
    let (mut sm, mut store) = (GameStateMachine::new(), PersistentStore::new());
    let _scope = system.state_scope(&mut sm, &mut store, &resources);
    let kind: String = system.with_world(&mut world, |rhai| {
        rhai.run(r#"set_velocity_2d(get_entity("puck"), 3.0, -1.5)"#)
            .unwrap();
        rhai.eval(r#"type_of(get_velocity_2d(get_entity("puck")))"#)
            .unwrap()
    });
    assert_eq!(kind, "()");
    let commands = system.drain_commands();
    let [flint_script::context::ScriptCommand::Module(command)] = commands.as_slice() else {
        panic!("one module command: {commands:?}");
    };
    assert_eq!(command.function(), "set_velocity_2d");
    let v = command.payload::<super::Velocity2D>().unwrap();
    assert_eq!(
        (v.entity, v.vx, v.vy),
        (world.get_id("puck").unwrap(), 3.0, -1.5)
    );
}
