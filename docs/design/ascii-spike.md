# ASCII spike: per-entity vs full screen (#261)

*2026-11-06. Part of the stylize epic, #260. The renders, the zooms and a
pan animation are on the presentation page linked from PR and issue #261.*

## The question

Chris prefers ASCII per entity, because it should give objects definition.
His worry is that per-entity glyphs won't sit on one grid. A full-screen
pass is simpler and keeps one grid. This spike built both as throwaway
code, so the choice could be made by eye:

| Variant | `flint render` flags | What it does |
|---|---|---|
| Full screen | `--ascii full` | Glyph pass over the whole HDR frame, after Kuwahara and before the composite |
| Per entity, screen grid | `--ascii entity --ascii-entities <list>` | A mask pass redraws the chosen entities into a side target; the glyph pass glyphizes only masked pixels, on one screen grid |
| Per entity, object grid | `--ascii entity-object --ascii-entities <list>` | As above, but each entity's grid is anchored at its projected origin |

The code is `crates/flint-render/src/ascii_spike.rs` and its two shaders.
It is off by default and adds nothing to a frame unless `--ascii` is set.
`flint render` also gained `--render-scale` and `--gpu-time <frames>`.

## How the prototypes work

- **Glyphs.** Ten hand-made 8x8 bitmaps sit in the shader, ordered by how
  many pixels they ink: `" .-:+=%#*@"`. Each cell averages a 4x4 lattice of
  the scene and picks a glyph by the Reinhard-compressed luminance.
  Glyph density carries brightness. The ink is the cell's hue at full
  value, so a dark object's sparse glyphs still read as its colour.
- **Mask.** After the scene pass, the chosen entities' opaque draws are
  redrawn with their cached transform bind groups into an `Rgba32Float`
  target (style id, anchor x, anchor y). There's no depth attachment. Each
  fragment tests itself against the frame's single-sample depth with a
  tolerance of twice its own depth slope. Under MSAA that is the sample-0
  resolve, and under a render scale it is the scaled depth.
- **Masked cells** average only lattice points of their own entity. A cell
  cut by a silhouette keeps the object's colour, and the silhouette itself
  stays pixel-exact.

## Scenes

`demo/phase4_runtime` (the tavern), `demo/mesh_meadow` (barrel, bench,
lantern), `demo/campfire` (stones, with particles) and `demo/pbr_test`.
Woozy Grove (#217) isn't on main yet, so the meadow stands in for it.

## Findings

1. **The grid worry belongs to the object grid, not to per-entity.**
   Per-entity on one screen grid lines up everywhere. With every entity
   masked (`--ascii-entities '*'`) it is nearly indistinguishable from full
   screen. Per-entity is about *selection*. The object grid is where the
   seams come from: where the tavern's wall meets its floor, and around the
   bench, neighbouring grids start their rows at different offsets.
2. **The object grid buys less stability than it looks.** It keeps glyph
   phase only under translation: anchoring at a projected origin can't
   follow rotation or a change of depth. Under a camera pan the screen
   grid's glyphs crawl across the barrel by its screen motion mod 8 px.
   The object grid's stay put until the barrel turns or comes closer.
   Temporal stability (#277) needs more than an anchor, such as object-space
   cells or a temporal lock.
3. **Per-entity gives a crisp edge, not crisp glyphs.** The mask's
   silhouette is pixel-exact and cuts glyph cells in half, which reads as a
   cut-out with ASCII texture. Full screen defines an object differently:
   its own glyphs against the glyph field around it. That kept the meadow's
   bench readable, while per-entity turned the bench's one-cell legs into
   dark smudges. Thin features need edge glyphs (#267) or an outline more
   than per-entity needs a mask.
4. **Dark objects fade.** Luminance-to-coverage maps a dark barrel to `.`
   and `-`. Hue-carrying ink helps, but a per-style exposure or a per-object
   luminance normalisation would keep dark props legible.
5. **The mask redraw covers static meshes only.** The tavern's characters
   (skinned `YBotWithIdle`) stayed unmasked. Terrain, grass, particles and
   sprites aren't masked either. Each vertex format needs its own mask twin:
   skinned needs the bone group, and grass is instanced.
6. **MSAA 4 works.** The slope tolerance absorbs the difference between
   the resolved sample-0 depth and the mask's pixel-centre depth, and the
   silhouettes have no holes. The mask edge is aliased (single-sample)
   against MSAA colour edges, which is hard to see under glyphs.
7. **Render scale is the real problem.** Glyphs are drawn at scene
   resolution and then upscaled. At 0.5, an 8 px cell becomes a blurred
   16 px glyph. The glyph pass belongs at output resolution, after the
   upscale, with cells in output pixels.
8. **Placement.** The glyph pass runs in HDR, before the composite. Fog,
   SSAO, bloom, vignette and grade therefore apply over the glyphs: bloom
   haloes bright ink and fog washes the far ones. That is a look (terminal
   glow), but a crisp ASCII wants the option of running after tonemapping.
9. **Cost is small.** At 1920x1080 on an RTX 4070, GPU ms per pass,
   smoothed over 240 frames:

   | Scene | MSAA | Scale | Full: `ASCII` | Entity: `mask` + `ASCII` | Object: `mask` + `ASCII` |
   |---|---|---|---|---|---|
   | tavern | 1 | 1 | 0.188 | 0.098 + 0.054 | 0.061 + 0.062 |
   | tavern | 4 | 1 | 0.116 | 0.022 + 0.062 | 0.022 + 0.069 |
   | tavern | 1 | 0.5 | 0.031 | 0.011 + 0.016 | 0.011 + 0.017 |
   | meadow | 1 | 1 | 0.112 | 0.015 + 0.038 | 0.015 + 0.039 |
   | meadow | 4 | 1 | 0.198 | 0.014 + 0.088 | 0.014 + 0.106 |
   | meadow | 1 | 0.5 | 0.030 | 0.009 + 0.012 | 0.008 + 0.013 |

   Run-to-run noise is about ±0.1 ms, so the variants don't separate on
   cost: all of them fit any budget on this GPU. Phones and the web are
   untested (#270).

## Round 2: edges, simpler interiors and a black ground

Chris's review of the first renders (PR #296) asked for three changes:

1. **Textures turn to noise.** The tavern's wood came out as per-cell
   noise, not as a wooden surface, so a scene would have to be authored
   for the look.
2. **There are no edges.** Defined edges matter for ASCII and for pencil
   alike, with a simpler interior inside them.
3. **Unmasked things vanish.** Ground and sky were left out of the
   masked modes; they should at least default to black with edges, so
   the look stays consistent.

What changed:

- **An edge pass** (`ascii_spike_edges.wgsl`) runs before the glyphs. It
  finds edges per pixel from geometry only, never from colour, so wood
  grain makes none. It uses three Sobels over 3x3: relative view distance
  (outlines), normals rebuilt from the depth's world positions (creases
  where faces meet), and the mask's style id (silhouettes). An edge keeps
  only its near side, and a silhouette only its object side, so outlines
  aren't drawn twice.
- **Edge cells.** A cell becomes a line glyph (`| / - \`) when at least 3
  of its 16 lattice points sit on edge pixels, in the direction most of
  them agree on. A cell with more than 9 is treated as texture-like
  geometry and gets tone: grass would otherwise be a field of `|`.
- **Simpler interiors.** Tone comes from a footprint two cells wide and
  is quantized to 4 bands (`" -=#@"` steps), so a texture's grain settles
  into a few flat tones. `--ascii-levels 0 --ascii-footprint 1` restores
  the first round's continuous ramp.
- **A black ground with edges.** In the masked modes, unmasked pixels are
  black, and their edges are still drawn, so the floor, the walls, the
  sky line and even the skinned characters read as line drawing around
  the stylized props. `--ascii-scene-background` brings back the plain
  scene.

Flags: `--ascii-levels`, `--ascii-footprint`, `--ascii-no-edges`,
`--ascii-edge-depth`, `--ascii-edge-normal`, `--ascii-scene-background`.

What it showed:

- **Edges carry the definition.** The tavern became a line drawing, with
  tables as boxes whose tops and fronts meet at a drawn crease, and whose
  interiors are calm rows of one tone. That holds in full screen and in
  both per-entity modes. It confirms finding 3: edges, not the mask, give
  objects their shape.
- **Authoring still matters, but less.** Banding removes texture noise, so
  any lit surface reads as tone. A surface's *material* (wood vs stone)
  still doesn't come through. It would take a per-style texture input,
  e.g. a hatching direction from the material.
- **Particles vanish on the black ground.** In the masked modes, the
  campfire's flames sit on unmasked pixels and draw black. Emissive or
  particle pixels may need to pass through (relates to #290).
- **Cost.** The edge pass reads 25 depths a pixel, so the `ASCII` scope
  (edges + glyphs) is now 0.38 ms full screen and 0.21 ms per entity, at
  1080p on the RTX 4070 (tavern, MSAA 1), up from 0.05-0.19 ms.

## Recommendation

**Build per-entity selection on one screen grid, with full screen as its
"everything" case.** Offer the object grid as an opt-in per style, not as
the default.

- One glyph pass with an optional style mask. "Full screen" is that pass
  with no mask, or with the background given a style too. ASCII,
  watercolor and pencil then share one stylize interface (#262) and one
  mask (#263).
- Keep the mask as a separate redraw, not an attachment on every scene
  pipeline. It cost under 0.1 ms here, needs nothing from the scene
  pipelines, and already works with MSAA and render scale. Add a skinned
  twin. Leave grass, particles and sprites to the full-screen case. Store
  only an id (`R16Uint`), with each style's anchor and parameters in a
  buffer, rather than 16 bytes a pixel.
- Run stylize passes at output resolution, after the render-scale
  upscale, with a choice of before or after tonemapping.
- Edge-aware glyphs (#267) carry the "definition" Chris wants more than
  the mask does. Round 2's geometry-only edges (depth, normals, ids) are
  the starting point. The same edge texture is what a pencil style (#273)
  would draw its contours from, so it belongs in the shared framework
  (#262), not inside ASCII.
- Banded tone over a wider footprint should be the default interior for
  ASCII and pencil alike.

### What this changes in the epic

- **#262** (framework): one pass interface taking an optional style mask
  and choosing its place in the chain (HDR or post-tonemap, scene or
  output resolution).
- **#263** (mask): a redraw pass, with no new attachment on the scene
  pipelines. Static and skinned twins, an id-only target, sample-0 depth
  with a slope tolerance under MSAA.
- **#265** (full screen): the masked pass with no mask. It runs at output
  resolution.
- **#266** (per entity): screen grid by default; `grid = "object"` opt-in.
- **#270** (performance): adds the render-scale placement and the mobile
  numbers this spike couldn't take.
- **#277** (temporal): the object anchor alone doesn't fix rotation or
  depth.
- **#281** (ramps by measured coverage) agrees with the spike: its ramp
  is ordered by inked pixels, not by the classic `" .:-=+*#%@"`.
- None to drop.

### New tickets from the spike

- #288: per-entity ASCII keeps thin features at the silhouette (finding 3)
- #289: per-style exposure and luminance normalisation (finding 4)
- #290: transparents and particles in front of a masked entity (a risk the
  mask design carries; untested here)
- #291: `flint render --variants` grids and zoom crops (the bash and Python
  this spike needed)
- #292: `--gpu-time` reports p50/p90/max, not an average
- #293: remove this spike's code once #265/#266 land
- #294: the tavern's NPCs render 3 cm tall under green boxes
- #295: the tavern demos need a `[camera]` inside the room
