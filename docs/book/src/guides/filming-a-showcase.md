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

A 4K frame of the film's scenes averages about 7 MB and the whole film is
10,940 frames at 60 fps (77 GB), so the rig estimates the disk a run needs and refuses one that
won't fit. Frames go outside the repo on purpose.

## What a take is

One run of the player on one scene:

```bash
flint-player demo/film/02_type.scene.toml --schemas schemas \
  --width 3840 --height 2160 --pixels-per-point 2 \
  --frame-step 0.0166667 --screenshot out/frame.png --screenshot-every 0.0166667 \
  --exit-after 13.9917 --music-volume 0 --sfx-volume 0
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

## Cutting the film

The takes are cut into films by `tools/showcase/edit.py`, from a cut file
that names the shots in order. The 16:9 cut, for YouTube and the Flint page
on chaps.dev, is `tools/showcase/cut_16x9.toml`:

```bash
python tools/showcase/capture.py            # the takes, 3840x2160 at 60 fps
python tools/showcase/edit.py               # master MP4, web MP4 + WebM, poster
python tools/showcase/check_cut.py          # every encoded frame against the edit
```

Everything lands in `~/flint-film/cuts/<name>/`. The edit needs Python 3.11+,
Pillow, numpy and ffmpeg on the PATH (`winget install Gyan.FFmpeg`; 8.0 made
the first cut).

A shot is a range of one take:

```toml
[[shot]]
beat = "10_looks"
take = "matrix"
in = 1.0
out = 2.0
tag = "--look matrix"
```

`in` and `out` are seconds of the take, and a shot is exactly
`(out - in) * fps` of its frames, each used once and in order. The takes
must be captured at the cut's frame rate; the edit never resamples time.
A shot can carry a lower third (`number`, `title`, `caption`, with
`lower_third_at` / `lower_third_hold` to move it), a centred `card` (the
film's title, the closing line) and a `tag` that cuts with its shot. They
are drawn in the edit, on a 1920x1080-point canvas scaled to the frame, so
the takes stay clean and the vertical cut (#437) can lay out its own.

| Flag | What it does |
|------|--------------|
| `--list` | Each shot's film time, take and frame range, and the film's length |
| `--still 2.0,40.5` | Those film times as composed PNGs, to check titles without encoding |
| `--master-only` / `--web-only` | One half of the encode (the web files come from the master) |
| `--web-files webm` | Only those web files (`mp4`, `webm`, or both) |
| `--cut FILE` | Another cut file |

Each film frame is composed in a worker pool (the source PNG, the titles at
their fade, the fade from and to black) and piped to ffmpeg as raw RGB. The
master is H.264 High, `yuv420p`, BT.709 in its matrix and its tags, half-second
closed GOPs, `+faststart`: what YouTube asks for. The web files are scaled
from it: an H.264 MP4 and a two-pass VP9 WebM, and a poster JPEG from a
clean frame of a take, titles left off.

### Checking every frame

The edit saves a 160x90 luma thumbnail of every frame it composed.
`check_cut.py` reads each encoded file's packet times (every frame there,
once, exactly a frame apart), then decodes every frame, takes out its mean
level (the YUV round trip shifts luma by a level or two) and holds it
against those thumbnails:

- **in place**: each 2 s window matches best unshifted, not one to three
  frames early or late. A dropped or repeated frame shifts what follows.
- **not repeated**: where the composed film moves from one frame to the
  next, the decoded film moves too.
- **the right picture**: no frame is more than three times as far from its
  own thumbnail as the frames around it, plus a level. The codec's error
  depends on the picture (ASCII's glyphs cost the most), so each frame is
  judged against its own second of film.
- **not torn**: a frame's four horizontal bands match their own thumbnail
  about equally well.

Steps where the composed film barely moves (the black of the fades) can't
show a repeat and are counted as still. It writes `<name>.check.json`
beside the films and exits 1 if anything fails.

To be sure it can fail, it was run on a broken copy of the first master: the
same 7,560 frames with frame 3000 shown twice, frame 5000 dropped and frame
6000's bottom half taken from frame 6030. It failed it, and flagged nothing
outside the faults: the windows from frame 3000 to 5039 matched best a frame
late, and frame 6000 was flagged as wrong and torn.

| File | Frames | Frame steps | Error (mean / max, 8-bit luma) | Shifted, repeated, wrong, torn |
|---|---|---|---|---|
| master, 3840x2160 60 fps | 7,560 of 7,560 | 16.666..16.667 ms | 0.45 / 0.77 | none |
| web MP4, 1920x1080 30 fps | 3,780 of 3,780 | 33.333..33.334 ms | 0.82 / 2.38 | none |
| web WebM, 1920x1080 30 fps | 3,780 of 3,780 | 33..34 ms (WebM keeps ms) | 0.75 / 4.12 | none |

Played in Chrome 157 (headed, over CDP, `requestVideoFrameCallback` and
`getVideoPlaybackQuality`), the master ran in real time and Chrome decoded
all 7,560 frames; on a machine busy with other builds it presented 7,457 of
them (11 counted dropped, 92 steps of two frames), so a 4K60 film on a loaded
desktop is at the player's mercy, not the file's. Both web files presented
every frame, 3,780 of 3,780, each one step after the last, none dropped.
