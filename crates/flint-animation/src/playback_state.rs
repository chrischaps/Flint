//! Unified playback state for skeletal and node animation clips

/// Per-entity clip playback state shared by skeletal and node animation.
///
/// Tracks current time, speed, looping, and crossfade blending parameters.
#[derive(Debug, Clone)]
pub struct ClipPlaybackState {
    pub clip_name: String,
    pub time: f64,
    pub speed: f64,
    pub looping: bool,
    pub playing: bool,
    /// Clip name to crossfade into (empty = no blend)
    pub blend_target: String,
    /// Duration of the crossfade in seconds
    pub blend_duration: f32,
    /// Time elapsed in the current blend
    pub blend_elapsed: f32,
    /// Additive layer clip (empty = none): loops independently and
    /// composes onto the base pose relative to the joints' REST pose,
    /// only for joints the layer clip keys. Survives base crossfades.
    pub layer_clip: String,
    /// Additive layer strength (0 = off, 1 = full)
    pub layer_weight: f32,
    /// Independent playback time of the layer clip
    pub layer_time: f64,
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
            blend_duration: 0.3,
            blend_elapsed: 0.0,
            layer_clip: String::new(),
            layer_weight: 1.0,
            layer_time: 0.0,
        }
    }
}
