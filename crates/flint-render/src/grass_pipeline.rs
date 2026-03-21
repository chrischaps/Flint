// crates/flint-render/src/grass_pipeline.rs
//! GPU-instanced grass rendering pipeline
//!
//! Two-pass system: compute shader places instances, render pass draws cross-quads.

use bytemuck::{Pod, Zeroable};

/// Per-instance data written by compute shader, read by vertex shader.
/// 24 bytes, tightly packed.
#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
pub struct GrassInstanceGpu {
    pub position: [f32; 3],  // World XYZ on terrain
    pub rotation: f32,       // Y-axis rotation (radians)
    pub height: f32,         // Scale factor (1.0 ± variation)
    pub tint: u32,           // Packed RGBA8 color shift
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
            (0.0, half_w),     // base: full width
            (0.33, half_w * 0.7),
            (0.66, half_w * 0.35),
            (1.0, 0.0),       // tip: zero width (point)
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
    // Shared blade mesh
    pub blade_vertex_buffer: wgpu::Buffer,
    pub blade_index_buffer: wgpu::Buffer,
}
