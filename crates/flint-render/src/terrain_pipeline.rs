//! Terrain rendering pipeline — splat-map blended PBR terrain

use crate::gpu_counts::CountedDevice;
use crate::primitives::Vertex;
use crate::TrustedShader;
use bytemuck::{Pod, Zeroable};

/// Terrain-specific uniform data (bind group 1, binding 0)
#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
pub struct TerrainUniforms {
    pub texture_tile: f32,
    pub metallic: f32,
    pub roughness: f32,
    /// What each frame writes from here on: [`TerrainFrameFlags`], 20 bytes
    /// at [`Self::FRAME_OFFSET`]. The struct is 32 bytes, as the WGSL's
    /// (its `chunks` is a `vec2`, aligned to 8: offset 24).
    pub frame: TerrainFrameFlags,
}

/// The tail of [`TerrainUniforms`] a frame writes (byte offset 12): the
/// tonemapping flag, the debug view and the chunk grid (#1692).
#[repr(C)]
#[derive(Copy, Clone, Debug, Default, PartialEq, Pod, Zeroable)]
pub struct TerrainFrameFlags {
    pub enable_tonemapping: u32,
    /// [`crate::DebugMode::as_u32`]: which view `fs_debug` draws.
    pub debug_mode: u32,
    pub _pad: u32,
    /// Chunks along x and z, for `fs_wire`'s chunk borders.
    pub chunks: [f32; 2],
}

impl TerrainUniforms {
    /// Where [`TerrainFrameFlags`] starts in the uniform buffer.
    pub const FRAME_OFFSET: u64 = 12;
}

/// A draw call for one terrain chunk
pub struct TerrainDrawCall {
    pub vertex_buffer: wgpu::Buffer,
    pub index_buffer: wgpu::Buffer,
    pub index_count: u32,
    pub transform_buffer: wgpu::Buffer,
    pub transform_bind_group: wgpu::BindGroup,
    pub model: [[f32; 4]; 4],
    pub model_inv_transpose: [[f32; 4]; 4],
    /// World-space AABB minimum corner (chunk local AABB + model translation)
    pub aabb_min: [f32; 3],
    /// World-space AABB maximum corner (chunk local AABB + model translation)
    pub aabb_max: [f32; 3],
    /// Which of the terrain's chunk shapes (its triangle lists, kept once
    /// each) this chunk is: where the wireframe views find its edges.
    pub shape: usize,
}

/// The terrain render pipeline
pub struct TerrainPipeline {
    pub pipeline: wgpu::RenderPipeline,
    pub material_bind_group_layout: wgpu::BindGroupLayout,
    /// What [`Self::debug_pipelines`] builds from, later: the module, the
    /// layout, and the targets the lit pipeline was built for.
    shader: wgpu::ShaderModule,
    layout: wgpu::PipelineLayout,
    format: wgpu::TextureFormat,
    sample_count: u32,
}

/// The debug views' pipelines for a terrain's chunks (#1692), over the lit
/// pipeline's module and layout, so a chunk binds the same groups. Built the
/// first frame a debug view draws a terrain
/// (`ScenePipelines::ensure_terrain_debug`), so a game pays nothing for
/// them. The lit `fs_main` is none of them: a frame with no debug view is
/// drawn by the code it always was.
pub struct TerrainDebugPipelines {
    /// `fs_debug` over a chunk's triangles: normals, depth, UV checker,
    /// unlit and metal/rough, by the uniforms' `debug_mode`.
    pub shaded: wgpu::RenderPipeline,
    /// `fs_wire` over a chunk's edges (a line list): depth-tested
    /// `LessEqual` against what is drawn and writing none, as the meshes'
    /// overlay lines are, and unbiased (#944).
    pub wire: wgpu::RenderPipeline,
    /// A chunk's depth and no colour, pushed back by [`LINE_ROOM`]: the
    /// wireframe view draws it before the lines, so a hill hides the lines
    /// behind it and its own lie on it unbroken.
    pub depth: wgpu::RenderPipeline,
    /// The lit chunk (`fs_main`) pushed back by [`LINE_ROOM`]: what the
    /// wireframe overlay draws the terrain with, so its lines aren't cut to
    /// dashes where they and the surface round to the same depth.
    pub lit_under_lines: wgpu::RenderPipeline,
}

/// How far a chunk drawn under its own lines is pushed back: a few depth
/// steps, and more where the surface slopes away from the eye.
const LINE_ROOM: wgpu::DepthBiasState = wgpu::DepthBiasState {
    constant: 16,
    slope_scale: 1.0,
    clamp: 0.0,
};

/// How one terrain pipeline differs from the lit one.
struct Variant {
    label: &'static str,
    entry: &'static str,
    topology: wgpu::PrimitiveTopology,
    depth_write: bool,
    compare: wgpu::CompareFunction,
    write_mask: wgpu::ColorWrites,
    bias: wgpu::DepthBiasState,
}

impl Variant {
    /// The lit chunk: back faces culled, depth written, no bias.
    const LIT: Self = Self {
        label: "Terrain Render Pipeline",
        entry: "fs_main",
        topology: wgpu::PrimitiveTopology::TriangleList,
        depth_write: true,
        compare: wgpu::CompareFunction::Less,
        write_mask: wgpu::ColorWrites::ALL,
        bias: wgpu::DepthBiasState {
            constant: 0,
            slope_scale: 0.0,
            clamp: 0.0,
        },
    };
}

impl TerrainPipeline {
    /// Create a new terrain pipeline.
    ///
    /// `transform_layout` and `light_layout` are shared with the main PBR pipeline.
    pub fn new(
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        transform_layout: &wgpu::BindGroupLayout,
        light_layout: &wgpu::BindGroupLayout,
        sample_count: u32,
    ) -> Self {
        let shader = device.create_trusted_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Terrain Shader"),
            source: wgpu::ShaderSource::Wgsl(crate::wgsl_sources::TERRAIN.into()),
        });

        // Bind group 1: terrain material (uniforms + splat + 4 layers)
        let material_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("Terrain Material Bind Group Layout"),
                entries: &[
                    // binding 0: TerrainUniforms
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
                    // binding 1: splat map texture
                    texture_entry(1),
                    // binding 2: splat map sampler
                    sampler_entry(2),
                    // binding 3: layer 0 texture
                    texture_entry(3),
                    // binding 4: layer 0 sampler
                    sampler_entry(4),
                    // binding 5: layer 1 texture
                    texture_entry(5),
                    // binding 6: layer 1 sampler
                    sampler_entry(6),
                    // binding 7: layer 2 texture
                    texture_entry(7),
                    // binding 8: layer 2 sampler
                    sampler_entry(8),
                    // binding 9: layer 3 texture
                    texture_entry(9),
                    // binding 10: layer 3 sampler
                    sampler_entry(10),
                ],
            });

        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Terrain Pipeline Layout"),
            bind_group_layouts: &[
                Some(transform_layout),
                Some(&material_bind_group_layout),
                Some(light_layout),
            ],
            immediate_size: 0,
        });

        let pipeline = build(
            device,
            &shader,
            &layout,
            format,
            sample_count,
            &Variant::LIT,
        );
        Self {
            pipeline,
            material_bind_group_layout,
            shader,
            layout,
            format,
            sample_count,
        }
    }

    /// The debug views' pipelines, for this pipeline's targets.
    pub fn debug_pipelines(&self, device: &wgpu::Device) -> TerrainDebugPipelines {
        let make = |variant: Variant| {
            build(
                device,
                &self.shader,
                &self.layout,
                self.format,
                self.sample_count,
                &variant,
            )
        };
        TerrainDebugPipelines {
            shaded: make(Variant {
                label: "Terrain Debug Pipeline",
                entry: "fs_debug",
                ..Variant::LIT
            }),
            wire: make(Variant {
                label: "Terrain Wire Pipeline",
                entry: "fs_wire",
                topology: wgpu::PrimitiveTopology::LineList,
                depth_write: false,
                compare: wgpu::CompareFunction::LessEqual,
                ..Variant::LIT
            }),
            depth: make(Variant {
                label: "Terrain Depth Pipeline",
                entry: "fs_wire",
                write_mask: wgpu::ColorWrites::empty(),
                bias: LINE_ROOM,
                ..Variant::LIT
            }),
            lit_under_lines: make(Variant {
                label: "Terrain Under Lines Pipeline",
                bias: LINE_ROOM,
                ..Variant::LIT
            }),
        }
    }
}

/// One terrain pipeline: the lit one's state but for what `variant` says.
/// Triangles cull their back faces; lines cull nothing.
fn build(
    device: &wgpu::Device,
    shader: &wgpu::ShaderModule,
    layout: &wgpu::PipelineLayout,
    format: wgpu::TextureFormat,
    sample_count: u32,
    variant: &Variant,
) -> wgpu::RenderPipeline {
    let lines = variant.topology == wgpu::PrimitiveTopology::LineList;
    device.create_render_pipeline_counted(&wgpu::RenderPipelineDescriptor {
        label: Some(variant.label),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some("vs_main"),
            buffers: &[Some(Vertex::desc())],
            compilation_options: Default::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some(variant.entry),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                write_mask: variant.write_mask,
            })],
            compilation_options: Default::default(),
        }),
        primitive: wgpu::PrimitiveState {
            topology: variant.topology,
            strip_index_format: None,
            front_face: wgpu::FrontFace::Ccw,
            cull_mode: (!lines).then_some(wgpu::Face::Back),
            polygon_mode: wgpu::PolygonMode::Fill,
            unclipped_depth: false,
            conservative: false,
        },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: wgpu::TextureFormat::Depth32Float,
            depth_write_enabled: Some(variant.depth_write),
            depth_compare: Some(variant.compare),
            stencil: wgpu::StencilState::default(),
            bias: variant.bias,
        }),
        multisample: wgpu::MultisampleState {
            count: sample_count,
            ..Default::default()
        },
        multiview_mask: None,
        cache: None,
    })
}

fn texture_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: true },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    }
}

fn sampler_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
        count: None,
    }
}
