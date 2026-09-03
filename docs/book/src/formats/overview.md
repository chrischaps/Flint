# File Formats

All Flint data formats use TOML (session recordings are JSON Lines). This page is the reference for every file type the engine reads or writes. Procgen specs (`.procgen.toml`) and terrain files (`.terrain.toml`) are covered on their own pages: [Procedural Generation](../concepts/procgen.md) and [Terrain](../concepts/terrain.md).

## Scene Files (`.scene.toml`)

The primary data format. Each scene file contains metadata and a collection of named entities with their component data.

```toml
[scene]
name = "Scene Name"
version = "1.0"
description = "Optional one-line description"
input_config = "custom_input.toml"  # Optional input binding config
preload_audio = true                # Optional; false skips the blanket audio/ preload

[camera]                            # Optional authored framing
position = [0, 4, 12]
target = [0, 1, 0]

[entities.<name>]
archetype = "<archetype>"
parent = "<parent_name>"          # Optional parent entity

[entities.<name>.<component>]
field = value
```

| `[scene]` key | Type | Default | Description |
|---------------|------|---------|-------------|
| `name` | string | (required) | Human-readable scene name |
| `version` | string | `"1.0"` | Format version |
| `description` | string | (none) | Free-text description |
| `input_config` | string | (none) | Game-level input binding overlay (see [Input Configuration](#input-configuration-configinputtoml-flintinput_game_idtoml)) |
| `preload_audio` | bool | `true` | When `false`, the player skips preloading every file under `audio/` at scene load so a scene with a large audio folder starts instantly. Sounds named by `audio_source` components and music-session stems still load through their own paths; this only gates the convenience preload for script-triggered sounds. |

Scenes may also include optional top-level `[camera]`, `[environment]` and `[post_process]` blocks, and a `[prefabs]` section (see [Prefab Templates](#prefab-templates-prefabsprefabtoml)).

### `[camera]`

The authored framing. `flint render` starts from it when no camera flags are given, and the scene viewer seeds its orbit camera from it (**Space** returns to it). Absent, both fall back to an automatic framing.

```toml
[camera]
projection = "perspective"   # or "orthographic"
position = [0, 4, 12]
target = [0, 1, 0]
fov = 60.0                   # perspective only
near = 0.1
far = 500.0
# ortho_height = 10.0        # orthographic only: half-height in world units
```

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `projection` | string | `"perspective"` | `"perspective"` or `"orthographic"` |
| `ortho_height` | f32 | `0` | Orthographic half-height in world units (orthographic only) |
| `position` | `[f32; 3]` | (auto) | Camera position |
| `target` | `[f32; 3]` | (auto) | Look-at point |
| `fov` | f32 | (renderer default) | Vertical field of view in degrees (perspective only) |
| `near` | f32 | (renderer default) | Near clipping plane |
| `far` | f32 | (renderer default) | Far clipping plane |

### `[environment]`

Skybox and the scene-wide shading levers. Every lever is optional and its absence means "exactly the legacy shading" (see [Lighting](../concepts/lighting.md)). Fog is **not** here; it lives in `[post_process]`.

```toml
[environment]
skybox = "textures/dusk_panorama.png"
ambient_sky = [0.35, 0.40, 0.50]
ambient_ground = [0.18, 0.14, 0.10]
diffuse_wrap = 0.3
oren_nayar = 0.7
sheen_color = [1.0, 0.9, 0.8]
sheen_strength = 0.15
```

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `skybox` | string | (none) | Equirectangular panorama image for the skybox |
| `ambient_sky` | `[f32; 3]` | renderer default | Hemisphere ambient colour from above (linear) |
| `ambient_ground` | `[f32; 3]` | renderer default | Hemisphere ambient colour from below (linear) |
| `diffuse_wrap` | f32 | `0` | Diffuse terminator wrap; `0` = physically sharp, `0.2`–`0.5` = soft matte |
| `oren_nayar` | f32 | `0` | Blend from Lambert toward Oren-Nayar diffuse (0–1); sigma comes from material roughness |
| `sheen_color` | `[f32; 3]` | `[1, 1, 1]` | Charlie-sheen rim tint (linear); only matters with a non-zero strength |
| `sheen_strength` | f32 | `0` | Charlie-sheen rim strength; keep at or below about `0.3` |

### `[post_process]`

Configures the HDR post-processing pipeline. Every key is optional. See [Post-Processing](../concepts/post-processing.md) for what each effect does and the matching `flint render` flags.

```toml
[post_process]
bloom_enabled = true
bloom_intensity = 0.04
ssao_samples = 16
dof_strength = 0.5
dof_focus_distance = 8.0
film_grain = 0.03
grade_gain = [1.04, 1.0, 0.94]
fxaa = false
```

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `bloom_enabled` | bool | `true` | Bloom pass |
| `bloom_intensity` | f32 | `0.04` | Bloom strength |
| `bloom_threshold` | f32 | `1.0` | Brightness above which pixels bloom |
| `vignette_enabled` | bool | `false` | Edge darkening |
| `vignette_intensity` | f32 | `0.3` | Vignette strength |
| `vignette_smoothness` | f32 | `2.0` | Vignette falloff curve |
| `exposure` | f32 | `1.0` | Exposure multiplier before tone mapping |
| `ssao_enabled` | bool | `true` | Screen-space ambient occlusion |
| `ssao_radius` | f32 | `0.5` | SSAO sample radius (world units) |
| `ssao_intensity` | f32 | `1.0` | SSAO darkening strength |
| `ssao_samples` | u32 | `64` | Hemisphere samples per pixel, 1–64. The heaviest per-pixel cost in the stack; `16` is ~4x cheaper and usually indistinguishable on matte scenes |
| `fog_enabled` | bool | `false` | Distance fog |
| `fog_color` | `[f32; 3]` | `[0.7, 0.75, 0.82]` | Fog colour |
| `fog_density` | f32 | `0.02` | Exponential fog density |
| `fog_start` | f32 | `5.0` | Distance at which fog begins |
| `fog_end` | f32 | `100.0` | Distance at which fog saturates |
| `fog_height_enabled` | bool | `false` | Height-based fog |
| `fog_height_falloff` | f32 | `0.1` | How quickly height fog thins with altitude |
| `fog_height_origin` | f32 | `0.0` | World Y where height fog is densest |
| `dither_enabled` | bool | `false` | Ordered dither |
| `dither_intensity` | f32 | `0.03` | Dither strength |
| `volumetric_enabled` | bool | `false` | Volumetric light (god rays) |
| `volumetric_samples` | u32 | `32` | Ray-march steps |
| `volumetric_density` | f32 | `1.0` | Scattering density |
| `volumetric_max_distance` | f32 | `100.0` | Ray-march cut-off |
| `volumetric_decay` | f32 | `0.98` | Per-step energy decay |
| `chromatic_aberration` | f32 | `0` | Colour-fringe amount at the frame edge |
| `radial_blur` | f32 | `0` | Zoom-blur amount from the frame centre |
| `desaturate` | f32 | `0` | Drain toward ash-grey; `0` = full colour, `1` = fully drained |
| `dof_strength` | f32 | `0` | Depth-of-field defocus; `0` = sharp |
| `dof_focus_distance` | f32 | `10.0` | Focus plane distance in view metres |
| `dof_focus_range` | f32 | `5.0` | Half-width of the in-focus band in view metres |
| `kuwahara_enabled` | bool | `false` | Anisotropic Kuwahara (painterly) pre-pass |
| `kuwahara_radius` | u32 | `4` | Filter radius in pixels |
| `kuwahara_sharpness` | f32 | `8.0` | Sector weighting sharpness |
| `kuwahara_hardness` | f32 | `8.0` | Sector edge hardness |
| `kuwahara_anisotropy` | f32 | `1.0` | `0` = isotropic, `1` = fully anisotropic |
| `film_grain` | f32 | `0` | Animated grain; `0.02`–`0.05` is subtle |
| `grade_lift` | `[f32; 3]` | `[0, 0, 0]` | Per-channel add after ACES tone mapping |
| `grade_gamma` | `[f32; 3]` | `[1, 1, 1]` | Per-channel midtone curve |
| `grade_gain` | `[f32; 3]` | `[1, 1, 1]` | Per-channel multiply |
| `fxaa` | bool | `false` | FXAA pass on the final composite. Off by default so headless pixel-diff gates stay single-path |

Scenes are loaded by `flint-scene` and can be edited with `flint entity create`, `flint entity delete`, or by hand. At load, each authored component is validated against its schema (warnings only) and any schema field with a `default` that the entity did not set is filled in (see [Scenes](../concepts/scenes.md)). The `flint edit --watch` viewer reloads automatically when the file changes.

## Component Schemas (`schemas/components/*.toml`)

Define the fields, types, and defaults for each component kind. Components are dynamic --- they exist as schema TOML, not compiled Rust types.

```toml
[component.<name>]
description = "Human-readable description"

[component.<name>.fields]
field_name = { type = "<type>", default = <value>, description = "..." }
```

Supported field types: `bool`, `i32`, `i64`, `f32`, `f64`, `string`, `vec2`, `vec3`, `vec4`, `color`, `transform`, `enum`, `entity_ref`, `array`. `vec2` and `vec4` are validated as float arrays. Schema `default` values are applied at scene load for every listed component that omits the field (see [Schemas](../concepts/schemas.md)).

Key component schemas: `transform`, `material`, `door`, `bounds`, `rigidbody`, `collider`, `character_controller`, `audio_source`, `audio_listener`, `audio_trigger`, `animator`, `skeleton`, `script`, `interactable`, `sprite`, `asset_def`.

## Archetype Schemas (`schemas/archetypes/*.toml`)

Bundle components together with sensible defaults for common entity types.

```toml
[archetype.<name>]
description = "..."
components = ["comp1", "comp2"]

[archetype.<name>.defaults.<component>]
field = value
```

## Constraint Files (`schemas/constraints/*.toml`)

Declarative validation rules checked by `flint validate`. Each file can contain multiple `[[constraint]]` entries.

```toml
[[constraint]]
name = "rule_name"
description = "What this constraint checks"
query = "entities where archetype == 'door'"
severity = "error"                 # "error" or "warning"
message = "Door '{name}' is missing a transform component"

[constraint.kind]
type = "required_component"        # Constraint type
archetype = "door"
component = "transform"
```

Constraint kinds: `required_component`, `required_child`, `value_range`, `reference_valid`, `query_rule`.

## Animation Clips (`animations/*.anim.toml`)

TOML-defined keyframe animation clips for property tweens. Loaded by scanning the animations directory at startup.

```toml
name = "clip_name"
duration = 0.8

[[tracks]]
interpolation = "Linear"           # "Step", "Linear", or "CubicSpline"

[tracks.target]
type = "Rotation"                  # "Position", "Rotation", "Scale", or "CustomFloat"
# component = "material"           # Required for CustomFloat
# field = "emissive_strength"      # Required for CustomFloat

[[tracks.keyframes]]
time = 0.0
value = [0.0, 0.0, 0.0]           # [x, y, z] (euler degrees for rotation)

[[tracks.keyframes]]
time = 0.8
value = [0.0, 90.0, 0.0]
# in_tangent = [...]               # Optional, for CubicSpline
# out_tangent = [...]

[[events]]                         # Optional timed events
time = 0.0
event_name = "door_start"
```

`CubicSpline` tracks read `in_tangent` / `out_tangent` on each keyframe; the glTF importer fills them from `CUBICSPLINE` samplers.

## Animation Sequences (`animations/*.sequence.toml`)

An ordered list of timestamped animator events: crossfade the base clip, set a layer, change speed, or raise a named cue for the entity's script. The player loads every sequence in the scene's `animations/` directory; scripts start one with `play_sequence(entity, name)`, and the model previewer plays one with `flint edit model.glb --sequence <file>`. See [Animation: Sequences](../concepts/animation.md#sequences).

```toml
name = "intro_bow"
loop = false                       # Optional; default false
# duration = 6.0                   # Optional; default = last event time + its transition

[[events]]
time = 0.0
kind = "blend"                     # blend | layer | speed | cue
clip = "walk"
duration = 0.3                     # Crossfade seconds; 0 = hard cut

[[events]]
time = 1.0
kind = "layer"
index = 0                          # Layer slot (0..254)
clip = "wave"                      # Omitted fields keep their current value
weight = 1.0
fade = 0.25                        # Ramp the weight over this many seconds
mode = "additive"                  # additive | override
mask = "spine"                     # Root joint of the affected subtree

[[events]]
time = 2.5
kind = "speed"
value = 0.5

[[events]]
time = 4.0
kind = "cue"
name = "done"                      # Delivered to on_sequence_cue(sequence, cue)
```

| Key | Type | Description |
|-----|------|-------------|
| `name` | string | Sequence name, as used by `play_sequence` |
| `loop` | bool | Wrap at `duration` and fire events again from `t = 0` |
| `duration` | f64 | Explicit length in seconds. A looping sequence whose resolved duration is `0` is a load error |
| `events[].time` | f64 | Seconds from the start |
| `events[].kind` | string | `blend` (`clip`, `duration`), `layer` (`index`, `clip`, `weight`, `fade`, `mode`, `mask`), `speed` (`value`), `cue` (`name`) |

## Asset Sidecars (`assets/**/*.asset.toml`)

Metadata files stored alongside imported assets in the catalog.

```toml
[asset]
name = "asset_name"
type = "mesh"                      # mesh, texture, material, audio, script
hash = "sha256:a1b2c3..."
source_path = "models/chair.glb"
format = "glb"
tags = ["furniture", "medieval"]

[asset.properties]                 # Optional provider-specific metadata
prompt = "wooden tavern chair"
provider = "meshy"
```

## Style Guides (`styles/*.style.toml`)

Define visual vocabulary for consistent AI asset generation. Searched in `styles/` then `.flint/styles/`.

```toml
[style]
name = "medieval_tavern"
description = "Weathered medieval fantasy tavern"
prompt_prefix = "Medieval fantasy tavern style, low-fantasy realism"
prompt_suffix = "Photorealistic textures, warm candlelight tones"
negative_prompt = "modern, sci-fi, neon, plastic"
palette = ["#8B4513", "#A0522D", "#D4A574", "#4A4A4A"]

[style.materials]
roughness_range = [0.6, 0.95]
metallic_range = [0.0, 0.15]
preferred_materials = ["aged oak wood", "rough-hewn stone", "hammered wrought iron"]

[style.geometry]
max_triangles = 5000
require_uvs = true
require_normals = true
```

## Semantic Asset Definitions (`schemas/components/asset_def.toml`)

The `asset_def` component schema describes what an entity needs in terms of assets, expressed as intent. Used by the batch resolver to auto-generate missing assets.

```toml
[entities.tavern_wall.asset_def]
name = "tavern_wall_texture"
description = "Rough stone wall with mortar lines"
type = "texture"
material_intent = "rough stone"
wear_level = 0.7
size_class = "large"
tags = ["wall", "interior"]
```

## Prefab Templates (`prefabs/*.prefab.toml`)

Reusable entity group templates with variable substitution. Prefabs define a set of entities that can be instantiated multiple times in a scene with different prefixes and per-instance overrides.

```toml
[prefab]
name = "template_name"
description = "Optional description"

[entities.body]

[entities.body.transform]
position = [0, 0, 0]

[entities.body.model]
asset = "model_name"

[entities.child]
parent = "${PREFIX}_body"

[entities.child.transform]
position = [0.5, 0, 0]
```

All string values containing `${PREFIX}` are replaced with the instance prefix. Entity names are prepended with the prefix (e.g., `body` becomes `player_body` with prefix `"player"`).

Scenes instantiate prefabs in a `[prefabs]` section:

```toml
[prefabs.player]
template = "template_name"
prefix = "player"

[prefabs.player.overrides.body.transform]
position = [0, 0, 0]

[prefabs.ai1]
template = "template_name"
prefix = "ai1"

[prefabs.ai1.overrides.body.transform]
position = [5, 0, -3]
```

Overrides are deep-merged at the field level --- specifying one field in a component preserves all other fields from the template.

See [Scenes: Prefabs](../concepts/scenes.md#prefabs) for usage details.

## Spline Files (`splines/*.spline.toml`)

Define smooth 3D paths using Catmull-Rom control points. Used for track layouts, camera paths, and procedural geometry generation.

```toml
[spline]
name = "Track Name"
closed = true             # true for closed loops, false for open paths

[sampling]
spacing = 2.0             # Distance between sampled points (meters)

[[control_points]]
position = [0, 0, 0]
twist = 0.0               # Banking angle in degrees

[[control_points]]
position = [0, 0, -50]
twist = 0.0

[[control_points]]
position = [50, 0, -100]
twist = 5.0               # Banked turn
```

| Field | Type | Description |
|-------|------|-------------|
| `spline.name` | string | Human-readable name |
| `spline.closed` | bool | Whether the spline forms a closed loop |
| `sampling.spacing` | f32 | Distance between sampled points along the curve |
| `control_points[].position` | `[f32; 3]` | 3D position `[x, y, z]` |
| `control_points[].twist` | f32 | Banking angle in degrees (interpolated with C1 continuity via Catmull-Rom) |

The engine samples the control points into a dense array using Catmull-Rom interpolation, stored as the `spline_data` ECS component. Scripts can query this data via `spline_closest_point()` and `spline_sample_at()`.

## UI Layouts (`ui/*.ui.toml`)

Data-driven UI element trees. Loaded by scripts via `load_ui()`. Each layout file references a companion style file.

```toml
[ui]
name = "Race HUD"
style = "ui/race_hud.style.toml"

[elements.<id>]
type = "panel"                     # panel, text, rect, circle, image
anchor = "bottom-center"           # Screen anchor (root elements only)
class = "hud-panel"                # Style class from .style.toml
parent = "parent_id"               # Optional parent element
text = "Default text"              # For text elements
src = "logo.png"                   # For image elements
visible = true
```

Element types: `panel` (container with background), `text` (styled text), `rect` (filled or outlined rectangle), `circle` (filled circle), `image` (sprite).

Anchor points: `top-left`, `top-center`, `top-right`, `center-left`, `center`, `center-right`, `bottom-left`, `bottom-center`, `bottom-right`.

See [Scripting: Data-Driven UI](../concepts/scripting.md#data-driven-ui-system) for the full layout/style/API reference.

## UI Styles (`ui/*.style.toml`)

Named style classes for UI elements. Referenced by `.ui.toml` layout files.

```toml
[styles.<class-name>]
width = 200
height = 60
color = [1.0, 1.0, 1.0, 1.0]     # Primary color (RGBA)
bg_color = [0.0, 0.0, 0.0, 0.6]  # Background color
font_size = 24
text_align = "center"              # left, center, right
rounding = 8
opacity = 1.0
padding = [12, 8, 12, 8]          # [left, top, right, bottom]
layout = "stack"                   # stack (vertical) or horizontal
layer = 0                          # Render depth ordering
width_pct = 100                    # Percentage of parent width
margin_bottom = 4                  # Spacing in flow layout
```

Style properties support float, color (`[r,g,b,a]`), string, and boolean values. See [Scripting: Style Properties](../concepts/scripting.md#file-format-styletoml) for the complete property table.

## Rhai Scripts (`scripts/*.rhai`)

Game logic scripts written in [Rhai](https://rhai.rs/). Attached to entities via the `script` component. See [Scripting](../concepts/scripting.md) for the full API reference.

```rust
fn on_init() {
    log("Entity initialized");
}

fn on_update() {
    let dt = delta_time();
    // Called every frame — use delta_time() for frame delta
}

fn on_interact() {
    // Called when the player interacts with this entity
    play_sound("door_open");
}
```

## Input Configuration (`config/input.toml`, `~/.flint/input_{game_id}.toml`)

Define action-to-binding mappings for keyboard, mouse, and gamepad input. Loaded with layered precedence: engine defaults → game config → user overrides → CLI override.

```toml
version = 1
game_id = "doom_fps"

[actions.move_forward]
kind = "button"
[[actions.move_forward.bindings]]
type = "key"
code = "KeyW"
[[actions.move_forward.bindings]]
type = "gamepad_axis"
axis = "LeftStickY"
direction = "negative"
threshold = 0.35
gamepad = "any"

[actions.fire]
kind = "button"
[[actions.fire.bindings]]
type = "mouse_button"
button = "Left"
[[actions.fire.bindings]]
type = "gamepad_button"
button = "RightTrigger"
gamepad = "any"

[actions.look_x]
kind = "axis1d"
[[actions.look_x.bindings]]
type = "mouse_delta"
axis = "x"
scale = 2.0
[[actions.look_x.bindings]]
type = "gamepad_axis"
axis = "RightStickX"
deadzone = 0.15
scale = 1.0
invert = false
gamepad = "any"
```

Binding types: `key`, `mouse_button`, `mouse_delta`, `mouse_wheel`, `gamepad_button`, `gamepad_axis`. Action kinds: `button` (discrete), `axis1d` (analog). Gamepad selector: `"any"` or a numeric index. User overrides are written automatically when bindings are remapped at runtime.

## Music Session Files

The rhythm system ([Music Sessions](../concepts/music-sessions.md)) has its own family of files. All carry `schema_version = 0`. The full grammar lives with the `flint-music` crate; this is the map.

| File | Purpose |
|------|---------|
| `*.suite.toml` | Suite manifest: `[suite]` id and title, `[audio] sample_rate`, `[[tempo]]` anchors (`sample`, `bpm`, `time_signature`), `[[sections]]` (`name`, `start_sample`, `pulse_window_ms`), `[reintegration]` (`re_entry_sections`, `lead_bus`, `reassembly_bars`), and one `[buses.<name>]` per fixed bus (`foundation`, `harmony`, `world_voice`, `home_theme`, `child_motif`, `texture`) with `file` or `silent = true`. Optional `[[degraded_alternates]]`. Checked by `flint validate-suite`. |
| `*.chart.toml` | Beatmap: `suite` id, `[[curves]]` (`channel` ∈ `lean`, `sway`, `pressure_l`, `pressure_r`; `beat`; `value`; `interp` ∈ `linear`, `hold`, `smooth`), `[[pulses]]` (`beat`, `kind` ∈ `pulse`, `press`, `flick`, optional `window_ms`, `strength`, `direction`), `[[cues]]` (`beat`, `cue`, optional `params`), `[[intensity]]` (`beat`, `value`). |
| `*.events.toml` | Offline event script for `flint render-suite`: `[[events]]` with `at = "bar:N"` or `"beat:N"`, `bus`, `action` ∈ `set_gain` (`db`, `ramp_ms`), `set_lpf` (`hz`, `ramp_ms`), `set_detune` (`semitones`, `ramp_ms`), `marker` (`label`). |
| `*.session.jsonl` | Recorded input session (`flint play-chart --record`): one JSON object per line, a `header` (suite, chart, sample rate, latency and calibration offsets, config snapshots) followed by `lean` (`sample`, `x`, `y`) and `pulse` (`sample`, `kind`) events stamped in suite samples. Replayed by `flint replay-chart --session`. |
| `config/coherence.toml` | Coherence integrator tuning (`--config`) |
| `config/ladder.toml` | Disintegration ladder: rungs, hysteresis, per-rung audio and visual params, and the `[seam]` table (`lead_in_beats` 0–8, default 0) (`--ladder`) |
| `config/gradient.toml` | Error-driven audio gradient (`--gradient`) |
| `config/haptics.toml` | Rumble entrainment (`--haptics`) |
| `logs/latency/calibration-*.toml` | Written by `flint calibrate`; the median tap offset feeds later sessions. `flint spike-rumble` writes its timing report beside it |
| `logs/sessions/*.session.jsonl`, `logs/judgment/*.jsonl` | Default locations for recorded sessions and replay judgment logs |

## Configuration (`~/.flint/config.toml`, `.flint/config.toml`)

Layered configuration for API keys and generation settings. Global config is merged with project-level config; environment variables override both.

```toml
[providers.flux]
api_key = "your-api-key"
enabled = true

[providers.meshy]
api_key = "your-api-key"
enabled = true

[providers.elevenlabs]
api_key = "your-api-key"
enabled = true

[generation]
default_style = "medieval_tavern"
```

Environment variable overrides: `FLINT_FLUX_API_KEY`, `FLINT_MESHY_API_KEY`, `FLINT_ELEVENLABS_API_KEY`.
