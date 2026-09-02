//! Composable texture field with named float channels.
//!
//! [`TextureField`] is the intermediate representation for the composable
//! texture pipeline. Each channel stores one `f32` per pixel, keyed by name
//! (e.g. `"cell_id"`, `"height"`, `"r"`, `"roughness"`). Texture ops read
//! and write channels to build up the final PBR output images.

use std::collections::HashMap;

use crate::types::{ChannelSemantics, ImageData};

// ─── TextureField ─────────────────────────────────────────────────────────

/// A 2D grid of named float channels for composable texture generation.
///
/// Standard channel names:
/// - Grid/structure: `cell_id`, `edge_dist`
/// - Height: `height`
/// - Color: `r`, `g`, `b`, `a`
/// - Normal: `normal_x`, `normal_y`, `normal_z`
/// - Roughness: `roughness`
#[derive(Clone)]
pub struct TextureField {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Named per-pixel float buffers, each of length `width * height`.
    channels: HashMap<String, Vec<f32>>,
}

impl TextureField {
    /// Create a new empty field (no channels allocated yet).
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            channels: HashMap::new(),
        }
    }

    /// Read a channel value at `(x, y)`. Returns `0.0` if the channel doesn't exist.
    #[inline]
    pub fn get(&self, channel: &str, x: u32, y: u32) -> f32 {
        let idx = (y * self.width + x) as usize;
        self.channels
            .get(channel)
            .map(|buf| buf[idx])
            .unwrap_or(0.0)
    }

    /// Write a channel value at `(x, y)`. Creates the channel if it doesn't exist.
    #[inline]
    pub fn set(&mut self, channel: &str, x: u32, y: u32, value: f32) {
        let w = self.width;
        let h = self.height;
        let idx = (y * w + x) as usize;
        let buf = self
            .channels
            .entry(channel.to_string())
            .or_insert_with(|| vec![0.0; (w * h) as usize]);
        buf[idx] = value;
    }

    /// Ensure a channel exists, creating it (filled with `0.0`) if missing.
    pub fn ensure_channel(&mut self, name: &str) {
        let len = (self.width * self.height) as usize;
        self.channels
            .entry(name.to_string())
            .or_insert_with(|| vec![0.0; len]);
    }

    /// Check whether a channel exists.
    pub fn has_channel(&self, name: &str) -> bool {
        self.channels.contains_key(name)
    }

    /// Get the raw float slice for a channel, if it exists.
    pub fn channel(&self, name: &str) -> Option<&[f32]> {
        self.channels.get(name).map(|v| v.as_slice())
    }

    /// Get a mutable float slice for a channel, if it exists.
    pub fn channel_mut(&mut self, name: &str) -> Option<&mut [f32]> {
        self.channels.get_mut(name).map(|v| v.as_mut_slice())
    }

    /// Copy all pixel data from channel `src` to channel `dst`.
    ///
    /// If `src` doesn't exist, `dst` is filled with zeros.
    /// If `dst` already exists, it is overwritten.
    pub fn copy_channel(&mut self, src: &str, dst: &str) {
        let len = (self.width * self.height) as usize;
        let data = self
            .channels
            .get(src)
            .cloned()
            .unwrap_or_else(|| vec![0.0; len]);
        self.channels.insert(dst.to_string(), data);
    }

    /// Remove a channel by name. Returns true if it existed.
    pub fn remove_channel(&mut self, name: &str) -> bool {
        self.channels.remove(name).is_some()
    }

    /// Return a sorted list of all allocated channel names.
    pub fn channel_names(&self) -> Vec<String> {
        let mut names: Vec<String> = self.channels.keys().cloned().collect();
        names.sort();
        names
    }

    /// Render a single float channel as a grayscale RGBA8 image.
    ///
    /// Values are clamped to [0, 1] and mapped linearly to [0, 255].
    /// Returns an image with `ChannelSemantics::Roughness` (linear grayscale).
    pub fn to_channel_grayscale(&self, channel: &str) -> ImageData {
        let w = self.width;
        let h = self.height;
        let mut img = ImageData::new(w, h, ChannelSemantics::Roughness);

        for y in 0..h {
            for x in 0..w {
                let val = self.get(channel, x, y);
                let byte = (val.clamp(0.0, 1.0) * 255.0) as u8;
                let offset = ((y * w + x) * 4) as usize;
                img.pixels[offset] = byte;
                img.pixels[offset + 1] = byte;
                img.pixels[offset + 2] = byte;
                img.pixels[offset + 3] = 255;
            }
        }

        img
    }

    // ── Output image extraction ──────────────────────────────────────────

    /// Convert `r`, `g`, `b` (and optional `a`) channels to an sRGB RGBA8 albedo image.
    pub fn to_albedo_image(&self) -> ImageData {
        let w = self.width;
        let h = self.height;
        let mut img = ImageData::new(w, h, ChannelSemantics::Color);
        let has_alpha = self.has_channel("a");

        for y in 0..h {
            for x in 0..w {
                let r = self.get("r", x, y);
                let g = self.get("g", x, y);
                let b = self.get("b", x, y);
                let a = if has_alpha { self.get("a", x, y) } else { 1.0 };

                let offset = ((y * w + x) * 4) as usize;
                img.pixels[offset] = linear_to_srgb(r);
                img.pixels[offset + 1] = linear_to_srgb(g);
                img.pixels[offset + 2] = linear_to_srgb(b);
                img.pixels[offset + 3] = (a.clamp(0.0, 1.0) * 255.0) as u8;
            }
        }

        img
    }

    /// Convert `normal_x`, `normal_y`, `normal_z` channels to a linear RGBA8 normal map.
    pub fn to_normal_image(&self) -> ImageData {
        let w = self.width;
        let h = self.height;
        let mut img = ImageData::new(w, h, ChannelSemantics::Normal);

        for y in 0..h {
            for x in 0..w {
                let nx = self.get("normal_x", x, y);
                let ny = self.get("normal_y", x, y);
                let nz = self.get("normal_z", x, y);

                let offset = ((y * w + x) * 4) as usize;
                img.pixels[offset] = ((nx * 0.5 + 0.5).clamp(0.0, 1.0) * 255.0) as u8;
                img.pixels[offset + 1] = ((ny * 0.5 + 0.5).clamp(0.0, 1.0) * 255.0) as u8;
                img.pixels[offset + 2] = ((nz * 0.5 + 0.5).clamp(0.0, 1.0) * 255.0) as u8;
                img.pixels[offset + 3] = 255;
            }
        }

        img
    }

    /// Convert the `roughness` channel to a grayscale linear RGBA8 image.
    pub fn to_roughness_image(&self) -> ImageData {
        let w = self.width;
        let h = self.height;
        let mut img = ImageData::new(w, h, ChannelSemantics::Roughness);

        for y in 0..h {
            for x in 0..w {
                let roughness = self.get("roughness", x, y);
                let byte = (roughness.clamp(0.0, 1.0) * 255.0) as u8;

                let offset = ((y * w + x) * 4) as usize;
                img.pixels[offset] = byte;
                img.pixels[offset + 1] = byte;
                img.pixels[offset + 2] = byte;
                img.pixels[offset + 3] = 255;
            }
        }

        img
    }
}

// ─── Helpers ──────────────────────────────────────────────────────────────

/// Gamma-correct a linear float to an sRGB byte.
fn linear_to_srgb(v: f32) -> u8 {
    let s = if v <= 0.003_130_8 {
        v * 12.92
    } else {
        1.055 * v.powf(1.0 / 2.4) - 0.055
    };
    (s.clamp(0.0, 1.0) * 255.0) as u8
}

// ─── Tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_field_has_no_channels() {
        let field = TextureField::new(16, 16);
        assert!(!field.has_channel("height"));
        assert_eq!(field.get("height", 0, 0), 0.0);
    }

    #[test]
    fn set_creates_channel() {
        let mut field = TextureField::new(4, 4);
        field.set("height", 2, 3, 0.75);
        assert!(field.has_channel("height"));
        assert_eq!(field.get("height", 2, 3), 0.75);
        assert_eq!(field.get("height", 0, 0), 0.0);
    }

    #[test]
    fn ensure_channel_creates_zeroed() {
        let mut field = TextureField::new(8, 8);
        field.ensure_channel("cell_id");
        assert!(field.has_channel("cell_id"));
        assert_eq!(field.channel("cell_id").unwrap().len(), 64);
        assert!(field.channel("cell_id").unwrap().iter().all(|&v| v == 0.0));
    }

    #[test]
    fn to_albedo_image_dimensions() {
        let mut field = TextureField::new(4, 4);
        field.set("r", 0, 0, 0.5);
        field.set("g", 0, 0, 0.3);
        field.set("b", 0, 0, 0.1);
        let img = field.to_albedo_image();
        assert_eq!(img.width, 4);
        assert_eq!(img.height, 4);
        assert!(img.validate().is_ok());
        // Alpha should be 255 when no 'a' channel
        assert_eq!(img.pixels[3], 255);
    }

    #[test]
    fn to_normal_image_z_positive() {
        let mut field = TextureField::new(4, 4);
        // Default normal pointing up (0, 0, 1)
        for y in 0..4 {
            for x in 0..4 {
                field.set("normal_z", x, y, 1.0);
            }
        }
        let img = field.to_normal_image();
        assert!(img.validate().is_ok());
        // Z channel at offset+2 should be 255 (nz=1.0 → 0.5+0.5=1.0 → 255)
        assert_eq!(img.pixels[2], 255);
    }

    #[test]
    fn to_roughness_image_range() {
        let mut field = TextureField::new(4, 4);
        field.set("roughness", 0, 0, 0.5);
        field.set("roughness", 1, 0, 1.0);
        let img = field.to_roughness_image();
        assert!(img.validate().is_ok());
        assert_eq!(img.pixels[0], 127); // 0.5 * 255 ≈ 127
        assert_eq!(img.pixels[4], 255); // 1.0 * 255 = 255
    }

    #[test]
    fn copy_channel_basic() {
        let mut field = TextureField::new(4, 4);
        field.set("height", 1, 2, 0.75);
        field.set("height", 3, 0, 0.5);
        field.copy_channel("height", "height_copy");
        assert!(field.has_channel("height_copy"));
        assert_eq!(field.get("height_copy", 1, 2), 0.75);
        assert_eq!(field.get("height_copy", 3, 0), 0.5);
        assert_eq!(field.get("height_copy", 0, 0), 0.0);
    }

    #[test]
    fn copy_channel_missing_source() {
        let mut field = TextureField::new(4, 4);
        field.copy_channel("nonexistent", "dst");
        assert!(field.has_channel("dst"));
        assert_eq!(field.get("dst", 0, 0), 0.0);
        assert_eq!(field.channel("dst").unwrap().len(), 16);
    }

    #[test]
    fn copy_channel_overwrite() {
        let mut field = TextureField::new(4, 4);
        field.set("a", 0, 0, 1.0);
        field.set("b", 0, 0, 0.5);
        field.copy_channel("a", "b");
        assert_eq!(field.get("b", 0, 0), 1.0);
    }

    #[test]
    fn channel_mut_access() {
        let mut field = TextureField::new(4, 4);
        field.ensure_channel("height");
        let buf = field.channel_mut("height").unwrap();
        buf[0] = 0.9;
        assert_eq!(field.get("height", 0, 0), 0.9);
    }
}
