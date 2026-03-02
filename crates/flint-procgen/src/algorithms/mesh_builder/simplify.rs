//! Mesh simplification via Garland & Heckbert quadric error metrics.
//!
//! Iteratively collapses the lowest-cost edge until the target triangle count
//! is reached. Uses f64 internally for numerical stability with large meshes.

use crate::types::{BoundingBox, MeshData, Vertex};
use std::collections::{BinaryHeap, HashMap, HashSet};

/// Simplify a mesh to approximately `target_triangles` using quadric error metrics.
///
/// If the mesh already has fewer triangles than the target, it is returned unchanged.
/// The output mesh has no degenerate triangles and valid indices.
pub fn simplify(mesh: &MeshData, target_triangles: usize) -> MeshData {
    let current_tris = mesh.triangle_count();
    if current_tris <= target_triangles || mesh.vertices.is_empty() {
        return mesh.clone();
    }

    let mut state = SimplifyState::new(mesh);
    state.collapse_until(target_triangles);
    state.compact(mesh)
}

/// Symmetric 4x4 quadric stored as 10 f64 values.
///
/// Represents the error metric Q = sum of outer products of plane equations.
/// For plane `ax + by + cz + d = 0`, the quadric is `[a,b,c,d]^T * [a,b,c,d]`.
#[derive(Clone, Copy)]
struct Quadric {
    // Upper triangle of symmetric 4x4:
    // [a2,  ab, ac, ad]
    // [ab,  b2, bc, bd]
    // [ac,  bc, c2, cd]
    // [ad,  bd, cd, d2]
    a2: f64,
    ab: f64,
    ac: f64,
    ad: f64,
    b2: f64,
    bc: f64,
    bd: f64,
    c2: f64,
    cd: f64,
    d2: f64,
}

impl Quadric {
    fn zero() -> Self {
        Self {
            a2: 0.0,
            ab: 0.0,
            ac: 0.0,
            ad: 0.0,
            b2: 0.0,
            bc: 0.0,
            bd: 0.0,
            c2: 0.0,
            cd: 0.0,
            d2: 0.0,
        }
    }

    /// Create a quadric from a plane equation `ax + by + cz + d = 0`.
    fn from_plane(a: f64, b: f64, c: f64, d: f64) -> Self {
        Self {
            a2: a * a,
            ab: a * b,
            ac: a * c,
            ad: a * d,
            b2: b * b,
            bc: b * c,
            bd: b * d,
            c2: c * c,
            cd: c * d,
            d2: d * d,
        }
    }

    /// Sum two quadrics.
    fn add(&self, other: &Quadric) -> Self {
        Self {
            a2: self.a2 + other.a2,
            ab: self.ab + other.ab,
            ac: self.ac + other.ac,
            ad: self.ad + other.ad,
            b2: self.b2 + other.b2,
            bc: self.bc + other.bc,
            bd: self.bd + other.bd,
            c2: self.c2 + other.c2,
            cd: self.cd + other.cd,
            d2: self.d2 + other.d2,
        }
    }

    /// Evaluate the error `v^T * Q * v` at position (x, y, z).
    fn evaluate(&self, x: f64, y: f64, z: f64) -> f64 {
        self.a2 * x * x
            + 2.0 * self.ab * x * y
            + 2.0 * self.ac * x * z
            + 2.0 * self.ad * x
            + self.b2 * y * y
            + 2.0 * self.bc * y * z
            + 2.0 * self.bd * y
            + self.c2 * z * z
            + 2.0 * self.cd * z
            + self.d2
    }

    /// Find the optimal position that minimizes the quadric error.
    /// Falls back to the midpoint of the two vertices if the matrix is singular.
    fn optimal_position(&self, v1: [f64; 3], v2: [f64; 3]) -> [f64; 3] {
        // Try to solve the linear system from the derivative of v^T*Q*v = 0
        // The 3x3 upper-left submatrix and the 3x1 right-hand side:
        // [a2, ab, ac] [x]   [-ad]
        // [ab, b2, bc] [y] = [-bd]
        // [ac, bc, c2] [z]   [-cd]
        let det = self.a2 * (self.b2 * self.c2 - self.bc * self.bc)
            - self.ab * (self.ab * self.c2 - self.bc * self.ac)
            + self.ac * (self.ab * self.bc - self.b2 * self.ac);

        if det.abs() > 1e-12 {
            let inv_det = 1.0 / det;

            let x = inv_det
                * (-self.ad * (self.b2 * self.c2 - self.bc * self.bc)
                    + self.bd * (self.ab * self.c2 - self.ac * self.bc)
                    - self.cd * (self.ab * self.bc - self.ac * self.b2));

            let y = inv_det
                * (self.ad * (self.ab * self.c2 - self.bc * self.ac)
                    - self.bd * (self.a2 * self.c2 - self.ac * self.ac)
                    + self.cd * (self.a2 * self.bc - self.ac * self.ab));

            let z = inv_det
                * (-self.ad * (self.ab * self.bc - self.b2 * self.ac)
                    + self.bd * (self.a2 * self.bc - self.ab * self.ac)
                    - self.cd * (self.a2 * self.b2 - self.ab * self.ab));

            [x, y, z]
        } else {
            // Singular: try midpoint, v1, v2 — pick the lowest-error one
            let mid = [
                (v1[0] + v2[0]) * 0.5,
                (v1[1] + v2[1]) * 0.5,
                (v1[2] + v2[2]) * 0.5,
            ];

            let e_mid = self.evaluate(mid[0], mid[1], mid[2]);
            let e_v1 = self.evaluate(v1[0], v1[1], v1[2]);
            let e_v2 = self.evaluate(v2[0], v2[1], v2[2]);

            if e_mid <= e_v1 && e_mid <= e_v2 {
                mid
            } else if e_v1 <= e_v2 {
                v1
            } else {
                v2
            }
        }
    }
}

/// Edge collapse candidate in the priority queue.
#[derive(Clone)]
struct EdgeCollapse {
    cost: f64,
    v1: u32,
    v2: u32,
    generation_v1: u32,
    generation_v2: u32,
}

impl PartialEq for EdgeCollapse {
    fn eq(&self, other: &Self) -> bool {
        self.cost == other.cost
    }
}

impl Eq for EdgeCollapse {}

impl PartialOrd for EdgeCollapse {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for EdgeCollapse {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        // Min-heap: reverse comparison so lowest cost comes first
        other
            .cost
            .partial_cmp(&self.cost)
            .unwrap_or(std::cmp::Ordering::Equal)
    }
}

/// Internal simplification state.
struct SimplifyState {
    positions: Vec<[f64; 3]>,
    normals: Vec<[f64; 3]>,
    uvs: Vec<[f64; 2]>,
    tangents: Vec<[f64; 4]>,
    triangles: Vec<[u32; 3]>, // active triangles
    tri_active: Vec<bool>,    // per-triangle active flag
    vertex_active: Vec<bool>,
    quadrics: Vec<Quadric>,
    generation: Vec<u32>,            // per-vertex generation counter
    vertex_tris: Vec<Vec<usize>>,    // triangles adjacent to each vertex
    remap: Vec<u32>,                 // vertex remap (v -> canonical vertex)
    boundary_vertices: HashSet<u32>, // vertices on boundary edges
}

impl SimplifyState {
    fn new(mesh: &MeshData) -> Self {
        let n = mesh.vertices.len();

        let positions: Vec<[f64; 3]> = mesh
            .vertices
            .iter()
            .map(|v| {
                [
                    v.position[0] as f64,
                    v.position[1] as f64,
                    v.position[2] as f64,
                ]
            })
            .collect();
        let normals: Vec<[f64; 3]> = mesh
            .vertices
            .iter()
            .map(|v| [v.normal[0] as f64, v.normal[1] as f64, v.normal[2] as f64])
            .collect();
        let uvs: Vec<[f64; 2]> = mesh
            .vertices
            .iter()
            .map(|v| [v.uv[0] as f64, v.uv[1] as f64])
            .collect();
        let tangents: Vec<[f64; 4]> = mesh
            .vertices
            .iter()
            .map(|v| {
                [
                    v.tangent[0] as f64,
                    v.tangent[1] as f64,
                    v.tangent[2] as f64,
                    v.tangent[3] as f64,
                ]
            })
            .collect();

        let tri_count = mesh.indices.len() / 3;
        let mut triangles = Vec::with_capacity(tri_count);
        let mut vertex_tris: Vec<Vec<usize>> = vec![Vec::new(); n];

        for (ti, chunk) in mesh.indices.chunks_exact(3).enumerate() {
            let tri = [chunk[0], chunk[1], chunk[2]];
            triangles.push(tri);
            for &vi in &tri {
                vertex_tris[vi as usize].push(ti);
            }
        }

        let tri_active = vec![true; tri_count];
        let vertex_active = vec![true; n];
        let generation = vec![0u32; n];
        let remap: Vec<u32> = (0..n as u32).collect();

        // Detect boundary edges (edges that appear in only one triangle)
        let mut edge_count: HashMap<(u32, u32), u32> = HashMap::new();
        for tri in &triangles {
            for k in 0..3 {
                let a = tri[k].min(tri[(k + 1) % 3]);
                let b = tri[k].max(tri[(k + 1) % 3]);
                *edge_count.entry((a, b)).or_insert(0) += 1;
            }
        }
        let mut boundary_vertices = HashSet::new();
        for (&(a, b), &count) in &edge_count {
            if count == 1 {
                boundary_vertices.insert(a);
                boundary_vertices.insert(b);
            }
        }

        // Compute per-vertex quadrics from adjacent face plane equations
        let mut quadrics = vec![Quadric::zero(); n];
        for tri in &triangles {
            let p0 = positions[tri[0] as usize];
            let p1 = positions[tri[1] as usize];
            let p2 = positions[tri[2] as usize];

            let e0 = [p1[0] - p0[0], p1[1] - p0[1], p1[2] - p0[2]];
            let e1 = [p2[0] - p0[0], p2[1] - p0[1], p2[2] - p0[2]];

            let nx = e0[1] * e1[2] - e0[2] * e1[1];
            let ny = e0[2] * e1[0] - e0[0] * e1[2];
            let nz = e0[0] * e1[1] - e0[1] * e1[0];
            let len = (nx * nx + ny * ny + nz * nz).sqrt();

            if len < 1e-14 {
                continue;
            }

            let a = nx / len;
            let b = ny / len;
            let c = nz / len;
            let d = -(a * p0[0] + b * p0[1] + c * p0[2]);

            let q = Quadric::from_plane(a, b, c, d);
            for &vi in tri {
                quadrics[vi as usize] = quadrics[vi as usize].add(&q);
            }
        }

        // Add boundary penalty: large-weight planes perpendicular to boundary edges
        let boundary_weight = 1000.0;
        for (&(a, b), &count) in &edge_count {
            if count == 1 {
                let pa = positions[a as usize];
                let pb = positions[b as usize];
                let edge = [pb[0] - pa[0], pb[1] - pa[1], pb[2] - pa[2]];
                let edge_len = (edge[0] * edge[0] + edge[1] * edge[1] + edge[2] * edge[2]).sqrt();
                if edge_len < 1e-14 {
                    continue;
                }

                // Find the triangle that contains this edge to get its normal
                let mut face_normal = [0.0f64; 3];
                for &ti in &vertex_tris[a as usize] {
                    let tri = &triangles[ti];
                    if tri.contains(&b) {
                        let p0 = positions[tri[0] as usize];
                        let p1 = positions[tri[1] as usize];
                        let p2 = positions[tri[2] as usize];
                        let e0 = [p1[0] - p0[0], p1[1] - p0[1], p1[2] - p0[2]];
                        let e1 = [p2[0] - p0[0], p2[1] - p0[1], p2[2] - p0[2]];
                        face_normal = [
                            e0[1] * e1[2] - e0[2] * e1[1],
                            e0[2] * e1[0] - e0[0] * e1[2],
                            e0[0] * e1[1] - e0[1] * e1[0],
                        ];
                        let fn_len = (face_normal[0] * face_normal[0]
                            + face_normal[1] * face_normal[1]
                            + face_normal[2] * face_normal[2])
                            .sqrt();
                        if fn_len > 1e-14 {
                            face_normal[0] /= fn_len;
                            face_normal[1] /= fn_len;
                            face_normal[2] /= fn_len;
                        }
                        break;
                    }
                }

                // Boundary plane: perpendicular to both edge and face normal
                let bn = [
                    edge[1] * face_normal[2] - edge[2] * face_normal[1],
                    edge[2] * face_normal[0] - edge[0] * face_normal[2],
                    edge[0] * face_normal[1] - edge[1] * face_normal[0],
                ];
                let bn_len = (bn[0] * bn[0] + bn[1] * bn[1] + bn[2] * bn[2]).sqrt();
                if bn_len < 1e-14 {
                    continue;
                }
                let bn_a = bn[0] / bn_len * boundary_weight;
                let bn_b = bn[1] / bn_len * boundary_weight;
                let bn_c = bn[2] / bn_len * boundary_weight;
                let bn_d = -(bn_a * pa[0] + bn_b * pa[1] + bn_c * pa[2]);

                let bq = Quadric::from_plane(bn_a, bn_b, bn_c, bn_d);
                quadrics[a as usize] = quadrics[a as usize].add(&bq);
                quadrics[b as usize] = quadrics[b as usize].add(&bq);
            }
        }

        Self {
            positions,
            normals,
            uvs,
            tangents,
            triangles,
            tri_active,
            vertex_active,
            quadrics,
            generation,
            vertex_tris,
            remap,
            boundary_vertices,
        }
    }

    fn active_triangle_count(&self) -> usize {
        self.tri_active.iter().filter(|&&a| a).count()
    }

    fn build_edge_queue(&self) -> BinaryHeap<EdgeCollapse> {
        let mut edges_seen = HashSet::new();
        let mut heap = BinaryHeap::new();

        for (ti, tri) in self.triangles.iter().enumerate() {
            if !self.tri_active[ti] {
                continue;
            }
            for k in 0..3 {
                let a = tri[k].min(tri[(k + 1) % 3]);
                let b = tri[k].max(tri[(k + 1) % 3]);
                if edges_seen.insert((a, b)) {
                    let cost = self.edge_cost(a, b);
                    heap.push(EdgeCollapse {
                        cost,
                        v1: a,
                        v2: b,
                        generation_v1: self.generation[a as usize],
                        generation_v2: self.generation[b as usize],
                    });
                }
            }
        }

        heap
    }

    fn edge_cost(&self, v1: u32, v2: u32) -> f64 {
        let q = self.quadrics[v1 as usize].add(&self.quadrics[v2 as usize]);
        let opt = q.optimal_position(self.positions[v1 as usize], self.positions[v2 as usize]);
        q.evaluate(opt[0], opt[1], opt[2]).max(0.0)
    }

    fn collapse_until(&mut self, target: usize) {
        let mut heap = self.build_edge_queue();

        while self.active_triangle_count() > target {
            let collapse = match heap.pop() {
                Some(c) => c,
                None => break,
            };

            // Stale check
            if !self.vertex_active[collapse.v1 as usize]
                || !self.vertex_active[collapse.v2 as usize]
            {
                continue;
            }
            if self.generation[collapse.v1 as usize] != collapse.generation_v1
                || self.generation[collapse.v2 as usize] != collapse.generation_v2
            {
                continue;
            }

            self.collapse_edge(collapse.v1, collapse.v2, &mut heap);
        }
    }

    fn collapse_edge(&mut self, v1: u32, v2: u32, heap: &mut BinaryHeap<EdgeCollapse>) {
        let q_combined = self.quadrics[v1 as usize].add(&self.quadrics[v2 as usize]);
        let opt =
            q_combined.optimal_position(self.positions[v1 as usize], self.positions[v2 as usize]);

        // Interpolation parameter: where is opt relative to v1-v2?
        let p1 = self.positions[v1 as usize];
        let p2 = self.positions[v2 as usize];
        let edge = [p2[0] - p1[0], p2[1] - p1[1], p2[2] - p1[2]];
        let edge_len_sq = edge[0] * edge[0] + edge[1] * edge[1] + edge[2] * edge[2];
        let t = if edge_len_sq > 1e-14 {
            let dp = [opt[0] - p1[0], opt[1] - p1[1], opt[2] - p1[2]];
            let dot = dp[0] * edge[0] + dp[1] * edge[1] + dp[2] * edge[2];
            (dot / edge_len_sq).clamp(0.0, 1.0)
        } else {
            0.5
        };

        // Move v1 to optimal position, interpolate attributes
        self.positions[v1 as usize] = opt;
        self.normals[v1 as usize] =
            lerp3(&self.normals[v1 as usize], &self.normals[v2 as usize], t);
        self.uvs[v1 as usize] = lerp2(&self.uvs[v1 as usize], &self.uvs[v2 as usize], t);
        self.tangents[v1 as usize] =
            lerp4(&self.tangents[v1 as usize], &self.tangents[v2 as usize], t);

        // Renormalize the interpolated normal
        let n = &mut self.normals[v1 as usize];
        let nlen = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
        if nlen > 1e-14 {
            n[0] /= nlen;
            n[1] /= nlen;
            n[2] /= nlen;
        }

        // Update quadric
        self.quadrics[v1 as usize] = q_combined;

        // Deactivate v2
        self.vertex_active[v2 as usize] = false;
        self.remap[v2 as usize] = v1;

        // Update generation
        self.generation[v1 as usize] += 1;
        self.generation[v2 as usize] += 1;

        // If v2 was boundary, v1 inherits boundary status
        if self.boundary_vertices.contains(&v2) {
            self.boundary_vertices.insert(v1);
        }

        // Remap triangles: replace v2 with v1, deactivate degenerate triangles
        let v2_tris: Vec<usize> = self.vertex_tris[v2 as usize].clone();
        for &ti in &v2_tris {
            if !self.tri_active[ti] {
                continue;
            }
            let tri = &mut self.triangles[ti];
            for vi in tri.iter_mut() {
                if *vi == v2 {
                    *vi = v1;
                }
            }
            // Check for degenerate (two or more identical vertices)
            if tri[0] == tri[1] || tri[1] == tri[2] || tri[0] == tri[2] {
                self.tri_active[ti] = false;
            } else {
                // Add triangle to v1's adjacency if not already there
                if !self.vertex_tris[v1 as usize].contains(&ti) {
                    self.vertex_tris[v1 as usize].push(ti);
                }
            }
        }

        // Collect affected vertices for re-evaluation
        let mut affected = HashSet::new();
        for &ti in &self.vertex_tris[v1 as usize] {
            if !self.tri_active[ti] {
                continue;
            }
            for &vi in &self.triangles[ti] {
                if vi != v1 && self.vertex_active[vi as usize] {
                    affected.insert(vi);
                }
            }
        }

        // Re-insert affected edges into the heap
        for &vi in &affected {
            let cost = self.edge_cost(v1, vi);
            heap.push(EdgeCollapse {
                cost,
                v1: v1.min(vi),
                v2: v1.max(vi),
                generation_v1: self.generation[v1.min(vi) as usize],
                generation_v2: self.generation[v1.max(vi) as usize],
            });
        }
    }

    /// Compact the mesh: remove inactive vertices/triangles, build new buffers.
    fn compact(&self, original: &MeshData) -> MeshData {
        // Build vertex remap: old index -> new index
        let mut new_index = vec![u32::MAX; self.positions.len()];
        let mut vertices = Vec::new();
        let mut next_idx = 0u32;

        // Resolve remap chains
        let resolved: Vec<u32> = (0..self.positions.len() as u32)
            .map(|i| self.resolve(i))
            .collect();

        for (ti, tri) in self.triangles.iter().enumerate() {
            if !self.tri_active[ti] {
                continue;
            }
            for &vi in tri {
                let canonical = resolved[vi as usize];
                if new_index[canonical as usize] == u32::MAX {
                    let p = self.positions[canonical as usize];
                    let n = self.normals[canonical as usize];
                    let uv = self.uvs[canonical as usize];
                    let t = self.tangents[canonical as usize];

                    vertices.push(Vertex {
                        position: [p[0] as f32, p[1] as f32, p[2] as f32],
                        normal: [n[0] as f32, n[1] as f32, n[2] as f32],
                        tangent: [t[0] as f32, t[1] as f32, t[2] as f32, t[3] as f32],
                        uv: [uv[0] as f32, uv[1] as f32],
                    });
                    new_index[canonical as usize] = next_idx;
                    next_idx += 1;
                }
            }
        }

        let mut indices = Vec::new();
        for (ti, tri) in self.triangles.iter().enumerate() {
            if !self.tri_active[ti] {
                continue;
            }
            let i0 = new_index[resolved[tri[0] as usize] as usize];
            let i1 = new_index[resolved[tri[1] as usize] as usize];
            let i2 = new_index[resolved[tri[2] as usize] as usize];

            // Skip degenerate
            if i0 == i1 || i1 == i2 || i0 == i2 {
                continue;
            }
            indices.push(i0);
            indices.push(i1);
            indices.push(i2);
        }

        let bbox =
            BoundingBox::from_positions(&vertices.iter().map(|v| v.position).collect::<Vec<_>>());

        MeshData {
            vertices,
            indices,
            materials: original.materials.clone(),
            bounding_box: bbox,
        }
    }

    /// Resolve remap chains to find the canonical vertex.
    fn resolve(&self, mut v: u32) -> u32 {
        let mut seen = 0;
        while self.remap[v as usize] != v && seen < self.positions.len() {
            v = self.remap[v as usize];
            seen += 1;
        }
        v
    }
}

fn lerp2(a: &[f64; 2], b: &[f64; 2], t: f64) -> [f64; 2] {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t]
}

fn lerp3(a: &[f64; 3], b: &[f64; 3], t: f64) -> [f64; 3] {
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
    ]
}

fn lerp4(a: &[f64; 4], b: &[f64; 4], t: f64) -> [f64; 4] {
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
        a[3] + (b[3] - a[3]) * t,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::algorithms::mesh_builder::primitives::{sphere, SphereParams};
    use crate::types::{MaterialData, Vertex};

    fn make_quad_mesh() -> MeshData {
        // 2x2 grid = 9 vertices, 8 triangles
        let mut vertices = Vec::new();
        for j in 0..3 {
            for i in 0..3 {
                vertices.push(Vertex {
                    position: [i as f32, 0.0, j as f32],
                    normal: [0.0, 1.0, 0.0],
                    tangent: [1.0, 0.0, 0.0, 1.0],
                    uv: [i as f32 / 2.0, j as f32 / 2.0],
                });
            }
        }
        let mut indices = Vec::new();
        for j in 0..2u32 {
            for i in 0..2u32 {
                let a = j * 3 + i;
                let b = a + 1;
                let c = (j + 1) * 3 + i;
                let d = c + 1;
                indices.extend_from_slice(&[a, c, b, b, c, d]);
            }
        }

        let bbox =
            BoundingBox::from_positions(&vertices.iter().map(|v| v.position).collect::<Vec<_>>());
        MeshData {
            vertices,
            indices,
            materials: vec![MaterialData::default()],
            bounding_box: bbox,
        }
    }

    #[test]
    fn reduces_triangle_count() {
        let mesh = make_quad_mesh();
        assert_eq!(mesh.triangle_count(), 8);

        let simplified = simplify(&mesh, 4);
        assert!(
            simplified.triangle_count() <= 4,
            "should reduce to <=4 tris, got {}",
            simplified.triangle_count()
        );
        assert!(
            simplified.triangle_count() > 0,
            "should have at least 1 triangle"
        );
    }

    #[test]
    fn preserves_validity() {
        let mesh = make_quad_mesh();
        let simplified = simplify(&mesh, 4);
        assert!(
            simplified.validate().is_ok(),
            "simplified mesh should be valid"
        );
    }

    #[test]
    fn sphere_target_within_tolerance() {
        let mesh = sphere(&SphereParams {
            radius: 1.0,
            segments: 16,
            rings: 12,
        });
        let original_tris = mesh.triangle_count();
        let target = original_tris / 2;

        let simplified = simplify(&mesh, target);
        assert!(simplified.validate().is_ok());

        // Within 10% of target (or below target)
        let upper = (target as f32 * 1.1) as usize;
        assert!(
            simplified.triangle_count() <= upper,
            "should be within 10% of target {}, got {}",
            target,
            simplified.triangle_count()
        );
    }

    #[test]
    fn no_degenerate_triangles() {
        let mesh = sphere(&SphereParams {
            radius: 1.0,
            segments: 12,
            rings: 8,
        });
        let simplified = simplify(&mesh, mesh.triangle_count() / 4);

        for chunk in simplified.indices.chunks_exact(3) {
            assert!(
                chunk[0] != chunk[1] && chunk[1] != chunk[2] && chunk[0] != chunk[2],
                "found degenerate triangle: {:?}",
                chunk
            );
        }
    }

    #[test]
    fn target_above_current_returns_unchanged() {
        let mesh = make_quad_mesh();
        let original_tris = mesh.triangle_count();

        let result = simplify(&mesh, original_tris + 100);
        assert_eq!(result.triangle_count(), original_tris);
        assert_eq!(result.vertex_count(), mesh.vertex_count());
    }
}
