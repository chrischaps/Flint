//! CLI command implementations

pub mod asset;
pub mod calibrate;
pub mod common_args;
pub mod edit_router;
pub mod entity;
pub mod gen;
pub mod gen_preview;
pub mod gen_preview_common;
pub mod init;
pub mod particle_edit;
pub mod play;
pub mod play_chart;
pub mod play_chart_window;
pub mod play_suite;
pub mod prefab;
pub mod preview;
pub mod query;
pub mod render;
pub mod render_suite;
pub mod replay_chart;
pub mod scene;
pub mod schema;
pub mod spike_rumble;
pub mod spline_edit;
pub mod terrain_edit;
pub mod tex_edit;
pub mod validate;
pub mod validate_suite;

/// Write a report into `<base_dir>/logs/latency/` as
/// `<prefix>-<host>-<epoch>.toml` — the shared naming scheme the offset
/// readers sort lexicographically (name embeds host + epoch, so max =
/// newest). One writer for calibrate, spike-rumble, and the input spike.
pub fn write_latency_report(
    base_dir: &std::path::Path,
    prefix: &str,
    contents: &str,
) -> anyhow::Result<std::path::PathBuf> {
    use anyhow::Context;
    let dir = base_dir.join("logs/latency");
    std::fs::create_dir_all(&dir).context("creating logs/latency")?;
    let host = std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_else(|_| "host".into());
    let epoch = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let path = dir.join(format!("{prefix}-{host}-{epoch}.toml"));
    std::fs::write(&path, contents).with_context(|| format!("writing {path:?}"))?;
    Ok(path)
}
