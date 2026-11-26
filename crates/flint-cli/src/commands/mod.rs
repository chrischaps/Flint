//! CLI command implementations
//!
//! A module that names a package's crate is compiled only with that
//! package's feature (#1333). A package's own commands are its crate's
//! extension (#1553, #1555), and a kernel command it fills in asks its
//! point (#1557): without the package, `absent` names it, with exit 2.

pub mod absent;
pub mod asset;
pub mod asset_where;
pub mod attach;
pub mod build;
pub mod bundle;
#[cfg(feature = "rhythm")]
pub mod calibrate;
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
// `flint gen` is the kernel's in every build: a glyph set runs here, a
// spec through the procgen package's point (#1543, #1557), which asks the
// ai package's steps (`gen_ai`) and living's strip (`gen_strip`) by points
// of its own
pub mod gen;
#[cfg(feature = "ai")]
pub mod gen_ai;
pub mod gen_args;
#[cfg(feature = "ai")]
pub mod gen_refine;
/// The GIF writer, the Package API's since #1558 (living's `gen --strip`
/// writes with it too)
pub use flint_package::cli::gif as gif_writer;
pub mod glyph_preview;
pub mod golden_sheet;
pub mod init;
pub mod look;
pub mod look_measure;
#[cfg(feature = "ai")]
pub mod look_refine;
pub mod mesh_seq;
// The rhythm package's commands under the music package's `music`
// (#1555; its own crate takes them, #1556)
#[cfg(feature = "rhythm")]
pub mod music;
pub mod package;
pub mod particle_edit;
pub mod play;
#[cfg(feature = "rhythm")]
pub mod play_chart;
#[cfg(feature = "rhythm")]
pub mod play_chart_window;
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
pub mod render_watch;
#[cfg(feature = "rhythm")]
pub mod replay_chart;
pub mod scene;
pub mod schema;
pub mod schema_export;
pub mod script;
// `gen-preview` / `tex-edit`: the kernel's hidden aliases of `flint edit`
// on a spec, whose editors are the procgen package's (#1557)
pub mod spec_edit;
pub mod spline_edit;
pub mod style;
pub mod style_sheet;
pub mod sweep;
pub mod terrain_edit;
pub(crate) mod trace;
pub mod validate;
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
