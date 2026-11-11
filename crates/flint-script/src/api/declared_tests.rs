//! The declared API (ADR 0116): the registry, the argument kinds, the
//! world handle and the `entity` module ported through the builder.

use super::builder::kind::{Color, Entity, Num, Str};
use crate::context::WorldScope;
use crate::engine::ScriptEngine;
use flint_ecs::FlintWorld;
use rhai::Dynamic;

/// An engine with the whole API and a world holding `cube` at the origin.
fn engine_and_world() -> (ScriptEngine, FlintWorld, i64) {
    let engine = ScriptEngine::new();
    let mut world = FlintWorld::new();
    let id = world.spawn("cube").unwrap();
    world
        .set_field(
            id,
            "transform",
            "position",
            toml::Value::Array(vec![toml::Value::Float(0.0); 3]),
        )
        .unwrap();
    (engine, world, id.raw() as i64)
}

fn eval<T: Clone + Send + Sync + 'static>(
    engine: &ScriptEngine,
    world: &mut FlintWorld,
    src: &str,
) -> Result<T, String> {
    let _scope = WorldScope::new(&engine.ctx, world);
    engine
        .rhai_engine()
        .eval::<T>(src)
        .map_err(|e| e.to_string())
}

#[test]
fn every_declared_function_has_a_doc_and_named_parameters() {
    let engine = ScriptEngine::new();
    let entries = engine.api_registry().entries();
    assert!(!entries.is_empty());
    for e in entries {
        assert!(
            !e.doc.trim().is_empty(),
            "{}::{} has no doc",
            e.module,
            e.name
        );
        assert!(!e.doc.contains('\n'), "{}: one line", e.name);
        for p in &e.params {
            assert!(!p.name.is_empty(), "{}: a parameter has no name", e.name);
        }
    }
}

#[test]
fn the_entity_module_is_declared_whole() {
    let engine = ScriptEngine::new();
    let entity: Vec<&str> = engine
        .api_registry()
        .entries()
        .iter()
        .filter(|e| e.module == "entity")
        .map(|e| e.name)
        .collect();
    for name in [
        "self_entity",
        "get_entity",
        "entity_exists",
        "set_position",
        "get_rotation_quat",
        "set_shear",
        "set_material_emissive",
        "get_joint_position",
        "entity_count_with",
    ] {
        assert!(entity.contains(&name), "{name} not declared");
    }
    // Both arities of the overload
    assert!(engine.api_registry().declares("set_material_emissive", 5));
    assert!(engine.api_registry().declares("set_material_emissive", 6));
    let sig = engine
        .api_registry()
        .entries()
        .iter()
        .find(|e| e.name == "set_position")
        .unwrap()
        .signature();
    assert_eq!(sig, "set_position(id: entity, x: num, y: num, z: num)");
}

#[test]
fn integers_work_where_numbers_are_wanted() {
    let (engine, mut world, id) = engine_and_world();
    let _: () = eval(
        &engine,
        &mut world,
        &format!("set_position({id}, 1, 2.5, -3)"),
    )
    .unwrap();
    let p = world
        .get_transform(flint_core::EntityId::from_raw(id as u64))
        .unwrap()
        .position;
    assert_eq!((p.x, p.y, p.z), (1.0, 2.5, -3.0));
    // And floats, through the exact-typed twin, as before
    let x: f64 = eval(
        &engine,
        &mut world,
        &format!("set_position({id}, 4.0, 5.0, 6.0); get_position({id}).x"),
    )
    .unwrap();
    assert_eq!(x, 4.0);
    // An entity id given as a whole float is an entity id
    let name: String = eval(&engine, &mut world, &format!("entity_name({id}.0)")).unwrap();
    assert_eq!(name, "cube");
}

#[test]
fn a_wrong_argument_names_the_function_the_parameter_and_the_kind() {
    let (engine, mut world, id) = engine_and_world();
    let err = eval::<()>(
        &engine,
        &mut world,
        &format!("let a = 1;\nset_position({id}, \"left\", 0.0, 0.0)"),
    )
    .unwrap_err();
    assert!(err.contains("set_position"), "{err}");
    assert!(err.contains("`x`"), "{err}");
    assert!(err.contains("a number"), "{err}");
    assert!(err.contains("string"), "{err}");
    assert!(err.contains("line 2"), "{err}");
}

#[test]
fn a_missing_entity_returns_the_fallback_and_warns_once() {
    let (engine, mut world, id) = engine_and_world();
    let empty: rhai::Map = eval(&engine, &mut world, "get_position(-1)").unwrap();
    assert!(empty.is_empty());
    let far: f64 = eval(&engine, &mut world, &format!("distance({id}, 9999)")).unwrap();
    assert_eq!(far, f64::MAX);
    let parent: i64 = eval(&engine, &mut world, "get_parent(-1)").unwrap();
    assert_eq!(parent, -1);
    let shear: rhai::Map = eval(&engine, &mut world, "get_shear(-1)").unwrap();
    assert_eq!(shear.len(), 6, "zeros, as before");
    // entity_exists asks, so it doesn't warn
    let exists: bool = eval(&engine, &mut world, "entity_exists(-1)").unwrap();
    assert!(!exists);
    let exists: bool = eval(&engine, &mut world, &format!("entity_exists({id})")).unwrap();
    assert!(exists);
    let warned = crate::lock_or_recover(&engine.ctx)
        .bad_target_warned
        .clone();
    let mut funcs: Vec<&str> = warned.iter().map(|(_, f)| *f).collect();
    funcs.sort();
    assert_eq!(
        funcs,
        ["distance", "get_parent", "get_position", "get_shear"]
    );
}

#[test]
fn no_world_is_a_script_error_not_a_panic() {
    let engine = ScriptEngine::new();
    let err = engine
        .rhai_engine()
        .eval::<i64>("get_entity(\"player\")")
        .unwrap_err()
        .to_string();
    assert!(err.contains("get_entity: there is no world here"), "{err}");
    // A setter with no world does nothing, as it always has
    let _: () = engine
        .rhai_engine()
        .eval("set_position(1, 0.0, 0.0, 0.0)")
        .unwrap();
}

#[test]
fn an_extension_declares_through_the_same_builder() {
    let mut system = crate::ScriptSystem::new();
    {
        let mut api = system.script_api();
        let mut m = api.module("game");
        m.func("game_double", "Twice the number.")
            .arg("n", Num)
            .returns(Num, "")
            .run(|_, (n,)| n * 2.0);
        m.func("game_hot", "A hot function on Rhai's typed path.")
            .arg("id", Entity)
            .arg("x", Num)
            .run_typed(|call, (id, x)| {
                let world = call.world_mut()?;
                let _ = world.set_field(id, "transform", "position", {
                    toml::Value::Array(vec![toml::Value::Float(x); 3])
                });
                Ok(())
            });
        m.func("game_tint", "A colour, parsed.")
            .arg("c", Color)
            .returns(Str, "")
            .run(|_, (c,)| format!("{:.2} {:.2} {:.2} {:.2}", c[0], c[1], c[2], c[3]));
    }
    let listing = system.api_listing();
    let hot = listing
        .declared
        .iter()
        .find(|e| e.name == "game_hot")
        .unwrap();
    assert!(hot.typed);
    assert_eq!(hot.module, "game");
    assert_eq!(hot.signature(), "game_hot(id: entity, x: float)");

    let rhai = system.rhai_engine_mut();
    assert_eq!(rhai.eval::<f64>("game_double(21)").unwrap(), 42.0);
    assert_eq!(
        rhai.eval::<String>("game_tint(\"#ff800080\")").unwrap(),
        "1.00 0.50 0.00 0.50"
    );
    assert_eq!(
        rhai.eval::<String>("game_tint([1, 0.5, 0])").unwrap(),
        "1.00 0.50 0.00 1.00"
    );
    assert!(rhai.eval::<String>("game_tint(\"red\")").is_err());
    // The typed path converts nothing: an int isn't a float there
    assert!(rhai.eval::<()>("game_hot(1, 2)").is_err());

    let mut world = FlintWorld::new();
    let id = world.spawn("box").unwrap();
    {
        let _scope = WorldScope::new(&system.engine.ctx, &mut world);
        let src = format!("game_hot({}, 3.0)", id.raw());
        let _: Dynamic = system.rhai_engine_mut().eval(&src).unwrap();
    }
    assert_eq!(world.get_transform(id).unwrap().position.y, 3.0);
}

#[test]
fn every_callback_is_listed_at_its_arity() {
    for &(name, arity) in crate::engine::CALLBACK_ARITIES {
        let doc = super::registry::CALLBACKS
            .iter()
            .find(|c| c.name == name)
            .unwrap_or_else(|| panic!("{name} has no listing"));
        assert_eq!(doc.params.len(), arity, "{name}");
    }
}

#[cfg(feature = "metadata")]
#[test]
fn the_listing_names_the_undeclared_functions_too() {
    let system = crate::ScriptSystem::new();
    let listing = system.api_listing();
    assert!(listing.undeclared_listed);
    let undeclared: Vec<&str> = listing.undeclared.iter().map(|f| f.name.as_str()).collect();
    assert!(undeclared.contains(&"draw_rect"), "{undeclared:?}");
    assert!(
        !undeclared.contains(&"set_position"),
        "declared, not undeclared"
    );
}
