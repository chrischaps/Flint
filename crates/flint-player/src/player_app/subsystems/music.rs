//! The scene's music, and with the `rhythm` feature its chart (#745): the
//! subsystem is `crate::music`'s, built for a scene with a `music_session`,
//! in a build with the `music` feature (#754). Without it, such a scene is
//! told so at its entry and plays silent.

use super::SceneCtx;
#[cfg(not(feature = "music"))]
use super::Subsystem;
use flint_package::Registry;

/// The `music` package's part in this crate (#1555 moves it to its own):
/// its builder, the line a scene with a session hears when the project
/// leaves it out (#1528), and its module (#698): a rhythm session's state,
/// enabled in a scene whose `music_session` has a chart (`music_beat()`
/// and the rest are flint-script's core: any scene asks them). The music
/// plays on under a pause menu, as a loop does: no game state pauses it.
#[cfg(feature = "music")]
pub(crate) fn register(registry: &mut Registry) {
    registry
        .scene_subsystem(crate::music::for_scene)
        .when_left_out(left_out)
        .script_modules(&[&crate::music::conducted::CONDUCTED]);
}

/// The `rhythm` package's part in this crate (#1556 moves it): its debug
/// keys (#717), a charted session's guide, map and forced fail
/// (`music::rhythm::RhythmSubsystem`), with the debug HUD. Music's builder
/// makes the rhythm subsystem itself. A suite with no chart has none, and
/// with no session playing a chart the keys are the game's.
#[cfg(feature = "rhythm")]
pub(crate) fn register_rhythm(registry: &mut Registry) {
    #[cfg(feature = "debug-hud")]
    registry.debug_keys(flint_runtime::DebugKeyGroup {
        owner: "music",
        when: "a scene whose music_session has a chart",
        keys: crate::music::rhythm::DEBUG_KEYS,
    });
    #[cfg(not(feature = "debug-hud"))]
    let _ = registry;
}

/// What a scene with a `music_session` hears in a project that leaves the
/// package out: the line to add to `[packages]`.
#[cfg(feature = "music")]
fn left_out(ctx: &SceneCtx) -> Option<String> {
    crate::player_app::missing_features::music_left_out_line(ctx.packages?, ctx.world)
}

/// A build without the package: the kernel says to a scene with a
/// `music_session`, once, that it plays silent.
#[cfg(not(feature = "music"))]
pub(crate) fn register_absent(registry: &mut Registry) {
    registry.scene_subsystem(unbuilt);
}

#[cfg(not(feature = "music"))]
fn unbuilt(ctx: &mut SceneCtx) -> Option<Box<dyn Subsystem>> {
    if let Some(line) = crate::player_app::missing_features::music_line(ctx.world) {
        tracing::warn!(target: "load", "{line}");
    }
    None
}
