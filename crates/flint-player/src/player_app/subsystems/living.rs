//! Living props and the wooze field, with shallow wooze, woozy grass and the
//! A/B comparison (#211-#252): the subsystem is
//! `living_subsystem::LivingSubsystem`.

use super::{SceneCtx, Subsystem};
use crate::player_app::living_subsystem::LivingSubsystem;

/// A game state pauses the grove's clock by `living: "pause"` (#727).
pub(crate) const POLICY: Option<&str> = Some(LivingSubsystem::NAME);

/// The subsystem for the scene being entered, if it has living props or wooze.
pub(crate) fn for_scene(ctx: &mut SceneCtx) -> Option<Box<dyn Subsystem>> {
    LivingSubsystem::for_scene(ctx)
}
