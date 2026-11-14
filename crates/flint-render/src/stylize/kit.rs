//! The building blocks most styles are made of: a full-screen triangle
//! pipeline, the two bind group shapes the post chain uses, a uniform
//! buffer and the pass that draws one triangle. With these a style is
//! mostly its shader and its uniform struct.

use super::dev_shader;
pub(crate) use super::dev_shader::wgsl;
pub use super::dev_shader::Wgsl;
use crate::trusted_shader::TrustedShader as _;

/// A fragment-stage uniform buffer at binding 0.
pub fn uniform_layout(device: &wgpu::Device, label: &str) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some(label),
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        }],
    })
}

/// A filterable float texture at binding 0 and a filtering sampler at
/// binding 1 (bind it with [`StyleFrame::sampled`](super::StyleFrame::sampled)).
pub fn sampled_layout(device: &wgpu::Device, label: &str) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some(label),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
        ],
    })
}

/// A float texture at binding 0 with no sampler, read with `textureLoad`:
/// the shared edges (bind them with
/// [`StyleFrame::edges_bind`](super::StyleFrame::edges_bind), #301).
pub fn texture_layout(device: &wgpu::Device, label: &str) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some(label),
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        }],
    })
}

/// The entity style mask at binding 0: a `texture_2d<u32>` read with
/// `textureLoad` (bind it with [`StyleFrame::mask_bind`](super::StyleFrame::mask_bind)).
pub fn mask_layout(device: &wgpu::Device, label: &str) -> wgpu::BindGroupLayout {
    uint_layout(device, label)
}

/// A `texture_2d<u32>` at binding 0, read with `textureLoad`: the style
/// mask, or a style's own integer state (ASCII's cells, #277).
pub fn uint_layout(device: &wgpu::Device, label: &str) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some(label),
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Uint,
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        }],
    })
}

/// A pipeline drawing one full-screen triangle with `module`'s `vs_main`
/// and `fs_main` into `format`, unblended.
///
/// Returned as the caller's own `wgpu::RenderPipeline`, or as an
/// `Arc<wgpu::RenderPipeline>` that a style's `create` may share with the
/// next renderer on the device while [`crate::pipeline_cache`] is on
/// (#911): the type it is stored as picks which ([`KitPipeline`]). Only a
/// [`shader`]'s module is shared, since its text is the key.
pub fn fullscreen_pipeline<P: KitPipeline>(
    device: &wgpu::Device,
    label: &str,
    module: &impl ShaderText,
    layouts: &[&wgpu::BindGroupLayout],
    format: wgpu::TextureFormat,
) -> P {
    fullscreen_pipeline_with(device, label, module, "fs_main", layouts, format, None)
}

/// [`fullscreen_pipeline`] with the fragment entry point `fragment`, for a
/// module that has more than one.
pub fn fullscreen_pipeline_entry<P: KitPipeline>(
    device: &wgpu::Device,
    label: &str,
    module: &impl ShaderText,
    fragment: &str,
    layouts: &[&wgpu::BindGroupLayout],
    format: wgpu::TextureFormat,
) -> P {
    fullscreen_pipeline_entry_targets(device, label, module, fragment, layouts, &[format], None)
}

/// [`fullscreen_pipeline_entry`] with a blend: a style with several passes
/// in one module (a bloom chain's threshold, downsample, additive upsample
/// and composite, #321).
pub fn fullscreen_pipeline_with<P: KitPipeline>(
    device: &wgpu::Device,
    label: &str,
    module: &impl ShaderText,
    fragment: &str,
    layouts: &[&wgpu::BindGroupLayout],
    format: wgpu::TextureFormat,
    blend: Option<wgpu::BlendState>,
) -> P {
    fullscreen_pipeline_entry_targets(device, label, module, fragment, layouts, &[format], blend)
}

/// [`fullscreen_pipeline`] writing one target per format, `fs_main`'s
/// `@location(i)` into the i-th (#313).
pub fn fullscreen_pipeline_targets<P: KitPipeline>(
    device: &wgpu::Device,
    label: &str,
    module: &impl ShaderText,
    layouts: &[&wgpu::BindGroupLayout],
    formats: &[wgpu::TextureFormat],
) -> P {
    fullscreen_pipeline_entry_targets(device, label, module, "fs_main", layouts, formats, None)
}

/// The full-screen pipelines' one maker: entry point `fragment`, a target
/// per format, each with `blend`.
fn fullscreen_pipeline_entry_targets<P: KitPipeline>(
    device: &wgpu::Device,
    label: &str,
    module: &impl ShaderText,
    fragment: &str,
    layouts: &[&wgpu::BindGroupLayout],
    formats: &[wgpu::TextureFormat],
    blend: Option<wgpu::BlendState>,
) -> P {
    // Everything but the layouts' entries, which the style's scope stands
    // for (`pipeline_cache::in_scope`): one create makes the same ones
    let key = module.text().map(|text| {
        format!(
            "{label}|{}|{fragment}|{formats:?}|{blend:?}|{} layouts",
            crate::pipeline_cache::text_key(text),
            layouts.len()
        )
    });
    P::make(device, label, key, || {
        build_fullscreen(
            device,
            label,
            module.module(),
            fragment,
            layouts,
            formats,
            blend,
        )
    })
}

fn build_fullscreen(
    device: &wgpu::Device,
    label: &str,
    module: &wgpu::ShaderModule,
    fragment: &str,
    layouts: &[&wgpu::BindGroupLayout],
    formats: &[wgpu::TextureFormat],
    blend: Option<wgpu::BlendState>,
) -> wgpu::RenderPipeline {
    let targets: Vec<Option<wgpu::ColorTargetState>> = formats
        .iter()
        .map(|format| {
            Some(wgpu::ColorTargetState {
                format: *format,
                blend,
                write_mask: wgpu::ColorWrites::ALL,
            })
        })
        .collect();
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some(label),
        bind_group_layouts: &layouts.iter().copied().map(Some).collect::<Vec<_>>(),
        immediate_size: 0,
    });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(label),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module,
            entry_point: Some("vs_main"),
            buffers: &[],
            compilation_options: Default::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module,
            entry_point: Some(fragment),
            targets: &targets,
            compilation_options: Default::default(),
        }),
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            ..Default::default()
        },
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        multiview_mask: None,
        cache: None,
    })
}

/// What a kit pipeline function returns (#911): a `wgpu::RenderPipeline`
/// of the caller's own, always built, or an `Arc<wgpu::RenderPipeline>`,
/// which a style's `create` keeps for the next renderer on the device
/// while [`crate::pipeline_cache`] is on. The built-in styles store the
/// `Arc`; a game's style that stores the pipeline itself builds as before.
pub trait KitPipeline: Sized {
    /// Build with `build`, or take the pipeline kept under `key` (the
    /// module's text and the rest; `None` for a module with no text).
    #[doc(hidden)]
    fn make(
        device: &wgpu::Device,
        label: &str,
        key: Option<String>,
        build: impl FnOnce() -> wgpu::RenderPipeline,
    ) -> Self;
}

impl KitPipeline for wgpu::RenderPipeline {
    fn make(
        _device: &wgpu::Device,
        label: &str,
        _key: Option<String>,
        build: impl FnOnce() -> wgpu::RenderPipeline,
    ) -> Self {
        crate::pipeline_cache::built(label);
        build()
    }
}

impl KitPipeline for std::sync::Arc<wgpu::RenderPipeline> {
    fn make(
        device: &wgpu::Device,
        label: &str,
        key: Option<String>,
        build: impl FnOnce() -> wgpu::RenderPipeline,
    ) -> Self {
        match key {
            Some(key) => crate::pipeline_cache::scoped(device, label, key, build),
            None => {
                crate::pipeline_cache::built(label);
                std::sync::Arc::new(build())
            }
        }
    }
}

/// A shader module and the WGSL it was made from: what [`shader`] returns.
/// It derefs to the `wgpu::ShaderModule`; the text is what a shared
/// pipeline is keyed by (#911), so a swapped shader file is a new key.
pub struct Shader {
    module: wgpu::ShaderModule,
    text: std::borrow::Cow<'static, str>,
}

impl Shader {
    /// The WGSL the module was made from.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// The module alone, for a field of type `wgpu::ShaderModule`.
    pub fn into_module(self) -> wgpu::ShaderModule {
        self.module
    }
}

impl std::ops::Deref for Shader {
    type Target = wgpu::ShaderModule;

    fn deref(&self) -> &wgpu::ShaderModule {
        &self.module
    }
}

/// A module a kit pipeline function takes: a [`Shader`], which a shared
/// pipeline is keyed by, or a bare `wgpu::ShaderModule`, never shared.
pub trait ShaderText {
    fn module(&self) -> &wgpu::ShaderModule;
    /// The WGSL the module was made from, when it is known.
    fn text(&self) -> Option<&str>;
}

impl ShaderText for wgpu::ShaderModule {
    fn module(&self) -> &wgpu::ShaderModule {
        self
    }

    fn text(&self) -> Option<&str> {
        None
    }
}

impl ShaderText for Shader {
    fn module(&self) -> &wgpu::ShaderModule {
        &self.module
    }

    fn text(&self) -> Option<&str> {
        Some(&self.text)
    }
}

/// A WGSL module: a [`wgsl!`] (or a bare string). With
/// `FLINT_STYLE_SHADER_DIR` set, natively, a [`wgsl!`]'s files are read
/// from that folder instead and hot-reload (#418, [`dev_shader`]).
pub fn shader(device: &wgpu::Device, label: &str, source: impl Into<Wgsl>) -> Shader {
    let text = dev_shader::source(label, source.into());
    let module = device.create_module(wgpu::ShaderModuleDescriptor {
        label: Some(label),
        source: wgpu::ShaderSource::Wgsl(text.clone()),
    });
    Shader { module, text }
}

/// A uniform buffer the size of `T`, written with `queue.write_buffer`.
pub fn uniform_buffer<T>(device: &wgpu::Device, label: &str) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size: std::mem::size_of::<T>() as u64,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

/// Draw one full-screen triangle into `target`, cleared to black first,
/// with `groups` bound in order.
pub fn fullscreen_pass(
    encoder: &mut wgpu::CommandEncoder,
    label: &str,
    target: &wgpu::TextureView,
    pipeline: &wgpu::RenderPipeline,
    groups: &[&wgpu::BindGroup],
) {
    crate::postprocess::fullscreen_pass(
        encoder,
        label,
        target,
        Some(wgpu::Color::BLACK),
        pipeline,
        groups,
    );
}

/// [`fullscreen_pass`] into several targets at once (a pipeline from
/// [`fullscreen_pipeline_targets`]), each cleared to black first.
pub fn fullscreen_pass_targets(
    encoder: &mut wgpu::CommandEncoder,
    label: &str,
    targets: &[&wgpu::TextureView],
    pipeline: &wgpu::RenderPipeline,
    groups: &[&wgpu::BindGroup],
) {
    let attachments: Vec<Option<wgpu::RenderPassColorAttachment>> = targets
        .iter()
        .map(|view| {
            Some(wgpu::RenderPassColorAttachment {
                view,
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })
        })
        .collect();
    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some(label),
        color_attachments: &attachments,
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    });
    pass.set_pipeline(pipeline);
    for (i, group) in groups.iter().enumerate() {
        pass.set_bind_group(i as u32, *group, &[]);
    }
    pass.draw(0..3, 0..1);
}

/// A rectangle of a target in whole pixels, from the top left: where a
/// scissored pass draws (#385).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PixelRect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl PixelRect {
    /// Nothing.
    pub const EMPTY: Self = Self {
        x: 0,
        y: 0,
        width: 0,
        height: 0,
    };

    /// The whole of a `width` x `height` target.
    pub fn full(width: u32, height: u32) -> Self {
        Self {
            x: 0,
            y: 0,
            width,
            height,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.width == 0 || self.height == 0
    }

    /// Whether the rect is all of a `width` x `height` target.
    pub fn covers(&self, width: u32, height: u32) -> bool {
        self.x == 0 && self.y == 0 && self.width >= width && self.height >= height
    }

    /// The rect clamped to a `width` x `height` target.
    pub fn clamp_to(self, width: u32, height: u32) -> Self {
        let x = self.x.min(width);
        let y = self.y.min(height);
        Self {
            x,
            y,
            width: self.width.min(width - x),
            height: self.height.min(height - y),
        }
    }

    /// The rect grown by `pad` pixels on every side, clamped to the target.
    pub fn padded(self, pad: u32, width: u32, height: u32) -> Self {
        if self.is_empty() {
            return self;
        }
        let x = self.x.saturating_sub(pad);
        let y = self.y.saturating_sub(pad);
        let right = self.x.saturating_add(self.width).saturating_add(pad);
        let bottom = self.y.saturating_add(self.height).saturating_add(pad);
        Self {
            x,
            y,
            width: right - x,
            height: bottom - y,
        }
        .clamp_to(width, height)
    }

    /// The same region of a target `from` pixels across at `to`'s size,
    /// rounded outwards, so every pixel of `to` that overlaps the rect is in.
    pub fn rescaled(self, from: [u32; 2], to: [u32; 2]) -> Self {
        if self.is_empty() || from[0] == 0 || from[1] == 0 {
            return Self::EMPTY;
        }
        let sx = to[0] as f64 / from[0] as f64;
        let sy = to[1] as f64 / from[1] as f64;
        let x0 = (self.x as f64 * sx).floor() as u32;
        let y0 = (self.y as f64 * sy).floor() as u32;
        let x1 = ((self.x + self.width) as f64 * sx).ceil() as u32;
        let y1 = ((self.y + self.height) as f64 * sy).ceil() as u32;
        Self {
            x: x0,
            y: y0,
            width: x1.saturating_sub(x0),
            height: y1.saturating_sub(y0),
        }
        .clamp_to(to[0], to[1])
    }

    /// The smallest rect holding both.
    pub fn union(self, other: Self) -> Self {
        if self.is_empty() {
            return other;
        }
        if other.is_empty() {
            return self;
        }
        let x = self.x.min(other.x);
        let y = self.y.min(other.y);
        let right = (self.x + self.width).max(other.x + other.width);
        let bottom = (self.y + self.height).max(other.y + other.height);
        Self {
            x,
            y,
            width: right - x,
            height: bottom - y,
        }
    }

    /// The parts of a `width` x `height` target outside the rect, as up to
    /// four bands that don't overlap: the rows above and below, then the
    /// left and right of the rect's own rows.
    pub fn outside(&self, width: u32, height: u32) -> Vec<PixelRect> {
        let r = self.clamp_to(width, height);
        if r.is_empty() {
            return vec![Self::full(width, height)];
        }
        let bottom = r.y + r.height;
        let right = r.x + r.width;
        [
            Self {
                x: 0,
                y: 0,
                width,
                height: r.y,
            },
            Self {
                x: 0,
                y: bottom,
                width,
                height: height - bottom,
            },
            Self {
                x: 0,
                y: r.y,
                width: r.x,
                height: r.height,
            },
            Self {
                x: right,
                y: r.y,
                width: width - right,
                height: r.height,
            },
        ]
        .into_iter()
        .filter(|b| !b.is_empty())
        .collect()
    }
}

/// The scissored pass's copy (#385): the input through unchanged, for a
/// target of one format. Make it in the style's `create` over the style's
/// own input layout (a [`sampled_layout`]), so the style's input bind
/// group binds it.
pub struct Blit {
    pipeline: std::sync::Arc<wgpu::RenderPipeline>,
}

impl Blit {
    pub fn new(
        device: &wgpu::Device,
        label: &str,
        input_layout: &wgpu::BindGroupLayout,
        format: wgpu::TextureFormat,
    ) -> Self {
        let module = shader(device, label, BLIT_SHADER);
        Self {
            pipeline: fullscreen_pipeline(device, label, &module, &[input_layout], format),
        }
    }
}

/// The blit's WGSL.
pub const BLIT_SHADER: Wgsl = wgsl!("blit.wgsl");

/// [`fullscreen_pass`] that draws the style only inside `rect` (#385):
/// the rest of `target` is the input, copied by `blit` with `input` (the
/// style's own input bind group) in up to four bands around the rect, so
/// every pixel is written once. With `rect` `None` (or covering the
/// target) it is `fullscreen_pass` exactly; an empty rect copies the
/// whole input and draws nothing.
///
/// The output is the same as the full pass's only when the style returns
/// its input pixel (read with `textureLoad` at the pixel) everywhere
/// outside the rect: [`StyleFrame::styled_rect`](super::StyleFrame::styled_rect)
/// is made to hold that for a per-entity style padded by its reach.
#[allow(clippy::too_many_arguments)]
pub fn scissored_pass(
    encoder: &mut wgpu::CommandEncoder,
    label: &str,
    target: &wgpu::TextureView,
    size: [u32; 2],
    pipeline: &wgpu::RenderPipeline,
    groups: &[&wgpu::BindGroup],
    rect: Option<PixelRect>,
    blit: &Blit,
    input: &wgpu::BindGroup,
) {
    let [width, height] = size;
    let rect = match rect.map(|r| r.clamp_to(width, height)) {
        Some(r) if !r.covers(width, height) => r,
        _ => return fullscreen_pass(encoder, label, target, pipeline, groups),
    };
    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some(label),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: target,
            resolve_target: None,
            depth_slice: None,
            ops: wgpu::Operations {
                load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                store: wgpu::StoreOp::Store,
            },
        })],
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    });
    pass.set_pipeline(&blit.pipeline);
    pass.set_bind_group(0, input, &[]);
    for band in rect.outside(width, height) {
        pass.set_scissor_rect(band.x, band.y, band.width, band.height);
        pass.draw(0..3, 0..1);
    }
    if rect.is_empty() {
        return;
    }
    pass.set_scissor_rect(rect.x, rect.y, rect.width, rect.height);
    pass.set_pipeline(pipeline);
    for (i, group) in groups.iter().enumerate() {
        pass.set_bind_group(i as u32, *group, &[]);
    }
    pass.draw(0..3, 0..1);
}

/// [`fullscreen_pass`] drawn only inside `rect` (#385), for a pass whose
/// result is read only there (a prep pass): the rest of `target` stays
/// cleared to black. `None` draws everything.
pub fn fullscreen_pass_in(
    encoder: &mut wgpu::CommandEncoder,
    label: &str,
    target: &wgpu::TextureView,
    size: [u32; 2],
    pipeline: &wgpu::RenderPipeline,
    groups: &[&wgpu::BindGroup],
    rect: Option<PixelRect>,
) {
    let rect = match rect.map(|r| r.clamp_to(size[0], size[1])) {
        Some(r) if !r.covers(size[0], size[1]) => r,
        _ => return fullscreen_pass(encoder, label, target, pipeline, groups),
    };
    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some(label),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: target,
            resolve_target: None,
            depth_slice: None,
            ops: wgpu::Operations {
                load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                store: wgpu::StoreOp::Store,
            },
        })],
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    });
    if rect.is_empty() {
        return;
    }
    pass.set_scissor_rect(rect.x, rect.y, rect.width, rect.height);
    pass.set_pipeline(pipeline);
    for (i, group) in groups.iter().enumerate() {
        pass.set_bind_group(i as u32, *group, &[]);
    }
    pass.draw(0..3, 0..1);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(x: u32, y: u32, width: u32, height: u32) -> PixelRect {
        PixelRect {
            x,
            y,
            width,
            height,
        }
    }

    #[test]
    fn the_bands_outside_a_rect_tile_the_rest_of_the_target_once() {
        let (w, h) = (37, 23);
        let rects = [
            r(5, 4, 10, 7),
            r(0, 0, 10, 7),
            r(30, 20, 7, 3),
            r(0, 0, 37, 23),
            r(3, 3, 0, 0),
        ];
        for rect in rects {
            let mut hits = vec![0u8; (w * h) as usize];
            let mut paint = |b: PixelRect| {
                for y in b.y..b.y + b.height {
                    for x in b.x..b.x + b.width {
                        hits[(y * w + x) as usize] += 1;
                    }
                }
            };
            for band in rect.outside(w, h) {
                paint(band);
            }
            if !rect.is_empty() {
                paint(rect);
            }
            assert!(hits.iter().all(|&n| n == 1), "{rect:?}");
        }
    }

    #[test]
    fn padding_rescaling_and_union_round_outwards_and_stay_on_the_target() {
        assert_eq!(r(5, 5, 4, 4).padded(3, 100, 100), r(2, 2, 10, 10));
        assert_eq!(r(1, 1, 4, 4).padded(3, 6, 100), r(0, 0, 6, 8));
        assert!(PixelRect::EMPTY.padded(3, 100, 100).is_empty());
        // Half size to full: a pixel becomes two
        assert_eq!(r(3, 2, 1, 1).rescaled([10, 10], [20, 20]), r(6, 4, 2, 2));
        // Full to a third: anything touching a coarse pixel takes it whole
        assert_eq!(r(4, 4, 1, 1).rescaled([30, 30], [10, 10]), r(1, 1, 1, 1));
        assert_eq!(r(5, 5, 2, 2).rescaled([30, 30], [10, 10]), r(1, 1, 2, 2));
        assert_eq!(r(1, 1, 2, 2).union(r(5, 0, 1, 1)), r(1, 0, 5, 3));
        assert_eq!(PixelRect::EMPTY.union(r(5, 0, 1, 1)), r(5, 0, 1, 1));
    }

    #[test]
    fn the_blit_shader_validates() {
        let module = naga::front::wgsl::parse_str(BLIT_SHADER.text).expect("parses");
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::default(),
        )
        .validate(&module)
        .expect("validates");
    }
}
