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

    println!("Created Flint project: {}", name);
    println!("");
    println!("Project structure:");
    println!("  {}/", name);
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
    println!("");
    println!("Next steps:");
    println!("  cd {}", name);
    println!("  flint serve --watch levels/demo.scene.toml");

    Ok(())
}
