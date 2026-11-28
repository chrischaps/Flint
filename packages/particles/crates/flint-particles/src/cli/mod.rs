//! The particles package's part of the `flint` command line (#1622): no
//! command of its own, the points of the kernel's commands it fills, which
//! flint-cli and flint-viewer did with the crate named until the package
//! had a folder.
//!
//! - `flint edit <x>.particles.toml`'s effect editor ([`editor`],
//!   `flint_viewer::host::EffectEditors`), its `--render` still included;
//! - the particles `flint render`'s captures and stills, `flint check`'s
//!   `bad-emitter` and the scene viewer step
//!   (`flint_package::particles::Particles`, [`crate::point`]).
//!
//! A launcher lists [`extensions`] from the manifest's `[rust] cli`.

pub mod editor;

use flint_package::cli::{CliExtension, Points};
use flint_viewer::host::{Editor, EffectEdit, EffectEditors};

/// What a launcher lists.
pub fn extensions() -> Vec<Box<dyn CliExtension>> {
    vec![Box::new(ParticlesCli)]
}

/// The package's extension: no command of its own, its points.
struct ParticlesCli;

fn open(edit: EffectEdit) -> anyhow::Result<Box<dyn Editor>> {
    Ok(Box::new(editor::editor(edit)?))
}

impl CliExtension for ParticlesCli {
    fn commands(&self) -> Vec<clap::Command> {
        Vec::new()
    }

    fn run(&mut self, _matches: &clap::ArgMatches) -> anyhow::Result<()> {
        unreachable!("the particles package adds no command of its own")
    }

    fn register(&self, points: &mut Points) {
        points.insert(crate::point()).insert(EffectEditors { open });
    }
}
