//! The rig package's part of the `flint` command line (#1621, ADR 0137
//! decision 4): the model previewer. It adds no command of its own: the
//! kernel's `flint edit <model>.glb` and hidden `flint preview` open the
//! previewer through the viewer's [`ModelEditors`] point, and the ai
//! package's refine loop draws a candidate mesh through the
//! [`ModelStill`] point, both of which this fills. flint-cli's `build.rs`
//! lists [`extensions`] from `packages/rig/package.toml`'s `[rust] cli`.

mod preview;

use flint_package::cli::gen::ModelStill;
use flint_package::cli::{CliExtension, Points};
use flint_viewer::host::{Editor, ModelEdit, ModelEditors};

/// What the CLI launcher lists for the rig package.
pub fn extensions() -> Vec<Box<dyn CliExtension>> {
    vec![Box::new(RigCli)]
}

/// The package's extension: no command of its own, its points.
struct RigCli;

impl CliExtension for RigCli {
    fn commands(&self) -> Vec<clap::Command> {
        Vec::new()
    }

    fn run(&mut self, _matches: &clap::ArgMatches) -> anyhow::Result<()> {
        unreachable!("the rig package adds no command of its own")
    }

    fn register(&self, points: &mut Points) {
        points
            .insert(ModelEditors { open: open_editor })
            .insert(ModelStill(preview::render_model));
    }
}

/// The model previewer, for the host to open or draw.
fn open_editor(edit: ModelEdit) -> anyhow::Result<Box<dyn Editor>> {
    Ok(Box::new(preview::editor(edit)?))
}
