// crates/flint-render/src/grass_pipeline.rs
//! GPU-instanced grass rendering pipeline
//!
//! Two-pass system: compute shader places instances, render pass draws cross-quads.

use bytemuck::{Pod, Zeroable};
use wgpu::util::DeviceExt;

/// Per-instance data written by compute shader, read by vertex shader.
/// 32 bytes — must match WGSL `GrassInstance` stride (vec3<f32> forces 16-byte alignment).
#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
pub struct GrassInstanceGpu {
    pub position: [f32; 3], // World XYZ on terrain
    pub rotation: f32,      // Y-axis rotation (radians)
    pub height: f32,        // Scale factor (1.0 ± variation)
    pub tint: u32,          // Packed RGBA8 color shift
    pub _pad0: u32,         // Padding to match 32-byte WGSL stride
    pub _pad1: u32,
}

/// Uniform buffer for the compute shader.
#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
pub struct GrassComputeUniforms {
    pub camera_pos: [f32; 3],
    pub time: f32,
    pub terrain_offset: [f32; 3],
    pub density: f32,
    pub terrain_width: f32,
    pub terrain_depth: f32,
    pub height_scale: f32,
    pub max_distance: f32,
    pub fade_start: f32,
    pub density_threshold: f32,
    pub density_layer: u32,
    pub blade_height: f32,
    pub height_variation: f32,
    pub _pad0: f32,
    pub _pad1: f32,
    pub _pad2: f32,
}

/// Uniform buffer for the render (vertex/fragment) shader.
#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
pub struct GrassRenderUniforms {
    pub wind_direction: [f32; 3],
    pub wind_speed: f32,
    pub wind_strength: f32,
    pub time: f32,
    pub bend_radius: f32,
    pub bend_strength: f32,
    pub color_base: [f32; 3],
    pub blade_width: f32,
    pub color_tip: [f32; 3],
    pub blade_height: f32,
    pub color_dry: [f32; 3],
    pub dry_amount: f32,
    pub entity_count: u32,
    pub _pad0: f32,
    pub _pad1: f32,
    pub _pad2: f32,
    // Entity positions follow as a separate binding
}

/// Entity position for bend-on-contact (max 8).
#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
pub struct GrassEntityPosition {
    pub position: [f32; 3],
    pub _pad: f32,
}

/// Maximum number of entities that can bend grass
pub const MAX_GRASS_ENTITIES: usize = 8;

/// Vertex for the shared blade quad mesh.
#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
pub struct GrassVertex {
    pub position: [f32; 3],
    pub uv: [f32; 2],
}

impl GrassVertex {
    const ATTRIBS: [wgpu::VertexAttribute; 2] = wgpu::vertex_attr_array![
        0 => Float32x3,
        1 => Float32x2,
    ];

    pub fn desc() -> wgpu::VertexBufferLayout<'static> {
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<GrassVertex>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &Self::ATTRIBS,
        }
    }
}

/// Number of indices in the shared blade mesh (3 quads × 4 triangles × 3 = 36)
pub const BLADE_INDEX_COUNT: u32 = 36;

/// Generate the shared cross-quad blade mesh.
/// Returns (vertices, indices) for 3 intersecting quads at 60° intervals.
/// Each quad has 7 vertices (4 segments + pointed tip).
pub fn generate_blade_mesh() -> (Vec<GrassVertex>, Vec<u16>) {
    let mut vertices = Vec::with_capacity(21);
    let mut indices = Vec::with_capacity(36);

    let half_w = 0.5_f32; // Normalized; scaled by blade_width in vertex shader

    for quad_idx in 0..3u32 {
        let angle = (quad_idx as f32) * std::f32::consts::PI / 3.0; // 0°, 60°, 120°
        let cos_a = angle.cos();
        let sin_a = angle.sin();

        let base_vertex = (quad_idx * 7) as u16;

        // 7 vertices per quad: 2 per segment row (4 rows) minus shared tip
        // Row 0 (base): v=0.0
        // Row 1: v=0.33
        // Row 2: v=0.66
        // Row 3 (tip): v=1.0 (single vertex)
        let rows: [(f32, f32); 4] = [
            (0.0, half_w), // base: full width
            (0.33, half_w * 0.7),
            (0.66, half_w * 0.35),
            (1.0, 0.0), // tip: zero width (point)
        ];

        for (row_idx, &(v, hw)) in rows.iter().enumerate() {
            if row_idx < 3 {
                // Two vertices per row (left + right)
                vertices.push(GrassVertex {
                    position: [-hw * cos_a, v, -hw * sin_a],
                    uv: [0.0, v],
                });
                vertices.push(GrassVertex {
                    position: [hw * cos_a, v, hw * sin_a],
                    uv: [1.0, v],
                });
            } else {
                // Tip: single vertex
                vertices.push(GrassVertex {
                    position: [0.0, v, 0.0],
                    uv: [0.5, v],
                });
            }
        }

        // Indices: 3 rectangular segments + 1 tip triangle = 4 triangles
        // Segment 0: row0-row1
        indices.push(base_vertex);
        indices.push(base_vertex + 1);
        indices.push(base_vertex + 2);
        indices.push(base_vertex + 2);
        indices.push(base_vertex + 1);
        indices.push(base_vertex + 3);

        // Segment 1: row1-row2
        indices.push(base_vertex + 2);
        indices.push(base_vertex + 3);
        indices.push(base_vertex + 4);
        indices.push(base_vertex + 4);
        indices.push(base_vertex + 3);
        indices.push(base_vertex + 5);

        // Tip triangle: row2-tip
        // Note: 2 triangles from the 2 row2 vertices to the single tip vertex
        // But since tip is a point, we get a degenerate second triangle.
        // Better: one triangle left-right-tip, skip the degenerate
        // Actually for consistent 12 indices per quad (4 tris), use both:
        indices.push(base_vertex + 4);
        indices.push(base_vertex + 5);
        indices.push(base_vertex + 6);
        // Backface of tip (same triangle, reversed for double-sided)
        indices.push(base_vertex + 6);
        indices.push(base_vertex + 5);
        indices.push(base_vertex + 4);
    }

    (vertices, indices)
}

/// The grass rendering pipeline (compute + render)
pub struct GrassPipeline {
    pub compute_pipeline: wgpu::ComputePipeline,
    pub render_pipeline: wgpu::RenderPipeline,
    pub shadow_pipeline: wgpu::RenderPipeline,
    // Bind group layouts
    pub compute_uniform_layout: wgpu::BindGroupLayout,
    pub compute_texture_layout: wgpu::BindGroupLayout,
    pub compute_storage_layout: wgpu::BindGroupLayout,
    pub render_grass_layout: wgpu::BindGroupLayout,
    pub render_instance_layout: wgpu::BindGroupLayout,
    pub shadow_dummy_layout: wgpu::BindGroupLayout,
    pub shadow_dummy_bind_group: wgpu::BindGroup,
    // Shared blade mesh
    pub blade_vertex_buffer: wgpu::Buffer,
    pub blade_index_buffer: wgpu::Buffer,
}

impl GrassPipeline {
    pub fn new(
        device: &wgpu::Device,
        scene_format: wgpu::TextureFormat,
        transform_bind_group_layout: &wgpu::BindGroupLayout,
        light_bind_group_layout: &wgpu::BindGroupLayout,
        sample_count: u32,
    ) -> Option<Self> {
        // Compute shader
        let compute_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Grass Compute Shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("grass_compute.wgsl").into()),
        });

        // Render shader
        let render_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Grass Render Shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("grass_render.wgsl").into()),
        });

        // --- Compute bind group layouts ---

        // Group 0: Compute uniforms
        let compute_uniform_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                entries: &[wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                }],
                label: Some("Grass Compute Uniform Layout"),
            });

        // Group 1: Heightmap + splat textures
        let compute_texture_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                entries: &[
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::COMPUTE,
                        ty: wgpu::BindingType::Texture {
                            // R32Float is not filterable without Float32Filterable feature
                            sample_type: wgpu::TextureSampleType::Float { filterable: false },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: wgpu::ShaderStages::COMPUTE,
                        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::NonFiltering),
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 2,
                        visibility: wgpu::ShaderStages::COMPUTE,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: true },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 3,
                        visibility: wgpu::ShaderStages::COMPUTE,
                        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                        count: None,
                    },
                ],
                label: Some("Grass Compute Texture Layout"),
            });

        // Group 2: Instance storage buffer (read-write) + atomic counter
        let compute_storage_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                entries: &[
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::COMPUTE,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Storage { read_only: false },
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: wgpu::ShaderStages::COMPUTE,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Storage { read_only: false },
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    },
                ],
                label: Some("Grass Compute Storage Layout"),
            });

        // Compute pipeline
        let compute_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                bind_group_layouts: &[
                    &compute_uniform_layout,
                    &compute_texture_layout,
                    &compute_storage_layout,
                ],
                push_constant_ranges: &[],
                label: Some("Grass Compute Pipeline Layout"),
            });

        let compute_pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("Grass Compute Pipeline"),
            layout: Some(&compute_pipeline_layout),
            module: &compute_shader,
            entry_point: Some("cs_main"),
            compilation_options: Default::default(),
            cache: None,
        });

        // --- Render bind group layouts ---

        // Group 1: Grass render uniforms
        let render_grass_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                entries: &[wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                }],
                label: Some("Grass Render Uniform Layout"),
            });

        // Group 3: Instance buffer (read) + entity positions (read)
        let render_instance_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                entries: &[
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::VERTEX,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Storage { read_only: true },
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: wgpu::ShaderStages::VERTEX,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Storage { read_only: true },
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    },
                ],
                label: Some("Grass Render Instance Layout"),
            });

        // Render pipeline
        let render_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                bind_group_layouts: &[
                    transform_bind_group_layout, // Group 0
                    &render_grass_layout,        // Group 1
                    light_bind_group_layout,     // Group 2
                    &render_instance_layout,     // Group 3
                ],
                push_constant_ranges: &[],
                label: Some("Grass Render Pipeline Layout"),
            });

        let render_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Grass Render Pipeline"),
            layout: Some(&render_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &render_shader,
                entry_point: Some("vs_main"),
                buffers: &[GrassVertex::desc()],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &render_shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: scene_format,
                    blend: None, // Opaque with alpha test (discard in shader)
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                cull_mode: None, // Double-sided grass blades
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: true,
                depth_compare: wgpu::CompareFunction::Less,
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: wgpu::MultisampleState {
                count: sample_count,
                ..Default::default()
            },
            multiview: None,
            cache: None,
        });

        // Shadow pipeline (depth-only, uses vs_shadow entry point)
        // Shadow pipeline uses same 4-group layout as render so @group bindings match,
        // but group 2 is an EMPTY placeholder instead of the light bind group.
        // This avoids a texture usage conflict: the light bind group references the
        // shadow depth texture as a resource, but the shadow pass writes to that same
        // texture as a depth attachment. Using an empty group at position 2 sidesteps this.
        let shadow_dummy_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                entries: &[],
                label: Some("Grass Shadow Dummy Layout"),
            });

        let shadow_dummy_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            layout: &shadow_dummy_layout,
            entries: &[],
            label: Some("Grass Shadow Dummy Bind Group"),
        });

        let shadow_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                bind_group_layouts: &[
                    transform_bind_group_layout, // Group 0
                    &render_grass_layout,        // Group 1
                    &shadow_dummy_layout, // Group 2: empty placeholder (NOT light bind group)
                    &render_instance_layout, // Group 3
                ],
                push_constant_ranges: &[],
                label: Some("Grass Shadow Pipeline Layout"),
            });

        let shadow_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Grass Shadow Pipeline"),
            layout: Some(&shadow_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &render_shader,
                entry_point: Some("vs_shadow"),
                buffers: &[GrassVertex::desc()],
                compilation_options: Default::default(),
            },
            fragment: None, // Depth-only
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: true,
                depth_compare: wgpu::CompareFunction::Less,
                stencil: Default::default(),
                bias: wgpu::DepthBiasState {
                    constant: 2,
                    slope_scale: 1.5,
                    clamp: 0.0,
                },
            }),
            multisample: Default::default(),
            multiview: None,
            cache: None,
        });

        // Generate blade mesh
        let (blade_verts, blade_indices) = generate_blade_mesh();

        let blade_vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Grass Blade Vertex Buffer"),
            contents: bytemuck::cast_slice(&blade_verts),
            usage: wgpu::BufferUsages::VERTEX,
        });

        let blade_index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Grass Blade Index Buffer"),
            contents: bytemuck::cast_slice(&blade_indices),
            usage: wgpu::BufferUsages::INDEX,
        });

        Some(Self {
            compute_pipeline,
            render_pipeline,
            shadow_pipeline,
            compute_uniform_layout,
            compute_texture_layout,
            compute_storage_layout,
            render_grass_layout,
            render_instance_layout,
            shadow_dummy_layout,
            shadow_dummy_bind_group,
            blade_vertex_buffer,
            blade_index_buffer,
        })
    }
}
