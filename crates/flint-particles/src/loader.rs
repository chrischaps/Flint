//! Loading `*.particles.toml` effect assets through the vfs.
//!
//! Effects are found by the project's one asset search (ADR 0114,
//! `flint_core::project`): `particles/` beside the scene, then beside each
//! directory above it up to the project root, then in the mounted engine. A
//! nearer file hides a farther one of the same name. Scenes reference them
//! by `name` through the `particle_effect` component.

use crate::effect::ParticleEffect;
use crate::ParticleSystem;
use flint_core::project::{AssetKind, Project};
use flint_core::vfs;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// File suffix for effect assets.
pub const EFFECT_SUFFIX: &str = ".particles.toml";

/// Parse an effect from TOML text; `origin` labels errors.
pub fn load_effect_from_str(text: &str, origin: &str) -> Result<ParticleEffect, String> {
    ParticleEffect::from_toml_str(text, origin)
}

/// Load one effect file. An empty `name` falls back to the file stem
/// (`fire.particles.toml` → `fire`).
pub fn load_effect_from_file(path: &Path) -> Result<ParticleEffect, String> {
    let text = vfs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let origin = path.display().to_string();
    // Allow the name to be omitted: patch it in before strict parsing.
    let mut effect = match load_effect_from_str(&text, &origin) {
        Ok(fx) => fx,
        Err(e) if e.contains("missing field `name`") => {
            let stem = effect_stem(path).unwrap_or_default();
            let patched = format!("name = \"{stem}\"\n{text}");
            load_effect_from_str(&patched, &origin)?
        }
        Err(e) => return Err(e),
    };
    if effect.name.is_empty() {
        effect.name = effect_stem(path).unwrap_or_default();
    }
    Ok(effect)
}

/// `fire.particles.toml` → `Some("fire")`.
pub fn effect_stem(path: &Path) -> Option<String> {
    let name = path.file_name()?.to_str()?;
    name.strip_suffix(EFFECT_SUFFIX).map(|s| s.to_string())
}

/// Load every `*.particles.toml` in a directory (non-recursive), returning
/// each path with its result so callers can report failures individually.
pub fn load_effects_from_dir(dir: &Path) -> Vec<(PathBuf, Result<ParticleEffect, String>)> {
    let mut out = Vec::new();
    let Ok(entries) = vfs::read_dir(dir) else {
        return out;
    };
    let mut paths: Vec<PathBuf> = entries
        .into_iter()
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.ends_with(EFFECT_SUFFIX))
        })
        .collect();
    paths.sort();
    for path in paths {
        let result = load_effect_from_file(&path);
        out.push((path, result));
    }
    out
}

/// The directory a scene file is in (`.` for a bare file name).
fn scene_dir_of(scene_path: &str) -> &Path {
    Path::new(scene_path)
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
}

/// Every effect file a scene can see, nearest first, each with what loading
/// it gave: `project.list(AssetKind::Particles, ..)`, loaded.
pub fn load_visible_effects(
    project: &Project,
    scene_path: &str,
) -> Vec<(PathBuf, Result<ParticleEffect, String>)> {
    project
        .list(AssetKind::Particles, scene_dir_of(scene_path))
        .into_iter()
        .map(|listed| {
            let result = load_effect_from_file(&listed.path);
            (listed.path, result)
        })
        .collect()
}

/// Register every effect `scene_path` can see with the system, and remember
/// where they were searched for, so a missing effect's warning can list the
/// paths tried. Returns the number registered; failures are logged and
/// skipped. An effect is registered under the `name` its file gives, and
/// where two visible files give the same name the nearer one is kept.
pub fn load_particle_effects_from_world(
    project: &Project,
    scene_path: &str,
    system: &mut ParticleSystem,
) -> usize {
    system
        .sync
        .set_search(project.clone(), scene_dir_of(scene_path).to_path_buf());
    let mut names = HashSet::new();
    let mut n = 0;
    for (path, result) in load_visible_effects(project, scene_path) {
        match result {
            Ok(fx) => {
                if !names.insert(fx.name.clone()) {
                    tracing::warn!(
                        "particle effect '{}' in {} is hidden by a nearer file giving the same name",
                        fx.name,
                        path.display()
                    );
                    continue;
                }
                eprintln!(
                    "Loaded particle effect: {} ({} emitter{})",
                    fx.name,
                    fx.emitters.len(),
                    if fx.emitters.len() == 1 { "" } else { "s" }
                );
                system.sync.register_effect(Arc::new(fx));
                n += 1;
            }
            Err(e) => tracing::warn!("Failed to load particle effect '{}': {e}", path.display()),
        }
    }
    n
}

#[cfg(test)]
mod tests {
    use super::*;

    fn demo_dir() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../examples")
            .canonicalize()
            .expect("examples dir")
    }

    #[test]
    fn effect_stem_strips_suffix() {
        assert_eq!(
            effect_stem(Path::new("a/b/fire.particles.toml")),
            Some("fire".into())
        );
        assert_eq!(effect_stem(Path::new("fire.toml")), None);
    }

    #[test]
    fn loads_demo_campfire_effect() {
        let path = demo_dir().join("particles/campfire.particles.toml");
        let fx = load_effect_from_file(&path).expect("campfire parses");
        assert_eq!(fx.name, "campfire");
        assert!(fx.emitters.len() >= 3);
        assert!(fx.emitter_index("flames").is_some());
        fx.validate().unwrap();
    }

    #[test]
    fn loads_every_demo_effect() {
        let results = load_effects_from_dir(&demo_dir().join("particles"));
        assert!(!results.is_empty());
        for (path, r) in results {
            r.unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        }
    }

    #[test]
    fn missing_name_falls_back_to_stem() {
        let dir = std::env::temp_dir().join(format!("flint_fx_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("glow.particles.toml");
        std::fs::write(&path, "[[emitters]]\nname = \"a\"\n").unwrap();
        let fx = load_effect_from_file(&path).unwrap();
        assert_eq!(fx.name, "glow");
        let _ = std::fs::remove_dir_all(&dir);
    }
    /// A memory file source for the one thread, with `files` in it.
    fn with_files<R>(files: &[(&str, &str)], f: impl FnOnce() -> R) -> R {
        let mut source = vfs::MemorySource::new();
        for (path, text) in files {
            source.insert(path, *text);
        }
        vfs::scoped(Arc::new(source), f)
    }

    const ONE: &str = "[[emitters]]\nname = \"a\"\n";

    #[test]
    fn a_nested_scenes_particle_effect_loads_and_plays() {
        use flint_runtime::RuntimeSystem;
        let scene = "game/scenes/act1/cave/a.scene.toml";
        with_files(
            &[
                ("game/flint.toml", ""),
                ("game/particles/fire.particles.toml", ONE),
                ("game/engine/particles/dust.particles.toml", ONE),
                (scene, ""),
            ],
            || {
                let project = Project::for_scene(Path::new(scene));
                let mut system = ParticleSystem::new();
                assert_eq!(
                    load_particle_effects_from_world(&project, scene, &mut system),
                    2
                );
                assert_eq!(system.sync.effect_names(), ["dust", "fire"]);

                let mut world = flint_ecs::FlintWorld::new();
                let id = world.spawn("torch").unwrap();
                let component: toml::Value = toml::toml! { effect = "fire" }.into();
                world
                    .set_component(id, "particle_effect", component)
                    .unwrap();
                system.initialize(&mut world).unwrap();
                assert_eq!(system.sync.emitter_count(), 1, "the root's effect is live");
            },
        );
    }

    #[test]
    fn the_nearer_effect_hides_the_farther_one() {
        let scene = "game/scenes/a.scene.toml";
        let near = "seed = 7\n[[emitters]]\nname = \"near\"\n";
        with_files(
            &[
                ("game/flint.toml", ""),
                ("game/particles/fire.particles.toml", ONE),
                ("game/particles/rain.particles.toml", ONE),
                ("game/scenes/particles/fire.particles.toml", near),
                (scene, ""),
            ],
            || {
                let project = Project::for_scene(Path::new(scene));
                let mut system = ParticleSystem::new();
                assert_eq!(
                    load_particle_effects_from_world(&project, scene, &mut system),
                    2
                );
                let fire = system.sync.effect("fire").expect("fire");
                assert_eq!(fire.emitters[0].name, "near");
                assert!(system.sync.effect("rain").is_some());
            },
        );
    }

    #[test]
    fn a_missing_effect_says_where_it_looked() {
        let scene = "game/scenes/act1/a.scene.toml";
        let renamed = "name = \"other\"\n[[emitters]]\nname = \"a\"\n";
        with_files(
            &[
                ("game/flint.toml", ""),
                ("game/particles/renamed.particles.toml", renamed),
                (scene, ""),
            ],
            || {
                let project = Project::for_scene(Path::new(scene));
                let mut system = ParticleSystem::new();
                load_particle_effects_from_world(&project, scene, &mut system);
                let report = system.sync.missing_effect_report("ice");
                let lines: Vec<&str> = report.lines().collect();
                assert_eq!(lines[0], "particle effect 'ice' not found. Tried:");
                let tried: Vec<PathBuf> =
                    lines[1..].iter().map(|l| PathBuf::from(l.trim())).collect();
                assert_eq!(
                    tried,
                    [
                        PathBuf::from("game/scenes/act1/particles/ice.particles.toml"),
                        PathBuf::from("game/scenes/particles/ice.particles.toml"),
                        PathBuf::from("game/particles/ice.particles.toml"),
                    ]
                );
                // A file that is there but calls itself something else.
                let report = system.sync.missing_effect_report("renamed");
                assert!(report.contains("is not loaded, though"), "{report}");
                assert!(report.contains("renamed.particles.toml"), "{report}");
            },
        );
    }
}
