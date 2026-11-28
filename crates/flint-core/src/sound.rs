//! The sound's plain data, for a host that reads it without naming the
//! audio package (#1619): what the mixer is doing, for the perf readout.
//! The audio package's `flint_audio::MixCounts` is this type.

/// What the mixer is doing, for the perf readout (#138): the sounds and
/// spatial tracks kira holds, and how many of each are paused because
/// nobody can hear them. A paused track skips its sounds and its
/// spatialisation; a paused sound skips its resampling.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MixCounts {
    /// Sounds on the buses and on spatial tracks.
    pub voices: usize,
    /// Of those, paused ones, and ones on a paused track.
    pub paused_voices: usize,
    /// Spatial tracks: `audio_source`s' and placed one-shots'.
    pub tracks: usize,
    /// Of those, paused ones.
    pub paused_tracks: usize,
    /// Placed one-shots never started because the listener was past their
    /// `max` distance.
    pub skipped_oneshots: u64,
    /// One-shots stopped early to make room under
    /// `MAX_ONESHOTS_PER_SOUND` (flint-audio).
    pub stolen_oneshots: u64,
}

impl MixCounts {
    /// Voices the mixer runs.
    pub fn live_voices(&self) -> usize {
        self.voices.saturating_sub(self.paused_voices)
    }

    /// Spatial tracks the mixer runs.
    pub fn live_tracks(&self) -> usize {
        self.tracks.saturating_sub(self.paused_tracks)
    }
}
