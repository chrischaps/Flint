//! The editor's selection outline (#602): what `flint edit` draws round the
//! entity you picked.
//!
//! It is drawn in two steps, neither of which touches the scene:
//!
//! 1. **The mask.** After the scene pass, every draw of the selected entity
//!    and of everything under it in the hierarchy (a mesh-less parent's
//!    children, a glTF's expanded nodes) is drawn again, with its own cached
//!    bind groups, into an `Rg8Unorm` target at scene resolution: `.r` where
//!    it covers the pixel, `.g` where it is also the nearest surface,
//!    tested in the shader against the frame's single-sample depth. Static,
//!    woozy, skinned, wireframe, transparent, terrain and billboard draws
//!    each have a pipeline, so the mask follows what the scene drew,
//!    pose, drift and sprite cut-out included.
//! 2. **The outline.** Over the finished image (after tonemapping, bloom,
//!    fog and every style, before world lines), one full-screen pass inks a
//!    band [`OUTLINE_WIDTH_POINTS`] wide round the part you can see and a
//!    fainter one ([`HIDDEN_ALPHA`]) round what is hidden behind something.
//!
//! It replaced an inverted hull (front faces culled, back faces pushed out
//! in clip space) that wrote depth and colour into the HDR scene. That one
//! crashed with no terrain bound (its pipeline wanted a material group it
//! was never given), was tonemapped from orange to yellow, filled glass and
//! sprites solid, broke at hard corners, leaked into skinned figures, missed
//! parents and multi-node models, and was 1.8x wider across than down.
//! The captures are in the PR for #602.
//!
//! With nothing selected nothing here runs, and the frame is the frame
//! without it, byte for byte.

use crate::TrustedShader;
use bytemuck::{Pod, Zeroable};

/// The selection colour, in display (sRGB) values: what you see on screen.
/// The one place it is defined.
pub const SELECTION_COLOR_SRGB: [u8; 3] = [0xFF, 0xC8, 0x1E];

/// The outline's width, in points (times the host's pixels per point).
pub const OUTLINE_WIDTH_POINTS: f32 = 2.0;

/// The most mask texels the outline searches either way (#945); about 4 at
/// a scale factor of 1.
const MAX_REACH: i32 = 64;

/// How strongly the outline of a hidden part of the selection is drawn.
pub const HIDDEN_ALPHA: f32 = 0.4;

/// The mask's format: `.r` covered, `.g` visible.
pub const SELECTION_MASK_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rg8Unorm;

/// The selection colour as the outline pass writes it into a target of
/// `format`: linear for an sRGB target (the hardware encodes it back),
/// display values for a plain one.
pub fn selection_color(format: wgpu::TextureFormat) -> [f32; 4] {
    let c = SELECTION_COLOR_SRGB.map(|v| v as f32 / 255.0);
    let c = if format.is_srgb() {
        c.map(srgb_to_linear)
    } else {
        c
    };
    [c[0], c[1], c[2], 1.0]
}

fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
struct OutlineUniforms {
    color: [f32; 4],
    mask_per_px: [f32; 2],
    width: f32,
    hidden_alpha: f32,
    mask_size: [i32; 2],
    reach: i32,
    _pad: i32,
}

/// The layouts the mask's pipelines share with the scene's, so a draw's
/// cached bind groups bind as they are.
pub(crate) struct OutlineLayouts<'a> {
    pub transform: &'a wgpu::BindGroupLayout,
    pub bones: Option<&'a wgpu::BindGroupLayout>,
    pub billboard: Option<(&'a wgpu::BindGroupLayout, &'a wgpu::BindGroupLayout)>,
}

/// The mask's pipelines, one a kind of draw.
pub(crate) struct MaskPipelines {
    pub static_tris: wgpu::RenderPipeline,
    pub static_lines: wgpu::RenderPipeline,
    pub woozy_tris: wgpu::RenderPipeline,
    pub skinned: Option<wgpu::RenderPipeline>,
    pub woozy_skinned: Option<wgpu::RenderPipeline>,
    pub billboard: Option<wgpu::RenderPipeline>,
}

/// Everything the outline keeps between frames: pipelines, made the first
/// time something is selected, and the mask at the scene's size.
pub(crate) struct SelectionOutline {
    pub mask: MaskPipelines,
    /// Group 1 of the mesh pipelines (group 2 of the billboard's): the
    /// scene's depth.
    pub depth_bgl: wgpu::BindGroupLayout,
    outline_pipeline: wgpu::RenderPipeline,
    outline_bgl: wgpu::BindGroupLayout,
    outline_format: wgpu::TextureFormat,
    uniforms: wgpu::Buffer,
    target: Option<MaskTarget>,
    /// How many draws the last frame's mask drew; 0 skips the outline.
    pub last_draws: std::cell::Cell<usize>,
}

struct MaskTarget {
    view: wgpu::TextureView,
    size: (u32, u32),
    outline_bind: wgpu::BindGroup,
}

fn module(device: &wgpu::Device, label: &str, source: &str) -> wgpu::ShaderModule {
    device.create_trusted_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some(label),
        source: wgpu::ShaderSource::Wgsl(source.into()),
    })
}

/// The mask shader's source; `woozy` prefixes the wooze noise and turns on
/// the drift lines.
pub(crate) fn mask_source(woozy: bool) -> String {
    let base = include_str!("selection_mask.wgsl");
    if woozy {
        format!(
            "{}\n{}",
            include_str!("wooze_noise.wgsl"),
            base.replace("//FLINT_WOOZE ", "")
        )
    } else {
        base.to_string()
    }
}

/// The mask's blend: each channel keeps the most any draw wrote.
const MAX_BLEND: wgpu::BlendState = wgpu::BlendState {
    color: wgpu::BlendComponent {
        src_factor: wgpu::BlendFactor::One,
        dst_factor: wgpu::BlendFactor::One,
        operation: wgpu::BlendOperation::Max,
    },
    alpha: wgpu::BlendComponent {
        src_factor: wgpu::BlendFactor::One,
        dst_factor: wgpu::BlendFactor::One,
        operation: wgpu::BlendOperation::Max,
    },
};

#[allow(clippy::too_many_arguments)]
fn mask_pipeline(
    device: &wgpu::Device,
    shader: &wgpu::ShaderModule,
    label: &str,
    vs: &str,
    layouts: &[&wgpu::BindGroupLayout],
    buffers: &[wgpu::VertexBufferLayout<'static>],
    topology: wgpu::PrimitiveTopology,
) -> wgpu::RenderPipeline {
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some(label),
        bind_group_layouts: &layouts.iter().copied().map(Some).collect::<Vec<_>>(),
        immediate_size: 0,
    });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(label),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some(vs),
            buffers: &buffers.iter().cloned().map(Some).collect::<Vec<_>>(),
            compilation_options: Default::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some("fs_mask"),
            targets: &[Some(wgpu::ColorTargetState {
                format: SELECTION_MASK_FORMAT,
                blend: Some(MAX_BLEND),
                write_mask: wgpu::ColorWrites::RED | wgpu::ColorWrites::GREEN,
            })],
            compilation_options: Default::default(),
        }),
        primitive: wgpu::PrimitiveState {
            topology,
            // Both faces: an open mesh, a plane or a card seen from behind
            // is still the selection
            cull_mode: None,
            ..Default::default()
        },
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        multiview_mask: None,
        cache: None,
    })
}

impl SelectionOutline {
    pub fn new(
        device: &wgpu::Device,
        layouts: &OutlineLayouts,
        outline_format: wgpu::TextureFormat,
    ) -> Self {
        use crate::primitives::{SkinnedVertex, Vertex};
        let depth_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Selection Mask Depth"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Depth,
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            }],
        });
        let plain = module(device, "Selection Mask", &mask_source(false));
        let woozy = module(device, "Selection Mask (woozy)", &mask_source(true));
        let tris = wgpu::PrimitiveTopology::TriangleList;
        let static_layouts = [layouts.transform, &depth_bgl];
        let static_tris = mask_pipeline(
            device,
            &plain,
            "Selection Mask Static",
            "vs_static",
            &static_layouts,
            &[Vertex::desc()],
            tris,
        );
        let static_lines = mask_pipeline(
            device,
            &plain,
            "Selection Mask Lines",
            "vs_static",
            &static_layouts,
            &[Vertex::desc()],
            wgpu::PrimitiveTopology::LineList,
        );
        let woozy_tris = mask_pipeline(
            device,
            &woozy,
            "Selection Mask Woozy",
            "vs_static",
            &static_layouts,
            &[Vertex::desc()],
            tris,
        );
        let (skinned, woozy_skinned) = match layouts.bones {
            Some(bones) => {
                let skinned_layouts = [layouts.transform, &depth_bgl, bones];
                (
                    Some(mask_pipeline(
                        device,
                        &plain,
                        "Selection Mask Skinned",
                        "vs_skinned",
                        &skinned_layouts,
                        &[SkinnedVertex::desc()],
                        tris,
                    )),
                    Some(mask_pipeline(
                        device,
                        &woozy,
                        "Selection Mask Woozy Skinned",
                        "vs_skinned",
                        &skinned_layouts,
                        &[SkinnedVertex::desc()],
                        tris,
                    )),
                )
            }
            None => (None, None),
        };
        let billboard = layouts.billboard.map(|(uniforms, texture)| {
            let shader = module(
                device,
                "Selection Mask Billboard",
                include_str!("selection_mask_billboard.wgsl"),
            );
            mask_pipeline(
                device,
                &shader,
                "Selection Mask Billboard",
                "vs_billboard",
                &[uniforms, texture, &depth_bgl],
                &[],
                tris,
            )
        });

        let outline_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Selection Outline"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
            ],
        });
        let outline_shader = module(
            device,
            "Selection Outline",
            include_str!("selection_outline.wgsl"),
        );
        let outline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Selection Outline"),
            bind_group_layouts: &[Some(&outline_bgl)],
            immediate_size: 0,
        });
        let outline_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Selection Outline"),
            layout: Some(&outline_layout),
            vertex: wgpu::VertexState {
                module: &outline_shader,
                entry_point: Some("vs_fullscreen"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &outline_shader,
                entry_point: Some("fs_outline"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: outline_format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });
        let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Selection Outline Uniforms"),
            size: std::mem::size_of::<OutlineUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Self {
            mask: MaskPipelines {
                static_tris,
                static_lines,
                woozy_tris,
                skinned,
                woozy_skinned,
                billboard,
            },
            depth_bgl,
            outline_pipeline,
            outline_bgl,
            outline_format,
            uniforms,
            target: None,
            last_draws: std::cell::Cell::new(0),
        }
    }

    /// The mask's view, once [`ensure_mask`](Self::ensure_mask) made it.
    pub fn mask_view(&self) -> &wgpu::TextureView {
        &self.target.as_ref().expect("ensure_mask first").view
    }

    /// Make the mask at `size` (with the outline's bind group over it) the
    /// first frame at that size.
    pub fn ensure_mask(&mut self, device: &wgpu::Device, size: (u32, u32)) {
        if self.target.as_ref().is_none_or(|t| t.size != size) {
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("Selection Mask"),
                size: wgpu::Extent3d {
                    width: size.0.max(1),
                    height: size.1.max(1),
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: SELECTION_MASK_FORMAT,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            });
            let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
            let outline_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("Selection Outline"),
                layout: &self.outline_bgl,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: self.uniforms.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(&view),
                    },
                ],
            });
            self.target = Some(MaskTarget {
                view,
                size,
                outline_bind,
            });
        }
    }

    /// Ink the outline over `target` (the finished image, `output` pixels
    /// in size) from the mask the frame drew. `pixels_per_point` sizes it.
    pub fn encode_outline(
        &self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        output: (u32, u32),
        pixels_per_point: f32,
    ) {
        let Some(mask) = &self.target else {
            return;
        };
        if self.last_draws.get() == 0 || output.0 == 0 || output.1 == 0 {
            return;
        }
        let mask_per_px = [
            mask.size.0 as f32 / output.0 as f32,
            mask.size.1 as f32 / output.1 as f32,
        ];
        let width = OUTLINE_WIDTH_POINTS * pixels_per_point.max(0.25);
        // At most MAX_REACH: the shader loops `-reach..=reach` with loop
        // bounding off (built-in WGSL is trusted, #945), so an absurd scale
        // factor must not take it near i32::MAX, where it would never end
        let reach = (((width + 1.5) * mask_per_px[0].max(mask_per_px[1])).ceil() as i32)
            .clamp(0, MAX_REACH);
        let uniforms = OutlineUniforms {
            color: selection_color(self.outline_format),
            mask_per_px,
            width,
            hidden_alpha: HIDDEN_ALPHA,
            mask_size: [mask.size.0 as i32, mask.size.1 as i32],
            reach,
            _pad: 0,
        };
        queue.write_buffer(&self.uniforms, 0, bytemuck::bytes_of(&uniforms));
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Selection Outline"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(&self.outline_pipeline);
        pass.set_bind_group(0, &mask.outline_bind, &[]);
        pass.draw(0..3, 0..1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shaders_parse() {
        for (name, source) in [
            ("selection_mask", mask_source(false)),
            ("selection_mask (woozy)", mask_source(true)),
            (
                "selection_mask_billboard",
                include_str!("selection_mask_billboard.wgsl").to_string(),
            ),
            (
                "selection_outline",
                include_str!("selection_outline.wgsl").to_string(),
            ),
        ] {
            let module = naga::front::wgsl::parse_str(&source)
                .unwrap_or_else(|e| panic!("{name}: {}", e.emit_to_string(&source)));
            naga::valid::Validator::new(
                naga::valid::ValidationFlags::all(),
                naga::valid::Capabilities::all(),
            )
            .validate(&module)
            .unwrap_or_else(|e| panic!("{name}: {e:?}"));
        }
    }

    #[test]
    fn the_colour_on_screen_is_the_constant() {
        // A plain target takes display values as they are
        let plain = selection_color(wgpu::TextureFormat::Rgba8Unorm);
        assert_eq!(
            [plain[0], plain[1], plain[2]].map(|c| (c * 255.0).round() as u8),
            SELECTION_COLOR_SRGB
        );
        // An sRGB target takes linear values that encode back to them
        let srgb = selection_color(wgpu::TextureFormat::Rgba8UnormSrgb);
        for (lin, want) in [srgb[0], srgb[1], srgb[2]].iter().zip(SELECTION_COLOR_SRGB) {
            let back = if *lin <= 0.0031308 {
                lin * 12.92
            } else {
                1.055 * lin.powf(1.0 / 2.4) - 0.055
            };
            assert_eq!((back * 255.0).round() as u8, want);
        }
    }
}
