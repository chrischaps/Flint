//! Shared utility functions for procedural generators.
//!
//! Common parsing and color-space helpers used across tree, texture, and other
//! generator modules.

use crate::{ProcGenError, Result};

/// Parse a hex color string (e.g. `"#8B4513"` or `"8B4513"`) into linear RGBA.
///
/// Supports 6-digit (`RRGGBB`) and 8-digit (`RRGGBBAA`) hex, with or without
/// a leading `#`. The sRGB->linear conversion uses the standard threshold
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
pub fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// Parse a 2-character hex string into a u8.
pub fn u8_from_hex(s: &str, channel: &str) -> Result<u8> {
    u8::from_str_radix(s, 16).map_err(|_| ProcGenError::InvalidParameter {
        name: "hex_color".into(),
        reason: format!("invalid hex for {channel} channel: \"{s}\""),
    })
}

/// Extract an f32 from a TOML value (handles both integer and float).
pub fn toml_f32(v: &toml::Value, name: &str) -> Result<f32> {
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
pub fn toml_u32(v: &toml::Value, name: &str) -> Result<u32> {
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

/// Extract a string from a TOML value.
pub fn toml_string(v: &toml::Value, name: &str) -> Result<String> {
    v.as_str()
        .map(|s| s.to_string())
        .ok_or_else(|| ProcGenError::InvalidParameter {
            name: name.into(),
            reason: "expected a string".into(),
        })
}

/// Extract a `Vec<String>` from a TOML array of strings.
pub fn toml_string_array(v: &toml::Value, name: &str) -> Result<Vec<String>> {
    let arr = v.as_array().ok_or_else(|| ProcGenError::InvalidParameter {
        name: name.into(),
        reason: "expected an array".into(),
    })?;
    arr.iter()
        .map(|item| {
            item.as_str()
                .map(|s| s.to_string())
                .ok_or_else(|| ProcGenError::InvalidParameter {
                    name: name.into(),
                    reason: "array elements must be strings".into(),
                })
        })
        .collect()
}
