//! The toy package's command: `flint toy springs <scene>`, each `toy_spring`
//! a scene file declares, read from the file as written.

use crate::TOY_SPRING;
use flint_package::cli::{emit, subcommands, Answer, CliExtension};
use std::path::PathBuf;

#[derive(clap::Subcommand)]
enum Top {
    /// The toy package (#1560): springs pulled each fixed physics step
    #[command(subcommand)]
    Toy(ToyCommands),
}

#[derive(clap::Subcommand)]
enum ToyCommands {
    /// List the springs a scene file declares
    Springs {
        /// The scene file
        scene: PathBuf,
    },
}

/// The package's commands, as the CLI launcher lists them.
pub fn extensions() -> Vec<Box<dyn CliExtension>> {
    vec![subcommands(|Top::Toy(command)| run(command))]
}

#[allow(clippy::disallowed_methods)] // a command reads the file it's named
fn run(command: ToyCommands) -> anyhow::Result<()> {
    let ToyCommands::Springs { scene } = command;
    let text = std::fs::read_to_string(&scene)?;
    let table: toml::Table = text.parse()?;
    let mut lines = Vec::new();
    let mut found = Vec::new();
    let entities = table.get("entities").and_then(|e| e.as_table());
    for (name, entity) in entities.into_iter().flatten() {
        let Some(spring) = entity.get(TOY_SPRING) else {
            continue;
        };
        let k = match spring.get("k") {
            Some(toml::Value::Float(k)) => *k,
            Some(toml::Value::Integer(k)) => *k as f64,
            _ => 40.0,
        };
        lines.push(format!("{name}: k = {k} N/m"));
        found.push(serde_json::json!({ "entity": name, "k": k }));
    }
    let text = format!(
        "{} spring(s) in {}{}{}",
        found.len(),
        scene.display(),
        if lines.is_empty() { "" } else { "\n" },
        lines.join("\n")
    );
    emit(&Answer::new(text, serde_json::json!({ "springs": found })))
}
