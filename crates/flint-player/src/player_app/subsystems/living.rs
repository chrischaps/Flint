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

/// Its debug keys (#717): the A/B of #238 and the pick of #248. Live only
/// in a scene the subsystem runs in; elsewhere F5-F8 are the game's.
pub(crate) const DEBUG_KEYS: flint_runtime::DebugKeyGroup = flint_runtime::DebugKeyGroup {
    owner: "living",
    when: "a scene with living props or wooze",
    keys: KEYS,
};

#[cfg(feature = "living")]
const A_B: [flint_runtime::DebugKey; 3] = [
    flint_runtime::DebugKey {
        action: "living_freeze",
        key: "F5",
        does: "Freeze or thaw the living clock",
    },
    flint_runtime::DebugKey {
        action: "living_flip",
        key: "F6",
        does: "A/B: flip between the two presets",
    },
    flint_runtime::DebugKey {
        action: "living_split",
        key: "F7",
        does: "A/B: split the view between the two presets",
    },
];
#[cfg(all(feature = "living", feature = "debug-hud"))]
const KEYS: &[flint_runtime::DebugKey] = &[
    A_B[0],
    A_B[1],
    A_B[2],
    flint_runtime::DebugKey {
        action: "living_pick",
        key: "F8",
        does: "Pick the living prop at the view centre",
    },
    flint_runtime::DebugKey {
        action: "living_pick_next",
        key: "Shift+F8",
        does: "Pick the next living prop by name",
    },
];
#[cfg(all(feature = "living", not(feature = "debug-hud")))]
const KEYS: &[flint_runtime::DebugKey] = &A_B;
#[cfg(not(feature = "living"))]
const KEYS: &[flint_runtime::DebugKey] = &[];

/// The subsystem for the scene being entered, if it has living props or
/// wooze; in a capture, the grove posed at once (`ctx.capture`, #1252).
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
        tracing::warn!(target: "load", "{line}");
    }
    None
}
