//! CLI command implementations
//!
//! A module that names a package's crate is compiled only with that
//! package's feature (#1333); without it, `absent::stub!` stands in for a
//! command's file, so the parser keeps the command and running it exits 2.

pub mod absent;
#[cfg(feature = "ai")]
pub mod ai_costs;
pub mod asset;
pub mod asset_where;
pub mod attach;
pub mod build;
pub mod bundle;
#[cfg(feature = "rhythm")]
pub mod calibrate;
#[cfg(not(feature = "rhythm"))]
absent::stub!(calibrate, CalibrateArgs, "music calibrate", "rhythm");
pub mod capture;
mod capture_compare;
mod capture_disk;
pub mod check;
pub mod common_args;
pub mod dxc;
pub mod edit_router;
pub mod entity;
pub mod env;
pub mod examples;
pub mod export;
pub mod frame;
pub mod frame_seq;
pub mod gen;
#[cfg(feature = "ai")]
pub mod gen_ai;
pub mod gen_catalog;
pub mod gen_preview;
pub mod gen_preview_common;
#[cfg(feature = "ai")]
pub mod gen_refine;
pub mod gen_store;
#[cfg(feature = "living")]
pub mod gen_strip;
pub mod gif_writer;
pub mod glyph_preview;
pub mod golden_sheet;
pub mod init;
pub mod look;
pub mod look_measure;
#[cfg(feature = "ai")]
pub mod look_refine;
pub mod mesh_seq;
pub mod music;
pub mod package;
pub mod particle_edit;
pub mod play;
#[cfg(feature = "rhythm")]
pub mod play_chart;
#[cfg(not(feature = "rhythm"))]
absent::stub!(play_chart, PlayChartArgs, "music chart play", "rhythm");
#[cfg(feature = "rhythm")]
pub mod play_chart_window;
#[cfg(feature = "music")]
pub mod play_suite;
#[cfg(not(feature = "music"))]
absent::stub!(play_suite, PlaySuiteArgs, "music play", "music");
pub mod playtest;
pub mod prefab;
pub mod preview;
pub mod profile;
pub mod query;
pub mod render;
pub mod render_args;
mod render_frame_dump;
pub mod render_goldens;
pub mod render_plan;
pub mod render_post;
pub mod render_report;
pub mod render_retired;
#[cfg(feature = "music")]
pub mod render_suite;
#[cfg(not(feature = "music"))]
absent::stub!(render_suite, RenderSuiteArgs, "music render", "music");
pub mod render_watch;
#[cfg(feature = "rhythm")]
pub mod replay_chart;
#[cfg(not(feature = "rhythm"))]
absent::stub!(
    replay_chart,
    ReplayChartArgs,
    "music chart replay",
    "rhythm"
);
pub mod scene;
pub mod schema;
pub mod schema_export;
pub mod script;
pub mod spec_save;
pub mod spline_edit;
pub mod style;
pub mod style_sheet;
pub mod sweep;
pub mod terrain_edit;
pub mod tex_edit;
pub(crate) mod trace;
pub mod validate;
#[cfg(feature = "music")]
pub mod validate_suite;
#[cfg(not(feature = "music"))]
absent::stub!(validate_suite, ValidateSuiteArgs, "music validate", "music");
pub mod variants;

/// Write a report into `<base_dir>/logs/latency/` as
/// `<prefix>-<host>-<epoch>.toml` — the shared naming scheme the offset
/// readers sort lexicographically (name embeds host + epoch, so max =
/// newest). One writer for `music calibrate` and the input spike
/// (`music chart play --spike-input-secs`); the rumble spike, an example
/// of flint-input-capture since #709, writes its own.
#[cfg(feature = "rhythm")]
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
