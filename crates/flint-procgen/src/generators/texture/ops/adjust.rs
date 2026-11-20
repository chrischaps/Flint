//! Colour and value adjustment, pixel by pixel (`invert`,
//! `brightness_contrast`, `hsv_adjust`, `gamma`, `clamp`).

use crate::rng::{hsv_to_rgb, rgb_to_hsv};
use crate::SeededRng;

use super::{lerp, TextureOp};
use crate::generators::texture::descriptor::{OpDescriptor, ParamDesc as P};
use crate::generators::texture::field::TextureField;

// ─── InvertOp ────────────────────────────────────────────────────────────

/// Value inversion.
///
/// Reads `<input>`, writes `<output>`.
pub struct InvertOp {
    pub input: String,
    pub output: String,
    pub factor: f32,
}

/// The [`InvertOp`] op, as the pipeline knows it.
pub const INVERT: OpDescriptor = OpDescriptor {
    type_name: "invert",
    label: "Invert",
    summary: "Turn a channel's values over (v becomes 1 − v), fully or partway.",
    reads: &["<input>"],
    writes: &["<output>"],
    modifies: &[],
    params: &[
        P::channel("input", "input").help("Channel to invert"),
        P::channel("output", "output").help("Channel the result goes to"),
        P::float("factor", 1.0)
            .range(0.0, 1.0)
            .help("How far toward 1 − v: 0 leaves the values, 1 inverts them"),
    ],
    build: |p| {
        Ok(Box::new(InvertOp {
            input: p.text("input")?,
            output: p.text("output")?,
            factor: p.f32("factor")?,
        }))
    },
};

impl TextureOp for InvertOp {
    fn descriptor(&self) -> &'static OpDescriptor {
        &INVERT
    }

    fn apply(&self, field: &mut TextureField, _rng: &mut SeededRng) {
        let w = field.width;
        let h = field.height;
        field.ensure_channel(&self.output);

        for y in 0..h {
            for x in 0..w {
                let v = field.get(&self.input, x, y);
                field.set(&self.output, x, y, lerp(v, 1.0 - v, self.factor));
            }
        }
    }
}

// ─── BrightnessContrastOp ────────────────────────────────────────────────

/// Brightness and contrast adjustment.
///
/// Reads `<input>`, writes `<output>`.
pub struct BrightnessContrastOp {
    pub input: String,
    pub output: String,
    pub brightness: f32,
    pub contrast: f32,
}

/// The [`BrightnessContrastOp`] op, as the pipeline knows it.
pub const BRIGHTNESS_CONTRAST: OpDescriptor = OpDescriptor {
    type_name: "brightness_contrast",
    label: "Brightness/Contrast",
    summary: "Shift and stretch a channel about 0.5, clamped to 0–1.",
    reads: &["<input>"],
    writes: &["<output>"],
    modifies: &[],
    params: &[
        P::channel("input", "input").help("Channel to adjust"),
        P::channel("output", "output").help("Channel the result goes to"),
        P::float("brightness", 0.0).range(-1.0, 1.0).help("Added to every value"),
        P::float("contrast", 0.0).range(-1.0, 10.0).help("Stretch about 0.5: the result is (v − 0.5)(1 + contrast) + 0.5 + brightness; −1 flattens to grey"),
    ],
    build: |p| {
        Ok(Box::new(BrightnessContrastOp {
            input: p.text("input")?,
            output: p.text("output")?,
            brightness: p.f32("brightness")?,
            contrast: p.f32("contrast")?,
        }))
    },
};

impl TextureOp for BrightnessContrastOp {
    fn descriptor(&self) -> &'static OpDescriptor {
        &BRIGHTNESS_CONTRAST
    }

    fn apply(&self, field: &mut TextureField, _rng: &mut SeededRng) {
        let w = field.width;
        let h = field.height;
        field.ensure_channel(&self.output);

        for y in 0..h {
            for x in 0..w {
                let v = field.get(&self.input, x, y);
                let result = (v - 0.5) * (1.0 + self.contrast) + 0.5 + self.brightness;
                field.set(&self.output, x, y, result.clamp(0.0, 1.0));
            }
        }
    }
}

// ─── HsvAdjustOp ─────────────────────────────────────────────────────────

/// Hue/saturation/value adjustment on `r`, `g`, `b` channels.
///
/// Modifies `r`, `g`, `b` in-place.
pub struct HsvAdjustOp {
    pub hue_offset: f32,
    pub saturation_factor: f32,
    pub value_factor: f32,
}

/// The [`HsvAdjustOp`] op, as the pipeline knows it.
pub const HSV_ADJUST: OpDescriptor = OpDescriptor {
    type_name: "hsv_adjust",
    label: "HSV Adjust",
    summary: "Turn the albedo's hue and scale its saturation and value, in place.",
    reads: &[],
    writes: &[],
    modifies: &["r", "g", "b"],
    params: &[
        P::float("hue_offset", 0.0).range(-1.0, 1.0).help(
            "Turn of the hue, in whole turns of the colour wheel (0.5 is the opposite colour)",
        ),
        P::float("saturation_factor", 1.0)
            .range(0.0, 3.0)
            .help("Saturation multiplier: 0 is grey, 1 unchanged"),
        P::float("value_factor", 1.0)
            .range(0.0, 3.0)
            .help("Value (brightness) multiplier: 1 unchanged"),
    ],
    build: |p| {
        Ok(Box::new(HsvAdjustOp {
            hue_offset: p.f32("hue_offset")?,
            saturation_factor: p.f32("saturation_factor")?,
            value_factor: p.f32("value_factor")?,
        }))
    },
};

impl TextureOp for HsvAdjustOp {
    fn descriptor(&self) -> &'static OpDescriptor {
        &HSV_ADJUST
    }

    fn apply(&self, field: &mut TextureField, _rng: &mut SeededRng) {
        let w = field.width;
        let h = field.height;
        field.ensure_channel("r");
        field.ensure_channel("g");
        field.ensure_channel("b");

        for y in 0..h {
            for x in 0..w {
                let r = field.get("r", x, y);
                let g = field.get("g", x, y);
                let b = field.get("b", x, y);

                let (mut hue, mut sat, mut val) = rgb_to_hsv(r, g, b);
                hue = (hue + self.hue_offset).fract();
                if hue < 0.0 {
                    hue += 1.0;
                }
                sat = (sat * self.saturation_factor).clamp(0.0, 1.0);
                val = (val * self.value_factor).clamp(0.0, 1.0);

                let (nr, ng, nb) = hsv_to_rgb(hue, sat, val);
                field.set("r", x, y, nr.clamp(0.0, 1.0));
                field.set("g", x, y, ng.clamp(0.0, 1.0));
                field.set("b", x, y, nb.clamp(0.0, 1.0));
            }
        }
    }
}

// ─── GammaOp ─────────────────────────────────────────────────────────────

/// Gamma correction.
///
/// Reads `<input>`, writes `<output>`.
pub struct GammaOp {
    pub input: String,
    pub output: String,
    pub gamma: f32,
}

/// The [`GammaOp`] op, as the pipeline knows it.
pub const GAMMA: OpDescriptor = OpDescriptor {
    type_name: "gamma",
    label: "Gamma",
    summary: "Raise a channel to a power, bending its midtones.",
    reads: &["<input>"],
    writes: &["<output>"],
    modifies: &[],
    params: &[
        P::channel("input", "input").help("Channel to adjust"),
        P::channel("output", "output").help("Channel the result goes to"),
        P::float("gamma", 1.0)
            .range(0.01, 10.0)
            .help("The exponent: v^gamma, so above 1 darkens the midtones and below 1 lifts them"),
    ],
    build: |p| {
        Ok(Box::new(GammaOp {
            input: p.text("input")?,
            output: p.text("output")?,
            gamma: p.f32("gamma")?,
        }))
    },
};

impl TextureOp for GammaOp {
    fn descriptor(&self) -> &'static OpDescriptor {
        &GAMMA
    }

    fn apply(&self, field: &mut TextureField, _rng: &mut SeededRng) {
        let w = field.width;
        let h = field.height;
        field.ensure_channel(&self.output);

        for y in 0..h {
            for x in 0..w {
                let v = field.get(&self.input, x, y);
                field.set(&self.output, x, y, v.max(0.0).powf(self.gamma));
            }
        }
    }
}

// ─── ClampOp ─────────────────────────────────────────────────────────────

/// Constrain values to a min/max range.
///
/// Reads `<input>`, writes `<output>`.
pub struct ClampOp {
    pub input: String,
    pub output: String,
    pub min: f32,
    pub max: f32,
}

/// The [`ClampOp`] op, as the pipeline knows it.
pub const CLAMP: OpDescriptor = OpDescriptor {
    type_name: "clamp",
    label: "Clamp",
    summary: "Hold a channel's values between a minimum and a maximum.",
    reads: &["<input>"],
    writes: &["<output>"],
    modifies: &[],
    params: &[
        P::channel("input", "input").help("Channel to clamp"),
        P::channel("output", "output").help("Channel the result goes to"),
        P::float("min", 0.0).help("Lowest value let through"),
        P::float("max", 1.0).help("Highest value let through"),
    ],
    build: |p| {
        Ok(Box::new(ClampOp {
            input: p.text("input")?,
            output: p.text("output")?,
            min: p.f32("min")?,
            max: p.f32("max")?,
        }))
    },
};

impl TextureOp for ClampOp {
    fn descriptor(&self) -> &'static OpDescriptor {
        &CLAMP
    }

    fn apply(&self, field: &mut TextureField, _rng: &mut SeededRng) {
        let w = field.width;
        let h = field.height;
        field.ensure_channel(&self.output);

        for y in 0..h {
            for x in 0..w {
                let v = field.get(&self.input, x, y);
                field.set(&self.output, x, y, v.clamp(self.min, self.max));
            }
        }
    }
}
