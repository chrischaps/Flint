//! The flipbook clip kind (ADR 0139 section 6, #1620): sprite-sheet clips
//! played on animation's clock, writing `sprite.source_rect`.
//!
//! - **The cursor owns the rules.** Each sprite's playback is a
//!   [`ClipPlaybackState`] that follows its `animator` through the one rule
//!   set every kind follows ([`Animator::start`], [`Animator::apply_to`],
//!   [`animator::publish`]): a new clip cuts to its start, `playing` is live
//!   (a pause resumes where it stopped), a finished one-shot replays, and
//!   `speed`, `loop` and `autoplay` are the animator's.
//! - **The flipbook owns its frames.** A clip's period is its frames, and for
//!   ping-pong every frame forward then frames `n-2` down to `1` back (the
//!   ends aren't shown twice): a fold of the clip, so the cursor learns
//!   nothing of flipbooks. The kind steps through the period by each frame's
//!   `duration_ms`, in `f32` milliseconds as `sprite_sync`'s `advance_frame`
//!   did, so every frame lands on the step it landed on before (an `f64`
//!   second at a boundary lands a step apart either way, ADR 0139's risk),
//!   and keeps the cursor's `time` at the same place in seconds.
//! - **Looping is the clip's and the animator's together**: `loop_mode =
//!   "once"` plays once, and `animator.loop = false` makes any flipbook play
//!   once.
//! - **A crossfade is a cut**: frames are discrete, so a `blend_target` (and
//!   a sequence's `blend` step) switches clip when the crossfade would start.
//! - **It writes only on change**: `sprite.source_rect` when the frame shown
//!   changes, so a still sprite causes no change-detection churn.

use crate::clip::{LoopMode, SpriteAnimClip};
use flint_animation::animator::{self, Animator};
use flint_animation::{ClipEnd, ClipKind, ClipPlaybackState};
use flint_core::components as comp;
use flint_core::EntityId;
use flint_ecs::FlintWorld;
use std::collections::{BTreeSet, HashMap};

/// The kind's name.
pub const KIND: &str = "flipbook";

/// A clip as the kind plays it: its frames' order through one period.
#[derive(Debug, Clone)]
struct Book {
    clip: SpriteAnimClip,
    /// The frames in the order a period shows them (ping-pong folded)
    order: Vec<usize>,
    /// Where each step of `order` starts, in ms, and the period's length last
    starts: Vec<u32>,
}

impl Book {
    fn of(clip: SpriteAnimClip) -> Self {
        let n = clip.frames.len();
        let mut order: Vec<usize> = (0..n).collect();
        if clip.loop_mode == LoopMode::PingPong && n > 2 {
            order.extend((1..n - 1).rev());
        }
        let mut starts = Vec::with_capacity(order.len() + 1);
        let mut at = 0;
        for &frame in &order {
            starts.push(at);
            at += clip.frames[frame].duration_ms;
        }
        starts.push(at);
        Self {
            clip,
            order,
            starts,
        }
    }

    /// One period, in seconds: the cursor's clip length.
    fn period(&self) -> f64 {
        *self.starts.last().unwrap_or(&0) as f64 / 1000.0
    }

    fn loops(&self) -> bool {
        self.clip.loop_mode != LoopMode::Once
    }

    fn duration_ms(&self, step: usize) -> f32 {
        self.clip.frames[self.order[step]].duration_ms as f32
    }

    /// The step a time into the period falls in, and the ms into it.
    fn step_at(&self, time: f64) -> (usize, f32) {
        let ms = time * 1000.0;
        let step = (0..self.order.len())
            .rev()
            .find(|&s| self.starts[s] as f64 <= ms)
            .unwrap_or(0);
        (step, (ms - self.starts[step] as f64) as f32)
    }

    /// The step's place on the cursor, in seconds.
    fn time_of(&self, step: usize, elapsed_ms: f32) -> f64 {
        (self.starts[step] as f64 + elapsed_ms as f64) / 1000.0
    }

    /// The sheet's rectangle of the frame at `step`.
    fn rect(&self, step: usize) -> toml::Value {
        let frame = &self.clip.frames[self.order[step]];
        let floats = [
            (frame.col * self.clip.frame_width) as f64,
            (frame.row * self.clip.frame_height) as f64,
            self.clip.frame_width as f64,
            self.clip.frame_height as f64,
        ];
        toml::Value::Array(floats.into_iter().map(toml::Value::Float).collect())
    }
}

/// One sprite's flipbook: the cursor and where in its clip's frames it is.
#[derive(Debug, Clone)]
struct Playing {
    state: ClipPlaybackState,
    /// The step of the period shown and the ms into it, as `advance_frame`
    /// stepped them
    step: usize,
    elapsed_ms: f32,
    /// The cursor's time as the kind last set it: another time is the
    /// rules' (a new clip, a replay), which the frames follow
    time_set: f64,
    /// The clip and step whose rectangle the sprite shows
    shown: Option<(String, usize)>,
    /// A crossfade was cut this frame: the animator's `blend_target` is
    /// retired, or it would ask again
    cut: bool,
}

/// The flipbook clip kind: every `.sprite.toml` clip the scene can see, and
/// each sprite playing one.
#[derive(Default)]
pub struct FlipbookKind {
    books: HashMap<String, Book>,
    playing: HashMap<EntityId, Playing>,
}

impl FlipbookKind {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a clip; one of the same name replaces it.
    pub fn add_clip(&mut self, clip: SpriteAnimClip) {
        self.books.insert(clip.name.clone(), Book::of(clip));
    }

    /// The animator as this clip plays it: `loop` is the animator's and the
    /// clip's together.
    fn as_played<'a>(&self, animator: &Animator<'a>, current: Option<&str>) -> Animator<'a> {
        let mut played = animator.clone();
        let clip = match current {
            Some(current)
                if animator.clip.is_empty() || !self.books.contains_key(animator.clip) =>
            {
                current
            }
            _ => animator.clip,
        };
        played.looping = animator.looping && self.books.get(clip).is_none_or(Book::loops);
        played
    }
}

impl ClipKind for FlipbookKind {
    fn name(&self) -> &'static str {
        KIND
    }

    fn clip_count(&self) -> usize {
        self.books.len()
    }

    fn active_count(&self) -> usize {
        self.playing.len()
    }

    fn has_clip(&self, clip: &str) -> bool {
        self.books.contains_key(clip)
    }

    /// Every sprite with an animator (or, for one release, the retired
    /// `sprite_animator`, which `Animator::of` reads as one) whose clip is a
    /// flipbook.
    fn sync_from_world(&mut self, world: &FlintWorld) {
        let entities: BTreeSet<EntityId> = world
            .entities_with_component(comp::ANIMATOR)
            .iter()
            .chain(world.entities_with_component(comp::SPRITE_ANIMATOR))
            .copied()
            .collect();
        for entity in entities {
            if !world
                .get_components(entity)
                .is_some_and(|c| c.has(comp::SPRITE))
            {
                continue;
            }
            let animator = Animator::of(world, entity);
            let books = &self.books;
            let duration_of = |name: &str| books.get(name).map(Book::period);
            match self.playing.get(&entity).map(|p| p.state.clip_name.clone()) {
                Some(current) => {
                    let played = self.as_played(&animator, Some(&current));
                    let books = &self.books;
                    let duration_of = |name: &str| books.get(name).map(Book::period);
                    let p = self.playing.get_mut(&entity).expect("just found");
                    played.apply_to(&mut p.state, duration_of);
                    // A crossfade is a cut: the target plays from its start
                    if let Some(period) = duration_of(&p.state.blend_target) {
                        let target = p.state.blend_target.clone();
                        p.state.cut_to(&target, period);
                        p.cut = true;
                    }
                }
                None => {
                    let played = self.as_played(&animator, None);
                    if let Some(mut state) = played.start(duration_of) {
                        let mut cut = false;
                        if let Some(period) = duration_of(&state.blend_target) {
                            let target = state.blend_target.clone();
                            state.cut_to(&target, period);
                            cut = true;
                        }
                        let time = state.time;
                        let (step, elapsed_ms) = self
                            .books
                            .get(&state.clip_name)
                            .map_or((0, 0.0), |b| b.step_at(time));
                        self.playing.insert(
                            entity,
                            Playing {
                                state,
                                step,
                                elapsed_ms,
                                time_set: time,
                                shown: None,
                                cut,
                            },
                        );
                    }
                }
            }
        }
    }

    fn advance(&mut self, world: &mut FlintWorld, dt: f64, ends: &mut Vec<ClipEnd>) {
        let dt_ms = (dt * 1000.0) as f32;
        for (&entity, p) in &mut self.playing {
            let Some(book) = self.books.get(&p.state.clip_name) else {
                continue;
            };
            if book.order.is_empty() {
                continue;
            }
            // The rules moved the cursor (a new clip, a replay): the frames
            // follow it
            let clip_moved = p
                .shown
                .as_ref()
                .is_some_and(|(c, _)| *c != p.state.clip_name);
            if p.state.time != p.time_set || clip_moved {
                (p.step, p.elapsed_ms) = book.step_at(p.state.time);
            }
            if p.state.playing {
                let end = book.order.len() - 1;
                p.elapsed_ms += dt_ms * p.state.speed as f32;
                // Forward, as `advance_frame` stepped
                while p.state.playing && p.elapsed_ms >= book.duration_ms(p.step) {
                    if p.step == end && !p.state.looping {
                        p.elapsed_ms = book.duration_ms(p.step);
                        p.state.finish_at(book.period());
                        ends.push(ClipEnd {
                            entity_id: entity,
                            clip_name: p.state.clip_name.clone(),
                        });
                        break;
                    }
                    p.elapsed_ms -= book.duration_ms(p.step);
                    p.step = if p.step == end { 0 } else { p.step + 1 };
                }
                // Backward, for a negative speed
                while p.state.playing && p.elapsed_ms < 0.0 {
                    if p.step == 0 && !p.state.looping {
                        p.elapsed_ms = 0.0;
                        p.state.finish_at(0.0);
                        ends.push(ClipEnd {
                            entity_id: entity,
                            clip_name: p.state.clip_name.clone(),
                        });
                        break;
                    }
                    p.step = if p.step == 0 { end } else { p.step - 1 };
                    p.elapsed_ms += book.duration_ms(p.step);
                }
                if p.state.playing {
                    p.state.time = book.time_of(p.step, p.elapsed_ms);
                }
            }
            p.time_set = p.state.time;

            let shown = (p.state.clip_name.clone(), p.step);
            let changed = p.shown.as_ref() != Some(&shown);
            if !changed && !p.state.is_unpublished() && !p.cut {
                continue;
            }
            let Some(mut comps) = world.get_components_mut(entity) else {
                continue;
            };
            if changed {
                comps.set_field(comp::SPRITE, "source_rect", book.rect(p.step));
                p.shown = Some(shown);
            }
            if std::mem::take(&mut p.cut) {
                animator::write(&mut comps).no_blend();
            }
            // A one-shot's end, an autoplay's start: the animator says so
            animator::publish(&mut p.state, &mut comps);
        }
    }

    fn state(&self, entity: EntityId) -> Option<&ClipPlaybackState> {
        self.playing.get(&entity).map(|p| &p.state)
    }

    fn reset(&mut self, entity: EntityId) {
        self.playing.remove(&entity);
    }

    fn clear(&mut self) {
        self.playing.clear();
    }
}

#[cfg(test)]
mod tests;
