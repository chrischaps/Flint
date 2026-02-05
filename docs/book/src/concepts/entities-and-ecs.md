# Entities and ECS

Flint uses an Entity-Component-System (ECS) architecture, built on top of the [hecs](https://crates.io/crates/hecs) crate. This page explains how entities, components, and IDs work in Flint.

## What Is ECS?

In an ECS architecture:
- **Entities** are unique identifiers (not objects with methods)
- **Components** are pure data attached to entities
- **Systems** are logic that operates on entities with specific component combinations

Flint's twist: components are **dynamic**. Instead of being Rust structs compiled into the engine, they're defined at runtime as TOML schema files and stored as `toml::Value`. This means you can define new component types without recompiling the engine.

## Entity IDs

Every entity gets a stable `EntityId` --- a 64-bit integer that:
- Is unique within a scene
- Never gets recycled (monotonically increasing)
- Persists across save/load cycles
- Is deterministic (the same scene always produces the same IDs)

Internally, Flint maintains a bidirectional map (`BiMap`) between `EntityId` values and hecs `Entity` handles. This allows efficient lookup in both directions.

```rust
// From flint-core
pub struct EntityId(pub u64);
```

When loading a saved scene, the ID counter is adjusted to be higher than any existing ID, preventing collisions when new entities are created.

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
- Hierarchical transforms (child positions are relative to parent)
- Tree queries ("find all children of main_hall")
- Cascading operations (deleting a parent removes children)

## Archetypes

An archetype is a named bundle of components that defines an entity "type":

```toml
# schemas/archetypes/door.toml
[archetype.door]
description = "A door entity"
components = ["transform", "door"]

[archetype.door.defaults.door]
style = "hinged"
locked = false
```

When you create an entity with `--archetype door`, Flint ensures it has the required components and fills in defaults for any missing values.

Archetypes are not rigid types --- an entity can have components beyond what its archetype specifies. The archetype defines the *minimum* set.

## Working with Entities via CLI

```bash
# Create an entity
flint entity create --archetype door --name "vault_door" \
    --scene levels/dungeon.scene.toml \
    --schemas schemas \
    --props '{"transform":{"position":[0,0,0]},"door":{"locked":true}}'

# Delete an entity
flint entity delete --name "vault_door" --scene levels/dungeon.scene.toml

# List entities in a scene
flint query "entities" --scene levels/dungeon.scene.toml

# Filter by archetype
flint query "entities where archetype == 'door'" --scene levels/dungeon.scene.toml
```

## Further Reading

- [Schemas](schemas.md) --- how components and archetypes are defined
- [Scenes](scenes.md) --- how entities are serialized to TOML
- [Queries](queries.md) --- how to filter and inspect entities
