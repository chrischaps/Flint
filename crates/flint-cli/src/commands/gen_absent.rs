//! `flint gen` in a flint built without the procgen package (#1543).
//!
//! The arguments are `gen_args.rs`'s, as in every build, so a command line
//! that parses with the package parses without it. A glyph set
//! (`<set>.glyphs.toml`, `glyph_preview.rs`) is flint-render's and runs
//! here as it does there; anything else (a spec, `--list-generators`,
//! `--describe`) is [`absent::missing`](super::absent::missing): exit 2,
//! naming the package. The AI flags name the ai package first, as they do
//! in a build with procgen and without ai, since ai needs procgen anyway.

use anyhow::Result;

pub use super::gen_args::GenArgs;

pub fn run(args: GenArgs) -> Result<()> {
    super::gen_args::refuse_ai(&args)?;
    if args
        .spec
        .as_deref()
        .is_some_and(super::glyph_preview::is_glyph_set)
    {
        return super::glyph_preview::run(&args);
    }
    let what = if args.list_generators {
        "flint gen --list-generators"
    } else if args.describe.is_some() {
        "flint gen --describe"
    } else {
        "flint gen"
    };
    Err(super::absent::missing(what, "procgen"))
}
