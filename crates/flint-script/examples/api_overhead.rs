//! ADR 0116's overhead measurement: what the builder's converting path
//! costs over today's typed closures, per call and on the heaviest
//! script-drawn example (`examples/seeded_stars.scene.toml`).
//!
//! ```bash
//! cargo run --release -p flint-script --example api_overhead
//! ```
//!
//! Each side is a fresh `ScriptEngine` with the whole API (so Rhai looks
//! calls up among every function a game has) plus the subjects of
//! `flint_script::api::overhead`, registered one way under names nothing
//! else uses (`bench_rect`, `bench_circle`, `bench_set_position`). The demo
//! is `seeded_stars.rhai` itself with its `draw_rect`/`draw_circle` calls
//! renamed to those. Rounds alternate the sides and each side's best round
//! is kept, so a slow moment on a shared machine doesn't land on one side.
//! Results: docs/design/script-api-overhead.md.

use flint_ecs::FlintWorld;
use flint_script::api::overhead::{
    declare_draw, declare_math, declare_set_position, MathPath, Names, Path,
};
use flint_script::context::{InputSnapshot, WorldScope};
use flint_script::engine::ScriptEngine;
use std::time::Instant;

const SIDES: [Path; 5] = [
    Path::Today,
    Path::Typed,
    Path::ConvertingNoIntCopy,
    Path::Converting,
    Path::DynamicOnly,
];

fn side_name(path: Path) -> &'static str {
    match path {
        Path::Today => "typed closure (today)",
        Path::Typed => "builder run_typed",
        Path::ConvertingNoIntCopy => "run, step 1 (no int copy)",
        Path::Converting => "run, step 2 (all-int copy)",
        Path::DynamicOnly => "builder run, no exact twin",
    }
}

fn engine(path: Path) -> ScriptEngine {
    let mut engine = ScriptEngine::new();
    declare_draw(&mut engine.script_api(), path, Names::Bench);
    declare_set_position(&mut engine.script_api(), path);
    engine
}

/// ns per call of `call` over an empty loop's: one `call_fn` a "frame",
/// `calls` calls in it, `frames` frames.
///
/// `before`: what the frame calls before its loop. Rhai keeps its cache of
/// which function a call resolved to in the innermost block that first
/// needed one, so a loop that is a callback's first call resolves afresh
/// every iteration; one native call before the loop keeps the cache for it.
fn per_call(path: Path, call: &str, before: &str, calls: usize, frames: usize) -> f64 {
    per_call_on(engine(path), call, before, calls, frames)
}

/// [`per_call`] on a given engine.
fn per_call_on(
    mut engine: ScriptEngine,
    call: &str,
    before: &str,
    calls: usize,
    frames: usize,
) -> f64 {
    let ctx = engine.script_api().ctx().clone();
    let src = format!(
        "fn frame(id) {{ {before} for i in 0..{calls} {{ {call}; }} }}\n\
         fn empty(id) {{ {before} for i in 0..{calls} {{ }} }}"
    );
    let ast = engine.compile(&src).unwrap();
    let mut world = FlintWorld::new();
    let id = world.spawn("bench").unwrap();
    let raw = id.raw() as i64;
    let _scope = WorldScope::new(&ctx, &mut world);
    let rhai = engine.rhai_engine();
    let time = |f: &str| {
        let mut scope = rhai::Scope::new();
        let start = Instant::now();
        for _ in 0..frames {
            rhai.call_fn::<()>(&mut scope, &ast, f, (raw,)).unwrap();
            engine.drain_draw_commands();
        }
        start.elapsed().as_secs_f64()
    };
    let _ = time("frame"); // warm
    let full = time("frame");
    let empty = time("empty");
    (full - empty) / (frames * calls) as f64 * 1e9
}

/// `seeded_stars.rhai`'s `on_draw_ui`, after four seconds of skies, over
/// `frames` frames. Returns (ms per frame, draws a frame).
fn demo(path: Path, frames: usize) -> (f64, usize) {
    let file = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../examples/scripts/seeded_stars.rhai"
    );
    let src = std::fs::read_to_string(file)
        .unwrap()
        .replace("draw_rect(", "bench_rect(")
        .replace("draw_circle(", "bench_circle(");
    let mut engine = engine(path);
    let ast = engine.compile(&src).unwrap();
    let mut world = FlintWorld::new();
    let id = world.spawn("sky_painter").unwrap();
    engine.add_script(id, ast, "seeded_stars.rhai".into());
    let dt = 1.0 / 60.0;
    let mut t = 0.0;
    engine.provide_context(InputSnapshot::default(), dt, t);
    engine.call_inits(&mut world);
    let step = |engine: &mut ScriptEngine, world: &mut FlintWorld, t: f64| {
        engine.provide_context(InputSnapshot::default(), dt, t);
        engine.call_updates(world);
        engine.drain_commands();
    };
    while t < 4.0 {
        t += dt;
        step(&mut engine, &mut world, t);
        engine.call_draw_uis(&mut world);
        engine.drain_draw_commands();
    }
    let mut draw_time = 0.0;
    let mut draws = 0;
    for _ in 0..frames {
        t += dt;
        step(&mut engine, &mut world, t);
        let start = Instant::now();
        engine.call_draw_uis(&mut world);
        draw_time += start.elapsed().as_secs_f64();
        draws += engine.drain_draw_commands().len();
    }
    (draw_time / frames as f64 * 1e3, draws / frames)
}

fn print_row(label: &str, v: f64, base: f64, unit: &str, note: &str) {
    println!(
        "  {label:<28} {v:>9.1} {unit}  {:>+8.1} {unit}  {:>+7.1}%  {note}",
        v - base,
        (v - base) / base * 100.0
    );
}

/// The pure math subjects (`bench_lerp`, `bench_sin`) three ways: what
/// the math module's lock-free closures cost, what `.run()` (which locks
/// the call context) cost, and `.run_pure()`.
fn math(rounds: usize) {
    const PATHS: [(MathPath, &str); 3] = [
        (MathPath::Today, "typed closure (before)"),
        (MathPath::Locking, "builder run (locks)"),
        (MathPath::Pure, "builder run_pure"),
    ];
    let cases = [
        ("bench_lerp, 3 floats", "bench_lerp(1.0, 2.0, 0.5)"),
        ("bench_sin, a float", "bench_sin(0.5)"),
        ("bench_lerp, 3 ints", "bench_lerp(1, 2, 0)"),
    ];
    println!();
    println!(
        "pure math: ns per call over an empty loop's; best of {rounds} rounds of 400 frames x 1000 calls,"
    );
    println!("screen_width() before the loop (the cache lasts the frame)");
    // An int call is compared with the typed closure's lerp in floats
    let mut lerp_floats = f64::NAN;
    for (label, call) in cases {
        let ints = label.contains("ints");
        let mut best = [f64::MAX; 3];
        for _ in 0..rounds {
            for (i, (path, _)) in PATHS.iter().enumerate() {
                if ints && *path == MathPath::Today {
                    continue;
                }
                let mut engine = ScriptEngine::new();
                declare_math(&mut engine.script_api(), *path);
                let ns = per_call_on(engine, call, "let w = screen_width();", 1000, 400);
                best[i] = best[i].min(ns);
            }
        }
        println!("{label}");
        if label.starts_with("bench_lerp, 3 floats") {
            lerp_floats = best[0];
        }
        let base = if ints { lerp_floats } else { best[0] };
        for ((_, name), ns) in PATHS.iter().zip(best) {
            if ns < f64::MAX {
                print_row(name, ns, base, "ns", "");
            } else {
                println!("  {name:<28}   function not found (an int isn't a float)");
            }
        }
    }
}

fn main() {
    let rounds = 7;
    if std::env::args().any(|a| a == "--math") {
        math(rounds);
        return;
    }
    let cases = [
        (
            "bench_rect, 8 floats",
            "bench_rect(1.0, 2.0, 3.0, 4.0, 0.5, 0.5, 0.5, 1.0)",
            false,
        ),
        (
            "bench_rect, 8 ints",
            "bench_rect(1, 2, 3, 4, 0, 1, 0, 1)",
            true,
        ),
        (
            "bench_rect, mixed (5 of 8 ints)",
            "bench_rect(1, 2, 3, 4, 0.5, 0.5, 0.5, 1)",
            true,
        ),
        (
            "bench_set_position, floats",
            "bench_set_position(id, 1.0, 2.0, 3.0)",
            false,
        ),
        (
            "bench_set_position, ints",
            "bench_set_position(id, 1, 2, 3)",
            true,
        ),
    ];
    println!(
        "ns per call over an empty loop's; best of {rounds} rounds of 400 frames x 1000 calls"
    );
    for (title, before) in [
        (
            "the loop is the frame's first call (Rhai's resolution cache is per iteration)",
            "",
        ),
        (
            "screen_width() before the loop (the cache lasts the frame)",
            "let w = screen_width();",
        ),
    ] {
        println!();
        println!("{title}");
        let mut floats_base = f64::NAN;
        for (label, call, ints) in cases {
            let mut best = [f64::MAX; SIDES.len()];
            for _ in 0..rounds {
                for (i, side) in SIDES.iter().enumerate() {
                    // Typed registrations don't take an int for a float
                    if ints && matches!(side, Path::Today | Path::Typed) {
                        continue;
                    }
                    best[i] = best[i].min(per_call(*side, call, before, 1000, 400));
                }
            }
            println!("{label}");
            // An int call is compared with today's same call in floats
            let base = if ints { floats_base } else { best[0] };
            if !ints {
                floats_base = best[0];
            }
            for (side, ns) in SIDES.iter().zip(best) {
                if ns < f64::MAX {
                    print_row(side_name(*side), ns, base, "ns", "");
                } else {
                    println!(
                        "  {:<28}   function not found (an int isn't a float)",
                        side_name(*side)
                    );
                }
            }
        }
    }

    println!();
    println!("seeded_stars.rhai on_draw_ui: us per frame, best of {rounds} rounds of 600 frames");
    let mut best = [(f64::MAX, 0usize); SIDES.len()];
    for _ in 0..rounds {
        for (i, side) in SIDES.iter().enumerate() {
            let (ms, draws) = demo(*side, 600);
            if ms < best[i].0 {
                best[i] = (ms, draws);
            }
        }
    }
    for (side, (ms, draws)) in SIDES.iter().zip(best) {
        print_row(
            side_name(*side),
            ms * 1e3,
            best[0].0 * 1e3,
            "us",
            &format!("({draws} draws a frame)"),
        );
    }
}
