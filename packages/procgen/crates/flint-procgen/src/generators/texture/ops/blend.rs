//! `blend`: one channel mixed into another by a blend mode.

use crate::SeededRng;

use super::{lerp, TextureOp};
use crate::generators::texture::descriptor::{OpDescriptor, ParamDesc as P};
use crate::generators::texture::field::TextureField;

// ─── BlendOp ──────────────────────────────────────────────────────────────

/// Blend a source channel into a target channel.
///
/// Reads `<source>`, modifies `<target>`.
pub struct BlendOp {
    /// Channel to read from (e.g. `"noise"`).
    pub source: String,
    /// Channel to modify (e.g. `"height"`).
    pub target: String,
    /// Blend mode.
    pub mode: BlendMode,
    /// Blend strength / amplitude.
    pub strength: f32,
    /// Optional mask channel – blend is scaled by the mask value at each pixel.
    pub mask: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BlendMode {
    /// target += (source - 0.5) * strength
    Add,
    /// target += source * strength (no centering)
    AddRaw,
    /// target *= lerp(1.0, source, strength)
    Multiply,
    /// target = lerp(target, source, strength)
    Mix,
    /// 1 - (1-a)(1-b)
    Screen,
    /// a<0.5 ? 2ab : 1-2(1-a)(1-b)
    Overlay,
    /// Pegtop soft light
    SoftLight,
    /// 2a + b - 1
    LinearLight,
    /// abs(a - b)
    Difference,
    /// min(a, b)
    Darken,
    /// max(a, b)
    Lighten,
    /// a / (1 - b)
    ColorDodge,
    /// 1 - (1-a) / b
    ColorBurn,
    /// a - b
    Subtract,
}

impl BlendMode {
    pub fn from_str(s: &str) -> Self {
        match s {
            "multiply" => BlendMode::Multiply,
            "mix" => BlendMode::Mix,
            "screen" => BlendMode::Screen,
            "overlay" => BlendMode::Overlay,
            "soft_light" => BlendMode::SoftLight,
            "linear_light" => BlendMode::LinearLight,
            "difference" => BlendMode::Difference,
            "darken" => BlendMode::Darken,
            "lighten" => BlendMode::Lighten,
            "color_dodge" => BlendMode::ColorDodge,
            "color_burn" => BlendMode::ColorBurn,
            "subtract" => BlendMode::Subtract,
            "add_raw" => BlendMode::AddRaw,
            _ => BlendMode::Add,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            BlendMode::Add => "add",
            BlendMode::AddRaw => "add_raw",
            BlendMode::Multiply => "multiply",
            BlendMode::Mix => "mix",
            BlendMode::Screen => "screen",
            BlendMode::Overlay => "overlay",
            BlendMode::SoftLight => "soft_light",
            BlendMode::LinearLight => "linear_light",
            BlendMode::Difference => "difference",
            BlendMode::Darken => "darken",
            BlendMode::Lighten => "lighten",
            BlendMode::ColorDodge => "color_dodge",
            BlendMode::ColorBurn => "color_burn",
            BlendMode::Subtract => "subtract",
        }
    }
}

/// The [`BlendOp`] op, as the pipeline knows it.
pub const BLEND: OpDescriptor = OpDescriptor {
    type_name: "blend",
    label: "Blend",
    summary: "Blend one channel into another in place, optionally through a mask.",
    reads: &["<source>"],
    writes: &[],
    modifies: &["<target>"],
    params: &[
        P::channel("source", "noise").help("Channel blended in"),
        P::channel("target", "height").help("Channel changed in place"),
        P::choice("mode", &[
            "add", "add_raw", "multiply", "mix", "screen", "overlay", "soft_light",
            "linear_light", "difference", "darken", "lighten", "color_dodge", "color_burn",
            "subtract",
        ], "add").help("How source meets target: add centres source on 0.5 first and add_raw doesn't; multiply and mix move toward source by strength; the rest are the usual image blend modes"),
        P::float("strength", 0.1).min(0.0).help("How much of the source goes in"),
        P::opt_channel("mask").help("Channel scaling the blend per pixel (a grid's mask keeps it off the mortar); unset, it applies everywhere"),
    ],
    build: |p| {
        Ok(Box::new(BlendOp {
            source: p.text("source")?,
            target: p.text("target")?,
            mode: BlendMode::from_str(p.choice("mode")?),
            strength: p.f32("strength")?,
            mask: p.opt_text("mask")?,
        }))
    },
};

impl TextureOp for BlendOp {
    fn descriptor(&self) -> &'static OpDescriptor {
        &BLEND
    }

    fn apply(&self, field: &mut TextureField, _rng: &mut SeededRng) {
        // Work on whole channels: the per-pixel get/get/set cost three map
        // lookups a pixel, more than the blend itself (#187). Source and
        // mask are copied first, so a channel may blend into itself; a
        // missing one reads as 0, as `TextureField::get` does.
        let len = (field.width * field.height) as usize;
        let read = |name: &str| {
            field
                .channel(name)
                .map_or_else(|| vec![0.0; len], <[f32]>::to_vec)
        };
        let src = read(&self.source);
        let mask = self.mask.as_deref().map(read);
        field.ensure_channel(&self.target);
        let dst = field
            .channel_mut(&self.target)
            .expect("target channel was just ensured");

        for (i, d) in dst.iter_mut().enumerate() {
            let blended = self.blend_pixel(*d, src[i]);
            let blended = match &mask {
                Some(m) => lerp(*d, blended, m[i]),
                None => blended,
            };
            *d = blended.clamp(0.0, 1.0);
        }
    }
}

impl BlendOp {
    /// The blended value before masking and clamping.
    #[inline]
    fn blend_pixel(&self, dst: f32, src: f32) -> f32 {
        match self.mode {
            BlendMode::Add => dst + (src - 0.5) * self.strength,
            BlendMode::AddRaw => dst + src * self.strength,
            BlendMode::Multiply => dst * (1.0 + (src - 1.0) * self.strength),
            BlendMode::Mix => dst + (src - dst) * self.strength,
            BlendMode::Screen => {
                let b = 1.0 - (1.0 - dst) * (1.0 - src);
                lerp(dst, b, self.strength)
            }
            BlendMode::Overlay => {
                let b = if dst < 0.5 {
                    2.0 * dst * src
                } else {
                    1.0 - 2.0 * (1.0 - dst) * (1.0 - src)
                };
                lerp(dst, b, self.strength)
            }
            BlendMode::SoftLight => {
                // Pegtop formula
                let b = (1.0 - 2.0 * src) * dst * dst + 2.0 * src * dst;
                lerp(dst, b, self.strength)
            }
            BlendMode::LinearLight => {
                let b = 2.0 * src + dst - 1.0;
                lerp(dst, b, self.strength)
            }
            BlendMode::Difference => {
                let b = (dst - src).abs();
                lerp(dst, b, self.strength)
            }
            BlendMode::Darken => {
                let b = dst.min(src);
                lerp(dst, b, self.strength)
            }
            BlendMode::Lighten => {
                let b = dst.max(src);
                lerp(dst, b, self.strength)
            }
            BlendMode::ColorDodge => {
                let b = if src >= 1.0 {
                    1.0
                } else {
                    (dst / (1.0 - src)).min(1.0)
                };
                lerp(dst, b, self.strength)
            }
            BlendMode::ColorBurn => {
                let b = if src <= 0.0 {
                    0.0
                } else {
                    (1.0 - (1.0 - dst) / src).max(0.0)
                };
                lerp(dst, b, self.strength)
            }
            BlendMode::Subtract => {
                let b = dst - src;
                lerp(dst, b, self.strength)
            }
        }
    }
}
