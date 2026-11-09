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
| `--cut reel` | What a cut uses, at its size and point canvas, into `<out>/<cut>/` ([A cut](#a-cut)) |

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

## A cut

A cut is a film edited for one place, and is one file,
`tools/showcase/cuts/<name>.toml`: a `[cut]` table (frame size, frame rate,
the music's bar grid, where captions may go) and its `[[shot]]`s in order,
each a take of a beat from `start_s` seconds for a number of bars:

```toml
[cut]
name = "reel"
bpm = 100
beats_per_bar = 4
fps = 30
width = 1080
height = 1920
story_bars = 6                 # split into pieces of at most 6 bars too
points_wide = 720              # the point canvas: pixels_per_point = 1.5
safe = [120, 250, 960, 1480]   # captions and key content stay inside

[[shot]]
beat = "02_type"
take = "main"
start_s = 2.4
bars = 3.0
caption = "the world in type"
label = "ASCII  ·  palettes  ·  glyphs that hold still"
```

Shots are measured in bars, not seconds, so the music (#438) can be
written to the same list and change on every cut. Then:

```bash
python tools/showcase/capture.py --cut reel   # the takes the cut uses, at its size
python tools/showcase/edit.py reel            # the MP4s
```

`capture.py --cut` captures only the takes the cut's shots use, each only
as far as its last shot reaches, at the cut's size and on its point canvas,
into `~/flint-film/<cut>/`. A cut's `points_wide` is the frame's width in
points: the master lays out on 1920, which in a 1080-pixel-wide frame makes
an 8-point ASCII cell 4.5 pixels, too fine for a phone. The reel uses 720
(a cell is 12 pixels); `flint render --pixels-per-point 1 --sweep
ascii.cell=4.5,8,12,16 --sweep-crop ...` at the cut's size shows the
choice side by side.

`edit.py` reads the frames, lays each shot's caption over it inside
`safe` (a serif line and a monospaced label under it; consecutive shots
with the same caption share one), adds the `[cut.end]` card over the last
bars, and pipes every frame once to ffmpeg. It writes the whole cut and,
from the same frames in the same pass, each story-length piece:

```
~/flint-film/reel/edit/reel.mp4
~/flint-film/reel/edit/reel_story_1.mp4 ... reel_story_4.mp4
~/flint-film/reel/edit/edit.json
```

H.264 High at level 4.2, yuv420p tagged BT.709, a keyframe a second, and
AAC audio: `--audio track.wav` (or the cut's `audio` key), else a silent
track, so the file is what a phone and Instagram expect either way.
`--guides` shades everything outside the safe area for review, `--stills
6` also writes frames as PNG, and `--dry-run` prints the timeline. It needs
Pillow, numpy and ffmpeg on `PATH`; the captions use Windows' Georgia
Italic and Cascadia Mono, falling back as the cut's `caption_font` and
`label_font` lists say.

### Framing for a tall frame

A 9:16 window at the same field of view keeps less than a third of a 16:9
frame's width, so a cut that isn't 16:9 is captured, not cropped. The
film's camera moves all go through `film::orbit` and `film::dolly`, which
ask `film::portrait()` (`screen_height() > screen_width()`) and, in a tall
window, back the camera off along its line of sight, open the field of
view to 60° and aim a little low, so the still life sits in the middle of
the safe area with sky above it and grass below. The same scene and
script serve both cuts. A beat whose move doesn't survive that rule draws
a portrait path of its own with `film::orbit_portrait` /
`film::dolly_portrait` (beats 04, 06 and 07 film from the barrel's far
side, where the figure, the barrel and the crate stand closest together
across a narrow frame).

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
