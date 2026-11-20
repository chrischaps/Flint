//! What a script's top level cost before #692: Rhai's `call_fn` runs an
//! AST's statements before the function, so every callback re-ran them.
//! This times one call of an empty callback both ways on the same engine:
//! on the whole AST (before) and on `ScriptInstance::calls`, the functions
//! and top-level imports alone (now).
//!
//! ```bash
//! cargo run --release -p flint-script --example top_level_rerun
//! ```
#![allow(clippy::disallowed_methods)] // an example: reads its files off the disk

use flint_ecs::FlintWorld;
use flint_script::context::WorldScope;
use flint_script::engine::ScriptEngine;
use rhai::{Scope, Stmt, AST};
use std::time::Instant;

/// The script's functions and top-level imports (as `instance::callable`).
fn callable(ast: &AST) -> AST {
    let imports: Vec<Stmt> = ast
        .statements()
        .iter()
        .filter(|s| matches!(s, Stmt::Import(..)))
        .cloned()
        .collect();
    AST::new(imports, ast.shared_lib().clone())
}

/// ns per call, the best of `rounds` rounds of `n` calls.
fn time(engine: &ScriptEngine, scope: &mut Scope, ast: &AST, n: u32, rounds: u32) -> f64 {
    let rhai = engine.rhai_engine();
    let mut best = f64::MAX;
    for _ in 0..rounds {
        let t = Instant::now();
        for _ in 0..n {
            rhai.call_fn::<()>(scope, ast, "on_bench", ()).unwrap();
        }
        best = best.min(t.elapsed().as_nanos() as f64 / n as f64);
        // What a top-level `log` queued, so the queue doesn't grow
        let _ = engine.drain_commands();
    }
    best
}

fn main() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let subjects: Vec<(String, String)> = vec![
        (
            "audio_clock.rhai (12 top-level lets)".into(),
            std::fs::read_to_string(root.join("tests/fixtures/scripts/audio_clock.rhai")).unwrap(),
        ),
        (
            "showcase_game/scripts/zone_hud.rhai (6 lets)".into(),
            std::fs::read_to_string(root.join("showcase_game/scripts/zone_hud.rhai")).unwrap(),
        ),
        (
            "a keeper: get_entity + self_entity + log at the top".into(),
            r#"let door = get_entity("door");
               let me = self_entity();
               log("keeper loaded");
               fn on_update() {}"#
                .into(),
        ),
    ];

    let mut engine = ScriptEngine::new();
    let ctx = engine.script_api().ctx().clone();
    let mut world = FlintWorld::new();
    world.spawn("door").unwrap();
    let _lent = WorldScope::new(&ctx, &mut world);

    println!(
        "{:<56} {:>10} {:>10} {:>8}",
        "script", "before ns", "now ns", "ratio"
    );
    for (name, src) in subjects {
        let bench = engine.compile("fn on_bench() {}").unwrap();
        let ast = engine.compile(&src).unwrap().merge(&bench);
        let calls = callable(&ast);
        let mut scope = Scope::new();
        engine
            .rhai_engine()
            .run_ast_with_scope(&mut scope, &ast)
            .unwrap();
        let (n, rounds) = (20_000, 15);
        let mut before = f64::MAX;
        let mut now = f64::MAX;
        // Alternate the sides so a slow moment lands on neither
        for _ in 0..3 {
            before = before.min(time(&engine, &mut scope, &ast, n, rounds));
            now = now.min(time(&engine, &mut scope, &calls, n, rounds));
        }
        println!(
            "{name:<56} {before:>10.0} {now:>10.0} {:>7.1}x",
            before / now
        );
    }
}
