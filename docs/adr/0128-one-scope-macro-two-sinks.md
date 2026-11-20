# ADR 0128: One scope macro, two sinks: Tracy and Flint's own recorder

Status: accepted, 2026-11-17 (#1095). Extends [ADR 0109](0109-tracy-frame-profiler.md).

## Context

ADR 0109 put the frame loop's scopes through the `profiling` crate with
Tracy as its backend (flint-player's `profile` feature). Epic #1094 adds a
recorder of Flint's own: a ring that always holds the last few seconds of
scopes, saved to a file after a hitch (#1097), read by `flint profile`
(#1098) and drawn as a flame chart (#1099). Those scopes have to be the same
ones Tracy sees, written once at each site, on the web as well, and cheap
enough to leave compiled in for dev builds.

`profiling` picks one backend at compile time and says so ("Only one may be
active at a time"). Measured: turning on `profile-with-tracy` and
`profile-with-tracing` in one build fails to compile
(`profiling-procmacros`: `impl_block` defined twice). wgpu depends on
`profiling` too, so a second backend asked for by any crate breaks every
build that has the first.

Three ways were tried on the player's frame loop (branch
[`spike/1095-scope-sinks`](https://github.com/chrischaps/Flint/tree/spike/1095-scope-sinks)):

1. `profiling`'s `profile-with-tracing` backend with a `tracing` layer that
   records spans.
2. A scope macro in `flint_core` that writes to a per-thread ring and also
   calls `profiling::scope!`.
3. Tracy alone, with captures saved by `tracy-capture` and exported.

## Decision

**Option 2.** `flint_core::scope!` is the one way Flint marks a scope.

### The macro

```rust
flint_core::scope!("physics::step");              // the rest of the block
flint_core::scope!("load_model", path);           // with text detail
flint_core::scope!("cull", draws.len());          // with a count
flint_core::profile::finish_frame();              // a frame's end, both sinks
```

- One line reaches both sinks. It expands to a `static Site { name, file,
  line }` for the call site, a `Scope` guard for the recorder and
  `profiling::scope!(name)` for Tracy (with the detail as Tracy's zone text).
- The name is a string literal, so the site is a `static` and a record names
  its scope with one pointer. A name made at run time (#1100's script
  functions) needs an interned `&'static Site` per distinct name; that ticket
  adds it.
- **The gate is flint-core's.** The macro is defined twice under `cfg` in
  flint-core, so `cfg(feature = ..)` is never evaluated in the crate that
  writes `scope!`. A crate that scopes its code needs flint-core, which every
  crate already has, and no feature of its own to forward. `profiling` is
  re-exported as `flint_core::profile::__profiling`, so no crate needs it as
  a dependency either.
- `profiling` stays the way to Tracy, so ADR 0109 is unchanged: `profile`
  turns on its Tracy backend for the whole build, and wgpu's own scopes go to
  Tracy with Flint's. They do not reach the recorder.
- Games scope their own code with `flint_core::scope!` too, and their scopes
  reach both sinks. `profiling::scope!` in a game still reaches Tracy alone.

### When it is compiled in

- **The recorder's half** is compiled in under `debug_assertions` or
  flint-core's **`trace`** feature (flint-player forwards it:
  `trace = ["flint-core/trace"]`). A default release build has neither, and
  `scope!` is then only `profiling::scope!`, which is nothing without
  `profile`: a shipped game carries none of it.
- Compiled in, it records only after `flint_core::profile::set_recording(true)`.
  Until then a scope is one relaxed atomic load and a branch.
- **Tracy's half** is ADR 0109's `profile` feature, independent of `trace`.
  Both can be on in one build.
- The feature is `trace`, not `profiler` as the issue suggested: next to
  `profile` (Tracy) and #1101's `profiling` Cargo profile, a third word a
  letter apart would be confused; `trace` is what #1097's `--trace` writes.

### The record

One `Record` per finished scope, 40 bytes:

| field | type | |
|---|---|---|
| `site` | `&'static Site` | name, file, line |
| `start` | `u64` | clock ticks; nanoseconds from the recorder's epoch when read out |
| `dur` | `u64` | ticks |
| `frame` | `u32` | `finish_frame` count when the scope closed |
| `depth` | `u16` | open scopes around it on its thread (0 = outermost) |
| detail | none, `u64` count, or text | text is copied into a 64 KB per-thread text ring and read back only if not yet overwritten |

The thread is the ring's, named from `std::thread::current()`. Records are
written when a scope closes, so a parent follows its children and a scope
still open when the ring is read is not in it (#1097's writer adds the open
stack if it wants the frame under way).

**The clock** is the time-stamp counter on x86_64, as Tracy's is (an
invariant TSC, which every x86_64 CPU Windows 11 runs on has, and which QPC
itself reads), calibrated against `Instant` over the whole run when read out.
Elsewhere, and on the web, `web_time::Instant` (`performance.now()` in a
browser).

**The ring** is per thread, a fixed count of records
(`DEFAULT_RING_CAPACITY`, 65 536 = 2.6 MB a thread, or what
`set_ring_capacity` said before the thread's first record), allocated once
on that first record and never again; the oldest is overwritten. Each ring sits behind an
uncontended `Mutex` in a global list, so a writer on another thread can read
every thread's ring (`with_rings`). Sizing it by seconds is #1097's.

### The file

**Chrome trace-event JSON**: one `"ph":"X"` event per record (`name`, `ts`
and `dur` in µs, `tid`), `args: { frame, depth, detail? }`, and a
`thread_name` metadata event per ring. Perfetto and `chrome://tracing` open
it with no tooling of Flint's, and #1098 / #1099 read it. On the spike a
record is about 112 bytes of JSON (14 914 scopes, 1.67 MB for 4 s of the
player's 16 scopes): five seconds at 60 fps with 500 scopes a frame is about
17 MB.

**No compact binary file.** The ring is the compact form already (40 bytes a
record, and nothing is written per scope but the record). A binary file is
worth adding only if `--trace-seconds 0` (a whole session) proves too large
as JSON; #1097 says so if it does.

## Alternatives considered

**1. `profile-with-tracing` and a recording layer.** Rejected on cost and on
the one-backend rule. A span through the registry and a layer that stamps on
enter and pushes on exit costs 200-225 ns in release and 2.2 µs in a dev
build; with the player's own fmt layer and env-filter beside it (every span
passes through them too), 300 ns and 3.4 µs. It would also have to replace
Tracy as `profiling`'s backend (both can't be on), sending Tracy's half
through `tracing-tracy`, and wgpu's scopes would flood the subscriber.

**3. Tracy alone.** Cheapest (5 ns a scope with the client running and no
viewer), but it can't be the recorder. `tracy-client-sys` does not build for
`wasm32-unknown-unknown` (its build script fails), so the web gets nothing.
With `ondemand` (ADR 0109) the client keeps nothing until a viewer connects,
so a hitch a minute ago is gone; without it the client keeps everything in
memory, without bound, until one does. An agent would need `tracy-capture`
attached for the whole run. It stays what it is: the live, deep view.

## Measured

`crates/flint-core/examples/scope_bench.rs` on the spike branch: a loop of
2 000 000 empty scopes, the best of five runs, nothing subtracted (the loop
body alone is 0.2 ns release, 5.9 ns dev). Windows 11, Intel Core Ultra 9
275HX, 2026-11-17, with the TSC clock. "Release + Tracy" is the same
release build with `profile-with-tracy` on and no viewer connected. A
second run of the same build moved figures by up to 6 ns. The spike names the
feature `profiler`; it is `trace` here.

| per scope | release | release + Tracy | dev build |
|---|---|---|---|
| `scope!`, recorder compiled out (the release default) | 0.2 ns (the loop alone) | | |
| `scope!`, compiled in, recording off | 0.6 ns | 6.4 ns | 12.1 ns (6.3 over the loop) |
| `scope!`, recording on | 41 ns | 35 ns | 64 ns |
| `scope!`, recording on, count detail | 35 ns | 63 ns (Tracy's text) | 73 ns |
| `scope!`, recording on, text detail | 56 ns | 82 ns | 210 ns |
| `scope!`, 3 nested, recording on, per scope | 35 ns | 29 ns | 56 ns |
| `profiling::scope!` -> Tracy, no viewer | | 5.1 ns | |
| `tracing` span, no subscriber | 1.3 ns | 1.3 ns | 15.8 ns |
| `tracing` span -> registry + recording layer | 225 ns | 197 ns | 2 192 ns |
| ... + the player's fmt layer and env-filter | 302 ns | 305 ns | 3 372 ns |
| two `Instant::now()` | 56 ns | 50 ns | 52 ns |
| two `rdtsc` | 17 ns | 13 ns | 18 ns |

- With `Instant::now` as the clock (the first try), recording on was 60 ns
  in release and 99 ns in dev; the TSC took it to 35-41 and 64. Most of what
  is left is the ring's `Mutex` and the thread-local; #1097 may replace the
  lock if a frame's scopes ever make it matter.
- A count's detail costs nothing more in the recorder; under Tracy it
  becomes zone text, which formats a string.
- At those rates the player's 16 scopes cost about 1 µs a frame recording
  in a dev build. #1096's next two levels, say 300 scopes a frame, would be
  about 19 µs in dev and 12 µs in release, against a 16.7 ms frame.
- Tracy with a viewer connected was not measured: running the downloaded
  `tracy-capture` was refused in this session. Tracy's own figure is a few
  nanoseconds a zone; the recorder's half is the same with or without it.

**The web.** `bash scripts/wasm-check.sh` passes with the recorder compiled
in (its dev build has `debug_assertions`), and `cargo check -p flint-core
--release --features trace --target wasm32-unknown-unknown` passes.
Tracy's client does not build for wasm32. The cost of `performance.now()` per
scope in a browser was not measured. Browsers coarsen it on a page that
isn't cross-origin isolated, and GitHub Pages can't send the headers that
isolate one (docs/design/web.md, which already finds one frame's phases
noise there), so on the web short scopes read as 0 and only frame-sized ones
mean anything. #1097 takes the number with `tools/web-perf/run.mjs`.

**Size.** `flint-player` release builds (`cargo build --release -p
flint-player --bin flint-player`, default features):

| build | bytes | |
|---|---|---|
| main before this change | 72 847 360 | |
| this change | 72 845 824 | 1 536 smaller |
| the spike with `--features profiler` (the recorder and its JSON writer) | 72 880 640 | 33 280 larger than main |

The default build is not byte-identical with main's. No scope name is in it
(`publish_style_reader` occurs in the spike's `profiler` build's bytes and in neither
default build), so the difference is layout, not a recorder left behind; the
likeliest cause is flint-render no longer linking `profiling`, which was not
checked further.

**The throwaway branch.** On `spike/1095-scope-sinks`, `flint-player
--features profile` (Tracy and the recorder in one build) run on
`flint-examples/hitch_metronome.scene.toml` with `SPIKE_TRACE=<file>
--exit-after 4` wrote a 1.67 MB Chrome JSON trace: 14 914 scopes, all 16
names, 932 frames, every frame's 16. Its slowest `tick` is 100.9 ms (the
scene's 100 ms busy wait) and no scope below it covers the stall: the scripts
have none yet, which is #1096.

## Consequences

- **What landed with this ADR** (so #1096 and #1097 can start side by side
  from main): `flint_core::profile` (the macro, the ring, the clock,
  `with_rings`, `finish_frame`), the `trace` features, and the player's 16
  scopes on it. Not the file writer, the flags or any call of
  `set_recording`: nothing on main records yet, so in a dev build each scope
  is the 6 ns of a recorder that's off.
- The player's 16 scopes are `flint_core::scope!` lines, named by their
  function (`tick`, `render`, `update_from_world`, ...), and the frame ends
  with `flint_core::profile::finish_frame()`. In Tracy a zone is named by
  that literal instead of the function's path. flint-render no longer
  depends on `profiling`.
- #1096 writes `flint_core::scope!` everywhere and needs no per-crate
  feature (its risk table's mitigation is moot). A count is the cheap detail;
  text is copied, so it belongs on rare scopes (a model load), not a loop.
- #1097 builds on `flint_core::profile`: it sets the ring's size from
  seconds, decides when recording is on (`set_recording`), and writes the
  file this ADR names, from `with_rings`. The spike's `chrome_json`
  (`crates/flint-core/src/profile.rs` on that branch) is a starting point.
- #1100 needs a run-time name: an interned `&'static Site` per distinct
  script function, or a fixed site with the function as text detail.
- wgpu's scopes reach Tracy only.
- A default release build carries none of it: the macro is
  `profiling::scope!`, which is nothing (Size, above). The `trace` feature
  adds the recorder to a release build.
- A game's existing `profiling::scope!` and `#[profiling::function]` still
  reach Tracy; moving them to `flint_core::scope!` adds the recorder.
- Three names sit close together: `profile` (Tracy, a feature), `trace` (the
  recorder, a feature) and #1101's planned `profiling` Cargo profile. The
  book's profiling guide says which is which.

## Sources

- #1095, epic #1094; branch `spike/1095-scope-sinks` (the bench, the spike
  player's `SPIKE_TRACE` writer, `tools/spike-1095/`)
- Code: `crates/flint-core/src/profile.rs`, `crates/flint-core/Cargo.toml`
  (`trace`), `crates/flint-player/Cargo.toml` (`profile`, `trace`)
- Book: [Profiling](../book/src/guides/profiling.md)
