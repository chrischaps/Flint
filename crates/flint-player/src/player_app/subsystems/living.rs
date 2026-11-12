//! Living props and the wooze field, with shallow wooze, woozy grass and the
//! A/B comparison (#211-#252): the subsystem is
//! `living_subsystem::LivingSubsystem`.

use super::{SceneCtx, Subsystem};

/// The subsystem for the scene being entered, if it has living props or wooze.
pub(crate) fn for_scene(ctx: &mut SceneCtx) -> Option<Box<dyn Subsystem>> {
    crate::player_app::living_subsystem::LivingSubsystem::for_scene(ctx)
}
