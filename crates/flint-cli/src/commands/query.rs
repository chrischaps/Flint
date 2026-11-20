//! Query command: a scene file's world, or a running game's (`--attach`,
//! #713), answered by the one `flint_query::answer`.

use super::common_args::load_scene;
use anyhow::{Context, Result};
use flint_ecs::FlintWorld;
use flint_query::{answer, parse_query, Ask};
use serde_json::Value;

/// `flint query`'s flags. Boxed in `Commands`, as every variant there is,
/// so the debug parser's stack doesn't grow with them.
#[derive(clap::Args)]
pub struct QueryArgs {
    /// Query string (e.g., "entities where archetype == 'door'")
    pub query: String,

    /// Path to scene file
    #[arg(long)]
    pub scene: Option<String>,

    /// Ask a running game instead of a file: the address it was started
    /// with as `--inspect-listen` (IP:PORT, or a port on 127.0.0.1). The
    /// same grammar and the same answer, of the world as it stands (#713)
    #[arg(long, value_name = "IP:PORT", conflicts_with_all = ["scene", "schemas"])]
    pub attach: Option<String>,

    /// Each entity with its components' values, as `flint entity show`
    /// prints one, not only its name and component list
    #[arg(long)]
    pub data: bool,

    /// Output format (json or toml)
    #[arg(long, default_value = "json")]
    pub format: String,

    /// Schema directories, later ones overriding earlier (repeatable).
    /// Without it: the project's flint.toml `schemas`, else schemas/ in
    /// the working directory, beside the scene and one level up
    #[arg(long, action = clap::ArgAction::Append)]
    pub schemas: Vec<String>,
}

pub fn run(args: QueryArgs) -> Result<()> {
    // Parsed here too, so a typo fails before a scene loads or a game is asked
    let query = parse_query(&args.query).context("Failed to parse query")?;
    let ask = Ask::Query {
        query: args.query.clone(),
        data: args.data,
    };

    let result = match args.attach.as_deref() {
        Some(addr) => super::attach::ask(addr, &ask)?.answer,
        None => {
            let world = if let Some(path) = args.scene.as_deref() {
                // The one resolver (ADR 0114). With no directory found the
                // scene still loads, with no archetype defaults
                let (registry, _) = super::common_args::load_schemas(Some(path), &args.schemas)?;
                let (world, _) = load_scene(path, &registry)?;
                world
            } else {
                // Create empty world if no scene
                FlintWorld::new()
            };
            answer(&world, &ask).map_err(anyhow::Error::msg)?
        }
    };

    match crate::output::format_or_json(&args.format) {
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
