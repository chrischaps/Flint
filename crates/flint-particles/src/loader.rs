//! Loading `*.particles.toml` effect assets from disk.
//!
//! Effects live in `<scene_dir>/particles/` (falling back to
//! `<scene_dir>/../particles/` for projects that keep scenes in a
//! subdirectory), mirroring how `animations/` holds `*.sequence.toml`.
//! Scenes reference them by `name` through the `particle_effect` component.

use crate::effect::ParticleEffect;
use crate::ParticleSystem;
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
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
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
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    let mut paths: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
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

/// `<scene_dir>/particles/`, falling back to `<scene_dir>/../particles/`.
pub fn resolve_particles_dir(scene_path: &str) -> Option<PathBuf> {
    let scene_dir = Path::new(scene_path)
        .parent()
        .unwrap_or_else(|| Path::new("."));
    let local = scene_dir.join("particles");
    if local.is_dir() {
        return Some(local);
    }
    let up = scene_dir.parent()?.join("particles");
    up.is_dir().then_some(up)
}

/// Directories in which particle textures are searched, most specific
/// first: `particles/`, the scene dir, then its parent (game root).
pub fn texture_search_dirs(scene_path: &str) -> Vec<PathBuf> {
    let scene_dir = Path::new(scene_path)
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .to_path_buf();
    let mut dirs = Vec::new();
    if let Some(p) = resolve_particles_dir(scene_path) {
        dirs.push(p);
    }
    dirs.push(scene_dir.clone());
    if let Some(parent) = scene_dir.parent() {
        dirs.push(parent.to_path_buf());
    }
    dirs
}

/// Register every effect found next to `scene_path` with the system.
/// Returns the number registered; failures are logged and skipped.
pub fn load_particle_effects_from_world(scene_path: &str, system: &mut ParticleSystem) -> usize {
    let Some(dir) = resolve_particles_dir(scene_path) else {
        return 0;
    };
    let mut n = 0;
    for (path, result) in load_effects_from_dir(&dir) {
        match result {
            Ok(fx) => {
                println!(
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
            .join("../../demo")
            .canonicalize()
            .expect("demo dir")
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
    fn resolves_particles_dir_next_to_demo_scene() {
        let scene = demo_dir().join("particles_demo.scene.toml");
        let dir = resolve_particles_dir(scene.to_str().unwrap()).expect("demo/particles exists");
        assert!(dir.ends_with("particles"));
        let dirs = texture_search_dirs(scene.to_str().unwrap());
        assert_eq!(dirs[0], dir);
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
}
