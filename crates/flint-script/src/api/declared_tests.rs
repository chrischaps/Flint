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
    // The first-draft call #696 names, all ints
    let y: f64 = eval(
        &engine,
        &mut world,
        &format!("set_position({id}, 0, 1, 0); get_position({id}).y"),
    )
    .unwrap();
    assert_eq!(y, 1.0);
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
fn the_ported_modules_take_ints_for_numbers_and_whole_floats_for_ints() {
    let (engine, mut world, id) = engine_and_world();
    let _: () = eval(
        &engine,
        &mut world,
        &format!(
            "set_render_mode(1.0, 1); set_camera_fov(60); set_time_scale(2); \
             set_fog_color(1, 0, 0); play_sound(\"blip\", 1, 2); \
             set_anim_layer({id}, 1.0, \"wave\", 1); set_velocity_2d({id}, 3, 4)"
        ),
    )
    .unwrap();
    let pressed: bool = eval(&engine, &mut world, "is_mouse_pressed(0.0)").unwrap();
    assert!(!pressed);
    {
        let c = crate::lock_or_recover(&engine.ctx);
        assert_eq!(c.requests.render_mode, Some((1, 1.0)));
        assert_eq!(c.requests.camera.fov, Some(60.0));
        assert_eq!(c.requests.time_scale, Some(2.0));
        assert_eq!(c.requests.fog_color, Some([1.0, 0.0, 0.0]));
        assert_eq!(c.commands.len(), 2, "the sound and the velocity");
    }
    let eid = flint_core::EntityId::from_raw(id as u64);
    let layers = world
        .get_components(eid)
        .and_then(|c| c.get_field("animator", "layers").cloned())
        .and_then(|v| v.as_array().cloned())
        .unwrap();
    assert_eq!(layers.len(), 2, "slot 0 padded, slot 1 set");
    // A fractional float is not an int
    let err = eval::<()>(&engine, &mut world, "is_mouse_pressed(0.5)").unwrap_err();
    assert!(
        err.contains("`button`") && err.contains("an integer"),
        "{err}"
    );
}

/// `.run_pure()` (the math module) never takes the call context's lock:
/// these calls run while the test holds it, which `.run()` would deadlock
/// on. Its entry and its errors are `.run()`'s.
#[test]
fn pure_functions_run_without_the_call_context() {
    let engine = ScriptEngine::new();
    let guard = crate::lock_or_recover(&engine.ctx);
    let v: f64 = engine
        .rhai_engine()
        .eval("lerp(0, 10, 0.5) + sin(0.0) + clamp(5, 0, 3) + random_range(1, 1)")
        .unwrap();
    assert_eq!(v, 9.0);
    let err = engine
        .rhai_engine()
        .eval::<f64>("let a = 1;\nsin(\"x\")")
        .unwrap_err()
        .to_string();
    drop(guard);
    for want in ["sin: argument `x` must be a number", "string", "line 2"] {
        assert!(err.contains(want), "{err}");
    }
    let lerp = engine
        .api_registry()
        .entries()
        .iter()
        .find(|e| e.name == "lerp")
        .unwrap();
    assert!(!lerp.typed);
    assert_eq!(lerp.module, "math");
    assert_eq!(lerp.signature(), "lerp(a: num, b: num, t: num) -> num");
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

/// A script value of each kind, for calling every declared function.
fn sample(kind: &str, live: i64) -> String {
    match kind {
        "num" | "float" => "1.0".into(),
        "int" | "any" => "0".into(),
        "bool" => "true".into(),
        "entity" | "entity?" => live.to_string(),
        "str" => "\"x\"".into(),
        "map" => "#{}".into(),
        "array" => "[]".into(),
        "color" | "color array" => "[1.0, 1.0, 1.0]".into(),
        "fn" => "Fn(\"f\")".into(),
        other => panic!("no sample for kind {other}"),
    }
}

/// #701: every declared function with an entity parameter, given `-1` or a
/// despawned id there, skips its body, warns (once per script and
/// function), returns its fallback and queues nothing for the host.
#[test]
fn every_function_taking_an_entity_warns_once_on_a_dead_id() {
    let (engine, mut world, live) = engine_and_world();
    let gone = world.spawn("gone").unwrap();
    world.despawn(gone).unwrap();
    let entries = engine.api_registry().entries().to_vec();
    let mut checked = Vec::new();
    for e in &entries {
        for (i, p) in e.params.iter().enumerate() {
            if p.kind != "entity" {
                continue;
            }
            for dead in [-1, gone.raw() as i64] {
                let args: Vec<String> = e
                    .params
                    .iter()
                    .enumerate()
                    .map(|(j, q)| {
                        if j == i {
                            dead.to_string()
                        } else {
                            sample(q.kind, live)
                        }
                    })
                    .collect();
                let src = format!("{}({})", e.name, args.join(", "));
                crate::lock_or_recover(&engine.ctx)
                    .bad_target_warned
                    .clear();
                for _ in 0..2 {
                    if let Err(err) = eval::<Dynamic>(&engine, &mut world, &src) {
                        panic!("{src}: {err}");
                    }
                }
                let c = crate::lock_or_recover(&engine.ctx);
                let warned = c
                    .bad_target_warned
                    .iter()
                    .filter(|(_, f)| *f == e.name)
                    .count();
                assert_eq!(warned, 1, "{src} warned {warned} times");
                assert!(
                    c.commands.is_empty(),
                    "{src} queued a command for a dead id: {:?}",
                    c.commands
                );
            }
            checked.push(e.name);
        }
    }
    // The ported modules' entity functions are all here
    for name in [
        "set_position",
        "play_clip",
        "set_anim_layer",
        "set_anim_layer_ex",
        "play_sequence",
        "audio_position",
        "set_velocity_2d",
        "get_velocity_2d",
        "set_sprite_tint",
        "get_sprite_layer",
        "play_sprite",
        "sprite_play",
        "set_text",
        "set_text_color",
        "follow_with_camera",
        "camera_follow",
        "move_character",
        "get_collider_extents",
        "emit_burst",
        "start_emitter",
        "set_emission_rate",
        "set_entity_style",
        "set_style_param",
        "tween_style_param",
        "clear_entity_style",
        "get_entity_style",
        "get_style_param",
        "spline_closest_point",
        "spline_sample_at",
    ] {
        assert!(checked.contains(&name), "{name} not checked");
    }
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

/// ADR 0116 step 2 is done: the engine registers nothing outside the
/// builder, so the undeclared part is what an extension adds raw, and a
/// deterministic run's `timestamp()` is declared too.
#[cfg(feature = "metadata")]
#[test]
fn only_an_extensions_raw_registration_is_undeclared() {
    let mut system = crate::ScriptSystem::new();
    system.make_deterministic(7);
    system.make_deterministic(8);
    let listing = system.api_listing();
    assert!(listing.undeclared_listed);
    let undeclared: Vec<&str> = listing
        .undeclared
        .iter()
        .map(|f| f.signature.as_str())
        .collect();
    assert!(undeclared.is_empty(), "{undeclared:?}");
    let stamps = listing
        .declared
        .iter()
        .filter(|e| e.name == "timestamp")
        .count();
    assert_eq!(stamps, 1, "declared once, however often it's asked for");

    system
        .rhai_engine_mut()
        .register_fn("game_raw", |n: i64| n + 1);
    let listing = system.api_listing();
    let undeclared: Vec<&str> = listing.undeclared.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(undeclared, ["game_raw"]);
}

/// Batch 3's modules (#696): ints where numbers are wanted, whole floats
/// where ints are, the same commands either way.
#[test]
fn the_last_modules_take_ints_for_numbers_and_whole_floats_for_ints() {
    use crate::context::ScriptCommand;
    let (engine, mut world, id) = engine_and_world();
    let commands = |world: &mut FlintWorld, src: &str| -> Vec<String> {
        let _: Dynamic = eval(&engine, world, src).unwrap_or_else(|e| panic!("{src}: {e}"));
        let mut c = crate::lock_or_recover(&engine.ctx);
        c.commands.drain(..).map(|c| format!("{c:?}")).collect()
    };
    let ints = commands(
        &mut world,
        &format!(
            "set_wooze(#{{ step: 1 }}, 2); set_wooze_zone(\"z\", #{{}}, 3); \
             remove_wooze_zone(\"z\", 1); tween_style_param({id}, \"cell\", 8, 1); \
             tween_style_param({id}, \"cell\", 8, 1, \"linear\"); emit_burst({id}, 3.0); \
             set_effect_param(1.0, \"scale\", 2); load_chunk(\"c.scene.toml\", 1, 2, \"c\")"
        ),
    );
    let floats = commands(
        &mut world,
        &format!(
            "set_wooze(#{{ step: 1 }}, 2.0); set_wooze_zone(\"z\", #{{}}, 3.0); \
             remove_wooze_zone(\"z\", 1.0); tween_style_param({id}, \"cell\", 8, 1.0); \
             tween_style_param({id}, \"cell\", 8, 1.0, \"linear\"); emit_burst({id}, 3); \
             set_effect_param(1, \"scale\", 2.0); load_chunk(\"c.scene.toml\", 1.0, 2.0, \"c\")"
        ),
    );
    assert_eq!(ints.len(), 8, "{ints:?}");
    assert_eq!(ints, floats);
    let eid = flint_core::EntityId::from_raw(id as u64);
    assert_eq!(
        ints[5],
        format!(
            "{:?}",
            ScriptCommand::EmitBurst {
                entity: eid,
                count: 3
            }
        )
    );
    // Pure reads take ints too
    let h: f64 = eval(
        &engine,
        &mut world,
        "terrain_height(1, 2) + ocean_height(0, 0)",
    )
    .unwrap();
    assert_eq!(h, 0.0);
    let gap: bool = eval(&engine, &mut world, &format!("spline_is_gap({id}, 0)")).unwrap();
    assert!(!gap, "no spline_data: no gap");
    // A fractional float is not an int
    let err = eval::<()>(&engine, &mut world, &format!("emit_burst({id}, 2.5)")).unwrap_err();
    assert!(
        err.contains("`count`") && err.contains("an integer"),
        "{err}"
    );
    // A tween's seconds must be a number: an error now, not a warning queued
    let err = eval::<()>(
        &engine,
        &mut world,
        &format!("tween_style_param({id}, \"cell\", 8, \"soon\")"),
    )
    .unwrap_err();
    assert!(err.contains("`seconds`"), "{err}");
}

/// Rhai hands a raw function its first argument by reference when the
/// script passed a variable there, so a kind that takes its value (a
/// string, map, array, any or fn) must leave that one in place: the
/// script's variable is still there after the call. `showcase_hud.rhai`
/// drew `()` for a string it had measured the line before.
#[test]
fn a_variable_passed_first_is_left_as_it_was() {
    let (engine, mut world, _) = engine_and_world();
    let kept: rhai::Map = eval(
        &engine,
        &mut world,
        "let s = \"SPARKS: 3\"; let w = measure_text(s, 16.0); \
         let n = \"cube\"; get_entity(n); \
         let k = #{ msaa: 1 }; set_quality(k); \
         let f = \"x.toml\"; load_data(f); \
         let m = \"hello\"; log(m); \
         let a = [[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]]; draw_polygon(a, 1, 1, 1, 1, 0); \
         #{ s: s, w: w, n: n, k: k, f: f, m: m, a: a }",
    )
    .unwrap();
    assert_eq!(kept["s"].clone().into_string().unwrap(), "SPARKS: 3");
    assert!(kept["w"].is_map());
    assert_eq!(kept["n"].clone().into_string().unwrap(), "cube");
    assert_eq!(kept["k"].clone().cast::<rhai::Map>().len(), 1);
    assert_eq!(kept["f"].clone().into_string().unwrap(), "x.toml");
    assert_eq!(kept["m"].clone().into_string().unwrap(), "hello");
    assert_eq!(kept["a"].clone().into_array().unwrap().len(), 3);
}

#[test]
fn the_last_modules_are_declared_under_their_subsystems() {
    let engine = ScriptEngine::new();
    let reg = engine.api_registry();
    for (module, name, arity) in [
        ("quality", "set_quality", 1),
        ("quality", "set_quality", 2),
        ("conducted", "conducted_cues", 0),
        ("ocean", "ocean_normal", 2),
        ("spline", "spline_sample_at", 2),
        ("particle", "play_effect", 4),
        ("style", "tween_style_param", 5),
        ("style", "set_look", 1),
        ("expression", "expression", 0),
        ("state", "register_state", 2),
        ("scene", "quit_game", 0),
        ("wooze", "remove_wooze_zone", 2),
        ("living", "tune_living", 1),
        ("living", "clear_living", 2),
        ("debug", "compare_living", 2),
        ("debug", "stop_comparing_living", 0),
        ("persistence", "persist_slot", 1),
        ("data", "load_data", 2),
        ("terrain", "terrain_height", 2),
        ("chunk", "is_chunk_loaded", 1),
    ] {
        let e = reg
            .entries()
            .iter()
            .find(|e| e.name == name && e.params.len() == arity)
            .unwrap_or_else(|| panic!("{name}/{arity} not declared"));
        assert_eq!(e.module, module, "{name}");
    }
}

/// The draw module: an all-int call, a float call and a mixed call of the
/// same draw make the same command, and `layer` takes a whole float
/// (#696; ADR 0116, Decisions, 2026-11-11).
#[test]
fn draws_take_ints_floats_and_a_whole_float_layer_alike() {
    let (engine, mut world, _) = engine_and_world();
    let mut draw = |src: &str| -> Vec<String> {
        let _: () = eval(&engine, &mut world, src).unwrap_or_else(|e| panic!("{src}: {e}"));
        engine
            .drain_draw_commands()
            .into_iter()
            .map(|item| format!("{:?}", item.cmd))
            .collect()
    };
    let ints = draw("draw_rect(10, 10, 100, 50, 1, 0, 0, 1)");
    assert_eq!(ints.len(), 1);
    assert_eq!(
        ints,
        draw("draw_rect(10.0, 10.0, 100.0, 50.0, 1.0, 0.0, 0.0, 1.0)")
    );
    assert_eq!(ints, draw("draw_rect(10, 10.0, 100, 50.0, 1, 0.0, 0, 1.0)"));
    let layered = draw("draw_rect_ex(1, 2, 3, 4, 1, 1, 1, 1, 0, 2)");
    assert_eq!(
        layered,
        draw("draw_rect_ex(1.0, 2.0, 3.0, 4.0, 1.0, 1.0, 1.0, 1.0, 0.0, 2.0)")
    );
    assert!(layered[0].contains("layer: 2"), "{layered:?}");
    assert_eq!(
        draw("draw_text_ex(4, 5, \"hp\", 16, 1, 1, 1, 1, 3.0)"),
        draw("draw_text_ex(4.0, 5.0, \"hp\", 16.0, 1.0, 1.0, 1.0, 1.0, 3)")
    );
    assert_eq!(
        draw("draw_sprite_ex(0, 0, 8, 8, \"coin\", 0, 0, 1, 1, 1, 1, 1, 1, 1)").len(),
        1,
        "fourteen parameters"
    );
    let err = eval::<()>(
        &engine,
        &mut world,
        "draw_rect_ex(1, 2, 3, 4, 1, 1, 1, 1, 0, 0.5)",
    )
    .unwrap_err();
    assert!(
        err.contains("`layer`") && err.contains("an integer"),
        "{err}"
    );
}

/// Only functions with four or more `Num` parameters get the all-int copy.
#[test]
fn the_all_int_copy_is_for_four_or_more_numbers() {
    use super::builder::kind::Int;
    use super::builder::{Kinds, INT_COPY_MIN_NUMS};
    use std::any::TypeId;
    assert_eq!(INT_COPY_MIN_NUMS, 4);
    assert_eq!(<(Num, Num, Num) as Kinds>::int_copy_types(), None);
    assert_eq!(
        <(Num, Str, Num, Num, Num, Int) as Kinds>::int_copy_types(),
        Some(vec![
            TypeId::of::<i64>(),
            TypeId::of::<rhai::ImmutableString>(),
            TypeId::of::<i64>(),
            TypeId::of::<i64>(),
            TypeId::of::<i64>(),
            TypeId::of::<i64>(),
        ])
    );
    // An entity is an int already; four numbers after it
    assert_eq!(
        <(Entity, Num, Num, Num, Num) as Kinds>::int_copy_types(),
        Some(vec![TypeId::of::<i64>(); 5])
    );
}

#[cfg(feature = "metadata")]
#[test]
fn a_many_number_function_registers_its_all_int_copy() {
    let engine = ScriptEngine::new();
    let sigs: Vec<String> = engine
        .rhai_engine()
        .gen_fn_signatures(false)
        .into_iter()
        .filter(|s| s.starts_with("draw_rect(") || s.starts_with("set_position("))
        .collect();
    // The eight-number form (the opts form is draw_rect/5)
    let rects = sigs
        .iter()
        .filter(|s| s.starts_with("draw_rect(") && s.matches(',').count() == 7)
        .count();
    assert_eq!(
        rects, 3,
        "the float twin, the all-int copy, Dynamic: {sigs:?}"
    );
    let positions = sigs
        .iter()
        .filter(|s| s.starts_with("set_position("))
        .count();
    assert_eq!(positions, 2, "three numbers: no copy: {sigs:?}");
}

#[test]
fn the_draw_and_ui_modules_are_declared() {
    let engine = ScriptEngine::new();
    let reg = engine.api_registry();
    for (module, name, arity) in [
        ("draw", "draw_rect", 8),
        ("draw", "draw_rect", 5),
        ("draw", "draw_text_stroked_ex", 14),
        ("draw", "draw_rect_gradient", 14),
        ("draw", "draw_rect_gradient", 5),
        ("draw", "screen_width", 0),
        ("draw_world", "draw_world_text", 6),
        ("view", "world_to_screen", 3),
        ("ui", "load_ui", 1),
        ("screen_ui", "set_text", 2),
        ("sprite", "play_sprite", 2),
        ("sprite", "sprite_play", 2),
        ("camera_2d", "follow_with_camera", 6),
        ("touch", "touch_x", 1),
        ("touch", "touch_id", 1),
        ("physics", "move_character", 4),
        ("entity", "find_nearest_interactable", 0),
    ] {
        let e = reg
            .entries()
            .iter()
            .find(|e| e.name == name && e.params.len() == arity)
            .unwrap_or_else(|| panic!("{name}/{arity} not declared"));
        assert_eq!(e.module, module, "{name}");
    }
}
