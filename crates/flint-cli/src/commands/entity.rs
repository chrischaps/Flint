//! Entity management commands
//!
//! `create` and `delete` edit the scene file through [`SceneDocument`] (#678):
//! one table goes in or comes out and every other byte stays as it was, with
//! comments, order and `[prefabs]` intact. The text about to be written is
//! loaded first, so an edit that would leave the scene unloadable (a dangling
//! `parent`, props a schema rejects) is refused before the file changes.

use anyhow::{Context, Result};
use clap::Subcommand;
use flint_scene::{load_scene, load_scene_text, SceneDocument, SceneFile};
use flint_schema::{validate_component_data, SchemaRegistry};
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

        /// Properties as JSON: an object of components, each an object of
        /// fields, as {"transform": {"position": [0, 1, 0]}}
        #[arg(long)]
        props: Option<String>,

        /// Schema directories, later ones overriding earlier (repeatable).
        /// Without it: the project's flint.toml `schemas`, else schemas/ in
        /// the working directory, beside the scene and one level up
        #[arg(long, action = clap::ArgAction::Append)]
        schemas: Vec<String>,
    },

    /// List all entities
    List {
        /// Path to scene file
        #[arg(long)]
        scene: String,

        /// Output format (json or toml)
        #[arg(long, default_value = "json")]
        format: String,

        /// Schema directories, later ones overriding earlier (repeatable).
        /// Without it: the project's flint.toml `schemas`, else schemas/ in
        /// the working directory, beside the scene and one level up
        #[arg(long, action = clap::ArgAction::Append)]
        schemas: Vec<String>,
    },

    /// Delete an entity
    Delete {
        /// Entity name
        name: String,

        /// Path to scene file
        #[arg(long)]
        scene: String,

        /// Also delete every entity under it. Without it, an entity with
        /// children is refused
        #[arg(long)]
        recursive: bool,

        /// Schema directories, later ones overriding earlier (repeatable).
        /// Without it: the project's flint.toml `schemas`, else schemas/ in
        /// the working directory, beside the scene and one level up
        #[arg(long, action = clap::ArgAction::Append)]
        schemas: Vec<String>,
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

        /// Schema directories, later ones overriding earlier (repeatable).
        /// Without it: the project's flint.toml `schemas`, else schemas/ in
        /// the working directory, beside the scene and one level up
        #[arg(long, action = clap::ArgAction::Append)]
        schemas: Vec<String>,
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
        } => create(
            &archetype,
            &name,
            &scene,
            parent.as_deref(),
            props.as_deref(),
            &schemas,
        ),

        EntityCommands::List {
            scene,
            format,
            schemas,
        } => list(&scene, &format, &schemas),

        EntityCommands::Delete {
            name,
            scene,
            recursive,
            schemas,
        } => delete(&name, &scene, recursive, &schemas),

        EntityCommands::Show {
            name,
            scene,
            format,
            schemas,
        } => show(&name, &scene, &format, &schemas),
    }
}

fn create(
    archetype: &str,
    name: &str,
    scene_path: &str,
    parent: Option<&str>,
    props: Option<&str>,
    schemas: &[String],
) -> Result<()> {
    let registry = load_registry(scene_path, schemas)?;
    if registry.get_archetype(archetype).is_none() {
        anyhow::bail!("Unknown archetype: {}", archetype);
    }
    let path = Path::new(scene_path);
    if !path.exists() {
        anyhow::bail!("Scene file not found: {}", scene_path);
    }

    // The scene as it loads, prefab instances and all, for the names in it
    let (world, _) = load_scene(scene_path, &registry).context("Failed to load scene")?;
    if world.contains_name(name) {
        anyhow::bail!("Entity '{}' already exists", name);
    }
    if let Some(parent) = parent {
        if !world.contains_name(parent) {
            anyhow::bail!("Parent entity '{}' not found in {}", parent, scene_path);
        }
    }

    let components = match props {
        Some(json) => parse_props(json)?,
        None => Vec::new(),
    };

    // Only what was asked for goes in the file: the archetype's defaults are
    // filled in at load, as for any entity written by hand
    let mut doc = SceneDocument::from_file(path).map_err(anyhow::Error::msg)?;
    doc.insert_entity(name, Some(archetype), parent, &components)
        .map_err(anyhow::Error::msg)?;
    let text = doc.to_string();

    // The new file must load, and what the props set must fit the schemas
    let (world, _) = load_scene_text(&text, path, &registry)
        .context("Refusing to write: the scene would not load")?;
    let id = world
        .get_id(name)
        .ok_or_else(|| anyhow::anyhow!("Refusing to write: '{}' did not load", name))?;
    for (component, _) in &components {
        let Some(schema) = registry.get_component(component) else {
            eprintln!(
                "warning: no schema for component '{}'; written unchecked",
                component
            );
            continue;
        };
        let data = world
            .get_components(id)
            .and_then(|c| c.data.get(component))
            .ok_or_else(|| anyhow::anyhow!("Refusing to write: '{}' lost '{}'", name, component))?;
        validate_component_data(schema, data)
            .map_err(|e| anyhow::anyhow!("Refusing to write: component '{}': {}", component, e))?;
    }

    std::fs::write(scene_path, text)?;
    println!("Created entity '{}' with archetype '{}'", name, archetype);
    Ok(())
}

/// `--props` as components, components and fields sorted by name so the same
/// edit always writes the same bytes: a JSON object of objects, each the
/// fields of one component. Strings stay exactly as typed, colons and all.
fn parse_props(json: &str) -> Result<Vec<(String, toml::Value)>> {
    let value: serde_json::Value =
        serde_json::from_str(json).context("Failed to parse props JSON")?;
    let serde_json::Value::Object(obj) = value else {
        anyhow::bail!(
            "--props must be a JSON object of components, as {{\"transform\": {{\"position\": [0, 1, 0]}}}}"
        );
    };
    obj.iter()
        .map(|(component, data)| {
            if !data.is_object() {
                anyhow::bail!(
                    "--props: component '{}' must be an object of fields, not {}",
                    component,
                    data
                );
            }
            let value = json_to_toml(data)
                .with_context(|| format!("--props: component '{}'", component))?;
            Ok((component.clone(), value))
        })
        .collect()
}

fn list(scene_path: &str, format: &str, schemas: &[String]) -> Result<()> {
    let registry = load_registry(scene_path, schemas)?;
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

fn delete(name: &str, scene_path: &str, recursive: bool, schemas: &[String]) -> Result<()> {
    let registry = load_registry(scene_path, schemas)?;
    let path = Path::new(scene_path);
    let mut doc = SceneDocument::from_file(path).map_err(anyhow::Error::msg)?;

    if !doc.has_entity(name) {
        let (world, _) = load_scene(scene_path, &registry).context("Failed to load scene")?;
        if !world.contains_name(name) {
            anyhow::bail!("Entity '{}' not found", name);
        }
        // Made at load by a prefab instance: there's no table to take out
        let raw: SceneFile = toml::from_str(&std::fs::read_to_string(path)?)?;
        let instance = raw
            .prefabs
            .iter()
            .find(|(_, p)| name.starts_with(&format!("{}_", p.prefix)))
            .map(|(k, _)| k.clone());
        match instance {
            Some(instance) => anyhow::bail!(
                "Entity '{}' comes from the prefab instance [prefabs.{}]; remove that instead",
                name,
                instance
            ),
            None => anyhow::bail!("Entity '{}' is not declared in {}", name, scene_path),
        }
    }

    let doomed = doc.descendants_of(name);
    if doomed.len() > 1 && !recursive {
        anyhow::bail!(
            "Entity '{}' has children ({}); delete them first, or pass --recursive to delete all {}",
            name,
            doc.children_of(name).join(", "),
            doomed.len()
        );
    }
    for entity in &doomed {
        doc.remove_entity(entity).map_err(anyhow::Error::msg)?;
    }
    let text = doc.to_string();

    // A prefab instance's `parent`, say, can still name what went
    load_scene_text(&text, path, &registry)
        .context("Refusing to write: the scene would not load")?;
    std::fs::write(scene_path, text)?;

    match doomed.len() {
        1 => println!("Deleted entity '{}'", name),
        n => println!(
            "Deleted entity '{}' and {} under it: {}",
            name,
            n - 1,
            doomed[..n - 1].join(", ")
        ),
    }
    Ok(())
}

fn show(name: &str, scene_path: &str, format: &str, schemas: &[String]) -> Result<()> {
    let registry = load_registry(scene_path, schemas)?;
    let (world, _) = load_scene(scene_path, &registry).context("Failed to load scene")?;

    let id = world
        .get_id(name)
        .ok_or_else(|| anyhow::anyhow!("Entity '{}' not found", name))?;

    let info = world
        .all_entities()
        .into_iter()
        .find(|e| e.name == name)
        .ok_or_else(|| anyhow::anyhow!("Entity '{}' not found", name))?;

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

/// The scene's schemas from the one resolver (ADR 0114). With no directory
/// found the registry is empty, as it was for a missing `schemas/`.
fn load_registry(scene_path: &str, schemas: &[String]) -> Result<SchemaRegistry> {
    Ok(super::common_args::load_schemas(Some(scene_path), schemas)?.0)
}

/// JSON as TOML, value for value. TOML has no null, so a null is refused
/// rather than written as some string.
fn json_to_toml(json: &serde_json::Value) -> Result<toml::Value> {
    Ok(match json {
        serde_json::Value::Null => anyhow::bail!("TOML has no null; leave the field out"),
        serde_json::Value::Bool(b) => toml::Value::Boolean(*b),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                toml::Value::Integer(i)
            } else if let Some(f) = n.as_f64() {
                toml::Value::Float(f)
            } else {
                anyhow::bail!("{} doesn't fit a TOML number", n)
            }
        }
        serde_json::Value::String(s) => toml::Value::String(s.clone()),
        serde_json::Value::Array(arr) => {
            toml::Value::Array(arr.iter().map(json_to_toml).collect::<Result<_>>()?)
        }
        serde_json::Value::Object(obj) => {
            let mut map = toml::map::Map::new();
            for (k, v) in obj {
                map.insert(k.clone(), json_to_toml(v).with_context(|| k.clone())?);
            }
            toml::Value::Table(map)
        }
    })
}
