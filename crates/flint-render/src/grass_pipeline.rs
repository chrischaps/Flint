// crates/flint-render/src/grass_pipeline.rs
//! GPU-instanced grass rendering pipeline
//!
//! Two-pass system: compute shader places instances, render pass draws cross-quads.

use bytemuck::{Pod, Zeroable};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;
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
    /// 1: a density weight above the threshold keeps a blade with that
    /// probability, so the mask fades a field out. 0: full density above it.
    pub scale_by_density: u32,
    /// First grid cell of this frame's dispatch window (ADR 0089): only the
    /// cells within `max_distance` of the camera are dispatched.
    pub grid_start_x: u32,
    pub grid_start_z: u32,
    /// The camera frustum's planes (`ax + by + cz + d >= 0` is inside),
    /// each pushed out by [`grass_cull_padding`] (ADR 0092). A blade whose
    /// root is outside any of them isn't drawn.
    pub frustum_planes: [[f32; 4]; 6],
}

/// The grass draw's `DrawIndexedIndirect` arguments (ADR 0092). The compute
/// pass counts its blades into `instance_count`, and the main pass and the
/// shadow cascades draw from this buffer, so the CPU never reads it.
#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
pub struct GrassDrawArgs {
    pub index_count: u32,
    pub instance_count: u32,
    pub first_index: u32,
    pub base_vertex: i32,
    pub first_instance: u32,
}

impl GrassDrawArgs {
    /// The arguments each frame starts from: one blade mesh, no instances.
    pub const RESET: Self = Self {
        index_count: BLADE_INDEX_COUNT,
        instance_count: 0,
        first_index: 0,
        base_vertex: 0,
        first_instance: 0,
    };
    /// Byte offset of `instance_count`, for the stats read-back.
    pub const INSTANCE_COUNT_OFFSET: u64 = 4;
}

/// How far the grass cull pushes each frustum plane out (ADR 0092).
///
/// A blade is culled by its root, so the planes move out by the tallest
/// blade, and a blade leaning in from just outside the view still shows.
/// With shadows on, a blade outside the view can also cast into it, as far
/// as its shadow reaches along the ground: its height over the tangent of
/// the sun's elevation. That reach is capped at eight blade heights (a sun
/// about 7° up), past which the shadow is too faint and long to pop.
/// `sun_direction` is the shadow-casting light's direction, or `None` when
/// shadows are off.
pub fn grass_cull_padding(max_blade_height: f32, sun_direction: Option<[f32; 3]>) -> f32 {
    const MAX_SHADOW_REACH: f32 = 8.0;
    let reach = sun_direction.map_or(0.0, |d| {
        let across = (d[0] * d[0] + d[2] * d[2]).sqrt();
        let down = d[1].abs();
        if down * MAX_SHADOW_REACH <= across {
            MAX_SHADOW_REACH
        } else {
            across / down
        }
    });
    max_blade_height * (1.0 + reach)
}

/// The camera frustum's planes, each pushed out by `padding` world units,
/// for the grass compute pass (ADR 0092).
pub fn grass_cull_planes(view_proj: &[[f32; 4]; 4], padding: f32) -> [[f32; 4]; 6] {
    let mut planes = crate::frustum::Frustum::from_view_projection(view_proj).planes;
    for plane in &mut planes {
        plane[3] += padding;
    }
    planes
}

/// Reads the number of blades drawn back to the CPU for [`RenderStats`],
/// a frame or more late, without ever waiting on the GPU (ADR 0092).
///
/// Each frame [`record_copy`](Self::record_copy) copies the count into a
/// small buffer, unless an earlier copy is still on its way, and after the
/// submit [`start_map`](Self::start_map) maps it. [`poll`](Self::poll)
/// picks the value up once the map has finished.
///
/// [`RenderStats`]: crate::RenderStats
pub struct GrassCountReadback {
    buffer: wgpu::Buffer,
    /// A copy was recorded and hasn't been mapped yet
    copy_recorded: bool,
    /// A map is in flight; its callback sets `state`
    mapping: bool,
    /// 0 while the map is pending, 1 mapped, 2 failed
    state: Arc<AtomicU8>,
}

impl GrassCountReadback {
    pub fn new(device: &wgpu::Device) -> Self {
        Self {
            buffer: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Grass Count Read-back"),
                size: 4,
                usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
            copy_recorded: false,
            mapping: false,
            state: Default::default(),
        }
    }

    /// Records a copy of the count from the draw arguments, unless the last
    /// one hasn't come back yet.
    pub fn record_copy(&mut self, encoder: &mut wgpu::CommandEncoder, draw_args: &wgpu::Buffer) {
        if self.mapping || self.copy_recorded {
            return;
        }
        encoder.copy_buffer_to_buffer(
            draw_args,
            GrassDrawArgs::INSTANCE_COUNT_OFFSET,
            &self.buffer,
            0,
            4,
        );
        self.copy_recorded = true;
    }

    /// Starts mapping the copy. Call after the submit that holds it.
    pub fn start_map(&mut self) {
        if !self.copy_recorded {
            return;
        }
        self.copy_recorded = false;
        self.mapping = true;
        self.state.store(0, Ordering::Release);
        let state = self.state.clone();
        self.buffer
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |result| {
                state.store(if result.is_ok() { 1 } else { 2 }, Ordering::Release);
            });
    }

    /// The count, if a map has finished since the last call. Polls the
    /// device without blocking.
    pub fn poll(&mut self, device: &wgpu::Device) -> Option<u32> {
        if !self.mapping {
            return None;
        }
        device.poll(wgpu::Maintain::Poll);
        match self.state.load(Ordering::Acquire) {
            1 => {
                let count = {
                    let data = self.buffer.slice(..).get_mapped_range();
                    u32::from_le_bytes([data[0], data[1], data[2], data[3]])
                };
                self.buffer.unmap();
                self.mapping = false;
                Some(count)
            }
            2 => {
                self.mapping = false;
                None
            }
            _ => None,
        }
    }
}

/// The ground a game grows grass on instead of a Flint terrain (ADR 0089).
///
/// The ground is a world-space rectangle on the XZ plane, from `origin`
/// (its minimum corner) across `size`. The mask and the heightmap each
/// cover the whole rectangle, whatever their resolution: texel (0, 0) is at
/// the origin and rows run toward +Z.
#[derive(Clone, Copy, Debug)]
pub struct GrassGround<'a> {
    /// Minimum corner of the rectangle, world X and Z.
    pub origin: [f32; 2],
    /// Extent of the rectangle in world units, X and Z.
    pub size: [f32; 2],
    /// Where grass grows: one byte per texel, 0 none and 255 full density,
    /// `mask_width × mask_height` bytes, row by row.
    pub mask: &'a [u8],
    pub mask_width: u32,
    pub mask_height: u32,
    /// Whether a mask value between the threshold and 255 thins the field
    /// in proportion (`true`), or grows it at full density (`false`, the
    /// terrain splat rule).
    pub scale_by_mask: bool,
    /// What the blades stand on.
    pub height: GrassHeight<'a>,
}

/// The height of a [`GrassGround`].
#[derive(Clone, Copy, Debug)]
pub enum GrassHeight<'a> {
    /// Level ground at world `y`.
    Flat(f32),
    /// World `y` per texel, `width × depth` values, row by row toward +Z,
    /// sampled nearest.
    Map {
        heights: &'a [f32],
        width: u32,
        depth: u32,
    },
}

impl GrassGround<'_> {
    /// Checks the slices against their stated sizes.
    pub fn validate(&self) -> Result<(), String> {
        if self.size[0] <= 0.0 || self.size[1] <= 0.0 {
            return Err(format!(
                "grass ground size must be positive, got {:?}",
                self.size
            ));
        }
        if self.mask_width == 0 || self.mask_height == 0 {
            return Err("grass mask is empty".into());
        }
        let want = self.mask_width as usize * self.mask_height as usize;
        if self.mask.len() != want {
            return Err(format!(
                "grass mask is {} bytes, {}x{} needs {want}",
                self.mask.len(),
                self.mask_width,
                self.mask_height
            ));
        }
        if let GrassHeight::Map {
            heights,
            width,
            depth,
        } = self.height
        {
            let want = width as usize * depth as usize;
            if want == 0 || heights.len() != want {
                return Err(format!(
                    "grass heightmap is {} values, {width}x{depth} needs {want}",
                    heights.len()
                ));
            }
        }
        Ok(())
    }
}

/// Checks a repaint of part of a `mask_width × mask_height` grass mask
/// (ADR 0093): the rectangle at texel (`x`, `y`), `width × height`, must lie
/// inside the mask, and `bytes` must hold one byte for each of its texels.
pub fn check_grass_mask_patch(
    mask_width: u32,
    mask_height: u32,
    x: u32,
    y: u32,
    width: u32,
    height: u32,
    bytes: usize,
) -> Result<(), String> {
    let fits = |at: u32, len: u32, of: u32| at.checked_add(len).is_some_and(|end| end <= of);
    if !fits(x, width, mask_width) || !fits(y, height, mask_height) {
        return Err(format!(
            "grass mask patch {width}x{height} at ({x}, {y}) runs off the {mask_width}x{mask_height} mask"
        ));
    }
    let want = width as usize * height as usize;
    if bytes != want {
        return Err(format!(
            "grass mask patch is {bytes} bytes, {width}x{height} needs {want}"
        ));
    }
    Ok(())
}

/// Grid cells, and the first cell, that a grass dispatch covers this frame.
///
/// The grid has a cell every `spacing` units from `origin` across `extent`.
/// Only the cells within `max_distance` of the camera, on X and Z, can grow
/// a blade, so the dispatch covers that square clipped to the grid. The
/// cells keep their place in the whole grid, so a blade doesn't move as
/// the window follows the camera. `None` when the camera is too far away.
pub fn grass_dispatch_window(
    origin: [f32; 2],
    extent: [f32; 2],
    spacing: f32,
    camera_xz: [f32; 2],
    max_distance: f32,
) -> Option<([u32; 2], [u32; 2])> {
    let mut start = [0u32; 2];
    let mut count = [0u32; 2];
    for axis in 0..2 {
        let cells = (extent[axis] / spacing).ceil().max(0.0) as i64;
        // One extra cell each side covers the jitter of half a cell.
        let lo = ((camera_xz[axis] - max_distance - origin[axis]) / spacing).floor() as i64 - 1;
        let hi = ((camera_xz[axis] + max_distance - origin[axis]) / spacing).ceil() as i64 + 1;
        let lo = lo.clamp(0, cells);
        let hi = hi.clamp(0, cells);
        if hi <= lo {
            return None;
        }
        start[axis] = lo as u32;
        count[axis] = (hi - lo) as u32;
    }
    Some((start, count))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_covers_the_reach_of_the_camera_clipped_to_the_grid() {
        // A 1000 m ground, a cell every 0.5 m, the camera in the middle
        let (start, count) =
            grass_dispatch_window([0.0, 0.0], [1000.0, 1000.0], 0.5, [500.0, 500.0], 40.0).unwrap();
        // 460..540 m is cells 920..1080, with one more each side for jitter
        assert_eq!(start, [919, 919]);
        assert_eq!(count, [162, 162]);

        // At a corner the window is clipped to the grid
        let (start, count) =
            grass_dispatch_window([0.0, 0.0], [1000.0, 1000.0], 0.5, [0.0, 1000.0], 40.0).unwrap();
        assert_eq!(start, [0, 1919]);
        assert_eq!(count, [81, 81]);
    }

    #[test]
    fn window_keeps_cells_in_place_on_an_offset_ground() {
        // Cells count from the ground's origin, not from the camera
        let (a, _) =
            grass_dispatch_window([-300.0, 200.0], [600.0, 600.0], 0.25, [0.0, 500.0], 10.0)
                .unwrap();
        let (b, _) =
            grass_dispatch_window([-300.0, 200.0], [600.0, 600.0], 0.25, [1.0, 500.0], 10.0)
                .unwrap();
        assert_eq!(b[0] - a[0], 4); // 1 m further is four cells on
        assert_eq!(a[1], b[1]);
    }

    #[test]
    fn no_window_when_the_camera_is_out_of_reach() {
        assert!(
            grass_dispatch_window([0.0, 0.0], [100.0, 100.0], 0.5, [300.0, 50.0], 80.0).is_none()
        );
        assert!(
            grass_dispatch_window([0.0, 0.0], [100.0, 100.0], 0.5, [50.0, -200.0], 80.0).is_none()
        );
    }

    #[test]
    fn cull_padding_reaches_as_far_as_a_blade_shadow() {
        // Shadows off: the tallest blade
        assert_eq!(grass_cull_padding(0.5, None), 0.5);
        // A sun 45° up casts a shadow as long as the blade is tall
        let pad = grass_cull_padding(0.5, Some([0.6, -0.8, 0.0].map(|v| v * 1.25)));
        assert!((pad - 0.5 * (1.0 + 0.75)).abs() < 1e-5, "{pad}");
        let pad = grass_cull_padding(0.5, Some([0.0, -1.0, 1.0]));
        assert!((pad - 1.0).abs() < 1e-5, "{pad}");
        // A sun on the horizon is capped at eight blade heights
        assert_eq!(grass_cull_padding(0.5, Some([1.0, 0.0, 0.0])), 4.5);
        assert_eq!(grass_cull_padding(0.5, Some([1.0, -0.01, 0.0])), 4.5);
    }

    #[test]
    fn cull_planes_are_the_frustum_pushed_out() {
        let camera = crate::Camera::new();
        let view_proj = camera.view_projection_matrix();
        let plain = crate::frustum::Frustum::from_view_projection(&view_proj).planes;
        let padded = grass_cull_planes(&view_proj, 2.0);
        for (p, q) in plain.iter().zip(&padded) {
            assert_eq!(p[..3], q[..3]);
            assert!((q[3] - p[3] - 2.0).abs() < 1e-5);
        }
        // A point just behind the camera is outside the plain frustum and
        // inside the padded one
        let eye = camera.position_array();
        let fwd = {
            let t = [
                camera.target.x - eye[0],
                camera.target.y - eye[1],
                camera.target.z - eye[2],
            ];
            let len = (t[0] * t[0] + t[1] * t[1] + t[2] * t[2]).sqrt();
            t.map(|v| v / len)
        };
        let behind = [eye[0] - fwd[0], eye[1] - fwd[1], eye[2] - fwd[2]];
        let inside = |planes: &[[f32; 4]; 6]| {
            planes
                .iter()
                .all(|p| p[0] * behind[0] + p[1] * behind[1] + p[2] * behind[2] + p[3] >= 0.0)
        };
        assert!(!inside(&plain));
        assert!(inside(&padded));
    }

    #[test]
    fn draw_args_match_draw_indexed_indirect() {
        // wgpu's DrawIndexedIndirectArgs: five 32-bit words, the count second
        assert_eq!(std::mem::size_of::<GrassDrawArgs>(), 20);
        let args = GrassDrawArgs {
            instance_count: 7,
            ..GrassDrawArgs::RESET
        };
        let bytes = bytemuck::bytes_of(&args);
        let at = GrassDrawArgs::INSTANCE_COUNT_OFFSET as usize;
        assert_eq!(u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap()), 7);
        assert_eq!(GrassDrawArgs::RESET.index_count, BLADE_INDEX_COUNT);
    }

    #[test]
    fn ground_validation_checks_sizes() {
        let mask = [255u8; 4];
        let heights = [0.0f32; 6];
        let ground = GrassGround {
            origin: [0.0, 0.0],
            size: [10.0, 10.0],
            mask: &mask,
            mask_width: 2,
            mask_height: 2,
            scale_by_mask: true,
            height: GrassHeight::Flat(1.0),
        };
        assert!(ground.validate().is_ok());
        assert!(GrassGround {
            mask_width: 3,
            ..ground
        }
        .validate()
        .is_err());
        assert!(GrassGround {
            size: [0.0, 10.0],
            ..ground
        }
        .validate()
        .is_err());
        let map = GrassHeight::Map {
            heights: &heights,
            width: 3,
            depth: 2,
        };
        assert!(GrassGround {
            height: map,
            ..ground
        }
        .validate()
        .is_ok());
        let short = GrassHeight::Map {
            heights: &heights,
            width: 4,
            depth: 2,
        };
        assert!(GrassGround {
            height: short,
            ..ground
        }
        .validate()
        .is_err());
    }

    #[test]
    fn mask_patch_must_fit_the_mask() {
        assert!(check_grass_mask_patch(8, 4, 0, 0, 8, 4, 32).is_ok());
        assert!(check_grass_mask_patch(8, 4, 5, 1, 3, 3, 9).is_ok());
        assert!(check_grass_mask_patch(8, 4, 2, 2, 0, 0, 0).is_ok());
        // Off the right edge, off the far edge, and the wrong length.
        assert!(check_grass_mask_patch(8, 4, 6, 0, 3, 1, 3).is_err());
        assert!(check_grass_mask_patch(8, 4, 0, 2, 1, 3, 3).is_err());
        assert!(check_grass_mask_patch(8, 4, 0, 0, 2, 2, 5).is_err());
        // No overflow past u32.
        assert!(check_grass_mask_patch(8, 4, u32::MAX, 0, 2, 1, 2).is_err());
    }
}
