//! Who is drawn on ticks, and when a tick turns (#405, #873).
//!
//! The clock and the hierarchy rule of `motion_hold`, shared by everything
//! that draws a held entity: the renderer (its matrix, bones and lights,
//! `flint_render::motion_hold`) and the particle sync (where a held emitter
//! emits from). They live here, beside the world, so that crates which
//! cannot see the renderer hold on the same frames it does.

use std::collections::HashMap;

use flint_core::components as comp;
use flint_core::EntityId;

use crate::FlintWorld;

/// A tick lands this far (in ticks) early, so a time that sums to a whole
/// tick a hair short in floating point (5 steps of 1/60 against 1/12) still
/// counts as on it. A thousandth of a tick is 83 µs at 12 a second: far below
/// a frame at any rate a film uses.
pub const TICK_EARLY: f32 = 1e-3;

/// The default rate of a `motion_hold` component: twelve drawings a second,
/// on twos at 24 frames a second.
pub const DEFAULT_RATE: f32 = 12.0;

/// The post time as styles see it: game seconds wrapped at 100,000 s, in f32.
/// Everything that ticks against the post time goes through this, so a hold
/// and a boil fed the same seconds agree to the bit.
pub fn post_time(seconds: f64) -> f32 {
    (seconds % 100_000.0) as f32
}

/// Which tick `time` falls in at `rate` ticks a second, or `None` for a rate
/// of 0 (or below, or not finite): nothing held. Time before 0 is tick 0.
pub fn tick(time: f32, rate: f32) -> Option<i64> {
    if !(rate > 0.0 && rate.is_finite() && time.is_finite()) {
        return None;
    }
    Some((time.max(0.0) * rate + TICK_EARLY).floor() as i64)
}

/// `time` held to the start of its tick: what time-driven styles see under a
/// whole-frame hold. A rate of 0 gives `time` back.
pub fn held_time(time: f32, rate: f32) -> f32 {
    match tick(time, rate) {
        Some(t) => t as f32 / rate,
        None => time,
    }
}

/// The rate `entity` is drawn at: its own `motion_hold`, or its nearest
/// ancestor's, else `whole` (the `[post_process]` rate, 0 for none). `None`
/// when that comes to 0: it moves smoothly. Walks the parent chain only in a
/// scene with a `motion_hold` somewhere.
pub fn held_rate(world: &FlintWorld, entity: EntityId, whole: f32) -> Option<f32> {
    let tagged = world.entities_with_component(comp::MOTION_HOLD);
    let rate = if tagged.is_empty() {
        whole
    } else {
        rate_in(world, tagged, entity).unwrap_or(whole)
    };
    (rate > 0.0 && rate.is_finite()).then_some(rate)
}

fn rate_in(
    world: &FlintWorld,
    tagged: &std::collections::HashSet<EntityId>,
    entity: EntityId,
) -> Option<f32> {
    let mut at = entity;
    // Depth-capped as `hidden_entities` is: a parent chain never loops
    for _ in 0..256 {
        if tagged.contains(&at) {
            return Some(world.view(at, comp::MOTION_HOLD).f32("rate", DEFAULT_RATE));
        }
        at = world.get_parent(at)?;
    }
    None
}

/// The rate each held entity draws at this frame (see [`held_rate`]).
/// Entities that end up at 0 are left out, so an empty map means nothing is
/// held, and a scene with no component and no whole-frame rate walks nothing.
pub fn held_rates(world: &FlintWorld, whole: f32) -> HashMap<EntityId, f32> {
    let tagged = world.entities_with_component(comp::MOTION_HOLD);
    let mut rates = HashMap::new();
    let whole_holds = whole > 0.0 && whole.is_finite();
    if tagged.is_empty() && !whole_holds {
        return rates;
    }
    for entity in world.all_entities() {
        if let Some(rate) = held_rate(world, entity.id, whole) {
            rates.insert(entity.id, rate);
        }
    }
    rates
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hold(world: &mut FlintWorld, id: EntityId, rate: f64) {
        world
            .set_field(id, comp::MOTION_HOLD, "rate", toml::Value::Float(rate))
            .unwrap();
    }

    #[test]
    fn the_nearest_hold_up_the_hierarchy_wins() {
        let mut world = FlintWorld::new();
        let walker = world.spawn("walker").unwrap();
        let hand = world.spawn("hand").unwrap();
        let torch = world.spawn("torch").unwrap();
        let lamp = world.spawn("lamp").unwrap();
        world.set_parent(hand, walker).unwrap();
        world.set_parent(torch, hand).unwrap();
        assert_eq!(held_rate(&world, torch, 0.0), None, "nothing held");
        assert_eq!(held_rate(&world, torch, 8.0), Some(8.0), "the whole frame");

        hold(&mut world, walker, 12.0);
        assert_eq!(held_rate(&world, torch, 0.0), Some(12.0), "two levels up");
        assert_eq!(held_rate(&world, lamp, 0.0), None, "not under it");
        assert_eq!(held_rate(&world, lamp, 6.0), Some(6.0));

        hold(&mut world, hand, 0.0);
        assert_eq!(held_rate(&world, torch, 6.0), None, "0 lets a branch move");
        assert_eq!(held_rate(&world, walker, 0.0), Some(12.0));

        let rates = held_rates(&world, 0.0);
        assert_eq!(rates.len(), 1);
        assert_eq!(rates.get(&walker), Some(&12.0));
    }
}
