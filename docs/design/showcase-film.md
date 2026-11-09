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

The set is `demo/film/set.toml`, written once; each beat is a fragment in
`demo/film/beats/` that adds its styles and its script, and
`tools/showcase/build_scenes.py` joins them into `demo/film/<beat>.scene.toml`.
All content is Flint's own (the meadow mask, the demo props, the built-in
glyphs and palettes, the styles in `demo/styles/`).

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

Within a beat, the change is scripted on a timeline (`demo/film/scripts/`,
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
- **Music** (#438) can lean on the beats' internal cues: 02 changes every
  2.4 s (100 bpm in quarter bars), 03's edges come up at 2.5 s, 08's and
  09's whole-frame switches are at 5 s.

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
`tools/showcase/edit.py reel` into `reel.mp4` and four story pieces.

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
at the top of it, the still life in its middle. `edit.py --guides` shades
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

**Encoding.** `edit.py` pipes each composed frame once to ffmpeg for the
whole cut and for the story piece it falls in: libx264 High at level 4.2,
CRF 16 capped at 24 Mb/s, yuv420p tagged BT.709 limited range, a keyframe
a second (Instagram recompresses; a clean, high-bitrate upload loses
least), AAC 192 kb/s at 48 kHz, `+faststart`. The soundtrack is the cut's
`audio`, #438's `many-hands-reel.wav`, written to the same 24 bars; until
it is built the cut is silent and `edit.py` says so.

**Checked (2026-11-09, RTX 4070, Vulkan).** `reel.mp4` is 1080x1920, 1728
frames, 57.600 s of video and audio, -14.0 LUFS integrated after AAC;
each story is 432 frames, 14.400 s. Two portrait captures of 07 and of the
blueprint look (`capture.py --cut reel --verify --scale 0.5`) were
byte-identical, every frame. The portrait branch never runs in a landscape
window: 01, 04, 06 and 07 at 960x540 with these scripts and with main's
were byte-identical, so the master's takes are unchanged.

## Capture

```bash
cargo build --release --bin flint-player
python tools/showcase/capture.py            # every beat, every take, 3840x2160
python tools/showcase/capture.py --scale 0.5 02_type  # one beat at 1920x1080
python tools/showcase/capture.py --verify   # each take twice, compared
```

Takes land outside the repo (`~/flint-film/<beat>/<take>/frame_0001.png`;
`--out` moves them). A 4K frame of these scenes is about 14 MB, so the
whole film is about 76 GB at 4K and 19 GB at 1080p; the rig refuses to
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
