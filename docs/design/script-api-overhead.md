# What the declared script API costs a call (ADR 0116, steps 1 and 2; opts maps, #702)

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

## Pure functions: `.run_pure()` (step 2)

Step 2 ported the math module (`sin`, `lerp`, `clamp`, ...) through `.run()`,
which locks the `ScriptCallContext` for every call, even for a body that never
touches it. The old math closures locked nothing, and procedural layouts call
them hundreds of times a frame. `.run_pure(|args| ...)` registers the same
signatures (exact-typed twin, converting signature) and the same registry
entry, but its body gets no call handle, so neither signature locks anything.
The math module and `random*` use it.

`cargo run --release -p flint-script --example api_overhead -- --math`,
2026-11-11, same machine as above, with `screen_width()` before the loop.
ns per call over an empty loop's, best of 7 rounds of 400 frames x 1000
calls, two runs:

| | `lerp(1.0, 2.0, 0.5)` | `sin(0.5)` | `lerp(1, 2, 0)` (ints) |
|---|---:|---:|---:|
| typed closure (before ADR 0116) | 159.8 / 157.6 | 118.5 / 117.4 | not found |
| builder `.run()` (locks) | 161.5 / 160.3 | 127.3 / 125.8 | 164.3 / 164.2 |
| builder `.run_pure()` | **154.6 / 153.0** | **120.8 / 119.1** | **155.5 / 157.1** |

The lock cost 8 to 9 ns a call on `sin` (+7%), and about 2 ns on `lerp`, where
the argument handling is a larger share. `.run_pure()` takes that away: `lerp`
measured 3% under the old closure in both runs, and `sin` 1.4 to 2% over it
(about 2 ns, within this machine's noise between runs of identical code).
An int `lerp` costs the same as a float one once the callback has made a
native call before its loop. A loop whose first call is the math function
still pays Rhai's search for ints, which for three parameters is up to 8
hashes.

## The all-int copy, and the draw functions ported (step 2)

Chris's decision of 2026-11-11 (ADR 0116, Decisions 5): a function declared
with `.run()` or `.run_pure()` that has four or more `Num` parameters gets an
all-int exact copy beside its float twin, ints for `Num` and the twin's types
for the other kinds. It is automatic. The second step 2 batch ported the
draw and UI functions (`draw`, `draw_world`, `view`, `ui`, `screen_ui`,
`sprite`, `camera_2d`, `touch`) through `.run()`, so every draw call has its
float twin and, with four or more numbers, its all-int copy: 43 declared
functions have one.

`cargo run --release -p flint-script --example api_overhead`, 2026-11-11,
same machine, busier than in step 1 (the typed closure's float `draw_rect`
read 380 ns where step 1 had 338, and two rows running identical code,
`set_position` floats under step 1's and step 2's `.run()`, read 1161 and
564 ns in the uncached case of run 1). The subjects now include `.run()`
without the copy (`Path::ConvertingNoIntCopy`, step 1's registration). ns per
call over an empty loop's, best of 7 rounds of 400 frames x 1000 calls (run 1
of two; run 2 is below the tables)

**The loop is the callback's first call** (Rhai's lookup cache is per
iteration):

| `bench_rect` | 8 floats | 8 ints | mixed (5 of 8 ints) |
|---|---:|---:|---:|
| typed closure (before ADR 0116) | 380.4 | not found | not found |
| `.run()`, step 1 (no copy) | 361.3 | 4301.0 | 4696.0 |
| `.run()`, step 2 (all-int copy) | 362.0 | **384.2** | 4314.8 |

**`screen_width()` before the loop** (the cache lasts the frame):

| `bench_rect` | 8 floats | 8 ints | mixed |
|---|---:|---:|---:|
| typed closure (before ADR 0116) | 566.9 | not found | not found |
| `.run()`, step 1 (no copy) | 540.3 | 595.2 | 584.5 |
| `.run()`, step 2 (all-int copy) | 545.5 | **569.9** | 575.3 |

The all-int call went from 4.3 us to within 6% of the float call (22 ns)
where Rhai can't cache it, and is as fast as the old typed float call where
it can. A mixed call still takes the search: about 4.3 us uncached, about
the same as a float call cached. `flint script check` (#694) is to flag int
literals in hot draw calls.

Run 2, an hour later on a busier machine, kept the shape and not the
numbers: uncached, floats 383 ns, all ints 644 ns under step 2 against
9356 ns under step 1, mixed about 9.7 us either way; its cached rows swung
up to 2x between registrations that differ in nothing the call reaches
(the typed closure's float call 1230 ns, the all-`Dynamic` one 614 ns), so
run 1's are the ones tabled. One oddity held in both runs: uncached
`bench_set_position` with floats read about 2x slower under the step 1 side
(1161 / 1168 ns) than under step 2 (564 / 582 ns), though the function is
registered identically in both (three numbers: no copy). The two engines
differ only in whether `bench_rect` has its all-int copy, so Rhai's
uncached lookup cost depends on the rest of the registered set in a way not
looked into here; the real engine is the step 2 side.

**The demo.** `seeded_stars.rhai`'s `on_draw_ui` (146 draws a frame, all
floats), us per frame, best of 7 rounds of 600 frames: typed closures 253.3,
`.run_typed()` 253.7, step 1 `.run()` 249.8, step 2 `.run()` 248.0 (-2.1%),
`.run()` without the twin 271.1 (run 2: 249.5, 250.1, 245.6, 244.9 (-1.8%),
265.2).

**The player A/B.** `flint-player` release on `seeded_stars`,
`FLINT_VSYNC=0 --exit-after 22 --perf`: A is this branch (every draw call
declared), B is `origin/main` at `b9f540be` built from `git archive` in its
own target dir (draw calls as typed closures), C a second copy of B's binary,
the control. Five runs each, alternated A B C, two 10-second windows a run:

| | `script ui`, mean ms | min to max | frame p50, mean ms |
|---|---:|---:|---:|
| A branch | 0.160 | 0.14 to 0.20 | 0.82 |
| B origin/main | 0.160 | 0.14 to 0.19 | 0.84 |
| C origin/main again | 0.154 | 0.14 to 0.16 | 0.78 |

A and B are equal to the player's 0.01 ms resolution, and B differs from its
own copy by more than from A. The gate is +5% of the frame, about 40 us at
this frame time; the harness puts the port at -5 us. **The phone was not
measured**; Chris waived that half of the gate (ADR 0116, Decisions 4).

## Opts maps (#702)

#702 gave every draw primitive an opts form under its own name,
`draw_rect(x, y, w, h, #{color, rounding, layer})`, and deprecated the
`_ex` variants and the long positional lists it replaces. What a map costs
a call, against the positional forms that stay:

`cargo run --release -p flint-script --example api_overhead -- --opts`,
2026-11-11, same machine as above, the engine's own functions (a fresh
`ScriptEngine` a case), `screen_width()` before the loop. ns per call over
an empty loop's, best of 7 rounds of 400 frames x 1000 calls; two runs, an
idle machine:

| | run 1 | run 2 | over positional |
|---|---:|---:|---:|
| `draw_rect(1.0, 2.0, 3.0, 4.0, 0.5, 0.5, 0.5, 1.0)` | 316.3 | 315.1 | |
| `draw_rect_ex(.., 4.0, 2)`, 10 positional (deprecated) | 353.8 | 351.6 | +12% |
| `draw_rect(.., #{color: [0.5, 0.5, 0.5, 1.0]})` | 470.6 | 472.9 | +49% |
| `draw_rect(.., #{color: [..], rounding: 4.0, layer: 2})` | 533.5 | 534.4 | +69% |
| the same with `color: "#808080"` | 410.5 | 404.3 | +29% |
| the three-key map held in a variable | 528.8 | 532.9 | +68% |
| `draw_text(1.0, 2.0, "score", 16.0, 1.0, 1.0, 1.0, 1.0)` | 362.8 | 361.3 | |
| `draw_text_ex(.., 2)`, 9 positional (deprecated) | 375.9 | 381.0 | +4% |
| `draw_text(.., #{color: [1.0, 1.0, 1.0, 1.0], layer: 2})` | 563.5 | 564.3 | +56% |

A map costs a call 0.15 to 0.22 us: Rhai building it (a map, and for an
array colour an array of four), then the reader walking its keys. A hex
colour is the cheaper map colour, a string where an array is four values.
A map in a variable is no cheaper than a literal: Rhai copies an argument
that isn't the first into the call.

At 1000 `draw_rect` calls a frame that is 315 us positional and 534 us with
a three-key map, 0.22 ms more, 1.3% of a 60 Hz frame. The heaviest
script-drawn demo (`seeded_stars`, 146 draws) keeps its positional
`draw_rect`/`draw_circle` and makes one map call a frame (its title, now
`draw_text(.., opts)`), so it doesn't move. The migrated demos' maps are in
HUDs of a few dozen draws: under 10 us a frame. The book's convention text
says so: a loop of hundreds of draws that needs only a colour keeps the
positional form. One that needs a layer or a rounding now pays the map
(the `_ex` forms are deprecated); if that matters for a game, a positional
form with a layer under the base name (`draw_rect(x, y, w, h, r, g, b, a,
layer)`) is the cheap addition, left undone until something needs it.

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
- String arguments on their own. Map arguments are measured above (#702).
