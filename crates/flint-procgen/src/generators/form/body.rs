//! The form's rest shape: a smooth spine, a frame carried along it, and the
//! surface swept around it.
//!
//! The spine is a curve from the base to the tip, `height` long. It can lean,
//! curl through an angle in one plane (a hook, a wave) and coil (a thread of
//! smoke). A frame (tangent, normal, binormal) is carried up it by parallel
//! transport, so the cross-section never spins on its own where the curve bends.
//! The cross-section is an ellipse, `aspect` thin across, turned by `section_deg`
//! and twisted `twist_turns` times over the length, whose radius follows the
//! `profile` and the lineage's lobes. Everything here is the rest pose: a pose
//! only turns and scales the spine's segments (see `pose.rs`).

use std::f32::consts::TAU;

use flint_core::quat::{quat_from_two_vectors, quat_rotate_vec3};
use flint_core::Vec3;

use super::FormParams;
use crate::types::BranchSegment;
use crate::SeededRng;

/// Samples along the spine. Frames between them are interpolated, so this only
/// has to be fine enough that a curl of 200° looks smooth.
const SPINE_SAMPLES: usize = 512;

/// What a lineage fixes besides the authored params: the lobes' phases, a small
/// turn of the facing and the coil's phase. The lineage is the form's identity.
#[derive(Debug, Clone)]
pub(super) struct Lineage {
    lobe_phase: [f32; 3],
    lobe_freq: f32,
    /// Radians added to `facing_deg`.
    pub facing_turn: f32,
    coil_phase: f32,
}

impl Lineage {
    pub fn new(rng: &mut SeededRng) -> Self {
        Self {
            lobe_phase: std::array::from_fn(|_| rng.next_range(0.0, TAU)),
            lobe_freq: rng.next_range(1.2, 1.8),
            facing_turn: rng.next_range(-0.26, 0.26),
            coil_phase: rng.next_range(0.0, TAU),
        }
    }
}

/// The spine as a curve with a frame at every sample.
#[derive(Debug, Clone)]
pub(super) struct Spine {
    points: Vec<Vec3>,
    tangents: Vec<Vec3>,
    normals: Vec<Vec3>,
}

/// A frame on the spine at one height.
#[derive(Debug, Clone, Copy)]
pub(super) struct Frame {
    pub centre: Vec3,
    pub tangent: Vec3,
    pub normal: Vec3,
    pub binormal: Vec3,
}

impl Spine {
    pub fn new(p: &FormParams, lin: &Lineage) -> Self {
        let facing = p.facing_deg.to_radians() + lin.facing_turn;
        let (fx, fz) = (facing.cos(), facing.sin());
        let along = Vec3::new(fx, 0.0, fz);
        let curl = p.curl_deg.to_radians();
        let k = SPINE_SAMPLES;
        let du = 1.0 / k as f32;

        // The curled centreline: the direction turns through `curl` in the
        // facing plane, more toward the tip as `curl_bias` grows.
        let mut points = Vec::with_capacity(k + 1);
        let mut at = Vec3::new(0.0, p.lift, 0.0);
        points.push(at);
        for i in 0..k {
            let u = (i as f32 + 0.5) * du;
            let phi = curl * u.powf(p.curl_bias);
            let dir = Vec3::UP * phi.cos() + along * phi.sin();
            at = at + dir * (p.height * du);
            points.push(at);
        }
        // Lean and coil are offsets on top: lean grows with the square of the
        // height, and the coil grows from nothing at the base, so the base stays
        // where it stands.
        for (i, point) in points.iter_mut().enumerate() {
            let u = i as f32 * du;
            let swirl = TAU * p.coil_turns * u + lin.coil_phase;
            let coil = Vec3::new(swirl.cos(), 0.0, swirl.sin()) * (p.coil * u);
            *point = *point + along * (p.lean * u * u) + coil;
        }

        let tangents: Vec<Vec3> = (0..=k)
            .map(|i| {
                let (a, b) = (i.saturating_sub(1), (i + 1).min(k));
                let t = points[b] - points[a];
                if t.length() < 1e-9 {
                    Vec3::UP
                } else {
                    t.normalized()
                }
            })
            .collect();

        // Parallel transport: the normal starts in the facing plane and is
        // turned only as much as the tangent turns.
        let mut normals = Vec::with_capacity(k + 1);
        let t0 = tangents[0];
        let mut n = along - t0 * along.dot(&t0);
        if n.length() < 1e-6 {
            n = Vec3::FORWARD - t0 * Vec3::FORWARD.dot(&t0);
        }
        n = n.normalized();
        normals.push(n);
        for i in 1..=k {
            let q = quat_from_two_vectors(tangents[i - 1].to_array(), tangents[i].to_array());
            n = Vec3::from_array(quat_rotate_vec3(&q, n.to_array()));
            n = (n - tangents[i] * n.dot(&tangents[i])).normalized();
            normals.push(n);
        }
        Self {
            points,
            tangents,
            normals,
        }
    }

    /// The frame at height fraction `u`, interpolated between samples.
    pub fn frame(&self, u: f32) -> Frame {
        let k = SPINE_SAMPLES;
        let s = u.clamp(0.0, 1.0) * k as f32;
        let i = (s.floor() as usize).min(k - 1);
        let f = s - i as f32;
        let mix = |a: Vec3, b: Vec3| a + (b - a) * f;
        let centre = mix(self.points[i], self.points[i + 1]);
        let tangent = mix(self.tangents[i], self.tangents[i + 1]).normalized();
        let n = mix(self.normals[i], self.normals[i + 1]);
        let normal = (n - tangent * n.dot(&tangent)).normalized();
        Frame {
            centre,
            tangent,
            normal,
            binormal: tangent.cross(&normal),
        }
    }

    /// The point on the centreline at `u`.
    pub fn point(&self, u: f32) -> Vec3 {
        self.frame(u).centre
    }

    /// The rest spine as `segments` chords, base first, each the parent of the
    /// next. `waves` depth bands, so per-level stagger ripples up the form in
    /// that many waves, and `[living] variance` holds the base steadier than
    /// the tip.
    pub fn segments(&self, p: &FormParams, lin: &Lineage) -> Vec<BranchSegment> {
        let n = p.segments as usize;
        (0..n)
            .map(|i| {
                let (u0, u1) = (i as f32 / n as f32, (i + 1) as f32 / n as f32);
                BranchSegment {
                    start: self.point(u0),
                    end: self.point(u1),
                    radius_start: radius_along(p, lin, u0, 0.0).max(1e-4),
                    radius_end: radius_along(p, lin, u1, 0.0).max(1e-4),
                    depth: (i * p.waves as usize / n) as u32,
                    id: i as u32,
                    parent: if i == 0 { None } else { Some(i as u32 - 1) },
                }
            })
            .collect()
    }
}

/// The profile at `u`: Catmull-Rom through the authored fractions, evenly
/// spaced from base to tip, and never below 0.
pub(super) fn profile_at(profile: &[f32], u: f32) -> f32 {
    let n = profile.len();
    if n == 1 {
        return profile[0].max(0.0);
    }
    let s = u.clamp(0.0, 1.0) * (n - 1) as f32;
    let i = (s.floor() as usize).min(n - 2);
    let t = s - i as f32;
    let at = |j: isize| profile[j.clamp(0, n as isize - 1) as usize];
    let (p0, p1, p2, p3) = (
        at(i as isize - 1),
        at(i as isize),
        at(i as isize + 1),
        at(i as isize + 2),
    );
    let t2 = t * t;
    let t3 = t2 * t;
    let v = 0.5
        * (2.0 * p1
            + (p2 - p0) * t
            + (2.0 * p0 - 5.0 * p1 + 4.0 * p2 - p3) * t2
            + (3.0 * p1 - p0 - 3.0 * p2 + p3) * t3);
    v.max(0.0)
}

/// An end rounded over `round` of the length: a quarter ellipse that reaches 0
/// at the very end. A `round` of 0 is a flat end.
fn cap(d: f32, round: f32) -> f32 {
    if round <= 0.0 || d >= round {
        return 1.0;
    }
    let x = 1.0 - (d / round).max(0.0);
    (1.0 - x * x).max(0.0).sqrt()
}

/// The section's radius at `u` and angle `a`, before `aspect` thins it.
pub(super) fn radius_along(p: &FormParams, lin: &Lineage, u: f32, a: f32) -> f32 {
    let u = u.clamp(0.0, 1.0);
    let ends = cap(u, p.round_base) * cap(1.0 - u, p.round_tip);
    let lobes = 1.0
        + p.lobes * (2.0 * a + 3.1 * u + lin.lobe_phase[0]).sin()
        + 0.6 * p.lobes * (3.0 * a - 5.0 * u + lin.lobe_phase[1]).sin()
        + 0.5 * p.lobes * (TAU * lin.lobe_freq * u + lin.lobe_phase[2]).sin();
    p.radius * profile_at(&p.profile, u) * ends * lobes.max(0.0)
}

/// The section's turn at `u`: its authored angle plus the twist so far.
fn section_turn(p: &FormParams, u: f32) -> f32 {
    p.section_deg.to_radians() + TAU * p.twist_turns * u
}

/// The rest surface at height fraction `u` and angle `a`, with its outward
/// normal. The first and last rings close on the spine, so the form is shut.
pub(super) fn surface(
    p: &FormParams,
    lin: &Lineage,
    spine: &Spine,
    u: f32,
    a: f32,
) -> (Vec3, Vec3) {
    let at = |u: f32, a: f32| {
        let f = spine.frame(u);
        let r = if u <= 0.0 || u >= 1.0 {
            0.0
        } else {
            radius_along(p, lin, u, a)
        };
        let (x, y) = (a.cos() * r, a.sin() * r * p.aspect);
        let turn = section_turn(p, u);
        let (c, s) = (turn.cos(), turn.sin());
        let (x, y) = (x * c - y * s, x * s + y * c);
        (f.centre + f.normal * x + f.binormal * y, f)
    };
    let (pos, f) = at(u, a);
    let e = 1e-3;
    // One-sided differences at the ends, so they stay on the surface.
    let (u_lo, u_hi) = ((u - e).max(0.0), (u + e).min(1.0));
    let du = at(u_hi, a).0 - at(u_lo, a).0;
    let da = at(u, a + e).0 - at(u, a - e).0;
    let mut n = du.cross(&da);
    let out = pos - f.centre;
    if n.length() < 1e-10 || out.length() < 1e-6 {
        n = if u < 0.5 { f.tangent * -1.0 } else { f.tangent };
    } else if n.dot(&out) < 0.0 {
        n = n * -1.0;
    }
    (pos, n.normalized())
}

/// The direction up the surface at `(u, a)`: a card's long axis lies along it.
pub(super) fn up_the_surface(p: &FormParams, lin: &Lineage, spine: &Spine, u: f32, a: f32) -> Vec3 {
    let e = 2e-3;
    let d = surface(p, lin, spine, (u + e).min(1.0), a).0
        - surface(p, lin, spine, (u - e).max(0.0), a).0;
    if d.length() < 1e-9 {
        spine.frame(u).tangent
    } else {
        d.normalized()
    }
}

/// The widest the section gets anywhere, for weighting where cards land.
pub(super) fn widest(p: &FormParams, lin: &Lineage) -> f32 {
    (0..=64)
        .flat_map(|i| (0..8).map(move |j| (i as f32 / 64.0, j as f32 / 8.0 * TAU)))
        .map(|(u, a)| radius_along(p, lin, u, a))
        .fold(0.0, f32::max)
        .max(1e-6)
}
