//! Rays against the triangles a game draws (ADR 0105).
//!
//! A [`RayMesh`] is a mesh's triangles under a bounding volume hierarchy, in
//! the mesh's own space. A [`RayScene`] places meshes in the world, one
//! instance for each entity that draws one, and answers where a ray first
//! meets any of them: "is there a wall between the camera and the player?",
//! "what's under the cursor?".
//!
//! The renderer builds a scene each frame from the runtime meshes uploaded
//! as ray targets (`SceneRenderer::set_ray_mesh`), so a game describes a
//! building once, to draw it, and the same triangles are what a ray meets.
//! Scripts cast through it with `raycast_meshes` and `screen_to_mesh`.
//!
//! Triangles are two-sided: a ray meets a wall from either face, so a mesh
//! that's only drawn from outside still stops a ray from inside.

use std::sync::Arc;

use crate::EntityId;

/// Triangles per leaf of the hierarchy, at most.
const LEAF: usize = 4;

/// The deepest a hierarchy may grow. A median split halves each node, so a
/// mesh would need 2^60 triangles to reach it; it bounds the cast's stack.
const MAX_DEPTH: usize = 60;

/// Where a ray meets a [`RayMesh`], in the mesh's own space.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MeshHit {
    /// How far along the ray, in units of the ray's direction.
    pub distance: f32,
    /// The triangle met, as an index into the mesh's triangles (the
    /// `indices` it was built from, in threes).
    pub triangle: u32,
    /// The triangle's unit normal, turned to face the ray's origin.
    pub normal: [f32; 3],
}

/// Where a ray meets a [`RayScene`], in the world.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RayHit {
    /// The entity drawing the mesh that was met.
    pub entity: EntityId,
    /// How far from the ray's origin, in world units.
    pub distance: f32,
    /// The world point met.
    pub point: [f32; 3],
    /// The surface's unit normal there, turned to face the ray's origin.
    pub normal: [f32; 3],
    /// The triangle met, as in [`MeshHit::triangle`].
    pub triangle: u32,
}

/// A node of the hierarchy: a box round its triangles. A leaf holds
/// `count` triangles from `first`; an inner node (`count == 0`) has its
/// children at the next index and at `first`.
#[derive(Debug, Clone, Copy)]
struct Node {
    min: [f32; 3],
    max: [f32; 3],
    first: u32,
    count: u32,
}

/// A mesh's triangles, ready to be cast against. Built once from the same
/// positions and indices a mesh is drawn with.
#[derive(Debug, Clone)]
pub struct RayMesh {
    positions: Vec<[f32; 3]>,
    /// Triangles in the hierarchy's order, each with its index in the
    /// original `indices`.
    tris: Vec<([u32; 3], u32)>,
    nodes: Vec<Node>,
}

impl RayMesh {
    /// Build from vertex positions and a triangle list (three indices a
    /// triangle). Triangles naming a vertex past the end are left out, as
    /// are those with a position that isn't finite, and so is any trailing
    /// index short of a triangle.
    pub fn new(positions: &[[f32; 3]], indices: &[u32]) -> Self {
        let n = positions.len();
        let usable = |t: &[u32; 3]| {
            t.iter()
                .all(|&i| (i as usize) < n && positions[i as usize].iter().all(|v| v.is_finite()))
        };
        let mut items: Vec<Item> = indices
            .as_chunks::<3>()
            .0
            .iter()
            .enumerate()
            .map(|(k, &t)| (t, k as u32))
            .filter(|(t, _)| usable(t))
            .map(|(t, k)| {
                let [a, b, c] = t.map(|i| positions[i as usize]);
                let centroid = std::array::from_fn(|i| (a[i] + b[i] + c[i]) / 3.0);
                Item { tri: t, index: k, centroid }
            })
            .collect();
        let mut nodes = Vec::new();
        if !items.is_empty() {
            build(positions, &mut items, 0, &mut nodes, 0);
        }
        RayMesh {
            positions: positions.to_vec(),
            tris: items.into_iter().map(|it| (it.tri, it.index)).collect(),
            nodes,
        }
    }

    /// How many triangles a ray can meet.
    pub fn triangle_count(&self) -> usize {
        self.tris.len()
    }

    /// Whether there's nothing to meet.
    pub fn is_empty(&self) -> bool {
        self.tris.is_empty()
    }

    /// The box round every triangle, as `(min, max)`. `None` when empty.
    pub fn bounds(&self) -> Option<([f32; 3], [f32; 3])> {
        self.nodes.first().map(|n| (n.min, n.max))
    }

    /// The nearest triangle the ray `origin + t·dir` meets for `t` in
    /// `0..=max`, in the mesh's own space. `dir` needn't be unit length;
    /// `distance` is in its units.
    pub fn cast(&self, origin: [f32; 3], dir: [f32; 3], max: f32) -> Option<MeshHit> {
        let root = self.nodes.first()?;
        let inv = dir.map(|d| 1.0 / d);
        let mut best = max;
        let mut hit: Option<(f32, usize)> = None;
        let mut stack = [(0u32, 0.0f32); MAX_DEPTH + 2];
        let mut top = 0;
        if let Some(t) = slab(root.min, root.max, origin, inv, best) {
            stack[0] = (0, t);
            top = 1;
        }
        while top > 0 {
            top -= 1;
            let (i, entry) = stack[top];
            if entry > best {
                continue;
            }
            let node = self.nodes[i as usize];
            if node.count > 0 {
                let first = node.first as usize;
                for k in first..first + node.count as usize {
                    let [a, b, c] = self.tris[k].0.map(|v| self.positions[v as usize]);
                    if let Some(t) = triangle(origin, dir, a, b, c, best) {
                        best = t;
                        hit = Some((t, k));
                    }
                }
                continue;
            }
            // Visit the nearer child first: push it last.
            let (l, r) = (i + 1, node.first);
            let tl = self.slab_of(l, origin, inv, best);
            let tr = self.slab_of(r, origin, inv, best);
            let mut push = |n: u32, t: Option<f32>| {
                if let Some(t) = t {
                    stack[top] = (n, t);
                    top += 1;
                }
            };
            if tl.unwrap_or(f32::INFINITY) <= tr.unwrap_or(f32::INFINITY) {
                push(r, tr);
                push(l, tl);
            } else {
                push(l, tl);
                push(r, tr);
            }
        }
        let (distance, k) = hit?;
        let [a, b, c] = self.tris[k].0.map(|v| self.positions[v as usize]);
        let mut normal = normalize(cross(sub(b, a), sub(c, a)));
        if dot(normal, dir) > 0.0 {
            normal = normal.map(|v| -v);
        }
        Some(MeshHit {
            distance,
            triangle: self.tris[k].1,
            normal,
        })
    }

    fn slab_of(&self, i: u32, origin: [f32; 3], inv: [f32; 3], best: f32) -> Option<f32> {
        let n = &self.nodes[i as usize];
        slab(n.min, n.max, origin, inv, best)
    }
}

/// A triangle while the hierarchy is built.
struct Item {
    tri: [u32; 3],
    index: u32,
    centroid: [f32; 3],
}

/// Build the node for `items` at the end of `nodes`, and its children after
/// it. `offset` is where `items` starts in the whole list.
fn build(positions: &[[f32; 3]], items: &mut [Item], offset: usize, nodes: &mut Vec<Node>, depth: usize) {
    let (mut min, mut max) = ([f32::INFINITY; 3], [f32::NEG_INFINITY; 3]);
    let (mut cmin, mut cmax) = ([f32::INFINITY; 3], [f32::NEG_INFINITY; 3]);
    for it in items.iter() {
        for v in it.tri {
            let p = positions[v as usize];
            for a in 0..3 {
                min[a] = min[a].min(p[a]);
                max[a] = max[a].max(p[a]);
            }
        }
        for a in 0..3 {
            cmin[a] = cmin[a].min(it.centroid[a]);
            cmax[a] = cmax[a].max(it.centroid[a]);
        }
    }
    let here = nodes.len();
    nodes.push(Node {
        min,
        max,
        first: offset as u32,
        count: items.len() as u32,
    });
    let axis = (0..3)
        .max_by(|&a, &b| (cmax[a] - cmin[a]).total_cmp(&(cmax[b] - cmin[b])))
        .unwrap_or(0);
    // A leaf when small, too deep, or when every centroid is at one point
    // (no split would separate them).
    if items.len() <= LEAF || depth >= MAX_DEPTH || cmax[axis] - cmin[axis] <= 0.0 {
        return;
    }
    let mid = items.len() / 2;
    items.select_nth_unstable_by(mid, |a, b| a.centroid[axis].total_cmp(&b.centroid[axis]));
    let (left, right) = items.split_at_mut(mid);
    build(positions, left, offset, nodes, depth + 1);
    let right_at = nodes.len() as u32;
    build(positions, right, offset + mid, nodes, depth + 1);
    nodes[here].first = right_at;
    nodes[here].count = 0;
}

/// Where the ray enters the box, if it does before `best`: the slab test.
/// A zero direction component gives an infinite inverse; where that meets a
/// face edge-on (0 × ∞) the axis is skipped, which only ever lets a box in.
fn slab(min: [f32; 3], max: [f32; 3], o: [f32; 3], inv: [f32; 3], best: f32) -> Option<f32> {
    let (mut t0, mut t1) = (0.0f32, best);
    for a in 0..3 {
        let ta = (min[a] - o[a]) * inv[a];
        let tb = (max[a] - o[a]) * inv[a];
        // `f32::max`/`min` ignore a NaN, so the edge-on case drops out.
        t0 = t0.max(ta.min(tb));
        t1 = t1.min(ta.max(tb));
        if t0 > t1 {
            return None;
        }
    }
    Some(t0)
}

/// Where the ray meets the triangle (a, b, c), from either face, if it does
/// in `0..best`: Möller–Trumbore.
fn triangle(o: [f32; 3], d: [f32; 3], a: [f32; 3], b: [f32; 3], c: [f32; 3], best: f32) -> Option<f32> {
    let e1 = sub(b, a);
    let e2 = sub(c, a);
    let p = cross(d, e2);
    let det = dot(e1, p);
    if det.abs() < f32::MIN_POSITIVE || !det.is_finite() {
        return None;
    }
    let inv = 1.0 / det;
    let s = sub(o, a);
    let u = dot(s, p) * inv;
    if !(0.0..=1.0).contains(&u) {
        return None;
    }
    let q = cross(s, e1);
    let v = dot(d, q) * inv;
    if v < 0.0 || u + v > 1.0 {
        return None;
    }
    let t = dot(e2, q) * inv;
    (t >= 0.0 && t < best).then_some(t)
}

/// A mesh placed in the world by an entity's transform.
#[derive(Debug, Clone)]
struct Instance {
    entity: EntityId,
    mesh: Arc<RayMesh>,
    /// World to the mesh's space; `None` when the transform is the identity.
    to_local: Option<[[f32; 4]; 4]>,
    min: [f32; 3],
    max: [f32; 3],
}

/// Meshes placed in the world, to cast rays against. Cheap to clone: the
/// meshes are shared.
#[derive(Debug, Clone, Default)]
pub struct RayScene {
    instances: Vec<Instance>,
}

impl RayScene {
    pub fn new() -> Self {
        Self::default()
    }

    /// Place `mesh` in the world with `entity`'s world matrix (column-major,
    /// `m[col][row]`, as the renderer draws it). An empty mesh, or a matrix
    /// with no inverse (a zero scale), adds nothing.
    pub fn add(&mut self, entity: EntityId, mesh: Arc<RayMesh>, model: [[f32; 4]; 4]) {
        let Some((lo, hi)) = mesh.bounds() else {
            return;
        };
        let identity = model == IDENTITY;
        let to_local = if identity {
            None
        } else {
            match crate::projection::mat4_inverse_f64(&model) {
                Some(inv) => Some(inv.map(|col| col.map(|v| v as f32))),
                None => return,
            }
        };
        let (mut min, mut max) = ([f32::INFINITY; 3], [f32::NEG_INFINITY; 3]);
        for k in 0..8 {
            let corner = [
                if k & 1 == 0 { lo[0] } else { hi[0] },
                if k & 2 == 0 { lo[1] } else { hi[1] },
                if k & 4 == 0 { lo[2] } else { hi[2] },
            ];
            let w = point(&model, corner);
            for a in 0..3 {
                min[a] = min[a].min(w[a]);
                max[a] = max[a].max(w[a]);
            }
        }
        self.instances.push(Instance {
            entity,
            mesh,
            to_local,
            min,
            max,
        });
    }

    /// How many placed meshes there are.
    pub fn len(&self) -> usize {
        self.instances.len()
    }

    pub fn is_empty(&self) -> bool {
        self.instances.is_empty()
    }

    /// The nearest place the ray from `origin` along `dir` meets a mesh,
    /// within `max` world units (`f32::INFINITY` for no limit). `dir` needn't
    /// be unit length. `None` for a zero `dir`.
    pub fn cast(&self, origin: [f32; 3], dir: [f32; 3], max: f32) -> Option<RayHit> {
        self.cast_filtered(origin, dir, max, |_| true)
    }

    /// As [`cast`](Self::cast), meeting only the meshes of entities `keep`
    /// passes (to look past the player's own model, say).
    pub fn cast_filtered(
        &self,
        origin: [f32; 3],
        dir: [f32; 3],
        max: f32,
        mut keep: impl FnMut(EntityId) -> bool,
    ) -> Option<RayHit> {
        let len = dot(dir, dir).sqrt();
        if len.is_nan() || len <= 0.0 || len.is_infinite() {
            return None;
        }
        let dir = dir.map(|v| v / len);
        let inv = dir.map(|d| 1.0 / d);
        let mut best = max;
        let mut found: Option<(&Instance, MeshHit)> = None;
        for inst in &self.instances {
            if slab(inst.min, inst.max, origin, inv, best).is_none() || !keep(inst.entity) {
                continue;
            }
            // An affine map keeps the ray's parameter: the local ray at t is
            // the world ray at t, so distances compare across instances.
            let hit = match &inst.to_local {
                None => inst.mesh.cast(origin, dir, best),
                Some(m) => inst.mesh.cast(point(m, origin), vector(m, dir), best),
            };
            if let Some(hit) = hit {
                best = hit.distance;
                found = Some((inst, hit));
            }
        }
        let (inst, hit) = found?;
        let normal = match &inst.to_local {
            None => hit.normal,
            // Normals go by the inverse transpose: n_world = (M⁻¹)ᵀ n.
            Some(m) => {
                let n = std::array::from_fn(|r| (0..3).map(|k| m[r][k] * hit.normal[k]).sum());
                let n = normalize(n);
                if dot(n, dir) > 0.0 {
                    n.map(|v| -v)
                } else {
                    n
                }
            }
        };
        Some(RayHit {
            entity: inst.entity,
            distance: hit.distance,
            point: std::array::from_fn(|a| origin[a] + dir[a] * hit.distance),
            normal,
            triangle: hit.triangle,
        })
    }

    /// The first mesh met on the way from `from` to `to`; its `distance` is
    /// from `from`. `None` when the way is clear, or the two are one point.
    pub fn segment(&self, from: [f32; 3], to: [f32; 3]) -> Option<RayHit> {
        let d = sub(to, from);
        self.cast(from, d, dot(d, d).sqrt())
    }
}

const IDENTITY: [[f32; 4]; 4] = [
    [1.0, 0.0, 0.0, 0.0],
    [0.0, 1.0, 0.0, 0.0],
    [0.0, 0.0, 1.0, 0.0],
    [0.0, 0.0, 0.0, 1.0],
];

/// `m · (p, 1)`, for an affine `m`.
fn point(m: &[[f32; 4]; 4], p: [f32; 3]) -> [f32; 3] {
    std::array::from_fn(|r| m[0][r] * p[0] + m[1][r] * p[1] + m[2][r] * p[2] + m[3][r])
}

/// `m · (v, 0)`.
fn vector(m: &[[f32; 4]; 4], v: [f32; 3]) -> [f32; 3] {
    std::array::from_fn(|r| m[0][r] * v[0] + m[1][r] * v[1] + m[2][r] * v[2])
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn normalize(a: [f32; 3]) -> [f32; 3] {
    let l = dot(a, a).sqrt();
    if l > 0.0 {
        a.map(|v| v / l)
    } else {
        a
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A unit box from `lo` to `lo + size`, twelve triangles wound outward.
    fn cube(lo: [f32; 3], size: [f32; 3]) -> (Vec<[f32; 3]>, Vec<u32>) {
        let p = (0..8)
            .map(|k| {
                [
                    lo[0] + if k & 1 == 0 { 0.0 } else { size[0] },
                    lo[1] + if k & 2 == 0 { 0.0 } else { size[1] },
                    lo[2] + if k & 4 == 0 { 0.0 } else { size[2] },
                ]
            })
            .collect();
        let quads = [[0, 2, 3, 1], [4, 5, 7, 6], [0, 1, 5, 4], [2, 6, 7, 3], [0, 4, 6, 2], [1, 3, 7, 5]];
        let i = quads.iter().flat_map(|q| [q[0], q[1], q[2], q[0], q[2], q[3]]).collect();
        (p, i)
    }

    fn mesh(lo: [f32; 3], size: [f32; 3]) -> Arc<RayMesh> {
        let (p, i) = cube(lo, size);
        Arc::new(RayMesh::new(&p, &i))
    }

    fn close(a: [f32; 3], b: [f32; 3]) -> bool {
        (0..3).all(|k| (a[k] - b[k]).abs() < 1e-4)
    }

    #[test]
    fn a_ray_meets_the_near_face_with_its_normal_toward_the_origin() {
        let m = mesh([1.0, -1.0, -1.0], [2.0, 2.0, 2.0]);
        let hit = m.cast([0.0, 0.0, 0.0], [1.0, 0.0, 0.0], f32::INFINITY).unwrap();
        assert!((hit.distance - 1.0).abs() < 1e-5, "{hit:?}");
        assert!(close(hit.normal, [-1.0, 0.0, 0.0]), "{hit:?}");
        assert!(m.cast([0.0, 0.0, 0.0], [-1.0, 0.0, 0.0], f32::INFINITY).is_none());
        assert!(m.cast([0.0, 0.0, 0.0], [1.0, 0.0, 0.0], 0.5).is_none(), "past max");
    }

    #[test]
    fn from_inside_the_far_face_is_met() {
        let m = mesh([-1.0, -1.0, -1.0], [2.0, 2.0, 2.0]);
        let hit = m.cast([0.0, 0.0, 0.0], [0.0, 0.0, 1.0], f32::INFINITY).unwrap();
        assert!((hit.distance - 1.0).abs() < 1e-5);
        assert!(close(hit.normal, [0.0, 0.0, -1.0]), "{hit:?}");
    }

    #[test]
    fn the_hierarchy_finds_what_brute_force_does() {
        // A street of 200 boxes, which forces many levels.
        let (mut p, mut idx) = (Vec::new(), Vec::new());
        for k in 0..200 {
            let (cp, ci) = cube([k as f32 * 3.0, 0.0, (k % 7) as f32], [2.0, 1.0 + (k % 5) as f32, 2.0]);
            let base = p.len() as u32;
            p.extend(cp);
            idx.extend(ci.into_iter().map(|i| i + base));
        }
        let m = RayMesh::new(&p, &idx);
        assert_eq!(m.triangle_count(), 200 * 12);
        let mut state = 12345u32;
        let mut rnd = || {
            state = state.wrapping_mul(1664525).wrapping_add(1013904223);
            (state >> 8) as f32 / (1u32 << 24) as f32
        };
        let mut met = 0;
        for _ in 0..500 {
            let o = [rnd() * 600.0, rnd() * 8.0 - 1.0, rnd() * 12.0 - 3.0];
            let d = [rnd() - 0.5, rnd() - 0.5, rnd() - 0.5];
            let brute = idx
                .as_chunks::<3>()
                .0
                .iter()
                .filter_map(|t| triangle(o, d, p[t[0] as usize], p[t[1] as usize], p[t[2] as usize], f32::INFINITY))
                .fold(None, |acc: Option<f32>, t| Some(acc.map_or(t, |a| a.min(t))));
            let got = m.cast(o, d, f32::INFINITY).map(|h| h.distance);
            match (brute, got) {
                (None, None) => {}
                (Some(b), Some(g)) => {
                    met += 1;
                    assert!((b - g).abs() < 1e-4, "{o:?} {d:?}: {b} vs {g}");
                }
                other => panic!("{o:?} {d:?}: {other:?}"),
            }
        }
        assert!(met > 50, "only {met} rays met anything");
    }

    #[test]
    fn bad_triangles_are_left_out() {
        let p = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [f32::NAN, 0.0, 0.0]];
        let m = RayMesh::new(&p, &[0, 1, 2, 0, 1, 9, 0, 1, 3, 2]);
        assert_eq!(m.triangle_count(), 1);
        assert!(RayMesh::new(&p, &[]).is_empty());
        assert!(RayMesh::new(&p, &[]).cast([0.0; 3], [0.0, 0.0, 1.0], 9.0).is_none());
    }

    #[test]
    fn a_hit_names_the_triangle_it_was_given_as() {
        // Two triangles; the second, in the plane z = 2, is met first from below.
        let p = [
            [-1.0, -1.0, 5.0],
            [1.0, -1.0, 5.0],
            [0.0, 1.0, 5.0],
            [-1.0, -1.0, 2.0],
            [1.0, -1.0, 2.0],
            [0.0, 1.0, 2.0],
        ];
        let m = RayMesh::new(&p, &[0, 1, 2, 3, 4, 5]);
        let hit = m.cast([0.0, 0.0, 0.0], [0.0, 0.0, 1.0], f32::INFINITY).unwrap();
        assert_eq!(hit.triangle, 1);
        assert!((hit.distance - 2.0).abs() < 1e-5);
    }

    #[test]
    fn the_scene_meets_the_nearest_instance_and_says_whose() {
        let mut scene = RayScene::new();
        let (near, far) = (EntityId::from_raw(7), EntityId::from_raw(8));
        scene.add(far, mesh([10.0, -1.0, -1.0], [1.0, 2.0, 2.0]), IDENTITY);
        scene.add(near, mesh([4.0, -1.0, -1.0], [1.0, 2.0, 2.0]), IDENTITY);
        let hit = scene.cast([0.0, 0.0, 0.0], [2.0, 0.0, 0.0], f32::INFINITY).unwrap();
        assert_eq!(hit.entity, near);
        assert!((hit.distance - 4.0).abs() < 1e-5, "distance is in world units: {hit:?}");
        assert!(close(hit.point, [4.0, 0.0, 0.0]));
        let past = scene.cast_filtered([0.0, 0.0, 0.0], [1.0, 0.0, 0.0], f32::INFINITY, |e| e != near);
        assert_eq!(past.unwrap().entity, far);
        assert!(scene.cast([0.0, 0.0, 0.0], [0.0; 3], 9.0).is_none());
    }

    #[test]
    fn a_placed_mesh_is_met_where_its_transform_puts_it() {
        // A unit cube at the origin, scaled ×2 on x, turned 90° about y
        // (x → -z) and moved to (0, 0, -10).
        let m = mesh([-0.5, -0.5, -0.5], [1.0, 1.0, 1.0]);
        let model = [
            [0.0, 0.0, -2.0, 0.0], // x axis → -z, scaled 2
            [0.0, 1.0, 0.0, 0.0],
            [1.0, 0.0, 0.0, 0.0], // z axis → x
            [0.0, 0.0, -10.0, 1.0],
        ];
        let mut scene = RayScene::new();
        scene.add(EntityId::from_raw(1), m, model);
        // Along -z from the origin: the box spans z -11..-9.
        let hit = scene.cast([0.0, 0.0, 0.0], [0.0, 0.0, -1.0], f32::INFINITY).unwrap();
        assert!((hit.distance - 9.0).abs() < 1e-4, "{hit:?}");
        assert!(close(hit.normal, [0.0, 0.0, 1.0]), "{hit:?}");
        // Across x at z = -10: it spans x -0.5..0.5.
        let hit = scene.cast([-5.0, 0.0, -10.0], [1.0, 0.0, 0.0], f32::INFINITY).unwrap();
        assert!((hit.distance - 4.5).abs() < 1e-4, "{hit:?}");
        assert!(close(hit.normal, [-1.0, 0.0, 0.0]), "{hit:?}");
        // A zero scale has no inverse: nothing is placed.
        let mut flat = RayScene::new();
        let mut squashed = IDENTITY;
        squashed[1][1] = 0.0;
        flat.add(EntityId::from_raw(2), mesh([0.0; 3], [1.0; 3]), squashed);
        assert!(flat.is_empty());
    }

    #[test]
    fn a_segment_stops_at_the_wall_or_runs_clear() {
        let mut scene = RayScene::new();
        scene.add(EntityId::from_raw(1), mesh([1.0, 0.0, -5.0], [1.0, 3.0, 10.0]), IDENTITY);
        let hit = scene.segment([0.0, 1.5, 0.0], [4.0, 1.5, 0.0]).unwrap();
        assert!((hit.distance - 1.0).abs() < 1e-5);
        assert!(scene.segment([0.0, 3.5, 0.0], [4.0, 3.6, 0.0]).is_none(), "over it");
        assert!(scene.segment([0.0, 1.5, 0.0], [0.9, 1.5, 0.0]).is_none(), "short of it");
        assert!(scene.segment([0.0, 1.5, 0.0], [0.0, 1.5, 0.0]).is_none());
    }
}
