//! Tree generator — produces procedural tree meshes with bark materials.
//!
//! Task 3.1 provides trunk generation. Tasks 3.2 (branches) and 3.3
//! (leaves/LOD) will extend this module into a full `TreeGenerator`.

mod bark;
mod trunk;

pub use bark::generate_bark_normal_map;
pub use trunk::{generate_trunk, TrunkOutput};

use crate::{ProcGenError, Result};

/// Typed trunk configuration extracted from a `ProcGenSpec`'s params table.
#[derive(Debug, Clone)]
pub struct TrunkParams {
    /// Total trunk height in meters.
    pub height: f32,
    /// Radius at the base of the trunk.
    pub radius_base: f32,
    /// Radius at the top of the trunk.
    pub radius_top: f32,
    /// Number of segments along the spine (vertical resolution).
    pub segments: u32,
    /// Number of radial segments in the cross-section.
    pub radial_segments: u32,
    /// Perlin displacement magnitude for trunk curvature.
    pub curve_noise: f32,
    /// Bark base color as linear RGBA.
    pub bark_color: [f32; 4],
    /// Bark roughness factor.
    pub bark_roughness: f32,
    /// Normal map strength multiplier.
    pub bark_normal_strength: f32,
    /// Normal map resolution (width = height).
    pub bark_normal_resolution: u32,
}

impl Default for TrunkParams {
    fn default() -> Self {
        Self {
            height: 5.0,
            radius_base: 0.3,
            radius_top: 0.1,
            segments: 10,
            radial_segments: 8,
            curve_noise: 0.1,
            bark_color: [0.34, 0.2, 0.1, 1.0],
            bark_roughness: 0.9,
            bark_normal_strength: 1.0,
            bark_normal_resolution: 256,
        }
    }
}

impl TrunkParams {
    /// Parse trunk parameters from a TOML `params` value.
    ///
    /// Missing fields fall back to sensible defaults. Invalid values produce
    /// descriptive errors.
    pub fn from_toml(params: &toml::Value) -> Result<Self> {
        let table = params
            .as_table()
            .ok_or_else(|| ProcGenError::InvalidParameter {
                name: "params".into(),
                reason: "expected a TOML table".into(),
            })?;

        let mut p = Self::default();

        if let Some(v) = table.get("trunk_height") {
            p.height = toml_f32(v, "trunk_height")?;
            if p.height <= 0.0 {
                return Err(ProcGenError::InvalidParameter {
                    name: "trunk_height".into(),
                    reason: "must be positive".into(),
                });
            }
        }

        if let Some(v) = table.get("trunk_radius_base") {
            p.radius_base = toml_f32(v, "trunk_radius_base")?;
            if p.radius_base <= 0.0 {
                return Err(ProcGenError::InvalidParameter {
                    name: "trunk_radius_base".into(),
                    reason: "must be positive".into(),
                });
            }
        }

        if let Some(v) = table.get("trunk_radius_top") {
            p.radius_top = toml_f32(v, "trunk_radius_top")?;
            if p.radius_top < 0.0 {
                return Err(ProcGenError::InvalidParameter {
                    name: "trunk_radius_top".into(),
                    reason: "must be non-negative".into(),
                });
            }
        }

        if let Some(v) = table.get("trunk_segments") {
            p.segments = toml_u32(v, "trunk_segments")?;
            if p.segments < 2 {
                return Err(ProcGenError::InvalidParameter {
                    name: "trunk_segments".into(),
                    reason: "must be at least 2".into(),
                });
            }
        }

        if let Some(v) = table.get("radial_segments") {
            p.radial_segments = toml_u32(v, "radial_segments")?;
            if p.radial_segments < 3 {
                return Err(ProcGenError::InvalidParameter {
                    name: "radial_segments".into(),
                    reason: "must be at least 3".into(),
                });
            }
        }

        if let Some(v) = table.get("trunk_curve_noise") {
            p.curve_noise = toml_f32(v, "trunk_curve_noise")?;
            if p.curve_noise < 0.0 {
                return Err(ProcGenError::InvalidParameter {
                    name: "trunk_curve_noise".into(),
                    reason: "must be non-negative".into(),
                });
            }
        }

        if let Some(v) = table.get("bark_color_base") {
            let hex = v.as_str().ok_or_else(|| ProcGenError::InvalidParameter {
                name: "bark_color_base".into(),
                reason: "expected a hex color string (e.g. \"#8B4513\")".into(),
            })?;
            p.bark_color = parse_hex_color(hex)?;
        }

        if let Some(v) = table.get("bark_roughness") {
            p.bark_roughness = toml_f32(v, "bark_roughness")?;
        }

        if let Some(v) = table.get("bark_normal_strength") {
            p.bark_normal_strength = toml_f32(v, "bark_normal_strength")?;
        }

        if let Some(v) = table.get("bark_normal_resolution") {
            p.bark_normal_resolution = toml_u32(v, "bark_normal_resolution")?;
            if !p.bark_normal_resolution.is_power_of_two() || p.bark_normal_resolution < 16 {
                return Err(ProcGenError::InvalidParameter {
                    name: "bark_normal_resolution".into(),
                    reason: "must be a power of 2 and at least 16".into(),
                });
            }
        }

        Ok(p)
    }
}

/// Parse a hex color string (e.g. `"#8B4513"` or `"8B4513"`) into linear RGBA.
///
/// Supports 6-digit (`RRGGBB`) and 8-digit (`RRGGBBAA`) hex, with or without
/// a leading `#`. The sRGB→linear conversion uses the standard threshold
/// formula (IEC 61966-2-1).
pub fn parse_hex_color(hex: &str) -> Result<[f32; 4]> {
    let hex = hex.strip_prefix('#').unwrap_or(hex);

    let (r, g, b, a) = match hex.len() {
        6 => {
            let r = u8_from_hex(&hex[0..2], "red")?;
            let g = u8_from_hex(&hex[2..4], "green")?;
            let b = u8_from_hex(&hex[4..6], "blue")?;
            (r, g, b, 255u8)
        }
        8 => {
            let r = u8_from_hex(&hex[0..2], "red")?;
            let g = u8_from_hex(&hex[2..4], "green")?;
            let b = u8_from_hex(&hex[4..6], "blue")?;
            let a = u8_from_hex(&hex[6..8], "alpha")?;
            (r, g, b, a)
        }
        _ => {
            return Err(ProcGenError::InvalidParameter {
                name: "hex_color".into(),
                reason: format!(
                    "expected 6 or 8 hex digits (got {} chars: \"{}\")",
                    hex.len(),
                    hex
                ),
            });
        }
    };

    Ok([
        srgb_to_linear(r as f32 / 255.0),
        srgb_to_linear(g as f32 / 255.0),
        srgb_to_linear(b as f32 / 255.0),
        a as f32 / 255.0,
    ])
}

/// Convert a single sRGB channel value to linear.
fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// Parse a 2-character hex string into a u8.
fn u8_from_hex(s: &str, channel: &str) -> Result<u8> {
    u8::from_str_radix(s, 16).map_err(|_| ProcGenError::InvalidParameter {
        name: "hex_color".into(),
        reason: format!("invalid hex for {channel} channel: \"{s}\""),
    })
}

/// Extract an f32 from a TOML value (handles both integer and float).
fn toml_f32(v: &toml::Value, name: &str) -> Result<f32> {
    match v {
        toml::Value::Float(f) => Ok(*f as f32),
        toml::Value::Integer(i) => Ok(*i as f32),
        _ => Err(ProcGenError::InvalidParameter {
            name: name.into(),
            reason: "expected a number".into(),
        }),
    }
}

/// Extract a u32 from a TOML integer value.
fn toml_u32(v: &toml::Value, name: &str) -> Result<u32> {
    match v {
        toml::Value::Integer(i) => {
            if *i < 0 {
                Err(ProcGenError::InvalidParameter {
                    name: name.into(),
                    reason: "must be non-negative".into(),
                })
            } else {
                Ok(*i as u32)
            }
        }
        _ => Err(ProcGenError::InvalidParameter {
            name: name.into(),
            reason: "expected an integer".into(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_hex_6_digit() {
        let c = parse_hex_color("#8B4513").unwrap();
        // sRGB (139, 69, 19) → linear; verify channels are in expected range
        // r: 139/255 = 0.545 sRGB → ~0.26 linear
        // g: 69/255 = 0.271 sRGB → ~0.06 linear
        // b: 19/255 = 0.075 sRGB → ~0.005 linear
        assert!(c[0] > 0.2 && c[0] < 0.35, "red channel: {}", c[0]);
        assert!(c[1] > 0.04 && c[1] < 0.08, "green channel: {}", c[1]);
        assert!(c[2] > 0.003 && c[2] < 0.01, "blue channel: {}", c[2]);
        assert!((c[3] - 1.0).abs() < 1e-5);
    }

    #[test]
    fn parse_hex_without_hash() {
        let c = parse_hex_color("8B4513").unwrap();
        assert!((c[3] - 1.0).abs() < 1e-5);
    }

    #[test]
    fn parse_hex_8_digit() {
        let c = parse_hex_color("#8B451380").unwrap();
        assert!((c[3] - 128.0 / 255.0).abs() < 0.01);
    }

    #[test]
    fn parse_hex_invalid() {
        assert!(parse_hex_color("#ZZZ").is_err());
        assert!(parse_hex_color("#12345").is_err());
        assert!(parse_hex_color("").is_err());
    }

    #[test]
    fn srgb_to_linear_black_and_white() {
        assert!((srgb_to_linear(0.0)).abs() < 1e-6);
        assert!((srgb_to_linear(1.0) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn trunk_params_defaults() {
        let toml_val: toml::Value = toml::toml! {
            trunk_height = 8.0
        }
        .into();
        let p = TrunkParams::from_toml(&toml_val).unwrap();
        assert!((p.height - 8.0).abs() < 1e-5);
        assert_eq!(p.radial_segments, 8); // default
        assert_eq!(p.segments, 10); // default
    }

    #[test]
    fn trunk_params_full() {
        let toml_val: toml::Value = toml::toml! {
            trunk_height = 10.0
            trunk_radius_base = 0.5
            trunk_radius_top = 0.15
            trunk_segments = 20
            radial_segments = 12
            trunk_curve_noise = 0.3
            bark_color_base = "#8B4513"
            bark_roughness = 0.85
            bark_normal_strength = 1.5
            bark_normal_resolution = 128
        }
        .into();
        let p = TrunkParams::from_toml(&toml_val).unwrap();
        assert!((p.height - 10.0).abs() < 1e-5);
        assert!((p.radius_base - 0.5).abs() < 1e-5);
        assert!((p.radius_top - 0.15).abs() < 1e-5);
        assert_eq!(p.segments, 20);
        assert_eq!(p.radial_segments, 12);
        assert!((p.curve_noise - 0.3).abs() < 1e-5);
        assert!((p.bark_roughness - 0.85).abs() < 1e-5);
        assert!((p.bark_normal_strength - 1.5).abs() < 1e-5);
        assert_eq!(p.bark_normal_resolution, 128);
    }

    #[test]
    fn trunk_params_invalid_height() {
        let toml_val: toml::Value = toml::toml! {
            trunk_height = -1.0
        }
        .into();
        assert!(TrunkParams::from_toml(&toml_val).is_err());
    }

    #[test]
    fn trunk_params_invalid_resolution() {
        let toml_val: toml::Value = toml::toml! {
            bark_normal_resolution = 100
        }
        .into();
        assert!(TrunkParams::from_toml(&toml_val).is_err());
    }

    #[test]
    fn trunk_params_not_a_table() {
        let toml_val = toml::Value::String("not a table".into());
        assert!(TrunkParams::from_toml(&toml_val).is_err());
    }
}
