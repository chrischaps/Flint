//! Starter effects. Each is a complete `*.particles.toml` embedded at build
//! time; `bootstrap` writes one to disk when `flint edit new.particles.toml`
//! targets a file that does not exist yet.

use std::path::Path;

use anyhow::{Context, Result};
use flint_particles::{load_effect_from_str, ParticleEffect};

pub const PRESETS: &[(&str, &str)] = &[
    ("fire", include_str!("../../../presets/fire.particles.toml")),
    (
        "smoke",
        include_str!("../../../presets/smoke.particles.toml"),
    ),
    (
        "sparks",
        include_str!("../../../presets/sparks.particles.toml"),
    ),
    ("rain", include_str!("../../../presets/rain.particles.toml")),
];

pub fn names() -> impl Iterator<Item = &'static str> {
    PRESETS.iter().map(|(n, _)| *n)
}

/// Parsed preset by name.
pub fn preset(name: &str) -> Option<ParticleEffect> {
    PRESETS
        .iter()
        .find(|(n, _)| *n == name)
        .and_then(|(n, text)| load_effect_from_str(text, n).ok())
}

/// Create `path` from a preset, naming the effect after the file stem.
pub fn bootstrap(path: &Path, preset_name: &str) -> Result<ParticleEffect> {
    let (_, text) = PRESETS
        .iter()
        .find(|(n, _)| *n == preset_name)
        .with_context(|| {
            format!(
                "unknown preset '{preset_name}' (available: {})",
                names().collect::<Vec<_>>().join(", ")
            )
        })?;
    let stem = flint_particles::loader::effect_stem(path).unwrap_or_else(|| "effect".to_string());
    let text = text.replacen(
        &format!("name = \"{preset_name}\""),
        &format!("name = \"{stem}\""),
        1,
    );
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
    }
    std::fs::write(path, &text).with_context(|| format!("writing {}", path.display()))?;
    load_effect_from_str(&text, &path.display().to_string()).map_err(|e| anyhow::anyhow!(e))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_preset_parses_and_validates() {
        for (name, text) in PRESETS {
            let fx = load_effect_from_str(text, name).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert_eq!(fx.name, *name);
            fx.validate().unwrap();
        }
    }
}
