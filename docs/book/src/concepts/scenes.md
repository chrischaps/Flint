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
| `input_config` | no | Path or name of an input config file for this scene (see [Input System](physics-and-runtime.md#input-system)) |

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

## Prefabs

Prefabs are reusable entity group templates that reduce scene file duplication. A prefab defines a set of entities in a `.prefab.toml` file, and scenes instantiate them with variable substitution and optional overrides.

### Defining a Prefab

Prefab files live in the `prefabs/` directory and follow the same entity format as scenes, with a `[prefab]` metadata header:

```toml
[prefab]
name = "kart"
description = "Racing kart with body, wheels, and driver"

[entities.kart]

[entities.kart.transform]
position = [0, 0, 0]

[entities.kart.model]
asset = "kart_body"

[entities.wheel_fl]
parent = "${PREFIX}_kart"

[entities.wheel_fl.transform]
position = [-0.4, 0.15, 0.55]

[entities.wheel_fl.model]
asset = "kart_wheel"
```

All string values containing `${PREFIX}` are substituted with the instance prefix at load time. Entity names are automatically prefixed (e.g., `kart` becomes `player_kart` when the prefix is `"player"`).

### Using Prefabs in a Scene

Scenes reference prefabs in a `[prefabs]` section:

```toml
[prefabs.player]
template = "kart"
prefix = "player"

[prefabs.player.overrides.kart.transform]
position = [0, 0, 0]

[prefabs.ai1]
template = "kart"
prefix = "ai1"

[prefabs.ai1.overrides.kart.transform]
position = [3, 0, -5]
```

Each prefab instance specifies:
- **`template`** --- the prefab name (matches the `.prefab.toml` filename without extension)
- **`prefix`** --- substituted for `${PREFIX}` in all string values and prepended to entity names
- **`overrides`** --- per-entity component field overrides (deep-merged with the template)

### Override Deep Merge

Overrides are merged at the field level, not the component level. If a prefab template defines a component with five fields and an override specifies one field, only that one field is replaced --- the other four are preserved from the template.

### Path Resolution

The loader searches for prefab templates in:
1. `<scene_directory>/prefabs/`
2. `<scene_directory>/../prefabs/`

This means a `prefabs/` directory at the project root is found when loading scenes from `scenes/`.

### Previewing Prefabs

Use the CLI to visually inspect a prefab template:

```bash
flint prefab view prefabs/kart.prefab.toml --schemas schemas
```

## Splines

Splines define smooth paths through 3D space using Catmull-Rom interpolation. They're used for track layouts, camera paths, and procedural geometry generation.

### Spline Component

Attach a spline to an entity with the `spline` component:

```toml
[entities.track_path]

[entities.track_path.spline]
source = "oval_plus.spline.toml"
```

The engine loads the `.spline.toml` file, samples it into a dense point array stored as the `spline_data` ECS component, and makes it available for script queries via the [Spline API](scripting.md#spline-api).

### Spline Meshes

The `spline_mesh` component generates geometry by sweeping a rectangular cross-section along a spline:

```toml
[entities.road_surface]

[entities.road_surface.spline_mesh]
spline = "track_path"
width = 12.0
height = 0.3
offset_y = -0.15

[entities.road_surface.material]
base_color = [0.3, 0.3, 0.3]
roughness = 0.8
```

One spline can feed multiple mesh entities (road surface, walls, guardrails) with different cross-section dimensions and materials.

## Further Reading

- [Your First Scene](../getting-started/first-scene.md) --- hands-on guide to building a scene
- [Entities and ECS](entities-and-ecs.md) --- how scene entities map to the ECS
- [Schemas](schemas.md) --- how component structure is defined
- [Constraints](constraints.md) --- how to validate scenes
- [File Formats](../formats/overview.md) --- prefab and spline file format details
