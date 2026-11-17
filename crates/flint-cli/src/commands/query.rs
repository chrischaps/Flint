//! Query command: a scene file's world, or a running game's (`--attach`,
//! #713), answered by the one `flint_query::answer`.

use anyhow::{Context, Result};
use flint_ecs::FlintWorld;
use flint_query::{answer, parse_query, Ask};
use flint_scene::load_scene;
use serde_json::Value;

pub struct QueryArgs<'a> {
    pub query: &'a str,
    pub scene: Option<&'a str>,
    pub attach: Option<&'a str>,
    pub data: bool,
    pub format: &'a str,
    pub schemas: &'a [String],
}

pub fn run(args: QueryArgs) -> Result<()> {
    // Parsed here too, so a typo fails before a scene loads or a game is asked
    let query = parse_query(args.query).context("Failed to parse query")?;
    let ask = Ask::Query {
        query: args.query.to_owned(),
        data: args.data,
    };

    let result = match args.attach {
        Some(addr) => super::attach::ask(addr, &ask)?.answer,
        None => {
            let world = if let Some(path) = args.scene {
                // The one resolver (ADR 0114). With no directory found the
                // scene still loads, with no archetype defaults
                let (registry, _) = super::common_args::load_schemas(Some(path), args.schemas)?;
                let (world, _) = load_scene(path, &registry).context("Failed to load scene")?;
                world
            } else {
                // Create empty world if no scene
                FlintWorld::new()
            };
            answer(&world, &ask).map_err(anyhow::Error::msg)?
        }
    };

    match crate::output::format_or_json(args.format) {
        "json" => crate::output::document(&result),
        "toml" => println!("{}", to_toml(&query.resource, result)?),
        _ => anyhow::bail!("Unknown format: {}", args.format),
    }
    Ok(())
}

/// The answer as TOML, under `entities` or `components` (TOML has no
/// top-level array).
fn to_toml(resource: &str, result: Value) -> Result<String> {
    let key = if resource == "components" {
        "components"
    } else {
        "entities"
    };
    let wrapped = serde_json::json!({ key: super::attach::without_nulls(result) });
    Ok(toml::to_string_pretty(&wrapped)?)
}
