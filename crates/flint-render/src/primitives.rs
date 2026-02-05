//! Mesh primitives (box, plane, sphere)

use bytemuck::{Pod, Zeroable};
use std::collections::HashSet;

/// A vertex with position, normal, color, and UV coordinates
#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
pub struct Vertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub color: [f32; 4],
    pub uv: [f32; 2],
}

impl Vertex {
    const ATTRIBS: [wgpu::VertexAttribute; 4] = wgpu::vertex_attr_array![
        0 => Float32x3,
        1 => Float32x3,
        2 => Float32x4,
        3 => Float32x2,
    ];

    pub fn desc() -> wgpu::VertexBufferLayout<'static> {
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Vertex>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &Self::ATTRIBS,
        }
    }
}

/// A vertex with position, normal, color, UV, and bone skinning data
#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
pub struct SkinnedVertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub color: [f32; 4],
    pub uv: [f32; 2],
    pub joint_indices: [u32; 4],
    pub joint_weights: [f32; 4],
}

impl SkinnedVertex {
    const ATTRIBS: [wgpu::VertexAttribute; 6] = wgpu::vertex_attr_array![
        0 => Float32x3,   // position
        1 => Float32x3,   // normal
        2 => Float32x4,   // color
        3 => Float32x2,   // uv
        4 => Uint32x4,    // joint_indices
        5 => Float32x4,   // joint_weights
    ];

    pub fn desc() -> wgpu::VertexBufferLayout<'static> {
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<SkinnedVertex>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &Self::ATTRIBS,
        }
    }
}

/// A mesh with skinned vertices and indices
pub struct SkinnedMesh {
    pub vertices: Vec<SkinnedVertex>,
    pub indices: Vec<u32>,
}

/// A mesh with vertices and indices
pub struct Mesh {
    pub vertices: Vec<Vertex>,
    pub indices: Vec<u32>,
}

impl Mesh {
    pub fn vertex_count(&self) -> usize {
        self.vertices.len()
    }

    pub fn index_count(&self) -> usize {
        self.indices.len()
    }
}

/// Create a box mesh with the given dimensions and color
pub fn create_box_mesh(
    width: f32,
    height: f32,
    depth: f32,
    color: [f32; 4],
) -> Mesh {
    let hw = width / 2.0;
    let hh = height / 2.0;
    let hd = depth / 2.0;

    // 8 corners
    let positions = [
        [-hw, -hh, -hd], // 0: back-bottom-left
        [hw, -hh, -hd],  // 1: back-bottom-right
        [hw, hh, -hd],   // 2: back-top-right
        [-hw, hh, -hd],  // 3: back-top-left
        [-hw, -hh, hd],  // 4: front-bottom-left
        [hw, -hh, hd],   // 5: front-bottom-right
        [hw, hh, hd],    // 6: front-top-right
        [-hw, hh, hd],   // 7: front-top-left
    ];

    let normals = [
        [0.0, 0.0, -1.0], // back
        [0.0, 0.0, 1.0],  // front
        [-1.0, 0.0, 0.0], // left
        [1.0, 0.0, 0.0],  // right
        [0.0, -1.0, 0.0], // bottom
        [0.0, 1.0, 0.0],  // top
    ];

    // Build vertices with proper normals per face (6 faces x 4 vertices = 24)
    // Vertex order per face must produce CCW winding for the outward normal
    // when indexed with [base, base+1, base+2, base, base+2, base+3]
    //
    // World-space UVs: 1 UV unit = 1 world unit, so textures tile naturally.
    let vertices = vec![
        // Back face (z-), normal [0,0,-1] — U spans width, V spans height
        Vertex { position: positions[0], normal: normals[0], color, uv: [0.0, 0.0] },
        Vertex { position: positions[3], normal: normals[0], color, uv: [0.0, height] },
        Vertex { position: positions[2], normal: normals[0], color, uv: [width, height] },
        Vertex { position: positions[1], normal: normals[0], color, uv: [width, 0.0] },
        // Front face (z+), normal [0,0,1] — U spans width, V spans height
        Vertex { position: positions[4], normal: normals[1], color, uv: [0.0, 0.0] },
        Vertex { position: positions[5], normal: normals[1], color, uv: [width, 0.0] },
        Vertex { position: positions[6], normal: normals[1], color, uv: [width, height] },
        Vertex { position: positions[7], normal: normals[1], color, uv: [0.0, height] },
        // Left face (x-), normal [-1,0,0] — U spans depth, V spans height
        Vertex { position: positions[0], normal: normals[2], color, uv: [0.0, 0.0] },
        Vertex { position: positions[4], normal: normals[2], color, uv: [depth, 0.0] },
        Vertex { position: positions[7], normal: normals[2], color, uv: [depth, height] },
        Vertex { position: positions[3], normal: normals[2], color, uv: [0.0, height] },
        // Right face (x+), normal [1,0,0] — U spans depth, V spans height
        Vertex { position: positions[5], normal: normals[3], color, uv: [0.0, 0.0] },
        Vertex { position: positions[1], normal: normals[3], color, uv: [depth, 0.0] },
        Vertex { position: positions[2], normal: normals[3], color, uv: [depth, height] },
        Vertex { position: positions[6], normal: normals[3], color, uv: [0.0, height] },
        // Bottom face (y-), normal [0,-1,0] — U spans width, V spans depth
        Vertex { position: positions[0], normal: normals[4], color, uv: [0.0, 0.0] },
        Vertex { position: positions[1], normal: normals[4], color, uv: [width, 0.0] },
        Vertex { position: positions[5], normal: normals[4], color, uv: [width, depth] },
        Vertex { position: positions[4], normal: normals[4], color, uv: [0.0, depth] },
        // Top face (y+), normal [0,1,0] — U spans width, V spans depth
        Vertex { position: positions[3], normal: normals[5], color, uv: [0.0, 0.0] },
        Vertex { position: positions[7], normal: normals[5], color, uv: [0.0, depth] },
        Vertex { position: positions[6], normal: normals[5], color, uv: [width, depth] },
        Vertex { position: positions[2], normal: normals[5], color, uv: [width, 0.0] },
    ];

    // Indices (two triangles per face)
    let indices: Vec<u32> = (0..6u32)
        .flat_map(|face| {
            let base = face * 4;
            [base, base + 1, base + 2, base, base + 2, base + 3]
        })
        .collect();

    Mesh { vertices, indices }
}

/// Create a wireframe box (edges only)
pub fn create_wireframe_box_mesh(
    width: f32,
    height: f32,
    depth: f32,
    color: [f32; 4],
) -> Mesh {
    let hw = width / 2.0;
    let hh = height / 2.0;
    let hd = depth / 2.0;

    let positions = [
        [-hw, -hh, -hd],
        [hw, -hh, -hd],
        [hw, hh, -hd],
        [-hw, hh, -hd],
        [-hw, -hh, hd],
        [hw, -hh, hd],
        [hw, hh, hd],
        [-hw, hh, hd],
    ];

    let vertices: Vec<Vertex> = positions
        .iter()
        .map(|&position| Vertex {
            position,
            normal: [0.0, 1.0, 0.0],
            color,
            uv: [0.0, 0.0],
        })
        .collect();

    // Line indices for 12 edges
    let indices: Vec<u32> = vec![
        // Bottom face edges
        0, 1, 1, 5, 5, 4, 4, 0,
        // Top face edges
        3, 2, 2, 6, 6, 7, 7, 3,
        // Vertical edges
        0, 3, 1, 2, 5, 6, 4, 7,
    ];

    Mesh { vertices, indices }
}

/// Create a plane mesh
pub fn create_plane_mesh(width: f32, depth: f32, color: [f32; 4]) -> Mesh {
    let hw = width / 2.0;
    let hd = depth / 2.0;

    let vertices = vec![
        Vertex {
            position: [-hw, 0.0, -hd],
            normal: [0.0, 1.0, 0.0],
            color,
            uv: [0.0, 0.0],
        },
        Vertex {
            position: [hw, 0.0, -hd],
            normal: [0.0, 1.0, 0.0],
            color,
            uv: [1.0, 0.0],
        },
        Vertex {
            position: [hw, 0.0, hd],
            normal: [0.0, 1.0, 0.0],
            color,
            uv: [1.0, 1.0],
        },
        Vertex {
            position: [-hw, 0.0, hd],
            normal: [0.0, 1.0, 0.0],
            color,
            uv: [0.0, 1.0],
        },
    ];

    let indices: Vec<u32> = vec![0, 1, 2, 0, 2, 3];

    Mesh { vertices, indices }
}

/// Create a grid mesh for the ground plane
pub fn create_grid_mesh(size: f32, divisions: u32, color: [f32; 4]) -> Mesh {
    let half = size / 2.0;
    let step = size / divisions as f32;

    let mut vertices = Vec::new();
    let mut indices = Vec::new();

    let mut idx = 0u32;

    // Lines along X axis
    for i in 0..=divisions {
        let z = -half + i as f32 * step;
        vertices.push(Vertex {
            position: [-half, 0.0, z],
            normal: [0.0, 1.0, 0.0],
            color,
            uv: [0.0, 0.0],
        });
        vertices.push(Vertex {
            position: [half, 0.0, z],
            normal: [0.0, 1.0, 0.0],
            color,
            uv: [1.0, 0.0],
        });
        indices.push(idx);
        indices.push(idx + 1);
        idx += 2;
    }

    // Lines along Z axis
    for i in 0..=divisions {
        let x = -half + i as f32 * step;
        vertices.push(Vertex {
            position: [x, 0.0, -half],
            normal: [0.0, 1.0, 0.0],
            color,
            uv: [0.0, 0.0],
        });
        vertices.push(Vertex {
            position: [x, 0.0, half],
            normal: [0.0, 1.0, 0.0],
            color,
            uv: [0.0, 1.0],
        });
        indices.push(idx);
        indices.push(idx + 1);
        idx += 2;
    }

    Mesh { vertices, indices }
}

/// Extract unique edges from a triangle index list for wireframe rendering.
///
/// Returns line-list indices that reuse the same vertex buffer. Shared edges
/// between adjacent triangles are deduplicated using canonical ordering.
pub fn triangles_to_wireframe_indices(indices: &[u32]) -> Vec<u32> {
    let mut edges: HashSet<(u32, u32)> = HashSet::new();
    let mut line_indices = Vec::new();

    for tri in indices.chunks_exact(3) {
        let (a, b, c) = (tri[0], tri[1], tri[2]);
        for &(i, j) in &[(a, b), (b, c), (c, a)] {
            let edge = if i < j { (i, j) } else { (j, i) };
            if edges.insert(edge) {
                line_indices.push(edge.0);
                line_indices.push(edge.1);
            }
        }
    }

    line_indices
}

/// Generate line-segment geometry showing face-normal directions.
///
/// For each triangle, computes the face center and averaged face normal, then
/// emits a line from the center to `center + normal * arrow_length`. The
/// resulting mesh uses cyan coloring and is rendered with the line pipeline.
pub fn generate_normal_arrows(vertices: &[Vertex], indices: &[u32], arrow_length: f32) -> Mesh {
    let cyan = [0.0, 1.0, 1.0, 1.0];
    let mut arrow_verts = Vec::new();
    let mut arrow_indices = Vec::new();
    let mut idx = 0u32;

    for tri in indices.chunks_exact(3) {
        let (a, b, c) = (tri[0] as usize, tri[1] as usize, tri[2] as usize);
        if a >= vertices.len() || b >= vertices.len() || c >= vertices.len() {
            continue;
        }

        let pa = vertices[a].position;
        let pb = vertices[b].position;
        let pc = vertices[c].position;

        // Face center
        let center = [
            (pa[0] + pb[0] + pc[0]) / 3.0,
            (pa[1] + pb[1] + pc[1]) / 3.0,
            (pa[2] + pb[2] + pc[2]) / 3.0,
        ];

        // Averaged vertex normal (more robust than cross product for degenerate faces)
        let na = vertices[a].normal;
        let nb = vertices[b].normal;
        let nc = vertices[c].normal;
        let avg = [
            (na[0] + nb[0] + nc[0]) / 3.0,
            (na[1] + nb[1] + nc[1]) / 3.0,
            (na[2] + nb[2] + nc[2]) / 3.0,
        ];
        let len = (avg[0] * avg[0] + avg[1] * avg[1] + avg[2] * avg[2]).sqrt();
        if len < 1e-8 {
            continue;
        }
        let normal = [avg[0] / len, avg[1] / len, avg[2] / len];

        let tip = [
            center[0] + normal[0] * arrow_length,
            center[1] + normal[1] * arrow_length,
            center[2] + normal[2] * arrow_length,
        ];

        arrow_verts.push(Vertex {
            position: center,
            normal,
            color: cyan,
            uv: [0.0, 0.0],
        });
        arrow_verts.push(Vertex {
            position: tip,
            normal,
            color: cyan,
            uv: [0.0, 0.0],
        });

        arrow_indices.push(idx);
        arrow_indices.push(idx + 1);
        idx += 2;
    }

    Mesh {
        vertices: arrow_verts,
        indices: arrow_indices,
    }
}
