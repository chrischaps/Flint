//! ASCII spike (#261): two throwaway prototypes of an ASCII look, so the
//! stylize epic (#260) can choose its architecture by eye.
//!
//! - **Full screen**: a glyph pass over the whole HDR image, after Kuwahara
//!   and before the composite, on one screen grid.
//! - **Per entity**: a mask pass redraws the chosen entities into a side
//!   target (style id + anchor), then the glyph pass glyphizes only masked
//!   pixels, on one screen grid or on a grid anchored at each entity's
//!   projected origin.
//!
//! Off by default and absent from every frame unless a host sets a mode
//! with [`SceneRenderer::set_ascii_spike`](crate::SceneRenderer::set_ascii_spike).
//! `flint render --ascii full|entity|entity-object` drives it. What it
//! found is in the Vault note and PR for #261; the real passes are #262-#266.

use bytemuck::{Pod, Zeroable};

/// Which prototype draws.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AsciiSpikeMode {
    #[default]
    Off,
    /// Every pixel, one screen grid.
    FullScreen,
    /// Masked entities only, one screen grid.
    EntityScreenGrid,
    /// Masked entities only, each on a grid anchored at its projected origin.
    EntityObjectGrid,
}

impl AsciiSpikeMode {
    /// From the CLI's spelling: `full`, `entity`, `entity-object`.
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "off" => Self::Off,
            "full" => Self::FullScreen,
            "entity" | "entity-screen" => Self::EntityScreenGrid,
            "entity-object" | "object" => Self::EntityObjectGrid,
            _ => return None,
        })
    }

    pub fn is_masked(self) -> bool {
        matches!(self, Self::EntityScreenGrid | Self::EntityObjectGrid)
    }

    fn shader_mode(self) -> u32 {
        match self {
            Self::Off => 0,
            Self::FullScreen => 1,
            Self::EntityScreenGrid => 2,
            Self::EntityObjectGrid => 3,
        }
    }
}

/// What the spike draws and how.
#[derive(Debug, Clone)]
pub struct AsciiSpikeConfig {
    pub mode: AsciiSpikeMode,
    /// Cell size in scene pixels (8 draws the 8x8 glyphs one to one).
    pub cell_px: f32,
    /// Entities to stylize in the masked modes: exact names, `prefix*`, or
    /// `*` for every entity. Children of a match go with it.
    pub entities: Vec<String>,
    /// Exposure for the glyph ramp's luminance.
    pub exposure: f32,
    /// The cell ground's share of the cell colour.
    pub ground: f32,
    /// The glyph ink's brightness: the cell's hue at full value times
    /// `ink * (0.35 + ramp position)`.
    pub ink: f32,
    /// The mask's depth tolerance, in multiples of a fragment's depth slope.
    pub slope_tolerance: f32,
}

impl Default for AsciiSpikeConfig {
    fn default() -> Self {
        Self {
            mode: AsciiSpikeMode::Off,
            cell_px: 8.0,
            entities: Vec::new(),
            exposure: 2.0,
            ground: 0.2,
            ink: 1.0,
            slope_tolerance: 2.0,
        }
    }
}

impl AsciiSpikeConfig {
    /// Whether `name` is one of [`entities`](Self::entities).
    pub fn matches(&self, name: &str) -> bool {
        self.entities.iter().any(|p| {
            if p == "*" {
                true
            } else if let Some(prefix) = p.strip_suffix('*') {
                name.starts_with(prefix)
            } else {
                p == name
            }
        })
    }
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct AsciiUniforms {
    size: [f32; 2],
    cell: f32,
    mode: u32,
    exposure: f32,
    ground: f32,
    ink: f32,
    _pad: f32,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct MaskDraw {
    id: f32,
    slope_tolerance: f32,
    anchor: [f32; 2],
}

/// The dynamic-offset stride of the mask's per-draw uniforms.
const DRAW_STRIDE: u64 = 256;
/// Masked draws one frame can hold; more aren't masked.
pub const MAX_MASK_DRAWS: usize = 2048;

const HDR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
const MASK_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba32Float;

/// One masked draw: its buffers, its transform bind group, its id and anchor.
pub(crate) struct MaskDrawRef<'a> {
    pub vertex_buffer: &'a wgpu::Buffer,
    pub index_buffer: &'a wgpu::Buffer,
    pub index_count: u32,
    pub transform_bind_group: &'a wgpu::BindGroup,
    pub id: f32,
    pub anchor: [f32; 2],
}

struct Targets {
    size: (u32, u32),
    mask_view: wgpu::TextureView,
    out_view: wgpu::TextureView,
    _mask: wgpu::Texture,
    _out: wgpu::Texture,
}

/// The spike's pipelines and targets.
pub struct AsciiSpike {
    mask_pipeline: wgpu::RenderPipeline,
    mask_draw_bgl: wgpu::BindGroupLayout,
    draw_buffer: wgpu::Buffer,
    glyph_pipeline: wgpu::RenderPipeline,
    glyph_uniform_bgl: wgpu::BindGroupLayout,
    glyph_tex_bgl: wgpu::BindGroupLayout,
    uniform_buffer: wgpu::Buffer,
    targets: Option<Targets>,
}

impl AsciiSpike {
    pub fn new(device: &wgpu::Device, transform_bgl: &wgpu::BindGroupLayout) -> Self {
        let mask_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("ASCII Spike Mask Shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("ascii_spike_mask.wgsl").into()),
        });
        let mask_draw_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("ASCII Spike Mask Draw BGL"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: true,
                        min_binding_size: wgpu::BufferSize::new(
                            std::mem::size_of::<MaskDraw>() as u64
                        ),
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Depth,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
            ],
        });
        let mask_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("ASCII Spike Mask Layout"),
            bind_group_layouts: &[transform_bgl, &mask_draw_bgl],
            push_constant_ranges: &[],
        });
        let mask_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("ASCII Spike Mask Pipeline"),
            layout: Some(&mask_layout),
            vertex: wgpu::VertexState {
                module: &mask_shader,
                entry_point: Some("vs_main"),
                buffers: &[crate::primitives::Vertex::desc()],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &mask_shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: MASK_FORMAT,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });
        let draw_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("ASCII Spike Mask Draws"),
            size: DRAW_STRIDE * MAX_MASK_DRAWS as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let glyph_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("ASCII Spike Glyph Shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("ascii_spike.wgsl").into()),
        });
        let glyph_uniform_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("ASCII Spike Uniform BGL"),
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
        });
        let tex = |binding, filterable| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let glyph_tex_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("ASCII Spike Texture BGL"),
            // Rgba32Float isn't filterable without a feature; both are read
            // with textureLoad.
            entries: &[tex(0, false), tex(1, false)],
        });
        let glyph_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("ASCII Spike Glyph Layout"),
            bind_group_layouts: &[&glyph_uniform_bgl, &glyph_tex_bgl],
            push_constant_ranges: &[],
        });
        let glyph_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("ASCII Spike Glyph Pipeline"),
            layout: Some(&glyph_layout),
            vertex: wgpu::VertexState {
                module: &glyph_shader,
                entry_point: Some("vs_main"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &glyph_shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: HDR_FORMAT,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });
        let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("ASCII Spike Uniforms"),
            size: std::mem::size_of::<AsciiUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        Self {
            mask_pipeline,
            mask_draw_bgl,
            draw_buffer,
            glyph_pipeline,
            glyph_uniform_bgl,
            glyph_tex_bgl,
            uniform_buffer,
            targets: None,
        }
    }

    /// Size the mask and output targets to the HDR chain's `size`.
    pub fn ensure_targets(&mut self, device: &wgpu::Device, size: (u32, u32)) {
        if self.targets.as_ref().is_some_and(|t| t.size == size) {
            return;
        }
        let make = |label, format| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d {
                    width: size.0,
                    height: size.1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            })
        };
        let mask = make("ASCII Spike Mask", MASK_FORMAT);
        let out = make("ASCII Spike Output", HDR_FORMAT);
        self.targets = Some(Targets {
            size,
            mask_view: mask.create_view(&Default::default()),
            out_view: out.create_view(&Default::default()),
            _mask: mask,
            _out: out,
        });
    }

    /// The glyph pass's output, for the composite to read as the scene.
    pub fn output_view(&self) -> Option<&wgpu::TextureView> {
        self.targets.as_ref().map(|t| &t.out_view)
    }

    /// Redraw `draws` into the mask, tested against `scene_depth`.
    /// The mask is cleared first, so an empty list leaves no entity masked.
    pub(crate) fn encode_mask(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        scene_depth: &wgpu::TextureView,
        config: &AsciiSpikeConfig,
        draws: &[MaskDrawRef<'_>],
    ) {
        let Some(targets) = &self.targets else { return };
        let draws = &draws[..draws.len().min(MAX_MASK_DRAWS)];
        let mut bytes = vec![0u8; draws.len() * DRAW_STRIDE as usize];
        for (i, d) in draws.iter().enumerate() {
            let at = i * DRAW_STRIDE as usize;
            let uniform = MaskDraw {
                id: d.id,
                slope_tolerance: config.slope_tolerance,
                anchor: d.anchor,
            };
            bytes[at..at + std::mem::size_of::<MaskDraw>()]
                .copy_from_slice(bytemuck::bytes_of(&uniform));
        }
        if !bytes.is_empty() {
            queue.write_buffer(&self.draw_buffer, 0, &bytes);
        }
        // Spike: made each frame, since the depth view may be the caller's.
        let draw_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("ASCII Spike Mask Draw BG"),
            layout: &self.mask_draw_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer: &self.draw_buffer,
                        offset: 0,
                        size: wgpu::BufferSize::new(std::mem::size_of::<MaskDraw>() as u64),
                    }),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(scene_depth),
                },
            ],
        });
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("ASCII Spike Mask Pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &targets.mask_view,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        pass.set_pipeline(&self.mask_pipeline);
        for (i, d) in draws.iter().enumerate() {
            pass.set_bind_group(0, d.transform_bind_group, &[]);
            pass.set_bind_group(1, &draw_bg, &[(i as u64 * DRAW_STRIDE) as u32]);
            pass.set_vertex_buffer(0, d.vertex_buffer.slice(..));
            pass.set_index_buffer(d.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
            pass.draw_indexed(0..d.index_count, 0, 0..1);
        }
    }

    /// Glyphize `scene` into the output target.
    pub fn encode_glyphs(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        scene: &wgpu::TextureView,
        config: &AsciiSpikeConfig,
    ) {
        let Some(targets) = &self.targets else { return };
        let uniforms = AsciiUniforms {
            size: [targets.size.0 as f32, targets.size.1 as f32],
            cell: config.cell_px.max(2.0),
            mode: config.mode.shader_mode(),
            exposure: config.exposure,
            ground: config.ground,
            ink: config.ink,
            _pad: 0.0,
        };
        queue.write_buffer(&self.uniform_buffer, 0, bytemuck::bytes_of(&uniforms));
        let uniform_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("ASCII Spike Uniform BG"),
            layout: &self.glyph_uniform_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: self.uniform_buffer.as_entire_binding(),
            }],
        });
        let tex_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("ASCII Spike Texture BG"),
            layout: &self.glyph_tex_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(scene),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&targets.mask_view),
                },
            ],
        });
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("ASCII Spike Glyph Pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &targets.out_view,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        pass.set_pipeline(&self.glyph_pipeline);
        pass.set_bind_group(0, &uniform_bg, &[]);
        pass.set_bind_group(1, &tex_bg, &[]);
        pass.draw(0..3, 0..1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modes_parse_from_the_cli_spelling() {
        assert_eq!(
            AsciiSpikeMode::parse("full"),
            Some(AsciiSpikeMode::FullScreen)
        );
        assert_eq!(
            AsciiSpikeMode::parse("entity"),
            Some(AsciiSpikeMode::EntityScreenGrid)
        );
        assert_eq!(
            AsciiSpikeMode::parse("entity-object"),
            Some(AsciiSpikeMode::EntityObjectGrid)
        );
        assert_eq!(AsciiSpikeMode::parse("sideways"), None);
    }

    #[test]
    fn entity_patterns_match_exact_prefix_and_all() {
        let config = AsciiSpikeConfig {
            entities: vec!["crate".into(), "tree_*".into()],
            ..Default::default()
        };
        assert!(config.matches("crate"));
        assert!(!config.matches("crate_2"));
        assert!(config.matches("tree_oak"));
        assert!(!config.matches("bush"));
        let all = AsciiSpikeConfig {
            entities: vec!["*".into()],
            ..Default::default()
        };
        assert!(all.matches("anything"));
    }

    #[test]
    fn uniforms_are_wgsl_sized() {
        assert_eq!(std::mem::size_of::<AsciiUniforms>(), 32);
        assert_eq!(std::mem::size_of::<MaskDraw>(), 16);
    }
}
