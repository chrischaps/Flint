//! Value mapping: per-pixel scalar math, remapping a range, and a colour
//! ramp from a value (`math`, `map_range`, `color_ramp`).

use crate::SeededRng;

use super::{lerp, TextureOp};
use crate::generators::texture::descriptor::{OpDescriptor, ParamDesc as P};
use crate::generators::texture::field::TextureField;

// ─── MathOp ──────────────────────────────────────────────────────────────

/// Scalar math operations (Blender Math node equivalent).
///
/// Reads `<input_a>` (and optional `<input_b>`), writes `<output>`.
pub struct MathOp {
    pub operation: MathOperation,
    pub input_a: String,
    pub input_b: Option<String>,
    pub value_b: f32,
    pub output: String,
    pub clamp_output: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MathOperation {
    Add,
    Subtract,
    Multiply,
    Divide,
    Power,
    Sqrt,
    Abs,
    Min,
    Max,
    Fract,
    Modulo,
    Snap,
    Sin,
    Cos,
    LessThan,
    GreaterThan,
    Sign,
    Floor,
    Ceil,
    Round,
    PingPong,
    Wrap,
    Negate,
}

impl MathOperation {
    pub fn from_str(s: &str) -> Self {
        match s {
            "add" => MathOperation::Add,
            "subtract" => MathOperation::Subtract,
            "multiply" => MathOperation::Multiply,
            "divide" => MathOperation::Divide,
            "power" => MathOperation::Power,
            "sqrt" => MathOperation::Sqrt,
            "abs" => MathOperation::Abs,
            "min" => MathOperation::Min,
            "max" => MathOperation::Max,
            "fract" => MathOperation::Fract,
            "modulo" => MathOperation::Modulo,
            "snap" => MathOperation::Snap,
            "sin" => MathOperation::Sin,
            "cos" => MathOperation::Cos,
            "less_than" => MathOperation::LessThan,
            "greater_than" => MathOperation::GreaterThan,
            "sign" => MathOperation::Sign,
            "floor" => MathOperation::Floor,
            "ceil" => MathOperation::Ceil,
            "round" => MathOperation::Round,
            "pingpong" => MathOperation::PingPong,
            "wrap" => MathOperation::Wrap,
            "negate" => MathOperation::Negate,
            _ => MathOperation::Add,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            MathOperation::Add => "add",
            MathOperation::Subtract => "subtract",
            MathOperation::Multiply => "multiply",
            MathOperation::Divide => "divide",
            MathOperation::Power => "power",
            MathOperation::Sqrt => "sqrt",
            MathOperation::Abs => "abs",
            MathOperation::Min => "min",
            MathOperation::Max => "max",
            MathOperation::Fract => "fract",
            MathOperation::Modulo => "modulo",
            MathOperation::Snap => "snap",
            MathOperation::Sin => "sin",
            MathOperation::Cos => "cos",
            MathOperation::LessThan => "less_than",
            MathOperation::GreaterThan => "greater_than",
            MathOperation::Sign => "sign",
            MathOperation::Floor => "floor",
            MathOperation::Ceil => "ceil",
            MathOperation::Round => "round",
            MathOperation::PingPong => "pingpong",
            MathOperation::Wrap => "wrap",
            MathOperation::Negate => "negate",
        }
    }
}

/// The [`MathOp`] op, as the pipeline knows it.
pub const MATH: OpDescriptor = OpDescriptor {
    type_name: "math",
    label: "Math",
    summary: "Per-pixel arithmetic on one channel, with a second channel or a constant.",
    reads: &["<input_a>"],
    writes: &["<output>"],
    modifies: &[],
    params: &[
        P::choice(
            "operation",
            &[
                "add",
                "subtract",
                "multiply",
                "divide",
                "power",
                "sqrt",
                "abs",
                "min",
                "max",
                "fract",
                "modulo",
                "snap",
                "sin",
                "cos",
                "less_than",
                "greater_than",
                "sign",
                "floor",
                "ceil",
                "round",
                "pingpong",
                "wrap",
                "negate",
            ],
            "add",
        )
        .help("What to do with a and b (the one-argument ones ignore b); dividing by 0 gives 0"),
        P::channel("input_a", "input").help("Channel a"),
        P::opt_channel("input_b").help("Channel b; unset, b is value_b"),
        P::float("value_b", 0.0).help("The constant b, when input_b is unset"),
        P::channel("output", "output").help("Channel the result goes to"),
        P::flag("clamp", false).help("Clamp the result to 0–1"),
    ],
    build: |p| {
        Ok(Box::new(MathOp {
            operation: MathOperation::from_str(p.choice("operation")?),
            input_a: p.text("input_a")?,
            input_b: p.opt_text("input_b")?,
            value_b: p.f32("value_b")?,
            output: p.text("output")?,
            clamp_output: p.flag("clamp")?,
        }))
    },
};

impl TextureOp for MathOp {
    fn descriptor(&self) -> &'static OpDescriptor {
        &MATH
    }

    fn apply(&self, field: &mut TextureField, _rng: &mut SeededRng) {
        let w = field.width;
        let h = field.height;
        field.ensure_channel(&self.output);

        for y in 0..h {
            for x in 0..w {
                let a = field.get(&self.input_a, x, y);
                let b = self
                    .input_b
                    .as_ref()
                    .map(|ch| field.get(ch, x, y))
                    .unwrap_or(self.value_b);

                let result = match self.operation {
                    MathOperation::Add => a + b,
                    MathOperation::Subtract => a - b,
                    MathOperation::Multiply => a * b,
                    MathOperation::Divide => {
                        if b.abs() < 1e-10 {
                            0.0
                        } else {
                            a / b
                        }
                    }
                    MathOperation::Power => a.powf(b),
                    MathOperation::Sqrt => a.max(0.0).sqrt(),
                    MathOperation::Abs => a.abs(),
                    MathOperation::Min => a.min(b),
                    MathOperation::Max => a.max(b),
                    MathOperation::Fract => a.fract(),
                    MathOperation::Modulo => {
                        if b.abs() < 1e-10 {
                            0.0
                        } else {
                            a % b
                        }
                    }
                    MathOperation::Snap => {
                        if b.abs() < 1e-10 {
                            a
                        } else {
                            (a / b).floor() * b
                        }
                    }
                    MathOperation::Sin => (a * std::f32::consts::TAU).sin(),
                    MathOperation::Cos => (a * std::f32::consts::TAU).cos(),
                    MathOperation::LessThan => {
                        if a < b {
                            1.0
                        } else {
                            0.0
                        }
                    }
                    MathOperation::GreaterThan => {
                        if a > b {
                            1.0
                        } else {
                            0.0
                        }
                    }
                    MathOperation::Sign => {
                        if a > 0.0 {
                            1.0
                        } else if a < 0.0 {
                            -1.0
                        } else {
                            0.0
                        }
                    }
                    MathOperation::Floor => a.floor(),
                    MathOperation::Ceil => a.ceil(),
                    MathOperation::Round => a.round(),
                    MathOperation::PingPong => {
                        if b.abs() < 1e-10 {
                            0.0
                        } else {
                            let t = (a / b).rem_euclid(2.0);
                            if t <= 1.0 {
                                t * b
                            } else {
                                (2.0 - t) * b
                            }
                        }
                    }
                    MathOperation::Wrap => {
                        // Wrap a into [0, b)
                        if b.abs() < 1e-10 {
                            0.0
                        } else {
                            a.rem_euclid(b)
                        }
                    }
                    MathOperation::Negate => -a,
                };

                let result = if self.clamp_output {
                    result.clamp(0.0, 1.0)
                } else {
                    result
                };
                field.set(&self.output, x, y, result);
            }
        }
    }
}

// ─── MapRangeOp ──────────────────────────────────────────────────────────

/// Remap a value from one range to another.
///
/// Reads `<input>`, writes `<output>`.
pub struct MapRangeOp {
    pub input: String,
    pub output: String,
    pub from_min: f32,
    pub from_max: f32,
    pub to_min: f32,
    pub to_max: f32,
    pub interpolation: MapRangeInterp,
    pub clamp_output: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MapRangeInterp {
    Linear,
    Smoothstep,
    Smootherstep,
}

impl MapRangeInterp {
    pub fn from_str(s: &str) -> Self {
        match s {
            "smoothstep" => MapRangeInterp::Smoothstep,
            "smootherstep" => MapRangeInterp::Smootherstep,
            _ => MapRangeInterp::Linear,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            MapRangeInterp::Linear => "linear",
            MapRangeInterp::Smoothstep => "smoothstep",
            MapRangeInterp::Smootherstep => "smootherstep",
        }
    }
}

/// The [`MapRangeOp`] op, as the pipeline knows it.
pub const MAP_RANGE: OpDescriptor = OpDescriptor {
    type_name: "map_range",
    label: "Map Range",
    summary: "Remap a channel from one range to another, straight or eased.",
    reads: &["<input>"],
    writes: &["<output>"],
    modifies: &[],
    params: &[
        P::channel("input", "input").help("Channel to remap"),
        P::channel("output", "output").help("Channel the result goes to"),
        P::float("from_min", 0.0).help("Input value that maps to to_min"),
        P::float("from_max", 1.0).help("Input value that maps to to_max"),
        P::float("to_min", 0.0).help("Output at from_min"),
        P::float("to_max", 1.0).help("Output at from_max"),
        P::choice("interpolation", &["linear", "smoothstep", "smootherstep"], "linear").help("The curve between: linear, or eased at both ends (smoothstep, smootherstep; both clamp the input to the range)"),
        P::flag("clamp", true).help("Keep the result between to_min and to_max"),
    ],
    build: |p| {
        Ok(Box::new(MapRangeOp {
            input: p.text("input")?,
            output: p.text("output")?,
            from_min: p.f32("from_min")?,
            from_max: p.f32("from_max")?,
            to_min: p.f32("to_min")?,
            to_max: p.f32("to_max")?,
            interpolation: MapRangeInterp::from_str(p.choice("interpolation")?),
            clamp_output: p.flag("clamp")?,
        }))
    },
};

impl TextureOp for MapRangeOp {
    fn descriptor(&self) -> &'static OpDescriptor {
        &MAP_RANGE
    }

    fn apply(&self, field: &mut TextureField, _rng: &mut SeededRng) {
        let w = field.width;
        let h = field.height;
        let from_range = self.from_max - self.from_min;
        field.ensure_channel(&self.output);

        for y in 0..h {
            for x in 0..w {
                let v = field.get(&self.input, x, y);
                let t = if from_range.abs() < 1e-10 {
                    0.0
                } else {
                    (v - self.from_min) / from_range
                };

                let t = match self.interpolation {
                    MapRangeInterp::Linear => t,
                    MapRangeInterp::Smoothstep => {
                        t.clamp(0.0, 1.0).powi(2) * (3.0 - 2.0 * t.clamp(0.0, 1.0))
                    }
                    MapRangeInterp::Smootherstep => {
                        let tc = t.clamp(0.0, 1.0);
                        tc * tc * tc * (tc * (tc * 6.0 - 15.0) + 10.0)
                    }
                };

                let mut result = self.to_min + t * (self.to_max - self.to_min);
                if self.clamp_output {
                    let lo = self.to_min.min(self.to_max);
                    let hi = self.to_min.max(self.to_max);
                    result = result.clamp(lo, hi);
                }
                field.set(&self.output, x, y, result);
            }
        }
    }
}

// ─── ColorRampOp ─────────────────────────────────────────────────────────

/// Map a scalar channel to a multi-stop color gradient.
///
/// Reads `<input>`, writes `r`, `g`, `b`.
pub struct ColorRampOp {
    pub input: String,
    pub interpolation: ColorRampInterp,
    pub stops: Vec<ColorStop>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ColorRampInterp {
    Linear,
    Constant,
    Ease,
}

impl ColorRampInterp {
    pub fn from_str(s: &str) -> Self {
        match s {
            "constant" => ColorRampInterp::Constant,
            "ease" => ColorRampInterp::Ease,
            _ => ColorRampInterp::Linear,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ColorStop {
    pub position: f32,
    pub color: [f32; 3],
}

/// The [`ColorRampOp`] op, as the pipeline knows it.
pub const COLOR_RAMP: OpDescriptor = OpDescriptor {
    type_name: "color_ramp",
    label: "Color Ramp",
    summary: "Colour the albedo by a channel through a gradient of stops.",
    reads: &["<input>"],
    writes: &["r", "g", "b"],
    modifies: &[],
    params: &[
        P::channel("input", "input").help("Channel (0–1) that picks the colour"),
        P::choice("interpolation", &["linear", "constant", "ease"], "linear").help("Between stops: linear blends, constant holds each stop's colour up to the next, ease blends with an S-curve"),
        P::tables("stops", "color_ramp stop", &[
            P::float("position", 0.0).range(0.0, 1.0).help("Where on the input this colour sits"),
            P::color("color", [0.0, 0.0, 0.0, 1.0]).help("The colour there, hex (alpha is ignored)"),
        ]).help("The ramp's colours, as [[params.ops.stops]] tables, at least one; sorted by position"),
    ],
    build: |p| {
        let mut stops = Vec::new();
        for stop in p.tables("stops")? {
            let c = stop.color("color")?;
            stops.push(ColorStop {
                position: stop.f32("position")?,
                color: [c[0], c[1], c[2]],
            });
        }
        stops.sort_by(|a, b| {
            a.position
                .partial_cmp(&b.position)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        Ok(Box::new(ColorRampOp {
            input: p.text("input")?,
            interpolation: ColorRampInterp::from_str(p.choice("interpolation")?),
            stops,
        }))
    },
};

impl TextureOp for ColorRampOp {
    fn descriptor(&self) -> &'static OpDescriptor {
        &COLOR_RAMP
    }

    fn apply(&self, field: &mut TextureField, _rng: &mut SeededRng) {
        let w = field.width;
        let h = field.height;
        field.ensure_channel("r");
        field.ensure_channel("g");
        field.ensure_channel("b");

        if self.stops.is_empty() {
            return;
        }

        for y in 0..h {
            for x in 0..w {
                let v = field.get(&self.input, x, y).clamp(0.0, 1.0);
                let (r, g, b) = self.evaluate(v);
                field.set("r", x, y, r);
                field.set("g", x, y, g);
                field.set("b", x, y, b);
            }
        }
    }
}

impl ColorRampOp {
    fn evaluate(&self, t: f32) -> (f32, f32, f32) {
        if self.stops.len() == 1 {
            let s = &self.stops[0];
            return (s.color[0], s.color[1], s.color[2]);
        }

        // Find the surrounding stops
        if t <= self.stops[0].position {
            let s = &self.stops[0];
            return (s.color[0], s.color[1], s.color[2]);
        }
        if t >= self.stops[self.stops.len() - 1].position {
            let s = &self.stops[self.stops.len() - 1];
            return (s.color[0], s.color[1], s.color[2]);
        }

        for i in 0..self.stops.len() - 1 {
            let a = &self.stops[i];
            let b = &self.stops[i + 1];
            if t >= a.position && t <= b.position {
                let range = b.position - a.position;
                if range < 1e-10 {
                    return (a.color[0], a.color[1], a.color[2]);
                }
                let frac = (t - a.position) / range;
                let frac = match self.interpolation {
                    ColorRampInterp::Linear => frac,
                    ColorRampInterp::Constant => 0.0,
                    ColorRampInterp::Ease => frac * frac * (3.0 - 2.0 * frac),
                };
                return (
                    lerp(a.color[0], b.color[0], frac),
                    lerp(a.color[1], b.color[1], frac),
                    lerp(a.color[2], b.color[2], frac),
                );
            }
        }

        let s = &self.stops[self.stops.len() - 1];
        (s.color[0], s.color[1], s.color[2])
    }
}
