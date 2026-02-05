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

Flint ships with three built-in component schemas:

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

## Further Reading

- [Entities and ECS](entities-and-ecs.md) --- how schemas connect to the entity system
- [Constraints](constraints.md) --- rules that validate entities against schemas
- [Scenes](scenes.md) --- how schema-defined entities are serialized
