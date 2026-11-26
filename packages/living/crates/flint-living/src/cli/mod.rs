//! The living package's part of the `flint` command line (#1558): no
//! command of its own, the points of the kernel's and procgen's commands it
//! fills, which flint-cli did behind its `living` feature until the package
//! had a crate.
//!
//! - `flint render`'s [`stage::Stages`] (`flint_package::cli::Stages`): the
//!   props at `--time`, `--sweep wooze.*|living.*`, the shading boil read
//!   off the field, `--report json`'s props;
//! - `flint check`'s [`check::checks`] (`flint_package::cli::TableChecks`):
//!   `[wooze]` and `[living.species]` held to what reads them;
//! - procgen's `gen --strip` ([`strip`], `flint_procgen_cli::StripSheet`);
//! - the scene viewer's field and clocks ([`viewer::fields`],
//!   `flint_viewer::field_clocks::SceneFields`).
//!
//! A launcher lists [`extensions`] from the manifest's `[rust] cli`.

pub mod check;
pub mod stage;
pub mod strip;
pub mod viewer;

use flint_package::cli::{CliExtension, Points, Stages};

/// The render stage's part, as the render asks for it.
static STAGES: stage::Stages = stage::Stages;

/// What a launcher lists.
pub fn extensions() -> Vec<Box<dyn CliExtension>> {
    vec![Box::new(Living)]
}

/// The package's extension: no command of its own, its points.
struct Living;

impl CliExtension for Living {
    fn commands(&self) -> Vec<clap::Command> {
        Vec::new()
    }

    fn run(&mut self, _matches: &clap::ArgMatches) -> anyhow::Result<()> {
        unreachable!("the living package adds no command of its own")
    }

    fn register(&self, points: &mut Points) {
        points
            .insert(Stages(&STAGES))
            .insert(check::checks())
            .insert(strip::point())
            .insert(viewer::fields());
    }
}
