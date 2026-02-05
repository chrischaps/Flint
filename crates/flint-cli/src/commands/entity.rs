//! Entity management commands

use anyhow::{Context, Result};
use clap::Subcommand;
use flint_scene::{load_scene, EntityDef};
use flint_schema::SchemaRegistry;
use std::path::Path;

#[derive(Subcommand)]
pub enum EntityCommands {
    /// Create a new entity
    Create {
        /// Archetype name
        #[arg(long)]
        archetype: String,

        /// Entity name
        #[arg(long)]
        name: String,

        /// Path to scene file
        #[arg(long)]
        scene: String,

        /// Parent entity name
        #[arg(long)]
        parent: Option<String>,

        /// Properties as JSON
        #[arg(long)]
        props: Option<String>,

        /// Path to schemas directory
        #[arg(long, default_value = "schemas")]
        schemas: String,
    },

    /// List all entities
    List {
        /// Path to scene file
        #[arg(long)]
        scene: String,

        /// Output format (json or toml)
        #[arg(long, default_value = "json")]
        format: String,

        /// Path to schemas directory
        #[arg(long, default_value = "schemas")]
        schemas: String,
    },

    /// Delete an entity
    Delete {
        /// Entity name
        name: String,

        /// Path to scene file
        #[arg(long)]
        scene: String,

        /// Path to schemas directory
        #[arg(long, default_value = "schemas")]
        schemas: String,
    },

    /// Show entity details
    Show {
        /// Entity name
        name: String,

        /// Path to scene file
        #[arg(long)]
        scene: String,

        /// Output format (json or toml)
        #[arg(long, default_value = "json")]
        format: String,

        /// Path to schemas directory
        #[arg(long, default_value = "schemas")]
        schemas: String,
    },
}

pub fn run(cmd: EntityCommands) -> Result<()> {
    match cmd {
        EntityCommands::Create {
            archetype,
            name,
            scene,
            parent,
            props,
            schemas,
        } => create(&archetype, &name, &scene, parent.as_deref(), props.as_deref(), &schemas),

        EntityCommands::List { scene, format, schemas } => list(&scene, &format, &schemas),

        EntityCommands::Delete { name, scene, schemas } => delete(&name, &scene, &schemas),

        EntityCommands::Show { name, scene, format, schemas } => show(&name, &scene, &format, &schemas),
    }
}

fn create(
    archetype: &str,
    name: &str,
    scene_path: &str,
    parent: Option<&str>,
    props: Option<&str>,
    schemas_path: &str,
) -> Result<()> {
    let registry = load_registry(schemas_path)?;

    // Check archetype exists
    if registry.get_archetype(archetype).is_none() {
        anyhow::bail!("Unknown archetype: {}", archetype);
    }

    // Load or create scene
    let (mut world, mut scene_file) = if Path::new(scene_path).exists() {
        load_scene(scene_path, &registry).context("Failed to load scene")?
    } else {
        anyhow::bail!("Scene file not found: {}", scene_path);
    };

    // Check entity doesn't already exist
    if world.contains_name(name) {
        anyhow::bail!("Entity '{}' already exists", name);
    }

    // Create entity
    let id = world
        .spawn_archetype(name, archetype, &registry)
        .context("Failed to create entity")?;

    // Set parent if provided
    if let Some(parent_name) = parent {
        world
            .set_parent_by_name(name, parent_name)
            .context("Failed to set parent")?;
    }

    // Parse and apply properties
    if let Some(props_json) = props {
        let props_value: serde_json::Value =
            serde_json::from_str(props_json).context("Failed to parse props JSON")?;

        if let Some(obj) = props_value.as_object() {
            let components = world.get_components_mut(id).unwrap();
            for (comp_name, comp_data) in obj {
                // Convert JSON to TOML value
                let toml_str = serde_json::to_string(comp_data)?;
                let toml_val: toml::Value = toml::from_str(&format!(
                    "data = {}",
                    toml_str.replace(":", "=")
                ))
                .ok()
                .and_then(|t: toml::Value| t.get("data").cloned())
                .unwrap_or_else(|| json_to_toml(comp_data));

                components.set(comp_name.clone(), toml_val);
            }
        }
    }

    // Update scene file
    let mut entity_def = EntityDef::new().with_archetype(archetype);
    if let Some(p) = parent {
        entity_def = entity_def.with_parent(p);
    }
    if let Some(components) = world.get_components(id) {
        for (comp_name, comp_data) in &components.data {
            entity_def.components.insert(comp_name.clone(), comp_data.clone());
        }
    }
    scene_file.entities.insert(name.to_string(), entity_def);

    // Save scene
    let content = toml::to_string_pretty(&scene_file)?;
    std::fs::write(scene_path, content)?;

    println!("Created entity '{}' with archetype '{}'", name, archetype);

    Ok(())
}

fn list(scene_path: &str, format: &str, schemas_path: &str) -> Result<()> {
    let registry = load_registry(schemas_path)?;
    let (world, _) = load_scene(scene_path, &registry).context("Failed to load scene")?;

    let entities = world.all_entities();

    match format {
        "json" => {
            println!("{}", serde_json::to_string_pretty(&entities)?);
        }
        "toml" => {
            #[derive(serde::Serialize)]
            struct Wrapper {
                entities: Vec<flint_ecs::EntityInfo>,
            }
            println!("{}", toml::to_string_pretty(&Wrapper { entities })?);
        }
        _ => anyhow::bail!("Unknown format: {}", format),
    }

    Ok(())
}

fn delete(name: &str, scene_path: &str, schemas_path: &str) -> Result<()> {
    let registry = load_registry(schemas_path)?;
    let (mut world, mut scene_file) = load_scene(scene_path, &registry).context("Failed to load scene")?;

    world
        .despawn_by_name(name)
        .context(format!("Failed to delete entity '{}'", name))?;

    scene_file.entities.remove(name);

    let content = toml::to_string_pretty(&scene_file)?;
    std::fs::write(scene_path, content)?;

    println!("Deleted entity '{}'", name);

    Ok(())
}

fn show(name: &str, scene_path: &str, format: &str, schemas_path: &str) -> Result<()> {
    let registry = load_registry(schemas_path)?;
    let (world, _) = load_scene(scene_path, &registry).context("Failed to load scene")?;

    let id = world
        .get_id(name)
        .ok_or_else(|| anyhow::anyhow!("Entity '{}' not found", name))?;

    let info = world
        .all_entities()
        .into_iter()
        .find(|e| e.name == name)
        .unwrap();

    let components = world.get_components(id);

    #[derive(serde::Serialize)]
    struct EntityDetails {
        #[serde(flatten)]
        info: flint_ecs::EntityInfo,
        data: Option<std::collections::HashMap<String, toml::Value>>,
    }

    let details = EntityDetails {
        info,
        data: components.map(|c| c.data.clone()),
    };

    match format {
        "json" => {
            println!("{}", serde_json::to_string_pretty(&details)?);
        }
        "toml" => {
            println!("{}", toml::to_string_pretty(&details)?);
        }
        _ => anyhow::bail!("Unknown format: {}", format),
    }

    Ok(())
}

fn load_registry(path: &str) -> Result<SchemaRegistry> {
    if Path::new(path).exists() {
        SchemaRegistry::load_from_directory(path).context("Failed to load schemas")
    } else {
        Ok(SchemaRegistry::new())
    }
}

fn json_to_toml(json: &serde_json::Value) -> toml::Value {
    match json {
        serde_json::Value::Null => toml::Value::String("null".to_string()),
        serde_json::Value::Bool(b) => toml::Value::Boolean(*b),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                toml::Value::Integer(i)
            } else if let Some(f) = n.as_f64() {
                toml::Value::Float(f)
            } else {
                toml::Value::String(n.to_string())
            }
        }
        serde_json::Value::String(s) => toml::Value::String(s.clone()),
        serde_json::Value::Array(arr) => {
            toml::Value::Array(arr.iter().map(json_to_toml).collect())
        }
        serde_json::Value::Object(obj) => {
            let mut map = toml::map::Map::new();
            for (k, v) in obj {
                map.insert(k.clone(), json_to_toml(v));
            }
            toml::Value::Table(map)
        }
    }
}
