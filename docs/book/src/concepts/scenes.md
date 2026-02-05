# Scenes

A scene in Flint is a TOML file that describes a collection of entities and their data. Scenes are the primary unit of content --- they're what you load, save, query, validate, and render.

## File Format

Scene files use the `.scene.toml` extension and have two sections:

```toml
# Metadata
[scene]
name = "The Rusty Flint Tavern"
version = "1.0"
description = "A showcase scene demonstrating Flint engine capabilities"

# Entity definitions
[entities.main_hall]
archetype = "room"
# ...
```

### The `[scene]` Table

| Field | Required | Description |
|-------|----------|-------------|
| `name` | yes | Human-readable scene name |
| `version` | yes | Format version (currently "1.0") |
| `description` | no | Optional description |

### The `[entities.*]` Tables

Each entity is a table under `[entities]`, keyed by its unique name:

```toml
[entities.front_door]
archetype = "door"
parent = "main_hall"

[entities.front_door.transform]
position = [0, 0, 5]

[entities.front_door.door]
style = "hinged"
locked = false
open_angle = 90.0
```

**Top-level fields:**
- `archetype` --- the archetype schema name (optional but recommended)
- `parent` --- name of the parent entity (optional)

**Component tables** are nested under the entity. Each component name (e.g., `transform`, `door`, `bounds`) corresponds to a schema in `schemas/components/`.

## Scene Operations

### Creating a Scene

```bash
flint scene create levels/tavern.scene.toml --name "The Tavern"
```

### Listing Scenes

```bash
flint scene list
```

### Getting Scene Info

```bash
flint scene info levels/tavern.scene.toml
```

### Loading and Saving

The `flint-scene` crate handles serialization. Scenes are loaded into the ECS world as entities with dynamic components, and saved back to TOML with stable ordering.

When a scene is loaded:
1. The TOML is parsed into a scene structure
2. Each entity definition creates an ECS entity with a stable `EntityId`
3. Parent-child relationships are established
4. The entity ID counter is adjusted to be above any existing ID (preventing collisions on subsequent creates)

When a scene is saved:
1. All entities are serialized to their TOML representation
2. Component data is written as nested tables
3. Parent references use entity names (not internal IDs)

## Reload Behavior

Scene reload is a full re-parse. When `flint serve --watch` detects a file change:

1. The entire scene file is re-read and re-parsed
2. The old world state is replaced with the new one
3. The renderer picks up the new state on the next frame

This approach is simple and correct --- there's no incremental diffing that could get out of sync. For the scene sizes Flint targets, re-parsing is fast enough.

## Scene as Source of Truth

A key design decision: **the scene file is the source of truth**, not the in-memory state. This means:

- You can edit the file with any text editor
- AI agents can write TOML directly
- Git diffs show exactly what changed
- No hidden state lives only in memory

The CLI commands (`entity create`, `entity delete`) modify the scene file, and the in-memory world loads from that file. The viewer watches the file, not the internal state.

## Example: The Showcase Scene

The demo scene `demo/showcase.scene.toml` demonstrates the full format:

```toml
[scene]
name = "The Rusty Flint Tavern"
version = "1.0"
description = "A showcase scene demonstrating Flint engine capabilities"

# Rooms - rendered as blue wireframe boxes
[entities.main_hall]
archetype = "room"

[entities.main_hall.transform]
position = [0, 0, 0]

[entities.main_hall.bounds]
min = [-7, 0, -5]
max = [7, 4, 5]

# Doors - rendered as orange boxes
[entities.front_entrance]
archetype = "door"
parent = "main_hall"

[entities.front_entrance.transform]
position = [0, 0, 5]

[entities.front_entrance.door]
style = "hinged"
locked = false
open_angle = 90.0

# Furniture - rendered as green boxes
[entities.bar_counter]
archetype = "furniture"
parent = "main_hall"

[entities.bar_counter.transform]
position = [-4, 0, 0]

[entities.bar_counter.bounds]
min = [-1.5, 0, -3]
max = [0, 1.2, 3]

# Characters - rendered as yellow boxes
[entities.bartender]
archetype = "character"
parent = "main_hall"

[entities.bartender.transform]
position = [-5, 0, 0]
```

This scene defines 4 rooms, 4 doors, 9 pieces of furniture, and 6 characters --- all in readable, diffable TOML.

## Further Reading

- [Your First Scene](../getting-started/first-scene.md) --- hands-on guide to building a scene
- [Entities and ECS](entities-and-ecs.md) --- how scene entities map to the ECS
- [Schemas](schemas.md) --- how component structure is defined
- [Constraints](constraints.md) --- how to validate scenes
