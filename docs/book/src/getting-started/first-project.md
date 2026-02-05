# Your First Project

This guide walks through creating a Flint project and building a simple scene using only CLI commands.

## Initialize a Project

```bash
flint init my-tavern
```

This creates a project directory with the standard structure:

```
my-tavern/
├── schemas/
│   ├── components/
│   │   ├── transform.toml
│   │   ├── bounds.toml
│   │   └── door.toml
│   ├── archetypes/
│   │   ├── room.toml
│   │   ├── door.toml
│   │   ├── furniture.toml
│   │   └── character.toml
│   └── constraints/
│       └── basics.toml
├── levels/
└── assets/
```

The `schemas/` directory contains default component definitions, archetype bundles, and constraint rules. You'll modify and extend these as your project grows.

## Create a Scene

```bash
flint scene create my-tavern/levels/tavern.scene.toml --name "The Rusty Flint Tavern"
```

This creates an empty scene file:

```toml
[scene]
name = "The Rusty Flint Tavern"
version = "1.0"
```

## Add Rooms

Build out the space with room entities:

```bash
flint entity create --archetype room --name "main_hall" \
    --scene my-tavern/levels/tavern.scene.toml \
    --schemas my-tavern/schemas \
    --props '{"transform":{"position":[0,0,0]},"bounds":{"min":[-7,0,-5],"max":[7,4,5]}}'
```

The `--archetype room` flag tells Flint to create an entity with the components defined in `schemas/archetypes/room.toml` (transform + bounds). The `--props` flag provides the specific values.

Add a kitchen connected to the main hall:

```bash
flint entity create --archetype room --name "kitchen" \
    --parent "main_hall" \
    --scene my-tavern/levels/tavern.scene.toml \
    --schemas my-tavern/schemas \
    --props '{"transform":{"position":[0,0,-9]},"bounds":{"min":[-4,0,-3],"max":[4,3.5,3]}}'
```

The `--parent` flag establishes a hierarchy --- the kitchen is a child of the main hall.

## Add a Door

```bash
flint entity create --archetype door --name "front_entrance" \
    --parent "main_hall" \
    --scene my-tavern/levels/tavern.scene.toml \
    --schemas my-tavern/schemas \
    --props '{"transform":{"position":[0,0,5]},"door":{"style":"hinged","locked":false}}'
```

## Query Your Scene

See what you've built:

```bash
flint query "entities" --scene my-tavern/levels/tavern.scene.toml
```

Filter for specific archetypes:

```bash
flint query "entities where archetype == 'door'" --scene my-tavern/levels/tavern.scene.toml
```

## Inspect the Scene File

The scene is plain TOML. Open `my-tavern/levels/tavern.scene.toml` and you'll see:

```toml
[scene]
name = "The Rusty Flint Tavern"
version = "1.0"

[entities.main_hall]
archetype = "room"

[entities.main_hall.transform]
position = [0, 0, 0]

[entities.main_hall.bounds]
min = [-7, 0, -5]
max = [7, 4, 5]

[entities.kitchen]
archetype = "room"
parent = "main_hall"

[entities.kitchen.transform]
position = [0, 0, -9]

[entities.kitchen.bounds]
min = [-4, 0, -3]
max = [4, 3.5, 3]

[entities.front_entrance]
archetype = "door"
parent = "main_hall"

[entities.front_entrance.transform]
position = [0, 0, 5]

[entities.front_entrance.door]
style = "hinged"
locked = false
```

Everything is readable, editable, and diffable. You can modify this file directly --- the CLI isn't the only way to edit scenes.

## View It

Launch the hot-reload viewer:

```bash
flint serve my-tavern/levels/tavern.scene.toml --watch --schemas my-tavern/schemas
```

A window opens showing your scene as colored boxes:
- **Blue** wireframes for rooms
- **Orange** boxes for doors
- **Green** boxes for furniture
- **Yellow** boxes for characters

The viewer hot-reloads --- any change to the scene file (from the CLI, a text editor, or an AI agent) updates the view instantly.

**Camera controls:**
| Input | Action |
|-------|--------|
| Left-drag | Orbit |
| Right-drag | Pan |
| Scroll | Zoom |
| Space | Reset camera |
| R | Force reload |
| Escape | Quit |

## What's Next

- [Your First Scene](first-scene.md) dives deeper into scene file structure
- [Querying Entities](first-query.md) covers the query language
- [Building a Tavern](../guides/building-a-tavern.md) walks through a complete scene build
