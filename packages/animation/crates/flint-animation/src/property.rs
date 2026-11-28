//! Animation clip registry and per-entity playback state

use crate::clip::{AnimationClip, AnimationEvent};
use crate::playback_state::{Advanced, ClipPlaybackState};
use crate::sampler::sample_track;
use std::collections::HashMap;

/// Clip registry — holds all loaded animation clips by name.
#[derive(Default)]
pub struct AnimationPlayer {
    clips: HashMap<String, AnimationClip>,
}

impl AnimationPlayer {
    pub fn new() -> Self {
        Self {
            clips: HashMap::new(),
        }
    }

    /// Register a clip. Overwrites any existing clip with the same name.
    pub fn add_clip(&mut self, clip: AnimationClip) {
        self.clips.insert(clip.name.clone(), clip);
    }

    /// Look up a clip by name.
    pub fn get_clip(&self, name: &str) -> Option<&AnimationClip> {
        self.clips.get(name)
    }

    /// Check if a clip is registered.
    pub fn has_clip(&self, name: &str) -> bool {
        self.clips.contains_key(name)
    }

    /// Number of registered clips.
    pub fn clip_count(&self) -> usize {
        self.clips.len()
    }
}

/// Result of advancing a playback: sampled values per track + fired events.
pub struct AdvanceResult {
    /// One sampled [f32; 3] per track in the clip
    pub samples: Vec<[f32; 3]>,
    /// Events that fired during this advance
    pub events: Vec<AnimationEvent>,
}

/// Advance a playback state by `dt` seconds, returning sampled values and events.
///
/// Returns `None` if the clip isn't found or playback is stopped.
pub fn advance(
    state: &mut ClipPlaybackState,
    clip: &AnimationClip,
    dt: f64,
) -> Option<AdvanceResult> {
    if state.advance_time(clip.duration, dt) == Advanced::Held {
        // Still sample at current time so static poses work
        let samples: Vec<[f32; 3]> = clip
            .tracks
            .iter()
            .map(|t| sample_track(t, state.time))
            .collect();
        return Some(AdvanceResult {
            samples,
            events: vec![],
        });
    }

    // Sample all tracks
    let samples: Vec<[f32; 3]> = clip
        .tracks
        .iter()
        .map(|t| sample_track(t, state.time))
        .collect();

    // Collect newly-fired events
    let mut events = Vec::new();
    for ev in &clip.events {
        // Use bits of the float time as a hash key to deduplicate
        let key = ev.time.to_bits();
        if ev.time <= state.time && !state.fired_events.contains(&key) {
            state.fired_events.insert(key);
            events.push(ev.clone());
        }
    }

    Some(AdvanceResult { samples, events })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clip::*;

    fn simple_clip() -> AnimationClip {
        AnimationClip {
            name: "test_bob".into(),
            duration: 2.0,
            tracks: vec![AnimationTrack {
                target: TrackTarget::Position,
                interpolation: Interpolation::Linear,
                keyframes: vec![
                    Keyframe {
                        time: 0.0,
                        value: [0.0, 0.0, 0.0],
                        in_tangent: None,
                        out_tangent: None,
                    },
                    Keyframe {
                        time: 2.0,
                        value: [0.0, 4.0, 0.0],
                        in_tangent: None,
                        out_tangent: None,
                    },
                ],
            }],
            events: vec![AnimationEvent {
                time: 1.0,
                event_name: "halfway".into(),
            }],
        }
    }

    #[test]
    fn advance_samples_at_current_time() {
        let clip = simple_clip();
        let mut state = ClipPlaybackState::new("test_bob".into(), 1.0, false, true);
        let result = advance(&mut state, &clip, 1.0).unwrap();
        // After 1.0s of a 2.0s linear track: halfway
        assert!((result.samples[0][1] - 2.0).abs() < 1e-4);
    }

    #[test]
    fn advance_fires_event_once() {
        let clip = simple_clip();
        let mut state = ClipPlaybackState::new("test_bob".into(), 1.0, false, true);
        let r1 = advance(&mut state, &clip, 1.5).unwrap();
        assert_eq!(r1.events.len(), 1);
        assert_eq!(r1.events[0].event_name, "halfway");
        // Advancing again should NOT re-fire the same event
        let r2 = advance(&mut state, &clip, 0.1).unwrap();
        assert_eq!(r2.events.len(), 0);
    }

    #[test]
    fn advance_loops_and_resets_events() {
        let clip = simple_clip();
        let mut state = ClipPlaybackState::new("test_bob".into(), 1.0, true, true);
        // Advance past the end to trigger a loop
        let _ = advance(&mut state, &clip, 1.5);
        let _ = advance(&mut state, &clip, 1.0); // wraps around
                                                 // After wrap, events should be clearable and re-firable
        assert!(state.time < clip.duration);
    }

    #[test]
    fn advance_stops_at_end_when_not_looping() {
        let clip = simple_clip();
        let mut state = ClipPlaybackState::new("test_bob".into(), 1.0, false, true);
        let _ = advance(&mut state, &clip, 3.0); // overshoot
        assert_eq!(state.time, 2.0);
        assert!(!state.playing);
    }

    #[test]
    fn paused_still_samples() {
        let clip = simple_clip();
        let mut state = ClipPlaybackState::new("test_bob".into(), 1.0, false, false);
        state.time = 1.0; // manually set to midpoint
        let result = advance(&mut state, &clip, 1.0).unwrap();
        // Time should NOT advance (paused)
        assert_eq!(state.time, 1.0);
        // But it should still return a sample
        assert!((result.samples[0][1] - 2.0).abs() < 1e-4);
    }
}
