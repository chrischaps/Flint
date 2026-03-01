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
        }
    }
}
