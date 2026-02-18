# File Formats

All Flint data formats use TOML. This page provides a complete reference for every file type.

## Scene Files (`.scene.toml`)

The primary data format. Each scene file contains metadata and a collection of named entities with their component data.

```toml
[scene]
name = "Scene Name"
version = "1.0"
input_config = "custom_input.toml"  # Optional input binding config

[entities.<name>]
archetype = "<archetype>"
parent = "<parent_name>"          # Optional parent entity

[entities.<name>.<component>]
field = value
```

Scenes may also include optional top-level blocks for post-processing and environment settings:

```toml
[post_process]
bloom_enabled = true
bloom_intensity = 0.04
bloom_threshold = 1.0
vignette_enabled = true
vignette_intensity = 0.3
exposure = 1.0

[environment]
ambient_color = [0.1, 0.1, 0.15]
ambient_intensity = 0.3
fog_enabled = false
fog_color = [0.5, 0.5, 0.6]
fog_density = 0.02
```

The `[post_process]` block configures the HDR post-processing pipeline (see [Post-Processing](../concepts/post-processing.md)). The `[environment]` block sets ambient lighting and fog parameters.

Scenes are loaded by `flint-scene` and can be edited with `flint entity create`, `flint entity delete`, or by hand. The `serve --watch` viewer reloads automatically when the file changes.

## Component Schemas (`schemas/components/*.toml`)

Define the fields, types, and defaults for each component kind. Components are dynamic --- they exist as schema TOML, not compiled Rust types.

```toml
[component.<name>]
description = "Human-readable description"

[component.<name>.fields]
field_name = { type = "<type>", default = <value>, description = "..." }
```

Supported field types: `bool`, `i32`, `f32`, `string`, `vec3`, `enum`, `entity_ref`, `array`.

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

## UI Layout Files (`ui/*.ui.toml`)

Define the structure of data-driven UI documents. Paired with a `.style.toml` file for styling.

```toml
style = "hud.style.toml"

[[elements]]
id = "score_panel"
type = "panel"
anchor = "top_right"
x = -20
y = 20

[[elements]]
id = "score_label"
type = "text"
parent = "score_panel"
class = "hud_text"
text = "Score: 0"
```

| Field | Type | Description |
|-------|------|-------------|
| `style` | string | Path to the companion `.style.toml` file |
| `elements[].id` | string | Unique element identifier (used by script API) |
| `elements[].type` | string | Element type: `panel`, `text`, `rect`, `circle`, `image` |
| `elements[].parent` | string | Parent element ID (for nesting) |
| `elements[].anchor` | string | Screen anchor: `top_left`, `top_center`, `top_right`, `center_left`, `center`, `center_right`, `bottom_left`, `bottom_center`, `bottom_right` |
| `elements[].class` | string | Style class name (defined in the `.style.toml`) |
| `elements[].text` | string | Initial text content (for `text` elements) |
| `elements[].x`, `y` | f32 | Offset from anchor position |

## UI Style Files (`ui/*.style.toml`)

Define named style classes referenced by UI layout elements.

```toml
[classes.hud_text]
font_size = 18
color = [1.0, 1.0, 1.0, 1.0]
text_align = "center"

[classes.panel_bg]
bg_color = [0.0, 0.0, 0.0, 0.5]
width = 200
height = 40
rounding = 4
thickness = 0
```

Supported style properties: `x`, `y`, `width`, `height`, `color`, `bg_color`, `font_size`, `rounding`, `layer`, `padding`, `opacity`, `text_align` (`"left"`, `"center"`, `"right"`), `layout` (`"vertical"`, `"horizontal"`), `thickness`.

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
