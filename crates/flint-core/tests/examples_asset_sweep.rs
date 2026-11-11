//! Every model, texture, sound, sprite, animation, script (and the modules
//! it imports), procgen spec, prefab, wooze preset, input config, particle
//! texture and path-named file (heightmap, splat map, grass mask, spline,
//! skybox) the engine's own scenes name, through the one resolver (ADR 0114,
//! #706), and with them the look and style assets: the looks, palettes,
//! glyph sets, bitmap fonts (and their images), paper and stroke images
//! scenes, their looks and the stylize goldens' cases name, and every user
//! style and font file a scene can see. None of them may resolve to
//! another file than its kind's old search picked. The old
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

/// A style setting that names a file: `palette`, `font`, `paper_image`,
/// `stroke_image`, in a style's table, a per-entity style component or a
/// `--style` word. `None` for a value that names no file.
fn style_setting(key: &str, value: &str) -> Option<(AssetKind, String)> {
    let kind = match key {
        "palette" => AssetKind::Palettes,
        "font" => AssetKind::GlyphSets,
        "paper_image" => AssetKind::Papers,
        "stroke_image" => AssetKind::Strokes,
        _ => return None,
    };
    (!value.is_empty() && value != "none" && value != "builtin").then(|| (kind, value.to_string()))
}

/// The style assets one table of settings names.
fn style_settings(table: &toml::Table) -> Vec<(AssetKind, String)> {
    table
        .iter()
        .filter_map(|(key, value)| style_setting(key, value.as_str()?))
        .collect()
}

/// The style assets a `[post_process]` block names: its look, and what its
/// style tables name.
fn post_process_assets(post: &toml::Table) -> Vec<(AssetKind, String)> {
    let mut out = Vec::new();
    if let Some(look) = post.get("look").and_then(|l| l.as_str()) {
        out.push((AssetKind::Looks, look.to_string()));
    }
    for style in post
        .get("styles")
        .and_then(|s| s.as_table())
        .into_iter()
        .flat_map(|s| s.values())
        .filter_map(|s| s.as_table())
    {
        out.extend(style_settings(style));
    }
    out
}

/// The look and style assets a scene names: its `[post_process]`, its
/// entities' style components, and the fonts its `ui_text` draws with.
fn styled_by(scene: &toml::Table) -> Vec<(AssetKind, String)> {
    let mut out = Vec::new();
    if let Some(post) = scene.get("post_process").and_then(|p| p.as_table()) {
        out.extend(post_process_assets(post));
    }
    for entity in scene
        .get("entities")
        .and_then(|e| e.as_table())
        .into_iter()
        .flat_map(|e| e.values())
        .filter_map(|e| e.as_table())
    {
        for (component, fields) in entity {
            let Some(fields) = fields.as_table() else {
                continue;
            };
            if component.ends_with("_style") || component == "style_mask" {
                out.extend(style_settings(fields));
            }
        }
        let font = entity
            .get("ui_text")
            .and_then(|t| t.get("font"))
            .and_then(|f| f.as_str())
            .filter(|f| !f.is_empty() && *f != "builtin");
        if let Some(font) = font {
            let kind = match font.ends_with(".font.toml") {
                true => AssetKind::Fonts,
                false => AssetKind::GlyphSets,
            };
            out.push((kind, font.to_string()));
        }
    }
    out
}

/// What a stylize golden's flags name, by scene: `--look`, `--ascii-font`,
/// `--ascii-palette`, the `--style` words and the `--sweep` keys.
fn named_by_goldens(repo: &Path) -> Vec<(PathBuf, AssetKind, String)> {
    let manifest = repo.join("crates/flint-cli/tests/golden/stylize.goldens.toml");
    let doc: toml::Table = std::fs::read_to_string(manifest).unwrap().parse().unwrap();
    let mut out = Vec::new();
    for case in doc["case"].as_array().unwrap() {
        let scene = repo.join(case["scene"].as_str().unwrap());
        let args: Vec<&str> = case
            .get("args")
            .and_then(|a| a.as_array())
            .into_iter()
            .flatten()
            .filter_map(|a| a.as_str())
            .collect();
        let mut named = Vec::new();
        for pair in args.windows(2) {
            let (flag, value) = (pair[0], pair[1]);
            match flag {
                "--look" => named.push((AssetKind::Looks, value.to_string())),
                "--ascii-font" => named.extend(style_setting("font", value)),
                "--ascii-palette" => named.extend(style_setting("palette", value)),
                "--style" => {
                    let settings = value.split_once(':').map(|(_, s)| s).unwrap_or("");
                    for (key, value) in settings.split(',').filter_map(|s| s.split_once('=')) {
                        named.extend(style_setting(key, value));
                    }
                }
                "--sweep" => {
                    let Some((key, values)) = value.split_once('=') else {
                        continue;
                    };
                    for value in values.split(',') {
                        match key {
                            "look" => named.push((AssetKind::Looks, value.to_string())),
                            _ => {
                                let key = key.rsplit('.').next().unwrap_or(key);
                                named.extend(style_setting(key, value));
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        out.extend(named.into_iter().map(|(k, n)| (scene.clone(), k, n)));
    }
    out
}

/// What the look or font description at `file` names in turn: a look's
/// style tables, a `.font.toml`'s image.
fn named_inside(kind: AssetKind, file: &Path) -> Vec<(AssetKind, String)> {
    let Ok(doc) = std::fs::read_to_string(file)
        .unwrap_or_default()
        .parse::<toml::Table>()
    else {
        return Vec::new();
    };
    match kind {
        AssetKind::Looks => doc
            .get("post_process")
            .and_then(|p| p.as_table())
            .map(post_process_assets)
            .unwrap_or_default(),
        AssetKind::Fonts => {
            let font = doc.get("font");
            let texture = font.and_then(|f| f.get("texture")).and_then(|t| t.as_str());
            let set = font.and_then(|f| f.get("glyphs")).and_then(|t| t.as_str());
            texture
                .map(|t| (AssetKind::Textures, t.to_string()))
                .into_iter()
                .chain(set.and_then(|s| style_setting("font", s)))
                .collect()
        }
        _ => Vec::new(),
    }
}

/// The kinds a style or a host loads by listing: every file a scene sees.
const STYLE_KINDS: &[AssetKind] = &[
    AssetKind::Palettes,
    AssetKind::GlyphSets,
    AssetKind::Fonts,
    AssetKind::FontFaces,
    AssetKind::Looks,
    AssetKind::Papers,
    AssetKind::Strokes,
    AssetKind::Styles,
];

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
    // The looks' and styles' old searches also looked in the working
    // directory. The engine's scenes are rendered from its root, so that is
    // where this test stands (it is the only test in this binary).
    std::env::set_current_dir(&repo).unwrap();
    let mut scenes = Vec::new();
    // examples/ with its film sets, the frozen scenes the
    // goldens and CLI tests load (#763), and the goldens' own fixture.
    scenes_under(&repo.join("examples"), &mut scenes);
    scenes_under(&repo.join("tests/fixtures"), &mut scenes);
    scenes_under(&repo.join("crates/flint-cli/tests/golden"), &mut scenes);
    scenes.sort();
    let golden_flags = named_by_goldens(&repo);
    assert!(
        golden_flags.len() > 10,
        "{} golden flags",
        golden_flags.len()
    );
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
        // The looks and style assets the scene and its goldens' flags name.
        named.extend(styled_by(&doc));
        named.extend(
            golden_flags
                .iter()
                .filter(|(golden, _, _)| golden == scene)
                .map(|(_, kind, name)| (*kind, name.clone())),
        );
        named.sort_by(|a, b| (a.0.label(), &a.1).cmp(&(b.0.label(), &b.1)));
        named.dedup();
        // A look names palettes and papers of its own, a font its image.
        let mut at = 0;
        while at < named.len() {
            let (kind, name) = named[at].clone();
            at += 1;
            if !matches!(kind, AssetKind::Looks | AssetKind::Fonts) {
                continue;
            }
            // A built-in look is the file of its name in the repo's styles/
            let file = project
                .explain(kind, &name, scene_dir)
                .found
                .unwrap_or_else(|| repo.join(format!("styles/{name}.look.toml")));
            for inner in named_inside(kind, &file) {
                if !named.contains(&inner) {
                    named.push(inner);
                }
            }
        }
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
        // The style kinds: every palette, glyph set, font, look, paper,
        // stroke and user style the scene can see, which is what the
        // menus offer and the user-style and HUD font loaders take whole.
        for &kind in STYLE_KINDS {
            let listed = project.list(kind, scene_dir);
            let label: &'static str = match kind {
                AssetKind::Palettes => "palettes (visible)",
                AssetKind::GlyphSets => "glyph-sets (visible)",
                AssetKind::Fonts => "fonts (visible)",
                AssetKind::FontFaces => "font-faces (visible)",
                AssetKind::Looks => "looks (visible)",
                AssetKind::Papers => "papers (visible)",
                AssetKind::Strokes => "strokes (visible)",
                _ => "styles (visible)",
            };
            tally.entry(label).or_default().found += listed.len();
            for change in project.list_order_changes(kind, scene_dir) {
                changes.push((scene.clone(), change));
            }
            for file in listed {
                let answer = project.explain(kind, &file.name, scene_dir);
                assert_eq!(answer.found.as_deref(), Some(file.path.as_path()));
            }
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
        "{} scenes under examples/, tests/fixtures/ and the goldens' fixture ({unread} not plain TOML tables)",
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
    // The sweep saw real assets of the kinds entities name, and of the
    // style kinds scenes and goldens name.
    for label in [
        "models",
        "textures",
        "audio",
        "scripts",
        "specs",
        "prefabs",
        "looks",
        "palettes",
        "glyph-sets",
        "fonts",
        "papers",
        "strokes",
        "styles (visible)",
    ] {
        assert!(tally.get(label).is_some_and(|t| t.found > 0), "{label}");
    }
}
