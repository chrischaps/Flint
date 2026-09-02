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

pub(crate) fn parse_debug_mode(s: &str) -> Result<String, String> {
    match s {
        "wireframe" | "normals" | "depth" | "uv" | "unlit" | "metalrough" => Ok(s.to_string()),
        _ => Err(format!(
            "unknown debug mode '{}'; valid values: wireframe, normals, depth, uv, unlit, metalrough",
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
