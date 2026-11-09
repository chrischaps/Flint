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

## A cut

A cut is a film edited for one place, and is one file,
`tools/showcase/cuts/<name>.toml`: a `[cut]` table (frame size, frame rate,
the music's bar grid, where captions may go) and its `[[shot]]`s in order,
each a take of a beat from `start_s` seconds for a number of bars. Both
films are made this way, by one editor, `cut.py`: the 16:9 master
(`cuts/master.toml`, [Cutting the film](#cutting-the-film)) and the
vertical reel (`cuts/reel.toml`). The music (#438) is scored to the same
file, so a cut and its track change on the same frames.

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
python tools/showcase/music.py reel           # its score, to the same list
python tools/showcase/cut.py reel             # the MP4s, with the score
python tools/showcase/check_cut.py reel       # every frame, the sound, every shot on its bar
```

`capture.py --cut` captures only the takes the cut's shots use, each only
as far as its last shot reaches (and the last through the cut's
`tail_bars`), at the cut's size, frame rate (the reel is 30 fps, the master 60) and point canvas,
into `~/flint-film/<cut>/`. With `--scale` the point canvas stays the
cut's, so a smaller capture keeps its layout and the input scripts' clicks
land on the same rows. A cut's `points_wide` is the frame's width in
points: the master lays out on 1920, which in a 1080-pixel-wide frame makes
an 8-point ASCII cell 4.5 pixels, too fine for a phone. The reel uses 720
(a cell is 12 pixels); `flint render --pixels-per-point 1 --sweep
ascii.cell=4.5,8,12,16 --sweep-crop ...` at the cut's size shows the
choice side by side.

`cut.py` reads the frames, lays the cut's titles over them, adds the
`[cut.end]` card over the last bars, and pipes every frame once to ffmpeg.
The reel's titles are `titles = "caption"`: a serif line at the top of
`safe` and a monospaced label under it, shared by consecutive shots with
the same caption. It writes the whole cut and, from the same frames in the
same pass, each story-length piece:

```
~/flint-film/reel/edit/reel.mp4
~/flint-film/reel/edit/reel_story_1.mp4 ... reel_story_4.mp4
~/flint-film/reel/edit/edit.json
```

H.264 High at level 4.2, yuv420p tagged BT.709, a keyframe a second
(the cut's `[encode]` table; the reel holds Instagram's 24 Mbit/s), and AAC
audio from the cut's `audio` key or `--audio`. Before it composes a frame
it reads the music's report beside the WAV (`many-hands-<cut>.json`) and
refuses a track scored to another list: another cut, another length, a
shot starting on a frame other than the music's, or a WAV the report
doesn't describe. `--silent` cuts without one, for a picture check.
`--guides` shades everything outside the safe area for review, `--stills
6` also writes frames as PNG, `--still 2.0,40.5` writes those film times and
stops, and `--dry-run` prints the timeline and whether the music fits it.
It needs Pillow, numpy and ffmpeg on `PATH`; the captions use Windows'
Georgia Italic and Cascadia Mono, falling back as the cut's `caption_font`
and `label_font` lists say.

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

## Cutting the film

The 16:9 film, for YouTube and the Flint page on chaps.dev, is the cut
`master` (`tools/showcase/cuts/master.toml`), made like the reel:

```bash
python tools/showcase/capture.py --cut master   # the takes it uses, 3840x2160 at 60 fps
python tools/showcase/music.py master           # its score
python tools/showcase/cut.py master             # master MP4, web MP4 + WebM, poster
python tools/showcase/check_cut.py master       # every frame, the sound, every shot on its bar
```

Everything lands in `~/flint-film/master/edit/`. Takes already captured
for the whole film (`capture.py` with no `--cut`, `~/flint-film/<beat>/`)
are at the master's size and rate, so `cut.py master --frames ~/flint-film`
cuts from them. `--scale` (or `--width`) cuts takes captured smaller, for a
preview on a disk without room for 4K.

Its shots are on the same bars as the reel's, with a beat's line under it:

```toml
[cut]
name = "master"
fps = 60
width = 3840
height = 2160
points_wide = 1920       # pixels_per_point = 2
tail_bars = 2            # the last take runs on 2 bars under the end card
titles = "lower_third"

[[shot]]
beat = "10_looks"
take = "matrix"
start_s = 0.85
bars = 0.5
tag = "--look matrix"
```

`titles = "lower_third"` gives each beat a lower third: its number of 13,
its title (film.toml's, or the shot's `title`) and the shot's `caption`,
shared by a beat's shots in a row and moved with `lower_third_at` /
`lower_third_hold` (seconds from the beat's first shot; `[lower_third]`
has the defaults). A shot can also carry a centred `card` (the film's
title) and a `tag` that cuts with it (each look's name). They are drawn in
the edit on the cut's point canvas, so the takes stay clean for other cuts.
The last shot's take runs on through `tail_bars`, where the music rings
out: 13's camera comes to rest at 12 s and the evening keeps moving under
the card, rather than holding a still frame.

Each film frame is composed in a worker pool (the source PNG, the titles at
their fade, the fade from and to black) and piped to ffmpeg as raw RGB with
the WAV beside it. `[encode]` makes the master H.264 High, `yuv420p`,
BT.709 in its matrix and its tags, half-second closed GOPs, `+faststart`,
AAC at 320 kbit/s: what YouTube asks for. `[web]` makes the page's files
from the master's picture and the WAV itself (so the sound isn't encoded
twice): an H.264/AAC MP4 and a two-pass VP9/Opus WebM at 1080p and 30 fps,
and a poster JPEG from a clean frame of a take, titles left off.
`--web-only` remakes them from a master already cut, `--web-files webm`
only those, `--no-web` skips them.

The first master (#436) was cut by an editor of its own from a list timed
in seconds, 126 s long and silent, while the music was scored to this list,
120 s long: the two drew one film to two clocks. Since #593 there is one
list and one editor, and the editor won't cut to a track scored to anything
else.

### Checking every frame

`cut.py` saves a small luma thumbnail of every frame it composed, and
`edit.json` lists each file it encoded and which of those frames it holds.
`check_cut.py <cut>` reads each encoded file's packet times (every frame there,
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
show a repeat and are counted as still.

Then the sound (#593). Each file's audio is decoded as a player would play
it (AAC's edit list and Opus's pre-skip applied) and measured with the
meters the music was mastered with (`synth.py`, BS.1770-4): it must be
there (unless the cut was made `--silent`), 48 kHz stereo, as long as the
picture to within one of the file's frames, within 0.5 LU of -14 LUFS for
a file of the whole cut (a story piece is a slice, so its loudness isn't),
and at most -1.0 dBTP true peak with no sample at full scale. And the bars:
the music's report must still describe its WAV, be this cut's, and start
every shot on the frame the edit started it. It writes `<cut>.check.json`
beside the films and exits 1 if anything fails.

To be sure it can fail, it was run on a broken copy of #436's first master: the
same 7,560 frames with frame 3000 shown twice, frame 5000 dropped and frame
6000's bottom half taken from frame 6030. It failed it, and flagged nothing
outside the faults: the windows from frame 3000 to 5039 matched best a frame
late, and frame 6000 was flagged as wrong and torn. That first master, the
silent 126 s one:

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

## Music on the same clock

The film's music (#438) is written and mixed with Flint too, and its clock
is the film's: 100 bpm in 4/4, so a bar is 2.4 s, 72 frames at 30 fps. Each
cut is a list of shots in `tools/showcase/cuts/<cut>.toml` (`master` for the
16:9 film, `reel` for the vertical one): the beat and take, where in the
take it starts, and how many bars it lasts. Cuts sit on bar or half-bar
lines, so the music can change where the picture does.

```bash
cargo build --release --bin flint
python tools/showcase/music.py                # every cut, into ~/flint-film/music/
python tools/showcase/music.py reel --plot    # one, with a sheet of its shots and levels
python tools/showcase/music.py --verify       # build twice, compare every byte
```

For each cut it arranges the score (`tools/showcase/score.py`) to the shot
list, synthesizes six stems (`tools/showcase/synth.py`: sines, noise and FM,
no samples), writes a suite and its mix automation
(`tools/showcase/music/<cut>.suite.toml`, `<cut>.events.toml`), renders
them with `flint render-suite`, and masters the result to -14 LUFS under a
-1.5 dBTP ceiling: `many-hands-<cut>.wav`, 48 kHz 24-bit, exactly as long as
the cut. The JSON beside it has its length in samples and frames, its
loudness and peaks, and each shot's start frame. It needs numpy and scipy
(matplotlib for `--plot`).

The piece plays one melody throughout, in a different instrument for each
style: electric piano for the plain render, pulse waves for the ASCII beats
(their width changing with 02's tables), plucked strings for edges and
pencil, a flute for watercolor, FM glass for the user styles, a new voice
for every look. Cues inside a shot (03's edges, the 5.0 s switches in 08
and 09, the clicks in 05 and 12) are placed by the shot's `start_s`, so
they move with an in-point. Change a cut list and rebuild: the track
follows. Every sound is synthesized, so Content ID and Instagram's audio
match have nothing to find.

The picture is cut from the same list ([A cut](#a-cut), [Cutting the
film](#cutting-the-film)), so the track and the picture change on the same
frames. Nothing has to be kept in step by hand: change a shot and `cut.py`
refuses the old track until `music.py` has rebuilt it, and `check_cut.py`
holds every encoded file's shot starts against the music's report again
after the encode. The JSON gives each shot's start frame at the cut's own
rate (`frame`; 60 fps for the master) as well as at 30 fps.
