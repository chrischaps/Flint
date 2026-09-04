//! Wireframe shape gizmos for the editor viewport: each emitter's spawn
//! volume and direction, the selected one in a warm accent.

use std::hash::{Hash, Hasher};

use flint_particles::{effect::ShapeField, EmitterDef, ParticleEffect, ResolveContext, ShapeDef};
use flint_render::{Mesh, Vertex};

use super::EmitterView;

const ACCENT: [f32; 4] = [1.0, 0.72, 0.35, 1.0];
const DIM: [f32; 4] = [0.55, 0.58, 0.65, 0.6];
const SEGMENTS: usize = 40;

/// Cheap change detector for the overlay (shape fields + selection + view flags).
pub fn shape_hash(effect: &ParticleEffect, selected: Option<usize>, views: &[EmitterView]) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    selected.hash(&mut h);
    for (i, em) in effect.emitters.iter().enumerate() {
        views
            .get(i)
            .map(|v| v.show_gizmo)
            .unwrap_or(true)
            .hash(&mut h);
        hash_f32s(&mut h, &em.direction);
        hash_f32s(&mut h, &em.shape_offset);
        hash_f32s(&mut h, &em.shape_axis_u);
        hash_f32s(&mut h, &em.shape_axis_v);
        hash_f32s(&mut h, &[em.spread]);
        match &em.shape {
            ShapeField::Named(n) => {
                n.hash(&mut h);
                hash_f32s(
                    &mut h,
                    &[
                        em.shape_radius.unwrap_or(0.0),
                        em.shape_angle.unwrap_or(0.0),
                    ],
                );
                hash_f32s(&mut h, &em.shape_extents.unwrap_or([0.0; 3]));
            }
            ShapeField::Def(d) => match d {
                ShapeDef::Point => 0u8.hash(&mut h),
                ShapeDef::Sphere { radius } => {
                    1u8.hash(&mut h);
                    hash_f32s(&mut h, &[*radius]);
                }
                ShapeDef::Cone { radius, angle } => {
                    2u8.hash(&mut h);
                    hash_f32s(&mut h, &[*radius, *angle]);
                }
                ShapeDef::Box { extents } => {
                    3u8.hash(&mut h);
                    hash_f32s(&mut h, extents);
                }
            },
        }
    }
    let v = h.finish();
    if v == 0 || v == u64::MAX {
        1
    } else {
        v
    }
}

fn hash_f32s<H: Hasher>(h: &mut H, xs: &[f32]) {
    for x in xs {
        x.to_bits().hash(h);
    }
}

/// Build the line-list overlay for every visible emitter.
pub fn build_overlay(
    effect: &ParticleEffect,
    selected: Option<usize>,
    views: &[EmitterView],
) -> Mesh {
    let mut mesh = Mesh {
        vertices: Vec::new(),
        indices: Vec::new(),
    };
    for (i, em) in effect.emitters.iter().enumerate() {
        if !views.get(i).map(|v| v.show_gizmo).unwrap_or(true) {
            continue;
        }
        let color = if selected == Some(i) { ACCENT } else { DIM };
        emitter_lines(em, color, &mut mesh);
    }
    mesh
}

fn emitter_lines(em: &EmitterDef, color: [f32; 4], mesh: &mut Mesh) {
    let Ok(cfg) = em.resolve(ResolveContext::asset()) else {
        return;
    };
    let origin = cfg.shape_offset;
    let dir = normalize(cfg.direction);
    let (right, up, _) = flint_particles::rand::perpendicular_basis(dir);

    match cfg.shape {
        flint_particles::EmissionShape::Point => {
            let s = 0.12;
            for axis in [[s, 0.0, 0.0], [0.0, s, 0.0], [0.0, 0.0, s]] {
                line(mesh, sub(origin, axis), add(origin, axis), color);
            }
        }
        flint_particles::EmissionShape::Sphere { radius } => {
            circle(
                mesh,
                origin,
                [1.0, 0.0, 0.0],
                [0.0, 1.0, 0.0],
                radius,
                color,
            );
            circle(
                mesh,
                origin,
                [0.0, 1.0, 0.0],
                [0.0, 0.0, 1.0],
                radius,
                color,
            );
            circle(
                mesh,
                origin,
                [1.0, 0.0, 0.0],
                [0.0, 0.0, 1.0],
                radius,
                color,
            );
        }
        flint_particles::EmissionShape::Cone { radius, angle } => {
            let base_r = radius.max(0.02);
            circle(mesh, origin, right, up, base_r, color);
            // Silhouette of the emission cone: the disc edge grows by tan(angle)
            // over a unit length along the direction.
            let len = 1.0;
            let top_r = base_r + len * angle.to_radians().tan().min(4.0);
            let top = add(origin, scale(dir, len));
            circle(mesh, top, right, up, top_r, color);
            for k in 0..6 {
                let a = k as f32 / 6.0 * std::f32::consts::TAU;
                let (c, s) = (a.cos(), a.sin());
                let p0 = add(origin, add(scale(right, c * base_r), scale(up, s * base_r)));
                let p1 = add(top, add(scale(right, c * top_r), scale(up, s * top_r)));
                line(mesh, p0, p1, color);
            }
        }
        flint_particles::EmissionShape::Box { extents } => {
            let (u, v, w) =
                flint_particles::sim::oriented_basis(cfg.shape_axis_u, cfg.shape_axis_v)
                    .unwrap_or(([1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]));
            let corner = |sx: f32, sy: f32, sz: f32| {
                add(
                    origin,
                    add(
                        add(scale(u, sx * extents[0]), scale(v, sy * extents[1])),
                        scale(w, sz * extents[2]),
                    ),
                )
            };
            let c = [
                corner(-1.0, -1.0, -1.0),
                corner(1.0, -1.0, -1.0),
                corner(1.0, 1.0, -1.0),
                corner(-1.0, 1.0, -1.0),
                corner(-1.0, -1.0, 1.0),
                corner(1.0, -1.0, 1.0),
                corner(1.0, 1.0, 1.0),
                corner(-1.0, 1.0, 1.0),
            ];
            for (a, b) in [
                (0, 1),
                (1, 2),
                (2, 3),
                (3, 0),
                (4, 5),
                (5, 6),
                (6, 7),
                (7, 4),
                (0, 4),
                (1, 5),
                (2, 6),
                (3, 7),
            ] {
                line(mesh, c[a], c[b], color);
            }
        }
    }

    // Direction arrow with spread whiskers.
    let tip = add(origin, scale(dir, 0.6));
    line(mesh, origin, tip, color);
    let head = 0.08;
    for side in [right, scale(right, -1.0), up, scale(up, -1.0)] {
        line(
            mesh,
            tip,
            add(sub(tip, scale(dir, head)), scale(side, head * 0.6)),
            color,
        );
    }
    if cfg.spread > 0.0 {
        let t = cfg.spread.to_radians().tan().min(6.0) * 0.6;
        for side in [right, scale(right, -1.0), up, scale(up, -1.0)] {
            line(
                mesh,
                origin,
                add(tip, scale(side, t)),
                [color[0], color[1], color[2], color[3] * 0.4],
            );
        }
    }
}

fn circle(mesh: &mut Mesh, center: [f32; 3], a: [f32; 3], b: [f32; 3], r: f32, color: [f32; 4]) {
    let mut prev = None;
    for k in 0..=SEGMENTS {
        let t = k as f32 / SEGMENTS as f32 * std::f32::consts::TAU;
        let p = add(center, add(scale(a, t.cos() * r), scale(b, t.sin() * r)));
        if let Some(q) = prev {
            line(mesh, q, p, color);
        }
        prev = Some(p);
    }
}

fn line(mesh: &mut Mesh, a: [f32; 3], b: [f32; 3], color: [f32; 4]) {
    let base = mesh.vertices.len() as u32;
    for p in [a, b] {
        mesh.vertices.push(Vertex {
            position: p,
            normal: [0.0, 1.0, 0.0],
            color,
            uv: [0.0, 0.0],
        });
    }
    mesh.indices.push(base);
    mesh.indices.push(base + 1);
}

fn add(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn scale(a: [f32; 3], s: f32) -> [f32; 3] {
    [a[0] * s, a[1] * s, a[2] * s]
}
fn normalize(v: [f32; 3]) -> [f32; 3] {
    flint_particles::rand::normalize(v)
}
