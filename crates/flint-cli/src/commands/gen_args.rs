//! `flint gen`'s arguments are the Package API's (#1557,
//! `flint_package::cli::gen`), so the procgen package's run takes the
//! kernel's command as it parsed. Here: the refusal of a package's flags
//! on it that no package of the run serves.

use flint_package::cli::Points;

pub use flint_package::cli::gen::GenArgs;

/// The flags of `gen` a package serves through a point (the ai package's
/// steps, #1333, #1572), refused with exit 2 in a run whose packages don't
/// serve them, the package named from its manifest's `[provides] flags`.
/// Asked before procgen's point, so a run with neither names the flag's
/// package first.
pub(super) fn refuse_unserved(args: &GenArgs, points: &Points) -> anyhow::Result<()> {
    let line = [
        (args.cost_report, "gen --cost-report"),
        (args.ai_create.is_some(), "gen --ai-create"),
        (args.ai_assist, "gen --ai-assist"),
        (args.ai_refine, "gen --ai-refine"),
        (args.style_guide.is_some(), "gen --style-guide"),
    ]
    .into_iter()
    .find_map(|(given, line)| (given && !points.serves(line)).then_some(line));
    match line {
        Some(line) => Err(super::absent::missing_served(line)),
        None => Ok(()),
    }
}
