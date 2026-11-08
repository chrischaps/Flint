# Phone perf: the web player on the Pixel 8 Pro over adb (#378)

How `docs/design/web.md`'s phone tables for the stylize passes were taken.
First take the shared phone's lock (`mkdir C:/Users/chris/dev/.pixel-lock`),
and check that nobody is using the phone: the driver brings Chrome to the
front.

```bash
# 1. a game folder with the scene and its variants: none, per entity, full screen, each + edges = 1
python -I tools/phone-perf/ascii_variants.py demo/ascii_entities.scene.toml <work>/asciiweb .
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

- `fps=native` unpaces the frames, so frame time shows each scene's cost. The
  phone's default, `auto`, holds a rate and hides it.
- Mali reads 0.00 ms for every GPU pass after `Scene`, so compare frame times
  against a control scene, not the GPU list.
- On the charger the phone warms to thermal status 1--2 in a few minutes and
  drops from 120 to 60 Hz. Run a control both first and last, and record the
  thermal state the driver logs with each load.
- Afterwards: `adb forward --remove tcp:9378`, `adb reverse --remove tcp:8378`,
  and remove the lock.
