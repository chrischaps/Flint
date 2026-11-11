//! Every model, texture, sound, sprite, animation, script (and the modules
//! it imports), procgen spec, prefab, wooze preset, input config, particle
//! texture and path-named file (heightmap, splat map, grass mask, spline,
//! skybox) the engine's own scenes name, through the one resolver (ADR 0114,
//! #706): none of them may resolve to another file than its kind's old
//! search picked. The old
//! searches are the resolver's `old_rule`s, so an `order_change` here is a
//! demo that would load a different file than before.
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
    let entities = scene.get("entities").and_then(|e| e.as_table());
    for entity in entities
        .into_iter()
        .flat_map(|e| e.values())
        .filter_map(|e| e.as_table())
    {
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
        out.extend(text("script", "source").map(|n| (AssetKind::Scripts, n)));
        for (component, key) in [
            ("terrain", "heightmap"),
            ("terrain", "splat_map"),
            ("grass_ground", "mask"),
            ("spline", "file"),
        ] {
            out.extend(text(component, key).map(|n| (AssetKind::Files, n)));
        }
        let emitter = text("particle_emitter", "texture");
        out.extend(emitter.map(|n| (AssetKind::ParticleTextures, n)));
    }
    // What the scene's own tables name.
    let top = |table: &str, key: &str| -> Option<String> {
        let value = scene.get(table)?.get(key)?.as_str()?;
        (!value.is_empty()).then(|| value.to_string())
    };
    out.extend(top("environment", "skybox").map(|n| (AssetKind::Files, n)));
    out.extend(top("scene", "input_config").map(|n| (AssetKind::Files, n)));
    out.extend(top("wooze", "preset").map(|n| (AssetKind::WoozePresets, n)));
    // What the scene's prefab instances are made from.
    for instance in scene
        .get("prefabs")
        .and_then(|p| p.as_table())
        .into_iter()
        .flat_map(|p| p.values())
    {
        let template = instance.get("template").and_then(|t| t.as_str());
        out.extend(template.map(|n| (AssetKind::Prefabs, n.to_string())));
    }
    out.sort_by(|a, b| (a.0.label(), &a.1).cmp(&(b.0.label(), &b.1)));
    out.dedup();
    out
}

/// The modules a script imports: `import "name"`.
fn imports(script: &str) -> Vec<String> {
    script
        .split("import \"")
        .skip(1)
        .filter_map(|rest| rest.split('"').next())
        .map(str::to_string)
        .collect()
}

/// Whether the two-level search these kinds had would have found nothing:
/// the kind's folder beside the scene or one level up, the first that exists
/// for scripts, both for prefabs. Only for the kinds a file found now where
/// none was found is worth a line (a script that did not run, a prefab that
/// failed the load).
fn old_search_found_none(kind: AssetKind, name: &str, scene_dir: &Path) -> bool {
    let folder = match kind {
        AssetKind::Scripts => "scripts",
        AssetKind::Prefabs => "prefabs",
        _ => return false,
    };
    let dirs: Vec<PathBuf> = [Some(scene_dir), scene_dir.parent()]
        .into_iter()
        .flatten()
        .map(|d| d.join(folder))
        .filter(|d| d.is_dir())
        .collect();
    let dirs = if kind == AssetKind::Scripts {
        &dirs[..dirs.len().min(1)]
    } else {
        &dirs[..]
    };
    let file = kind.info().file_names(name).remove(0);
    !dirs.iter().any(|d| d.join(&file).is_file())
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
    scenes_under(&repo.join("demo"), &mut scenes);
    scenes_under(&repo.join("testGame"), &mut scenes);
    scenes.sort();
    assert!(scenes.len() > 50, "found only {} scenes", scenes.len());

    let mut tally: BTreeMap<&'static str, Tally> = BTreeMap::new();
    let mut changes: Vec<(PathBuf, OrderChange)> = Vec::new();
    let mut unread = 0;
    // Names with no file under either rule: procgen specs make them, or a
    // scene is a bench for a missing asset.
    let mut no_file = std::collections::BTreeSet::new();
    // Found now where the kind's old search found nothing: not a change of
    // order, but for a script it is one that did not run before.
    let mut newly_found = std::collections::BTreeSet::new();
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
        let mut named = named_by(&doc);
        // A model or texture name may be a procgen spec's.
        let specs: Vec<String> = project
            .list(AssetKind::Specs, scene_dir)
            .into_iter()
            .map(|l| l.name)
            .collect();
        let made: Vec<(AssetKind, String)> = named
            .iter()
            .filter(|(kind, name)| {
                matches!(kind, AssetKind::Models | AssetKind::Textures) && specs.contains(name)
            })
            .map(|(_, name)| (AssetKind::Specs, name.clone()))
            .collect();
        named.extend(made);
        // A script's imports are scripts too: `lib/<module>.rhai`.
        let mut at = 0;
        while at < named.len() {
            let (kind, name) = named[at].clone();
            at += 1;
            if kind != AssetKind::Scripts {
                continue;
            }
            let Some(file) = project.explain(kind, &name, scene_dir).found else {
                continue;
            };
            for module in imports(&std::fs::read_to_string(file).unwrap_or_default()) {
                let import = (AssetKind::Scripts, format!("lib/{module}.rhai"));
                if !named.contains(&import) {
                    named.push(import);
                }
            }
        }
        for (kind, name) in named {
            let answer = project.explain(kind, &name, scene_dir);
            let entry = tally.entry(kind.label()).or_default();
            match &answer.found {
                Some(found) => {
                    entry.found += 1;
                    if old_search_found_none(kind, &name, scene_dir) {
                        newly_found.insert(format!(
                            "{}: {} {name} -> {}",
                            shown(scene.strip_prefix(&repo).unwrap_or(scene)),
                            kind.label(),
                            shown(found.strip_prefix(&repo).unwrap_or(found)),
                        ));
                    }
                }
                None => {
                    entry.missing += 1;
                    no_file.insert(format!("{} {name}", kind.label()));
                }
            }
            changes.extend(answer.order_change.map(|c| (scene.clone(), c)));
        }
        // The textures the visible effect files name.
        for effect in project.list(AssetKind::Particles, scene_dir) {
            let text = std::fs::read_to_string(&effect.path).unwrap_or_default();
            for line in text
                .lines()
                .filter(|l| l.trim_start().starts_with("texture"))
            {
                let Some(name) = line.split('"').nth(1).filter(|n| !n.is_empty()) else {
                    continue;
                };
                let kind = AssetKind::ParticleTextures;
                let answer = project.explain(kind, name, scene_dir);
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
        }
        // The game layer of the input config, when the scene names none.
        let default_input = project.explain(AssetKind::Files, "config/input.toml", scene_dir);
        changes.extend(default_input.order_change.map(|c| (scene.clone(), c)));
        for change in project.list_order_changes(AssetKind::WoozePresets, scene_dir) {
            changes.push((scene.clone(), change));
        }
        // Specs are loaded as a whole: every one the scene can see.
        tally.entry("specs (visible)").or_default().found += specs.len();
        for change in project.list_order_changes(AssetKind::Specs, scene_dir) {
            changes.push((scene.clone(), change));
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
        "{} scenes under demo/ and testGame/ ({unread} not plain TOML tables)",
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
    println!(
        "found now where the old search found none: {}",
        newly_found.len()
    );
    for line in &newly_found {
        println!("  {line}");
    }
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
    for label in ["models", "textures", "audio", "scripts", "specs", "prefabs"] {
        assert!(tally.get(label).is_some_and(|t| t.found > 0), "{label}");
    }
}
