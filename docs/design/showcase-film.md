# Showcase film: storyboard (#435)

*Many Hands*, the film for epic #434: every stylize feature from the #260
batch, in motion, captured from the engine itself. This page is the
storyboard: what each beat shows, which scene shows it and how it is cut.
The capture rig and how to run it are in the book, [Filming a
Showcase](../book/src/guides/filming-a-showcase.md).

## The idea

One place, drawn by many hands. Every beat is the same clearing at the end
of the day, a still life someone left on a table (a lantern, a mug, a
barrel, a crate with a lid, a pale stone ball, a standing stone) and a
figure waiting beside it. The film opens on it as Flint renders it, then
redraws it in type, in pencil, in paint, as a projection, and ends with
every prop in a different hand at once while the camera pulls back into the
evening. Seeing one familiar frame change is what makes each style read:
the viewer compares, which a reel of unrelated demo scenes never lets them
do.

The set is `examples/film/set.toml`, written once; each beat is a fragment in
`examples/film/beats/` that adds its styles and its script, and
`tools/showcase/build_scenes.py` joins them into `examples/film/<beat>.scene.toml`.
All content is Flint's own (the meadow mask, the demo props, the built-in
glyphs and palettes, the styles in `examples/styles/`).

## One scene per beat, cut together

Chris was fine with separate captures edited together, and every beat is
its own scene and its own take, for three reasons:

- **A beat can be reshot alone.** Retiming the pencil beat doesn't touch
  the ASCII ones, and the edit (#436, #437) picks takes, not time ranges.
- **Each beat starts clean.** ASCII's temporal history (#277), egui's open
  sections and the style ids all start from the scene's own state, so a
  beat looks the same whether it is captured first or tenth.
- **Looks need takes.** `--look` lays a look over a scene's whole
  post-processing, so the looks beat is one camera move captured once per
  look and cut between on the same frame of the move. (Since #356 a script
  can also switch looks mid-shot with `set_look`, but a match cut wants the
  same frame under two looks, which only two takes give.)

Within a beat, the change is scripted on a timeline (`examples/film/scripts/`,
game time only), so a beat is one continuous shot with its cuts inside.
Two engine additions made that possible: `set_post_style` /
`set_post_style_param` (scripts edit the whole-frame style tables, as
`set_style_param` does an entity's) and the player's `--look`.

## Beats

| # | Beat | Seconds | Feature (issue) | What happens | Camera | Scene |
|---|---|---|---|---|---|---|
| 01 | The clearing | 6 | the unstyled render (PBR, shadows, SSAO, bloom, grass) | nothing styled: the frame every later beat redraws | slow push from wide to the table | new |
| 02 | The world in type | 14 | full-screen ASCII (#265), colour modes and palettes (#276), temporal stability (#277) | every 2.4 s the scene's ASCII table changes: source, phosphor tint, CGA on black, amber blocks, mono, then source with the cells settling 8 → 5 pt | one slow orbit | new |
| 03 | Edges | 8 | edge glyphs (#267) and a mask outline (#263) | ASCII from tone alone, then the edges fade up over 1.5 s and every silhouette and crease is traced in `─ │ ╱ ╲`; the ball, as bright as the sky, keeps its outline from its style mask | low push past the ball, barrel and stone | new |
| 04 | Some things in type | 9 | per-entity ASCII (#266), `grid = "object"` | the barrel (green, its own grid), the figure (amber blocks) and the crate (CGA, the lid opted out) in glyphs, the world smooth; the barrel turns so its glyphs ride with it | truck across the still life | new |
| 05 | The mask | 9 | the entity style mask (#263), the F4 menu (#268) | props under ASCII, pencil and watercolor; the input script opens F4 and sets Debug mode to Style Mask (a hue per style id, white seams), then back to PBR | slow arc | new, input script |
| 06 | A story in styles | 9 | Rhai API and events (#269) | nothing styled in the file: the barrel comes apart into letters (24 → 7 pt), the figure follows in breathing amber, the crate is drawn in pencil, then all are given back | slow push | new |
| 07 | Fade and dissolve | 10 | blends and transitions (#278) | the barrel dissolves into phosphor in spreading patches, the figure cross-fades into blocks, both go back | slow orbit | new |
| 08 | Pencil | 10 | pencil sketch (#273): hatching, contours, paper, boil | each prop its own pencil (sepia, screen hatching, blue lead, object-anchored) boiling at 6 Hz; at 5 s the scene's pencil goes to the whole frame and the clearing becomes the page | orbit | new |
| 09 | Watercolor | 9 | watercolor (#272) | the whole clearing painted except the figure (`enabled = false`), standing real in a painting; at 5 s the paint lifts off ground and sky and stays on the props | descending push | new |
| 10 | Looks | 8 × 8 takes | looks (#279, #391) | one orbit under `ascii_noir`, `matrix`, `ink_wash`, `graphite`, `watercolor`, and the pencil family `coloured_pencil`, `blueprint`, `charcoal`: cut between takes on the same frame of the move | identical orbit in every take | new, five takes |
| 11 | Styles you write | 10 | user-authored WGSL styles (#274, #275) | the figure and the ball are holograms; the script prints the frame as a risograph, then lays survey contours over it | orbit | new |
| 12 | Tuning by hand | 12 | the F4 Styles editor (#268) | the input script opens F4 → Styles, drags the ASCII cell size coarse then fine, picks palette, amber, then CGA | near-still, menu on the right | new, input script |
| 13 | Many hands | 12 | per-entity ASCII, pencil, watercolor and a user style on one screen | every prop a different hand at once; the camera rises and pulls back until the sky takes the frame, room left for the title | long rise and pull back | new |

About 3 min 2 s of footage before the edit (182 s, the eight looks takes counted); a cut uses a few seconds of
most beats.

Every beat is a new scene on the shared set rather than an existing demo:
the demos (`ascii_motion`, `ascii_entities`, `ascii_edges`, `style_mask`,
`ascii_story`, `ascii_reveal`, `pencil_sketch`, `watercolor`,
`user_styles`) are benches, each framed for its own feature and its own
golden, and played one after another they look like a different place every
few seconds. They stay as they are; the beats borrow their ideas (the
story's beats, the reveal's ramps, the mask's colours).

### Cutting notes for #436 and #437

- **Order** is the table's: plain, then the type family (02–07), then the
  other hands (08–11), the tool (12), and the gathering (13). Beat 01 is the
  reference every later beat should feel like a redrawing of; open on it.
- **Looks** (10) cut on the move, a second or so per look, never back to
  the same one.
- **UI beats** (05, 12) are the only ones with egui on screen. In the
  vertical cut the menu falls outside a centre crop: crop right for the
  menu half of those beats, or keep them to the 16:9 master.
- **The vertical cut** is captured at 1080x1920, not cropped from the
  master: see [The vertical cut](#the-vertical-cut-437) below.
- **Music** (#438) is on the beats' own clock, 100 bpm in 4/4 (02 changes
  every 2.4 s, a bar), and it is arranged to the cut lists in
  `tools/showcase/cuts/`: cut on bar or half-bar lines and the music
  changes with the picture. See [Music](#music) below.

### The 16:9 cut (#436, #593)

One landscape film for the chaps.dev YouTube channel and the Flint page:
every beat in the table's order, 2 min 0 s (50 bars) at 3840x2160 and 60
fps, under its score. It opens on beat 01 out of black under the film's
title card and closes on 13's rise into the sky; the music's last two bars
are a tail in which 13's take runs on, the camera come to rest in the
evening, under the end card (the engine, the repo, the book). Each beat
gets a lower third (its number of 13, its name, one line on what it
shows); the looks beat is eight half-bar shots, one per take, cut on the
same orbit, under one lower third and a tag naming each look. Cuts are
hard and fall on the music's bar and half-bar lines: every beat is the same
clearing, so a cut reads as the place being redrawn, and the tune changes
instrument where the picture changes hand.

**Tools.** ffmpeg does the encode (8.0, the gyan.dev full build:
`winget install Gyan.FFmpeg`; it is not in the repo, and a rebuild needs it
on the PATH). The edit is `tools/showcase/cut.py` (Python, Pillow, numpy),
the reel's editor too: it composes each film frame from its source PNG,
draws the titles over it and pipes raw RGB to ffmpeg with the WAV, so no
editor project file has to be kept in step with the takes, and the film
rebuilds after a beat is recaptured:

```bash
python tools/showcase/capture.py --cut master 08_pencil   # reshoot a beat after a style ticket lands
python tools/showcase/music.py master                     # the score, if a shot moved
python tools/showcase/cut.py master                       # master, web, poster, with the score
python tools/showcase/check_cut.py master                 # every frame, the sound, the bars
```

The cut is a file, `tools/showcase/cuts/master.toml`, and the music is
scored to the same file: shots (a take, where in it to start, how many
bars), the titles, `[encode]` and `[web]`. Titles are drawn in the edit
rather than by the beats' `hud.rhai`, so the takes stay clean for the
vertical cut (#437), which is its own cut file over the same beats.

**Two lists, one film (#593).** #436 first cut this film from a list of its
own, `cut_16x9.toml`, timed in seconds (126 s), by an editor of its own,
`edit.py`, with no sound; #438 had scored `cuts/master.toml` (120 s). Every
ticket passed its own checks, and by beat 13 the picture came 10.8 s after its
music. #593 moved the titles onto `cuts/master.toml`, folded `edit.py` into
`cut.py` and deleted both. `cut.py` now refuses a track whose report says
it was scored to another list, and `check_cut.py` holds every shot's first
frame against the music's after the encode. The new list differs from the
draft #438 left in two places: 01 runs its whole 6 s push (2.5 bars, room
for the title card and its lower third) and 13 gives that half bar back
(4.5 bars), its camera settling in the tail rather than freezing there.

**What #436's first master made** (2026-11-08, silent, from `cut_16x9.toml`):

| File | For | Format | Size |
|---|---|---|---|
| `many-hands-16x9-master.mp4` | YouTube | 3840x2160, 60 fps, H.264 High 5.2, crf 14, yuv420p, BT.709, 0.5 s closed GOPs, 81 Mbit/s | 1,276 MB |
| `many-hands-web.mp4` | the Flint page | 1920x1080, 30 fps, H.264 High, crf 32, BT.709 | 36.4 MB |
| `many-hands-web.webm` | the Flint page | 1920x1080, 30 fps, VP9 two-pass 2.1 Mbit/s, BT.709 | 33.1 MB |
| `many-hands-poster.jpg` | the Flint page | 1920x1080, beat 13 at 3 s, no titles | 0.23 MB |

They landed in `~/flint-film/cuts/many-hands-16x9/` (now `~/flint-film/master/edit/`) beside `<name>.json`
(sizes), `<name>.check.json` (the frame checks), `<name>.frames.json` (which
source frame each film frame is) and the thumbnails; none is in the repo.
The 20 takes behind them, captured at 4K and 60 fps, are 10,940 frames and
77 GB; the capture took about 22 minutes of player time, the edit and
master encode 23 minutes (5.6 frames a second, x264 `slow` the bottleneck),
the web files about 16 (11 of them the VP9 passes).

Every frame of all three passed `check_cut.py` (in place, not repeated, the
right picture, not torn; the book's guide has the table and the broken copy
it was proved on). In Chrome 157 the web files presented every frame, none
dropped; the master decoded all 7,560 in real time, and dropped 11 at
presentation on a loaded machine.

**What #593's cut made** (2026-11-09, on the Legion, which has no room for
the 4K takes: the same commands at 1280x720, `capture.py --cut master
--width 1280 --height 720` then `cut.py master --width 1280`; the 4K files
are those commands without the size, on a disk with ~60 GB free):

| File | Format | Size | Sound |
|---|---|---|---|
| `many-hands-16x9-master.mp4` | 1280x720, 60 fps, H.264 High, crf 14, 7,200 frames | 225.5 MB | AAC 320k, 120.000 s, -14.01 LUFS, -1.49 dBTP |
| `many-hands-web.mp4` | 1280x720, 30 fps, H.264 High, crf 32 | 18.7 MB | AAC 160k, 120.000 s, -14.04 LUFS, -1.44 dBTP |
| `many-hands-web.webm` | 1280x720, 30 fps, VP9 two-pass 2.1 Mbit/s | 34.6 MB | Opus 160k, 120.000 s, -14.00 LUFS, -1.26 dBTP |

All three passed `check_cut.py master`: every frame in place, none repeated,
wrong or torn, the sound as long as the picture to the sample, and all 20
shot starts on the music's frames. The 20 takes were 8,996 frames (12 GB)
and five minutes of player time; the edit composed at ~100 frames a second
and the master encoded in 74 s, the VP9 passes taking most of the rest.
Recut by the same editor, the reel and its four stories passed too (-14.03
LUFS, 18 of 18 shots on their frames).

**Hosting.** The film is media, not part of the wasm or any game bundle, so
it doesn't count against the 4 MB gzip line in
[web.md's Size](web.md#size). On the Flint page it belongs where the page's
other films are, as the hero `video` with `videoPoster`, which the page
loads on its own. The page's current hero films are 1 to 6 MB for 20 to
60 s; this one is about 34 to 37 MB for 2:00 with its sound, because ASCII is fine detail
in every frame (the same cut at crf 27 and 60 fps was 80 MB, and 720p blurs
8-point cells), so it should be click-to-play behind its poster
(`preload="none"`), not a looping hero. Publishing it (the page, the book, YouTube) is outward-facing
and waits for Chris.

### What main brought since the first draft

The storyboard was drawn before 68 commits of stylize work landed. What
changed in it, and what didn't:

- **Changed.** The scripts call `set_post_style` / `set_post_style_param`
  (#369), which landed doing what this PR's `set_scene_style` did, so that
  one went. The looks beat has three more takes, the pencil looks
  (#391). Beats 05 and 12 click a menu that grew a Look row (#356) and, in
  05, a mask legend (#309) that widens the panel; their input scripts were
  re-measured and recaptured. Every take is now byte-identical (#341,
  below).
- **Unchanged, on purpose.** The beats still show the #260 batch, one
  feature each. The follow-ups (palette quantize #324, display bloom #321,
  whole-cell silhouettes #338, style ownership #384, world-anchored pencil
  #389, paper from images #390, per-prop dissolve ranks #398, per-entity
  user styles #414) draw inside the existing beats where those beats
  already use them (07's dissolve ranks each prop's cells by #398 now;
  11's holograms stay on the figure and ball, their `style_mask` being
  what switches a mask-reading user style on since #414), but none has a
  beat of its own.
- **Worth a decision.** Three would make the film better if a beat is
  added or reshaped: 08's switch to the whole page could cross-fade with a
  whole-frame `blend` ramp (#369) instead of cutting; 13's finale could put
  whole-frame ASCII behind the props and let style ownership (#384) keep
  every prop in its own hand; and a palette-quantize take (#324) would sit
  naturally among the looks.

## The vertical cut (#437)

The Instagram reel and its stories: `tools/showcase/cuts/reel.toml`,
captured with `capture.py --cut reel` at 1080x1920 and cut by
`tools/showcase/cut.py reel` into `reel.mp4` and four story pieces.

**Length.** 24 bars at 100 bpm (2.4 s a bar, the clock beat 02 already
changes on), 57.6 s, inside the 30 to 60 s asked for. Every cut is on a
bar or half bar so the music (#438, written to the same file) changes on
the cuts; the looks change on every beat, one take a quarter bar. Four
stories of exactly 6 bars (14.4 s) each, split at shot boundaries.

| Story | Shots (bars) |
|---|---|
| 1 | 01 the clearing, title (1.5) · 02 the world in type (3) · 03 edges (1.5) |
| 2 | 04 some things in type (1.5) · 06 a script tells it (2.5) · 07 fade and dissolve (2) |
| 3 | 08 pencil (2) · 09 watercolor (2) · 10 looks, eight takes (2) |
| 4 | 11 styles you write (2) · 13 many hands, the end card over its last 2 bars (4) |

In-points sit on the beats' own cues: 02 starts at 2.4 s so tint, CGA and
amber land on downbeats, and 08 and 09 start at 2.6 s so their whole-frame
switches at 5.0 s land on a shot's second bar.

**Left out, and why.** Beats 05 (the mask in the F4 menu) and 12 (tuning
by hand in F4) are the two whose subject is egui: on a 1080-pixel-wide
frame the menu's rows are a few millimetres tall on a phone and its panel
covers half the frame, and their input scripts click positions measured on
the master's 1920x1080-point canvas. They stay in the 16:9 master. What
they show that matters at phone size appears elsewhere: the style mask is
what draws 03's outline round the ball and keeps 13's styles apart.

**Framing.** Captured, not cropped: a 9:16 crop is a third of the
master's width and loses the barrel or the figure. In a portrait window
`film.rhai`'s moves back the camera off by 1.45x along its line of sight,
open the field of view to 60° and aim 0.3 m low, which keeps the still
life in the middle of the safe area; 04, 06 and 07, whose subjects (the
figure, the barrel, the crate) stand either side of the table, film from
the barrel's far side instead, where the three are closest together
across the frame. The master's scripts are unchanged in a landscape
window.

**Cells.** Point-sized things are laid out on a 720-point canvas
(`points_wide`, pixels per point 1.5): an 8-point ASCII cell is 12 px, 90
across the frame. A sweep at 1080x1920 (`flint render --pixels-per-point 1
--sweep ascii.cell=4.5,8,12,16 --sweep-crop 240,760,600,560`) showed the
master's canvas (4.5 px cells) reading as texture, 8 px glyphs only just
legible at 1:1, 12 px reading as characters, and 16 px too coarse to draw
the table. Pencil spacing, outlines and edge widths scale with it.

**Safe area.** Captions and key content stay inside x 120 to 960, y 250 to
1480: the intersection of the story safe area (the central 1080x1420) and
the reel's (clear of about 210 px of top bar, 440 px of caption and
buttons at the bottom and 120 px of buttons down the right). Captions sit
at the top of it, the still life in its middle. `cut.py --guides` shades
the rest.

**Instagram, as checked on 2026-11-08.** Sources disagree, and Meta's own
help pages weren't among them, so the cut stays inside the tightest of
what they say: 1080x1920 9:16, H.264 MP4 at 30 fps with AAC audio (larger
frames are downscaled, so 4K gains nothing); reels of up to 3 minutes in
the app (some sources say 90 s, one says more), and a story segment up to
60 s, longer ones split. A 15-second story is no longer a hard cap, but
the four 14.4 s pieces are what the ticket asked for and play as one story
sequence either way. Safe-zone figures range from 250 px top and bottom
(stories) to 210 top, 440 bottom and 120 right (reels); the cut uses the
tighter of each. Check again before posting.

**Encoding.** `cut.py` pipes each composed frame once to ffmpeg for the
whole cut and for the story piece it falls in: libx264 High at level 4.2,
CRF 16 capped at 24 Mb/s, yuv420p tagged BT.709 limited range, a keyframe
a second (Instagram recompresses; a clean, high-bitrate upload loses
least), AAC 192 kb/s at 48 kHz, `+faststart`. The soundtrack is the cut's
`audio`, #438's `many-hands-reel.wav`, written to the same 24 bars; until
it is built the cut is silent and `cut.py` says so.

**Checked (2026-11-09, RTX 4070, Vulkan).** `reel.mp4` is 1080x1920, 1728
frames, 57.600 s of video and audio, -14.0 LUFS integrated after AAC;
each story is 432 frames, 14.400 s. Two portrait captures of 07 and of the
blueprint look (`capture.py --cut reel --verify --scale 0.5`) were
byte-identical, every frame. The portrait branch never runs in a landscape
window: 01, 04, 06 and 07 at 960x540 with these scripts and with main's
were byte-identical, so the master's takes are unchanged.

## Music

*Many Hands* has a score of its own (#438), written for the film and
rendered by Flint's music tools. One melody plays through the whole cut and
each shot plays it in its own instrument, as each beat redraws the same
clearing in its own hand: an electric piano for the plain render; pulse
waves for the type beats, their width changing with each of 02's tables;
plucked lines when 03's edges come up; dry strings and the scratch of a
pencil trembling at the boil's 6 Hz for 08; a breathy flute for the
watercolor, its pad closing as the paint lifts at 5 s; FM glass for the
holograms; a new voice for every look; a music box and the menu's own
clicks for 05 and 12. In 13 every hand takes a bar of the tune, and it comes
home to D as the camera rises. The crickets that open the film are still
there as it fades.

| Cut | List | Bars | Length | Frames at 30 fps |
|---|---|---|---|---|
| 16:9 master (#436, #593) | `tools/showcase/cuts/master.toml` | 48 + a 2-bar tail where 13 runs on | 120.0 s | 3600 (7200 at its 60) |
| 9:16 reel (#437) | `tools/showcase/cuts/reel.toml` | 24, four 6-bar stories | 57.6 s | 1728 |

```bash
python tools/showcase/music.py          # ~/flint-film/music/many-hands-<cut>.wav (+ .json)
python tools/showcase/music.py --plot   # and a sheet: shots, spectrogram, bus levels
```

`score.py` arranges the theme to a cut's shots; `synth.py` synthesizes the
six stems flint-music mixes (foundation, harmony, world voice, theme,
the figure's motif, texture); `music.py` writes the suite and its event
script (`tools/showcase/music/<cut>.suite.toml`, `.events.toml`: a marker
at every shot, each shot's bus levels, the watercolor and menu low-passes,
the last bar's fade), renders them with `flint render-suite`, and masters
to -14 LUFS integrated (BS.1770-4) under a -1.5 dBTP ceiling, 48 kHz
24-bit. Each WAV is exactly its cut's length, and two builds are
byte-identical on one machine (numpy 2.3, scipy 1.17).

**Every sound is original.** The score was written for this film, and
every note and noise in it is synthesized from sines, noise and FM by
`tools/showcase/synth.py`: no samples, loops, presets or recordings from
anywhere else, and no library music. YouTube's Content ID and Instagram's
audio matching have nothing to match it against, and the rights are
Chris's. If a platform flags it anyway, this page and the build command
are the evidence.

The cut lists belong to the edits (#436, #437). Change one, rebuild the
music, and the track follows: the instruments change on the new cut lines,
and the cues inside a shot follow its `start_s`. Lengths stay in
quarter-bars, and each cut ends on a whole bar.

## Capture

```bash
cargo build --release --bin flint-player
python tools/showcase/capture.py            # every beat, every take, 3840x2160
python tools/showcase/capture.py --scale 0.5 02_type  # one beat at 1920x1080
python tools/showcase/capture.py --verify   # each take twice, compared
```

Takes land outside the repo (`~/flint-film/<beat>/<take>/frame_0001.png`;
`--out` moves them). At 60 fps the film is 10,940 frames; a 4K frame of these scenes averaged
7.0 MB, so the whole film is about 77 GB at 4K and 20 GB at 1080p; the rig refuses to
start a run that won't fit.

## Determinism

Every take is byte-identical run to run. All 20 takes were captured twice
with `--verify` at 1280x720 on 2026-11-08 (RTX 4070, Vulkan), after main's
#341 placed grass blades in grid order, and every frame of every pair
matched (the table is in the book's
[Filming a Showcase](../book/src/guides/filming-a-showcase.md#two-runs-one-film)).
The first storyboard's captures, before #341, differed by a few pixels a
frame at grass blades' edges, because the grass compute pass appended blades
with an `atomicAdd` and draw order changed each run. That is gone, so a
beat can be reshot and its frames cut together with an earlier take's.

Three things the rig pins so two runs can match:

- **Time.** `--frame-step 1/fps` steps the game clock, so every script,
  animation, particle and post time is the same on every run. egui's clock
  now steps with it (it read the wall clock, so a header opening in a
  capture landed differently each run).
- **Scale.** `--pixels-per-point width/1920`: ASCII cells and egui are
  sized in points, which followed the window's scale factor (1.25 on the
  machine this was built on), so the same capture differed between
  displays.
- **One sun.** The set has one directional light: the sky's sun disc
  follows "the first", and entities load from a map in no fixed order.
