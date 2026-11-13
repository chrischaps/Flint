//! Motion held on twos (#405): poses and drawn positions that change only on
//! a tick, `rate` times a second, the way a rotoscoped film holds each drawing
//! for two or three frames.
//!
//! The hold is the renderer's alone. The world goes on moving smoothly, so
//! physics (fixed at 1/60 s), IK and scripts read the same transforms they
//! always did; what is held is what the renderer draws of an entity: its
//! world matrix and its bone palette, kept from the frame a tick began until
//! the next tick. A `boil` on a style steps its seed with the same
//! [`tick`], so pencil lines at `boil = 12` redraw in exactly the frames held
//! poses move at `rate = 12`.
//!
//! Who is held: an entity with a `motion_hold` component and everything below
//! it (the nearest component up the hierarchy wins, and `rate = 0` there lets
//! a branch move smoothly), and, under `[post_process] motion_hold = <rate>`,
//! every entity that names no rate of its own. The whole-frame hold also
//! holds the post time styles, grain and the tear read, to the start of its
//! tick.
//!
//! Hosts stamp the clock each frame with
//! [`SceneRenderer::set_motion_time`](crate::SceneRenderer::set_motion_time)
//! before they upload bones; a renderer that is never stamped holds nothing.

use std::collections::HashMap;

use flint_core::EntityId;

// The clock and the hierarchy rule live beside the world (#873), so the
// particle sync, which cannot see the renderer, holds on the same ticks.
pub use flint_ecs::motion_hold::{
    held_rate, held_rates, held_time, post_time, tick, DEFAULT_RATE, TICK_EARLY,
};

/// What a held entity last showed, and on which tick.
#[derive(Debug, Clone)]
struct Held<T> {
    tick: i64,
    rate: f32,
    value: T,
}

/// The held matrices and bone palettes, and the clock they hold against.
#[derive(Debug, Default)]
pub struct MotionHold {
    /// This frame's post time (see [`post_time`]), `None` until a host
    /// stamps one: then nothing is held.
    time: Option<f32>,
    /// Who is held and at what rate, from the last `update_from_world`.
    rates: HashMap<EntityId, f32>,
    matrices: HashMap<EntityId, Held<[[f32; 4]; 4]>>,
    /// The tick each held entity's bones were last written on.
    bones: HashMap<EntityId, Held<()>>,
}

impl MotionHold {
    /// Stamp this frame's game time.
    pub fn set_time(&mut self, seconds: f64) {
        self.time = Some(post_time(seconds));
    }

    /// The stamped time, if any host stamped one.
    pub fn time(&self) -> Option<f32> {
        self.time
    }

    /// Who is held this frame (from `held_rates`); drops what an entity no
    /// longer held kept, so taking the hold off shows the live pose at once.
    pub fn set_rates(&mut self, rates: HashMap<EntityId, f32>) {
        self.matrices.retain(|id, _| rates.contains_key(id));
        self.bones.retain(|id, _| rates.contains_key(id));
        self.rates = rates;
    }

    /// The rate `entity` is held at, if it is held and the clock is stamped.
    pub fn rate_of(&self, entity: EntityId) -> Option<f32> {
        self.time?;
        self.rates.get(&entity).copied()
    }

    /// Whether anything is held this frame.
    pub fn any(&self) -> bool {
        self.time.is_some() && !self.rates.is_empty()
    }

    fn now(&self, entity: EntityId) -> Option<(i64, f32)> {
        let rate = self.rate_of(entity)?;
        Some((tick(self.time?, rate)?, rate))
    }

    /// The world matrix to draw `entity` with: `live` on the first frame of
    /// a tick (and kept), the kept one for the rest of it. Not held: `live`.
    pub fn matrix(&mut self, entity: EntityId, live: [[f32; 4]; 4]) -> [[f32; 4]; 4] {
        let Some((tick, rate)) = self.now(entity) else {
            return live;
        };
        match self.matrices.get(&entity) {
            Some(h) if h.tick == tick && h.rate == rate => h.value,
            _ => {
                self.matrices.insert(
                    entity,
                    Held {
                        tick,
                        rate,
                        value: live,
                    },
                );
                live
            }
        }
    }

    /// Whether `entity`'s bones should be written this frame: always when it
    /// isn't held, and once a tick when it is (the first frame of each).
    pub fn bones_due(&mut self, entity: EntityId) -> bool {
        let Some((tick, rate)) = self.now(entity) else {
            return true;
        };
        match self.bones.get(&entity) {
            Some(h) if h.tick == tick && h.rate == rate => false,
            _ => {
                self.bones.insert(
                    entity,
                    Held {
                        tick,
                        rate,
                        value: (),
                    },
                );
                true
            }
        }
    }

    /// `entity`'s bones on the GPU are this frame's: a hold that starts now
    /// keeps them for the rest of the tick. `update_from_world` calls it for
    /// every held entity with bones, since a host uploads before it reads
    /// the world, while the rates were still last frame's.
    pub fn bones_shown(&mut self, entity: EntityId) {
        if let Some((tick, rate)) = self.now(entity) {
            self.bones.entry(entity).or_insert(Held {
                tick,
                rate,
                value: (),
            });
        }
    }

    /// Forget what `entity` kept (#871): its next matrix and bones are drawn
    /// whatever the tick, and held from then on. For an editor whose pose
    /// changed for a reason other than time (a layer dial, a posed bone, a
    /// clip switch), which a hold would otherwise keep off screen until the
    /// next tick, and never while paused.
    pub fn release(&mut self, entity: EntityId) {
        self.matrices.remove(&entity);
        self.bones.remove(&entity);
    }

    /// Forget every kept pose (a scene load): the next frame shows live ones.
    pub fn clear(&mut self) {
        self.rates.clear();
        self.matrices.clear();
        self.bones.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ticks_count_from_zero_at_the_rate() {
        assert_eq!(tick(0.0, 12.0), Some(0));
        assert_eq!(tick(0.08, 12.0), Some(0));
        assert_eq!(tick(1.0 / 12.0, 12.0), Some(1));
        assert_eq!(tick(1.0, 12.0), Some(12));
        assert_eq!(tick(-3.0, 12.0), Some(0), "before 0 is tick 0");
    }

    #[test]
    fn no_rate_holds_nothing() {
        assert_eq!(tick(1.0, 0.0), None);
        assert_eq!(tick(1.0, -2.0), None);
        assert_eq!(tick(1.0, f32::NAN), None);
        assert_eq!(tick(f32::INFINITY, 12.0), None);
        assert_eq!(held_time(1.234, 0.0), 1.234);
    }

    /// The case the early margin is for: frames stepped by 1/60 sum to a
    /// tick of 1/12 a hair short, and the tick must still turn on frame 5,
    /// not 6, on every multiple, as a --frame-step capture steps them.
    #[test]
    fn stepped_frames_turn_the_tick_on_the_frame_it_lands() {
        let mut t = 0.0f64;
        let mut turned = Vec::new();
        let mut last = tick(post_time(t), 12.0);
        for frame in 1..=600 {
            t += 1.0 / 60.0;
            let now = tick(post_time(t), 12.0);
            if now != last {
                turned.push(frame);
            }
            last = now;
        }
        let expected: Vec<i32> = (1..=120).map(|k| k * 5).collect();
        assert_eq!(turned, expected);
        // and at 24 a second on 1/48 steps, on twos
        let mut t = 0.0f64;
        let mut turned = 0;
        let mut last = tick(post_time(t), 24.0);
        for frame in 1..=480 {
            t += 1.0 / 48.0;
            let now = tick(post_time(t), 24.0);
            if now != last {
                assert_eq!(frame % 2, 0, "frame {frame}");
                turned += 1;
            }
            last = now;
        }
        assert_eq!(turned, 240);
    }

    #[test]
    fn held_time_is_the_start_of_its_tick() {
        assert_eq!(held_time(0.0, 12.0), 0.0);
        assert_eq!(held_time(0.05, 12.0), 0.0);
        assert!((held_time(0.1, 12.0) - 1.0 / 12.0).abs() < 1e-7);
        assert!((held_time(1.04, 12.0) - 1.0).abs() < 1e-6);
        // a held time ticks to the tick it was held to, at its rate and at
        // any whole multiple of it, so a boil of 24 under a hold of 12 is
        // still one drawing a tick
        for k in 0..2000 {
            let at = held_time(k as f32 / 60.0, 12.0);
            let t12 = tick(at, 12.0).unwrap();
            assert_eq!(t12, tick(k as f32 / 60.0, 12.0).unwrap());
            assert_eq!(tick(at, 24.0).unwrap(), 2 * t12, "k {k}");
        }
    }

    fn id(n: u64) -> EntityId {
        EntityId::from_raw(n)
    }

    fn m(x: f32) -> [[f32; 4]; 4] {
        let mut m = [[0.0; 4]; 4];
        m[3][0] = x;
        m
    }

    #[test]
    fn a_held_matrix_changes_only_on_a_tick() {
        let mut hold = MotionHold::default();
        hold.set_rates(HashMap::from([(id(1), 12.0)]));
        let mut shown = Vec::new();
        for frame in 0..12 {
            hold.set_time(frame as f64 / 60.0);
            // the live position moves every frame
            shown.push(hold.matrix(id(1), m(frame as f32))[3][0]);
            // and an entity that isn't held shows it
            assert_eq!(hold.matrix(id(2), m(frame as f32))[3][0], frame as f32);
        }
        assert_eq!(
            shown,
            [0.0, 0.0, 0.0, 0.0, 0.0, 5.0, 5.0, 5.0, 5.0, 5.0, 10.0, 10.0]
        );
    }

    #[test]
    fn bones_are_due_once_a_tick() {
        let mut hold = MotionHold::default();
        hold.set_rates(HashMap::from([(id(1), 12.0)]));
        let due: Vec<bool> = (0..11)
            .map(|frame| {
                hold.set_time(frame as f64 / 60.0);
                hold.bones_due(id(1))
            })
            .collect();
        assert_eq!(
            due,
            [true, false, false, false, false, true, false, false, false, false, true]
        );
        assert!(hold.bones_due(id(2)) && hold.bones_due(id(2)), "not held");
    }

    #[test]
    fn an_unstamped_renderer_holds_nothing() {
        let mut hold = MotionHold::default();
        hold.set_rates(HashMap::from([(id(1), 12.0)]));
        assert!(!hold.any());
        for x in 0..4 {
            assert_eq!(hold.matrix(id(1), m(x as f32))[3][0], x as f32);
            assert!(hold.bones_due(id(1)));
        }
    }

    #[test]
    fn letting_go_shows_the_live_pose_at_once() {
        let mut hold = MotionHold::default();
        hold.set_rates(HashMap::from([(id(1), 12.0)]));
        hold.set_time(0.0);
        hold.matrix(id(1), m(0.0));
        hold.set_time(1.0 / 60.0);
        assert_eq!(hold.matrix(id(1), m(1.0))[3][0], 0.0);
        hold.set_rates(HashMap::new());
        assert_eq!(hold.matrix(id(1), m(1.0))[3][0], 1.0);
        // and taking it again starts a fresh hold from the live pose
        hold.set_rates(HashMap::from([(id(1), 12.0)]));
        assert_eq!(hold.matrix(id(1), m(1.0))[3][0], 1.0);
    }

    #[test]
    fn a_new_rate_starts_a_new_hold() {
        let mut hold = MotionHold::default();
        hold.set_rates(HashMap::from([(id(1), 12.0)]));
        hold.set_time(0.0);
        hold.matrix(id(1), m(0.0));
        hold.set_rates(HashMap::from([(id(1), 8.0)]));
        hold.set_time(1.0 / 60.0);
        assert_eq!(hold.matrix(id(1), m(1.0))[3][0], 1.0);
    }

    /// #871: an editor that changes a pose by hand within a tick (a layer
    /// dial while paused) lets that one through, then holds it.
    #[test]
    fn a_release_lets_one_pose_through_and_holds_it() {
        let mut hold = MotionHold::default();
        hold.set_rates(HashMap::from([(id(1), 12.0), (id(2), 12.0)]));
        hold.set_time(0.0);
        assert!(hold.bones_due(id(1)) && hold.bones_due(id(2)));
        hold.matrix(id(1), m(0.0));
        hold.matrix(id(2), m(0.0));
        // Paused: the clock stands still, so nothing is due
        assert!(!hold.bones_due(id(1)));
        hold.release(id(1));
        assert!(hold.bones_due(id(1)), "released: the new pose goes up");
        assert_eq!(hold.matrix(id(1), m(3.0))[3][0], 3.0);
        // ...and is held for the rest of the tick
        assert!(!hold.bones_due(id(1)));
        assert_eq!(hold.matrix(id(1), m(4.0))[3][0], 3.0);
        // The other entity was never let go
        assert!(!hold.bones_due(id(2)));
        assert_eq!(hold.matrix(id(2), m(3.0))[3][0], 0.0);
    }
}
