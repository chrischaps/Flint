# Filming a Showcase

Flint can film itself. `flint-player` steps its clock a fixed amount per
frame and writes every frame, so a scripted scene plays the same way however
slowly a 4K frame draws, and two captures of it can be compared pixel for
pixel. The stylize showcase film (#434) is made this way; its rig in
`tools/showcase/` is a template for filming anything else.

## One command

```bash
cargo build --release --bin flint-player
python tools/showcase/capture.py
```

That captures every beat of the film (`tools/showcase/film.toml`) at
3840x2160 into `~/flint-film/<beat>/<take>/frame_0001.png ...`, each take
with a `take.json` saying how it was made and a `player.log`. It needs
Python 3.11 or later; `--verify` also needs Pillow and numpy.

| Flag | What it does |
|------|--------------|
| `02_type 08_pencil` | Only these beats (`--list` shows them all, with frame counts and disk) |
| `--take matrix` | Only these takes of a beat |
| `--scale 0.5` | A fraction of the master size (0.5 is 1920x1080) |
| `--width W --height H` | Any size; point-sized things keep their layout (below) |
| `--out DIR` | Somewhere other than `[film] out` |
| `--verify` | Capture each take twice and compare the runs |

A 4K frame of the film's scenes is about 14 MB and the whole film about
4,700 frames, so the rig estimates the disk a run needs and refuses one that
won't fit. Frames go outside the repo on purpose.

## What a take is

One run of the player on one scene:

```bash
flint-player demo/film/02_type.scene.toml --schemas schemas \
  --width 3840 --height 2160 --pixels-per-point 2 \
  --frame-step 0.0333333 --screenshot out/frame.png --screenshot-every 0.0333333 \
  --exit-after 13.98 --music-volume 0 --sfx-volume 0
```

- `--frame-step 1/fps` advances game time exactly one frame per frame, and
  `--screenshot-every` the same writes each one. Scripts, animation,
  particles, a style's `boil` and egui's own animations all run on that
  clock.
- `--pixels-per-point width/1920` lays every frame out on a 1920x1080-point
  canvas. ASCII cells, pencil spacing and egui are sized in points, which
  otherwise follow the window's scale factor, so the same take would differ
  between a 100% and a 125% display. A 4K take and a 1080p preview frame the
  same.
- A take's `args` in `film.toml` add player flags; the looks beat's five
  takes are one scene with `--look ascii_noir`, `--look matrix`, and so on.
- A beat's `input` is an input script (`--input-script`) for the beats that
  drive the F4 menu. Its pointer positions are written in points; the rig
  writes a copy scaled to the take's pixels beside the frames.

## A beat

A beat is a scene whose script drives the camera and the styles on game
time. The film's beats share one set, `demo/film/set.toml`; each adds a
fragment (`demo/film/beats/<beat>.toml`: its `[scene]`, `[camera]`, styles
and script entity) and `python tools/showcase/build_scenes.py` writes the
scene from the two (`--check` says whether any is stale; the capture runs
it first). The fragment may add components to the set's props
(`[entities.barrel.ascii_style]`) and its style tables
(`[post_process.styles.ascii]`), but not repeat a table the set has.

Scripts change whole-frame styles with `set_scene_style(style, on)` and
`set_scene_style_param(style, param, value)`, the scene-wide twins of
`set_entity_style` and `set_style_param`
([Scripting → Style API](../concepts/scripting.md#style-api)). Shared moves
(`orbit`, `dolly`, `ease`, `ramp`, `pulse`) are in
`demo/film/scripts/lib/film.rhai`:

```rust
import "film" as film;

fn on_update() {
    let t = total_time();
    film::orbit(0.0, 0.75, 0.0, 5.2, 1.6, film::lerp(0.85, 1.6, t / 14.0));
    set_scene_style_param("ascii", "cell", film::lerp(8.0, 5.0, film::ramp(t, 12.0, 1.8)));
}
```

A one-shot change (a style switched on at 5 s) goes through a step number
kept in a component on the script's entity, so it is applied on the frame
the step starts and not again; see `demo/film/scripts/02_type.rhai`.

Everything a beat does comes from game time. Never wall time, never an
entity id (ids differ between runs; seed from names), and one directional
light (the sky's sun disc follows the first one, and entities load in no
fixed order).

The storyboard (each beat, its feature, its camera, and how the cuts are
meant to go) is `docs/design/showcase-film.md` in the repository.

## Two runs, one film

`--verify` captures each take twice and compares them frame by frame
(`tools/showcase/compare.py`, also usable on its own):

```bash
python tools/showcase/capture.py --verify --scale 0.333
python tools/showcase/compare.py ~/flint-film/02_type/main ~/flint-film/02_type/main.again
```

The report (`verify.json` beside each take and one for the run) lists the
frames that differ, in how many pixels and by how many 8-bit levels.

### What two runs of the film gave

Every take of the film, captured twice at 1280x720 (RTX 5070 Laptop,
Vulkan, Windows 11, 2026-11-07):

| Takes | Frames identical | Where the rest differ |
|---|---|---|
| `10_looks/ascii_noir` | all 241 | nowhere |
| `02_type`, `03_edges` (whole-frame ASCII) | 88% | 1 level in at most 29 pixels a frame |
| the other beats and looks (01, 04–09, 11, 13; `ink_wash`, `graphite`, `watercolor`) | 51–83% | up to 29 levels in at most 7 pixels a frame, each at a grass blade's edge; the painted ones (09 and the `ink_wash` and `watercolor` looks) spread one over up to 36 pixels as their washes diffuse it |
| `10_looks/matrix`, `12_menu` (whole-frame ASCII under temporal history) | 89–90% | the same, plus now and then one glyph cell (5x5 pixels here) drawing a different glyph for a few frames, up to 238 levels |

The same beats with the meadow's grass taken out (`01_plain`, `02_type`,
`13_hands`, two runs each) are **byte-identical**, every frame. The grass is
the whole of #341: its compute pass appends visible blades with an
`atomicAdd`, so their order in the instance buffer changes from run to run,
and where two opaque blades meet a pixel at the same depth whichever draws
first wins it. (Its count read-back, the first suspect, only feeds the
stats overlay.) ASCII averages a cell's colour, which turns that into a
1-level difference, and a cell sitting on a ramp boundary under temporal
history can take the neighbouring glyph and hold it for a few frames: the
amplification #341 worried about, seen here in one cell at a time.

For a film this is invisible: a handful of pixels in 921,600, at blade
edges in moving grass. The rule the storyboard follows is to capture each
beat once and cut from that take. A scene that must be byte-identical
(a golden, a flip count) leaves out `grass_ground` until #341 orders the
blades deterministically.
