//! Shallow wooze and woozy grass (#215, #252): the subsystem is
//! `shallow_wooze::ShallowWoozeSubsystem`.

use super::{SceneCtx, Subsystem};

/// The subsystem for the scene being entered, if it uses shallow wooze.
pub(crate) fn for_scene(ctx: &SceneCtx) -> Option<Box<dyn Subsystem>> {
    crate::player_app::shallow_wooze::ShallowWoozeSubsystem::for_scene(ctx)
}
