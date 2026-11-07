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
- **Looks need takes.** `--look` replaces a scene's whole post-processing,
  so the looks beat is one camera move captured five times and cut between.

Within a beat, the change is scripted on a timeline (`demo/film/scripts/`,
game time only), so a beat is one continuous shot with its cuts inside.
Two engine additions made that possible: `set_scene_style` /
`set_scene_style_param` (scripts edit the whole-frame style tables, as
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
| 10 | Looks | 8 × 5 takes | looks (#279) | one orbit under `ascii_noir`, `matrix`, `ink_wash`, `graphite`, `watercolor`: cut between takes on the same frame of the move | identical orbit in every take | new, five takes |
| 11 | Styles you write | 10 | user-authored WGSL styles (#274, #275) | the figure and the ball are holograms; the script prints the frame as a risograph, then lays survey contours over it | orbit | new |
| 12 | Tuning by hand | 12 | the F4 Styles editor (#268) | the input script opens F4 → Styles, drags the ASCII cell size coarse then fine, picks palette, amber, then CGA | near-still, menu on the right | new, input script |
| 13 | Many hands | 12 | per-entity ASCII, pencil, watercolor and a user style on one screen | every prop a different hand at once; the camera rises and pulls back until the sky takes the frame, room left for the title | long rise and pull back | new |

About 2 min 45 s of footage before the edit; a cut uses a few seconds of
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
- **The vertical cut** starts from a crop of the 16:9 master: a 9:16
  window is a third of its width, and the moves were framed for 16:9, so
  check each beat's subject stays inside the crop (the barrel and figure
  sit either side of the table and drift toward the edges in 04 and 07).
  A native vertical take is `capture.py --width 1080 --height 1920`, which
  needs the moves reframed for it (#437).
- **Music** (#438) can lean on the beats' internal cues: 02 changes every
  2.4 s (100 bpm in quarter bars), 03's edges come up at 2.5 s, 08's and
  09's whole-frame switches are at 5 s.

## Capture

```bash
cargo build --release --bin flint-player
python tools/showcase/capture.py            # every beat, every take, 3840x2160
python tools/showcase/capture.py --scale 0.5 02_type  # one beat at 1920x1080
python tools/showcase/capture.py --verify   # each take twice, compared
```

Takes land outside the repo (`~/flint-film/<beat>/<take>/frame_0001.png`;
`--out` moves them). A 4K frame of these scenes is about 14 MB, so the
whole film is about 66 GB at 4K and 17 GB at 1080p; the rig refuses to
start a run that won't fit.

## Determinism

Every take was captured twice with `--verify` (the table is in the book's
[Filming a Showcase](../book/src/guides/filming-a-showcase.md#two-runs-one-film)).
No take with grass is byte-identical, and none differs where anyone could
see it: at most a few pixels a frame at grass blades' edges, and now and
then a single ASCII cell's glyph for a few frames. With the grass taken
out, the same beats are byte-identical. That names #341's cause: the grass
compute pass appends blades with an `atomicAdd`, so draw order (and the
winner of an equal-depth pixel) changes each run. It doesn't matter for the
film; each beat is captured once and cut from that take.

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
