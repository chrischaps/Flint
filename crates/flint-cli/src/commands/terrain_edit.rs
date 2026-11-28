//! `flint terrain-edit <spec>` (a hidden alias of `flint edit`): the
//! kernel's command, in every build (#1623). The editor is the terrain
//! package's, opened through the [`TerrainEditors`] point it registers; a
//! flint without it names the package.

use anyhow::Result;
use clap::Args;
use flint_package::cli::Points;
use flint_viewer::host::{self, HostOptions, SpecEdit, TerrainEditors};

#[derive(Args)]
pub struct TerrainEditArgs {
    /// Path to a .terrain.toml spec file (created if it doesn't exist)
    pub spec: String,

    /// Override the spec's seed with a fixed value
    #[arg(long)]
    pub seed: Option<u64>,

    /// Initial window width
    #[arg(long, default_value = "1440")]
    pub width: u32,

    /// Initial window height
    #[arg(long, default_value = "900")]
    pub height: u32,

    /// Disable the ground grid
    #[arg(long)]
    pub no_grid: bool,
}

/// The terrain editor this run has, or the error naming the package that
/// provides `words` (`terrain-edit`: terrain).
pub fn editors<'p>(points: &'p Points, what: &str, words: &[&str]) -> Result<&'p TerrainEditors> {
    points
        .get::<TerrainEditors>()
        .ok_or_else(|| super::absent::missing_owned(what.to_string(), words))
}

/// `flint terrain-edit <spec>`: the terrain editor in a window.
pub fn run(args: TerrainEditArgs) -> Result<()> {
    run_in(args, crate::points())
}

pub(crate) fn run_in(args: TerrainEditArgs, points: &Points) -> Result<()> {
    let editors = editors(points, "flint terrain-edit", &["terrain-edit"])?;
    let options = HostOptions {
        width: args.width,
        height: args.height,
        camera: Default::default(),
        auto_orbit: false,
    };
    let editor = (editors.open)(SpecEdit {
        spec: args.spec,
        seed: args.seed,
        width: args.width,
        height: args.height,
        no_grid: args.no_grid,
        auto_orbit: false,
    })?;
    host::run(editor, options)
}
