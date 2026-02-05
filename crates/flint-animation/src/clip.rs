//! Core animation data types

use serde::{Deserialize, Serialize};

/// A complete animation clip with named tracks and timed events
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnimationClip {
    /// Human-readable name
    pub name: String,
    /// Total duration in seconds
    pub duration: f64,
    /// Animated property tracks
    pub tracks: Vec<AnimationTrack>,
    /// Events fired at specific times
    #[serde(default)]
    pub events: Vec<AnimationEvent>,
}

/// A single animated property track (e.g. position over time)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnimationTrack {
    /// What property this track drives
    pub target: TrackTarget,
    /// Interpolation mode between keyframes
    #[serde(default)]
    pub interpolation: Interpolation,
    /// Sorted keyframes (by time)
    pub keyframes: Vec<Keyframe>,
}

/// A keyframe: a value at a point in time
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Keyframe {
    /// Time in seconds from clip start
    pub time: f64,
    /// Value (3 floats — position xyz, rotation euler, scale xyz, or [v, 0, 0] for scalar)
    pub value: [f32; 3],
    /// Incoming tangent for cubic spline
    #[serde(default)]
    pub in_tangent: Option<[f32; 3]>,
    /// Outgoing tangent for cubic spline
    #[serde(default)]
    pub out_tangent: Option<[f32; 3]>,
}

/// What property an animation track drives
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type")]
pub enum TrackTarget {
    /// Drives transform.position
    Position,
    /// Drives transform.rotation (euler degrees)
    Rotation,
    /// Drives transform.scale
    Scale,
    /// Drives an arbitrary float field on a named component
    CustomFloat {
        component: String,
        field: String,
    },
}

/// How to interpolate between keyframes
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Default)]
pub enum Interpolation {
    /// Jump to next value (no blending)
    Step,
    /// Linear interpolation
    #[default]
    Linear,
    /// Cubic Hermite spline (requires tangents)
    CubicSpline,
}

/// An event fired at a specific time during playback
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnimationEvent {
    /// Time in seconds when this event fires
    pub time: f64,
    /// Event name (consumed by game logic / audio triggers)
    pub event_name: String,
}
