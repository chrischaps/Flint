//! Unified playback state for skeletal and node animation clips

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

    /// Serialize to the TOML table shape used by `animator.layers`.
    pub fn to_toml(&self) -> toml::Value {
        let mut t = toml::map::Map::new();
        t.insert("clip".into(), toml::Value::String(self.clip.clone()));
        t.insert("weight".into(), toml::Value::Float(self.weight as f64));
        t.insert(
            "mode".into(),
            toml::Value::String(self.mode.as_str().into()),
        );
        t.insert("mask".into(), toml::Value::String(self.mask.clone()));
        t.insert("speed".into(), toml::Value::Float(self.speed));
        if self.fade_duration > 0.0 {
            t.insert(
                "fade_target".into(),
                toml::Value::Float(self.fade_target as f64),
            );
            t.insert(
                "fade_duration".into(),
                toml::Value::Float(self.fade_duration as f64),
            );
        }
        toml::Value::Table(t)
    }

    /// Parse from one `animator.layers` entry. Non-table values yield an
    /// inactive slot so indices stay stable.
    pub fn from_toml(v: &toml::Value) -> Self {
        use flint_core::toml_util::{toml_f32, toml_f64};
        let Some(t) = v.as_table() else {
            return Self::default();
        };
        let weight = t.get("weight").and_then(toml_f32).unwrap_or(1.0);
        Self {
            clip: t
                .get("clip")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            weight,
            fade_target: t.get("fade_target").and_then(toml_f32).unwrap_or(weight),
            fade_duration: t
                .get("fade_duration")
                .and_then(toml_f32)
                .unwrap_or(0.0)
                .max(0.0),
            mode: t
                .get("mode")
                .and_then(|v| v.as_str())
                .map(LayerMode::parse)
                .unwrap_or_default(),
            mask: t
                .get("mask")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            time: 0.0,
            speed: t.get("speed").and_then(toml_f64).unwrap_or(1.0),
        }
    }
}

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
    /// Legacy single additive layer clip (mirror of `animator.layer_clip`).
    /// When `animator.layers` is empty this becomes `layers[0]`.
    pub layer_clip: String,
    /// Legacy additive layer strength (0 = off, 1 = full)
    pub layer_weight: f32,
    /// Legacy layer playback time
    pub layer_time: f64,
    /// Ordered animation layers, composed after base + crossfade.
    pub layers: Vec<AnimLayer>,
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
            layers: Vec::new(),
        }
    }
}
