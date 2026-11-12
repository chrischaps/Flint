# Post-Processing

Flint includes an HDR post-processing pipeline that transforms the raw scene render into polished final output: bloom, SSAO, fog, volumetric lighting, depth of field, a painterly Kuwahara filter, color grading, film grain, tonemapping, vignette, FXAA, and whole-screen render modes.

## How It Works

Instead of rendering directly to the screen, the scene is drawn to an intermediate **HDR buffer** (`Rgba16Float` format) that can store values brighter than 1.0. A series of fullscreen passes then process this buffer:

```
Scene render        Depth       Kuwahara     Contour      SSAO       Volumetric   Bloom chain    Composite pass        FXAA
(PBR, skinned,  ->  resolve  -> (optional -> (optional -> depth-  -> shadow-   -> downsample  -> DoF, AO, god rays  -> (optional
 billboard,         (MSAA       painterly    ink          based      based        upsample       bloom, fog, exposure    edge AA)
 particles,         sample 0)   pre-pass)    lines)       AO         god rays                    ACES, grade, desat,
 sky, ocean,                                                                                     render mode, vignette,
 terrain, grass)                                                                                 grain, dither
     |                 |            |            |           |           |            |                |                   |
 Rgba16Float       depth       filtered     inked        AO         vol          bloom          sRGB surface or       sRGB
 HDR buffer        texture     HDR          HDR          texture    texture      texture        FXAA intermediate     surface
```

All scene pipelines --- PBR, skinned mesh, billboard sprite, particle, skybox, sky, ocean, terrain and grass --- render to the HDR buffer when post-processing is active. The PBR shader's built-in tonemapping is automatically disabled so it outputs linear HDR values for the composite pass to process. Kuwahara and FXAA resources are only allocated when their effect is enabled. Kuwahara is a [stylize pass](stylize-passes.md); stylize passes of the display stage (such as `invert`) run after the composite and before FXAA.

## Bloom

Bloom creates the soft glow around bright light sources --- emissive materials, fire particles, bright specular highlights. The implementation uses the technique from Call of Duty: Advanced Warfare:

1. **Threshold** --- pixels brighter than `bloom_threshold` are extracted
2. **Downsample** --- a 5-level mip chain progressively halves the resolution using a 13-tap filter
3. **Upsample** --- each mip level is upsampled with a 9-tap tent filter and additively blended back up the chain
4. **Composite** --- the final bloom texture is mixed into the scene at `bloom_intensity` strength

![Bloom enabled — bright sources produce soft halos](../images/bloom-on.png)

*Post-processing enabled: bloom creates halos around emissive surfaces and bright lights.*

![Bloom disabled — same scene with raw PBR output](../images/bloom-off.png)

*Post-processing disabled: the same scene rendered with shader-level tonemapping only.*

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `bloom_enabled` | bool | `true` | Enable bloom |
| `bloom_intensity` | f32 | `0.04` | Mix strength of the bloom texture |
| `bloom_threshold` | f32 | `1.0` | Minimum brightness extracted into the bloom chain |

The mip chain depth is calculated as `floor(log2(min(width, height))) - 3`, capped at 5 levels, ensuring the smallest mip is at least 8x8 pixels.

## SSAO (Screen-Space Ambient Occlusion)

SSAO darkens crevices, corners, and areas where surfaces meet, adding depth and realism to a scene without requiring extra light sources. The implementation samples the depth buffer around each pixel to estimate how much ambient light would be blocked by nearby geometry.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `ssao_enabled` | bool | `true` | Enable SSAO |
| `ssao_radius` | f32 | `0.5` | Sample radius in world units (larger = wider darkening) |
| `ssao_intensity` | f32 | `1.0` | Occlusion strength (higher = darker crevices) |
| `ssao_samples` | u32 | `64` | Hemisphere samples per pixel, 1--64 ([ADR 0052](https://github.com/chrischaps/Flint/blob/main/docs/adr/0052-authorable-ssao-sample-count.md)) |

SSAO is the heaviest per-pixel pass in the post stack. The sample kernel is strided, so lower counts keep full radius coverage; `ssao_samples = 16` is usually indistinguishable on soft matte scenes and roughly four times cheaper. The clay-look demo scenes use 16.

An SSAO depth bias (default `0.025`) is exposed in the Rendering & Effects menu but has no scene key.

## Depth of Field

Depth of field defocuses everything outside a focus band. The composite pass computes a circle of confusion per pixel from the depth buffer and gathers a CoC-weighted disc from the HDR source, so in-focus foreground does not bleed into defocused regions.

![Depth of field, near focus](../images/dof-near.png)

*Focus plane at 3 m: the foreground is sharp and the room behind it falls away.*

![Depth of field, far focus](../images/dof-far.png)

*Focus plane at 12 m: the same frame with the focus pushed back.*

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `dof_strength` | f32 | `0.0` | 0 = off/sharp, 1 = full defocus |
| `dof_focus_distance` | f32 | `10.0` | Distance of the focus plane in world units |
| `dof_focus_range` | f32 | `5.0` | Half-width of the sharp band around the focus plane |

Distances are plain view-space meters. Earlier builds linearized depth with the OpenGL `[-1, 1]` convention, which made focus values drift with the far plane; [ADR 0055](https://github.com/chrischaps/Flint/blob/main/docs/adr/0055-wgpu-zero-to-one-depth-convention.md) moved every depth consumer to wgpu's `[0, 1]` convention, so `dof_focus_distance = 10` now means ten meters from the camera.

In the scene viewer, the Rendering & Effects menu offers **DoF follow**: the focus plane tracks the last selected entity, which is a fast way to find a focus distance by eye.

## Fog

Distance-based fog blends a configurable fog color into the scene based on pixel depth. A layer of height fog can be added to it, thick low down and thinning with altitude, and an aerial tint can turn far things toward a second colour. Fog is applied in linear HDR space, before tonemapping.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `fog_enabled` | bool | `false` | Enable distance fog (and the height fog and tint below) |
| `fog_color` | [f32; 3] | `[0.7, 0.75, 0.82]` | Fog color (linear RGB) |
| `fog_density` | f32 | `0.02` | Exponential density factor |
| `fog_start` | f32 | `5.0` | Distance where fog begins |
| `fog_end` | f32 | `100.0` | Distance where fog reaches full opacity |
| `fog_height_enabled` | bool | `false` | Add the height fog layer |
| `fog_height_density` | f32 | `0.0` | Height fog's density at `fog_height_origin`; 0 adds none |
| `fog_height_falloff` | f32 | `0.1` | How quickly height fog thins with altitude (density falls by e every `1/falloff` m) |
| `fog_height_origin` | f32 | `0.0` | World Y where height fog has `fog_height_density` |
| `fog_tint_color` | [f32; 3] | `[0.42, 0.50, 0.65]` | Aerial tint colour (linear RGB) |
| `fog_tint_strength` | f32 | `0.0` | How much of the tint the farthest things take; 0 is off |
| `fog_tint_start` | f32 | `100.0` | Distance where the tint starts to come in |
| `fog_tint_end` | f32 | `1000.0` | Distance where it reaches `fog_tint_strength` |

**Height fog** ([ADR 0101](https://github.com/chrischaps/Flint/blob/main/docs/adr/0101-height-fog-layer-and-aerial-tint.md)) is a layer of air whose density is `fog_height_density` at `fog_height_origin` and falls exponentially above it (and rises below it). The composite integrates that density exactly along each view ray, from the camera to the pixel, and adds it to the distance fog's optical depth: `fog = 1 - exp(-(fog_density * max(d - fog_start, 0) + height_depth))`. So a view low in the layer goes milky at a short distance, a view across it from above sees only the hollows fill, and the layer lies round the camera too (it isn't held off by `fog_start`). The layer is one horizontal slab across the whole scene: for mist that keeps to valleys at different heights, use the ground fog below. Before [ADR 0101](https://github.com/chrischaps/Flint/blob/main/docs/adr/0101-height-fog-layer-and-aerial-tint.md), height fog took `max(distance fog, exp(-falloff × h) × distance fog)`, which is never more than the distance fog, so it did nothing on screen. `fog_height_density` defaults to 0, so a scene that turned height fog on then looks as it did.

**The aerial tint** ([ADR 0101](https://github.com/chrischaps/Flint/blob/main/docs/adr/0101-height-fog-layer-and-aerial-tint.md)) mixes `fog_tint_color` over each drawn pixel by `fog_tint_strength × smoothstep(fog_tint_start, fog_tint_end, d)`, before the fog is mixed in. Far land turns blue (aerial perspective), while the farthest of it still runs into `fog_color` and, with a fog the colour of the sky's horizon, into the sky with no line. Like the fog, the tint is light in the air, not a surface colour: keep it with the hour, darker at night, as you would `fog_color`. It leaves the sky alone.

### Ground fog

**Ground fog** ([ADR 0106](https://github.com/chrischaps/Flint/blob/main/docs/adr/0106-ground-fog-on-own-floors.md)) is fog that lies on floors of its own, so a mist can fill hollows at many heights where the height fog is one level layer. The game gives it a map over a world rectangle, `GroundFog { origin, size, width, depth, texels }`, each texel a `[floor, amount]`: the floor a world Y (the ground's, usually) and the amount 0 to 1. Over a texel the density is `amount × ground_fog_density` at the floor, falling by e every `1 / ground_fog_falloff` metres above it and holding at the floor's under it. Set the map with `SceneRenderer::set_ground_fog(device, queue, &fog)` (`ExtensionGpu::set_ground_fog` from an extension), once and again when the ground changes; `clear_ground_fog` removes it, and so does a map with no fog on it. The look is on the post-process config and can change every frame:

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `ground_fog_density` | f32 | `0.0` | Density per metre at a floor where the amount is 1; 0 is off |
| `ground_fog_color` | [f32; 3] | `[0.8, 0.82, 0.86]` | Its colour (linear RGB). It isn't lit: give it the hour's |
| `ground_fog_falloff` | f32 | `0.5` | It falls by e every `1/falloff` m over the floor |
| `ground_fog_steps` | u32 | `24` | Steps each view ray marches, 1 to 64 |

The composite marches each view ray through the box the fog can lie in: the map's rectangle, from the lowest floor up to where the fog over the highest has thinned to e⁻⁷. The march ends at what the depth buffer says the ray meets, or runs on through the sky. Its steps lengthen with distance from the eye, in proportion, so they're fine near it wherever the march begins. The map is read at the ends of each step, so each read serves two, at the mip level the step spans, so a long step sees the fog averaged over what it crosses. Along a step the height over the floor runs straight from one end's to the other's, so on a slope the floor follows the ground, and the falloff is integrated along it exactly: a ray that dips into a thin layer between two reads still meets it. The ends are moved along the ray by a hash of the pixel, so what's left of the steps reads as fine grain, not bands. The mist is mixed over the scene before the aerial tint and the fog, which take it with what lies behind it. It rides `enabled`, not `fog_enabled`. The map is stored as half floats, each floor over the lowest, so keep its range of floors to a few hundred metres for centimetre steps.

A texel's value is at the middle of its cell, and the fog blends the four round a point, so give a texel with no fog the ground's height too: its floor still shapes the fog at its edges. Its cost is the march over every pixel whose ray crosses the box, `ground_fog_steps + 2` reads each; at 1080p with 24 steps it was within the noise of a frame without it in The Capitol's wilds.

Fog reaches **everything drawn, at any distance** ([ADR 0078](https://github.com/chrischaps/Flint/blob/main/docs/adr/0078-fog-reaches-everything-drawn.md)). The sky and skybox never write depth and the scene clears it to exactly 1.0, so the composite treats a pixel as sky only when its depth is 1.0; any geometry inside the camera's far plane is fogged. Set `far` to what the scene draws, not larger. SSAO, volumetric lighting and depth of field use the same sky test. Before [ADR 0078](https://github.com/chrischaps/Flint/blob/main/docs/adr/0078-fog-reaches-everything-drawn.md) the cut was depth ≥ 0.9999, which with a 1 m near plane is about 2.9 km at `far` = 4 km and never more than 10 km, so a ground running to the horizon ended in an unfogged band.

## Volumetric Lighting (God Rays)

Volumetric lighting simulates light scattering through participating media (dust, fog, haze), producing visible shafts of light (god rays). The effect ray-marches from each pixel toward the camera, sampling the shadow map at each step to determine whether that point in space is lit or in shadow.

### How it works

1. For each screen pixel, reconstruct its world position from the depth buffer
2. March `volumetric_samples` steps along the view ray from the pixel back toward the camera
3. At each step, project the position into shadow-map space and sample the cascaded shadow map
4. Accumulate light contribution where the sample is not in shadow, applying exponential decay
5. The resulting volumetric texture is additively blended into the scene during the composite pass

Because volumetric lighting depends on the shadow map, it requires at least one directional light with shadows enabled. The effect is disabled when shadows are off.

### Per-light configuration

Each directional light can control its volumetric contribution independently via its `light` component (see [Lighting](lighting.md) for the full component):

```toml
[entities.sun.light]
type = "directional"
direction = [0.4, 0.6, 0.05]
color = [1.0, 0.92, 0.75]
intensity = 6.0
volumetric_intensity = 4.0               # god ray brightness (0 = no rays)
volumetric_color = [1.0, 0.88, 0.6]      # tint for the light shafts
```

| Light field | Type | Default | Description |
|-------------|------|---------|-------------|
| `volumetric_intensity` | f32 | `0.0` | Per-light god ray strength (0 = disabled for this light) |
| `volumetric_color` | [f32; 3] | light color | Tint color for the shafts from this light |

### Global scene settings

The `[post_process]` block controls the overall volumetric pass:

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `volumetric_enabled` | bool | `false` | Enable volumetric lighting |
| `volumetric_samples` | u32 | `32` | Ray-march steps per pixel (higher = smoother, more expensive) |
| `volumetric_density` | f32 | `1.0` | Scattering density multiplier |
| `volumetric_max_distance` | f32 | `100.0` | Maximum ray-march distance from camera |
| `volumetric_decay` | f32 | `0.98` | Exponential decay per step (closer to 1.0 = shafts extend further) |

### Example: dungeon window

```toml
[post_process]
volumetric_enabled = true
volumetric_samples = 64
volumetric_density = 30.0
volumetric_max_distance = 15.0
volumetric_decay = 0.998
exposure = 2.5

[entities.sun.light]
type = "directional"
direction = [0.4, 0.4, 0.05]
color = [1.0, 0.92, 0.75]
intensity = 6.0
volumetric_intensity = 4.0
volumetric_color = [1.0, 0.88, 0.6]
```

High `volumetric_density` with a short `volumetric_max_distance` and `decay` close to 1.0 produces thick, concentrated shafts --- good for dusty interiors. For outdoor haze, use lower density and longer distance.

## Kuwahara (Painterly Filter)

The anisotropic Kuwahara filter turns the HDR image into flat, brush-like patches that follow local edge direction. It runs as a pre-pass on the HDR buffer before SSAO and bloom, so lighting effects stay crisp on top of the painted surface. Three shaders cooperate: a structure-tensor pass, a tensor blur, and the sector filter itself.

![Kuwahara off](../images/kuwahara-off.png)

*The tavern showcase with the Kuwahara pre-pass disabled.*

![Kuwahara on](../images/kuwahara-on.png)

*The same frame with the anisotropic Kuwahara filter, radius 4.*

Its settings are a [style table](stylize-passes.md), like every other style's. A table switches it on unless it says `enabled = false`:

```toml
[post_process.styles.kuwahara]
radius = 4          # whole pixels
sharpness = 6.0
hardness = 4.0
```

In a scene with a `[wooze]` table its paint can also boil: `boil` moves the kernel and turns the strokes a little on each drawing. See [Shading boil](#shading-boil).

| Key | Default | Menu range | Description |
|-----|---------|------------|-------------|
| `radius` | `4` | 1--8 | Kernel radius in scene pixels, rounded to a whole pixel; larger = broader strokes |
| `sharpness` | `8.0` | 1--16 | How strongly the lowest-variance sector wins |
| `hardness` | `8.0` | 1--16 | Edge hardness between sectors |
| `anisotropy` | `1.0` | 0--2 | 0 = isotropic disc, 1 = sectors stretch fully along the local edge direction |
| `boil` | `0.0` | 0--1 | With a `[wooze]` table, how far each drawing moves the paint, as a share of the radius |

Scenes written before the table used `kuwahara_enabled`, `kuwahara_radius`, `kuwahara_sharpness`, `kuwahara_hardness`, `kuwahara_anisotropy` and `kuwahara_boil` directly in `[post_process]` (the tavern showcase still does). They still work: on load each one moves into the table, so `kuwahara_enabled = true` with `kuwahara_radius = 4` renders the same bytes as the table above with `radius = 4`. A key the table also sets is the table's, and the old keys without `kuwahara_enabled = true` leave the filter off, as they always did.

The filter's textures and pipelines are allocated the first time it is enabled, so scenes that never use it pay nothing. Kuwahara is the first [stylize pass](stylize-passes.md): it runs through the same framework as every later style, in its HDR stage.

## Contour (Ink Lines)

The contour style draws ink lines in screen space. It finds silhouettes as breaks in depth, and the edges between fills as breaks in luminance. It's one full-screen draw, so its cost doesn't grow with the scene. It's an HDR-stage [style](stylize-passes.md) registered after every other built-in, so it runs after the Kuwahara filter: paint sits under the ink and never smears it. A table's `order` can move it, and a [user style](stylize-passes.md#user-authored-styles) at the HDR stage comes after it unless its `order` says otherwise.

Depth is read as the second difference of 1 / distance, which is zero on any flat surface however steep the view, so a floor seen at a grazing angle draws no lines.

```toml
[post_process.styles.contour]
width = 1.5
fade = 26.0
```

| Key | Default | Menu range | Description |
|-----|---------|------------|-------------|
| `width` | `1.25` | 0.25--5 | Half a line's width, in pixels at 1080p. Scaled with the frame, and never under a pixel's reach |
| `threshold` | `0.08` | 0.01--0.5 | How strong an edge must be before it starts to ink. Lower draws more |
| `strength` | `0.9` | 0--1 | How dark the ink goes |
| `luma` | `0.5` | 0--2 | How much edges in luminance count against edges in depth. 0 is silhouettes only |
| `color` | `[0.015, 0.012, 0.02]` | | The ink's colour as the HDR scene holds it (linear); `"#rrggbb"` works too |
| `fade` | `30.0` | 0--200 | Metres at which a line has faded by half, so distant foliage doesn't go solid with ink. 0 never fades |
| `boil` | `0.0` | 0--2 | With a `[wooze]` table, how far each drawing moves the lines, and how much it changes their width and pressure |

Being a style, it also takes `blend` and `order` and turns up in F4's Styles editor, `--style contour:width=2` and `flint render --list-styles`. `flint render --contour` and the `--contour-*` flags still work, as shorthands for its keys.

Scenes written before #599 used `contour_enabled`, `contour_width`, `contour_threshold`, `contour_strength`, `contour_luma`, `contour_color`, `contour_fade` and `contour_boil` in `[post_process]`. They still load, each moving into the table as Kuwahara's old keys do, and without `contour_enabled = true` they leave the ink off, as they always did. `examples/shading_boil.scene.toml` renders the same bytes in either spelling.

Its pipeline is built the first time it is enabled, so scenes that never use it pay nothing. It costs about 0.07 ms at 1080p on an RTX 5070 Laptop GPU, or 0.14 ms boiling.

## Shading Boil

In a scene with a [`[wooze]` table](procgen.md#the-wooze-field), paint and ink can re-decide themselves on the field's beat, the way every frame of a rotoscoped film was redrawn. Each *drawing* moves the Kuwahara filter's kernel and turns its strokes, and moves the contour's lines, changes their width and lifts the pen in places. The scene under them stays where it is.

![One drawing](../images/shading-boil-a.png)

*`examples/shading_boil.scene.toml`, in which nothing moves.*

![The next drawing](../images/shading-boil-b.png)

*The same frame half a second later.*

```toml
[wooze]
preset = "linklater"

[post_process]
boil_on = 1

[post_process.styles.kuwahara]
boil = 0.5

[post_process.styles.contour]
boil = 1.0
```

Each style's `boil` is its own amount; how long each drawing is held is one `[post_process]` key the two share:

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `boil_on` | u32 | `2` | Film frames each drawing is held: 1, 2 or 3 |

A beat of the field's `step` is six film frames, which ones, twos and threes all divide, so a drawing always lands on the beat. The `linklater` preset's half second is therefore 12 frames a second, and on ones a drawing lasts 1/12 s. Leave `boil_on` at its default with that preset and a drawing lasts 1/6 s, which reads as a throb. `examples/shading_boil.scene.toml` gets the same 1/12 s from a quarter-second step on twos. The clock runs on game time, not frames, so a drawing lasts as long at 30 fps as at 144, and it stops when animation does. A change of `step` bends it from where it stands.

The field's `jitter` sets how hard it boils: both styles' `boil` values are multiplied by it, so a preset or zone with `jitter = 0` holds still, and `amplitude` has no say, so a zone can hold its trees still and go on boiling their lines. The field is sampled once a frame where the camera is, since the passes are full-screen. A zone boils when you're in it, and not when you're looking at it from outside.

With either `boil` at 0, or no `[wooze]` table, that pass renders byte for byte as if boil didn't exist. The contour still draws in a scene with no field; its lines just hold still.

To check the cadence, capture one film frame per frame. In a scene where nothing else moves, consecutive pairs are byte for byte the same on twos, and each pair differs from the next:

```bash
flint-player examples/shading_boil.scene.toml \
  --frame-step 0.0416666666666667 --screenshot out/f.png --screenshot-every 0.0416666666666667 --exit-after 2
```

Headless, `--boil-time <seconds>` picks the drawing: the same value gives the same pixels.

## Color Grade, Film Grain and FXAA

Three finishing controls from [ADR 0050](https://github.com/chrischaps/Flint/blob/main/docs/adr/0050-color-grade-film-grain-and-fxaa.md) sit at the end of the composite pass.

**Color grade** is an ASC-CDL-shaped lift/gamma/gain applied right after ACES tonemapping, so it grades the display-referred image: `pow(max(color * gain + lift, 0), 1 / gamma)`. The grade is skipped entirely while all three are neutral.

**Film grain** is hash noise on pixel coordinates, luma-weighted so highlights stay clean, and time-quantized to 24 Hz. It reads the shared post time, which `flint render` fixes at 0 unless you pass `--grain-time`, so headless renders stay deterministic.

**FXAA** is a separate fullscreen pass after composite. It is off by default because the headless pixel-diff gates run without it.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `grade_lift` | [f32; 3] | `[0, 0, 0]` | Per-channel add after ACES (neutral = 0) |
| `grade_gamma` | [f32; 3] | `[1, 1, 1]` | Per-channel midtone curve (neutral = 1) |
| `grade_gain` | [f32; 3] | `[1, 1, 1]` | Per-channel multiply (neutral = 1) |
| `film_grain` | f32 | `0.0` | Grain intensity; 0 = off, 0.02--0.05 is subtle |
| `fxaa` | bool | `false` | Run the FXAA pass on the final composite |

## Desaturation, Chromatic Aberration and Radial Blur

These three are the "feel" levers scripts reach for most; see the script notes below for which ones are sticky.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `desaturate` | f32 | `0.0` | Mix toward a darkened ash grey (0 = full color, 1 = drained). Applied after the grade, before render modes ([ADR 0021](https://github.com/chrischaps/Flint/blob/main/docs/adr/0021-ladder-visuals-through-the-post-stack.md)) |
| `chromatic_aberration` | f32 | `0.0` | Splits the red and blue channels radially from the screen center |
| `radial_blur` | f32 | `0.0` | 8-tap blur that grows toward the screen edges while the center stays sharp |

Desaturation mixes toward `luma * 0.62` rather than neutral grey on purpose: it matches the disintegration ladder of the music-session harness, so the language of "the world draining" reads the same in both.

## Vignette, Exposure and Dither

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `exposure` | f32 | `1.0` | Multiplier applied before ACES |
| `vignette_enabled` | bool | `false` | Darken screen edges |
| `vignette_intensity` | f32 | `0.3` | Vignette strength |
| `vignette_smoothness` | f32 | `2.0` | Falloff exponent from center to edge |
| `dither_enabled` | bool | `false` | 8x8 Bayer ordered dither to reduce banding |
| `dither_intensity` | f32 | `0.03` | Dither strength (0.02--0.05 works best) |

## MSAA

The scene passes can run with 4x multisample anti-aliasing ([ADR 0058](https://github.com/chrischaps/Flint/blob/main/docs/adr/0058-opt-in-4x-msaa-for-scene-passes.md)). Post-processing, shadow and blit passes stay single-sample; depth consumers such as SSAO, fog and DoF read a sample-0 depth resolve.

![MSAA comparison](../images/msaa-compare.png)

*Left: single-sample. Right: `--msaa 4`. Geometry edges smooth out; post effects are unchanged.*

MSAA is a launch option, not a scene key: `flint render --msaa 4`, `flint-player --msaa 4`, or `RendererConfig::sample_count` in code. Valid values are 1 and 4; anything else is clamped to 1 with a warning. The default stays 1 so headless pixel gates remain single-sample.

## Scene Configuration

Add a `[post_process]` block to your scene TOML to configure per-scene settings:

```toml
[post_process]
bloom_enabled = true
bloom_intensity = 0.04
bloom_threshold = 1.0
ssao_enabled = true
ssao_radius = 0.5
ssao_intensity = 1.0
ssao_samples = 16
fog_enabled = true
fog_density = 0.02
fog_color = [0.7, 0.75, 0.82]
volumetric_enabled = false
dof_strength = 0.4
dof_focus_distance = 8.0
dof_focus_range = 4.0
grade_lift = [0.03, 0.02, 0.015]
grade_gamma = [1.0, 1.0, 1.0]
grade_gain = [1.04, 1.0, 0.94]
film_grain = 0.03
fxaa = false
vignette_enabled = true
vignette_intensity = 0.3
exposure = 1.0
```

All fields are optional --- omitted values use their defaults. The full key list with defaults is in [File Formats](../formats/overview.md).

A key that is no field is ignored, so its field keeps its default, and the load warns with the file and the closest known name (#354). The scene still loads, and `flint validate` shows the same line:

```text
WARN levels/demo.scene.toml: [post_process] has no field 'bloom_intensty', did you mean 'bloom_intensity'? (ignored)
```

When the pipeline is disabled (`--no-postprocess` or the menu checkbox), most effects are zeroed: bloom, vignette, fog, dither, desaturate, film grain, DoF, render mode and grade. Two are not: `chromatic_aberration` and `radial_blur` predate the gate and stay live even with post-processing off.

The scene viewer applies the scene's `[post_process]` block on load ([ADR 0046](https://github.com/chrischaps/Flint/blob/main/docs/adr/0046-viewer-applies-scene-post-process.md)). The Rendering & Effects menu can swap between the authored block and the viewer's default look.

## CLI Flags

Override post-processing settings from the command line:

```bash
# Disable all post-processing
flint render scene.toml --no-postprocess

# Adjust bloom and exposure
flint render scene.toml --bloom-intensity 0.08 --bloom-threshold 0.8 --exposure 1.5

# SSAO, with the cheap sample count
flint render scene.toml --ssao-radius 0.5 --ssao-intensity 1.0 --ssao-samples 16

# Fog
flint render scene.toml --fog-density 0.02 --fog-color 0.7,0.75,0.82 --fog-height-falloff 0.1

# Volumetric lighting
flint render scene.toml --volumetric-density 1.0 --volumetric-samples 32

# Depth of field
flint render scene.toml --dof 0.6 --dof-focus 10 --dof-range 5

# Kuwahara
flint render scene.toml --kuwahara-radius 4 --kuwahara-sharpness 8 --kuwahara-hardness 8 --kuwahara-anisotropy 1

# Ink, and a drawing of the shading boil in a scene with a [wooze] table
flint render scene.toml --contour --contour-width 1.5 --contour-boil 1 --kuwahara-boil 0.5 --boil-on 2 --boil-time 0.5

# Grade, grain, FXAA, MSAA
flint render scene.toml --grade-lift 0.03,0.02,0.015 --grade-gain 1.04,1,0.94 --film-grain 0.03 --fxaa --msaa 4

# Combine flags
flint render scene.toml --bloom-intensity 0.1 --exposure 1.2 --volumetric-density 5.0
```

| Flag | Description |
|------|-------------|
| `--no-postprocess` | Disable the entire post-processing pipeline |
| `--no-shadows` | Disable shadow mapping (also disables volumetric) |
| `--msaa <1\|4>` | Scene-pass MSAA sample count (default 1) |
| `--bloom-intensity <f32>` | Override bloom intensity |
| `--bloom-threshold <f32>` | Override bloom brightness threshold |
| `--exposure <f32>` | Override exposure multiplier |
| `--ssao-radius <f32>` | Override SSAO sample radius |
| `--ssao-intensity <f32>` | Override SSAO strength |
| `--ssao-samples <u32>` | Override SSAO samples per pixel, 1--64 |
| `--fog-density <f32>` | Override fog density (0 disables fog) |
| `--fog-color <r,g,b>` | Override fog color |
| `--fog-height-falloff <f32>` | Enable height fog with given falloff |
| `--volumetric-density <f32>` | Override volumetric density (0 disables) |
| `--volumetric-samples <u32>` | Override volumetric ray-march steps |
| `--dither-intensity <f32>` | Override dither strength |
| `--desaturate <f32>` | Desaturation toward ash grey, 0--1 |
| `--dof <f32>` | Depth-of-field strength (0 = off) |
| `--dof-focus <f32>` | Focus plane distance in world units |
| `--dof-range <f32>` | Focus half-width in world units |
| `--kuwahara-radius <u32>` | Enable Kuwahara with this radius |
| `--kuwahara-sharpness <f32>` | Kuwahara sector sharpness |
| `--kuwahara-hardness <f32>` | Kuwahara sector hardness |
| `--kuwahara-anisotropy <f32>` | Kuwahara anisotropy, 0--1 |
| `--kuwahara-boil <f32>` | How far each drawing moves the paint |
| `--contour` | Enable the contour pass |
| `--contour-width <f32>` | Half a line's width at 1080p (enables contour) |
| `--contour-threshold <f32>` | How strong an edge must be before it inks (enables contour) |
| `--contour-strength <f32>` | How dark the ink goes, 0--1 (enables contour) |
| `--contour-luma <f32>` | How much luminance edges count (enables contour) |
| `--contour-color <R,G,B>` | The ink's colour, linear (enables contour) |
| `--contour-fade <f32>` | Metres at which a line has faded by half (enables contour) |
| `--contour-boil <f32>` | How far each drawing moves the lines (enables contour) |
| `--boil-on <u32>` | Film frames each drawing is held: 1, 2 or 3 |
| `--boil-time <f64>` | Seconds of the wooze field to run, which picks the drawing |
| `--film-grain <f32>` | Film grain intensity |
| `--grain-time <f32>` | Post time for grain and mode animation (default 0) |
| `--grade-lift <r,g,b>` | Color grade lift |
| `--grade-gamma <r,g,b>` | Color grade gamma |
| `--grade-gain <r,g,b>` | Color grade gain |
| `--fxaa` | Enable the FXAA pass |
| `--render-mode <0-5>` | Stylized render mode (see below) |
| `--mode-mix <f32>` | Render mode blend strength, 0--1 |
| `--mode-params <x,y,z,w>` | Per-mode parameters |
| `--oren-nayar`, `--sheen-strength`, `--sheen-color` | Lighting levers, documented in [Lighting](lighting.md) |

CLI flags take precedence over scene TOML settings.

## The Rendering & Effects Menu (F4)

There are no longer per-effect function keys. [ADR 0053](https://github.com/chrischaps/Flint/blob/main/docs/adr/0053-one-rendering-and-effects-menu-on-f4.md) consolidated every render and post-process debug control into one egui window, opened with **F4** in both the player and the scene viewer. Every toggle above appears there with its non-binary parameters, and changes write straight through to the renderer.

| Section | Controls |
|---------|----------|
| Post chain | Post-processing on/off, exposure, vignette + intensity + smoothness, chromatic aberration, radial blur, desaturate. Player only: **Freeze script post overrides**, so a running script cannot fight your edits |
| SSAO | Enabled, radius, intensity, bias, samples |
| Depth of field | Strength, focus distance, focus range |
| Fog | Enabled, color, density, start, end, height fog + falloff + origin + density, tint color + strength + start + end |
| Bloom | Enabled, intensity, threshold, soft threshold |
| Grade / Grain / FXAA | Lift, gamma, gain, Neutral grade button, film grain, FXAA |
| Styles | A section per registered style (Kuwahara among them), drawn from its declaration, with Defaults, Authored, Copy TOML and Commit to scene |
| Contour | Enabled, width, threshold, strength, luma, ink colour, fade, boil |
| Shading boil | Paint boil (Kuwahara's), hold each drawing (ones, twos, threes), and what the wooze field says now: step, jitter and the drawing |
| Render mode | Mode combo (None, Matrix, Blood, Drunk, Tron, Underwater), mix, params |
| Dither / Volumetric | Dither + intensity, volumetric + samples + density + max distance + decay |
| Shadows | Enabled, resolution (512 / 1024 / 2048 / 4096; rebuilds the shadow pass) |
| Lighting | Ambient sky, ambient ground, diffuse wrap, Oren-Nayar, sheen color, sheen strength, Reset lighting |
| Camera | Vertical FOV |
| Debug mode | Shading combo over all eight [debug modes](rendering.md#debug-visualization) |

The scene viewer adds two entries of its own: a toggle between the scene's authored `[post_process]` block and the viewer's default look, and **DoF follow**. In the player the menu is compiled in with the `debug-hud` feature (on by default) and F3 deliberately leaves it alone: F3 toggles the scene-component panels (ocean, grass, time of day, and so on), F4 owns this one.

When the pipeline is toggled off, the PBR shader's built-in ACES tonemapping and gamma correction are automatically restored, so the scene always looks correct regardless of the pipeline state.

## Shader Integration

When the post-processing pipeline is active, the engine sets `enable_tonemapping = 0` in the PBR uniform buffer, forcing shaders to output raw linear HDR values. The composite pass then applies, in this order:

1. **Drunk sway** --- render mode 3 warps the sample UV before anything is read
2. **Radial blur** --- 8-tap gather from the HDR source, edges only
3. **Depth of field** --- CoC-weighted disc gather from the same source
4. **Chromatic aberration** --- radial red/blue channel split
5. **SSAO** --- multiplies by the AO texture
6. **Volumetric** --- adds the god-ray texture
7. **Bloom** --- adds the bloom texture at `bloom_intensity`
8. **Fog** --- blends fog color by depth and optional height falloff, still in linear HDR
9. **Exposure** and **ACES tonemapping** --- maps HDR to displayable range
10. **Color grade** --- lift/gamma/gain on the display-referred image
11. **Desaturation** --- mix toward ash grey
12. **Render mode** --- Matrix, Blood, Tron, or Underwater own the color from here
13. **Vignette** --- edge darkening
14. **Film grain** --- 24 Hz hash noise, before dither because grain is signal
15. **Dither** --- 8x8 Bayer, always last inside composite

The composite output stays linear; the sRGB render target applies gamma encoding in hardware. If FXAA is on, composite renders into an intermediate texture and the FXAA pass resolves it to the surface.

When post-processing is disabled (via `--no-postprocess` or the menu), the shader handles tonemapping and gamma internally. This dual-path design ensures backward compatibility with scenes that don't use post-processing.

## Render Modes

Render modes are whole-screen stylizations that branch **after ACES tonemapping
and before vignette**. They are not debug views: they are a supported way to
make the world visibly stop being itself for a while.

![Tron render mode](../images/render-mode-tron.png)

*Mode 4 over an ocean scene: depth-reconstructed world grid, Sobel edges on
depth and luminance.*

![Matrix render mode](../images/render-mode-matrix.png)

*Mode 1: dot-matrix glyphs whose brightness and density track scene luminance.*

![Blood render mode](../images/render-mode-blood.png)

*Mode 2: the palette remap, with the ocean recoloring itself through its own shader.*

![Drunk render mode](../images/render-mode-drunk.png)

*Mode 3: UV sway across the whole composite chain plus a ghosting double-tap.*

![Underwater render mode](../images/render-mode-underwater.png)

*Mode 5: masked by the per-pixel waterline, with banded absorption and a foam lip on the line.*

| Mode | Name | What it does |
|------|------|--------------|
| 0 | None | Normal output. |
| 1 | Matrix | Procedural dot-matrix glyphs per 8x12 px cell. Brightness *and* density track scene luminance, with per-column rain heads. |
| 2 | Blood | Palette remap. Ocean scenes recolor through the ocean shader too, so the water is genuinely a different sea rather than a red filter over a blue one. |
| 3 | Drunk | Pre-sample UV sway warping the whole composite chain, plus a ghosting double-tap. |
| 4 | Tron | Depth Sobel + luminance edges in cyan over a dimmed scene, with a world-space grid reconstructed from depth so it rides real geometry. |
| 5 | Underwater | Masks per pixel against a **waterline plane**, not a patch mask. Banded murk absorption, a wobbled foam lip on the line, light shafts, Snell-window brightening. |

Four uniforms carry the state: `render_mode`, `mode_mix` (0..1 blend),
`mode_time`, and a `mode_params` vec4 whose meaning is per-mode.

| Mode | `mode_params` |
|------|---------------|
| 1--4 | `x` = bleed-mask scale, `y` = mask style (0 = fbm patches, 1 = radial iris), `z` = rate, `w` = spare |
| 5 | `x` = signed eye depth in meters (+ = submerged), `y` = sea energy 0..1, `z` = daylight 0..1, `w` = bioluminescence 0..1 |

Modes 1--4 fade in through an fbm **bleed mask** whose threshold sweeps across
the noise span, so patch coverage grows with `mode_mix` and pins to full
coverage at 1. Mode 5 ignores the mask entirely: being underwater is a fact
about where your eye is, not a patch that spreads.

### From scripts

```rhai
set_render_mode(4, 0.85);                  // mode, mix
set_render_mode_params(3.0, 0.0, 6.0, 0.0);
set_desaturation(0.5);                     // sticky
set_dof(0.6);                              // sticky
set_dof_focus(8.0, 4.0);                   // distance, range; sticky
```

**`set_render_mode` is transient, and it is the only post-process override that
is.** Every other override (exposure, bloom, fog, chromatic aberration,
desaturation, depth of field) is sticky: set it once and it persists. The
render mode is zeroed the frame your script *stops calling it*.

That asymmetry is deliberate. A sticky render mode plus a script that crashed,
hot-reloaded, or was disabled mid-effect would leave the world permanently
inside a hallucination with no way out. Instead, a dead script means a healed
world. The cost is that you must call it every frame the effect is active:

```rhai
fn envelope(t) { max(1.0 - t / 4.0, 0.0) }   // your own: 1 at the start, 0 after 4 s

fn on_update() {
    let m = envelope(total_time());
    if m > 0.001 {
        set_render_mode(3, m);          // 3 is drunk
    }
    // stop calling -> the engine restores the world by itself
}
```

Sticky effects a mode borrows (FOV, radial blur, chromatic aberration) are
*not* covered by this, so zero them yourself when your effect ends. The
player's F4 menu has a **Freeze script post overrides** checkbox for the
moments you want to tune by hand while a script is still writing.

### Headless

```bash
flint render scene.toml --schemas schemas \
  --render-mode 4 --mode-mix 1.0 --mode-params 3,0,6,0
```

Since `flint render` runs no scripts, these flags **are** the fixture: there
is no script to drive the mode for you.

One practical note: modes 1 and 4 key off scene luminance, so they read best
in daylight even if your game schedules them at night. A night scene renders
the Matrix mode nearly black.

## Design Decisions

- **Rgba16Float** for the HDR buffer provides sufficient precision for bloom extraction without the memory cost of Rgba32Float
- **Progressive downsample/upsample** (rather than a single Gaussian blur) produces wide, natural-looking bloom cheaply
- **1x1 black fallback texture** when bloom is disabled avoids conditional bind group creation
- **Lazy allocation** --- Kuwahara, contour and FXAA pipelines and textures are created on the first frame they are enabled, however the config was set (scene load, a hot reload, the debug menu, a script), so the default path pays nothing for them
- **Gate-safe defaults** --- MSAA, FXAA and film grain default off, and grain time is pinned at 0 headlessly, so `flint render` output stays byte-stable for pixel-diff gates
- **Resize handling** --- `PostProcessResources` are recreated on window resize since the HDR texture and bloom mip chain are resolution-dependent
- **Extended shadow depth** --- the shadow frustum's depth range is extended beyond the camera frustum so off-screen casters (ceilings, walls behind the camera) are captured in the shadow map, which is critical for correct volumetric shafts in enclosed spaces

## Further Reading

- [Rendering](rendering.md) --- the PBR pipeline that feeds into post-processing
- [Lighting](lighting.md) --- the light component, shadows, and the clay-look shading levers
- [Headless Rendering](../guides/headless-rendering.md) --- using post-processing flags in CI
- [CLI Reference](../cli-reference/overview.md) --- full command options
- [File Formats](../formats/overview.md) --- the `[post_process]` scene block
