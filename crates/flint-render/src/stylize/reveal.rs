//! A reveal mask for whole-frame styles (#406): the style shows inside a
//! screen-space region and the frame it read shows outside, with a drawn
//! line along the edge between them. One number, `reveal`, grows the
//! region from nothing (0) to the whole frame (1), so a scene can start
//! live and tear open into the sketch with one tween:
//!
//! ```toml
//! [post_process.styles.pencil]
//! reveal = 0            # all live; 1 is all sketch
//! reveal_shape = "tear" # tear, wipe, iris or panel
//! ```
//!
//! ```rhai
//! tween_post_style_param("pencil", "reveal", 1.0, 4.0);
//! ```
//!
//! Every style takes it, as every style takes `blend` (#369): the stage
//! draws it, not the style. After a style whose reveal is between 0 and 1
//! has drawn, [`RevealPass::draw`] draws what the style read back over its
//! result outside the region, and the ink along the edge. At 0 the style
//! leaves the chain ([`StyleSettings::shows`](super::StyleSettings::shows)),
//! so the frame is the live one byte for byte; at 1 the draw is skipped.
//! One full-screen draw, no per-frame allocation: a 256-byte slot of one
//! uniform buffer per revealing style.
//!
//! The ragged edge is the render-mode tears' noise (`tear_mask.wgsl`'s
//! `fbm` and `value_noise`, prepended to `reveal.wgsl`): big lobes that stay
//! put and small teeth that the boil redraws `reveal_boil` times a second of
//! post time, so `--style-time` moves it and the same time draws the same
//! edge.

use super::{kit, StyleFrame, StyleInput, StyleStage};
use std::sync::{Mutex, OnceLock};

/// What shape the revealed region grows in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RevealShape {
    /// A ragged hole that rips open from a point (`reveal_x`, `reveal_y`).
    #[default]
    Tear,
    /// A ragged line that crosses the frame toward `reveal_angle`.
    Wipe,
    /// A circle from a point, its rim wobbling a little.
    Iris,
    /// A comic panel: a rectangle the frame's shape, from a point out to
    /// every edge.
    Panel,
}

impl RevealShape {
    pub const NAMES: &'static [&'static str] = &["tear", "wipe", "iris", "panel"];

    pub fn from_name(name: &str) -> Option<Self> {
        Some(match name {
            "tear" => Self::Tear,
            "wipe" => Self::Wipe,
            "iris" => Self::Iris,
            "panel" => Self::Panel,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        Self::NAMES[self as usize]
    }

    /// `reveal_ragged` when the table sets none: how far the edge strays
    /// from the shape, as a share of the frame.
    pub fn default_ragged(self) -> f32 {
        match self {
            Self::Tear => 0.3,
            Self::Wipe => 0.12,
            Self::Iris => 0.05,
            Self::Panel => 0.0,
        }
    }
}

/// One of the reveal's number keys.
#[derive(Debug, Clone, Copy)]
pub struct RevealNumber {
    pub key: &'static str,
    /// `None`: the shape's ([`RevealShape::default_ragged`]).
    pub default: Option<f32>,
    pub min: f32,
    pub max: f32,
    pub help: &'static str,
}

/// The reveal's number keys besides `reveal` itself.
pub const NUMBERS: &[RevealNumber] = &[
    RevealNumber {
        key: "reveal_scale",
        default: Some(2.5),
        min: 0.25,
        max: 24.0,
        help: "how many lobes the ragged edge has across the frame's height",
    },
    RevealNumber {
        key: "reveal_ragged",
        default: None,
        min: 0.0,
        max: 1.0,
        help: "how far the edge strays from the shape, as a share of the frame \
               (tear 0.3, wipe 0.12, iris 0.05, panel 0)",
    },
    RevealNumber {
        key: "reveal_edge",
        default: Some(2.0),
        min: 0.0,
        max: 16.0,
        help: "the drawn line's width in points; 0 draws none",
    },
    RevealNumber {
        key: "reveal_angle",
        default: Some(0.0),
        min: -360.0,
        max: 360.0,
        help: "a wipe's direction of travel in degrees: 0 left to right, 90 top to bottom",
    },
    RevealNumber {
        key: "reveal_x",
        default: Some(0.5),
        min: 0.0,
        max: 1.0,
        help: "where a tear, iris or panel starts, across the frame (0 left)",
    },
    RevealNumber {
        key: "reveal_y",
        default: Some(0.5),
        min: 0.0,
        max: 1.0,
        help: "where a tear, iris or panel starts, down the frame (0 top)",
    },
    RevealNumber {
        key: "reveal_boil",
        default: Some(12.0),
        min: 0.0,
        max: 30.0,
        help: "how many times a second of post time the edge's teeth are redrawn; 0 holds them",
    },
    RevealNumber {
        key: "reveal_seed",
        default: Some(0.0),
        min: 0.0,
        max: 1000.0,
        help: "another tear of the same shape",
    },
    RevealNumber {
        key: "reveal_rim",
        default: Some(0.0),
        min: 0.0,
        max: 16.0,
        help: "a band of bare paper on the style's side of the edge, in points, \
               ragged as torn paper's is; 0 draws none",
    },
    RevealNumber {
        key: "reveal_shadow",
        default: Some(0.0),
        min: 0.0,
        max: 1.0,
        help: "how dark the shadow the torn edge casts on the live side is; 0 casts none",
    },
];

/// The ink a reveal's edge draws in when the table names none: soft lead.
pub const DEFAULT_INK: [f32; 3] = [0.12, 0.11, 0.1];

/// The rim's paper when the table names no `reveal_rim_color` and the
/// style has no `paper` colour of its own: pencil's.
pub const DEFAULT_PAPER: [f32; 3] = [0.95, 0.93, 0.88];

/// How far an unnamed rim colour is lifted from the paper toward white.
const RIM_LIFT: f32 = 0.5;

/// How far the edge's shadow reaches onto the live side, in points (#876).
const SHADOW_POINTS: f32 = 14.0;

/// Every key the reveal reads, in the order a table writes them.
pub const KEYS: [&str; 14] = [
    "reveal",
    "reveal_shape",
    "reveal_ink",
    "reveal_rim_color",
    "reveal_scale",
    "reveal_ragged",
    "reveal_edge",
    "reveal_angle",
    "reveal_x",
    "reveal_y",
    "reveal_boil",
    "reveal_seed",
    "reveal_rim",
    "reveal_shadow",
];

/// Whether `key` is one of the reveal's.
pub fn is_key(key: &str) -> bool {
    KEYS.contains(&key)
}

/// A whole-frame style's reveal, as its table sets it. With no `reveal`
/// (no [`amount`](Self::amount)) there is no mask, whatever else it sets.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct StyleReveal {
    /// `reveal`, 0 to 1.
    pub amount: Option<f32>,
    pub shape: Option<RevealShape>,
    /// `reveal_ink`, in display values.
    pub ink: Option<[f32; 3]>,
    /// `reveal_rim_color`, in display values (#876); `None` is the
    /// style's paper.
    pub rim_color: Option<[f32; 3]>,
    /// The [`NUMBERS`] the table sets, by key.
    pub numbers: std::collections::BTreeMap<&'static str, f32>,
}

impl StyleReveal {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    pub fn shape(&self) -> RevealShape {
        self.shape.unwrap_or_default()
    }

    pub fn ink(&self) -> [f32; 3] {
        self.ink.unwrap_or(DEFAULT_INK)
    }

    /// The rim's colour: `reveal_rim_color` as set, else `paper` (the
    /// style's own) lifted halfway to white, the fresh core a tear shows,
    /// so the rim reads on a blank sheet too.
    pub fn rim_color(&self, paper: [f32; 3]) -> [f32; 3] {
        self.rim_color
            .unwrap_or_else(|| paper.map(|c| c + (1.0 - c) * RIM_LIFT))
    }

    /// Number `key` (one of [`NUMBERS`]) as set, else its default.
    pub fn number(&self, key: &str) -> f32 {
        if let Some(v) = self.numbers.get(key) {
            return *v;
        }
        match NUMBERS.iter().find(|n| n.key == key) {
            Some(RevealNumber {
                default: Some(d), ..
            }) => *d,
            Some(_) => self.shape().default_ragged(),
            None => 0.0,
        }
    }

    /// Set `key` from a TOML value: a number for `reveal` (0..1) and the
    /// [`NUMBERS`], a name for `reveal_shape`, a colour (`[r, g, b]` or
    /// `"#rrggbb"`) for `reveal_ink` and `reveal_rim_color`. Out of range, a number is clamped
    /// when `strict` is off and refused, naming the range, when it is on
    /// (the command line, #355). `Ok(false)`: not a reveal key.
    pub fn set(&mut self, key: &str, value: &toml::Value, strict: bool) -> Result<bool, String> {
        let number = || {
            flint_core::toml_util::toml_f64(value)
                .filter(|n| n.is_finite())
                .map(|n| n as f32)
        };
        let ranged = |n: f32, min: f32, max: f32| {
            if strict && !(min..=max).contains(&n) {
                Err(format!("{key} must be from {min} to {max}, not {n}"))
            } else {
                Ok(n.clamp(min, max))
            }
        };
        match key {
            "reveal" => {
                let n = number().ok_or_else(|| format!("{key} must be a number from 0 to 1"))?;
                self.amount = Some(ranged(n, 0.0, 1.0)?);
            }
            "reveal_shape" => {
                let name = value.as_str().ok_or_else(|| {
                    format!("{key} must be one of {}", RevealShape::NAMES.join(", "))
                })?;
                self.shape = Some(RevealShape::from_name(name).ok_or_else(|| {
                    format!(
                        "{key}: no shape '{name}'; one of {}",
                        RevealShape::NAMES.join(", ")
                    )
                })?);
            }
            "reveal_ink" | "reveal_rim_color" => {
                let rgb = super::color_from_toml(value)
                    .ok_or_else(|| format!("{key} must be a colour: [r, g, b] or \"#rrggbb\""))?
                    .map(|c| c.clamp(0.0, 1.0));
                if key == "reveal_ink" {
                    self.ink = Some(rgb);
                } else {
                    self.rim_color = Some(rgb);
                }
            }
            _ => {
                let Some(spec) = NUMBERS.iter().find(|n| n.key == key) else {
                    return Ok(false);
                };
                let n = number().ok_or_else(|| format!("{key} must be a number"))?;
                self.numbers
                    .insert(spec.key, ranged(n, spec.min, spec.max)?);
            }
        }
        Ok(true)
    }

    /// [`set`](Self::set) from command-line text: a colour as `#rrggbb` or
    /// `r/g/b`, a shape by name, else a number.
    pub fn set_text(&mut self, key: &str, text: &str) -> Result<bool, String> {
        let value = match key {
            "reveal_shape" => toml::Value::String(text.to_string()),
            "reveal_ink" | "reveal_rim_color" => match super::color_from_text(text) {
                Some(rgb) => toml::Value::Array(
                    rgb.iter()
                        .map(|c| toml::Value::Float(f64::from(*c)))
                        .collect(),
                ),
                None => {
                    return Err(format!(
                        "{key} must be a colour, #rrggbb or r/g/b, not '{text}'"
                    ))
                }
            },
            _ => match text.parse::<f64>() {
                Ok(n) => toml::Value::Float(n),
                Err(_) if is_key(key) => {
                    return Err(format!("{key} must be a number, not '{text}'"))
                }
                Err(_) => return Ok(false),
            },
        };
        self.set(key, &value, true)
    }

    /// The TOML lines that set this reveal, for Copy TOML.
    pub fn toml_lines(&self) -> String {
        let mut out = String::new();
        let float = |v: f32| {
            let r = (v * 1000.0).round() / 1000.0;
            if r.fract() == 0.0 {
                format!("{r:.1}")
            } else {
                format!("{r}")
            }
        };
        if let Some(a) = self.amount {
            out.push_str(&format!("reveal = {}\n", float(a)));
        }
        if let Some(s) = self.shape {
            out.push_str(&format!("reveal_shape = \"{}\"\n", s.name()));
        }
        if let Some(ink) = self.ink {
            let [r, g, b] = ink.map(float);
            out.push_str(&format!("reveal_ink = [{r}, {g}, {b}]\n"));
        }
        if let Some(rim) = self.rim_color {
            let [r, g, b] = rim.map(float);
            out.push_str(&format!("reveal_rim_color = [{r}, {g}, {b}]\n"));
        }
        for n in NUMBERS {
            if let Some(v) = self.numbers.get(n.key) {
                out.push_str(&format!("{} = {}\n", n.key, float(*v)));
            }
        }
        out
    }

    /// The uniform this reveal draws with into a `width` x `height`
    /// target at post `time`, `blend` and `pixels_per_point`; `linear`
    /// when the target holds linear light (the HDR stage, an sRGB view),
    /// so the ink and rim are given in display values either way. `paper`
    /// is the style's paper, the rim's colour unless the table names one.
    #[allow(clippy::too_many_arguments)]
    pub fn uniform(
        &self,
        width: u32,
        height: u32,
        time: f32,
        blend: f32,
        pixels_per_point: f32,
        linear: bool,
        paper: [f32; 3],
    ) -> RevealUniform {
        let amount = self.amount.unwrap_or(1.0).clamp(0.0, 1.0);
        let ragged = self.number("reveal_ragged");
        let threshold = threshold(self.shape(), amount, ragged);
        let angle = self.number("reveal_angle").to_radians();
        let boil = self.number("reveal_boil");
        let step = if boil > 0.0 {
            (time * boil).floor()
        } else {
            0.0
        };
        let ink = self
            .ink()
            .map(|c| if linear { srgb_to_linear(c) } else { c });
        let ppp = pixels_per_point.max(0.01);
        let half = 0.5 * self.number("reveal_edge") * ppp;
        let rim = self
            .rim_color(paper)
            .map(|c| if linear { srgb_to_linear(c) } else { c });
        RevealUniform {
            shape: [
                self.shape() as u32 as f32,
                self.number("reveal_scale"),
                ragged,
                threshold,
            ],
            place: [
                self.number("reveal_x"),
                self.number("reveal_y"),
                angle.cos(),
                angle.sin(),
            ],
            edge: [
                half,
                blend.clamp(0.0, 1.0),
                step,
                self.number("reveal_seed"),
            ],
            ink: [ink[0], ink[1], ink[2], 0.0],
            size: [width.max(1) as f32, height.max(1) as f32, 0.0, 0.0],
            rim: [rim[0], rim[1], rim[2], self.number("reveal_rim") * ppp],
            shadow: [self.number("reveal_shadow"), SHADOW_POINTS * ppp, 0.0, 0.0],
        }
    }
}

/// Where the field is cut at `amount` (0..1) of a reveal of `shape` whose
/// edge strays by up to `ragged`. The field runs 0 at the start to 1 at the
/// last pixel the shape reaches.
///
/// Neither end needs to be exact here: at 0 the style leaves the chain and
/// at 1 the draw is skipped, so those two frames are the live one and the
/// style's own, byte for byte. In between, the cut runs from where the
/// noise first breaks through (a pinhole of the tear) to past the field's
/// top by half its stray (the noise seldom strays further, so the last
/// scraps of the live frame linger in the corners until the very end).
/// A panel's amount is eased so the share of the frame shown grows with
/// it: its area goes as the square of its reach.
fn threshold(shape: RevealShape, amount: f32, ragged: f32) -> f32 {
    let eased = match shape {
        RevealShape::Wipe => amount,
        RevealShape::Panel => amount.sqrt(),
        RevealShape::Tear | RevealShape::Iris => amount,
    };
    let start = -0.35 * ragged;
    let end = 1.0 + 0.5 * ragged + 0.05;
    start + eased * (end - start)
}

fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// `reveal.wgsl`'s `Reveal`.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct RevealUniform {
    pub shape: [f32; 4],
    pub place: [f32; 4],
    pub edge: [f32; 4],
    pub ink: [f32; 4],
    pub size: [f32; 4],
    /// rim rgb (in the target's space), rim width in pixels (#876)
    pub rim: [f32; 4],
    /// shadow strength, reach in pixels, unused
    pub shadow: [f32; 4],
}

/// A uniform slot per revealing style in a frame; more than this many in
/// one frame draw the rest as a plain blend, with a warning.
const SLOTS: u64 = 16;
const SLOT: u64 = 256;
const BIND: &str = "style reveal input";

struct Shared {
    module: wgpu::ShaderModule,
    input_layout: wgpu::BindGroupLayout,
    uniform: wgpu::Buffer,
    uniform_group: wgpu::BindGroup,
    uniform_layout: wgpu::BindGroupLayout,
}

/// The reveal draw: its pipelines, one per target format, and its uniform
/// buffer, all made on first use.
#[derive(Default)]
pub struct RevealPass {
    shared: OnceLock<Shared>,
    pipelines: Mutex<Vec<(wgpu::TextureFormat, std::sync::Arc<wgpu::RenderPipeline>)>>,
    /// (frame, next free slot)
    slots: Mutex<(u64, u64)>,
    warned: std::sync::atomic::AtomicBool,
}

impl RevealPass {
    fn shared(&self, device: &wgpu::Device) -> &Shared {
        self.shared.get_or_init(|| {
            let module = kit::shader(
                device,
                "Style Reveal Shader",
                kit::wgsl!("../tear_mask.wgsl", "reveal.wgsl"),
            );
            let input_layout = kit::sampled_layout(device, "Style Reveal BGL");
            let uniform_layout =
                device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                    label: Some("Style Reveal Uniform BGL"),
                    entries: &[wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Uniform,
                            has_dynamic_offset: true,
                            min_binding_size: wgpu::BufferSize::new(
                                std::mem::size_of::<RevealUniform>() as u64,
                            ),
                        },
                        count: None,
                    }],
                });
            let uniform = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Style Reveal Uniforms"),
                size: SLOTS * SLOT,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            let uniform_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("Style Reveal Uniform Group"),
                layout: &uniform_layout,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer: &uniform,
                        offset: 0,
                        size: wgpu::BufferSize::new(std::mem::size_of::<RevealUniform>() as u64),
                    }),
                }],
            });
            Shared {
                module,
                input_layout,
                uniform,
                uniform_group,
                uniform_layout,
            }
        })
    }

    fn pipeline(
        &self,
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
    ) -> std::sync::Arc<wgpu::RenderPipeline> {
        let shared = self.shared(device);
        let mut pipelines = self.pipelines.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((_, p)) = pipelines.iter().find(|(f, _)| *f == format) {
            return p.clone();
        }
        // Premultiplied: what the shader writes, plus (1 - a) of the
        // style's result
        let component = wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::One,
            dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
            operation: wgpu::BlendOperation::Add,
        };
        let pipeline = std::sync::Arc::new(kit::fullscreen_pipeline_with(
            device,
            "Style Reveal Pipeline",
            &shared.module,
            "fs_main",
            &[&shared.input_layout, &shared.uniform_layout],
            format,
            Some(wgpu::BlendState {
                color: component,
                alpha: component,
            }),
        ));
        pipelines.push((format, pipeline.clone()));
        pipeline
    }

    /// The slot this draw writes, or `None` when the frame has used them
    /// all.
    fn slot(&self, frame: u64) -> Option<u64> {
        let mut slots = self.slots.lock().unwrap_or_else(|e| e.into_inner());
        if slots.0 != frame {
            *slots = (frame, 0);
        }
        let slot = slots.1;
        if slot >= SLOTS {
            return None;
        }
        slots.1 += 1;
        Some(slot)
    }

    /// Draw `reveal` of the style just drawn into `output`: what the style
    /// read (`input`) back outside the region, by `blend` inside it, and
    /// the edge's ink. `false` when it drew nothing (no slot left), so the
    /// caller draws the plain blend.
    pub fn draw(
        &self,
        frame: &StyleFrame<'_>,
        encoder: &mut wgpu::CommandEncoder,
        input: &StyleInput<'_>,
        output: &wgpu::TextureView,
        reveal: &StyleReveal,
        blend: f32,
    ) -> bool {
        let Some(slot) = self.slot(frame.frame) else {
            if !self.warned.swap(true, std::sync::atomic::Ordering::Relaxed) {
                tracing::warn!(
                    "more than {SLOTS} styles reveal in one frame; the rest draw without their reveal"
                );
            }
            return false;
        };
        let linear = frame.descriptor.stage == StyleStage::Hdr || frame.format.is_srgb();
        // The rim is bare paper: the style's own, where it has one
        let paper = if frame.descriptor.color_setting("paper").is_some() {
            frame.color("paper")
        } else {
            DEFAULT_PAPER
        };
        let uniform = reveal.uniform(
            frame.width,
            frame.height,
            frame.time,
            blend,
            frame.pixels_per_point,
            linear,
            paper,
        );
        let pipeline = self.pipeline(frame.device, frame.format);
        let shared = self.shared(frame.device);
        frame
            .queue
            .write_buffer(&shared.uniform, slot * SLOT, bytemuck::bytes_of(&uniform));
        let group = frame.sampled_input(BIND, &shared.input_layout, input);
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Style Reveal Pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: output,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &*group, &[]);
        pass.set_bind_group(1, &shared.uniform_group, &[(slot * SLOT) as u32]);
        pass.draw(0..3, 0..1);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_read_clamp_and_refuse() {
        let mut r = StyleReveal::default();
        assert!(r.set("reveal", &0.4.into(), false).unwrap());
        assert_eq!(r.amount, Some(0.4));
        assert!(r.set("reveal", &3_i64.into(), false).unwrap());
        assert_eq!(r.amount, Some(1.0), "clamped, from an integer too");
        assert!(
            r.set("reveal", &3.0.into(), true).is_err(),
            "refused on the command line"
        );
        assert!(r.set("reveal_shape", &"wipe".into(), false).unwrap());
        assert_eq!(r.shape(), RevealShape::Wipe);
        let e = r.set("reveal_shape", &"star".into(), false).unwrap_err();
        assert!(e.contains("panel"), "{e}");
        assert!(r.set_text("reveal_ink", "#ff0000").unwrap());
        assert_eq!(r.ink(), [1.0, 0.0, 0.0]);
        assert!(r.set_text("reveal_edge", "4").unwrap());
        assert_eq!(r.number("reveal_edge"), 4.0);
        assert!(
            !r.set("cell", &4.0.into(), false).unwrap(),
            "not a reveal key"
        );
        assert!(!r.set_text("ramp", "blocks").unwrap());
    }

    #[test]
    fn ragged_defaults_follow_the_shape() {
        let mut r = StyleReveal::default();
        assert_eq!(r.number("reveal_ragged"), 0.3);
        r.shape = Some(RevealShape::Panel);
        assert_eq!(r.number("reveal_ragged"), 0.0);
        r.numbers.insert("reveal_ragged", 0.2);
        assert_eq!(r.number("reveal_ragged"), 0.2);
    }

    #[test]
    fn the_threshold_climbs_past_the_field() {
        // Field 0..1, edge straying by up to `ragged`: at 1 the cut is past
        // the field's top by half the stray; it climbs all the way
        for shape in [
            RevealShape::Tear,
            RevealShape::Wipe,
            RevealShape::Iris,
            RevealShape::Panel,
        ] {
            let ragged = shape.default_ragged();
            assert!(
                threshold(shape, 1.0, ragged) > 1.0 + 0.5 * ragged,
                "{shape:?}"
            );
            assert!(threshold(shape, 0.0, ragged) <= 0.0, "{shape:?}");
            let mut last = f32::MIN;
            for i in 0..=20 {
                let t = threshold(shape, i as f32 / 20.0, ragged);
                assert!(t > last, "{shape:?} at {i}");
                last = t;
            }
        }
        // A panel's cut at a quarter is half its reach: a quarter of the frame
        let half = threshold(RevealShape::Panel, 0.25, 0.0);
        assert!((half - 0.525).abs() < 1e-4, "{half}");
    }

    #[test]
    fn a_reveal_at_0_leaves_the_chain_and_one_without_reveal_shows() {
        let mut s = super::super::StyleSettings::default();
        assert!(s.shows("pencil"));
        s.set_reveal("pencil", "reveal", &0.0.into(), true).unwrap();
        assert!(!s.shows("pencil"));
        s.set_reveal("pencil", "reveal", &0.01.into(), true)
            .unwrap();
        assert!(s.shows("pencil"));
        // A shape alone is no reveal
        let mut t = super::super::StyleSettings::default();
        t.set_reveal("pencil", "reveal_shape", &"wipe".into(), true)
            .unwrap();
        assert!(t.reveal("pencil").is_none() && t.shows("pencil"));
    }

    #[test]
    fn the_boil_steps_on_post_time() {
        let r = StyleReveal {
            amount: Some(0.5),
            ..Default::default()
        };
        let at = |t: f32| r.uniform(100, 100, t, 1.0, 1.0, false, DEFAULT_PAPER).edge[2];
        assert_eq!(at(0.0), 0.0);
        assert_eq!(at(0.05), 0.0, "inside the first twelfth of a second");
        assert_eq!(at(0.09), 1.0);
        assert_eq!(at(1.0), 12.0);
        let held = StyleReveal {
            numbers: [("reveal_boil", 0.0)].into_iter().collect(),
            ..r.clone()
        };
        assert_eq!(
            held.uniform(100, 100, 5.0, 1.0, 1.0, false, DEFAULT_PAPER)
                .edge[2],
            0.0
        );
    }

    #[test]
    fn the_uniform_is_seven_vec4s() {
        assert_eq!(std::mem::size_of::<RevealUniform>(), 112);
        assert!(std::mem::size_of::<RevealUniform>() as u64 <= SLOT);
    }

    #[test]
    fn the_rim_is_the_styles_paper_unless_named() {
        let mut r = StyleReveal {
            amount: Some(0.5),
            ..Default::default()
        };
        let paper = [0.8, 0.6, 0.4];
        let u = r.uniform(100, 100, 0.0, 1.0, 2.0, false, paper);
        for (got, want) in u.rim[..3].iter().zip([0.9, 0.8, 0.7]) {
            assert!(
                (got - want).abs() < 1e-6,
                "the paper lifted halfway to white"
            );
        }
        assert_eq!(u.rim[3], 0.0, "no rim by default");
        assert_eq!(u.shadow[0], 0.0, "no shadow by default");
        assert!(r.set_text("reveal_rim_color", "#ffffff").unwrap());
        assert!(r.set_text("reveal_rim", "3").unwrap());
        assert!(r.set_text("reveal_shadow", "0.4").unwrap());
        let u = r.uniform(100, 100, 0.0, 1.0, 2.0, false, paper);
        assert_eq!(u.rim, [1.0, 1.0, 1.0, 6.0], "points times pixels per point");
        assert_eq!(u.shadow[..2], [0.4, SHADOW_POINTS * 2.0]);
        assert!(r.set_text("reveal_shadow", "2").is_err(), "0 to 1");
    }

    #[test]
    fn every_reveal_key_is_reserved() {
        for key in KEYS {
            assert!(super::super::RESERVED_KEYS.contains(&key), "{key}");
        }
        for n in NUMBERS {
            assert!(KEYS.contains(&n.key), "{}", n.key);
        }
    }

    #[test]
    fn toml_lines_read_back() {
        let mut r = StyleReveal::default();
        r.set("reveal", &0.25.into(), false).unwrap();
        r.set("reveal_shape", &"panel".into(), false).unwrap();
        r.set("reveal_edge", &3.0.into(), false).unwrap();
        r.set("reveal_ink", &"#000000".into(), false).unwrap();
        r.set("reveal_rim", &3.0.into(), false).unwrap();
        r.set("reveal_shadow", &0.4.into(), false).unwrap();
        r.set("reveal_rim_color", &"#ffffff".into(), false).unwrap();
        let text = r.toml_lines();
        let table: toml::Table = toml::from_str(&text).unwrap();
        let mut back = StyleReveal::default();
        for (k, v) in &table {
            assert!(back.set(k, v, true).unwrap(), "{k}");
        }
        assert_eq!(back, r, "{text}");
    }
}
