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
//!   -> style mask (R16Uint ids, while a style reads it; #263)
//!   -> Hdr styles (Kuwahara)                 linear Rgba16Float, scene resolution
//!   -> [ASCII spike, #261]
//!   -> SSAO, volumetric, bloom (read the scene depth / original HDR)
//!   -> composite: fog, DoF, bloom add, tonemap, grade, vignette, grain, render modes
//!   -> Display styles (ASCII, invert)        surface format, output resolution
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
//! An `Hdr` style is shaded by what follows it: fog fades it, bloom haloes
//! its bright parts, the grade tints it (Kuwahara's painterly look wants
//! that). A `Display` style sees the finished picture at output
//! resolution, after the render-scale upscale, which is where the ASCII
//! spike found glyphs have to be drawn to stay crisp (#261, finding 7).
//! SSAO and bloom read the unstylized HDR; that is deliberate for now (the
//! light comes from the scene, not the brush strokes).
//!
//! [`BindCache`]: crate::bind_cache::BindCache

pub mod kit;
mod spec;
mod targets;

pub use spec::{apply_style_spec, style_spec_help, StyleSpec};
pub use targets::{StyleTargets, StyleTexture};

use crate::bind_cache::BindCache;
use crate::postprocess::PostProcessConfig;
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

/// What the framework needs to build a style.
#[derive(Clone, Copy)]
pub struct StyleDevice<'a> {
    pub device: &'a wgpu::Device,
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
    ) -> Vec<&'static StyleDescriptor> {
        let registry = registry();
        self.built.resize_with(registry.len(), || None);
        self.failed.resize(registry.len(), false);
        let mut failed = Vec::new();
        for (i, descriptor) in registry.iter().enumerate() {
            if self.built[i].is_some() || self.failed[i] || !descriptor.is_enabled(config) {
                continue;
            }
            let format = match descriptor.stage {
                StyleStage::Hdr => crate::postprocess::HDR_FORMAT,
                StyleStage::Display => surface_format,
            };
            let made = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                (descriptor.create)(&StyleDevice { device, format })
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

    /// The built styles of `stage` that `config` has on, in chain order.
    pub fn active(
        &self,
        stage: StyleStage,
        config: &PostProcessConfig,
    ) -> Vec<(&'static StyleDescriptor, &dyn StylePass)> {
        if !config.enabled {
            return Vec::new();
        }
        registry()
            .iter()
            .zip(&self.built)
            .filter(|(d, _)| d.stage == stage && d.is_enabled(config))
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

register_styles!(kuwahara, ascii, invert, mask_view);

/// Every registered style, in chain order.
pub fn registry() -> &'static [&'static StyleDescriptor] {
    REGISTRY
}

/// Whether any style `config` has on reads the entity style mask.
pub fn any_uses_mask(config: &PostProcessConfig) -> bool {
    config.enabled && REGISTRY.iter().any(|d| d.uses_mask && d.is_enabled(config))
}

/// The style called `name`.
pub fn find(name: &str) -> Option<&'static StyleDescriptor> {
    REGISTRY.iter().copied().find(|d| d.name == name)
}

/// The registered names, for messages.
pub fn names() -> Vec<&'static str> {
    REGISTRY.iter().map(|d| d.name).collect()
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
