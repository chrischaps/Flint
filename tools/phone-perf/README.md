# Phone perf: the web player on the Pixel 8 Pro over adb (#378)

How `docs/design/web.md`'s phone tables for the stylize passes were taken.
First take the shared phone's lock (`mkdir C:/Users/chris/dev/.pixel-lock`),
and check that nobody is using the phone: the driver brings Chrome to the
front.

```bash
# 1. a game folder with the scene and its variants: none, per entity, full screen, each + edges = 1
python -I tools/phone-perf/ascii_variants.py flint-examples/ascii_entities.scene.toml <work>/asciiweb .
flint bundle <work>/asciiweb --out-dir <work>          # asciiweb.flintpack

# 2. the site, with the bundle beside index.html, served on 8378
(cd crates/flint-player && trunk build --release --cargo-profile web-release --dist <work>/site)
cp <work>/asciiweb.flintpack <work>/site/
(cd <work>/site && python -m http.server 8378 --bind 127.0.0.1)

# 3. play each on the phone (adb reverse 8378, Chrome's devtools on 9378), read ?perf=1
EXTRA='&fps=native' node tools/phone-perf/phoneperf.mjs <work>/out \
  ascii_none,ascii_entities,ascii_full,ascii_entities_edges,ascii_full_edges 30 3
EXTRA='&fps=native' node tools/phone-perf/phoneperf.mjs <work>/bisect \
  "ascii_none,ascii_none+style=invert,ascii_none+style=ascii" 25 2

# 4. one row per 10 s window
python -I tools/phone-perf/summarize.py <work>/out/log.txt <work>/out/rows.json
```

## A pass's cost: `submits.mjs` (#568)

Frame times snap to whole refreshes, so a pass that costs 2 ms reads as
nothing, or as half the frame rate, depending on where the frame sat. To
cost a pass, make every page GPU-bound and count frames instead:

```bash
# scale=1 puts the scene past 60 fps's budget, so every page is GPU-bound and
# 10 000 / frames is the GPU's ms a frame; items split on ';', a style word keeps its commas
BASE='&scale=1' node tools/phone-perf/submits.mjs <work>/s \
  "ascii_none;ascii_none+style=ascii;ascii_none+style=ascii:edges=0" 12 3
# `dir/scene` plays the site copy in dir/ (a patched or rebuilt wasm, the root's pack)
BASE='&scale=1' node tools/phone-perf/submits.mjs <work>/ab \
  "ascii_none+style=ascii;fix/ascii_none+style=ascii" 12 4
```

It wraps `GPUQueue.submit` in the page for 10 s (the player submits once a
frame, #139) and counts the render passes between submissions. Each round
plays every item once, so heat is a trend across rounds, not a step between
items. A copy of the site in a folder needs `./` before its script paths in
`index.html`. To find which pass costs, stub one shader in a copy: the wasm
holds the WGSL as written (`include_str!`), so a same-length text swap that
returns early behind a runtime test (`if in.position.x > -1.0 { return ..; }`)
removes its work and keeps its pipeline.

A nearly free scene (`BASE='&scale=0.25&quality=ssao=0,bloom=0,shadows=0'`)
shows the opposite: there everything holds the refresh, so the display
stage's passes fit in what the scene leaves.

The readout now does this count itself (#1318): `?perf=1&fps=native` shows
a `mean N ms a frame` row, marked `GPU-bound` with the inputs it judged by
when the page is unpaced, the player's work is under half the frame and the
frames miss the refresh. `phoneperf.mjs`'s `?perf=1` rows carry it, and
`perfline.mjs` parses it as `throughput`. How the guess is made and where it
goes wrong: `../web-perf/README.md`, "GPU-bound pages". `submits.mjs` stays
the independent count, from outside the player.

## Notes

- ASCII's `edges` defaults to 1, so `ascii_variants.py`'s `_edges` scenes
  draw the same passes as the ones without the suffix. The edges' cost
  shows only against `?style=ascii:edges=0` (#568).
- `fps=native` unpaces the frames, so frame time shows each scene's cost. The
  phone's default, `auto`, holds a rate and hides it.
- Mali reads 0.00 ms for every GPU pass after `Scene`, so compare frame times
  against a control scene, not the GPU list.
- On the charger the phone warms to thermal status 1--2 in a few minutes and
  drops from 120 to 60 Hz. Run a control both first and last, and record the
  thermal state the driver logs with each load.
- Afterwards: `adb forward --remove tcp:9378`, `adb reverse --remove tcp:8378`,
  and remove the lock.
