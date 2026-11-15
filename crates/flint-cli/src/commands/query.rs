//! Query command

use anyhow::{Context, Result};
use flint_ecs::FlintWorld;
use flint_query::{execute_query, format_json, format_toml, parse_query};
use flint_scene::load_scene;

pub fn run(
    query_str: &str,
    scene_path: Option<&str>,
    format: &str,
    schemas: &[String],
) -> Result<()> {
    // Parse query
    let query = parse_query(query_str).context("Failed to parse query")?;

    // Load scene if provided
    let world = if let Some(path) = scene_path {
        // The one resolver (ADR 0114). With no directory found the scene
        // still loads, with no archetype defaults
        let (registry, _) = super::common_args::load_schemas(Some(path), schemas)?;

        let (world, _) = load_scene(path, &registry).context("Failed to load scene")?;
        world
    } else {
        // Create empty world if no scene
        FlintWorld::new()
    };

    // Execute query
    let result = execute_query(&world, &query);

    // Format output
    let output = match format {
        "json" => format_json(&result),
        "toml" => format_toml(&result),
        _ => anyhow::bail!("Unknown format: {}", format),
    };

    println!("{}", output);

    Ok(())
}
