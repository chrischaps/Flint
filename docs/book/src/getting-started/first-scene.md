# Your First Scene

A Flint scene is a TOML file describing entities, their components, and their relationships. This page explains the scene format by building one from scratch.

## Scene Structure

Every scene file has two sections: metadata and entities.

```toml
# Metadata
[scene]
name = "My Scene"
version = "1.0"
description = "An optional description"

# Entities
[entities.my_entity]
archetype = "room"

[entities.my_entity.transform]
position = [0, 0, 0]
```

The `[scene]` table holds metadata. Everything under `[entities.*]` defines the objects in your world.

## Entities

An entity is a named thing in the scene. Its name is the key under `[entities]`:

```toml
[entities.main_hall]
archetype = "room"
```

Entities can optionally have:
- An **archetype** --- a schema-defined bundle of components
- A **parent** --- another entity this one is attached to
- **Components** --- data tables nested under the entity

## Components

Components are data attached to entities. They're defined as nested TOML tables:

```toml
[entities.main_hall]
archetype = "room"

[entities.main_hall.transform]
position = [0, 0, 0]

[entities.main_hall.bounds]
min = [-7, 0, -5]
max = [7, 4, 5]
```

The `transform` and `bounds` components are defined by schema files in `schemas/components/`. The schema tells Flint what fields are valid and what types they are.

## Parent-Child Relationships

Entities form hierarchies through the `parent` field:

```toml
[entities.main_hall]
archetype = "room"

[entities.main_hall.transform]
position = [0, 0, 0]

[entities.kitchen]
archetype = "room"
parent = "main_hall"

[entities.kitchen.transform]
position = [0, 0, -9]
```

The kitchen is a child of the main hall. In the viewer, child transforms are relative to their parent.

## A Complete Example

Here's a small but complete scene --- a room with a door and a table:

```toml
[scene]
name = "Simple Room"
version = "1.0"

[entities.room]
archetype = "room"

[entities.room.transform]
position = [0, 0, 0]

[entities.room.bounds]
min = [-5, 0, -5]
max = [5, 3, 5]

[entities.door]
archetype = "door"
parent = "room"

[entities.door.transform]
position = [0, 0, 5]

[entities.door.door]
style = "hinged"
locked = false
open_angle = 90.0

[entities.table]
archetype = "furniture"
parent = "room"

[entities.table.transform]
position = [0, 0, 0]

[entities.table.bounds]
min = [-0.6, 0, -0.6]
max = [0.6, 0.8, 0.6]
```

## Editing Scenes

You can edit scene files in three ways:

1. **CLI commands** --- `flint entity create`, `flint entity delete`, etc.
2. **Text editor** --- open the TOML file directly
3. **Programmatically** --- any tool that can write TOML

All three approaches produce the same result. The `flint serve --watch` viewer detects changes from any source and reloads automatically.

## Validating Scenes

Run the constraint checker to verify your scene is well-formed:

```bash
flint validate levels/my-scene.scene.toml --schemas schemas
```

This checks your scene against the rules defined in `schemas/constraints/`. See [Constraints](../concepts/constraints.md) for details.

## What's Next

- [Entities and ECS](../concepts/entities-and-ecs.md) explains the entity-component system
- [Schemas](../concepts/schemas.md) covers how components and archetypes are defined
- [Scenes](../concepts/scenes.md) goes deeper into the scene system internals
