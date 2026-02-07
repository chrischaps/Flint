# File Formats

All Flint data formats use TOML. This page provides a complete reference for every file type.

## Scene Files (`.scene.toml`)

The primary data format. Each scene file contains metadata and a collection of named entities with their component data.

```toml
[scene]
name = "Scene Name"
version = "1.0"

[entities.<name>]
archetype = "<archetype>"
parent = "<parent_name>"          # Optional parent entity

[entities.<name>.<component>]
field = value
```

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

## Rhai Scripts (`scripts/*.rhai`)

Game logic scripts written in [Rhai](https://rhai.rs/). Attached to entities via the `script` component. See [Scripting](../concepts/scripting.md) for the full API reference.

```rust
fn on_init() {
    log("Entity initialized");
}

fn on_update(dt) {
    // Called every frame with delta time
}

fn on_interact() {
    // Called when the player interacts with this entity
    play_sound("door_open");
}
```

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
