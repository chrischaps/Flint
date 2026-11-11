# ADR 0116: A declared script API: one registration layer that lists, checks and coerces

Status: accepted, 2026-11-10 (#695, with #694, #696, #697, #698, #701, #702; audit epic #671)

## Context

The Rhai API is 326 function names registered as 344 hand-written closures,
almost all in one 6.3k-line file (`crates/flint-script/src/api.rs`), through
31 `register_*_api` functions called from one `register_all` (line numbers at
`54f1f6ba`). Each closure looks like this (`api.rs:590-611`):

```rust
// set_position(id: i64, x: f64, y: f64, z: f64)
{
    let ctx = ctx.clone();
    engine.register_fn("set_position", move |id: i64, x: f64, y: f64, z: f64| {
        let mut c = crate::lock_or_recover(&ctx);
        let Some(eid) = c.setter_target("set_position", id) else { return; };
        let world = unsafe { c.world_mut() };
        ...
    });
}
```

The signature and meaning exist only in the comment. Everything the audit
found about scripting follows from that:

- **No listing.** An agent learns the API from a 1,684-line book page or the
  Rust. The book is hand-kept (21 functions are missing from the reference
  page; one documented function, `mouse_position()`, doesn't exist).
- **No static check.** Nothing compiles a `.rhai` before the game runs, and
  an unknown function or wrong argument count surfaces only when that line
  executes (#694).
- **Strict numbers.** Rhai doesn't coerce, so `set_position(id, 0, 1, 0)` is
  "Function not found" at run time. Three functions register an integer twin;
  about 300 don't (#696).
- **Uneven entity handling.** 15 functions warn once on a dead id
  (`setter_target`, #635), 48 no-op in silence, and command-queuing functions
  pass `-1` through as `u64::MAX` (#701).
- **`unsafe` in every closure** (93 blocks), over a `world_mut(&self)` that
  hands out `&mut FlintWorld` from a shared borrow, and a load-time path that
  runs with no world at all and panics (#692).
- **Two host dispatchers.** 42 `ScriptCommand` variants are matched once in
  the player and again, incompletely, for headless runs; ten more effects use
  separate `take_*` channels. A new host-affecting function is a five-file
  edit (#697).
- **No modules.** One rhythm game's 22 `conducted_*` functions and the Living
  Props debug calls are in every game's API (#698).

## Decision

1. **Functions are declared through one builder that records what it
   registers.**

   ```rust
   api.module("entity")
      .func("set_position", "Move an entity to a position in its parent's space.")
      .args(("id", Entity), ("x", Num), ("y", Num), ("z", Num))
      .run(|call, (id, x, y, z)| {
          call.world().set_field(id, comp::TRANSFORM, "position", vec3(x, y, z));
      });
   ```

   The builder registers the Rhai function and appends an `ApiEntry
   { module, name, params: [(name, kind)], returns, doc }` to a registry.
   From the registry come `flint script api [--json]`, the book's tables, a
   `.d.rhai` definitions file, and the name-and-arity table the checker
   uses. A test fails when an entry has no doc.

2. **Argument kinds do the conversions once.** `Num` accepts an int or a
   float. `Int` accepts a whole float. `Entity` resolves an id to a live
   entity, warning once per script and function when it is dead or `-1` (the
   #635 behaviour, for every function). `Str`, `Map`, `Array`, `Color`
   (array or `"#rrggbb"`) and `Any` complete the set. An argument that can't
   convert is a script error that names the function, the parameter and the
   expected kind, with the call's file and line.

3. **One safe way to the world.** The `call` handle gives `world()` and
   `world_mut()` and returns a script error when no world is in scope. The
   raw pointer and its `unsafe` live in `context.rs` only. The load-time pass
   gets a scope of its own (#692).

4. **One host interface for effects.** `ScriptCommand` stays, the ten
   `take_*` channels fold into it, and a `ScriptHost` trait has one method
   per command with a default that reports "not supported in this host". The
   player and the headless runner implement the trait; there is one `match`.
   A test asserts that every variant is either applied headless or on a
   named presentation-only list.

5. **Modules.** Each `api.module(name)` is listed as such. A subsystem
   registers its own (`conducted` from the music layer, `living` and `wooze`
   from living props, `debug` for A/B and commit calls), enabled when the
   subsystem is present (ADR 0117). Calling into a module that isn't enabled
   is an error that says so. Game extensions use the same builder, so their
   functions are listed too.

6. **The file splits along the modules**: `api/<module>.rs`, one `register`
   each.

## Steps

One agent owns #695 and carries out step 1 under its one claim, a PR a
step (or several, where a step says so): `Part of #695` on each PR but the
last, `Closes #695` on the last. The ticket's note has the details. A step
that names another ticket is that ticket's work, and waits its turn through
the tickets' blocked-by links. The steps get no sub-issues.

1. The builder, registry and `flint script api --json`, with one module
   (`entity`) ported as the pattern, and a measurement of call overhead
   (see below) (#695).
2. Port the remaining modules, a few per PR, deleting each
   `register_*_api` as it goes; the file split falls out (#697). Coercion
   (#696) and uniform entity handling (#701) arrive with each ported module.
3. `ScriptHost` and the single dispatcher (#697).
4. `flint script check`, on the registry (#694). Naming conventions and
   opts-map forms (#702). Modules owned by subsystems (#698).

## Alternatives considered

- **Rhai plugin modules (`#[export_module]`).** Rhai's own answer: doc
  comments, metadata and definitions for free. But plugin functions are free
  functions, and every Flint function needs the call context that the
  closures capture today. It could ride in the engine's default tag and be
  fetched from `NativeCallContext` in each function, which trades one piece
  of boilerplate for another and gives no coercion or entity resolution.
- **Metadata dump only** (`Engine::gen_fn_signatures`, Rhai's `metadata`
  feature, CLI builds only). A day's work and worth doing as a stopgap: it
  lists names and parameter types. It gives no parameter names or docs for
  closures, and fixes none of the other six problems.
- **A macro that emits both `i64` and `f64` overloads.** Keeps Rhai's typed
  dispatch, but a function with `n` numeric arguments needs `2^n`
  registrations, so it stops at three or four arguments, and `draw_rect` has
  eight.

## Consequences

- **Dispatch cost.** Taking arguments as `Dynamic` and converting them costs
  more per call than Rhai's typed fast path. Scripts make thousands of draw
  calls a frame on a HUD-heavy game, and the phone is the tight budget. Step
  1 measures it (a draw-heavy demo scene, desktop and Pixel) before the rest
  is ported, and the builder keeps a typed escape hatch for hot functions.
  Measured in step 1 (docs/design/script-api-overhead.md): on the heaviest
  script-drawn example (146 draw calls a frame) the converting path is no
  slower than the typed closures on desktop (-2.9% in the harness, no
  difference in the player), because `.run()` registers an exact-typed twin
  that float calls land on. An int in a `Num` slot costs Rhai's 2^n search
  per call when the call can't be cached (about 4 us for 8 arguments). The
  phone was not measured.
- Integer arguments start working everywhere, which is additive. A few
  scripts may rely on an overload pair that the new kinds merge; the port
  keeps existing overloads by arity.
- The web build must not carry the registry's doc strings: the registry is
  compiled for native builds, and the wasm build keeps registration only.
- `flint script api` on the plain CLI lists the engine's functions. A game
  binary with extensions lists its own by exposing the same command.

## Decisions

Chris accepted the recommendations on 2026-11-10:

1. **The builder with argument kinds**, not Rhai plugin modules.
2. **The overhead gate** is 5% of frame time on the heaviest script-drawn
   demo, measured on desktop and on the phone in step 1. If it is exceeded,
   hot draw functions keep typed registrations.
3. **Deprecated names** stay for one release, marked in the listing and
   flagged by the checker, then go, with a changelog entry (#765).

Chris decided on 2026-11-11, after step 1's measurement and the first
step 2 batch (docs/design/script-api-overhead.md):

4. **The phone half of the overhead gate is waived.** Desktop passed it in
   step 1 (the converting path measured 2.9% faster than the typed closures
   on the heaviest script-drawn example) and again with the draw functions
   ported. The phone (Pixel 8 Pro, web build) was not measured, in step 1
   or since; that the ratios hold there is an expectation, not a
   measurement.
5. **Integers in many-number functions.** For every function declared with
   `.run()` (or `.run_pure()`) that has four or more `Num` parameters, the
   builder registers an all-int exact copy beside the float twin: ints for
   `Num`, the twin's types for the other kinds. It is automatic, not opt-in
   (`.without_int_copy()` exists, doc-hidden, for the harness), so a call
   like `draw_rect(10, 10, 100, 50, 1, 1, 1, 1)` is found on Rhai's first
   lookup instead of by its 2^n search. Measured: 4301 ns a call before,
   384 ns after, where the float call is 362 ns, when the loop's call is the
   callback's first. A call mixing ints and floats in `Num` slots still
   takes the search (about 4.3 us in that case; within 2% of the old typed float call
   once Rhai has cached it). `flint script check` (#694) will flag int
   literals in hot draw calls; that is not part of step 2.
