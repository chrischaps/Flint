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
- A take's `args` in `film.toml` add player flags; the looks beat's eight
  takes are one scene with `--look ascii_noir`, `--look matrix`, and so on.
- A beat's `input` is an input script (`--input-script`) for the beats that
  drive the F4 menu. Its pointer positions are written in points; the rig
  writes a copy scaled to the take's pixels beside the frames. They are the
  menu's rows as captured, so a change to the menu's layout (a new style
  parameter, a new section) moves them: capture the beat after one and look
  at its menu frames before trusting a take.

## A beat

A beat is a scene whose script drives the camera and the styles on game
time. The film's beats share one set, `demo/film/set.toml`; each adds a
fragment (`demo/film/beats/<beat>.toml`: its `[scene]`, `[camera]`, styles
and script entity) and `python tools/showcase/build_scenes.py` writes the
scene from the two (`--check` says whether any is stale; the capture runs
it first). The fragment may add components to the set's props
(`[entities.barrel.ascii_style]`) and its style tables
(`[post_process.styles.ascii]`), but not repeat a table the set has.

Scripts change whole-frame styles with `set_post_style(style, on)` and
`set_post_style_param(style, param, value)`, the scene-wide twins of
`set_entity_style` and `set_style_param`
([Scripting → Style API](../concepts/scripting.md#style-api)). Shared moves
(`orbit`, `dolly`, `ease`, `ramp`, `pulse`) are in
`demo/film/scripts/lib/film.rhai`:

```rust
import "film" as film;

fn on_update() {
    let t = total_time();
    film::orbit(0.0, 0.75, 0.0, 5.2, 1.6, film::lerp(0.85, 1.6, t / 14.0));
    set_post_style_param("ascii", "cell", film::lerp(8.0, 5.0, film::ramp(t, 12.0, 1.8)));
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

Every take of the film (all 13 beats, the looks beat's eight takes), captured
twice at 1280x720 (RTX 4070, Vulkan, Windows 11, 2026-11-08), is
**byte-identical**, every frame, the meadow's grass and the ASCII beats'
temporal history included.

That took #341. Before it, the grass compute pass appended visible blades
with an `atomicAdd`, so their order in the instance buffer changed from run
to run, and where two opaque blades met a pixel at the same depth whichever
drew first won it: a few pixels a frame at blade edges, which ASCII under
temporal history now and then turned into one cell holding a different
glyph for a few frames. Blades are placed in grid order now, so a beat can
be reshot and cut against an earlier take frame for frame.
