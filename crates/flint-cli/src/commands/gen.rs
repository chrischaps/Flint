//! `flint gen` (#1543, #1557): the kernel's command, in every build. A
//! glyph set (`<set>.glyphs.toml`, `glyph_preview.rs`) is flint-render's
//! and runs here; anything else (a spec, the AI's spec,
//! `--list-generators`, `--describe`, `--cost-report`) is the procgen
//! package's, run by the [`SpecGen`] it registered. A flint without the
//! package has none, and names it (exit 2). The AI flags name the ai
//! package first in a build without it, since ai needs procgen anyway.

use anyhow::Result;
use flint_package::cli::{Points, SpecGen};

pub use super::gen_args::GenArgs;

pub fn run(args: GenArgs) -> Result<()> {
    run_in(args, crate::points())
}

/// [`run`] against `points`, for a test's own.
pub(crate) fn run_in(args: GenArgs, points: &Points) -> Result<()> {
    super::gen_args::refuse_unserved(&args, points)?;
    if args
        .spec
        .as_deref()
        .is_some_and(super::glyph_preview::is_glyph_set)
    {
        return super::glyph_preview::run(&args);
    }
    if let Some(SpecGen(run)) = points.get::<SpecGen>() {
        return run(args);
    }
    let what = if args.list_generators {
        "flint gen --list-generators"
    } else if args.describe.is_some() {
        "flint gen --describe"
    } else {
        "flint gen"
    };
    Err(super::absent::missing_owned(what, &["gen"]))
}
