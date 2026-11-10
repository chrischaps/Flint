# ADR 0116: A declared script API: one registration layer that lists, checks and coerces

Status: proposed, 2026-11-10 (#695, with #694, #696, #697, #698, #701, #702;
audit epic #671). Not accepted: the "Decisions needed" at the end are Chris's
to make before work starts.

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
- Integer arguments start working everywhere, which is additive. A few
  scripts may rely on an overload pair that the new kinds merge; the port
  keeps existing overloads by arity.
- The web build must not carry the registry's doc strings: the registry is
  compiled for native builds, and the wasm build keeps registration only.
- `flint script api` on the plain CLI lists the engine's functions. A game
  binary with extensions lists its own by exposing the same command.

## Decisions needed

1. **Builder with argument kinds (recommended), or Rhai plugin modules?**
   The builder is more code of ours and solves coercion, entity handling and
   listing together. Plugin modules are less code and solve listing only.
2. **The overhead gate.** What per-call cost is acceptable on the phone
   before step 2 goes ahead? Proposed: no more than 5% on the heaviest
   script-drawn demo's frame time; hot draw functions keep typed
   registrations if it is exceeded.
3. **Deprecated names** (`this_entity`, `log_info`, `draw_line_3d`, the
   twelve single-purpose post setters): how long do they stay? Proposed: one
   release marked deprecated in the listing and flagged by the checker, then
   removed, with a changelog entry (#765).
