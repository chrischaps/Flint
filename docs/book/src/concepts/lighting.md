# Lighting

Lights in Flint are ordinary entities with a `light` component. The renderer reads them every frame, sorts them by name so the shadow-casting sun is stable across reloads, and feeds them to the PBR shader alongside a set of scene-wide shading levers authored in the `[environment]` block. This page owns all of that: the light component, cascaded and contact-hardening shadows, area lights, and the "clay look" levers.

## The Light Component

```toml
[entities.sun.light]
type = "directional"
direction = [0.5, 1.0, 0.3]        # points toward the light
color = [1.0, 0.98, 0.95]
intensity = 3.0
angular_size = 0.5                 # degrees; drives PCSS penumbra

[entities.lantern.light]
type = "point"
color = [1.0, 0.7, 0.4]
intensity = 8.0
range = 12.0
source_radius = 0.15               # meters; softens the specular hotspot

[entities.spot.light]
type = "spot"
direction = [0.0, -1.0, 0.0]
color = [1.0, 1.0, 1.0]
intensity = 20.0
range = 15.0
inner_angle = 0.3                  # radians
outer_angle = 0.5
source_radius = 0.1
```

Point and spot lights sit at their entity's world position, so they follow transform hierarchies and animation like any other entity. Directional lights ignore position.

| Field | Applies to | Type | Default | Description |
|-------|-----------|------|---------|-------------|
| `type` | all | string | `"directional"` | `directional`, `point` or `spot` |
| `color` | all | [f32; 3] | `[1, 1, 1]` | Linear RGB |
| `intensity` | all | f32 | `1.0` | Radiance multiplier |
| `direction` | directional, spot | [f32; 3] | `[0, -1, 0]` | For directional lights this points **toward** the light; for spots it is the cone axis |
| `angular_size` | directional | f32 | `0.0` | Apparent size of the source in degrees (sun about 0.5, softbox 2--5). 0 keeps hard 3x3 PCF shadows ([ADR 0056](https://github.com/chrischaps/Flint/blob/main/docs/adr/0056-source-radius-area-lights.md)) |
| `range` (alias `radius`) | point, spot | f32 | `10.0` | Falloff distance in world units |
| `source_radius` | point, spot | f32 | `0.0` | Physical radius of the emitter in meters; 0 = punctual ([ADR 0056](https://github.com/chrischaps/Flint/blob/main/docs/adr/0056-source-radius-area-lights.md)) |
| `inner_angle` | spot | f32 | `0.3` | Full-intensity cone half-angle, radians |
| `outer_angle` | spot | f32 | `0.5` | Cone edge half-angle, radians |
| `volumetric_intensity` | directional | f32 | `0.0` | God-ray strength for this light; see [Post-Processing](post-processing.md#volumetric-lighting-god-rays) |
| `volumetric_color` | directional | [f32; 3] | light color | Tint of the shafts |

The same fields are the engine's `light` schema (`schemas/components/light.toml`), so `flint schema light` prints them from a terminal, with each fallback in its description. Only `type`, `color` and `intensity` carry a schema default: a default is written into the component at load, and `range` given one would beat an authored `radius`.

**A light is the `light` table and no other.** Nothing reads `[entities.sun.directional_light]` or `[entities.lamp.point_light]`: a scene whose only lights are spelled that way is lit by the built-in key and fill described below, and looks plausible enough that the mistake can live for a long time (about forty of the engine's own examples carried it until #681). To turn one into a real light, name the table `light`, add `type = "directional"`, and check `direction`: it points *toward* the light, so a sun written as the way its rays travel, `[-0.5, -0.6, -0.3]`, becomes `[0.5, 0.6, 0.3]`.

Limits: the shader takes a fixed number of directional, point and spot lights. Only one directional light casts cascaded shadows: the one with the highest `intensity`, with entity-name order breaking ties ([ADR 0045](https://github.com/chrischaps/Flint/blob/main/docs/adr/0045-deterministic-shadow-caster-order.md)), so a fill light can never steal the sun's shadows because of how it was named. If no scene light exists at all, the renderer falls back to a built-in warm key light and cool fill, unless the scene says `default_lights = false` in [`[environment]`](#the-environment-block).

## Area Lights

`source_radius` on point and spot lights, and `angular_size` on directional lights, turn punctual lights into small area sources ([ADR 0056](https://github.com/chrischaps/Flint/blob/main/docs/adr/0056-source-radius-area-lights.md)). The specular lobe uses the representative-point approximation: effective roughness widens by `source_radius / (2 * distance)` and shading distance never falls inside the source, so the hotspot on a glossy floor becomes a soft disc instead of a pinprick. Diffuse lighting is unaffected. A value of 0 takes the original code path exactly.

## Shadows

Directional lights cast shadows through cascaded shadow maps: several cascades cover increasing distance bands from the camera, giving crisp shadows nearby and broad coverage far away. Shadows can be disabled per launch with `--no-shadows`, toggled at runtime in the Rendering & Effects menu (F4), and turned off for a whole scene with `[environment] shadows = false` (below).

**A dark sun casts nothing.** When the shadow-casting directional light has `intensity = 0` (and no `volumetric_intensity`, whose god rays read the maps), or there is no directional light at all, the shadow pass is skipped with every caster in it (#627). Nothing it drew could have darkened a pixel, so the frame is byte-identical; the stats line's `in shadows` reads 0. The tavern with its sun at 0 went from 166,884 shadow triangles a frame to 0.

**Resolution.** `--shadow-resolution` defaults to 2048 texels per cascade. Before [ADR 0049](https://github.com/chrischaps/Flint/blob/main/docs/adr/0049-upload-shadow-texel-size-per-resolution.md) the shader hardcoded a 1/2048 texel size, so any other resolution filtered incorrectly and the flag was effectively decorative. The texel size is now uploaded with the cascade uniforms, so 512, 1024 and 4096 are real choices; the F4 menu's Shadows section rebuilds the shadow pass when you pick one.

**Contact hardening (PCSS).** When the shadow-casting directional light has a non-zero `angular_size`, the shader switches from a fixed 3x3 PCF kernel to percentage-closer soft shadows ([ADR 0057](https://github.com/chrischaps/Flint/blob/main/docs/adr/0057-pcss-contact-hardening-shadows.md)): a Vogel-disk blocker search estimates the average occluder depth, the penumbra width grows with the occluder-to-receiver distance and `tan(angular_size)`, and the filter kernel is sized to match. Shadows are sharp where an object touches the ground and soften as it lifts away. `angular_size = 0` takes the legacy PCF path verbatim, which keeps existing renders byte-identical.

**Skinned casters.** Skinned meshes cast shadows through a dedicated `vs_skinned_shadow` entry point that applies bone transforms before the depth write.

**Turning casting off.** Every mesh casts by default, transparent ones included: the shadow pass writes depth only, so a sheet at `opacity = 0.02` casts as dark a shadow as a wall. Set **`material.cast_shadows = false`** ([ADR 0100](https://github.com/chrischaps/Flint/blob/main/docs/adr/0100-entity-can-opt-out-of-shadows.md)) on anything that shouldn't, like mist, glass, spray or a smoke card:

```toml
[entities.valley_mist.material]
opacity = 0.3
cast_shadows = false
```

- **Where it applies:** imported, runtime, skinned and procedural (`bounds`) meshes, opaque or transparent. An entity with no value of its own takes its parent's, so a GLB expanded into child nodes stops casting as one.
- **It's read every frame**, so a script or extension can turn it on and off. It costs nothing: the draw just stays out of the cascades. The mesh still takes shadows from everything else.
- **Terrain and grass** don't read it: terrain chunks always cast, and grass casts into the two nearest cascades.

## The `[environment]` Block

Scene-wide shading lives in `[environment]`, next to the skybox path. Every lever here follows the same convention: absent or zero means the exact original shading, so scenes that never set them look the way they always did.

```toml
[environment]
skybox = "textures/dusk.hdr"
ambient_sky = [0.12, 0.13, 0.18]
ambient_ground = [0.06, 0.05, 0.04]
diffuse_wrap = 0.3
oren_nayar = 0.7
sheen_color = [1.0, 0.9, 0.8]
sheen_strength = 0.15
```

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `skybox` | string | none | Equirectangular panorama for the sky |
| `ambient_sky` | [f32; 3] | `[0.12, 0.13, 0.18]` | Hemisphere ambient, upper half (linear RGB) |
| `ambient_ground` | [f32; 3] | `[0.06, 0.05, 0.04]` | Hemisphere ambient, lower half |
| `diffuse_wrap` | f32 | `0.0` | Softens the diffuse terminator; 0.2--0.5 reads as matte or faintly subsurface |
| `oren_nayar` | f32 | `0.0` | Blend from Lambert toward the Fujii Oren-Nayar approximation, 0--1. Roughness supplies sigma; this only blends ([ADR 0048](https://github.com/chrischaps/Flint/blob/main/docs/adr/0048-clay-lighting-levers-oren-nayar-and-sheen.md)) |
| `sheen_color` | [f32; 3] | `[1, 1, 1]` | Charlie-sheen rim tint |
| `sheen_strength` | f32 | `0.0` | Rim strength. No energy compensation, so keep it at or below about 0.3 ([ADR 0048](https://github.com/chrischaps/Flint/blob/main/docs/adr/0048-clay-lighting-levers-oren-nayar-and-sheen.md)) |
| `shadows` | bool | `true` | `false`: the scene casts no shadows, whatever the quality tier or F4 say; they keep their own setting for the next scene (#627) |
| `default_lights` | bool | `true` | `false`: a scene with no light entities is lit by its ambient (and emission) alone, not the built-in key and fill (#627) |

Fog is **not** here; it lives in `[post_process]`.

### Unlit and ambient-only scenes

A scene lit by its ambient alone, or by nothing but what glows, needs no dummy light and no script (#627):

```toml
[environment]
shadows = false
default_lights = false
ambient_sky = [0.95, 0.9, 0.82]
ambient_ground = [0.32, 0.28, 0.3]

[entities.neon.material]
color = [1.0, 0.18, 0.55, 1.0]
unlit = true
emissive_scale = 3.0
```

**`material.unlit = true`** draws a mesh's base colour (texture and vertex colour included) times its `emissive_scale`, plus its emission, straight into the HDR target: no lights, ambient or shadows reach it, so unlit neon can stand among lit props, and above 1 it blooms. It is read every frame and inherited from the parent, on imported, skinned and procedural (`bounds`) meshes. It still casts a shadow unless `cast_shadows = false`, and post-processing (SSAO, fog, the grade) still applies, as it does to emission.

![An ambient-only scene with unlit bars](../images/unlit-ambient.png)

*`examples/unlit_ambient.scene.toml`: props lit by the ambient alone, no shadow pass, and three unlit bars, two of them blooming.*

How they combine in the shader: hemisphere ambient interpolates between the two colors by the surface normal's Y. Diffuse wrap replaces the raw `n·l` term; Oren-Nayar scales the diffuse magnitude; the Charlie sheen adds a rim lobe on top. The three are orthogonal, and when all are zero the shader takes the original code path. The wrap and Oren-Nayar values ride the `w` components of the ambient uniforms encoded as `1 + value`, which is why a stale or default uniform (w = 0) still means "off".

### The clay look

[ADRs 0042](https://github.com/chrischaps/Flint/blob/main/docs/adr/0042-clay-look-research-spike.md) through 0052 grew these levers to give a scene a soft, sculpted, matte "clay" reading without touching materials. A recipe that works on the tavern showcase:

![Clay levers off](../images/clay-off.png)

*The tavern with default shading: Lambert diffuse, sharp terminator, neutral grade.*

![Clay levers on](../images/clay-on.png)

*The same frame with Oren-Nayar 0.7, sheen 0.15, and a warm grade gain.*

```toml
[environment]
diffuse_wrap = 0.25
oren_nayar = 0.7
sheen_color = [1.0, 0.9, 0.8]
sheen_strength = 0.15

[post_process]
ssao_samples = 16
grade_lift = [0.03, 0.02, 0.015]
grade_gain = [1.04, 1.0, 0.94]
film_grain = 0.02
```

The same levers are available headlessly, and CLI values win over the scene block:

```bash
flint render scene.toml --oren-nayar 0.7 --sheen-strength 0.15 --sheen-color 1,0.9,0.8 \
    --grade-gain 1.04,1,0.94 --ssao-samples 16
```

## Runtime Control

The Rendering & Effects menu's **Lighting** section exposes all six `[environment]` levers with a Reset button, plus shadow enable and resolution under **Shadows**. In code, `SceneRenderer` offers `set_ambient` / `reset_ambient`, `set_diffuse_wrap`, `set_oren_nayar`, `set_sheen`, `set_shadow_resolution`, and `lighting_levers()` to read the current state back for seeding UI.

Scripts do not currently have bindings for these levers; they are a per-scene look, not a per-frame effect. For per-frame mood changes use the sticky post-process overrides on the [Post-Processing](post-processing.md#from-scripts) page.

## Further Reading

- [Rendering](rendering.md) --- the PBR pipeline these lights feed
- [Post-Processing](post-processing.md) --- volumetric shafts, grade and grain
- [Sky](sky.md) --- the procedural sky, which carries its own `ambient_sky` / `ambient_ground` on the `sky` component, distinct from the `[environment]` keys here
- [File Formats](../formats/overview.md) --- the `[environment]` block reference
