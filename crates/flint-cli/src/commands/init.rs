//! Project initialization command

use anyhow::Result;
use std::fs;
use std::path::Path;

pub fn run(name: &str) -> Result<()> {
    let project_dir = Path::new(name);

    if project_dir.exists() {
        anyhow::bail!("Directory '{}' already exists", name);
    }

    // Create directory structure
    fs::create_dir_all(project_dir.join("schemas/components"))?;
    fs::create_dir_all(project_dir.join("schemas/archetypes"))?;
    fs::create_dir_all(project_dir.join("schemas/constraints"))?;
    fs::create_dir_all(project_dir.join("levels"))?;
    fs::create_dir_all(project_dir.join("assets/meshes"))?;
    fs::create_dir_all(project_dir.join("assets/textures"))?;
    fs::create_dir_all(project_dir.join("assets/materials"))?;

    // Create default component schemas
    fs::write(
        project_dir.join("schemas/components/transform.toml"),
        r#"[component.transform]
description = "Spatial transform for positioning entities"

[component.transform.fields]
position = { type = "vec3", default = [0, 0, 0] }
rotation = { type = "vec3", default = [0, 0, 0], description = "Euler angles in degrees" }
scale = { type = "vec3", default = [1, 1, 1] }
"#,
    )?;

    fs::write(
        project_dir.join("schemas/components/bounds.toml"),
        r#"[component.bounds]
description = "Axis-aligned bounding box"

[component.bounds.fields]
min = { type = "vec3", default = [0, 0, 0] }
max = { type = "vec3", default = [1, 1, 1] }
"#,
    )?;

    fs::write(
        project_dir.join("schemas/components/door.toml"),
        r#"[component.door]
description = "A door that can connect spaces"

[component.door.fields]
style = { type = "enum", values = ["hinged", "sliding", "rotating"], default = "hinged" }
locked = { type = "bool", default = false }
open_angle = { type = "f32", default = 90.0, min = 0.0, max = 180.0 }
"#,
    )?;

    // Create default archetype schemas
    fs::write(
        project_dir.join("schemas/archetypes/room.toml"),
        r#"[archetype.room]
description = "A room or enclosed space"
components = ["transform", "bounds"]

[archetype.room.defaults.bounds]
min = [0, 0, 0]
max = [10, 4, 10]
"#,
    )?;

    fs::write(
        project_dir.join("schemas/archetypes/door.toml"),
        r#"[archetype.door]
description = "A door entity"
components = ["transform", "door"]

[archetype.door.defaults.door]
style = "hinged"
locked = false
"#,
    )?;

    fs::write(
        project_dir.join("schemas/archetypes/furniture.toml"),
        r#"[archetype.furniture]
description = "A piece of furniture"
components = ["transform", "bounds"]

[archetype.furniture.defaults.bounds]
min = [0, 0, 0]
max = [1, 1, 1]
"#,
    )?;

    fs::write(
        project_dir.join("schemas/archetypes/character.toml"),
        r#"[archetype.character]
description = "A character entity (player or NPC)"
components = ["transform"]
"#,
    )?;

    // Create a sample constraint
    fs::write(
        project_dir.join("schemas/constraints/basics.toml"),
        r#"# Basic scene constraints
# These rules are checked by `flint validate`

[[constraint]]
name = "doors_have_transform"
description = "Every door must have a transform component"
query = "entities where archetype == 'door'"
severity = "error"
message = "Door '{name}' is missing a transform component"

[constraint.kind]
type = "required_component"
archetype = "door"
component = "transform"

[[constraint]]
name = "rooms_have_bounds"
description = "Every room must have a bounds component"
query = "entities where archetype == 'room'"
severity = "error"
message = "Room '{name}' is missing a bounds component"

[constraint.kind]
type = "required_component"
archetype = "room"
component = "bounds"
"#,
    )?;

    // Create an example scene
    fs::write(
        project_dir.join("levels/demo.scene.toml"),
        r#"[scene]
name = "Demo Scene"
version = "1.0"
description = "A simple demo scene"

[entities.main_room]
archetype = "room"

[entities.main_room.transform]
position = [0, 0, 0]

[entities.main_room.bounds]
min = [0, 0, 0]
max = [10, 4, 8]

[entities.front_door]
archetype = "door"
parent = "main_room"

[entities.front_door.transform]
position = [5, 0, 0]

[entities.front_door.door]
style = "hinged"
locked = false
"#,
    )?;

    // Create CLAUDE.md for AI agent discoverability
    fs::write(
        project_dir.join("CLAUDE.md"),
        format!(
            r#"# {name}

A game built with the [Flint engine](https://github.com/chrischaps/Flint).

## Build & Run

```bash
# Build the engine (from engine/ subtree)
cd engine && cargo build --release && cd ..

# Play the game
engine/target/release/flint play levels/demo.scene.toml --schemas engine/schemas --schemas schemas

# Validate visual changes by rendering to a PNG (no window required)
engine/target/release/flint render levels/demo.scene.toml --output render_test.png \
  --schemas engine/schemas --schemas schemas --width 1280 --height 720 --no-grid --shadows
```

## Development Workflow

Use the **edit → render → play** loop:

1. Edit scene files (`levels/*.scene.toml`), scripts (`scripts/*.rhai`), or models (`models/*.glb`)
2. **Render a snapshot** with `flint render` to validate visual changes — fast, headless, no window needed
3. **Play** with `flint play` to test interactively with physics, scripting, and input

## Project Structure

- `levels/` — Scene files (TOML)
- `schemas/` — Game-specific component and archetype schemas
- `scripts/` — Rhai game scripts
- `models/` — 3D models (GLB format)
- `assets/` — Textures, materials, audio
- `engine/` — Flint engine (git subtree)
"#,
            name = name
        ),
    )?;

    println!("Created Flint project: {}", name);
    println!();
    println!("Project structure:");
    println!("  {}/", name);
    println!("  ├── CLAUDE.md");
    println!("  ├── schemas/");
    println!("  │   ├── components/");
    println!("  │   │   ├── transform.toml");
    println!("  │   │   ├── bounds.toml");
    println!("  │   │   └── door.toml");
    println!("  │   ├── archetypes/");
    println!("  │   │   ├── room.toml");
    println!("  │   │   ├── door.toml");
    println!("  │   │   ├── furniture.toml");
    println!("  │   │   └── character.toml");
    println!("  │   └── constraints/");
    println!("  │       └── basics.toml");
    println!("  ├── assets/");
    println!("  │   ├── meshes/");
    println!("  │   ├── textures/");
    println!("  │   └── materials/");
    println!("  └── levels/");
    println!("      └── demo.scene.toml");
    println!();
    println!("Next steps:");
    println!("  cd {}", name);
    println!("  flint render levels/demo.scene.toml --output test.png --schemas schemas");
    println!("  flint serve --watch levels/demo.scene.toml --schemas schemas");

    Ok(())
}
