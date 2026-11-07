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

/// `--style` and `--style-time`, shared by `flint render` and the
/// headless `flint edit --render` paths (#271).
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
    /// Write every `--style` word into `config`, in order.
    pub fn apply(&self, config: &mut flint_render::PostProcessConfig) {
        for spec in &self.styles {
            // Checked by the value parser
            let _ = flint_render::stylize::apply_style_spec(config, spec);
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
    flint_render::stylize::StyleSpec::parse(s).map(|_| s.to_string())
}

pub(crate) fn parse_debug_mode(s: &str) -> Result<String, String> {
    match s {
        "wireframe" | "normals" | "depth" | "uv" | "unlit" | "metalrough" | "style" => Ok(s.to_string()),
        _ => Err(format!(
            "unknown debug mode '{}'; valid values: wireframe, normals, depth, uv, unlit, metalrough, style",
            s
        )),
    }
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
