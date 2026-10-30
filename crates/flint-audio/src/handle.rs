//! Handles to what the engine plays, wherever it is mixed (#136).
//!
//! On the desktop they wrap kira's handles. In the browser the mix runs in
//! an `AudioWorklet`, and the same calls become commands to it
//! ([`crate::remote`]). Either way a handle behaves like kira's: dropping a
//! [`SoundHandle`] lets the sound play on, dropping a [`SpatialTrack`]
//! removes the track and what plays on it.

use kira::sound::static_sound::StaticSoundHandle;
use kira::track::SpatialTrackHandle;
use kira::{Decibels, PlaybackRate, Tween};

use crate::remote::{Command, RemoteTrack, RemoteVoice};

/// A sound that was played.
pub enum SoundHandle {
    Local(StaticSoundHandle),
    Remote(RemoteVoice),
}

impl SoundHandle {
    pub fn set_volume(&mut self, volume: Decibels, tween: Tween) {
        match self {
            Self::Local(h) => h.set_volume(volume, tween),
            Self::Remote(v) => v.link.send(Command::VoiceVolume {
                voice: v.id,
                db: volume.0,
                tween: tween.duration.as_secs_f32(),
            }),
        }
    }

    pub fn set_playback_rate(&mut self, rate: PlaybackRate, tween: Tween) {
        match self {
            Self::Local(h) => h.set_playback_rate(rate, tween),
            Self::Remote(v) => v.link.send(Command::VoiceRate {
                voice: v.id,
                rate: rate.0 as f32,
                tween: tween.duration.as_secs_f32(),
            }),
        }
    }

    pub fn stop(&mut self, tween: Tween) {
        match self {
            Self::Local(h) => h.stop(tween),
            Self::Remote(v) => v.link.send(Command::VoiceStop {
                voice: v.id,
                tween: tween.duration.as_secs_f32(),
            }),
        }
    }
}

/// A track whose sounds are placed in the world.
pub enum SpatialTrack {
    /// Boxed: kira's handle is about 300 bytes, a remote one 16.
    Local(Box<SpatialTrackHandle>),
    Remote(RemoteTrack),
}

impl SpatialTrack {
    pub fn set_position(&mut self, position: glam::Vec3, tween: Tween) {
        match self {
            Self::Local(h) => h.set_position(position, tween),
            Self::Remote(t) => t.link.send(Command::TrackPosition {
                track: t.id,
                pos: position.to_array(),
            }),
        }
    }
}
