# Schemas

Schemas define the structure of your game world. They specify what components exist, what fields they contain, and how they bundle together into archetypes. Schemas are TOML files stored in the `schemas/` directory of your project.

## Component Schemas

A component schema defines a reusable data type. Components live in `schemas/components/`:

```toml
# schemas/components/door.toml
[component.door]
description = "A door that can connect spaces"

[component.door.fields]
style = { type = "enum", values = ["hinged", "sliding", "rotating"], default = "hinged" }
locked = { type = "bool", default = false }
open_angle = { type = "f32", default = 90.0, min = 0.0, max = 180.0 }
```

### Field Types

| Type | Description | Example |
|------|-------------|---------|
| `bool` | Boolean | `true` / `false` |
| `i32` | 32-bit integer | `42` |
| `f32` | 32-bit float | `3.14` |
| `string` | Text string | `"hello"` |
| `vec3` | 3D vector (array of 3 floats) | `[1.0, 2.0, 3.0]` |
| `enum` | One of a set of string values | `"hinged"` |
| `entity_ref` | Reference to another entity by name | `"main_hall"` |

### Field Constraints

Fields can include validation constraints:

```toml
open_angle = { type = "f32", default = 90.0, min = 0.0, max = 180.0 }
required_key = { type = "entity_ref", optional = true }
```

- `default` --- value used when not explicitly set
- `min` / `max` --- numeric range bounds
- `optional` --- whether the field can be omitted (defaults to false)
- `values` --- valid options for enum types

## Built-in Components

Flint ships with several built-in component schemas:

### Transform

```toml
# schemas/components/transform.toml
[component.transform]
description = "Position and rotation in 3D space"

[component.transform.fields]
position = { type = "vec3", default = [0, 0, 0] }
rotation = { type = "vec3", default = [0, 0, 0] }
scale = { type = "vec3", default = [1, 1, 1] }
```

### Bounds

```toml
# schemas/components/bounds.toml
[component.bounds]
description = "Axis-aligned bounding box"

[component.bounds.fields]
min = { type = "vec3", default = [0, 0, 0] }
max = { type = "vec3", default = [10, 4, 10] }
```

### Door

```toml
# schemas/components/door.toml
[component.door]
description = "A door that can connect spaces"

[component.door.fields]
style = { type = "enum", values = ["hinged", "sliding", "rotating"], default = "hinged" }
locked = { type = "bool", default = false }
open_angle = { type = "f32", default = 90.0, min = 0.0, max = 180.0 }
```

### Material

```toml
# schemas/components/material.toml
[component.material]
description = "PBR material properties"

[component.material.fields]
texture = { type = "string", default = "", optional = true }
roughness = { type = "f32", default = 0.5, min = 0.0, max = 1.0 }
metallic = { type = "f32", default = 0.0, min = 0.0, max = 1.0 }
color = { type = "vec3", default = [1.0, 1.0, 1.0] }
emissive = { type = "vec3", default = [0.0, 0.0, 0.0] }
```

### Rigidbody

```toml
# schemas/components/rigidbody.toml
[component.rigidbody]
description = "Physics rigid body"

[component.rigidbody.fields]
body_type = { type = "enum", values = ["static", "dynamic", "kinematic"], default = "static" }
mass = { type = "f32", default = 1.0, min = 0.0 }
gravity_scale = { type = "f32", default = 1.0 }
```

### Collider

```toml
# schemas/components/collider.toml
[component.collider]
description = "Physics collision shape"

[component.collider.fields]
shape = { type = "enum", values = ["box", "sphere", "capsule"], default = "box" }
size = { type = "vec3", default = [1.0, 1.0, 1.0] }
friction = { type = "f32", default = 0.5, min = 0.0, max = 1.0 }
```

### Character Controller

```toml
# schemas/components/character_controller.toml
[component.character_controller]
description = "First-person character controller"

[component.character_controller.fields]
move_speed = { type = "f32", default = 5.0, min = 0.0 }
jump_force = { type = "f32", default = 7.0, min = 0.0 }
height = { type = "f32", default = 1.8, min = 0.1 }
radius = { type = "f32", default = 0.4, min = 0.1 }
camera_mode = { type = "enum", values = ["first_person", "orbit"], default = "first_person" }
```

### Sprite

```toml
# schemas/components/sprite.toml
[component.sprite]
description = "Billboard sprite rendered as a camera-facing quad"

[component.sprite.fields]
texture = { type = "string", default = "", description = "Sprite sheet texture name" }
width = { type = "f32", default = 1.0, min = 0.01, description = "World-space width" }
height = { type = "f32", default = 1.0, min = 0.01, description = "World-space height" }
frame = { type = "i32", default = 0, min = 0, description = "Current frame index" }
frames_x = { type = "i32", default = 1, min = 1, description = "Columns in sprite sheet" }
frames_y = { type = "i32", default = 1, min = 1, description = "Rows in sprite sheet" }
anchor_y = { type = "f32", default = 0.0, description = "Vertical anchor (0=bottom, 0.5=center)" }
fullbright = { type = "bool", default = true, description = "Bypass PBR lighting" }
visible = { type = "bool", default = true }
```

The sprite component is used for billboard sprites that always face the camera. See [Rendering: Billboard Sprites](rendering.md#billboard-sprites) for details on the rendering pipeline.

## Archetype Schemas

Archetypes bundle components together with defaults. They live in `schemas/archetypes/`:

```toml
# schemas/archetypes/room.toml
[archetype.room]
description = "A room or enclosed space"
components = ["transform", "bounds"]

[archetype.room.defaults.bounds]
min = [0, 0, 0]
max = [10, 4, 10]
```

The `components` array lists which component schemas an entity of this archetype requires. The `defaults` section provides values used when a component field isn't explicitly set.

### Built-in Archetypes

| Archetype | Components | Description |
|-----------|------------|-------------|
| `room` | transform, bounds | An enclosed space |
| `door` | transform, door | A door entity |
| `furniture` | transform, bounds | A piece of furniture |
| `character` | transform | A character or NPC |
| `wall` | transform, bounds, material | A wall surface |
| `floor` | transform, bounds, material | A floor surface |
| `ceiling` | transform, bounds, material | A ceiling surface |
| `pillar` | transform, bounds, material | A structural pillar |
| `player` | transform, character_controller, rigidbody, collider | Player-controlled entity |

## Introspecting Schemas

Use the CLI to inspect schema definitions:

```bash
# Show a component schema
flint schema door --schemas schemas

# Show an archetype schema
flint schema room --schemas schemas
```

This outputs the component fields, types, defaults, and constraints --- useful for both humans exploring the schema and AI agents discovering what fields are available.

## Creating Custom Schemas

To add a new component:

1. Create a file in `schemas/components/`:

```toml
# schemas/components/health.toml
[component.health]
description = "Hit points and damage tracking"

[component.health.fields]
max_hp = { type = "i32", default = 100, min = 1 }
current_hp = { type = "i32", default = 100, min = 0 }
armor = { type = "f32", default = 0.0, min = 0.0, max = 1.0 }
```

2. Reference it in an archetype:

```toml
# schemas/archetypes/enemy.toml
[archetype.enemy]
description = "A hostile NPC"
components = ["transform", "health"]

[archetype.enemy.defaults.health]
max_hp = 50
current_hp = 50
```

3. Use it in a scene:

```toml
[entities.goblin]
archetype = "enemy"

[entities.goblin.transform]
position = [10, 0, 5]

[entities.goblin.health]
max_hp = 30
current_hp = 30
armor = 0.1
```

No engine recompilation needed --- schemas are loaded at runtime from the TOML files.

## Game Project Schemas

Games can define their own schemas that extend or override the engine's built-in schemas. The `--schemas` flag accepts multiple paths, with later paths taking priority:

```bash
# From a game project root (engine at engine/)
cargo run --manifest-path engine/Cargo.toml --bin flint-player -- \
  scenes/arena.scene.toml \
  --schemas engine/schemas \
  --schemas schemas
```

In this example, `engine/schemas/` provides the engine's built-in components (transform, material, rigidbody, etc.) and the game's own `schemas/` adds game-specific components (health, weapon, enemy AI). If both directories define a component with the same name, the game's definition wins.

### Game Project Directory Structure

Game projects live in their own repositories with the engine included as a git subtree:

```
my_game/                    (standalone git repo)
├── engine/                 (git subtree ← Flint repo)
│   ├── crates/
│   ├── schemas/            (engine schemas)
│   └── Cargo.toml
├── schemas/
│   ├── components/
│   │   ├── health.toml
│   │   ├── weapon.toml
│   │   └── enemy_ai.toml
│   └── archetypes/
│       ├── enemy.toml
│       └── pickup.toml
├── scripts/
│   ├── enemy_ai.rhai
│   ├── weapon.rhai
│   └── hud.rhai
├── scenes/
│   └── level_1.scene.toml
├── sprites/
│   └── imp.png
└── audio/
    ├── shotgun.ogg
    └── imp_death.ogg
```

This separation keeps game-specific data out of the engine directory, allowing multiple games to share the same engine schemas while defining their own components and archetypes. See [Building a Game Project](../guides/building-a-game-project.md) for the full setup guide.

## Further Reading

- [Entities and ECS](entities-and-ecs.md) --- how schemas connect to the entity system
- [Constraints](constraints.md) --- rules that validate entities against schemas
- [Scenes](scenes.md) --- how schema-defined entities are serialized
- [Rendering](rendering.md) --- billboard sprite rendering pipeline
- [CLI Reference](../cli-reference/overview.md) --- multi-schema CLI usage
