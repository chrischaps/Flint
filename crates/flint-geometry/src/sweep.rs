//! A profile swept along a sampled spline (#750): a road, a wall, a rail, a
//! pipe, a river bank.
//!
//! A [`Profile`] is a polygon in the path's own frame, `[right, up]` metres
//! from its centre, closed (a solid: a box, a tube, with end caps) or open
//! (a surface: a bank, a gutter, no caps). [`sweep`] places it at every
//! sample and joins neighbouring samples with a quad per profile edge.
//! [`Profile::rect`] is the box `spline_mesh` always made, and sweeps to the
//! same triangles it did.
//!
//! - **Normals.** A flat profile gives each edge one normal across its
//!   quad, the average of the two samples' frames. A smooth one gives each
//!   profile point the average of its two edges' normals, in its own
//!   sample's frame, so a tube shades round.
//! - **Texture.** `v` is metres along the sweep over `texture_length`,
//!   measured along the profile's centre path (the spline moved by
//!   `offset`), so a wall inside a bend keeps its texture's proportions. On
//!   a loop the length is nudged so a whole number of repeats meets at the
//!   seam. A flat profile's `u` runs 0..1 across each edge in the order the
//!   points are listed, except that an edge listed downward is read upward,
//!   so the walls on both sides of a road show their texture standing up.
//!   A smooth profile's `u` runs once around the whole profile.
//! - **Caps.** A closed profile is capped where the sweep starts and stops:
//!   the ends of an open spline and both edges of every gap. The cap is the
//!   profile polygon triangulated (ear clipping, a fan for a convex one),
//!   its `uv` the profile's bounding box.
//! - **Gaps.** Samples inside a `gap_after` range make no geometry.
//! - **Stripes.** An optional second colour alternates with the first in
//!   bands along the sweep (a kerb), as vertex colour.
//!
//! The output is in world space, as the samples are. Its triangles are also
//! the collider's: [`SweptMesh::triangles`].

use flint_core::Vec3;

use crate::spline::{self, SplineSample};

/// The cross-section swept along a path: points `[right, up]` in metres
/// from the path's centre.
#[derive(Debug, Clone, PartialEq)]
pub struct Profile {
    points: Vec<[f32; 2]>,
    closed: bool,
    smooth: bool,
}

impl Profile {
    /// A box `width` across and `height` tall, centred on the path: the
    /// shape `spline_mesh` has always swept, with flat sides.
    pub fn rect(width: f32, height: f32) -> Self {
        let hw = width / 2.0;
        let hh = height / 2.0;
        Profile {
            // top-left, top-right, bottom-right, bottom-left: clockwise
            // seen with right to the right and up up
            points: vec![[-hw, hh], [hw, hh], [hw, -hh], [-hw, -hh]],
            closed: true,
            smooth: false,
        }
    }

    /// A profile of these points. A closed one needs three or more and is
    /// a solid, its faces looking outward whichever way round it is listed.
    /// An open one needs two or more and is a surface whose faces look to
    /// the left of the way it is listed: list a road's points from left to
    /// right and it faces up. `smooth` shades it round instead of faceted.
    pub fn new(points: Vec<[f32; 2]>, closed: bool, smooth: bool) -> Result<Self, String> {
        let need = if closed { 3 } else { 2 };
        if points.len() < need {
            return Err(format!(
                "a {} profile needs at least {need} points, got {}",
                if closed { "closed" } else { "open" },
                points.len()
            ));
        }
        if let Some(i) = points
            .iter()
            .position(|p| !(p[0].is_finite() && p[1].is_finite()))
        {
            return Err(format!("profile point {i} is not a finite number"));
        }
        let edges = if closed {
            points.len()
        } else {
            points.len() - 1
        };
        for i in 0..edges {
            let (a, b) = (points[i], points[(i + 1) % points.len()]);
            if a == b {
                return Err(format!(
                    "profile points {i} and {} are the same place, [{}, {}]",
                    (i + 1) % points.len(),
                    a[0],
                    a[1]
                ));
            }
        }
        let mut points = points;
        if closed {
            let area = signed_area(&points);
            if area.abs() < 1e-12 {
                return Err("a closed profile encloses no area: its points are in a line".into());
            }
            // Clockwise, so every edge's left-hand normal looks out
            if area > 0.0 {
                points.reverse();
            }
        }
        Ok(Profile {
            points,
            closed,
            smooth,
        })
    }

    /// The points, a closed profile's clockwise.
    pub fn points(&self) -> &[[f32; 2]] {
        &self.points
    }

    /// A solid with end caps, or an open surface.
    pub fn is_closed(&self) -> bool {
        self.closed
    }

    /// Shaded round rather than faceted.
    pub fn is_smooth(&self) -> bool {
        self.smooth
    }

    /// How wide it is across the path, in metres: a square texture tile's
    /// length.
    pub fn width(&self) -> f32 {
        let (lo, hi) = self
            .points
            .iter()
            .fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), p| {
                (lo.min(p[0]), hi.max(p[0]))
            });
        hi - lo
    }

    fn edge_count(&self) -> usize {
        if self.closed {
            self.points.len()
        } else {
            self.points.len() - 1
        }
    }

    /// The ends of edge `e`.
    fn edge(&self, e: usize) -> (usize, usize) {
        (e, (e + 1) % self.points.len())
    }

    /// The unit normal of edge `e`, to the left of its direction. An edge
    /// along an axis gets an exact axis, so a box's normals are the frame's
    /// own vectors, bit for bit.
    fn edge_normal(&self, e: usize) -> [f32; 2] {
        let (a, b) = self.edge(e);
        let (a, b) = (self.points[a], self.points[b]);
        let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
        if dx == 0.0 {
            [-dy.signum(), 0.0]
        } else if dy == 0.0 {
            [0.0, dx.signum()]
        } else {
            let len = (dx * dx + dy * dy).sqrt();
            [-dy / len, dx / len]
        }
    }

    /// Each point's normal for smooth shading: its edges' normals averaged.
    fn point_normals(&self) -> Vec<[f32; 2]> {
        let n = self.points.len();
        let edges = self.edge_count();
        (0..n)
            .map(|i| {
                let before = if i > 0 {
                    Some(i - 1)
                } else if self.closed {
                    Some(n - 1)
                } else {
                    None
                };
                let after = (i < edges).then_some(i);
                let sum = [before, after]
                    .into_iter()
                    .flatten()
                    .map(|e| self.edge_normal(e))
                    .fold([0.0f32; 2], |s, m| [s[0] + m[0], s[1] + m[1]]);
                let len = (sum[0] * sum[0] + sum[1] * sum[1]).sqrt();
                if len < 1e-6 {
                    // A spike folding back on itself: either edge's will do
                    self.edge_normal(after.or(before).unwrap_or(0))
                } else {
                    [sum[0] / len, sum[1] / len]
                }
            })
            .collect()
    }

    /// `u` at each end of edge `e`.
    fn edge_u(&self, e: usize, around: &[f32]) -> (f32, f32) {
        let (a, b) = self.edge(e);
        if self.smooth {
            // Once around: the closing edge ends at 1, not back at 0
            (around[a], around[e + 1])
        } else if self.points[b][1] < self.points[a][1] {
            // Listed downward, read upward
            (1.0, 0.0)
        } else {
            (0.0, 1.0)
        }
    }

    /// The fraction of the way round at each point, with one more entry,
    /// 1, for a closed profile's return to its first point.
    fn around(&self) -> Vec<f32> {
        let mut dist = vec![0.0f32];
        for e in 0..self.edge_count() {
            let (a, b) = self.edge(e);
            let (pa, pb) = (self.points[a], self.points[b]);
            let d = ((pb[0] - pa[0]).powi(2) + (pb[1] - pa[1]).powi(2)).sqrt();
            dist.push(dist[e] + d);
        }
        let total = dist.last().copied().unwrap_or(0.0).max(1e-6);
        dist.into_iter().map(|d| d / total).collect()
    }

    /// The cap's triangles, over the points in reverse order (so the cap's
    /// vertex `k` is point `n - 1 - k`).
    fn cap_triangles(&self) -> Vec<[u32; 3]> {
        let reversed: Vec<[f32; 2]> = self.points.iter().rev().copied().collect();
        triangulate(&reversed)
    }
}

/// The polygon's signed area: positive when counter-clockwise.
fn signed_area(points: &[[f32; 2]]) -> f32 {
    let n = points.len();
    let mut sum = 0.0f64;
    for i in 0..n {
        let (a, b) = (points[i], points[(i + 1) % n]);
        sum += a[0] as f64 * b[1] as f64 - b[0] as f64 * a[1] as f64;
    }
    (sum * 0.5) as f32
}

/// Triangles covering a counter-clockwise polygon, by ear clipping. The
/// search for an ear starts at the second point, so a convex polygon comes
/// out as a fan from its first. A polygon with no ear left (it crosses
/// itself) is finished as a fan.
fn triangulate(points: &[[f32; 2]]) -> Vec<[u32; 3]> {
    let cross = |o: [f32; 2], a: [f32; 2], b: [f32; 2]| {
        (a[0] - o[0]) * (b[1] - o[1]) - (a[1] - o[1]) * (b[0] - o[0])
    };
    let inside = |p: [f32; 2], a: [f32; 2], b: [f32; 2], c: [f32; 2]| {
        cross(a, b, p) >= 0.0 && cross(b, c, p) >= 0.0 && cross(c, a, p) >= 0.0
    };
    let mut left: Vec<usize> = (0..points.len()).collect();
    let mut tris = Vec::with_capacity(points.len().saturating_sub(2));
    'clip: while left.len() > 3 {
        let len = left.len();
        for step in 0..len {
            let i = (1 + step) % len;
            let (prev, cur, next) = (left[(i + len - 1) % len], left[i], left[(i + 1) % len]);
            let (a, b, c) = (points[prev], points[cur], points[next]);
            if cross(a, b, c) <= 0.0 {
                continue; // reflex, or a straight run
            }
            let blocked = left
                .iter()
                .filter(|&&k| k != prev && k != cur && k != next)
                .any(|&k| inside(points[k], a, b, c));
            if blocked {
                continue;
            }
            tris.push([prev as u32, cur as u32, next as u32]);
            left.remove(i);
            continue 'clip;
        }
        // No ear: the polygon crosses itself. A fan covers what's left
        for k in 1..left.len() - 1 {
            tris.push([left[0] as u32, left[k] as u32, left[k + 1] as u32]);
        }
        return tris;
    }
    if left.len() == 3 {
        tris.push([left[0] as u32, left[1] as u32, left[2] as u32]);
    }
    tris
}

/// The path a profile is swept along.
#[derive(Debug, Clone, Copy)]
pub struct SweepPath<'a> {
    /// The spline's samples, as [`crate::spline`] makes them.
    pub samples: &'a [SplineSample],
    /// A loop: the last sample joins the first.
    pub closed: bool,
    /// The spacing the samples were made at, in metres (the stripes count
    /// segments in it).
    pub spacing: f32,
    /// The `t` ranges that make no geometry ([`spline::compute_gap_ranges`]).
    pub gaps: &'a [(f32, f32)],
}

/// Two colours alternating along the sweep.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Stripes {
    /// The colour of every other band.
    pub color: [f32; 4],
    /// The length of one band, in metres along the sweep.
    pub width: f32,
}

/// How a swept mesh sits on its path and looks.
#[derive(Debug, Clone, PartialEq)]
pub struct SweepStyle {
    /// `[right, up]`: where the profile's centre sits, in the path's frame.
    pub offset: [f32; 2],
    /// Vertex colour.
    pub color: [f32; 4],
    /// Bands of a second colour, or none.
    pub stripes: Option<Stripes>,
    /// Metres of sweep per texture repeat (above 0).
    pub texture_length: f32,
}

/// A vertex of a swept mesh, in world space.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SweptVertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub color: [f32; 4],
    pub uv: [f32; 2],
}

/// A swept mesh: every face's own vertices, so normals stay per face.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SweptMesh {
    pub vertices: Vec<SweptVertex>,
    /// Triangles, counter-clockwise seen from outside.
    pub indices: Vec<u32>,
}

impl SweptMesh {
    /// Nothing was swept (a path of one sample, or all of it in a gap).
    pub fn is_empty(&self) -> bool {
        self.indices.is_empty()
    }

    /// Every vertex's position, for a collider.
    pub fn positions(&self) -> Vec<[f32; 3]> {
        self.vertices.iter().map(|v| v.position).collect()
    }

    /// The triangles as index triples, for a collider.
    pub fn triangles(&self) -> Vec<[u32; 3]> {
        self.indices.as_chunks::<3>().0.to_vec()
    }
}

/// One piece of a sweep split into chunks.
#[derive(Debug, Clone, PartialEq)]
pub struct SweptChunk {
    /// Its place among all the chunks, gaps counted.
    pub index: usize,
    /// The spline `t` of its first and last samples.
    pub t_start: f32,
    pub t_end: f32,
    pub mesh: SweptMesh,
}

/// Sweep `profile` along the whole of `path`.
pub fn sweep(path: &SweepPath, profile: &Profile, style: &SweepStyle) -> SweptMesh {
    let v = sweep_v_coords(
        path.samples,
        path.closed,
        style.offset,
        style.texture_length,
    );
    sweep_samples(path.samples, path.closed, path, 0, profile, style, &v)
}

/// Sweep `profile` along `path` in pieces of `segments` segments each, for
/// a long path drawn and culled piece by piece. Texture `v` and the stripes
/// run on across the pieces, so they meet seamlessly. A piece wholly in a
/// gap, or with nothing in it, is left out.
pub fn sweep_chunks(
    path: &SweepPath,
    profile: &Profile,
    style: &SweepStyle,
    segments: usize,
) -> Vec<SweptChunk> {
    let samples = path.samples;
    let n = samples.len();
    if n < 2 || segments == 0 {
        return Vec::new();
    }
    let v_coords = sweep_v_coords(samples, path.closed, style.offset, style.texture_length);
    let num_segs = if path.closed { n } else { n - 1 };
    let num_chunks = num_segs.div_ceil(segments);
    let mut chunks = Vec::new();
    for index in 0..num_chunks {
        let seg_start = index * segments;
        let seg_end = ((index + 1) * segments).min(num_segs);
        // One more sample than segments; a loop's last chunk wraps to 0
        let sample_end = if path.closed {
            seg_end + 1
        } else {
            (seg_end + 1).min(n)
        };
        let chunk_samples: Vec<SplineSample> = (seg_start..sample_end)
            .map(|i| samples[i % n].clone())
            .collect();
        if chunk_samples.len() < 2 {
            continue;
        }
        let all_in_gap = (seg_start..seg_end).all(|seg| {
            let i = if path.closed { seg % n } else { seg };
            spline::is_in_gap(samples[i].t, path.gaps)
        });
        if all_in_gap {
            continue;
        }
        let chunk_v = &v_coords[seg_start..seg_start + chunk_samples.len()];
        // Each piece is an open run of the path
        let mesh = sweep_samples(
            &chunk_samples,
            false,
            path,
            seg_start,
            profile,
            style,
            chunk_v,
        );
        if mesh.is_empty() {
            continue;
        }
        chunks.push(SweptChunk {
            index,
            t_start: chunk_samples.first().unwrap().t,
            t_end: chunk_samples.last().unwrap().t,
            mesh,
        });
    }
    chunks
}

/// Texture `v` at each sample: distance along the profile's centre path
/// (the spline moved by `offset`) over `texture_length`. A closed spline
/// gets one extra entry, the full loop, for the segment that wraps back to
/// sample 0, and its length is adjusted so the loop holds a whole number of
/// repeats and the texture meets itself at the seam.
pub fn sweep_v_coords(
    samples: &[SplineSample],
    closed: bool,
    offset: [f32; 2],
    texture_length: f32,
) -> Vec<f32> {
    let n = samples.len();
    if n == 0 {
        return Vec::new();
    }
    let centre = |s: &SplineSample| s.position + s.right * offset[0] + s.up * offset[1];
    let mut dist = Vec::with_capacity(n + 1);
    dist.push(0.0f32);
    for i in 1..n {
        let d = (centre(&samples[i]) - centre(&samples[i - 1])).length();
        dist.push(dist[i - 1] + d);
    }
    let mut length = texture_length;
    if closed {
        let total = dist[n - 1] + (centre(&samples[0]) - centre(&samples[n - 1])).length();
        dist.push(total);
        let repeats = (total / texture_length).round().max(1.0);
        length = total / repeats;
    }
    dist.into_iter().map(|d| d / length).collect()
}

/// `a * x + b * y`, leaving out a term whose factor is exactly 0 so an
/// axis edge's normal is its frame vector to the bit.
fn combine(a: Vec3, x: f32, b: Vec3, y: f32) -> Vec3 {
    if x == 0.0 {
        b * y
    } else if y == 0.0 {
        a * x
    } else {
        a * x + b * y
    }
}

/// The sweep over `samples` (a whole path, or one chunk's run of it).
/// `first_seg` is the first segment's place in the whole path, for the
/// stripes; `v_coords` has an entry per sample, and one more on a loop.
fn sweep_samples(
    samples: &[SplineSample],
    closed: bool,
    path: &SweepPath,
    first_seg: usize,
    profile: &Profile,
    style: &SweepStyle,
    v_coords: &[f32],
) -> SweptMesh {
    let n = samples.len();
    if n < 2 {
        return SweptMesh::default();
    }
    let gaps = path.gaps;
    let offset = style.offset;
    let points = profile.points();
    let edges = profile.edge_count();

    // The profile placed at every sample
    let rings: Vec<Vec<Vec3>> = samples
        .iter()
        .map(|s| {
            let center = s.position + s.right * offset[0] + s.up * offset[1];
            points
                .iter()
                .map(|p| center + s.right * p[0] + s.up * p[1])
                .collect()
        })
        .collect();
    let edge_normals: Vec<[f32; 2]> = (0..edges).map(|e| profile.edge_normal(e)).collect();
    let point_normals = profile.smooth.then(|| profile.point_normals());
    let around = profile.around();
    let edge_u: Vec<(f32, f32)> = (0..edges).map(|e| profile.edge_u(e, &around)).collect();
    let cap = profile.closed.then(|| profile.cap_triangles());

    let num_segs = if closed { n } else { n - 1 };
    let mut mesh = SweptMesh {
        vertices: Vec::with_capacity(num_segs * edges * 4),
        indices: Vec::with_capacity(num_segs * edges * 6),
    };

    // A cap at sample `idx`, facing along the path or against it
    let push_cap = |mesh: &mut SweptMesh, idx: usize, facing_forward: bool| {
        let Some(tris) = &cap else {
            return;
        };
        let fwd = if facing_forward {
            samples[idx].forward
        } else {
            samples[idx].forward * -1.0
        };
        let normal = fwd.to_array();
        let (lo, hi) = points.iter().fold(
            ([f32::INFINITY; 2], [f32::NEG_INFINITY; 2]),
            |(lo, hi), p| {
                (
                    [lo[0].min(p[0]), lo[1].min(p[1])],
                    [hi[0].max(p[0]), hi[1].max(p[1])],
                )
            },
        );
        let base = mesh.vertices.len() as u32;
        for k in (0..points.len()).rev() {
            let p = points[k];
            mesh.vertices.push(SweptVertex {
                position: rings[idx][k].to_array(),
                normal,
                color: style.color,
                uv: [
                    (p[0] - lo[0]) / (hi[0] - lo[0]),
                    (p[1] - lo[1]) / (hi[1] - lo[1]),
                ],
            });
        }
        for t in tris {
            if facing_forward {
                mesh.indices
                    .extend_from_slice(&[base + t[0], base + t[1], base + t[2]]);
            } else {
                mesh.indices
                    .extend_from_slice(&[base + t[0], base + t[2], base + t[1]]);
            }
        }
    };

    for seg in 0..num_segs {
        let next = if closed { (seg + 1) % n } else { seg + 1 };
        let seg_in_gap = spline::is_in_gap(samples[seg].t, gaps);
        let next_in_gap = spline::is_in_gap(samples[next].t, gaps);
        // Leaving solid ground for a gap, and landing again
        if !seg_in_gap && next_in_gap {
            push_cap(&mut mesh, seg, true);
        }
        if seg_in_gap && !next_in_gap {
            push_cap(&mut mesh, next, false);
        }
        if seg_in_gap {
            continue;
        }

        let (s0, s1) = (&samples[seg], &samples[next]);
        let (v0, v1) = (v_coords[seg], v_coords[seg + 1]);
        let color = match style.stripes {
            Some(stripes) => {
                let dist = (first_seg + seg) as f32 * path.spacing;
                if (dist / stripes.width).floor() as i32 % 2 == 0 {
                    style.color
                } else {
                    stripes.color
                }
            }
            None => style.color,
        };
        let right_sum = s0.right + s1.right;
        let up_sum = s0.up + s1.up;

        for e in 0..edges {
            let (a, b) = profile.edge(e);
            let (ua, ub) = edge_u[e];
            // Flat: one normal across the quad, the two frames averaged
            let flat = || {
                let [nx, ny] = edge_normals[e];
                (combine(right_sum, nx, up_sum, ny) * 0.5)
                    .normalized()
                    .to_array()
            };
            // The normals at a and b on this sample and the next
            let [na0, nb0, na1, nb1] = match &point_normals {
                // Smooth: each point's normal in its own sample's frame. A
                // sample with no frame (a path that doubles back on itself)
                // takes the flat one, never a zero the shader would make NaN of
                Some(pn) => {
                    let at = |s: &SplineSample, k: usize| {
                        let n = combine(s.right, pn[k][0], s.up, pn[k][1]).normalized();
                        if n == Vec3::ZERO {
                            flat()
                        } else {
                            n.to_array()
                        }
                    };
                    [at(s0, a), at(s0, b), at(s1, a), at(s1, b)]
                }
                None => [flat(); 4],
            };
            let base = mesh.vertices.len() as u32;
            let corners = [
                (rings[seg][a], na0, [ua, v0]),
                (rings[seg][b], nb0, [ub, v0]),
                (rings[next][a], na1, [ua, v1]),
                (rings[next][b], nb1, [ub, v1]),
            ];
            for (position, normal, uv) in corners {
                mesh.vertices.push(SweptVertex {
                    position: position.to_array(),
                    normal,
                    color,
                    uv,
                });
            }
            mesh.indices.extend_from_slice(&[
                base,
                base + 1,
                base + 2,
                base + 1,
                base + 3,
                base + 2,
            ]);
        }
    }

    // An open path's two ends, where they are not in a gap
    if !closed {
        if !spline::is_in_gap(samples[0].t, gaps) {
            push_cap(&mut mesh, 0, false);
        }
        let last = n - 1;
        if !spline::is_in_gap(samples[last].t, gaps) {
            push_cap(&mut mesh, last, true);
        }
    }
    mesh
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A square loop of side `side` in the XZ plane, `per_side` samples a
    /// side, with right vectors pointing into the loop.
    fn square_loop(side: f32, per_side: usize) -> Vec<SplineSample> {
        let corners = [
            (Vec3::new(0.0, 0.0, 0.0), Vec3::new(1.0, 0.0, 0.0)),
            (Vec3::new(side, 0.0, 0.0), Vec3::new(0.0, 0.0, 1.0)),
            (Vec3::new(side, 0.0, side), Vec3::new(-1.0, 0.0, 0.0)),
            (Vec3::new(0.0, 0.0, side), Vec3::new(0.0, 0.0, -1.0)),
        ];
        let up = Vec3::new(0.0, 1.0, 0.0);
        let mut out = Vec::new();
        for (start, fwd) in corners {
            let right = Vec3::new(-fwd.z, 0.0, fwd.x);
            for i in 0..per_side {
                let t = out.len() as f32 / (4 * per_side) as f32;
                out.push(SplineSample {
                    position: start + fwd * (side * i as f32 / per_side as f32),
                    forward: fwd,
                    right,
                    up,
                    twist: 0.0,
                    t,
                });
            }
        }
        out
    }

    fn style() -> SweepStyle {
        SweepStyle {
            offset: [0.0, 0.0],
            color: [0.5, 0.5, 0.5, 1.0],
            stripes: None,
            texture_length: 4.0,
        }
    }

    fn open_path(samples: &[SplineSample]) -> SweepPath<'_> {
        SweepPath {
            samples,
            closed: false,
            spacing: 2.0,
            gaps: &[],
        }
    }

    #[test]
    fn open_sweep_v_is_metres_over_length() {
        let samples: Vec<SplineSample> = square_loop(40.0, 20).into_iter().take(11).collect();
        let v = sweep_v_coords(&samples, false, [0.0, 0.0], 4.0);
        assert_eq!(v.len(), 11);
        // samples 2 m apart, 4 m per repeat
        assert!((v[10] - 5.0).abs() < 1e-4, "{v:?}");
    }

    #[test]
    fn closed_sweep_meets_itself_at_the_seam() {
        // a 160 m loop with a 7 m texture: 22.86 repeats, rounded to 23
        let samples = square_loop(40.0, 20);
        let v = sweep_v_coords(&samples, true, [0.0, 0.0], 7.0);
        assert_eq!(v.len(), samples.len() + 1, "one more entry closes the loop");
        let last = *v.last().unwrap();
        assert!((last - 23.0).abs() < 1e-3, "{last}");
    }

    #[test]
    fn offset_sweeps_measure_their_own_path() {
        // each sweep is measured along its own offset path: a wall 2 m
        // outside the square cuts its corners wider than the centre line
        let samples = square_loop(40.0, 20);
        let len = |x: f32| {
            *sweep_v_coords(&samples, false, [x, 0.0], 1.0)
                .last()
                .unwrap()
        };
        assert!((len(0.0) - 158.0).abs() < 1e-3, "{}", len(0.0));
        assert!(len(-2.0) > len(0.0) + 1.0, "{}", len(-2.0));
    }

    #[test]
    fn a_box_has_four_faces_a_segment_and_two_caps() {
        let samples: Vec<SplineSample> = square_loop(40.0, 20).into_iter().take(5).collect();
        let mesh = sweep(&open_path(&samples), &Profile::rect(2.0, 1.0), &style());
        // 4 segments x 4 faces x 2 triangles, and two caps of 2
        assert_eq!(mesh.triangles().len(), 4 * 4 * 2 + 2 * 2);
        assert_eq!(mesh.vertices.len(), 4 * 4 * 4 + 2 * 4);
        // the top face looks up, the first wall left (-right is -z here)
        assert_eq!(mesh.vertices[0].normal, [0.0, 1.0, 0.0]);
        assert_eq!(mesh.vertices[0].position, [0.0, 0.5, -1.0]);
    }

    #[test]
    fn faces_look_outward_whichever_way_a_profile_is_listed() {
        let samples: Vec<SplineSample> = square_loop(40.0, 20).into_iter().take(3).collect();
        let tri = vec![[0.0, 1.0], [1.0, -1.0], [-1.0, -1.0]];
        let mut reversed = tri.clone();
        reversed.reverse();
        for points in [tri, reversed] {
            let profile = Profile::new(points, true, false).unwrap();
            let mesh = sweep(&open_path(&samples), &profile, &style());
            // every side vertex's normal points away from the path's centre
            for v in &mesh.vertices[..profile.edge_count() * 4] {
                let p = Vec3::from_array(v.position) - samples[0].position;
                let outward = p.y * v.normal[1] + p.z * v.normal[2];
                assert!(outward > 0.0, "{v:?}");
            }
        }
    }

    #[test]
    fn a_concave_cap_stays_inside_its_outline() {
        // an L: ear clipping, not a fan, which would cover the notch
        let l = Profile::new(
            vec![
                [0.0, 0.0],
                [2.0, 0.0],
                [2.0, 1.0],
                [1.0, 1.0],
                [1.0, 2.0],
                [0.0, 2.0],
            ],
            true,
            false,
        )
        .unwrap();
        let tris = l.cap_triangles();
        assert_eq!(tris.len(), 4);
        let reversed: Vec<[f32; 2]> = l.points().iter().rev().copied().collect();
        let area: f32 = tris
            .iter()
            .map(|t| {
                let (a, b, c) = (
                    reversed[t[0] as usize],
                    reversed[t[1] as usize],
                    reversed[t[2] as usize],
                );
                ((b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])) * 0.5
            })
            .sum();
        assert!((area - 3.0).abs() < 1e-5, "the L's area is 3, got {area}");
    }

    #[test]
    fn a_convex_cap_is_a_fan_from_its_first_point() {
        let tris = Profile::rect(2.0, 1.0).cap_triangles();
        assert_eq!(tris, vec![[0, 1, 2], [0, 2, 3]]);
    }

    #[test]
    fn an_open_profile_has_no_caps_and_faces_left_of_its_listing() {
        let samples: Vec<SplineSample> = square_loop(40.0, 20).into_iter().take(5).collect();
        // left to right: a surface facing up
        let road = Profile::new(vec![[-3.0, 0.0], [0.0, -0.2], [3.0, 0.0]], false, false).unwrap();
        let mesh = sweep(&open_path(&samples), &road, &style());
        assert_eq!(mesh.triangles().len(), 4 * 2 * 2, "no caps");
        assert!(mesh.vertices.iter().all(|v| v.normal[1] > 0.9));
    }

    #[test]
    fn a_smooth_tube_shades_round_and_wraps_u_once() {
        let samples: Vec<SplineSample> = square_loop(40.0, 20).into_iter().take(3).collect();
        let ring: Vec<[f32; 2]> = (0..8)
            .map(|i| {
                let a = i as f32 / 8.0 * std::f32::consts::TAU;
                [a.cos() * 0.5, a.sin() * 0.5]
            })
            .collect();
        let tube = Profile::new(ring, true, true).unwrap();
        let mesh = sweep(&open_path(&samples), &tube, &style());
        // a point's normal is the same on both faces that share it
        let first_face = &mesh.vertices[0..4];
        let second_face = &mesh.vertices[4..8];
        assert_eq!(first_face[1].normal, second_face[0].normal);
        // and points straight out from the centre line
        let v = first_face[0];
        let p = Vec3::from_array(v.position) - samples[0].position;
        let radial = Vec3::new(0.0, p.y, p.z).normalized();
        let n = Vec3::from_array(v.normal);
        assert!((radial.dot(&n) - 1.0).abs() < 1e-5, "{v:?}");
        // u runs 0..1 once around, the last edge ending at 1
        let last_edge = &mesh.vertices[7 * 4..8 * 4];
        assert_eq!(last_edge[1].uv[0], 1.0);
        assert_eq!(first_face[0].uv[0], 0.0);
    }

    #[test]
    fn a_smooth_sweep_has_no_zero_normal_on_its_sides() {
        // An open spline's last sample once had no frame (#1016); a smooth
        // profile's normal there was zero, which the shader made NaN and
        // bloom spread into a black block
        use crate::spline::{sample_open_spline, SplineControlPoint};
        let points: Vec<SplineControlPoint> = [[0.0, 0.0, 0.0], [3.0, 0.5, 0.0], [6.0, 0.0, 2.0]]
            .into_iter()
            .map(|p| SplineControlPoint {
                position: Vec3::from_array(p),
                twist: 0.0,
                gap_after: false,
            })
            .collect();
        let samples = sample_open_spline(&points, 0.5);
        let ring: Vec<[f32; 2]> = (0..6)
            .map(|i| {
                let a = i as f32 / 6.0 * std::f32::consts::TAU;
                [a.cos() * 0.1, a.sin() * 0.1]
            })
            .collect();
        let tube = Profile::new(ring, true, true).unwrap();
        let mesh = sweep(&open_path(&samples), &tube, &style());
        let sides = (samples.len() - 1) * 6 * 4;
        assert!(mesh.vertices[..sides].iter().all(|v| v.normal != [0.0; 3]));
    }

    #[test]
    fn an_open_sweep_keeps_its_width_and_caps_facing_along_the_path() {
        // #1016: the last ring collapsed to the centre line, and its cap
        // had a zero normal
        use crate::spline::{sample_open_spline, SplineControlPoint};
        let points: Vec<SplineControlPoint> = [[0.0, 0.0, 0.0], [3.0, 0.5, 0.0], [6.0, 0.0, 2.0]]
            .into_iter()
            .map(|p| SplineControlPoint {
                position: Vec3::from_array(p),
                twist: 0.0,
                gap_after: false,
            })
            .collect();
        let samples = sample_open_spline(&points, 0.5);
        let mesh = sweep(&open_path(&samples), &Profile::rect(2.0, 1.0), &style());
        let sides = (samples.len() - 1) * 4 * 4;
        // the last segment's top face: its far edge as wide as its near one
        let top = &mesh.vertices[sides - 16..sides - 12];
        let width = |a: &SweptVertex, b: &SweptVertex| {
            (Vec3::from_array(a.position) - Vec3::from_array(b.position)).length()
        };
        assert!((width(&top[2], &top[3]) - width(&top[0], &top[1])).abs() < 1e-3);
        // caps: the start's against the path, the end's along it
        let (start, end) = (&samples[0], &samples[samples.len() - 1]);
        let chord = |a: &SplineSample, b: &SplineSample| (b.position - a.position).normalized();
        let start_cap = Vec3::from_array(mesh.vertices[sides].normal);
        let end_cap = Vec3::from_array(mesh.vertices[sides + 4].normal);
        assert!(
            start_cap.dot(&chord(start, &samples[1])) < -0.99,
            "{start_cap:?}"
        );
        assert!(
            end_cap.dot(&chord(&samples[samples.len() - 2], end)) > 0.99,
            "{end_cap:?}"
        );
    }

    #[test]
    fn side_walls_read_upward_on_both_sides() {
        let samples: Vec<SplineSample> = square_loop(40.0, 20).into_iter().take(2).collect();
        let mesh = sweep(&open_path(&samples), &Profile::rect(2.0, 1.0), &style());
        // faces in profile order: top, right side, bottom, left side
        for face in [1usize, 3] {
            let quad = &mesh.vertices[face * 4..face * 4 + 2];
            let (low, high) = if quad[0].position[1] < quad[1].position[1] {
                (quad[0], quad[1])
            } else {
                (quad[1], quad[0])
            };
            assert_eq!((low.uv[0], high.uv[0]), (0.0, 1.0), "face {face}");
        }
    }

    #[test]
    fn gaps_make_no_geometry_and_are_capped() {
        let samples: Vec<SplineSample> = square_loop(40.0, 20).into_iter().take(11).collect();
        // samples are t = k / 80: a gap over samples 3..6
        let gaps = [(3.0 / 80.0, 6.0 / 80.0)];
        let path = SweepPath {
            gaps: &gaps,
            ..open_path(&samples)
        };
        let mesh = sweep(&path, &Profile::rect(2.0, 1.0), &style());
        // 10 segments, 3 in the gap, so 7 x 8 triangles; ends and gap edges capped
        assert_eq!(mesh.triangles().len(), 7 * 8 + 4 * 2);
    }

    #[test]
    fn chunks_cover_the_sweep_and_stripes_run_on_across_them() {
        let samples = square_loop(40.0, 20);
        let path = SweepPath {
            samples: &samples,
            closed: true,
            spacing: 2.0,
            gaps: &[],
        };
        let striped = SweepStyle {
            stripes: Some(Stripes {
                color: [1.0, 0.0, 0.0, 1.0],
                width: 6.0,
            }),
            ..style()
        };
        let profile = Profile::rect(2.0, 1.0);
        let whole = sweep(&path, &profile, &striped);
        let chunks = sweep_chunks(&path, &profile, &striped, 7);
        assert_eq!(chunks.len(), 80usize.div_ceil(7));
        // the same side quads in the same order, plus each chunk's two caps
        let sides: Vec<SweptVertex> = chunks
            .iter()
            .flat_map(|c| {
                let caps = 2 * 4;
                c.mesh.vertices[..c.mesh.vertices.len() - caps].to_vec()
            })
            .collect();
        assert_eq!(sides.len(), whole.vertices.len());
        for (a, b) in sides.iter().zip(&whole.vertices) {
            assert_eq!(a.color, b.color);
            assert_eq!(a.uv, b.uv);
        }
        assert_eq!(chunks[1].t_start, samples[7].t);
    }

    #[test]
    fn bad_profiles_say_why() {
        assert!(Profile::new(vec![[0.0, 0.0], [1.0, 0.0]], true, false)
            .unwrap_err()
            .contains("at least 3"));
        assert!(Profile::new(vec![[0.0, 0.0], [0.0, 0.0]], false, false)
            .unwrap_err()
            .contains("same place"));
        assert!(
            Profile::new(vec![[0.0, 0.0], [1.0, 0.0], [2.0, 0.0]], true, false)
                .unwrap_err()
                .contains("no area")
        );
    }
}
