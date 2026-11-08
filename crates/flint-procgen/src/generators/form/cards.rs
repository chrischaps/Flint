//! The form's cards: small two-sided quads on its surface, like scales or
//! flakes, which are what boils.
//!
//! A card rides one segment of the spine rigidly, and Living Props' jitter layer
//! turns it about its own normal on the jitter clock, so the surface keeps
//! shifting while the silhouette holds. `card_tilt` stands cards up off the
//! surface toward the tip: flat cards boil inside the outline, standing ones boil
//! the outline's edge too.

use std::f32::consts::TAU;

use flint_core::Vec3;

use super::body::{radius_along, surface, up_the_surface, widest, Lineage, Spine};
use super::FormParams;
use crate::living::LeafBinding;
use crate::types::{BoundingBox, BranchSegment, MaterialData, MeshData, SubMesh, Vertex};
use crate::SeededRng;

/// Vertices one card takes: a front face and a back face.
pub(super) const CARD_VERTICES: usize = 8;

/// Indices one card takes, front and back together.
pub(super) const CARD_INDICES: usize = 12;

/// How far a card's back face sits behind its front, so the two never fight.
const BACK_OFFSET: f32 = 4e-4;

/// The cards in the rest pose, in shade order: card `k` is indices
/// `k * CARD_INDICES ..`, and vertices `k * CARD_VERTICES ..`.
pub(super) fn card_mesh(
    p: &FormParams,
    lin: &Lineage,
    spine: &Spine,
    rng: &mut SeededRng,
) -> MeshData {
    let shades = p.card_shades as usize;
    let mut per_shade: Vec<Vec<[Vertex; CARD_VERTICES]>> = vec![Vec::new(); shades];
    let widest = widest(p, lin);
    let (from, to) = (p.card_from, p.card_to);
    for k in 0..p.cards {
        // Area-weighted, and crowded toward the tip by `card_rise`: more cards
        // where the form is wide, and where it's reaching.
        let (u, a) = loop {
            let u = rng.next_range(from, to);
            let a = rng.next_range(0.0, TAU);
            let wide = radius_along(p, lin, u, a) / widest;
            let rise = (1.0 + p.card_rise * u) / (1.0 + p.card_rise.max(0.0));
            if rng.next_f32() < wide * rise {
                break (u, a);
            }
        };
        let (pos, n) = surface(p, lin, spine, u, a);
        let up = up_the_surface(p, lin, spine, u, a);
        let up = (up - n * n.dot(&up)).normalized();
        let side = n.cross(&up).normalized();
        let turn = rng.next_range(-1.0, 1.0) * p.card_turn_deg.to_radians();
        let (ax, ay) = (
            side * turn.cos() + up * turn.sin(),
            up * turn.cos() - side * turn.sin(),
        );
        // Stand the card up about its width, its tip leaving the surface.
        let tilt = p.card_tilt * std::f32::consts::FRAC_PI_2 * rng.next_range(0.7, 1.0);
        let (ay, normal) = (
            ay * tilt.cos() + n * tilt.sin(),
            n * tilt.cos() - ay * tilt.sin(),
        );
        let size = p.card_size * rng.next_range(0.7, 1.3);
        let (hx, hy) = (ax * (size * 0.5), ay * (size * 0.5 * p.card_aspect));
        // A flat card is centred on its point; a standing one grows from it.
        let centre = pos + n * p.card_lift + hy * p.card_tilt;
        let corners = [
            centre - hx - hy,
            centre + hx - hy,
            centre + hx + hy,
            centre - hx + hy,
        ];
        let uvs = [[0.0, 1.0], [1.0, 1.0], [1.0, 0.0], [0.0, 0.0]];
        let back = normal * -BACK_OFFSET;
        let card: [Vertex; CARD_VERTICES] = std::array::from_fn(|c| {
            let (corner, facing, offset) = if c < 4 {
                (c, normal, Vec3::ZERO)
            } else {
                (c - 4, normal * -1.0, back)
            };
            Vertex {
                position: (corners[corner] + offset).to_array(),
                normal: facing.to_array(),
                tangent: [ax.x, ax.y, ax.z, 1.0],
                uv: uvs[corner],
            }
        });
        per_shade[k as usize % shades].push(card);
    }

    let mut vertices = Vec::new();
    let mut indices = Vec::new();
    let mut submeshes = Vec::new();
    for cards in &per_shade {
        let start = indices.len() as u32;
        for card in cards {
            let b = vertices.len() as u32;
            vertices.extend_from_slice(card);
            // Counter-clockwise seen from the side each face's normal points
            // to (`ax x ay` is the card's back), since only front faces draw.
            indices.extend_from_slice(&[b, b + 2, b + 1, b, b + 3, b + 2]);
            indices.extend_from_slice(&[b + 4, b + 5, b + 6, b + 4, b + 6, b + 7]);
        }
        submeshes.push(SubMesh {
            index_start: start,
            index_count: indices.len() as u32 - start,
            material_index: submeshes.len() + 1,
        });
    }
    let mut materials = vec![body_material(p)];
    for s in 0..shades {
        let t = if shades > 1 {
            s as f32 / (shades - 1) as f32 - 0.5
        } else {
            0.0
        };
        let c = p.card_color;
        materials.push(MaterialData {
            name: format!("form_card_{s}"),
            base_color: [
                c[0] * (1.0 + 0.16 * t),
                c[1] * (1.0 + 0.12 * t),
                c[2] * (1.0 + 0.04 * t),
                1.0,
            ],
            metallic: 0.0,
            roughness: p.card_roughness,
            normal_map: None,
        });
    }
    let positions: Vec<[f32; 3]> = vertices.iter().map(|v| v.position).collect();
    MeshData {
        bounding_box: BoundingBox::from_positions(&positions),
        vertices,
        indices,
        materials,
        submeshes,
    }
}

/// The body's material: every card material comes after it.
fn body_material(p: &FormParams) -> MaterialData {
    MaterialData {
        name: "form_body".into(),
        base_color: p.body_color,
        metallic: 0.0,
        roughness: p.body_roughness,
        normal_map: None,
    }
}

/// Each card bound to the spine segment nearest its centre.
pub(super) fn bind_cards(cards: &MeshData, segments: &[BranchSegment]) -> Vec<LeafBinding> {
    cards
        .vertices
        .chunks(CARD_VERTICES)
        .map(|card| {
            let centre = card[..4]
                .iter()
                .fold(Vec3::ZERO, |s, v| s + Vec3::from_array(v.position))
                * 0.25;
            let (segment, along, at) = segments
                .iter()
                .map(|s| {
                    let ab = s.end - s.start;
                    let t = ((centre - s.start).dot(&ab) / ab.dot(&ab).max(1e-9)).clamp(0.0, 1.0);
                    (s.id, t, s.start + ab * t)
                })
                .min_by(|a, b| (centre - a.2).length().total_cmp(&(centre - b.2).length()))
                .expect("a spine has segments");
            LeafBinding {
                segment,
                along,
                offset: centre - at,
            }
        })
        .collect()
}
