//! Living props and the wooze field, with shallow wooze, woozy grass and the
//! A/B comparison (#211-#252): the subsystem is
//! `living_subsystem::LivingSubsystem`, in a build with the `living` feature
//! (#754). Without it, a scene that asks for any of it is told so at its
//! entry and stands still.

use super::{SceneCtx, Subsystem};
use flint_package::Registry;

/// A game state pauses the grove's clock by `living: "pause"` (#727). The
/// name stays in a build without the feature, so a state that names it
/// still registers.
pub(crate) const POLICY: &str = "living";

/// Its script modules (#698): the wooze field, the knobs and the A/B,
/// enabled in a scene the subsystem runs in.
#[cfg(feature = "living")]
pub(crate) const SCRIPT_MODULES: &[&dyn flint_script::ScriptModule] = &[
    &crate::player_app::living_api::WOOZE,
    &crate::player_app::living_api::LIVING,
    &crate::player_app::living_api::DEBUG,
];

/// Its debug keys (#717): the A/B of #238 and the pick of #248. Live only
/// in a scene the subsystem runs in; elsewhere F5-F8 are the game's.
#[cfg(feature = "living")]
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

/// The `living` package's part in this crate (#1558 moves it to its own):
/// its policy name, its builder, the line a scene that uses it hears when
/// the project leaves it out (#1528), its modules and its keys.
#[cfg(feature = "living")]
pub(crate) fn register(registry: &mut Registry) {
    registry
        .policy(POLICY)
        .scene_subsystem(for_scene)
        .when_left_out(left_out)
        .script_modules(SCRIPT_MODULES)
        .debug_keys(DEBUG_KEYS);
}

/// A build without the package: the kernel keeps its policy name and says
/// to a scene that uses it, once, that it stands still.
#[cfg(not(feature = "living"))]
pub(crate) fn register_absent(registry: &mut Registry) {
    registry.policy(POLICY).scene_subsystem(unbuilt);
}

/// The subsystem for the scene being entered, if it has living props or
/// wooze; in a capture, the grove posed at once (`ctx.capture`, #1252).
#[cfg(feature = "living")]
fn for_scene(ctx: &mut SceneCtx) -> Option<Box<dyn Subsystem>> {
    debug_assert_eq!(
        POLICY,
        crate::player_app::living_subsystem::LivingSubsystem::NAME
    );
    crate::player_app::living_subsystem::LivingSubsystem::for_scene(ctx)
}

/// What a scene that uses living props hears in a project that leaves the
/// package out: the line to add to `[packages]`.
#[cfg(feature = "living")]
fn left_out(ctx: &SceneCtx) -> Option<String> {
    let grass = ctx.renderer.as_deref().and_then(|r| r.grass_config());
    crate::player_app::missing_features::living_left_out_line(
        ctx.packages?,
        ctx.world,
        ctx.tables,
        grass,
    )
}

/// A build without the feature: a scene that uses it hears so once, here.
#[cfg(not(feature = "living"))]
fn unbuilt(ctx: &mut SceneCtx) -> Option<Box<dyn Subsystem>> {
    let grass = ctx.renderer.as_deref().and_then(|r| r.grass_config());
    if let Some(line) =
        crate::player_app::missing_features::living_line(ctx.world, ctx.tables, grass)
    {
        tracing::warn!(target: "load", "{line}");
    }
    None
}
