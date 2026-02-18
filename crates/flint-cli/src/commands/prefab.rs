//! Prefab operations — view prefab templates in the interactive viewer.

use anyhow::{Context, Result};
use clap::Subcommand;
use flint_scene::{load_scene_string, PrefabFile, SceneFile};
use flint_schema::SchemaRegistry;
use std::collections::HashMap;
use std::path::Path;

#[derive(Subcommand)]
pub enum PrefabCommands {
    /// View a prefab template in the interactive viewer
    View {
        /// Path to .prefab.toml file
        path: String,

        /// Paths to schemas directories (can specify multiple)
        #[arg(long, default_value = "schemas", action = clap::ArgAction::Append)]
        schemas: Vec<String>,

        /// Instance prefix (defaults to prefab name)
        #[arg(long)]
        prefix: Option<String>,
    },
}

pub fn run(cmd: PrefabCommands) -> Result<()> {
    match cmd {
        PrefabCommands::View {
            path,
            schemas,
            prefix,
        } => run_view(&path, &schemas, prefix.as_deref()),
    }
}

fn run_view(path: &str, schemas: &[String], prefix_override: Option<&str>) -> Result<()> {
    // Load schemas
    let existing: Vec<&str> = schemas
        .iter()
        .map(|s| s.as_str())
        .filter(|p| Path::new(p).exists())
        .collect();
    let registry = if !existing.is_empty() {
        SchemaRegistry::load_from_directories(&existing).context("Failed to load schemas")?
    } else {
        println!("Warning: No schemas directories found");
        SchemaRegistry::new()
    };

    // Read and parse the prefab file
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read prefab file: {}", path))?;
    let prefab: PrefabFile = toml::from_str(&content)
        .with_context(|| format!("Failed to parse prefab file: {}", path))?;

    println!("Prefab: {}", prefab.prefab.name);
    if let Some(desc) = &prefab.prefab.description {
        println!("  {}", desc);
    }
    println!("  {} entities", prefab.entities.len());

    // Determine prefix
    let prefix = prefix_override.unwrap_or(&prefab.prefab.name);

    // Build a synthetic scene with the prefab entities expanded inline
    let mut scene = SceneFile::new(format!("Prefab Preview: {}", prefab.prefab.name));

    let mut vars = HashMap::new();
    vars.insert("PREFIX".to_string(), prefix.to_string());

    for (suffix, template_def) in &prefab.entities {
        let entity_name = format!("{}_{}", prefix, suffix);
        let mut entity = template_def.clone();

        // Prefix parent references
        if let Some(ref parent) = entity.parent {
            entity.parent = Some(format!("{}_{}", prefix, parent));
        }

        // Substitute ${PREFIX} in all string component values
        for (_comp_name, comp_value) in entity.components.iter_mut() {
            substitute_in_value(comp_value, &vars);
        }

        scene.entities.insert(entity_name, entity);
    }

    // Serialize to TOML string and load via load_scene_string
    let scene_toml =
        toml::to_string_pretty(&scene).context("Failed to serialize synthetic scene")?;
    let (world, _scene_file) =
        load_scene_string(&scene_toml, &registry).context("Failed to load synthetic scene")?;

    println!("Loaded {} entities", world.entity_count());

    // Use scenes/_prefab_preview.scene.toml as anchor so model resolution
    // tries scenes/models/ (miss) then models/ (hit)
    let anchor = "scenes/_prefab_preview.scene.toml";

    flint_viewer::app::run_with_world(world, registry, anchor, true)
}

/// Recursively substitute `${VAR}` patterns in string values within a toml::Value tree.
fn substitute_in_value(value: &mut toml::Value, vars: &HashMap<String, String>) {
    match value {
        toml::Value::String(s) => {
            for (key, replacement) in vars {
                let pattern = format!("${{{}}}", key);
                if s.contains(&pattern) {
                    *s = s.replace(&pattern, replacement);
                }
            }
        }
        toml::Value::Table(table) => {
            for (_k, v) in table.iter_mut() {
                substitute_in_value(v, vars);
            }
        }
        toml::Value::Array(arr) => {
            for v in arr.iter_mut() {
                substitute_in_value(v, vars);
            }
        }
        _ => {}
    }
}
