//! Runtime quality settings (FlintKart#55): the renderer's load levers as one
//! value a game can change while it runs, from a script (`set_quality`), the
//! command line (`--quality`), a page's `?quality=` or an Android dev
//! profile. The renderer applies them at a frame boundary
//! (`SceneRenderer::set_quality`); the player keeps the render scale.
//!
//! The post-process switches are caps, not settings: `ssao = false` turns
//! the scene's SSAO off, `ssao = true` leaves it as the scene authored it
//! (a scene with no SSAO still has none). So a quality tier can't add an
//! effect a scene's look didn't ask for, and a scene load keeps its own look
//! under the tier.
//!
//! Every value is clamped to what the renderer draws, so whatever a script
//! asks for, [`QualitySettings`] holds something drawable.

use std::fmt;

/// The smallest render scale, as `flint_render::MIN_RENDER_SCALE`.
pub const MIN_RENDER_SCALE: f32 = 0.25;
/// The renderer's shadow cascades at most.
pub const MAX_SHADOW_CASCADES: u32 = 3;
/// Shadow-map sizes per cascade, texels on a side.
pub const MIN_SHADOW_RESOLUTION: u32 = 256;
pub const MAX_SHADOW_RESOLUTION: u32 = 4096;
/// How far shadows reach from the camera, metres.
pub const MIN_SHADOW_DISTANCE: f32 = 10.0;
pub const MAX_SHADOW_DISTANCE: f32 = 500.0;
/// The reach before quality settings existed: the cascades covered 0.1 to
/// 200 m of view depth.
pub const DEFAULT_SHADOW_DISTANCE: f32 = 200.0;

/// The renderer's load levers; see the module docs.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct QualitySettings {
    /// The scene is drawn at this fraction of the surface on each side,
    /// `0.25..=1`. A ceiling: on the web the pixel-ratio cap can lower it
    /// further, and with `render_scale_auto` the player steps under it.
    pub render_scale: f32,
    /// Step under `render_scale` to hold the frame rate (`--render-scale auto`).
    pub render_scale_auto: bool,
    /// MSAA samples for the scene passes: 1 (off) or 4.
    pub msaa: u32,
    /// The shadow pass at all.
    pub shadows: bool,
    /// Each cascade's map, texels on a side: a power of two, 256 to 4096.
    pub shadow_resolution: u32,
    /// Cascades drawn, 1 to 3. Each is a draw of every shadow caster, so
    /// this is the shadow pass's cost (FlintKart#54), more than its size.
    pub shadow_cascades: u32,
    /// How far shadows reach from the camera, metres, 10 to 500.
    pub shadow_distance: f32,
    /// The whole post-process chain (bloom, SSAO, fog, vignette, grade...).
    /// Off draws the scene straight to the screen.
    pub post_process: bool,
    /// Caps: off turns the scene's effect off, on leaves it as authored.
    pub bloom: bool,
    pub ssao: bool,
    /// SSAO's hemisphere samples at most, 1 to 64 (a scene asking for fewer
    /// keeps its own).
    pub ssao_samples: u32,
    pub fog: bool,
    pub volumetric: bool,
}

impl Default for QualitySettings {
    /// Everything as the scene asks: the renderer as it drew before
    /// quality settings existed.
    fn default() -> Self {
        Self {
            render_scale: 1.0,
            render_scale_auto: false,
            msaa: 1,
            shadows: true,
            shadow_resolution: 2048,
            shadow_cascades: MAX_SHADOW_CASCADES,
            shadow_distance: DEFAULT_SHADOW_DISTANCE,
            post_process: true,
            bloom: true,
            ssao: true,
            ssao_samples: 64,
            fog: true,
            volumetric: true,
        }
    }
}

/// A value for one key, from a script, a TOML table or a `key=value` string.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum QualityValue {
    Bool(bool),
    Int(i64),
    Float(f64),
}

impl QualityValue {
    fn as_f64(self) -> Option<f64> {
        match self {
            Self::Int(i) => Some(i as f64),
            Self::Float(f) if f.is_finite() => Some(f),
            _ => None,
        }
    }

    fn as_bool(self) -> Option<bool> {
        match self {
            Self::Bool(b) => Some(b),
            // 0 / 1 too, so `--quality ssao=0` reads as it looks.
            Self::Int(0) => Some(false),
            Self::Int(1) => Some(true),
            _ => None,
        }
    }

    /// Read `true`, `off`, `4`, `0.75`... from text.
    pub fn parse(text: &str) -> Option<Self> {
        let t = text.trim();
        match t.to_ascii_lowercase().as_str() {
            "true" | "on" | "yes" => return Some(Self::Bool(true)),
            "false" | "off" | "no" => return Some(Self::Bool(false)),
            _ => {}
        }
        if let Ok(i) = t.parse::<i64>() {
            return Some(Self::Int(i));
        }
        t.parse::<f64>()
            .ok()
            .filter(|f| f.is_finite())
            .map(Self::Float)
    }
}

impl fmt::Display for QualityValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Bool(b) => write!(f, "{b}"),
            Self::Int(i) => write!(f, "{i}"),
            Self::Float(x) => write!(f, "{x}"),
        }
    }
}

/// Why a key wasn't taken.
#[derive(Debug, Clone, PartialEq)]
pub enum QualityError {
    UnknownKey(String),
    WrongType { key: String, value: QualityValue },
    Malformed(String),
}

impl fmt::Display for QualityError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownKey(k) => write!(
                f,
                "unknown quality setting `{k}` (known: {})",
                QualitySettings::KEYS.join(", ")
            ),
            Self::WrongType { key, value } => {
                write!(f, "quality setting `{key}` can't be {value}")
            }
            Self::Malformed(s) => write!(f, "`{s}` isn't key=value"),
        }
    }
}

impl std::error::Error for QualityError {}

/// An f32 as the f64 its decimal text reads as: 0.6, not 0.6000000238.
fn f32_to_f64(x: f32) -> f64 {
    x.to_string().parse().unwrap_or(x as f64)
}

/// `n` to the nearest power of two in `lo..=hi` (by ratio, so 1400 is 1024
/// and 1500 is 2048: the midpoint is 1448).
fn nearest_power_of_two(n: f64, lo: u32, hi: u32) -> u32 {
    let n = n.max(1.0).log2().round().clamp(0.0, 31.0) as u32;
    (1u32 << n).clamp(lo, hi)
}

impl QualitySettings {
    /// Every key, in the order [`Self::entries`] lists them.
    pub const KEYS: [&'static str; 13] = [
        "render_scale",
        "render_scale_auto",
        "msaa",
        "shadows",
        "shadow_resolution",
        "shadow_cascades",
        "shadow_distance",
        "post_process",
        "bloom",
        "ssao",
        "ssao_samples",
        "fog",
        "volumetric",
    ];

    /// Clamp every value to what the renderer draws.
    pub fn clamped(mut self) -> Self {
        self.render_scale = if self.render_scale.is_finite() {
            self.render_scale.clamp(MIN_RENDER_SCALE, 1.0)
        } else {
            1.0
        };
        self.msaa = if self.msaa >= 4 { 4 } else { 1 };
        self.shadow_resolution = nearest_power_of_two(
            self.shadow_resolution as f64,
            MIN_SHADOW_RESOLUTION,
            MAX_SHADOW_RESOLUTION,
        );
        self.shadow_cascades = self.shadow_cascades.clamp(1, MAX_SHADOW_CASCADES);
        self.shadow_distance = if self.shadow_distance.is_finite() {
            self.shadow_distance
                .clamp(MIN_SHADOW_DISTANCE, MAX_SHADOW_DISTANCE)
        } else {
            DEFAULT_SHADOW_DISTANCE
        };
        self.ssao_samples = self.ssao_samples.clamp(1, 64);
        self
    }

    /// Set one key, clamped. Numbers round where the setting is whole
    /// (`msaa = 3.7` is 4); `msaa` takes 1 or 4, anything from 4 up is 4.
    pub fn set(&mut self, key: &str, value: QualityValue) -> Result<(), QualityError> {
        let wrong = || QualityError::WrongType {
            key: key.to_string(),
            value,
        };
        let num = || value.as_f64().ok_or_else(wrong);
        let flag = || value.as_bool().ok_or_else(wrong);
        let whole = |n: f64| n.round().clamp(0.0, u32::MAX as f64) as u32;
        match key {
            "render_scale" => self.render_scale = num()? as f32,
            "render_scale_auto" => self.render_scale_auto = flag()?,
            "msaa" => self.msaa = whole(num()?),
            "shadows" => self.shadows = flag()?,
            "shadow_resolution" => self.shadow_resolution = whole(num()?),
            "shadow_cascades" => self.shadow_cascades = whole(num()?),
            "shadow_distance" => self.shadow_distance = num()? as f32,
            "post_process" => self.post_process = flag()?,
            "bloom" => self.bloom = flag()?,
            "ssao" => self.ssao = flag()?,
            "ssao_samples" => self.ssao_samples = whole(num()?),
            "fog" => self.fog = flag()?,
            "volumetric" => self.volumetric = flag()?,
            _ => return Err(QualityError::UnknownKey(key.to_string())),
        }
        *self = self.clamped();
        Ok(())
    }

    /// One key's value.
    pub fn get(&self, key: &str) -> Option<QualityValue> {
        self.entries()
            .into_iter()
            .find(|(k, _)| *k == key)
            .map(|(_, v)| v)
    }

    /// Every key and its value, in [`Self::KEYS`] order.
    pub fn entries(&self) -> [(&'static str, QualityValue); 13] {
        use QualityValue::{Bool, Float, Int};
        [
            ("render_scale", Float(f32_to_f64(self.render_scale))),
            ("render_scale_auto", Bool(self.render_scale_auto)),
            ("msaa", Int(self.msaa as i64)),
            ("shadows", Bool(self.shadows)),
            ("shadow_resolution", Int(self.shadow_resolution as i64)),
            ("shadow_cascades", Int(self.shadow_cascades as i64)),
            ("shadow_distance", Float(f32_to_f64(self.shadow_distance))),
            ("post_process", Bool(self.post_process)),
            ("bloom", Bool(self.bloom)),
            ("ssao", Bool(self.ssao)),
            ("ssao_samples", Int(self.ssao_samples as i64)),
            ("fog", Bool(self.fog)),
            ("volumetric", Bool(self.volumetric)),
        ]
    }

    /// Apply `key=value` pairs separated by commas or spaces, as
    /// `--quality msaa=4,shadow_cascades=2,ssao=off` gives them. Every
    /// good pair is applied; the errors are the pairs that weren't.
    pub fn apply_str(&mut self, text: &str) -> Vec<QualityError> {
        let mut errors = Vec::new();
        for pair in text
            .split(|c: char| c == ',' || c == ';' || c.is_whitespace())
            .filter(|p| !p.is_empty())
        {
            let Some((key, value)) = pair.split_once('=') else {
                errors.push(QualityError::Malformed(pair.to_string()));
                continue;
            };
            let key = key.trim();
            match QualityValue::parse(value) {
                Some(v) => {
                    if let Err(e) = self.set(key, v) {
                        errors.push(e);
                    }
                }
                None => errors.push(QualityError::Malformed(pair.to_string())),
            }
        }
        errors
    }

    /// Apply a TOML table's keys (an Android dev profile's `[quality]`).
    pub fn apply_toml(&mut self, table: &toml::Table) -> Vec<QualityError> {
        let mut errors = Vec::new();
        for (key, value) in table {
            let v = match value {
                toml::Value::Boolean(b) => QualityValue::Bool(*b),
                toml::Value::Integer(i) => QualityValue::Int(*i),
                toml::Value::Float(f) => QualityValue::Float(*f),
                toml::Value::String(s) => match QualityValue::parse(s) {
                    Some(v) => v,
                    None => {
                        errors.push(QualityError::Malformed(format!("{key}={s}")));
                        continue;
                    }
                },
                _ => {
                    errors.push(QualityError::Malformed(format!("{key}={value}")));
                    continue;
                }
            };
            if let Err(e) = self.set(key, v) {
                errors.push(e);
            }
        }
        errors
    }

    /// A TOML table as `key=value,...` with only the keys it sets, so a
    /// config's `[quality]` can travel the `--quality` path (Android's dev
    /// profile) without pinning the keys it leaves out.
    pub fn query_from_toml(table: &toml::Table) -> String {
        table
            .iter()
            .map(|(k, v)| match v {
                toml::Value::String(s) => format!("{k}={s}"),
                other => format!("{k}={other}"),
            })
            .collect::<Vec<_>>()
            .join(",")
    }

    /// The settings as `key=value,...`, which [`Self::apply_str`] reads back.
    pub fn to_query(&self) -> String {
        self.entries()
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join(",")
    }
}

/// What the GPU is, for a game choosing a starting tier (FlintKart#58).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct GpuInfo {
    /// The adapter's name ("NVIDIA GeForce RTX 4070", "Mali-G715"). Often
    /// empty in a browser, which hides it.
    pub name: String,
    /// The vendor, from its PCI id: "nvidia", "amd", "intel", "arm",
    /// "qualcomm", "apple", "imagination", "microsoft" or "unknown".
    pub vendor: String,
    /// wgpu's device type: "discrete", "integrated", "virtual", "cpu" or "other".
    pub device_type: String,
    /// The graphics API: "vulkan", "dx12", "metal", "gl", "webgpu".
    pub backend: String,
    /// The driver's own description, where it gives one.
    pub driver: String,
    /// The class a game picks a tier from: "desktop" (a discrete GPU),
    /// "integrated" (a laptop's or desktop's on-chip GPU), "mobile" (a
    /// phone's or tablet's), "software" (a CPU rasteriser) or "unknown".
    pub class: String,
    /// Where the game runs: "windows", "macos", "linux", "android", "ios" or "web".
    pub platform: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_is_the_renderer_before_quality_settings() {
        let q = QualitySettings::default();
        assert_eq!(q, q.clamped());
        assert_eq!(q.shadow_cascades, 3);
        assert_eq!(q.shadow_distance, 200.0);
        assert_eq!(q.msaa, 1);
    }

    #[test]
    fn values_clamp_to_what_draws() {
        let mut q = QualitySettings::default();
        q.set("render_scale", QualityValue::Float(0.1)).unwrap();
        assert_eq!(q.render_scale, MIN_RENDER_SCALE);
        q.set("render_scale", QualityValue::Int(3)).unwrap();
        assert_eq!(q.render_scale, 1.0);
        q.set("msaa", QualityValue::Int(2)).unwrap();
        assert_eq!(q.msaa, 1);
        q.set("msaa", QualityValue::Int(8)).unwrap();
        assert_eq!(q.msaa, 4);
        q.set("shadow_resolution", QualityValue::Int(1400)).unwrap();
        assert_eq!(q.shadow_resolution, 1024);
        q.set("shadow_resolution", QualityValue::Int(100_000))
            .unwrap();
        assert_eq!(q.shadow_resolution, 4096);
        q.set("shadow_cascades", QualityValue::Int(0)).unwrap();
        assert_eq!(q.shadow_cascades, 1);
        q.set("shadow_cascades", QualityValue::Float(9.0)).unwrap();
        assert_eq!(q.shadow_cascades, 3);
        q.set("shadow_distance", QualityValue::Float(-5.0)).unwrap();
        assert_eq!(q.shadow_distance, MIN_SHADOW_DISTANCE);
        q.set("ssao_samples", QualityValue::Int(1000)).unwrap();
        assert_eq!(q.ssao_samples, 64);
    }

    #[test]
    fn a_bad_key_or_type_changes_nothing() {
        let mut q = QualitySettings::default();
        let before = q;
        assert!(matches!(
            q.set("sharpness", QualityValue::Int(1)),
            Err(QualityError::UnknownKey(_))
        ));
        assert!(matches!(
            q.set("msaa", QualityValue::Bool(true)),
            Err(QualityError::WrongType { .. })
        ));
        assert!(q.set("ssao", QualityValue::Float(0.5)).is_err());
        assert_eq!(q, before);
    }

    #[test]
    fn a_query_string_round_trips() {
        let mut q = QualitySettings::default();
        let errors = q.apply_str("msaa=4, shadow_cascades=2 ssao=off;render_scale=0.75");
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(q.msaa, 4);
        assert_eq!(q.shadow_cascades, 2);
        assert!(!q.ssao);
        assert_eq!(q.render_scale, 0.75);

        let mut back = QualitySettings::default();
        assert!(back.apply_str(&q.to_query()).is_empty());
        assert_eq!(back, q);
    }

    #[test]
    fn a_query_string_keeps_its_good_pairs() {
        let mut q = QualitySettings::default();
        let errors = q.apply_str("bloom=0,nonsense,fov=90,msaa=lots");
        assert_eq!(errors.len(), 3, "{errors:?}");
        assert!(!q.bloom);
    }

    #[test]
    fn a_toml_table_applies() {
        let table: toml::Table = toml::from_str(
            "msaa = 4\nshadows = false\nrender_scale = \"0.6\"\nssao_samples = 16.0",
        )
        .unwrap();
        let mut q = QualitySettings::default();
        assert!(q.apply_toml(&table).is_empty());
        assert_eq!(q.msaa, 4);
        assert!(!q.shadows);
        assert!((q.render_scale - 0.6).abs() < 1e-6);
        assert_eq!(q.ssao_samples, 16);
    }

    #[test]
    fn a_table_travels_as_a_query_with_only_its_keys() {
        let table: toml::Table =
            toml::from_str("msaa = 4\nssao = false\nrender_scale = 0.75\nfog = \"off\"").unwrap();
        let query = QualitySettings::query_from_toml(&table);
        let mut q = QualitySettings {
            shadow_cascades: 1,
            ..QualitySettings::default()
        };
        assert!(q.apply_str(&query).is_empty(), "{query}");
        assert_eq!(q.msaa, 4);
        assert!(!q.ssao && !q.fog);
        assert_eq!(q.render_scale, 0.75);
        assert_eq!(q.shadow_cascades, 1, "a key the table leaves out is kept");
    }

    #[test]
    fn every_key_reads_back() {
        let q = QualitySettings::default();
        for key in QualitySettings::KEYS {
            assert!(q.get(key).is_some(), "{key}");
        }
        assert_eq!(q.get("nope"), None);
    }
}
