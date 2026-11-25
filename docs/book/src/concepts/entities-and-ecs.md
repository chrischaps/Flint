# Entities and ECS

> **Package:** `engine`, the kernel: every game has it ([Packages](packages.md)).

Flint uses an Entity-Component-System (ECS) architecture, with a world of its own (`flint-ecs`). This page explains how entities, components, and IDs work in Flint.

## What Is ECS?

In an ECS architecture:
- **Entities** are unique identifiers (not objects with methods)
- **Components** are pure data attached to entities
- **Systems** are logic that operates on entities with specific component combinations

Flint's twist: components are **dynamic**. Instead of being Rust structs compiled into the engine, they're defined at runtime as TOML schema files and stored as `toml::Value`. This means you can define new component types without recompiling the engine.

## Entity IDs

Every entity gets a stable `EntityId` --- a 64-bit integer that:
- Is unique within a world, and never reused by it (each world counts its own ids up from 1)
- Is deterministic: a scene spawns its entities in name order, so the same scene gives the same names the same ids every load, in this process or the next
- Is never reused across a scene transition or an editor's reload either: the next world's ids start past the last one's, so an id a script kept names nothing in the new scene

The world keys everything by `EntityId`: an entity's record (its name and components), its parent and its children. A name finds its id, and an id its name, in one map access.

```rust
// From flint-core
pub struct EntityId(pub u64);
```

Ids are a world's to hand out (`world.spawn(name)`); there is no way to make one on its own. An entity spawned with a given id (`spawn_with_id`) moves the world's counter past it, so the next spawn can't collide.

The ids are the names' ranks, so adding or renaming an entity moves the ids after it. Something that should stay the same while the scene is edited, such as a seed, comes from the entity's name.

### Order

Everything that walks the world does it in a fixed order, the same every run: the entities with a component (`entities_with_component`) by id, which for a loaded scene is name order; all entities, their names and an entity's children by name. Physics creates its bodies in that order, so a pile of dynamic bodies settles the same way every time a scene is loaded ([`flint-examples/physics_pile.scene.toml`](https://github.com/chrischaps/flint-examples/blob/main/physics_pile.scene.toml)).

## Named Entities

While entity IDs are the internal identifier, entities in Flint are also **named**. The name is the key in the scene file:

```toml
[entities.front_door]     # "front_door" is the name
archetype = "door"
```

Names must be unique within a scene. They're used in:
- CLI commands: `--name "front_door"`
- Parent references: `parent = "main_hall"`
- Query results
- Constraint violation messages

## Components as Dynamic Data

In most ECS implementations, components are Rust structs:

```rust
// NOT how Flint works
struct Transform { position: Vec3, rotation: Vec3 }
```

In Flint, components are `toml::Value` maps, defined by schema files:

```toml
# schemas/components/transform.toml
[component.transform]
description = "Position and rotation in 3D space"

[component.transform.fields]
position = { type = "vec3", default = [0, 0, 0] }
rotation = { type = "vec3", default = [0, 0, 0] }
scale = { type = "vec3", default = [1, 1, 1] }
```

This design trades some type safety and performance for flexibility --- archetypes and components can be defined, modified, and extended without touching Rust code.

## Parent-Child Relationships

Entities can form hierarchies. A child entity references its parent by name:

```toml
[entities.kitchen]
archetype = "room"
parent = "main_hall"
```

The ECS layer tracks these relationships, enabling:
- Hierarchical transforms (child positions are relative to parent). A parent with no `transform` is a bare group: it moves nothing, and its children are placed relative to the nearest ancestors above it that have one
- Tree queries ("find all children of main_hall")
- Cascading operations: despawning an entity removes all its descendants with it (a child's position is relative to its parent, so left behind it would jump). To keep a child, give it another parent first. `flint entity delete` refuses an entity with children unless given `--recursive`

## Archetypes

An archetype is a named bundle of components that defines an entity "type":

```toml
# schemas/examples/archetypes/door.toml (the examples' layer)
[archetype.door]
description = "A door entity"
components = ["transform", "door"]

[archetype.door.defaults.door]
style = "hinged"
locked = false
```

When you create an entity with `--archetype door`, Flint ensures it has the required components and fills in defaults for any missing values. The scene loader does the same at load time: archetype defaults go in first, then the authored fields, then any component-schema `default` for a field the author left out (see [Scenes](scenes.md#loading-and-saving)).

Archetypes are not rigid types --- an entity can have components beyond what its archetype specifies. The archetype defines the *minimum* set.

## Working with Entities via CLI

```bash
# Create an entity
flint entity create --archetype door --name "vault_door" \
    --scene levels/dungeon.scene.toml \
    --schemas schemas \
    --props '{"transform":{"position":[0,0,0]},"door":{"locked":true}}'

# Delete an entity (refused while it has children)
flint entity delete vault_door --scene levels/dungeon.scene.toml

# Delete an entity and everything under it
flint entity delete vault_room --recursive --scene levels/dungeon.scene.toml

# List entities in a scene
flint query "entities" --scene levels/dungeon.scene.toml

# Filter by archetype
flint query "entities where archetype == 'door'" --scene levels/dungeon.scene.toml
```

`create` and `delete` change one table in the file and leave every other line as it was, comments, order and `[prefabs]` included, so `git diff` shows only the entity added or removed. `create` writes the archetype, the parent and the `--props` you gave; the archetype's defaults are filled in when the scene loads, as for an entity written by hand. Both commands load the edited text before writing it, so an edit that would break the scene (a parent that isn't there, a field of the wrong type for its schema) is refused and the file is left alone. An entity made by a prefab instance has no table of its own: delete the `[prefabs.<instance>]` entry instead.

## Further Reading

- [Schemas](schemas.md) --- how components and archetypes are defined
- [Scenes](scenes.md) --- how entities are serialized to TOML
- [Queries](queries.md) --- how to filter and inspect entities
