//! The terrain package's part of the `flint` command line (#1623): the
//! terrain editor, which was flint-cli's `commands/terrain_edit.rs`.
//!
//! The commands stay the kernel's (`flint edit x.terrain.toml`, the hidden
//! `flint terrain-edit`), so `flint --help` is the same in every build; a
//! flint without this package names it. What this crate brings it registers
//! as a point ([`extensions`], in [`CliExtension::register`]):
//! [`TerrainEditors`], the editor a `.terrain.toml` opens in, and its entry
//! on `SceneChecks` ([`check`], #1697): a terrain held to its recipe.

pub mod check;
mod editor;

pub use editor::TerrainEditApp;

use flint_package::cli::{CliExtension, Points, SceneChecks};
use flint_viewer::host::{Editor, SpecEdit, TerrainEditors};

/// What the CLI launcher lists for the terrain package
/// (`packages/terrain/package.toml`'s `[rust] cli`, which flint-cli's
/// `build.rs` turns into `flint_terrain::cli::extensions()`, #1554).
pub fn extensions() -> Vec<Box<dyn CliExtension>> {
    vec![Box::new(Terrain)]
}

/// The package's extension: no command of its own, its point.
struct Terrain;

impl CliExtension for Terrain {
    fn commands(&self) -> Vec<clap::Command> {
        Vec::new()
    }

    fn run(&mut self, _matches: &clap::ArgMatches) -> anyhow::Result<()> {
        unreachable!("the terrain package adds no command of its own")
    }

    fn register(&self, points: &mut Points) {
        points.insert(TerrainEditors { open });
        // A terrain held to the recipe it names (#1697)
        points
            .get_or_insert_default::<SceneChecks>()
            .0
            .push((crate::PACKAGE_NAME, check::check));
    }
}

/// The terrain editor for a `.terrain.toml` (written when it isn't there).
fn open(edit: SpecEdit) -> anyhow::Result<Box<dyn Editor>> {
    Ok(Box::new(editor::editor(edit)?))
}
