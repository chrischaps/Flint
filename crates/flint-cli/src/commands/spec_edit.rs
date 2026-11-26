//! `flint gen-preview <spec>` and `flint tex-edit <spec>` (hidden aliases of
//! `flint edit`): the kernel's commands, in every build (#1557). The
//! editors are the procgen package's, opened through the
//! [`SpecEditors`] it registers; a flint without it names the package.

use anyhow::Result;
use clap::Args;
use flint_package::cli::Points;
use flint_viewer::host::{self, HostOptions, SpecEdit, SpecEditors, SpecKind};

#[derive(Args)]
pub struct GenPreviewArgs {
    /// Path to a .procgen.toml spec file
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

    /// Start with auto-orbit enabled
    #[arg(long)]
    pub auto_orbit: bool,
}

#[derive(Args)]
pub struct TexEditArgs {
    /// Path to a .procgen.toml spec file (must use pattern = "pipeline")
    pub spec: String,

    /// Override the spec's seed with a fixed value
    #[arg(long)]
    pub seed: Option<u64>,

    /// Window width
    #[arg(long, default_value = "1600")]
    pub width: u32,

    /// Window height
    #[arg(long, default_value = "1000")]
    pub height: u32,
}

/// The spec editors this run has, or the error naming the package that
/// provides `words` (`gen-preview`: procgen).
pub fn editors<'p>(points: &'p Points, what: &str, words: &[&str]) -> Result<&'p SpecEditors> {
    points
        .get::<SpecEditors>()
        .ok_or_else(|| super::absent::missing_owned(what.to_string(), words))
}

/// `flint gen-preview <spec>`: the previewer in a window.
pub fn gen_preview(args: GenPreviewArgs) -> Result<()> {
    gen_preview_in(args, crate::points())
}

pub(crate) fn gen_preview_in(args: GenPreviewArgs, points: &Points) -> Result<()> {
    let editors = editors(points, "flint gen-preview", &["gen-preview"])?;
    let options = HostOptions {
        width: args.width,
        height: args.height,
        camera: Default::default(),
        auto_orbit: args.auto_orbit,
    };
    let editor = (editors.open)(
        SpecKind::Mesh,
        SpecEdit {
            spec: args.spec,
            seed: args.seed,
            width: args.width,
            height: args.height,
            no_grid: args.no_grid,
            auto_orbit: args.auto_orbit,
        },
    )?;
    host::run(editor, options)
}

/// `flint tex-edit <spec>`: the node editor in a window.
pub fn tex_edit(args: TexEditArgs) -> Result<()> {
    tex_edit_in(args, crate::points())
}

pub(crate) fn tex_edit_in(args: TexEditArgs, points: &Points) -> Result<()> {
    let editors = editors(points, "flint tex-edit", &["tex-edit"])?;
    let options = HostOptions {
        width: args.width,
        height: args.height,
        camera: Default::default(),
        auto_orbit: false,
    };
    let editor = (editors.open)(
        SpecKind::Graph,
        SpecEdit {
            spec: args.spec,
            seed: args.seed,
            width: args.width,
            height: args.height,
            no_grid: false,
            auto_orbit: false,
        },
    )?;
    host::run(editor, options)
}
