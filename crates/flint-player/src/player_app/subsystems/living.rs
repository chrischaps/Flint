//! Living props and the wooze field, with shallow wooze, woozy grass and the
//! A/B comparison (#211-#252): the subsystem is
//! `living_subsystem::LivingSubsystem`, in a build with the `living` feature
//! (#754). Without it, a scene that asks for any of it is told so at its
//! entry and stands still.

use super::{SceneCtx, Subsystem};

/// A game state pauses the grove's clock by `living: "pause"` (#727). The
/// name stays in a build without the feature, so a state that names it
/// still registers.
pub(crate) const POLICY: Option<&str> = Some("living");

/// Its script modules (#698): the wooze field, the knobs and the A/B,
/// enabled in a scene the subsystem runs in.
#[cfg(feature = "living")]
pub(crate) const SCRIPT_MODULES: &[&dyn flint_script::ScriptModule] = &[
    &crate::player_app::living_api::WOOZE,
    &crate::player_app::living_api::LIVING,
    &crate::player_app::living_api::DEBUG,
];
#[cfg(not(feature = "living"))]
pub(crate) const SCRIPT_MODULES: &[&dyn flint_script::ScriptModule] = &[];

/// The subsystem for the scene being entered, if it has living props or wooze.
#[cfg(feature = "living")]
pub(crate) fn for_scene(ctx: &mut SceneCtx) -> Option<Box<dyn Subsystem>> {
    debug_assert_eq!(
        POLICY,
        Some(crate::player_app::living_subsystem::LivingSubsystem::NAME)
    );
    crate::player_app::living_subsystem::LivingSubsystem::for_scene(ctx)
}

/// A build without the feature: a scene that uses it hears so once, here.
#[cfg(not(feature = "living"))]
pub(crate) fn for_scene(ctx: &mut SceneCtx) -> Option<Box<dyn Subsystem>> {
    let grass = ctx.renderer.as_deref().and_then(|r| r.grass_config());
    if let Some(line) =
        crate::player_app::missing_features::living_line(ctx.world, ctx.tables, grass)
    {
        println!("{line}");
    }
    None
}
