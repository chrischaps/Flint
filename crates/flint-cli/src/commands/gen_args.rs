//! `flint gen`'s arguments are the Package API's (#1557,
//! `flint_package::cli::gen`), so the procgen package's run takes the
//! kernel's command as it parsed. Here: the refusal of the ai package's
//! flags in a build without it.

pub use flint_package::cli::gen::GenArgs;

/// The flags that need the ai package (#1333), refused with exit 2 in a
/// build without it. Nothing to refuse in a build with it.
pub(super) fn refuse_ai(args: &GenArgs) -> anyhow::Result<()> {
    if super::absent::built_with("ai") {
        return Ok(());
    }
    let flag = [
        (args.cost_report, "--cost-report"),
        (args.ai_create.is_some(), "--ai-create"),
        (args.ai_assist, "--ai-assist"),
        (args.ai_refine, "--ai-refine"),
        (args.style_guide.is_some(), "--style-guide"),
    ]
    .into_iter()
    .find_map(|(given, flag)| given.then_some(flag));
    match flag {
        Some(flag) => Err(super::absent::missing(format!("flint gen {flag}"), "ai")),
        None => Ok(()),
    }
}
