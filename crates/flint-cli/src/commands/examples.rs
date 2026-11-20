//! `flint examples` (#763): the curated example scenes, listed from an
//! `index.toml`, and `--check` that holds the index to the folder.
//!
//! ```bash
//! flint examples                    # every example, grouped by topic, each with its purpose
//! flint examples --topic stylize    # one topic
//! flint examples --json             # the same as one JSON document
//! flint examples --check            # the index matches the folder and every scene validates
//! flint examples --dir demos        # an index in another folder
//! ```
//!
//! The index is the project root's `index.toml` unless `--dir` names its
//! folder: since #984 the examples are their own repository,
//! [flint-examples](https://github.com/chrischaps/flint-examples), with the
//! scenes and the index at its root and the engine as a subtree at
//! `engine/`, so `flint examples --check` there needs no flag (its CI runs
//! it). The walk for scenes on disk passes over an engine checkout (that
//! subtree) and `dist/`.
//!
//! The examples are curated and free to change: nothing pins their pixels.
//! What the engine's goldens and CLI tests load are frozen copies under its
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

    /// Check the index against the folder and validate every example;
    /// exits 1 naming each failure
    #[arg(long)]
    pub check: bool,

    /// The folder holding the index.toml. Default: the project root (a
    /// flint-examples checkout's, where the scenes and index are)
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
        let mut text = String::new();
        for failure in &failures {
            text += &format!("FAIL {failure}\n");
        }
        if failures.is_empty() {
            text += &format!(
                "{} examples in {}: every one indexed and valid\n",
                examples.len(),
                shown_dir(&dir)
            );
        } else {
            text += &format!(
                "\n{} examples in {}: {} failure(s)\n",
                examples.len(),
                shown_dir(&dir),
                failures.len()
            );
        }
        let failures: Vec<String> = failures.iter().map(|f| f.to_string()).collect();
        let document = serde_json::json!({
            "dir": shown_dir(&dir),
            "examples": examples.len(),
            "valid": failures.is_empty(),
            "failures": failures,
        });
        let failed = !failures.is_empty();
        return crate::output::emit(&crate::output::Answer::new(text, document).failed_if(failed));
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
    if crate::output::json() {
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
        crate::output::document(&serde_json::Value::Array(list));
        return Ok(());
    }
    print!("{}", listing(&dir, &shown));
    Ok(())
}

/// The project root's folder, when it holds an index: spelled from the
/// working directory (empty when it is the working directory, so the
/// listing shows each scene as the index names it).
fn default_dir() -> Result<PathBuf> {
    let project = flint_core::project::Project::current().context("finding the project")?;
    let root = project.root;
    if !root.join(INDEX_FILE).is_file() {
        bail!(
            "no {INDEX_FILE} at the project root ({}): run this in a flint-examples checkout \
             (https://github.com/chrischaps/flint-examples), or name the index's folder with --dir",
            flint_core::project::shown(&root)
        );
    }
    Ok(from_here(root))
}

/// `dir` from the working directory where it lies under it.
fn from_here(dir: PathBuf) -> PathBuf {
    let Ok(here) = std::env::current_dir() else {
        return dir;
    };
    let canon = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    match canon(&dir).strip_prefix(canon(&here)) {
        Ok(rel) => rel.to_path_buf(),
        Err(_) => dir,
    }
}

/// A folder for a message: `.` for the working directory.
fn shown_dir(dir: &Path) -> String {
    if dir.as_os_str().is_empty() {
        ".".into()
    } else {
        flint_core::project::shown(dir)
    }
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
/// `dist/` (what `flint export web` and `flint bundle` write), hidden
/// folders (`.git`) and an engine checkout (flint-examples' `engine/`
/// subtree, whose scenes are the engine's) are skipped.
fn scenes_on_disk(dir: &Path) -> Vec<String> {
    fn walk(root: &Path, at: &Path, out: &mut Vec<String>) {
        let Ok(entries) = std::fs::read_dir(at) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                let name = entry.file_name();
                if (at == root && name == "dist")
                    || name.to_string_lossy().starts_with('.')
                    || flint_core::project::is_engine_checkout(&path)
                {
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
                "{scene}: not in {INDEX_FILE} (add it; a scene only a test needs belongs with the test)"
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
            Ok((warnings, report, diagnostics)) => {
                for warning in warnings {
                    failures.push(format!("{scene}: warning: {warning}"));
                }
                // An error diagnostic fails the example (ADR 0115): an
                // unknown archetype or component, a value of the wrong
                // type. A warning (a field no schema declares) does not.
                for d in diagnostics.iter().filter(|d| d.is_error()) {
                    let hint = d.hint.as_deref().map(|h| format!(" ({h})"));
                    failures.push(format!("{scene}: {d}{}", hint.unwrap_or_default()));
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
        let out = listing(Path::new("demos"), &refs);
        assert_eq!(
            out,
            "stylize\n\
             \x20 demos/b.scene.toml            b.scene.toml's purpose\n\
             \x20 demos/c/long_name.scene.toml  c/long_name.scene.toml's purpose\n\
             \n\
             showcase\n\
             \x20 demos/a.scene.toml            a.scene.toml's purpose\n\
             \n"
        );
        // The project root's index, from the root itself: each scene as the
        // index names it
        let out = listing(Path::new(""), &refs[1..2]);
        assert_eq!(out, "showcase\n  a.scene.toml  a.scene.toml's purpose\n\n");
    }

    /// flint-examples keeps the engine as a subtree beside its scenes; the
    /// walk for scenes on disk passes over it, `dist/` and `.git`.
    #[test]
    fn the_walk_passes_over_an_engine_checkout() {
        let dir = std::env::temp_dir().join(format!("flint-examples-walk-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for file in [
            "a.scene.toml",
            "film/b.scene.toml",
            "engine/crates/flint-core/Cargo.toml",
            "engine/tests/fixtures/c.scene.toml",
            "dist/d.scene.toml",
            ".git/e.scene.toml",
        ] {
            let path = dir.join(file);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, "").unwrap();
        }
        assert_eq!(scenes_on_disk(&dir), ["a.scene.toml", "film/b.scene.toml"]);
        assert_eq!(shown_dir(Path::new("")), ".");
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
