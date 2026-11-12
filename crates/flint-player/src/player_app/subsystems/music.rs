//! The scene's music, and with the `rhythm` feature its chart (#745): the
//! subsystem is `crate::music`'s, built for a scene with a `music_session`.

use super::{SceneCtx, Subsystem};

/// The music plays on under a pause menu, as a loop does: no game state
/// pauses it.
pub(crate) const POLICY: Option<&str> = None;

/// The subsystem for the scene being entered, if it has a `music_session`.
pub(crate) fn for_scene(ctx: &mut SceneCtx) -> Option<Box<dyn Subsystem>> {
    crate::music::for_scene(ctx)
}
