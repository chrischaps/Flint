//! Flags shared verbatim between subcommands, embedded via
//! `#[command(flatten)]` so each flag is declared exactly once.
//!
//! Only flags whose semantics *and* help text are identical across the
//! commands belong here — a flag whose doc comment differs per command
//! (e.g. `--ladder`, whose replay-chart help documents the `--render`
//! coupling) stays declared on each command so `--help` keeps saying the
//! right thing.

/// The manifest/chart/base-dir trio shared by `play-chart` and
/// `replay-chart`.
#[derive(clap::Args)]
pub struct ChartCommonArgs {
    /// Path to the suite manifest (.suite.toml)
    pub manifest: String,

    /// Beatmap chart (.chart.toml) for the suite
    #[arg(long)]
    pub chart: String,

    /// Directory the manifest's file paths are relative to (default: cwd)
    #[arg(long)]
    pub base_dir: Option<String>,
}

/// `--style` and `--style-time`, shared by `flint render`, the headless
/// `flint edit --render` paths (#271), `flint play` and the scene viewer
/// (#328, which write the words over every scene load and hot reload).
#[derive(clap::Args, Clone, Debug, Default)]
pub struct StyleArgs {
    /// Turn on a registered style over the scene's post-processing:
    /// `name[:key=value,...]`, e.g. `ascii`, `ascii:cell=12,ramp=blocks`,
    /// `invert:strength=0.5`, `kuwahara:radius=6`, `ascii:off`. Repeatable;
    /// a later word wins. `--style help` lists every style and its keys
    #[arg(long = "style", value_parser = parse_style_spec)]
    pub styles: Vec<String>,

    /// Post time in seconds that animated styles (and grain / render
    /// modes) see; headless captures default to 0, so the same value
    /// gives the same pixels
    #[arg(long)]
    pub style_time: Option<f32>,
}

impl StyleArgs {
    /// Check every `--style` word against the registry. The value parser
    /// lets an unknown name through, since a user-authored style (#274) is
    /// registered only once the scene's folder is known; call this after
    /// `stylize::custom::load_dir`.
    pub fn check(&self) -> Result<(), String> {
        for spec in &self.styles {
            flint_render::stylize::StyleSpec::parse(spec)?;
        }
        Ok(())
    }

    /// Write every `--style` word into `config`, in order.
    pub fn apply(&self, config: &mut flint_render::PostProcessConfig) {
        for spec in &self.styles {
            if let Err(e) = flint_render::stylize::apply_style_spec(config, spec) {
                eprintln!("warning: --style {spec}: {e}");
            }
        }
    }

    /// Apply the words to `renderer`'s config and build the styles they
    /// turned on; set the post time. A no-op without either flag, so a
    /// capture without them is unchanged.
    pub fn apply_to_renderer(
        &self,
        renderer: &mut flint_render::SceneRenderer,
        device: &wgpu::Device,
    ) {
        if !self.styles.is_empty() {
            let mut config = renderer.post_process_config().clone();
            self.apply(&mut config);
            renderer.set_post_process_config(config);
            renderer.ensure_style_resources(device);
        }
        if let Some(t) = self.style_time {
            renderer.ocean_time = t as f64;
        }
    }
}

fn parse_style_spec(s: &str) -> Result<String, String> {
    match flint_render::stylize::StyleSpec::parse(s) {
        Ok(_) => Ok(s.to_string()),
        // Maybe a user style the scene's styles/ folder registers (#274):
        // StyleArgs::check decides once it is loaded
        Err(e) if e.starts_with("unknown style") => Ok(s.to_string()),
        Err(e) => Err(e),
    }
}

/// What `--debug-mode` accepts, and the mode each name selects: the parser
/// and the render paths both read this one table, so a name can't be
/// accepted and then fall through to plain PBR, or matched and never
/// accepted (#308). Every mode but PBR, the default, has exactly one name
/// (held by `debug_mode_names_cover_every_mode`).
pub(crate) const DEBUG_MODES: &[(&str, flint_render::DebugMode)] = {
    use flint_render::DebugMode::*;
    &[
        ("wireframe", WireframeOnly),
        ("wireframe-overlay", WireframeOverlay),
        ("normals", Normals),
        ("depth", Depth),
        ("uv", UvChecker),
        ("unlit", Unlit),
        ("metalrough", MetallicRoughness),
        ("style", StyleMask),
        ("edges", StyleEdges),
    ]
};

pub(crate) fn parse_debug_mode(s: &str) -> Result<flint_render::DebugMode, String> {
    DEBUG_MODES
        .iter()
        .find(|(name, _)| *name == s)
        .map(|&(_, mode)| mode)
        .ok_or_else(|| {
            let names: Vec<&str> = DEBUG_MODES.iter().map(|(name, _)| *name).collect();
            format!(
                "unknown debug mode '{s}'; valid values: {}",
                names.join(", ")
            )
        })
}

/// The `--debug-mode` name of `mode`, for messages that quote the flag back.
pub(crate) fn debug_mode_name(mode: flint_render::DebugMode) -> &'static str {
    DEBUG_MODES
        .iter()
        .find(|&&(_, m)| m == mode)
        .map_or("pbr", |&(name, _)| name)
}

pub(crate) fn parse_vec3(s: &str) -> Result<[f32; 3], String> {
    let parts: Vec<&str> = s.split(',').collect();
    if parts.len() != 3 {
        return Err(format!(
            "expected 3 comma-separated values, got {}",
            parts.len()
        ));
    }
    let x: f32 = parts[0]
        .trim()
        .parse()
        .map_err(|e| format!("invalid x: {}", e))?;
    let y: f32 = parts[1]
        .trim()
        .parse()
        .map_err(|e| format!("invalid y: {}", e))?;
    let z: f32 = parts[2]
        .trim()
        .parse()
        .map_err(|e| format!("invalid z: {}", e))?;
    Ok([x, y, z])
}

pub(crate) fn parse_vec4(s: &str) -> Result<[f32; 4], String> {
    let parts: Vec<&str> = s.split(',').collect();
    if parts.len() != 4 {
        return Err(format!(
            "expected 4 comma-separated values, got {}",
            parts.len()
        ));
    }
    let mut out = [0.0f32; 4];
    for (i, p) in parts.iter().enumerate() {
        out[i] = p
            .trim()
            .parse::<f32>()
            .map_err(|e| format!("invalid number '{}': {}", p, e))?;
    }
    Ok(out)
}

/// The schemas for a scene, or for the working directory when a command
/// has no scene, from the one resolver (ADR 0114): `--schemas` flags if any
/// were given, else the project manifest's `schemas`, else `schemas/` in
/// the working directory, beside the scene and one level up. Returns the
/// directories too, and where the list came from. A directory that a flag
/// or the manifest names and that isn't there is the error, by name.
pub fn load_schemas(
    scene: Option<&str>,
    flags: &[String],
) -> anyhow::Result<(flint_schema::SchemaRegistry, flint_schema::SchemaDirs)> {
    use anyhow::Context;
    match flint_schema::SchemaRegistry::resolve(scene.map(std::path::Path::new), flags) {
        Ok(loaded) => Ok(loaded),
        // Says which directory and who named it: nothing to add
        Err(flint_core::FlintError::SchemaDirs(e)) => Err(e.into()),
        Err(e) => Err(e).context("Failed to load schemas"),
    }
}

/// TEMPORARY, for one release (ADR 0114 decision 3, #765): say on stderr
/// which `schemas/` beside the scene this command used to add to its
/// `--schemas` flags and no longer does. For the commands that did add
/// them: play, render, validate and the spline editor. Never stdout.
pub fn warn_dropped_schemas(dirs: &flint_schema::SchemaDirs) {
    for warning in dirs.dropped_warnings() {
        eprintln!("{warning}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flint_render::DebugMode;

    /// #308: `wireframe-overlay` was matched in render.rs and refused here.
    /// Now there is one table; this holds it to the renderer's modes: every
    /// mode but the default has exactly one name, and no name is repeated.
    #[test]
    fn debug_mode_names_cover_every_mode() {
        for mode in DebugMode::ALL {
            let names: Vec<&str> = DEBUG_MODES
                .iter()
                .filter(|&&(_, m)| m == mode)
                .map(|&(name, _)| name)
                .collect();
            let expected = usize::from(mode != DebugMode::Pbr);
            assert_eq!(
                names.len(),
                expected,
                "{mode:?} has --debug-mode names {names:?}; want {expected}"
            );
        }
        let mut names: Vec<&str> = DEBUG_MODES.iter().map(|(name, _)| *name).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(
            names.len(),
            DEBUG_MODES.len(),
            "a --debug-mode name repeats"
        );
    }

    #[test]
    fn every_listed_name_parses_to_its_mode_and_back() {
        for &(name, mode) in DEBUG_MODES {
            assert_eq!(parse_debug_mode(name), Ok(mode));
            assert_eq!(debug_mode_name(mode), name);
        }
        assert_eq!(
            parse_debug_mode("wireframe-overlay"),
            Ok(DebugMode::WireframeOverlay)
        );
    }

    #[test]
    fn an_unknown_name_lists_every_valid_one() {
        let err = parse_debug_mode("sparkles").unwrap_err();
        for (name, _) in DEBUG_MODES {
            assert!(err.contains(name), "{err:?} does not list {name}");
        }
    }
}
