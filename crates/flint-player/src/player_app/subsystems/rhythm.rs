//! The `rhythm` package's part in this crate (#745; #1556 moves it to its
//! own): a `music_session` with a `chart`, judged. The music is its own
//! package's (`flint_music::player`, #1555), whose builder hands a charted
//! session to the chart judge it inserted, which rhythm fills here.

use flint_package::{Registry, SceneCtx, Subsystem};
use std::path::PathBuf;

/// The rhythm package's registration: music's chart judge (`crate::rhythm`'s
/// subsystem), and its debug keys (#717), a charted session's guide, map
/// and forced fail, with the debug HUD. A suite with no chart has none, and
/// with no session playing a chart the keys are the game's.
pub(crate) fn register(registry: &mut Registry) {
    if let Some(point) = registry.point_mut::<flint_music::player::ChartJudge>() {
        point.judge = Some(judge);
    }
    #[cfg(feature = "debug-hud")]
    registry.debug_keys(flint_runtime::DebugKeyGroup {
        owner: "music",
        when: "a scene whose music_session has a chart",
        keys: crate::rhythm::DEBUG_KEYS,
    });
}

/// A charted session's subsystem: the rhythm game's.
fn judge(base: PathBuf, comp: toml::Value, ctx: &mut SceneCtx) -> Box<dyn Subsystem> {
    Box::new(crate::rhythm::RhythmSubsystem::new(base, comp, ctx))
}
