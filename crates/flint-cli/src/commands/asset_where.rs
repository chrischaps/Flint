//! `flint asset where <kind> <name> [--scene x] [--json]` (ADR 0114, #706):
//! the engine's own answer to "which file would you load?", from the same
//! resolver every loader uses (`flint_core::project::Project::explain`).

use anyhow::{bail, Context, Result};
use flint_core::project::{shown, AssetKind, Project, Resolution};
use std::path::{Path, PathBuf};

/// The kinds, for `--help` and for a kind nobody knows: label, folders and
/// extensions, straight from the resolver's table.
pub fn kinds_help() -> String {
    let mut text = String::from("Kinds:\n");
    for kind in AssetKind::ALL {
        let info = kind.info();
        let folders: Vec<String> = info.folders.iter().map(|f| format!("{f}/")).collect();
        text.push_str(&format!(
            "  {:<12}{}<name>{}\n",
            info.label,
            folders.join(" | "),
            info.extensions.join(" | ")
        ));
    }
    text.push_str(
        "\nSearched in order: beside the scene, beside each directory above it up to the \
         project root, then in the mounted engine (<root>/engine/).",
    );
    text
}

/// clap's parser for the kind argument: an unknown one lists the kinds.
pub fn parse_kind(label: &str) -> Result<AssetKind, String> {
    AssetKind::from_label(label)
        .ok_or_else(|| format!("no asset kind is called '{label}'\n\n{}", kinds_help()))
}

pub fn run(kind: AssetKind, name: &str, scene: Option<&str>, json: bool) -> Result<()> {
    let (project, scene_dir) = match scene {
        Some(scene) => {
            let path = Path::new(scene);
            let project = Project::for_path(path).with_context(|| format!("--scene {scene}"))?;
            (project, scene_dir_of(path))
        }
        None => (Project::current()?, PathBuf::from(".")),
    };
    let answer = project.explain(kind, name, &scene_dir);

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&to_json(&project, &scene_dir, &answer))?
        );
        if answer.found.is_none() {
            // The document is the answer; the exit code says there was none.
            std::process::exit(1);
        }
        return Ok(());
    }

    println!("{} '{}'", kind.info().noun, name);
    println!(
        "  project  {} ({})",
        shown(&project.root),
        project.found.describe()
    );
    println!("  scene    {}", shown(&scene_dir));
    match &answer.found {
        Some(path) => {
            println!("  loads    {}", shown(path));
            if !answer.tried.is_empty() {
                println!("  tried before it:");
            }
        }
        None => println!("  loads    nothing: no such file. Tried:"),
    }
    for path in &answer.tried {
        println!("    {}", shown(path));
    }
    if let Some(change) = &answer.order_change {
        println!("  note     {change}");
    }
    if answer.found.is_none() {
        bail!(
            "{} '{}' not found ({} place{} tried)",
            kind.info().noun,
            name,
            answer.tried.len(),
            if answer.tried.len() == 1 { "" } else { "s" }
        );
    }
    Ok(())
}

/// The directory a scene is in. A directory given as `--scene` is taken as
/// that directory.
fn scene_dir_of(scene: &Path) -> PathBuf {
    if scene.is_dir() {
        return scene.to_path_buf();
    }
    match scene.parent() {
        Some(dir) if !dir.as_os_str().is_empty() => dir.to_path_buf(),
        _ => PathBuf::from("."),
    }
}

fn to_json(project: &Project, scene_dir: &Path, answer: &Resolution) -> serde_json::Value {
    let text = shown;
    serde_json::json!({
        "kind": answer.kind.label(),
        "name": answer.name,
        "scene_dir": text(scene_dir),
        "project": {
            "root": text(&project.root),
            "found": project.found.describe(),
        },
        "found": answer.found.as_deref().map(text),
        "tried": answer.tried.iter().map(|p| text(p)).collect::<Vec<_>>(),
        "order_change": answer.order_change.as_ref().map(|change| serde_json::json!({
            "old": text(&change.old),
            "new": change.new.as_deref().map(text),
            "message": change.to_string(),
        })),
    })
}
