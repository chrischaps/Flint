# tools/

Helpers that sit beside the engine: picture and fixture generators, the
showcase film's rig, perf drivers, the play site's builder and the
architecture analyzer. None is part of a game build. Each file's docstring is
its manual; this page says what each is for, what it runs, and what it needs.

## Setup

```bash
python -m pip install -r tools/requirements.txt   # numpy, Pillow, scipy, matplotlib; Python 3.11+
```

That covers every Python tool here and `scripts/goldens-summary.py`. The
**Needs** column names what else a tool runs: a `flint` or `flint-player`
build (`cargo build --release`, or the tool's own flag for the path),
[ffmpeg](https://ffmpeg.org/) (with ffprobe) on PATH, Node 22+ (built-in
`fetch` and `WebSocket`; no npm packages), adb, or Chrome.

Run Python tools with `python -I` when they read files you didn't make, as
CLAUDE.md asks.

## Index

| Tool | What it's for | Wraps | Needs |
|---|---|---|---|
| **Architecture** | | | |
| `arch-analyzer/` | Rust (workspace member, not default). Reads every Cargo.toml and crate source: writes `arch-viewer/arch-data.json`, and with `--book` the crate table in `docs/book/src/architecture/crate-graph.md`, which CI's *crate table* job checks with `--check-book` | `cargo run -p flint-arch-analyzer -- [--book \| --check-book \| out.json]` | cargo |
| `arch-viewer/` | A browser graph of `arch-data.json`: crates by tier, their modules and public items. Run the analyzer, then serve this folder (`python -m http.server -d tools/arch-viewer`) | | a browser |
| **Pictures for the book and the demos** | | | |
| `living-strip/living_strip.py` | The book's strip and slit-scan pictures of a wooze look (#219) | `flint-player --frame-step` | numpy, Pillow, flint-player |
| `darkroom_images.py` | The darkroom style's LUT strips and papers in `demo/styles/` (#413); deterministic | | numpy, Pillow |
| `style-images/make_images.py` | The demo paper and stroke images (#390); deterministic | | numpy, Pillow |
| `ascii_flips.py` | Counts ASCII glyph flips and A-B-A reversals (shimmer) across captured frames (#277) | reads `flint-player` captures | numpy, Pillow |
| **Test fixtures** | | | |
| `make_sphere_glb.py` | `demo/models/Sphere.glb`, a UV sphere for the ASCII edge tests (#267) | | standard library |
| `test-models/double_sided_card.py` | `testModels/DoubleSidedCard.glb`, the glTF `doubleSided` fixture (#623) | | standard library |
| `test-models/double_sided_palm.py` | `demo/models/palm_{two,one}_sided.glb` for `demo/double_sided.scene.toml` (#623) | | standard library |
| **The showcase film** (`showcase/CLAUDE.md`) | | | |
| `showcase/build_scenes.py` | Writes `demo/film/*.scene.toml` from the set and each beat; `--check` for staleness | | standard library |
| `showcase/capture.py` | Captures each beat as a PNG sequence, unattended | `flint-player` | flint-player |
| `showcase/compare.py` | Two captures of one take, frame by frame: is it deterministic? | | numpy, Pillow |
| `showcase/music.py` (`score.py`, `synth.py`) | The film's score: synthesizes stems, then mixes them | `flint render-suite` | numpy, scipy (matplotlib for the plot), flint |
| `showcase/cut.py` | Cuts the takes into the master, web and reel files | | numpy, Pillow, ffmpeg |
| `showcase/check_cut.py` | Checks the encoded files frame by frame, and their sound | | numpy, ffmpeg |
| **The web player** | | | |
| `play-site/build.py` | Assembles play.chaps.dev from `play-site/games.toml` (#103) | `flint export web` | flint, a trunk build of the player |
| `web-touch/ipad.mjs` | Plays a game on an emulated iPad over CDP and reports FROZEN or LIVE (docs/design/web.md) | | Node 22+, Chrome |
| `phone-perf/` | The web player's frame times on the Pixel 8 Pro (#378); see its README | `flint bundle`, trunk | Node 22+, adb, Chrome on the phone |

## Fixtures and `flint gen`

The three fixture scripts write glTF by hand, with the standard library only,
because what they test is a glTF detail: `doubleSided` on a material, which
`flint gen`'s GLB exporter doesn't write, and a sphere with every normal
direction in it. Keep them as scripts, so a fixture can be remade and its
construction read.
