//! `flint examples` (#763): the curated example scenes, listed from
//! `examples/index.toml`, and `--check` that holds the index to the folder.
//!
//! ```bash
//! flint examples                    # every example, grouped by topic, each with its purpose
//! flint examples --topic stylize    # one topic
//! flint examples --json             # the same as one JSON document
//! flint examples --check            # the index matches the folder and every scene validates
//! ```
//!
//! The examples are curated and free to change: nothing pins their pixels.
//! What the goldens and CLI tests load are frozen copies under
//! `tests/fixtures/scenes/`. `--check` fails when an indexed scene is
//! missing, a scene on disk has no entry, an entry has no purpose or topic,
//! or a scene fails what `flint validate` checks (it doesn't load, a
//! constraint error, a bad `[wooze]` or species table, a style warning).

use anyhow::{bail, Context, Result};
use flint_constraint::Severity;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// The index, beside the scenes it lists.
pub const INDEX_FILE: &str = "index.toml";

#[derive(clap::Args)]
pub struct ExamplesArgs {
    /// Only the examples of this topic (showcase, rendering, stylize, ...)
    #[arg(long)]
    pub topic: Option<String>,

    /// Print the list as JSON
    #[arg(long)]
    pub json: bool,

    /// Check the index against the folder and validate every example;
    /// exits 1 naming each failure
    #[arg(long)]
    pub check: bool,

    /// The examples folder. Default: examples/, else engine/examples/ (a
    /// game that carries the engine as a subtree)
    #[arg(long, value_name = "DIR")]
    pub dir: Option<PathBuf>,

    /// Schema directories for --check, later ones overriding earlier
    /// (repeatable). Without it each scene's schemas are resolved as
    /// `flint validate` resolves them
    #[arg(long, action = clap::ArgAction::Append)]
    pub schemas: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Index {
    #[serde(default)]
    example: Vec<Example>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Example {
    /// The scene, relative to the examples folder
    pub scene: String,
    pub topic: String,
    pub purpose: String,
}

pub fn run(args: ExamplesArgs) -> Result<()> {
    let dir = match &args.dir {
        Some(dir) => dir.clone(),
        None => default_dir()?,
    };
    let examples = read_index(&dir)?;

    if args.check {
        let failures = check(&dir, &examples, &args.schemas);
        for failure in &failures {
            println!("FAIL {failure}");
        }
        if !failures.is_empty() {
            println!(
                "\n{} examples in {}: {} failure(s)",
                examples.len(),
                dir.display(),
                failures.len()
            );
            std::process::exit(1);
        }
        println!(
            "{} examples in {}: every one indexed and valid",
            examples.len(),
            dir.display()
        );
        return Ok(());
    }

    let shown: Vec<&Example> = examples
        .iter()
        .filter(|e| args.topic.as_deref().is_none_or(|t| e.topic == t))
        .collect();
    if shown.is_empty() {
        if let Some(topic) = &args.topic {
            bail!(
                "no examples with topic \"{topic}\"; topics: {}",
                topics(&examples).join(", ")
            );
        }
    }
    if args.json {
        let list: Vec<serde_json::Value> = shown
            .iter()
            .map(|e| {
                serde_json::json!({
                    "scene": dir.join(&e.scene).to_string_lossy().replace('\\', "/"),
                    "topic": e.topic,
                    "purpose": e.purpose,
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&list)?);
        return Ok(());
    }
    print!("{}", listing(&dir, &shown));
    Ok(())
}

/// `examples/` in the working directory, else `engine/examples/`.
fn default_dir() -> Result<PathBuf> {
    for dir in ["examples", "engine/examples"] {
        let dir = PathBuf::from(dir);
        if dir.join(INDEX_FILE).is_file() {
            return Ok(dir);
        }
    }
    bail!(
        "no examples/{INDEX_FILE} or engine/examples/{INDEX_FILE} here; name the folder with --dir"
    )
}

fn read_index(dir: &Path) -> Result<Vec<Example>> {
    let path = dir.join(INDEX_FILE);
    let text =
        std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    let index: Index =
        toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
    Ok(index.example)
}

/// The topics, in the order the index first uses them.
fn topics(examples: &[Example]) -> Vec<String> {
    let mut seen = Vec::new();
    for e in examples {
        if !seen.contains(&e.topic) {
            seen.push(e.topic.clone());
        }
    }
    seen
}

/// Grouped by topic in index order, each scene path padded so the
/// purposes line up.
fn listing(dir: &Path, examples: &[&Example]) -> String {
    let path = |e: &Example| dir.join(&e.scene).to_string_lossy().replace('\\', "/");
    let width = examples.iter().map(|e| path(e).len()).max().unwrap_or(0);
    let owned: Vec<Example> = examples.iter().map(|e| (*e).clone()).collect();
    let mut out = String::new();
    for topic in topics(&owned) {
        out.push_str(&format!("{topic}\n"));
        for e in examples.iter().filter(|e| e.topic == topic) {
            out.push_str(&format!("  {:width$}  {}\n", path(e), e.purpose));
        }
        out.push('\n');
    }
    out
}

/// Every scene file under `dir`, relative to it with `/` separators.
/// `dist/` (what `flint export web` and `flint bundle` write) is skipped.
fn scenes_on_disk(dir: &Path) -> Vec<String> {
    fn walk(root: &Path, at: &Path, out: &mut Vec<String>) {
        let Ok(entries) = std::fs::read_dir(at) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if at == root && entry.file_name() == "dist" {
                    continue;
                }
                walk(root, &path, out);
            } else if path.to_string_lossy().ends_with(".scene.toml") {
                let rel = path.strip_prefix(root).unwrap_or(&path);
                out.push(rel.to_string_lossy().replace('\\', "/"));
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, dir, &mut out);
    out.sort();
    out
}

/// What is wrong with the index and its scenes, one line each; empty when
/// nothing is.
fn check(dir: &Path, examples: &[Example], schemas: &[String]) -> Vec<String> {
    let mut failures = Vec::new();
    let mut indexed = BTreeSet::new();
    for e in examples {
        if !indexed.insert(e.scene.as_str()) {
            failures.push(format!("{}: listed twice in {INDEX_FILE}", e.scene));
        }
        if e.purpose.trim().is_empty() || e.topic.trim().is_empty() {
            failures.push(format!("{}: needs a topic and a purpose", e.scene));
        }
    }
    for scene in scenes_on_disk(dir) {
        if !indexed.contains(scene.as_str()) {
            failures.push(format!(
                "{scene}: not in {INDEX_FILE} (add it, or move a test-only scene to tests/fixtures/scenes/)"
            ));
        }
    }
    for e in examples {
        let path = dir.join(&e.scene);
        if !path.is_file() {
            failures.push(format!("{}: in {INDEX_FILE} but not on disk", e.scene));
            continue;
        }
        let scene = path.to_string_lossy().replace('\\', "/");
        match super::validate::check(&scene, schemas) {
            Err(err) => failures.push(format!("{scene}: {err:#}")),
            Ok((warnings, report, _diagnostics)) => {
                for warning in warnings {
                    failures.push(format!("{scene}: warning: {warning}"));
                }
                for v in report.violations {
                    if v.severity == Severity::Error {
                        failures.push(format!("{scene}: {}: {}", v.entity_name, v.message));
                    }
                }
            }
        }
    }
    failures
}

#[cfg(test)]
mod tests {
    use super::*;

    fn example(scene: &str, topic: &str) -> Example {
        Example {
            scene: scene.into(),
            topic: topic.into(),
            purpose: format!("{scene}'s purpose"),
        }
    }

    #[test]
    fn the_listing_groups_by_topic_in_index_order() {
        let list = [
            example("b.scene.toml", "stylize"),
            example("a.scene.toml", "showcase"),
            example("c/long_name.scene.toml", "stylize"),
        ];
        let refs: Vec<&Example> = list.iter().collect();
        let out = listing(Path::new("examples"), &refs);
        assert_eq!(
            out,
            "stylize\n\
             \x20 examples/b.scene.toml            b.scene.toml's purpose\n\
             \x20 examples/c/long_name.scene.toml  c/long_name.scene.toml's purpose\n\
             \n\
             showcase\n\
             \x20 examples/a.scene.toml            a.scene.toml's purpose\n\
             \n"
        );
    }

    #[test]
    fn an_index_with_an_unknown_key_is_refused() {
        let err = toml::from_str::<Index>(
            "[[example]]\nscene = \"a.scene.toml\"\ntopic = \"x\"\npurpose = \"y\"\nwhy = \"z\"\n",
        )
        .unwrap_err();
        assert!(err.to_string().contains("why"), "{err}");
    }
}
