//! Flags shared verbatim between subcommands, embedded via
//! `#[command(flatten)]` so each flag is declared exactly once.
//!
//! Only flags whose semantics *and* help text are identical across the
//! commands belong here — a flag whose doc comment differs per command
//! (e.g. `--ladder`, whose `chart replay` help documents the `--render`
//! coupling) stays declared on each command so `--help` keeps saying the
//! right thing.

/// The manifest/chart/base-dir trio shared by `music chart play` and
/// `music chart replay`.
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
    /// `invert:strength=0.5`, `kuwahara:radius=6`, `ascii:off` (off
    /// everywhere, entities' own `ascii_style` included). Repeatable;
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
    /// Check every `--style` word against `styles`. The value parser lets
    /// an unknown name through, since a user-authored style (#274) is known
    /// only once the scene's folder is: call this with the scene's registry
    /// (`SceneBinding::styles`).
    pub fn check(&self, styles: &flint_render::stylize::StyleRegistry) -> Result<(), String> {
        for spec in &self.styles {
            flint_render::stylize::StyleSpec::parse(spec, styles)?;
        }
        Ok(())
    }

    /// Write every `--style` word into `config`, in order.
    pub fn apply(
        &self,
        config: &mut flint_render::PostProcessConfig,
        styles: &flint_render::stylize::StyleRegistry,
    ) {
        for spec in &self.styles {
            if let Err(e) = flint_render::stylize::apply_style_spec(config, spec, styles) {
                eprintln!("warning: --style {spec}: {e}");
            }
        }
    }

    /// Apply the words to `renderer`'s config and build the styles they
    /// turned on. A no-op without any, so a capture without them is
    /// unchanged. The post time goes in with the frame's inputs
    /// ([`look_time`](Self::look_time)).
    pub fn apply_to_renderer(
        &self,
        renderer: &mut flint_render::SceneRenderer,
        device: &wgpu::Device,
    ) {
        if !self.styles.is_empty() {
            let mut config = renderer.post_process_config().clone();
            self.apply(&mut config, renderer.styles());
            renderer.set_post_process_config(config);
            renderer.ensure_style_resources(device);
        }
    }

    /// `--style-time` as the frame's `look_time`: the post time animated
    /// styles see, held there (#328); none leaves it at the frame's clock.
    pub fn look_time(&self) -> Option<f64> {
        self.style_time.map(f64::from)
    }
}

fn parse_style_spec(s: &str) -> Result<String, String> {
    match flint_render::stylize::StyleSpec::parse(s, &flint_render::stylize::StyleRegistry::new()) {
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

/// The scene at `path`, for a command: [`flint_scene::load_scene`] whose
/// failure names the file ([`flint_scene::load_failed`], #1044), so an
/// agent running a command over a folder of scenes knows which one to
/// open. Every command loads its scene through this, or through
/// [`load_settled`].
pub fn load_scene(
    path: &str,
    registry: &flint_schema::SchemaRegistry,
) -> anyhow::Result<(flint_ecs::FlintWorld, flint_scene::SceneFile)> {
    use anyhow::Context;
    flint_scene::load_scene(path, registry).with_context(|| flint_scene::load_failed(path))
}

/// [`load_scene`], then living props' tables checked and the preset
/// `[wooze] preset` names filled in (`flint_procgen::settle_scene`), as the
/// player loads one: a scene loaded for drawing.
pub fn load_settled(
    path: &str,
    registry: &flint_schema::SchemaRegistry,
) -> anyhow::Result<(flint_ecs::FlintWorld, flint_scene::SceneFile)> {
    use anyhow::Context;
    let (world, mut scene_file) = load_scene(path, registry)?;
    flint_procgen::settle_scene(&mut scene_file, std::path::Path::new(path))
        .with_context(|| flint_scene::load_failed(path))?;
    Ok((world, scene_file))
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

/// The value of a `--max-cost` flag: dollars, `$` allowed, not negative.
/// Here, not in `ai_costs`, so the flag parses alike in a build without the
/// ai package (#1333), whose command then names the package.
pub fn parse_usd(s: &str) -> std::result::Result<f64, String> {
    let v: f64 = s
        .trim()
        .trim_start_matches('$')
        .parse()
        .map_err(|_| format!("'{s}' isn't an amount in USD (e.g. 0.50)"))?;
    if !v.is_finite() || v < 0.0 {
        return Err(format!("--max-cost must be a non-negative amount, not {s}"));
    }
    Ok(v)
}

#[cfg(test)]
mod tests {
    use super::*;
    use flint_render::DebugMode;

    #[test]
    fn usd_amounts_parse() {
        assert_eq!(parse_usd("1.00"), Ok(1.0));
        assert_eq!(parse_usd("$0.50"), Ok(0.5));
        assert!(parse_usd("-1").is_err());
        assert!(parse_usd("lots").is_err());
    }

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
