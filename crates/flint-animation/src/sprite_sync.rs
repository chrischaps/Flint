//! Bridges ECS `sprite_animator` components to sprite sheet animation playback
//!
//! Follows the same dual-phase pattern as `AnimationSync`:
//! 1. `sync_from_world()` reads ECS state, detects clip/playing changes
//! 2. `advance_and_write()` steps playback, writes `sprite.source_rect` back to ECS

use crate::sprite_clip::{LoopMode, SpriteAnimClip};
use flint_core::toml_util::toml_f32;
use flint_core::EntityId;
use flint_ecs::FlintWorld;
use std::collections::HashMap;

/// Per-entity playback state for sprite animation.
#[derive(Debug, Clone)]
struct SpritePlaybackState {
    clip_name: String,
    elapsed_ms: f32,
    current_frame_index: usize,
    /// +1 forward, -1 backward (for ping-pong)
    direction: i8,
    playing: bool,
    speed: f32,
    /// Set when source_rect needs to be written (new entity, clip change, frame advance)
    dirty: bool,
}

impl SpritePlaybackState {
    fn new(clip_name: String, playing: bool, speed: f32) -> Self {
        Self {
            clip_name,
            elapsed_ms: 0.0,
            current_frame_index: 0,
            direction: 1,
            playing,
            speed,
            dirty: true, // write initial frame
        }
    }

    fn reset(&mut self) {
        self.elapsed_ms = 0.0;
        self.current_frame_index = 0;
        self.direction = 1;
        self.dirty = true;
    }
}

/// Event emitted when a `Once` clip finishes playing.
#[derive(Debug, Clone)]
pub struct SpriteAnimEndEvent {
    pub entity_id: EntityId,
    pub clip_name: String,
}

/// Manages sprite sheet animation playback for all entities with `sprite_animator`.
pub struct SpriteAnimSync {
    clips: HashMap<String, SpriteAnimClip>,
    states: HashMap<EntityId, SpritePlaybackState>,
    /// Events from this frame — drained by the script system.
    pending_events: Vec<SpriteAnimEndEvent>,
}

impl SpriteAnimSync {
    pub fn new() -> Self {
        Self {
            clips: HashMap::new(),
            states: HashMap::new(),
            pending_events: Vec::new(),
        }
    }

    /// Register a sprite animation clip.
    pub fn add_clip(&mut self, clip: SpriteAnimClip) {
        self.clips.insert(clip.name.clone(), clip);
    }

    /// Check if a clip is registered.
    pub fn has_clip(&self, name: &str) -> bool {
        self.clips.contains_key(name)
    }

    /// Number of registered sprite clips.
    pub fn clip_count(&self) -> usize {
        self.clips.len()
    }

    /// Number of active animated entities.
    pub fn active_count(&self) -> usize {
        self.states.len()
    }

    /// Drain animation-end events for script delivery.
    pub fn drain_events(&mut self) -> Vec<SpriteAnimEndEvent> {
        std::mem::take(&mut self.pending_events)
    }

    /// Clear all playback state for scene transitions. Preserves clip registry.
    pub fn clear(&mut self) {
        self.states.clear();
        self.pending_events.clear();
    }

    /// Scan the world for entities with `sprite_animator`, creating or updating
    /// playback states as needed.
    pub fn sync_from_world(&mut self, world: &FlintWorld) {
        for &entity_id in world.entities_with_component("sprite_animator") {
            let Some(components) = world.get_components(entity_id) else {
                continue;
            };
            let Some(sprite_animator) = components.get("sprite_animator") else {
                continue;
            };

            let clip_name = sprite_animator
                .get("clip")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();

            if clip_name.is_empty() || !self.clips.contains_key(&clip_name) {
                continue;
            }

            let speed = sprite_animator
                .get("speed")
                .and_then(toml_f32)
                .unwrap_or(1.0);

            let ecs_playing = sprite_animator
                .get("playing")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);

            if let Some(state) = self.states.get_mut(&entity_id) {
                // Clip changed → reset playback
                if state.clip_name != clip_name {
                    state.clip_name = clip_name;
                    state.reset();
                    state.playing = ecs_playing;
                    state.speed = speed;
                } else {
                    // ECS says play but we're stopped → restart
                    if ecs_playing && !state.playing {
                        state.playing = true;
                        state.reset();
                    }
                    state.speed = speed;
                }
                continue;
            }

            self.states.insert(
                entity_id,
                SpritePlaybackState::new(clip_name, ecs_playing, speed),
            );
        }
    }

    /// Advance all playback timers and write `sprite.source_rect` back to ECS.
    pub fn advance_and_write(&mut self, world: &mut FlintWorld, dt: f64) {
        let dt_ms = (dt * 1000.0) as f32;

        for (entity_id, state) in &mut self.states {
            let Some(clip) = self.clips.get(&state.clip_name) else {
                continue;
            };

            if clip.frames.is_empty() {
                continue;
            }

            if state.playing {
                state.elapsed_ms += dt_ms * state.speed;

                let current_duration = clip.frames[state.current_frame_index].duration_ms as f32;
                while state.elapsed_ms >= current_duration && state.playing {
                    state.elapsed_ms -= current_duration;
                    let prev_frame = state.current_frame_index;
                    advance_frame(state, clip, *entity_id, &mut self.pending_events);
                    if state.current_frame_index != prev_frame {
                        state.dirty = true;
                    }
                    if !state.playing {
                        state.dirty = true;
                        break;
                    }
                    // Re-read duration for new frame
                    let new_dur = clip.frames[state.current_frame_index].duration_ms as f32;
                    if state.elapsed_ms < new_dur {
                        break;
                    }
                }
            }

            // Only write source_rect when something changed
            if state.dirty {
                state.dirty = false;

                let frame = &clip.frames[state.current_frame_index];
                let x = (frame.col * clip.frame_width) as f64;
                let y = (frame.row * clip.frame_height) as f64;
                let w = clip.frame_width as f64;
                let h = clip.frame_height as f64;

                if let Some(components) = world.get_components_mut(*entity_id) {
                    components.set_field(
                        "sprite",
                        "source_rect",
                        toml::Value::Array(vec![
                            toml::Value::Float(x),
                            toml::Value::Float(y),
                            toml::Value::Float(w),
                            toml::Value::Float(h),
                        ]),
                    );

                    // If clip just finished, write playing=false back
                    if !state.playing {
                        components.set_field(
                            "sprite_animator",
                            "playing",
                            toml::Value::Boolean(false),
                        );
                    }
                }
            }
        }
    }
}

impl Default for SpriteAnimSync {
    fn default() -> Self {
        Self::new()
    }
}

/// Advance to the next frame, handling loop modes.
fn advance_frame(
    state: &mut SpritePlaybackState,
    clip: &SpriteAnimClip,
    entity_id: EntityId,
    events: &mut Vec<SpriteAnimEndEvent>,
) {
    let frame_count = clip.frames.len();
    if frame_count <= 1 {
        state.playing = false;
        return;
    }

    let next = state.current_frame_index as i64 + state.direction as i64;

    match clip.loop_mode {
        LoopMode::Loop => {
            state.current_frame_index = if next >= frame_count as i64 {
                0
            } else if next < 0 {
                frame_count - 1
            } else {
                next as usize
            };
        }
        LoopMode::PingPong => {
            if next >= frame_count as i64 {
                state.direction = -1;
                state.current_frame_index = frame_count.saturating_sub(2);
            } else if next < 0 {
                state.direction = 1;
                state.current_frame_index = 1.min(frame_count - 1);
            } else {
                state.current_frame_index = next as usize;
            }
        }
        LoopMode::Once => {
            if next >= frame_count as i64 || next < 0 {
                state.current_frame_index = if state.direction > 0 {
                    frame_count - 1
                } else {
                    0
                };
                state.playing = false;
                events.push(SpriteAnimEndEvent {
                    entity_id,
                    clip_name: state.clip_name.clone(),
                });
            } else {
                state.current_frame_index = next as usize;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sprite_clip::SpriteAnimFrame;

    fn test_clip(name: &str, loop_mode: LoopMode) -> SpriteAnimClip {
        SpriteAnimClip {
            name: name.to_string(),
            texture: "sheet.png".to_string(),
            frame_width: 32,
            frame_height: 32,
            frames: vec![
                SpriteAnimFrame {
                    col: 0,
                    row: 0,
                    duration_ms: 100,
                },
                SpriteAnimFrame {
                    col: 1,
                    row: 0,
                    duration_ms: 100,
                },
                SpriteAnimFrame {
                    col: 2,
                    row: 0,
                    duration_ms: 100,
                },
                SpriteAnimFrame {
                    col: 3,
                    row: 0,
                    duration_ms: 100,
                },
            ],
            loop_mode,
        }
    }

    #[test]
    fn advance_frame_loop_wraps() {
        let clip = test_clip("test", LoopMode::Loop);
        let mut state = SpritePlaybackState::new("test".into(), true, 1.0);
        state.current_frame_index = 3;
        let mut events = Vec::new();
        advance_frame(&mut state, &clip, EntityId::from_raw(1), &mut events);
        assert_eq!(state.current_frame_index, 0);
        assert!(state.playing);
        assert!(events.is_empty());
    }

    #[test]
    fn advance_frame_once_stops() {
        let clip = test_clip("test", LoopMode::Once);
        let mut state = SpritePlaybackState::new("test".into(), true, 1.0);
        state.current_frame_index = 3;
        let mut events = Vec::new();
        advance_frame(&mut state, &clip, EntityId::from_raw(1), &mut events);
        assert_eq!(state.current_frame_index, 3);
        assert!(!state.playing);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].clip_name, "test");
    }

    #[test]
    fn advance_frame_pingpong_reverses() {
        let clip = test_clip("test", LoopMode::PingPong);
        let mut state = SpritePlaybackState::new("test".into(), true, 1.0);
        state.current_frame_index = 3;
        let mut events = Vec::new();
        advance_frame(&mut state, &clip, EntityId::from_raw(1), &mut events);
        assert_eq!(state.direction, -1);
        assert_eq!(state.current_frame_index, 2);
        assert!(state.playing);
    }

    #[test]
    fn clip_registry() {
        let mut sync = SpriteAnimSync::new();
        assert_eq!(sync.clip_count(), 0);
        sync.add_clip(test_clip("walk", LoopMode::Loop));
        assert_eq!(sync.clip_count(), 1);
        assert!(sync.has_clip("walk"));
        assert!(!sync.has_clip("run"));
    }
}
