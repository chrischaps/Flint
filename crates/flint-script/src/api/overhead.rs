//! The ADR 0116 overhead measurement's subjects: the two draw calls the
//! heaviest script-drawn example makes (`draw_circle` per star, `draw_rect`
//! for the backdrop) and the 4-argument `set_position`, each registered
//! several ways. The bodies are the ones in the `draw` and `entity`
//! modules; only the registration differs.
//!
//! Used by `examples/api_overhead.rs` and, for the player A/B, by a build
//! that calls [`declare_draw`] after `PlayerApp::new`
//! (docs/design/script-api-overhead.md). Not part of the API: nothing in
//! the engine calls it.

use super::builder::kind::{Entity, Num};
use super::builder::{Call, ScriptApi};
use crate::context::DrawCommand;
use flint_core::components as comp;

/// How a subject is registered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Path {
    /// A typed closure on the engine, as every function was before ADR 0116
    Today,
    /// The builder's `.run_typed()`: the escape hatch
    Typed,
    /// The builder's `.run()`: converting, with the exact-typed twin and,
    /// for four or more `Num` parameters, the all-int copy (step 2)
    Converting,
    /// `.run()` as step 1 had it: the twin, no all-int copy
    ConvertingNoIntCopy,
    /// `.run()` without the twin: every call is found by Rhai's search
    /// over the mixes of `Dynamic` in place of its arguments' types
    DynamicOnly,
}

/// The names to register under.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Names {
    /// `draw_rect` / `draw_circle`, over the engine's (a registration of
    /// the same name and types replaces that one: `Today` replaces the
    /// float twin and leaves the all-int copy and the `Dynamic` signature)
    Engine,
    /// `bench_rect` / `bench_circle`, which nothing else registers
    Bench,
}

type Rect = (f64, f64, f64, f64, f64, f64, f64, f64);
type Circle = (f64, f64, f64, f64, f64, f64, f64);

fn rect(c: &mut crate::context::ScriptCallContext, (x, y, w, h, r, g, b, a): Rect) {
    c.push_draw(DrawCommand::RectFilled {
        x: x as f32,
        y: y as f32,
        w: w as f32,
        h: h as f32,
        color: [r as f32, g as f32, b as f32, a as f32],
        rounding: 0.0,
        layer: 0,
    });
}

fn circle(c: &mut crate::context::ScriptCallContext, (x, y, radius, r, g, b, a): Circle) {
    c.push_draw(DrawCommand::CircleFilled {
        x: x as f32,
        y: y as f32,
        radius: radius as f32,
        color: [r as f32, g as f32, b as f32, a as f32],
        layer: 0,
    });
}

/// Register `draw_rect` and `draw_circle` (or the `bench_` names) by `path`.
pub fn declare_draw(api: &mut ScriptApi, path: Path, names: Names) {
    let (rect_name, circle_name) = match names {
        Names::Engine => ("draw_rect", "draw_circle"),
        Names::Bench => ("bench_rect", "bench_circle"),
    };
    if path == Path::Today {
        let ctx = api.ctx().clone();
        api.engine_mut().register_fn(
            rect_name,
            move |x: f64, y: f64, w: f64, h: f64, r: f64, g: f64, b: f64, a: f64| {
                rect(&mut crate::lock_or_recover(&ctx), (x, y, w, h, r, g, b, a));
            },
        );
        let ctx = api.ctx().clone();
        api.engine_mut().register_fn(
            circle_name,
            move |x: f64, y: f64, radius: f64, r: f64, g: f64, b: f64, a: f64| {
                circle(
                    &mut crate::lock_or_recover(&ctx),
                    (x, y, radius, r, g, b, a),
                );
            },
        );
        return;
    }
    let mut m = api.module("ui");
    let f = m
        .func(
            rect_name,
            "Fill a rectangle, logical points and 0..1 colour.",
        )
        .arg("x", Num)
        .arg("y", Num)
        .arg("w", Num)
        .arg("h", Num)
        .arg("r", Num)
        .arg("g", Num)
        .arg("b", Num)
        .arg("a", Num);
    let body = |call: &mut Call<'_>, args: Rect| rect(call.ctx(), args);
    match path {
        Path::Typed => f.run_typed(body),
        Path::DynamicOnly => f.without_preferred_signature().run(body),
        Path::ConvertingNoIntCopy => f.without_int_copy().run(body),
        _ => f.run(body),
    }
    let f = m
        .func(
            circle_name,
            "Fill a circle, logical points and 0..1 colour.",
        )
        .arg("x", Num)
        .arg("y", Num)
        .arg("radius", Num)
        .arg("r", Num)
        .arg("g", Num)
        .arg("b", Num)
        .arg("a", Num);
    let body = |call: &mut Call<'_>, args: Circle| circle(call.ctx(), args);
    match path {
        Path::Typed => f.run_typed(body),
        Path::DynamicOnly => f.without_preferred_signature().run(body),
        Path::ConvertingNoIntCopy => f.without_int_copy().run(body),
        _ => f.run(body),
    }
}

/// Register `bench_set_position(id, x, y, z)` by `path`, with the body of
/// the `entity` module's `set_position`.
pub fn declare_set_position(api: &mut ScriptApi, path: Path) {
    fn write(world: &mut flint_ecs::FlintWorld, id: flint_core::EntityId, p: [f64; 3]) {
        if let Some(comps) = world.get_components_mut(id) {
            comps.set_field(
                comp::TRANSFORM,
                "position",
                toml::Value::Array(p.into_iter().map(toml::Value::Float).collect()),
            );
        }
    }
    if path == Path::Today {
        let ctx = api.ctx().clone();
        api.engine_mut().register_fn(
            "bench_set_position",
            move |id: i64, x: f64, y: f64, z: f64| {
                let mut c = crate::lock_or_recover(&ctx);
                let Some(eid) = c.setter_target("bench_set_position", id) else {
                    return;
                };
                if let Some(world) = c.try_world_mut() {
                    write(world, eid, [x, y, z]);
                }
            },
        );
        return;
    }
    let mut m = api.module("entity");
    let f = m
        .func("bench_set_position", "set_position, for the measurement.")
        .arg("id", Entity)
        .arg("x", Num)
        .arg("y", Num)
        .arg("z", Num);
    let body = |call: &mut Call<'_>,
                (id, x, y, z): (flint_core::EntityId, f64, f64, f64)|
     -> super::CallResult<()> {
        write(call.world_mut()?, id, [x, y, z]);
        Ok(())
    };
    match path {
        Path::Typed => f.run_typed(body),
        Path::DynamicOnly => f.without_preferred_signature().run(body),
        _ => f.run(body),
    }
}

/// How a pure math subject is registered (the follow-up to step 2's port
/// of the math module).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MathPath {
    /// A typed closure that touches nothing: the math module before ADR 0116
    Today,
    /// The builder's `.run()`, which locks the call context for every call
    Locking,
    /// The builder's `.run_pure()`, which doesn't
    Pure,
}

/// Register `bench_lerp(a, b, t)` and `bench_sin(x)` by `path`, with the
/// math module's bodies.
pub fn declare_math(api: &mut ScriptApi, path: MathPath) {
    if path == MathPath::Today {
        let engine = api.engine_mut();
        engine.register_fn("bench_lerp", |a: f64, b: f64, t: f64| -> f64 {
            a + (b - a) * t
        });
        engine.register_fn("bench_sin", |x: f64| -> f64 { x.sin() });
        return;
    }
    let mut m = api.module("math");
    let lerp = m
        .func("bench_lerp", "lerp, for the measurement.")
        .arg("a", Num)
        .arg("b", Num)
        .arg("t", Num)
        .returns(Num, "");
    match path {
        MathPath::Locking => lerp.run(|_, (a, b, t)| a + (b - a) * t),
        _ => lerp.run_pure(|(a, b, t)| a + (b - a) * t),
    }
    let sin = m
        .func("bench_sin", "sin, for the measurement.")
        .arg("x", Num)
        .returns(Num, "");
    match path {
        MathPath::Locking => sin.run(|_, (x,)| x.sin()),
        _ => sin.run_pure(|(x,)| x.sin()),
    }
}
