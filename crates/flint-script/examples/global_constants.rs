//! What `global::NAME` costs a callback since #1012. Each constant a script
//! reads as `global::` is set up again before every call (Rhai keeps it in
//! the call's global state), from the value the top level made once. This
//! times one `on_update` of a script whose constants are read bare (no
//! cost: the scope holds them) against the same script reading them as
//! `global::`, at one constant and at Float's weather script's 22 arrays.
//!
//! ```bash
//! cargo run --release -p flint-script --example global_constants
//! ```

use flint_ecs::FlintWorld;
use flint_script::engine::ScriptEngine;
use std::time::Instant;

/// weather.rhai's tables in Float: 22 five-entry arrays.
const TABLES: &[&str] = &[
    "WIND_T",
    "AMP_T",
    "CHOP_T",
    "FETCH_T",
    "FOAM_T",
    "RAIN01",
    "CLOUD_COV",
    "CLOUD_DEN",
    "CLOUD_SCL",
    "SUN_S",
    "AMB_S",
    "STAR_S",
    "TINT_M",
    "TINT_A",
    "OC_DARK",
    "FOG_M",
    "FOG_G",
    "GLOW_S",
    "DWELL_MIN",
    "DWELL_MAX",
    "P_DOWN",
    "P_UP",
];

/// A script declaring `names` as five-float arrays whose `on_update` reads
/// each one's first entry, as `global::NAME` or bare.
fn script(names: &[&str], global: bool) -> String {
    let mut src = String::new();
    for name in names {
        src.push_str(&format!("const {name} = [1.0, 2.0, 3.0, 4.0, 5.0];\n"));
    }
    src.push_str("let sum = 0.0;\nfn on_update() {\n");
    for name in names {
        let read = if global {
            format!("global::{name}")
        } else {
            name.to_string()
        };
        src.push_str(&format!("    sum += {read}[0];\n"));
    }
    src.push_str("}\n");
    src
}

/// ns per `on_update`, the best of `rounds` rounds of `n` frames.
fn time(src: &str, n: u32, rounds: u32) -> f64 {
    let mut engine = ScriptEngine::new();
    let mut world = FlintWorld::new();
    let id = world.spawn("bench").unwrap();
    engine.add_script(id, engine.compile(src).unwrap(), "bench.rhai".into());
    engine.call_inits(&mut world);
    let mut best = f64::MAX;
    for _ in 0..rounds {
        let t = Instant::now();
        for _ in 0..n {
            engine.call_updates(&mut world);
        }
        best = best.min(t.elapsed().as_nanos() as f64 / n as f64);
    }
    assert!(engine.errors().is_empty(), "{:?}", engine.errors().all());
    best
}

/// The same, called as before #692: Rhai's `call_fn` on the whole AST,
/// which runs every top-level statement again before the callback. A bare
/// `call_fn`, so without the engine's per-batch work, which favours it.
fn time_before_692(src: &str, n: u32, rounds: u32) -> f64 {
    let engine = ScriptEngine::new();
    let rhai = engine.rhai_engine();
    let ast = engine.compile(src).unwrap();
    let mut scope = rhai::Scope::new();
    rhai.run_ast_with_scope(&mut scope, &ast).unwrap();
    let mut best = f64::MAX;
    for _ in 0..rounds {
        let t = Instant::now();
        for _ in 0..n {
            rhai.call_fn::<()>(&mut scope, &ast, "on_update", ())
                .unwrap();
        }
        best = best.min(t.elapsed().as_nanos() as f64 / n as f64);
    }
    best
}

fn main() {
    println!(
        "{:<36} {:>10} {:>10} {:>10} {:>14}",
        "constants read", "bare ns", "global ns", "extra ns", "pre-#692 ns"
    );
    for (label, names) in [
        ("1 array", &TABLES[..1]),
        ("22 arrays (Float's weather)", TABLES),
    ] {
        let (n, rounds) = (20_000, 15);
        let mut bare = f64::MAX;
        let mut global = f64::MAX;
        let mut before = f64::MAX;
        // Alternate the sides so a slow moment lands on none
        for _ in 0..3 {
            bare = bare.min(time(&script(names, false), n, rounds));
            global = global.min(time(&script(names, true), n, rounds));
            before = before.min(time_before_692(&script(names, true), n, rounds));
        }
        println!(
            "{label:<36} {bare:>10.0} {global:>10.0} {:>10.0} {before:>14.0}",
            global - bare
        );
    }
}
