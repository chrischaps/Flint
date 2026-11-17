//! The scene's music, and with the `rhythm` feature its chart (#745): the
//! subsystem is `crate::music`'s, built for a scene with a `music_session`,
//! in a build with the `music` feature (#754). Without it, such a scene is
//! told so at its entry and plays silent.

use super::{SceneCtx, Subsystem};

/// The music plays on under a pause menu, as a loop does: no game state
/// pauses it.
pub(crate) const POLICY: Option<&str> = None;

/// Its script module (#698): a rhythm session's state, enabled in a scene
/// whose `music_session` has a chart. (`music_beat()` and the rest are
/// flint-script's core: any scene asks them.)
#[cfg(feature = "music")]
pub(crate) const SCRIPT_MODULES: &[&dyn flint_script::ScriptModule] =
    &[&crate::music::conducted::CONDUCTED];
#[cfg(not(feature = "music"))]
pub(crate) const SCRIPT_MODULES: &[&dyn flint_script::ScriptModule] = &[];

/// Its debug keys (#717): a charted session's guide, map and forced fail
/// (`music::rhythm::RhythmSubsystem`). A suite with no chart has none, and
/// with no session playing a chart the keys are the game's.
pub(crate) const DEBUG_KEYS: flint_runtime::DebugKeyGroup = flint_runtime::DebugKeyGroup {
    owner: "music",
    when: "a scene whose music_session has a chart",
    keys: KEYS,
};

#[cfg(all(feature = "rhythm", feature = "debug-hud"))]
const KEYS: &[flint_runtime::DebugKey] = crate::music::rhythm::DEBUG_KEYS;
#[cfg(not(all(feature = "rhythm", feature = "debug-hud")))]
const KEYS: &[flint_runtime::DebugKey] = &[];

/// The subsystem for the scene being entered, if it has a `music_session`.
#[cfg(feature = "music")]
pub(crate) fn for_scene(ctx: &mut SceneCtx) -> Option<Box<dyn Subsystem>> {
    crate::music::for_scene(ctx)
}

/// A build without the feature: a scene with a `music_session` hears so
/// once, here.
#[cfg(not(feature = "music"))]
pub(crate) fn for_scene(ctx: &mut SceneCtx) -> Option<Box<dyn Subsystem>> {
    if let Some(line) = crate::player_app::missing_features::music_line(ctx.world) {
        tracing::warn!(target: "load", "{line}");
    }
    None
}
