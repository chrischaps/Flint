//! Every model, texture, sound, sprite and animation file the engine's own
//! scenes name, through the one resolver (ADR 0114, #706): none of them may
//! resolve to another file than its kind's old search picked. The old
//! searches are the resolver's `old_rule`s, so an `order_change` here is a
//! scene that would load a different file than before.
//!
//! Run with `--nocapture` for the table.

use flint_core::project::{shown, AssetKind, OrderChange, Project};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn scenes_under(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            scenes_under(&path, out);
        } else if path.to_string_lossy().ends_with(".scene.toml") {
            out.push(path);
        }
    }
}

/// The assets a scene's entities name, by kind.
fn named_by(scene: &toml::Table) -> Vec<(AssetKind, String)> {
    let mut out = Vec::new();
    let Some(entities) = scene.get("entities").and_then(|e| e.as_table()) else {
        return out;
    };
    for entity in entities.values().filter_map(|e| e.as_table()) {
        let text = |component: &str, key: &str| -> Option<String> {
            let value = entity.get(component)?.get(key)?.as_str()?;
            (!value.is_empty()).then(|| value.to_string())
        };
        let runtime = entity
            .get("model")
            .and_then(|m| m.get("runtime"))
            .and_then(|r| r.as_bool())
            .unwrap_or(false);
        if !runtime {
            out.extend(text("model", "asset").map(|n| (AssetKind::Models, n)));
        }
        out.extend(text("material", "texture").map(|n| (AssetKind::Textures, n)));
        out.extend(text("sprite", "texture").map(|n| (AssetKind::Textures, n)));
        out.extend(text("audio_source", "file").map(|n| (AssetKind::Audio, n)));
    }
    out.sort_by(|a, b| (a.0.label(), &a.1).cmp(&(b.0.label(), &b.1)));
    out.dedup();
    out
}

#[derive(Default)]
struct Tally {
    found: usize,
    missing: usize,
}

#[test]
fn the_engines_own_scenes_load_the_files_they_loaded_before() {
    let repo = repo();
    let mut scenes = Vec::new();
    scenes_under(&repo.join("examples"), &mut scenes);
    scenes_under(&repo.join("testGame"), &mut scenes);
    scenes_under(&repo.join("tests/fixtures"), &mut scenes);
    scenes.sort();
    assert!(scenes.len() > 50, "found only {} scenes", scenes.len());

    let mut tally: BTreeMap<&'static str, Tally> = BTreeMap::new();
    let mut changes: Vec<(PathBuf, OrderChange)> = Vec::new();
    let mut unread = 0;
    // Names with no file under either rule: procgen specs make them, or a
    // scene is a bench for a missing asset.
    let mut no_file = std::collections::BTreeSet::new();
    for scene in &scenes {
        let project = Project::for_scene(scene);
        let scene_dir = scene.parent().unwrap();
        let Ok(doc) = std::fs::read_to_string(scene)
            .unwrap()
            .parse::<toml::Table>()
        else {
            unread += 1;
            continue;
        };
        // What entities name, one at a time.
        for (kind, name) in named_by(&doc) {
            let answer = project.explain(kind, &name, scene_dir);
            let entry = tally.entry(kind.label()).or_default();
            match answer.found {
                Some(_) => entry.found += 1,
                None => {
                    entry.missing += 1;
                    no_file.insert(format!("{} {name}", kind.label()));
                }
            }
            changes.extend(answer.order_change.map(|c| (scene.clone(), c)));
        }
        // What is loaded as a whole folder, and the UI sprites (which only
        // scripts name): every file the scene can see.
        for kind in [
            AssetKind::Animations,
            AssetKind::SpriteAnimations,
            AssetKind::Sequences,
            AssetKind::Audio,
            AssetKind::Sprites,
        ] {
            let listed = project.list(kind, scene_dir);
            let label = if kind == AssetKind::Audio {
                "audio (preload)"
            } else {
                kind.label()
            };
            tally.entry(label).or_default().found += listed.len();
            for change in project.list_order_changes(kind, scene_dir) {
                changes.push((scene.clone(), change));
            }
            if kind == AssetKind::Sprites {
                for sprite in listed {
                    let answer = project.explain(kind, &sprite.name, scene_dir);
                    assert_eq!(answer.found.as_deref(), Some(sprite.path.as_path()));
                    changes.extend(answer.order_change.map(|c| (scene.clone(), c)));
                }
            }
        }
    }

    println!(
        "{} scenes under examples/, testGame/ and tests/fixtures/ ({unread} not plain TOML tables)",
        scenes.len()
    );
    println!(
        "{:<20}{:>8}{:>26}",
        "kind", "found", "no file (procgen, etc.)"
    );
    for (label, t) in &tally {
        println!("{label:<20}{:>8}{:>26}", t.found, t.missing);
    }
    println!(
        "no file under the old rule or the new: {}",
        no_file.into_iter().collect::<Vec<_>>().join(", ")
    );
    println!("order changes: {}", changes.len());
    for (scene, change) in &changes {
        println!("  {}: {change}", shown(scene));
    }
    assert!(
        changes.is_empty(),
        "{} asset(s) would load another file than before",
        changes.len()
    );
    // The sweep saw real assets of the kinds entities name.
    for label in ["models", "textures", "audio"] {
        assert!(tally.get(label).is_some_and(|t| t.found > 0), "{label}");
    }
}
