//! The one playback state for property, node and skeletal clips

use std::collections::HashSet;

use flint_core::tween::Easing;

/// How an animation layer composes onto the pose beneath it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LayerMode {
    /// Each keyed joint contributes its delta-from-rest, scaled by weight.
    /// Good for overlays that were authored as "the rest pose plus a
    /// gesture" (a star-arm cower, a breathing chest).
    #[default]
    Additive,
    /// Each keyed joint is replaced by the layer's sampled pose, blended
    /// toward it by weight. Good for "upper body does X while the legs
    /// keep walking", usually paired with a mask.
    Override,
}

impl LayerMode {
    pub fn parse(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "override" | "replace" => LayerMode::Override,
            _ => LayerMode::Additive,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            LayerMode::Additive => "additive",
            LayerMode::Override => "override",
        }
    }
}

/// One animation layer: a clip looping on its own clock, composed onto
/// the base pose (after any crossfade) in array order.
#[derive(Debug, Clone, PartialEq)]
pub struct AnimLayer {
    /// Clip name; empty = inactive slot (kept so indices stay stable)
    pub clip: String,
    /// Live dial, 0 = off, 1 = full
    pub weight: f32,
    pub mode: LayerMode,
    /// Root joint name of a subtree mask; empty = every joint the clip keys
    pub mask: String,
    /// Independent playback time
    pub time: f64,
    /// Multiplier on the entity's base speed
    pub speed: f64,
    /// Weight to ramp toward when `fade_duration > 0` (see
    /// `SkeletalSync` — the ramped weight is written back to the ECS each
    /// frame, and `fade_duration` is reset to 0 when the ramp completes).
    pub fade_target: f32,
    /// Seconds for the ramp; 0 = no fade in progress
    pub fade_duration: f32,
    /// How the ramp moves (#734): linear unless the layer says otherwise
    pub fade_easing: Easing,
}

impl Default for AnimLayer {
    fn default() -> Self {
        Self {
            clip: String::new(),
            weight: 1.0,
            mode: LayerMode::Additive,
            mask: String::new(),
            time: 0.0,
            speed: 1.0,
            fade_target: 1.0,
            fade_duration: 0.0,
            fade_easing: Easing::Linear,
        }
    }
}

impl AnimLayer {
    pub fn new(clip: impl Into<String>, weight: f32) -> Self {
        Self {
            clip: clip.into(),
            weight,
            ..Default::default()
        }
    }

    pub fn is_active(&self) -> bool {
        !self.clip.is_empty()
    }

    // Read from, and written to, one `animator.layers` entry in
    // `crate::animator`, which holds every animator key (#733).
}

/// One entity's clip playback, the one state every `animator` sync keeps:
/// property clips (`AnimationSync`), node clips (`NodeSync`) and skinned
/// clips (`SkeletalSync`). What the animator asks of it is applied by
/// [`crate::animator::Animator::apply_to`], with one rule set for all three
/// (#733); the fields only a kind uses (the crossfade and layers are
/// skeletal, fired events are property clips') stay empty in the others.
#[derive(Debug, Clone)]
pub struct ClipPlaybackState {
    pub clip_name: String,
    /// Seconds into the clip
    pub time: f64,
    /// Multiplier on time (1 normal, negative plays backwards)
    pub speed: f64,
    pub looping: bool,
    pub playing: bool,
    /// Clip name to crossfade into (empty = no blend)
    pub blend_target: String,
    /// Duration of the crossfade in seconds
    pub blend_duration: f32,
    /// How the crossfade's weight moves (#1248); linear unless written
    pub blend_easing: Easing,
    /// Time elapsed in the current blend
    pub blend_elapsed: f32,
    /// Ordered animation layers, composed after base + crossfade. A legacy
    /// `layer_clip` / `layer_weight` pair is read as `layers[0]`.
    pub layers: Vec<AnimLayer>,
    /// The runtime changed `clip_name` or `playing` (a one-shot ended, a
    /// crossfade settled, `autoplay` started a clip whose `playing` is
    /// false) and the animator doesn't say so yet. Until the sync publishes
    /// them ([`crate::animator::publish`]) `apply_to` leaves the two alone,
    /// or it would read the stale values as a `play_clip` / `stop_clip`.
    pub(crate) unpublished: bool,
    /// Property clips' events fired this pass (by time bits), so each fires
    /// once a loop.
    pub(crate) fired_events: HashSet<u64>,
}

/// What one [`ClipPlaybackState::advance_time`] did to the clock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Advanced {
    /// Paused: the clock stood still.
    Held,
    /// Moved within the clip.
    Moved,
    /// A looping clip went past an end and wrapped.
    Wrapped,
    /// A one-shot reached the end it plays toward and stopped there.
    Finished,
}

impl ClipPlaybackState {
    pub fn new(clip_name: String, speed: f64, looping: bool, playing: bool) -> Self {
        Self {
            clip_name,
            time: 0.0,
            speed,
            looping,
            playing,
            blend_target: String::new(),
            blend_duration: crate::animator::DEFAULT_BLEND_SECONDS,
            blend_easing: Easing::Linear,
            blend_elapsed: 0.0,
            layers: Vec::new(),
            unpublished: false,
            fired_events: HashSet::new(),
        }
    }

    /// Whether the runtime changed `clip_name` or `playing` since the
    /// animator was last told.
    pub fn is_unpublished(&self) -> bool {
        self.unpublished
    }

    /// Clear fired event tracking so events can fire again on replay.
    pub fn clear_fired_events(&mut self) {
        self.fired_events.clear();
    }

    /// Move the clock `dt * speed` seconds through a clip of `duration`:
    /// a looping clip wraps, a one-shot stops at the end it plays toward
    /// (which the animator must then be told). The one clock every kind of
    /// clip runs on.
    pub fn advance_time(&mut self, duration: f64, dt: f64) -> Advanced {
        if !self.playing {
            return Advanced::Held;
        }
        self.time += dt * self.speed;
        if self.looping {
            if duration <= 0.0 {
                return Advanced::Moved;
            }
            if self.time >= duration {
                self.time %= duration;
            } else if self.time < 0.0 {
                self.time = duration - (-self.time % duration);
            } else {
                return Advanced::Moved;
            }
            self.fired_events.clear();
            return Advanced::Wrapped;
        }
        if self.time >= duration {
            self.time = duration;
        } else if self.time < 0.0 {
            self.time = 0.0;
        } else {
            return Advanced::Moved;
        }
        self.playing = false;
        self.unpublished = true;
        Advanced::Finished
    }
}

/// Where a clip starts for its direction of play: 0, or its end when the
/// speed is negative (played backwards from 0 it would end at once).
pub fn start_time(duration: f64, speed: f64) -> f64 {
    if speed < 0.0 {
        duration
    } else {
        0.0
    }
}

/// Whether a non-looping clip stands at the end it plays toward.
pub fn has_finished(time: f64, duration: f64, speed: f64) -> bool {
    if speed < 0.0 {
        time <= 0.0
    } else {
        time >= duration
    }
}
