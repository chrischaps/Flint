# What the declared script API costs a call (ADR 0116, step 1)

ADR 0116 declares script functions through a builder that takes arguments as
`Dynamic` and converts them by kind, where every function used to be a typed
closure Rhai calls on its fast path. Chris's gate (the ADR's Decisions): the
converting path may cost at most 5% of frame time on the heaviest
script-drawn demo, desktop and phone, or hot draw functions keep typed
registrations. This is the step 1 measurement, 2026-11-11.

**Verdict: under the gate on desktop, by a wide margin.** On the demo the
converting path measured no slower than today's typed closures (3% faster in
the harness, no difference the player can see). **The phone was not
measured.** One case is slow, and it is new: an integer where a number is
wanted, in a call Rhai can't cache (below).

## The demo

`examples/seeded_stars.scene.toml` is the heaviest script-drawn example: its
`on_draw_ui` makes 146 draw calls a frame (72 stars at two 7-argument
`draw_circle`s each, a `draw_rect` backdrop and a `draw_text_opts`). By a read of their scripts
the other examples draw a few dozen at most. Every argument it passes is a
float.

## How it was measured

Desktop: Intel i7-13700KF, RTX 4070, Windows 11, release builds, a machine
other agents were building on.

- **Harness** (`cargo run --release -p flint-script --example api_overhead`):
  a fresh `ScriptEngine` with the whole API per side, plus the subjects
  (`flint_script::api::overhead`) registered one way under names nothing
  else uses. Per call: one `call_fn` a "frame" with 1000 calls in a loop, 400
  frames, minus the same loop empty. The demo: `seeded_stars.rhai` itself,
  its `draw_rect`/`draw_circle` renamed to the subjects, `on_draw_ui` timed
  over 600 frames after four seconds of skies. Seven rounds alternating the
  sides, best round kept.
- **Player A/B**: `flint-player` release on the demo, `FLINT_VSYNC=0
  --exit-after 22 --perf`, 22 s a run. A is this branch; B is A with
  `overhead::declare_draw(Converting, Engine)` called after `PlayerApp::new`,
  which replaces the engine's typed `draw_rect` and `draw_circle` with the
  builder's (a three-line patch, not committed). C is a second copy of A, the
  control. Seven A runs, seven B, four C, interleaved.

## Per call

ns per call over an empty loop's, best of 7 (`api_overhead`, run 2):

| | 8-arg `draw_rect`, floats | 4-arg `set_position`, floats |
|---|---:|---:|
| typed closure (today) | 337.9 | 302.9 |
| builder `.run_typed()` | 348.3 (+3.1%) | 305.0 (+0.7%) |
| builder `.run()` | **315.2 (-6.7%)** | **292.3 (-3.5%)** |

`.run()` registers each function twice: an exact-typed twin (floats for
`Num`, ints for `Entity`), which Rhai finds on its first lookup, and an
all-`Dynamic` one for every other mix of types. A float call lands on the
twin and pays only the conversions, which cost less than Rhai's own casts of
typed arguments. `.run_typed()` adds a `NativeCallContext` (for the error
position) and is a few ns slower than a bare closure.

## On the demo

`on_draw_ui`, us per frame, best of 7 (harness):

| | us | vs today |
|---|---:|---:|
| typed closure (today) | 143.2 | |
| builder `.run_typed()` | 142.1 | -0.8% |
| builder `.run()` | **139.1** | **-2.9%** |
| `.run()` without the twin | 148.4 | +3.6% |

In the player, the `script ui` phase (which is `on_draw_ui`), mean of each
run's two 10-second windows: A 0.176 ms, B 0.172 ms, control C 0.184 ms. The
frame p50 was 0.97 / 0.98 / 1.10 ms and the average 3.07 / 3.39 / 2.79 ms,
the average swinging 0.96 to 4.44 ms between runs of the same binary (the
machine's hitches, p99 near 45 ms). The A-to-B difference is smaller than
A-to-C, so the player can't see one. Against a 1 ms frame, the harness's
-4 us is -0.4%; the gate is +5%.

## The slow case: an integer in a call Rhai can't cache

Rhai finds a call whose argument types match no registration exactly by
trying every mix of `Dynamic` in their place, 2^n hashes for n arguments,
until it reaches the all-`Dynamic` one. It caches what it found, in the
block where the callback first needed the cache. A loop whose call is the
callback's first native call keeps that cache inside the loop body, which is
dropped every iteration, so the search runs on every call:

| ns per call, ints in `Num` slots | loop is the first call | a native call before it |
|---|---:|---:|
| `draw_rect(1, 2, 3, 4, 0.5, 0.5, 0.5, 1)` | 4326 (+3943 over floats) | 330.5 (-2%) |
| `set_position(id, 1, 2, 3)` | 518 (+166) | 300.6 (-1%) |

So integers are free in the usual callback, which reads `screen_width()` or
the like before its loop (the demo does), and cost about 4 us a call in an
8-argument function when the loop comes first. 146 such calls would be
0.6 ms, over half of this machine's frame. Today those calls don't run at all
("function not found"), so nothing existing gets slower; it is a cost a new
script can walk into.

For the rest of the port (#697), the draw functions can go through `.run()`:
floats are as fast as now and ints work. What to do about the slow case is
open: `flint script check` (#694) could flag an int literal in a hot draw
call, or a function with many `Num` parameters could register a few more
twins (all ints, say). `.run_typed()` is there if a function must refuse
ints instead.

## The web build

The registry is native-only and the builder drops docs and return notes on
wasm. A `web-release` build of `flint-web.wasm` (17.5 MB, this branch) has
none of the entity module's doc strings or return notes in it (searched for
several, e.g. "Move the entity to a position", "its id, or -1 when no entity
has that name"); the parameter names and the argument errors stay, by design.

## Not measured

- **The phone** (Pixel 8 Pro, web build). Rhai's search and the conversions
  are the same code there, and wasm runs it several times slower, so the
  ratios should hold; that is an expectation, not a measurement.
- Calls with map or string arguments (`draw_text_opts`): the demo makes one a
  frame.
