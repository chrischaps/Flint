//! Stylize passes (#262): one interface for Kuwahara, ASCII, watercolor,
//! pencil and whatever comes after them.
//!
//! A style is a module in this directory that exports a
//! [`StyleDescriptor`] named `STYLE`, plus one name in [`register_styles!`]
//! at the bottom of this file. The descriptor says where the style sits in
//! the chain ([`StyleStage`]), which parameters it takes, when it is on,
//! and how to build its pipelines; the [`StylePass`] it builds records its
//! draws for one frame from a [`StyleFrame`]. Everything else is the
//! framework's:
//!
//! - **Pipelines** are built the first time a style is switched on
//!   ([`StyleSet::ensure`], from `SceneRenderer::ensure_style_resources`),
//!   under `catch_unwind`, since some drivers crash compiling a shader.
//!   A style that fails stays off and the renderer reports a lost device.
//! - **Targets.** Each stage hands a style its input and an output to draw
//!   into, ping-ponging between two targets the framework owns, so styles
//!   chain. A style's own intermediates come from [`StyleFrame::target`].
//!   All of them live in [`StyleTargets`] inside the post chain's
//!   resources, which `resize_postprocess()` rebuilds, so no style has a
//!   resize hook of its own and none can keep a stale view.
//! - **Bind groups** are cached with the chain (the same [`BindCache`] the
//!   rest of the post chain uses, #139), and samplers come from the chain's
//!   shared linear sampler: a style's bind groups hold no sampler of their
//!   own beyond that one, which keeps the DX12 sampler heap safe.
//! - **Config.** A style reads its parameters from
//!   [`PostProcessConfig::styles`], which a scene sets with
//!   `[post_process.styles.<name>]`; the F4 Rendering & Effects menu draws
//!   its sliders from the descriptor. Kuwahara predates the table and keeps
//!   its own `kuwahara_*` fields.
//!
//! ## Order
//!
//! Styles of one stage run in registration order, each reading the one
//! before it. The stages sit at two fixed places in the post chain:
//!
//! ```text
//! scene (HDR, render scale)
//!   -> style mask (Rg16Uint: id + blend, while a style reads it; #263, #278)
//!   -> Hdr styles (Kuwahara)                 linear Rgba16Float, scene resolution
//!   -> [ASCII spike, #261]
//!   -> SSAO, volumetric, bloom (read the scene depth / original HDR)
//!   -> composite: fog, DoF, bloom add, tonemap, grade, vignette, grain, render modes
//!   -> Display styles (ASCII, pencil, invert) surface format, output resolution
//!   -> FXAA
//!   -> surface
//! ```
//!
//! ## The entity style mask
//!
//! A style that stylizes only chosen entities sets
//! [`StyleDescriptor::uses_mask`] and reads [`StyleFrame::mask`]: one id a
//! pixel, the entity's `style_mask` style ([`StyleFrame::style_id`] finds
//! the style's own), drawn after the scene pass (`crate::style_mask`).
//! `mask_view` is the example, and the `style` debug view.
//!
//! A per-entity style ([`StyleDescriptor::component`], #266) goes further:
//! an entity's own component switches it on and carries its settings, each
//! distinct set an id of its own, read with [`StyleFrame::variants`]. ASCII
//! is the example.
//!
//! An `Hdr` style is shaded by what follows it: fog fades it, bloom haloes
//! its bright parts, the grade tints it (Kuwahara's painterly look wants
//! that). A `Display` style sees the finished picture at output
//! resolution, after the render-scale upscale, which is where the ASCII
//! spike found glyphs have to be drawn to stay crisp (#261, finding 7).
//! SSAO and bloom read the unstylized HDR; that is deliberate for now (the
//! light comes from the scene, not the brush strokes).
//!
//! [`BindCache`]: crate::bind_cache::BindCache

pub mod custom;
pub mod kit;
mod spec;
mod targets;

pub use spec::{apply_style_spec, style_spec_help, StyleSpec};
pub use targets::{StyleTargets, StyleTexture};

use crate::bind_cache::BindCache;
use crate::postprocess::PostProcessConfig;
use crate::style_mask::StyleIds;
use std::collections::BTreeMap;
use std::sync::Arc;

/// Where in the post chain a style runs; see the module docs for the order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum StyleStage {
    /// Linear HDR (`Rgba16Float`) at the scene's render scale, after the
    /// scene pass and before fog, bloom, grade and tonemapping.
    Hdr,
    /// Display-referred colour in the surface format, at output
    /// resolution, after the composite and before FXAA. When the surface
    /// format is sRGB, samples arrive linear and writes are encoded on the
    /// way out: a style that works in display values decodes and encodes
    /// itself ([`StyleFrame::srgb`]).
    Display,
}

impl StyleStage {
    /// Both stages, in chain order.
    pub const ALL: [StyleStage; 2] = [StyleStage::Hdr, StyleStage::Display];
}

/// One numeric parameter a style takes, with the range the F4 menu offers.
#[derive(Clone, Copy, Debug)]
pub struct StyleParam {
    pub name: &'static str,
    pub default: f32,
    pub min: f32,
    pub max: f32,
}

/// One named choice a style takes (a font, a ramp, a colour mode): a
/// string in `[post_process.styles.<name>]`, where a parameter is a number.
#[derive(Clone, Copy, Debug)]
pub struct StyleOption {
    pub name: &'static str,
    pub default: &'static str,
    /// The values the F4 menu offers. Empty means any text (a file or set
    /// name). A style still gets whatever the scene says and decides what
    /// a value outside the list means.
    pub choices: &'static [&'static str],
}

/// How the F4 menu presents one parameter or option (#268): a readable
/// label, a hover line, and whether only whole numbers make sense. Hints
/// are optional; a setting without one shows its name ("tint_r" ->
/// "Tint r") on a plain slider, so a new style needs none to be tunable.
///
/// A colour is three parameters named `<base>_r`, `<base>_g`, `<base>_b`
/// (each 0..1, display values: what the eye sees, sRGB-encoded): the
/// menu folds them into one colour picker, and its hint is the base's
/// (`tint`).
#[derive(Clone, Copy, Debug)]
pub struct StyleHint {
    /// The parameter, option or colour base the hint is for.
    pub name: &'static str,
    pub label: &'static str,
    /// One line for the hover text; empty for none.
    pub help: &'static str,
    /// Whole numbers only (a cell size, a count): the slider steps by 1.
    pub integer: bool,
}

impl StyleHint {
    pub const fn new(name: &'static str, label: &'static str, help: &'static str) -> Self {
        Self {
            name,
            label,
            help,
            integer: false,
        }
    }

    /// The same hint, stepping by whole numbers.
    pub const fn integer(self) -> Self {
        Self {
            integer: true,
            ..self
        }
    }
}

/// One row of a style's section in the F4 menu, in the order it is drawn:
/// options first (they change what the sliders mean), then parameters,
/// with each `<base>_r/_g/_b` triple folded into a [`Color`](Self::Color)
/// where its red channel stood.
#[derive(Clone, Copy, Debug)]
pub enum StyleControl {
    Option(&'static StyleOption),
    Param(&'static StyleParam),
    Color {
        /// The name before `_r` (`tint`), which hints are keyed by.
        base: &'static str,
        channels: [&'static StyleParam; 3],
    },
}

impl StyleControl {
    /// The parameter, option or colour base this row edits.
    pub fn name(&self) -> &'static str {
        match self {
            StyleControl::Option(o) => o.name,
            StyleControl::Param(p) => p.name,
            StyleControl::Color { base, .. } => base,
        }
    }
}

/// What the framework needs to build a style.
#[derive(Clone, Copy)]
pub struct StyleDevice<'a> {
    pub device: &'a wgpu::Device,
    /// The style being built: lets one `create` serve many descriptors
    /// (user-authored styles, #274, all share [`custom`]'s).
    pub descriptor: &'static StyleDescriptor,
    /// The format the style's output is drawn in: `Rgba16Float` for an
    /// [`Hdr`](StyleStage::Hdr) style, the surface's for a
    /// [`Display`](StyleStage::Display) one.
    pub format: wgpu::TextureFormat,
}

/// A registered style: what it is called, where it runs and how to make it.
pub struct StyleDescriptor {
    /// The name in `[post_process.styles.<name>]`; lowercase, no spaces.
    pub name: &'static str,
    /// What the F4 menu and `--gpu-time` call it.
    pub label: &'static str,
    pub stage: StyleStage,
    /// Parameters and their defaults, read with [`StyleFrame::param`].
    pub params: &'static [StyleParam],
    /// Named choices and their defaults, read with [`StyleFrame::option`].
    pub options: &'static [StyleOption],
    /// When the style is on. `None` reads `enabled` from
    /// [`PostProcessConfig::styles`]; a style with fields of its own on
    /// [`PostProcessConfig`] (Kuwahara) reads those instead.
    pub enabled: Option<fn(&PostProcessConfig) -> bool>,
    /// Whether the style reads the entity style mask (#263). While any
    /// such style is on, the renderer draws the mask each frame and hands
    /// it over as [`StyleFrame::mask`]; while none is, the mask isn't made.
    pub uses_mask: bool,
    /// How the F4 menu labels its settings ([`StyleHint`]); `&[]` is fine.
    pub hints: &'static [StyleHint],
    /// The component an entity takes this style with, its fields being the
    /// style's parameters and options (`ascii_style`, #266). A style with
    /// one is per-entity: an entity carrying it (or a `style_mask` naming
    /// the style) switches the style on for that entity's pixels alone,
    /// with no `[post_process.styles]` table, and the style reads each
    /// pixel's settings from [`StyleFrame::variants`] by its mask id.
    pub component: Option<&'static str>,
    /// Build the style's pipelines. May panic on a broken driver; the
    /// framework catches it.
    pub create: fn(&StyleDevice) -> Box<dyn StylePass>,
}

impl StyleDescriptor {
    /// Whether `config` switches this style on (post-processing as a whole
    /// aside).
    pub fn is_enabled(&self, config: &PostProcessConfig) -> bool {
        match self.enabled {
            Some(enabled) => enabled(config),
            None => config.styles.is_enabled(self.name),
        }
    }

    /// Whether this style runs this frame: `config` switches it on, or it
    /// is per-entity and an entity asks for it (#266).
    pub fn is_on(&self, config: &PostProcessConfig, ids: &StyleIds) -> bool {
        self.is_enabled(config) || (self.component.is_some() && ids.requests(self.name))
    }

    /// Whether this style reads the style mask this frame: it always does
    /// (`uses_mask`), or it is per-entity and an entity names it, opt-outs
    /// included.
    pub fn reads_mask(&self, config: &PostProcessConfig, ids: &StyleIds) -> bool {
        self.is_on(config, ids)
            && (self.uses_mask || (self.component.is_some() && ids.mentions(self.name)))
    }

    /// Whether the style's parameters live in the generic table (and so
    /// the generic F4 section can draw them).
    pub fn uses_style_table(&self) -> bool {
        self.enabled.is_none()
    }

    /// Parameter `name`'s value in `config`, or its default.
    pub fn param(&self, config: &PostProcessConfig, name: &str) -> f32 {
        let default = self
            .params
            .iter()
            .find(|p| p.name == name)
            .map_or(0.0, |p| p.default);
        config.styles.param(self.name, name).unwrap_or(default)
    }

    /// Option `name`'s value in `config`, or its default.
    pub fn option<'c>(&self, config: &'c PostProcessConfig, name: &str) -> &'c str {
        let default = self
            .options
            .iter()
            .find(|o| o.name == name)
            .map_or("", |o| o.default);
        config.styles.option(self.name, name).unwrap_or(default)
    }

    /// The hint for a parameter, option or colour base, if the style gives
    /// one.
    pub fn hint(&self, name: &str) -> Option<&'static StyleHint> {
        self.hints.iter().find(|h| h.name == name)
    }

    /// What the F4 menu calls setting `name`: its hint's label, or the
    /// name made readable (`tint_r` -> `Tint r`).
    pub fn label_for(&self, name: &str) -> String {
        match self.hint(name) {
            Some(h) => h.label.to_string(),
            None => humanize(name),
        }
    }

    /// The rows of this style's F4 section, in drawing order.
    pub fn controls(&self) -> Vec<StyleControl> {
        let mut rows: Vec<StyleControl> = self.options.iter().map(StyleControl::Option).collect();
        let find = |name: String| self.params.iter().find(|p| p.name == name);
        for p in self.params {
            if let Some(base) = p.name.strip_suffix("_r") {
                if let (Some(g), Some(b)) = (find(format!("{base}_g")), find(format!("{base}_b"))) {
                    rows.push(StyleControl::Color {
                        base,
                        channels: [p, g, b],
                    });
                    continue;
                }
            }
            let folded = ["_g", "_b"].iter().any(|suffix| {
                p.name.strip_suffix(suffix).is_some_and(|base| {
                    ["_r", "_g", "_b"]
                        .iter()
                        .all(|c| find(format!("{base}{c}")).is_some())
                })
            });
            if !folded {
                rows.push(StyleControl::Param(p));
            }
        }
        rows
    }
}

/// `tint_r` -> `Tint r`: a setting's name as a label.
fn humanize(name: &str) -> String {
    let spaced = name.replace('_', " ");
    let mut chars = spaced.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// Style `descriptor`'s settings in `config` as the scene TOML that would
/// set them (#268), for the F4 menu's Copy TOML: the table header, and
/// only the values that differ from the defaults. A style that is off says
/// so, since a table on its own switches its style on.
pub fn style_toml(descriptor: &StyleDescriptor, config: &PostProcessConfig) -> String {
    let mut out = format!("[post_process.styles.{}]\n", descriptor.name);
    if !descriptor.is_enabled(config) {
        out.push_str("enabled = false\n");
    }
    for o in descriptor.options {
        let value = descriptor.option(config, o.name);
        if value != o.default {
            out.push_str(&format!("{} = {:?}\n", o.name, value));
        }
    }
    for p in descriptor.params {
        let value = descriptor.param(config, p.name);
        if value != p.default {
            let integer =
                descriptor.hint(p.name).is_some_and(|h| h.integer) && value.fract() == 0.0;
            if integer {
                out.push_str(&format!("{} = {}\n", p.name, value as i64));
            } else {
                out.push_str(&format!("{} = {}\n", p.name, toml_float(value)));
            }
        }
    }
    out
}

/// An `f32` as a TOML float: the shortest spelling that reads back the
/// same, with a `.0` so it stays a float.
fn toml_float(value: f32) -> String {
    let text = format!("{value}");
    if text.contains(['.', 'e', 'E']) || !value.is_finite() {
        text
    } else {
        format!("{text}.0")
    }
}

/// A style's GPU side: pipelines, layouts and uniform buffers, made once.
pub trait StylePass: Send + Sync {
    /// Record this frame's draws: read `input`, write every pixel of
    /// `output` (both [`StyleFrame::width`] x [`StyleFrame::height`], in
    /// [`StyleFrame::format`]). `input_id` distinguishes the views the
    /// input can be; key bind groups over it with it
    /// ([`StyleFrame::sampled_input`] does).
    fn encode(
        &self,
        frame: &StyleFrame<'_>,
        encoder: &mut wgpu::CommandEncoder,
        input: &StyleInput<'_>,
        output: &wgpu::TextureView,
    );
}

/// The view a style reads, with an id that's stable for as long as the
/// chain's bind groups are.
pub struct StyleInput<'a> {
    pub view: &'a wgpu::TextureView,
    pub id: u64,
}

/// The camera, for styles that read depth (edges, fog-aware washes).
#[derive(Clone, Copy, Debug)]
pub struct StyleCamera {
    /// World to clip, column-major (for projecting an anchor, #266).
    pub view_proj: [[f32; 4]; 4],
    pub inv_view_proj: [[f32; 4]; 4],
    pub position: [f32; 3],
    pub near: f32,
    pub far: f32,
}

/// Everything a style may read in one frame.
pub struct StyleFrame<'a> {
    pub device: &'a wgpu::Device,
    pub queue: &'a wgpu::Queue,
    pub config: &'a PostProcessConfig,
    pub descriptor: &'static StyleDescriptor,
    /// The size of the input and output, in pixels.
    pub width: u32,
    pub height: u32,
    /// The input's and output's format.
    pub format: wgpu::TextureFormat,
    /// The scene's depth (single-sample; under a render scale, the scaled
    /// scene's). Sample it by UV: at the display stage it is smaller than
    /// the output.
    pub depth: &'a wgpu::TextureView,
    /// [`depth`](Self::depth)'s size in pixels, which a view doesn't say:
    /// the scene's, so smaller than `width` x `height` at the display stage
    /// under a render scale. The style mask is this size too.
    pub depth_size: [u32; 2],
    /// Whether `depth` is the chain's own, so bind groups over it may be
    /// kept ([`StyleFrame::depth_bind`]).
    pub depth_owned: bool,
    pub camera: StyleCamera,
    /// The entity style mask (#263): one style id a pixel
    /// ([`STYLE_MASK_FORMAT`](crate::style_mask::STYLE_MASK_FORMAT),
    /// `texture_2d<u32>`, 0 = unstyled), at the scene's resolution like
    /// [`depth`](Self::depth), so a display-stage style reads it by UV
    /// with `textureLoad`. `Some` whenever a style with
    /// [`uses_mask`](StyleDescriptor::uses_mask) is on. It lives with the
    /// chain; bind it with [`mask_bind`](Self::mask_bind).
    pub mask: Option<&'a wgpu::TextureView>,
    /// Which id each style name has in [`mask`](Self::mask).
    pub style_ids: &'a crate::style_mask::StyleIds,
    /// The chain's linear, clamped sampler.
    pub linear_sampler: &'a wgpu::Sampler,
    /// Seconds of game time, for animated styles.
    pub time: f32,
    /// This frame's number: one more than the last frame the renderer drew
    /// through the post chain. A style that keeps history from frame to
    /// frame (ASCII's temporal stability, #277) trusts it only when the
    /// last frame it drew is `frame - 1`, so a frame drawn without it (the
    /// style switched off a while) starts it over.
    pub frame: u64,
    /// Bumped by [`SceneRenderer::reset_style_history`](crate::SceneRenderer::reset_style_history)
    /// (a new scene, a camera cut): history from another epoch is not
    /// this picture's past.
    pub history_epoch: u64,
    /// Physical pixels per logical point of the window (the OS scale
    /// factor). A style sized in points multiplies by it, so its cells keep
    /// their size on a high-DPI display. 1 headless unless asked.
    pub pixels_per_point: f32,
    /// The scene's folder, for styles that load files (a glyph set from
    /// `fonts/`), when the host knows it.
    pub scene_dir: Option<&'a std::path::Path>,
    pub(crate) binds: &'a BindCache,
    pub(crate) targets: &'a StyleTargets,
}

impl StyleFrame<'_> {
    pub fn texel_size(&self) -> [f32; 2] {
        [1.0 / self.width as f32, 1.0 / self.height as f32]
    }

    /// Whether [`format`](Self::format) is sRGB: samples come back linear
    /// and writes are encoded.
    pub fn srgb(&self) -> bool {
        self.format.is_srgb()
    }

    /// This style's parameter `name`, from the config or its default.
    pub fn param(&self, name: &str) -> f32 {
        self.descriptor.param(self.config, name)
    }

    /// This style's option `name`, from the config or its default.
    pub fn option(&self, name: &str) -> &str {
        self.descriptor.option(self.config, name)
    }

    /// A texture of the style's own, `width` x `height` in `format`, made
    /// on first use and kept until the chain is rebuilt (on resize).
    /// `name` must be unique across styles; prefix it with the style's.
    pub fn target(&self, name: &'static str, format: wgpu::TextureFormat) -> Arc<StyleTexture> {
        self.targets
            .get(self.device, name, format, self.width, self.height)
    }

    /// [`target`](Self::target) at a size of the style's own, such as one
    /// texel per glyph cell. A name asked for at a new size is made again,
    /// so bind groups over it must be keyed by something that changes with
    /// it (see the ASCII style's history generation).
    pub fn target_sized(
        &self,
        name: &'static str,
        format: wgpu::TextureFormat,
        width: u32,
        height: u32,
    ) -> Arc<StyleTexture> {
        self.targets.get(self.device, name, format, width, height)
    }

    /// A bind group kept with the chain, made with `make` the first time.
    pub fn bind(
        &self,
        name: &'static str,
        variant: u64,
        make: impl FnOnce() -> wgpu::BindGroup,
    ) -> Arc<wgpu::BindGroup> {
        self.binds.get(name, variant, make)
    }

    /// A bind group over [`depth`](Self::depth): kept when the chain owns
    /// the depth, made for this frame alone when it doesn't.
    pub fn depth_bind(
        &self,
        name: &'static str,
        variant: u64,
        make: impl FnOnce() -> wgpu::BindGroup,
    ) -> Arc<wgpu::BindGroup> {
        self.binds.get_if(self.depth_owned, name, variant, make)
    }

    /// True the first time it's asked about `name` while the chain lives:
    /// for uniforms that only change on resize, such as texel sizes.
    pub fn first_write(&self, name: &'static str) -> bool {
        self.binds.first_write(name)
    }

    /// A kept bind group of `view` (binding 0) and the linear sampler
    /// (binding 1), the shape most passes read a texture in. `view` must
    /// live as long as the chain: a [`target`](Self::target) of this style.
    pub fn sampled(
        &self,
        name: &'static str,
        layout: &wgpu::BindGroupLayout,
        view: &wgpu::TextureView,
    ) -> Arc<wgpu::BindGroup> {
        self.bind(name, 0, || self.make_sampled(name, layout, view))
    }

    /// [`sampled`](Self::sampled) over the style's input, keyed by which
    /// view the input is this frame.
    pub fn sampled_input(
        &self,
        name: &'static str,
        layout: &wgpu::BindGroupLayout,
        input: &StyleInput<'_>,
    ) -> Arc<wgpu::BindGroup> {
        self.bind(name, input.id, || {
            self.make_sampled(name, layout, input.view)
        })
    }

    /// A kept bind group of the style mask at binding 0, for a layout made
    /// with [`kit::mask_layout`]; `None` while there's no mask.
    pub fn mask_bind(
        &self,
        name: &'static str,
        layout: &wgpu::BindGroupLayout,
    ) -> Option<Arc<wgpu::BindGroup>> {
        let mask = self.mask?;
        Some(self.bind(name, 0, || {
            self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(name),
                layout,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(mask),
                }],
            })
        }))
    }

    /// The style mask id of style `name` this frame, if an entity asks
    /// for it.
    pub fn style_id(&self, name: &str) -> Option<u16> {
        self.style_ids.id(name)
    }

    /// `(mask id, variant)` for each of this style's per-entity variants
    /// this frame (#266), in id order. Read a variant's settings with
    /// [`variant_param`](Self::variant_param) and
    /// [`variant_option`](Self::variant_option).
    pub fn variants(&self) -> impl Iterator<Item = (u16, &crate::style_mask::StyleVariant)> {
        self.style_ids.variants_of(self.descriptor.name)
    }

    /// Parameter `name` of `variant`: the entity's, else the scene's, else
    /// the style's default.
    pub fn variant_param(&self, variant: &crate::style_mask::StyleVariant, name: &str) -> f32 {
        variant.param(self.descriptor, self.config, name)
    }

    /// Option `name` of `variant`: the entity's, else the scene's, else the
    /// style's default.
    pub fn variant_option<'v>(
        &'v self,
        variant: &'v crate::style_mask::StyleVariant,
        name: &str,
    ) -> &'v str {
        variant.option(self.descriptor, self.config, name)
    }

    /// Whether `config` switches this style on for the whole frame, rather
    /// than only for the entities that ask for it.
    pub fn whole_frame(&self) -> bool {
        self.descriptor.is_enabled(self.config)
    }

    /// Where world point `p` lands in this frame, in pixels from the top
    /// left; `None` behind the camera.
    pub fn project(&self, p: [f32; 3]) -> Option<[f32; 2]> {
        let m = &self.camera.view_proj;
        let row = |r: usize| m[0][r] * p[0] + m[1][r] * p[1] + m[2][r] * p[2] + m[3][r];
        let w = row(3);
        if !w.is_finite() || w <= 1e-6 {
            return None;
        }
        let (x, y) = (row(0) / w, row(1) / w);
        Some([
            (x * 0.5 + 0.5) * self.width as f32,
            (0.5 - y * 0.5) * self.height as f32,
        ])
    }

    /// A kept bind group of one uniform buffer at binding 0.
    pub fn uniform(
        &self,
        name: &'static str,
        layout: &wgpu::BindGroupLayout,
        buffer: &wgpu::Buffer,
    ) -> Arc<wgpu::BindGroup> {
        self.bind(name, 0, || {
            self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(name),
                layout,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: buffer.as_entire_binding(),
                }],
            })
        })
    }

    fn make_sampled(
        &self,
        name: &'static str,
        layout: &wgpu::BindGroupLayout,
        view: &wgpu::TextureView,
    ) -> wgpu::BindGroup {
        self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some(name),
            layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(self.linear_sampler),
                },
            ],
        })
    }
}

// ---------------------------------------------------------------------------
// Settings: `[post_process.styles.<name>]`
// ---------------------------------------------------------------------------

/// One style's settings from the config.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct StyleSetting {
    pub enabled: bool,
    pub params: BTreeMap<String, f32>,
    pub options: BTreeMap<String, String>,
}

/// The generic style settings on [`PostProcessConfig`], by style name.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct StyleSettings {
    styles: BTreeMap<String, StyleSetting>,
}

impl StyleSettings {
    pub fn is_enabled(&self, name: &str) -> bool {
        self.styles.get(name).is_some_and(|s| s.enabled)
    }

    pub fn set_enabled(&mut self, name: &str, enabled: bool) {
        self.styles.entry(name.to_string()).or_default().enabled = enabled;
    }

    pub fn param(&self, name: &str, param: &str) -> Option<f32> {
        self.styles.get(name)?.params.get(param).copied()
    }

    pub fn set_param(&mut self, name: &str, param: &str, value: f32) {
        self.styles
            .entry(name.to_string())
            .or_default()
            .params
            .insert(param.to_string(), value);
    }

    pub fn option(&self, name: &str, option: &str) -> Option<&str> {
        self.styles
            .get(name)?
            .options
            .get(option)
            .map(String::as_str)
    }

    pub fn set_option(&mut self, name: &str, option: &str, value: &str) {
        self.styles
            .entry(name.to_string())
            .or_default()
            .options
            .insert(option.to_string(), value.to_string());
    }

    /// Style `name`'s settings, if it has any.
    pub fn get(&self, name: &str) -> Option<&StyleSetting> {
        self.styles.get(name)
    }

    /// Put style `name`'s parameters and options back to their defaults,
    /// leaving it on or off as it was.
    pub fn reset(&mut self, name: &str) {
        if let Some(setting) = self.styles.get_mut(name) {
            setting.params.clear();
            setting.options.clear();
        }
    }

    /// Make style `name` exactly what it is in `from` (the scene's
    /// authored settings): on, off and every value; no settings in `from`
    /// means none here.
    pub fn restore(&mut self, name: &str, from: &StyleSettings) {
        match from.styles.get(name) {
            Some(setting) => {
                self.styles.insert(name.to_string(), setting.clone());
            }
            None => {
                self.styles.remove(name);
            }
        }
    }

    /// Whether style `name` is set the same here and in `other`. A style
    /// with no settings is the same as one that is off with none.
    pub fn same_style(&self, name: &str, other: &StyleSettings) -> bool {
        let empty = StyleSetting::default();
        self.styles.get(name).unwrap_or(&empty) == other.styles.get(name).unwrap_or(&empty)
    }

    /// The styles that have settings, by name.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &StyleSetting)> {
        self.styles.iter().map(|(k, v)| (k.as_str(), v))
    }

    /// Read a scene's `[post_process.styles]` tables. A table switches its
    /// style on unless it says `enabled = false`; every other key is a
    /// number. Returns the settings and a warning for each name, key or
    /// value the registry doesn't know.
    pub fn from_tables(tables: &BTreeMap<String, toml::Table>) -> (Self, Vec<String>) {
        let mut settings = Self::default();
        let mut warnings = Vec::new();
        for (name, table) in tables {
            let Some(descriptor) = find(name) else {
                warnings.push(format!(
                    "unknown style '{name}' in [post_process.styles]; registered: {}",
                    names().join(", ")
                ));
                continue;
            };
            let mut setting = StyleSetting {
                enabled: true,
                ..Default::default()
            };
            for (key, value) in table {
                if key == "enabled" {
                    match value.as_bool() {
                        Some(on) => setting.enabled = on,
                        None => {
                            warnings.push(format!("styles.{name}.enabled must be true or false"))
                        }
                    }
                    continue;
                }
                if descriptor.options.iter().any(|o| o.name == key) {
                    match value.as_str() {
                        Some(text) => {
                            setting.options.insert(key.clone(), text.to_string());
                        }
                        None => warnings.push(format!("styles.{name}.{key} must be a string")),
                    }
                    continue;
                }
                if !descriptor.params.iter().any(|p| p.name == key) {
                    let known: Vec<&str> = descriptor
                        .params
                        .iter()
                        .map(|p| p.name)
                        .chain(descriptor.options.iter().map(|o| o.name))
                        .collect();
                    warnings.push(format!(
                        "style '{name}' has no parameter '{key}'; it takes: {}",
                        if known.is_empty() {
                            "none".to_string()
                        } else {
                            known.join(", ")
                        }
                    ));
                    continue;
                }
                let number = value
                    .as_float()
                    .or_else(|| value.as_integer().map(|i| i as f64));
                match number {
                    Some(n) => {
                        setting.params.insert(key.clone(), n as f32);
                    }
                    None => warnings.push(format!("styles.{name}.{key} must be a number")),
                }
            }
            settings.styles.insert(name.clone(), setting);
        }
        (settings, warnings)
    }
}

// ---------------------------------------------------------------------------
// The set of built styles
// ---------------------------------------------------------------------------

/// The styles whose pipelines exist, by registry index. Lives on the post
/// pipeline; nothing in it depends on the frame's size.
#[derive(Default)]
pub struct StyleSet {
    built: Vec<Option<Box<dyn StylePass>>>,
    /// Styles whose creation panicked; never retried.
    failed: Vec<bool>,
}

impl StyleSet {
    /// Build every style `config` switches on that isn't built yet.
    /// Returns the ones whose creation panicked (they stay off for the
    /// rest of the run).
    pub fn ensure(
        &mut self,
        device: &wgpu::Device,
        surface_format: wgpu::TextureFormat,
        config: &PostProcessConfig,
        ids: &StyleIds,
    ) -> Vec<&'static StyleDescriptor> {
        let registry = registry();
        self.built.resize_with(registry.len(), || None);
        self.failed.resize(registry.len(), false);
        let mut failed = Vec::new();
        for (i, descriptor) in registry.iter().enumerate() {
            if self.built[i].is_some() || self.failed[i] || !descriptor.is_on(config, ids) {
                continue;
            }
            let format = match descriptor.stage {
                StyleStage::Hdr => crate::postprocess::HDR_FORMAT,
                StyleStage::Display => surface_format,
            };
            let made = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                (descriptor.create)(&StyleDevice {
                    device,
                    descriptor,
                    format,
                })
            }));
            match made {
                Ok(pass) => self.built[i] = Some(pass),
                Err(_) => {
                    self.failed[i] = true;
                    failed.push(*descriptor);
                }
            }
        }
        failed
    }

    /// Whether style `name` is built.
    pub fn is_built(&self, name: &str) -> bool {
        registry()
            .iter()
            .position(|d| d.name == name)
            .and_then(|i| self.built.get(i))
            .is_some_and(Option::is_some)
    }

    /// The built styles of `stage` that are on this frame (by `config` or
    /// an entity's request in `ids`), in chain order.
    pub fn active(
        &self,
        stage: StyleStage,
        config: &PostProcessConfig,
        ids: &StyleIds,
    ) -> Vec<(&'static StyleDescriptor, &dyn StylePass)> {
        if !config.enabled {
            return Vec::new();
        }
        registry()
            .iter()
            .zip(&self.built)
            .filter(|(d, _)| d.stage == stage && d.is_on(config, ids))
            .filter_map(|(d, built)| built.as_deref().map(|p| (*d, p)))
            .collect()
    }
}

/// Which view each style of a stage reads and writes: `Input` is what the
/// stage starts from, `Ping(n)` the framework's two targets, `Final` the
/// caller's target.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Slot {
    Input,
    Ping(u8),
    Final,
}

/// The ping-pong plan for `count` styles. With `to_final`, the last style
/// writes the caller's target; without, the stage's result stays in a ping
/// target (the HDR stage, whose result the composite reads).
pub fn plan(count: usize, to_final: bool) -> Vec<(Slot, Slot)> {
    let mut steps = Vec::with_capacity(count);
    let mut src = Slot::Input;
    for i in 0..count {
        let dst = if to_final && i + 1 == count {
            Slot::Final
        } else {
            Slot::Ping((i % 2) as u8)
        };
        steps.push((src, dst));
        src = dst;
    }
    steps
}

/// Ids of the views a style's input can be, for keying bind groups.
pub(crate) mod input_id {
    /// The scene's HDR buffer.
    pub const HDR: u64 = 0;
    /// The HDR stage's ping targets: `HDR_PING + n`.
    pub const HDR_PING: u64 = 1;
    /// What the composite writes when display styles follow it.
    pub const DISPLAY_IN: u64 = 3;
    /// The display stage's ping targets: `DISPLAY_PING + n`.
    pub const DISPLAY_PING: u64 = 4;
}

const HDR_PINGS: [&str; 2] = ["style hdr ping 0", "style hdr ping 1"];
const DISPLAY_PINGS: [&str; 2] = ["style display ping 0", "style display ping 1"];

/// A GPU-timer scope around one style: `(label, encoder, draw)`.
pub(crate) type Scope<'s> = dyn FnMut(&'static str, &mut wgpu::CommandEncoder, &mut dyn FnMut(&mut wgpu::CommandEncoder))
    + 's;

/// Run `styles` (one stage's, in order) from `input`, ping-ponging through
/// the stage's targets. With `to`, the last style writes there and this
/// returns `None`; without, it returns the target holding the result and
/// that target's input id. `frame` carries the stage's size and format;
/// its descriptor is set per style.
pub(crate) fn run_stage(
    styles: &[(&'static StyleDescriptor, &dyn StylePass)],
    frame: &mut StyleFrame<'_>,
    stage: StyleStage,
    encoder: &mut wgpu::CommandEncoder,
    input: StyleInput<'_>,
    to: Option<&wgpu::TextureView>,
    scope: &mut Scope<'_>,
) -> Option<(Arc<StyleTexture>, u64)> {
    let (names, base) = match stage {
        StyleStage::Hdr => (HDR_PINGS, input_id::HDR_PING),
        StyleStage::Display => (DISPLAY_PINGS, input_id::DISPLAY_PING),
    };
    let mut result = None;
    let pings: Vec<Arc<StyleTexture>> = (0..styles.len().min(2))
        .map(|n| frame.target(names[n], frame.format))
        .collect();
    for ((descriptor, pass), (src, dst)) in styles.iter().zip(plan(styles.len(), to.is_some())) {
        frame.descriptor = *descriptor;
        let src = match src {
            Slot::Input => StyleInput {
                view: input.view,
                id: input.id,
            },
            Slot::Ping(n) => StyleInput {
                view: &pings[n as usize].view,
                id: base + u64::from(n),
            },
            Slot::Final => unreachable!("no style reads the final target"),
        };
        let dst_view = match dst {
            Slot::Ping(n) => {
                result = Some((pings[n as usize].clone(), base + u64::from(n)));
                &pings[n as usize].view
            }
            Slot::Final => {
                result = None;
                to.expect("planned with a final target")
            }
            Slot::Input => unreachable!("no style writes its stage's input"),
        };
        let frame = &*frame;
        scope(descriptor.label, encoder, &mut |encoder| {
            pass.encode(frame, encoder, &src, dst_view)
        });
    }
    result
}

// ---------------------------------------------------------------------------
// Registry
// ---------------------------------------------------------------------------

/// Declare the style modules and the registry in one list: adding a style
/// is its module plus its name here. Order is chain order within a stage.
macro_rules! register_styles {
    ($($module:ident),* $(,)?) => {
        $(mod $module;)*
        static REGISTRY: &[&StyleDescriptor] = &[$(&$module::STYLE),*];
    };
}

register_styles!(kuwahara, watercolor, ascii, pencil, invert, mask_view);

/// The built-ins followed by user styles registered at run time
/// ([`custom`], #274). Append-only, so a style's index in it (which
/// [`StyleSet`] keys its built passes by) never changes; each append leaks
/// one small slice, once per style file loaded.
static SNAPSHOT: std::sync::RwLock<&'static [&'static StyleDescriptor]> =
    std::sync::RwLock::new(REGISTRY);

/// Append `descriptor` to the registry. Its name must be new (checked by
/// the caller, [`custom::load_dir`]).
pub(crate) fn register_runtime(descriptor: &'static StyleDescriptor) {
    let mut snapshot = SNAPSHOT.write().unwrap_or_else(|e| e.into_inner());
    let mut all: Vec<&'static StyleDescriptor> = snapshot.to_vec();
    all.push(descriptor);
    *snapshot = Box::leak(all.into_boxed_slice());
}

/// Every registered style, in chain order: the built-ins, then user
/// styles in the order they were loaded.
pub fn registry() -> &'static [&'static StyleDescriptor] {
    *SNAPSHOT.read().unwrap_or_else(|e| e.into_inner())
}

/// Whether `name` is one of the styles compiled into the engine.
pub fn is_builtin(name: &str) -> bool {
    REGISTRY.iter().any(|d| d.name == name)
}

/// Whether any style on this frame reads the entity style mask.
pub fn any_uses_mask(config: &PostProcessConfig, ids: &StyleIds) -> bool {
    config.enabled && registry().iter().any(|d| d.reads_mask(config, ids))
}

/// The style called `name`.
pub fn find(name: &str) -> Option<&'static StyleDescriptor> {
    registry().iter().copied().find(|d| d.name == name)
}

/// The registered names, for messages.
pub fn names() -> Vec<&'static str> {
    registry().iter().map(|d| d.name).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_names_are_unique_and_lowercase() {
        let names = names();
        for (i, a) in names.iter().enumerate() {
            assert!(a.chars().all(|c| c.is_ascii_lowercase() || c == '_'), "{a}");
            assert!(!names[i + 1..].contains(a), "{a} registered twice");
        }
        assert!(names.contains(&"kuwahara") && names.contains(&"invert"));
    }

    #[test]
    fn param_defaults_sit_in_their_ranges() {
        for d in registry() {
            for p in d.params {
                assert!(
                    p.min <= p.default && p.default <= p.max,
                    "{}.{}",
                    d.name,
                    p.name
                );
            }
        }
    }

    #[test]
    fn hints_name_real_settings() {
        for d in registry() {
            let rows = d.controls();
            for h in d.hints {
                assert!(
                    rows.iter().any(|r| r.name() == h.name),
                    "{}: hint '{}' names no option, parameter or colour",
                    d.name,
                    h.name
                );
            }
        }
    }

    #[test]
    fn controls_fold_rgb_triples_and_keep_every_setting() {
        let ascii = find("ascii").unwrap();
        let rows = ascii.controls();
        // Options first
        let first_param = rows
            .iter()
            .position(|r| !matches!(r, StyleControl::Option(_)))
            .unwrap();
        assert_eq!(first_param, ascii.options.len());
        // tint_r/g/b became one colour row
        let colors: Vec<_> = rows
            .iter()
            .filter_map(|r| match r {
                StyleControl::Color { base, channels } => Some((*base, channels.map(|c| c.name))),
                _ => None,
            })
            .collect();
        assert_eq!(colors, vec![("tint", ["tint_r", "tint_g", "tint_b"])]);
        // Every param is reachable exactly once
        for d in registry() {
            let mut seen: Vec<&str> = Vec::new();
            for r in d.controls() {
                match r {
                    StyleControl::Param(p) => seen.push(p.name),
                    StyleControl::Color { channels, .. } => {
                        seen.extend(channels.iter().map(|c| c.name))
                    }
                    StyleControl::Option(_) => {}
                }
            }
            seen.sort();
            let mut all: Vec<&str> = d.params.iter().map(|p| p.name).collect();
            all.sort();
            assert_eq!(seen, all, "{}", d.name);
        }
    }

    #[test]
    fn labels_come_from_hints_or_the_name() {
        let ascii = find("ascii").unwrap();
        assert_eq!(ascii.label_for("cell"), "Cell size (pt)");
        assert!(ascii.hint("cell").unwrap().integer);
        assert_eq!(humanize("tint_r"), "Tint r");
        assert_eq!(humanize(""), "");
    }

    #[test]
    fn style_toml_writes_only_changes_and_reads_back() {
        let ascii = find("ascii").unwrap();
        let mut config = PostProcessConfig::default();
        config.styles.set_enabled("ascii", true);
        assert_eq!(style_toml(ascii, &config), "[post_process.styles.ascii]\n");
        config.styles.set_param("ascii", "cell", 12.0);
        config.styles.set_param("ascii", "glow", 0.35);
        config.styles.set_param("ascii", "tint_r", 1.0);
        config.styles.set_option("ascii", "color", "palette");
        config.styles.set_option("ascii", "font", "builtin"); // the default
        let text = style_toml(ascii, &config);
        assert_eq!(
            text,
            "[post_process.styles.ascii]\ncolor = \"palette\"\ncell = 12\ntint_r = 1.0\nglow = 0.35\n"
        );
        // It is the scene's syntax: parse it the way a scene is read
        let doc: toml::Table = text.parse().unwrap();
        let tables: BTreeMap<String, toml::Table> = doc["post_process"]["styles"]
            .as_table()
            .unwrap()
            .iter()
            .map(|(k, v)| (k.clone(), v.as_table().unwrap().clone()))
            .collect();
        let (read, warnings) = StyleSettings::from_tables(&tables);
        assert!(warnings.is_empty(), "{warnings:?}");
        let back = PostProcessConfig {
            styles: read,
            ..Default::default()
        };
        for p in ascii.params {
            assert_eq!(
                ascii.param(&back, p.name),
                ascii.param(&config, p.name),
                "{}",
                p.name
            );
        }
        for o in ascii.options {
            assert_eq!(
                ascii.option(&back, o.name),
                ascii.option(&config, o.name),
                "{}",
                o.name
            );
        }
        // An off style says so
        config.styles.set_enabled("ascii", false);
        assert!(style_toml(ascii, &config).contains("enabled = false\n"));
    }

    #[test]
    fn reset_restore_and_compare() {
        let mut live = StyleSettings::default();
        live.set_enabled("ascii", true);
        live.set_param("ascii", "cell", 16.0);
        live.set_option("ascii", "ramp", "blocks");
        live.reset("ascii");
        assert!(live.is_enabled("ascii"));
        assert_eq!(live.param("ascii", "cell"), None);
        assert_eq!(live.option("ascii", "ramp"), None);

        let mut authored = StyleSettings::default();
        authored.set_enabled("ascii", true);
        authored.set_param("ascii", "cell", 10.0);
        assert!(!live.same_style("ascii", &authored));
        live.set_param("ascii", "glow", 1.0);
        live.restore("ascii", &authored);
        assert!(live.same_style("ascii", &authored));
        assert_eq!(live.param("ascii", "glow"), None);
        // Restoring a style the scene never set removes it
        live.set_enabled("invert", true);
        live.restore("invert", &authored);
        assert!(!live.is_enabled("invert"));
        assert!(live.same_style("invert", &StyleSettings::default()));
    }

    #[test]
    fn plan_ping_pongs_and_ends_where_asked() {
        use Slot::*;
        assert_eq!(plan(0, true), vec![]);
        assert_eq!(plan(1, false), vec![(Input, Ping(0))]);
        assert_eq!(plan(1, true), vec![(Input, Final)]);
        assert_eq!(
            plan(3, false),
            vec![(Input, Ping(0)), (Ping(0), Ping(1)), (Ping(1), Ping(0))]
        );
        assert_eq!(
            plan(3, true),
            vec![(Input, Ping(0)), (Ping(0), Ping(1)), (Ping(1), Final)]
        );
        // No step reads the view it writes
        for n in 0..6 {
            for to_final in [false, true] {
                assert!(plan(n, to_final).iter().all(|(s, d)| s != d));
            }
        }
    }

    #[test]
    fn tables_switch_styles_on_and_warn_on_unknowns() {
        let tables: BTreeMap<String, toml::Table> = toml::from_str(
            r#"
            [invert]
            strength = 0.5
            [nope]
            x = 1
            "#,
        )
        .unwrap();
        let (settings, warnings) = StyleSettings::from_tables(&tables);
        assert!(settings.is_enabled("invert"));
        assert_eq!(settings.param("invert", "strength"), Some(0.5));
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("unknown style 'nope'"), "{warnings:?}");

        let tables: BTreeMap<String, toml::Table> =
            toml::from_str("[invert]\nenabled = false\nstrength = 2\nwobble = 3\n").unwrap();
        let (settings, warnings) = StyleSettings::from_tables(&tables);
        assert!(!settings.is_enabled("invert"));
        assert_eq!(
            settings.param("invert", "strength"),
            Some(2.0),
            "integers read as numbers"
        );
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(warnings[0].contains("no parameter 'wobble'"));
    }

    #[test]
    fn tables_read_options_as_strings() {
        let tables: BTreeMap<String, toml::Table> =
            toml::from_str("[ascii]\ncell = 12\nramp = \"blocks\"\ncolor = \"tint\"\nfont = 3\n")
                .unwrap();
        let (settings, warnings) = StyleSettings::from_tables(&tables);
        assert!(settings.is_enabled("ascii"));
        assert_eq!(settings.param("ascii", "cell"), Some(12.0));
        assert_eq!(settings.option("ascii", "ramp"), Some("blocks"));
        assert_eq!(settings.option("ascii", "color"), Some("tint"));
        assert_eq!(warnings, vec!["styles.ascii.font must be a string"]);

        let ascii = find("ascii").unwrap();
        let mut config = PostProcessConfig::default();
        assert_eq!(ascii.option(&config, "font"), "builtin");
        assert_eq!(ascii.option(&config, "color"), "source");
        config.styles = settings;
        assert_eq!(ascii.option(&config, "ramp"), "blocks");
        assert_eq!(ascii.option(&config, "nope"), "");

        // An unknown key names the options too
        let tables: BTreeMap<String, toml::Table> =
            toml::from_str("[ascii]\nglyphs = 1\n").unwrap();
        let (_, warnings) = StyleSettings::from_tables(&tables);
        assert!(warnings[0].contains("ramp"), "{warnings:?}");
    }

    #[test]
    fn descriptor_params_fall_back_to_defaults() {
        let invert = find("invert").unwrap();
        let mut config = PostProcessConfig::default();
        assert!(!invert.is_enabled(&config));
        assert_eq!(invert.param(&config, "strength"), 1.0);
        config.styles.set_enabled("invert", true);
        config.styles.set_param("invert", "strength", 0.25);
        assert!(invert.is_enabled(&config));
        assert_eq!(invert.param(&config, "strength"), 0.25);

        let kuwahara = find("kuwahara").unwrap();
        assert!(!kuwahara.uses_style_table());
        config.kuwahara_enabled = true;
        assert!(kuwahara.is_enabled(&config));
    }
}
