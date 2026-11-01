//! Leaf geometry generation for procedural trees.
//!
//! Generates billboard quads or diamond-shaped mesh clusters at branch tips,
//! with configurable density, size variation, and color variation.
//!
//! Every leaf is a *card*: a front face and a back face with opposite
//! winding, so it shows from both sides under back-face culling and each
//! side is lit by its own normal. Cards are stored whole and in order
//! ([`card_index_count`] indices each), which is what lets LODs thin
//! foliage a leaf at a time instead of collapsing edges through it.
//!
//! Leaves are coloured by a small palette of shades
//! ([`LeafParams::shades`]): each shade is its own material and submesh,
//! tinted by the season ([`season_tint`]) and spread in lightness by
//! `color_variation`. Every leaf wears one shade, picked at random.

use crate::algorithms::mesh_builder::MeshBuilder;
use crate::generators::util::srgb_to_linear;
use crate::types::{BranchSegment, MaterialData, MeshData, Vertex};
use crate::Result;
use crate::SeededRng;
use flint_core::spline::rotate_around_axis;
use flint_core::Vec3;

use super::LeafParams;
use super::LeafStyle;

/// Vertices and triangles one leaf card takes, front and back together.
pub fn card_size(style: LeafStyle) -> (u64, u64) {
    match style {
        // 4 corners a side
        LeafStyle::BillboardCluster => (8, 4),
        // a centre and 4 tips a side
        LeafStyle::MeshCluster => (10, 8),
        LeafStyle::None => (0, 0),
    }
}

/// How many leaf clusters sit along a branch segment `seg_len` long when it
/// carries leaves along its length: longer segments get several.
pub fn along_branch_clusters(seg_len: f32, params: &LeafParams) -> usize {
    let spacing = (params.leaf_spread_radius * 2.0).max(0.2);
    ((seg_len / spacing).ceil() as usize).max(1)
}

/// Indices one leaf card takes in a leaf mesh, front and back faces together.
pub fn card_index_count(style: LeafStyle) -> usize {
    match style {
        // 2 triangles a side
        LeafStyle::BillboardCluster => 12,
        // 4 triangles a side
        LeafStyle::MeshCluster => 24,
        LeafStyle::None => 0,
    }
}

/// Generate leaf geometry at branch tips and optionally along branch segments.
///
/// Dispatches to billboard or mesh cluster generation based on `params.style`.
/// Returns an empty mesh for [`LeafStyle::None`]. The result has one material
/// and one submesh per shade that any leaf wears.
pub fn generate_leaves(
    tips: &[Vec3],
    tip_directions: &[Vec3],
    segments: &[BranchSegment],
    params: &LeafParams,
    rng: &mut SeededRng,
) -> Result<MeshData> {
    if params.style == LeafStyle::None {
        return Ok(MeshBuilder::new().build());
    }

    // Collect along-branch positions
    let (along_positions, along_directions) = sample_along_branch_positions(segments, params, rng);

    let tip_count = tips.len().min(tip_directions.len());
    let all_positions: Vec<Vec3> = tips[..tip_count]
        .iter()
        .copied()
        .chain(along_positions)
        .collect();
    let all_directions: Vec<Vec3> = tip_directions[..tip_count]
        .iter()
        .copied()
        .chain(along_directions)
        .collect();

    let palette = leaf_palette(params, rng);
    let mut shades: Vec<MeshBuilder> = palette.iter().map(|_| MeshBuilder::new()).collect();

    for i in 0..all_positions.len() {
        if !rng.next_bool(params.density as f64) {
            continue;
        }

        let tip = all_positions[i];
        let dir = safe_normalize(all_directions[i], Vec3::UP);

        for _ in 0..params.leaves_per_tip {
            match params.style {
                LeafStyle::BillboardCluster => billboard_card(tip, dir, params, rng, &mut shades),
                _ => diamond_card(tip, dir, params, rng, &mut shades),
            }
        }
    }

    let mut out = MeshBuilder::new();
    for (k, shade) in shades.into_iter().enumerate() {
        let mesh = shade.build();
        if mesh.indices.is_empty() {
            continue;
        }
        let material = out.add_material(MaterialData {
            name: format!("leaf_{k}"),
            base_color: palette[k],
            metallic: 0.0,
            roughness: 0.8,
            normal_map: None,
        });
        out.merge_as_submesh(&mesh, material);
    }
    Ok(out.build())
}

/// Linear RGB of the season's stops after the base colour: yellow at 0.4,
/// orange at 0.7, red at 1.0 (from sRGB).
fn season_stops() -> [(f32, [f32; 3]); 3] {
    let lin = |r: f32, g: f32, b: f32| [srgb_to_linear(r), srgb_to_linear(g), srgb_to_linear(b)];
    [
        (0.4, lin(0.82, 0.66, 0.14)),
        (0.7, lin(0.84, 0.40, 0.09)),
        (1.0, lin(0.64, 0.12, 0.07)),
    ]
}

/// Tint a linear leaf colour toward the season: 0 leaves it as it is, then it
/// runs through yellow (0.4) and orange (0.7) to red (1.0), blending in linear
/// light. Alpha is kept.
pub fn season_tint(base: [f32; 4], season: f32) -> [f32; 4] {
    let s = season.clamp(0.0, 1.0);
    let mut from = (0.0, [base[0], base[1], base[2]]);
    for (at, rgb) in season_stops() {
        if s <= at {
            let t = (s - from.0) / (at - from.0);
            let mix = |a: f32, b: f32| a + (b - a) * t;
            return [
                mix(from.1[0], rgb[0]),
                mix(from.1[1], rgb[1]),
                mix(from.1[2], rgb[2]),
                base[3],
            ];
        }
        from = (at, rgb);
    }
    [from.1[0], from.1[1], from.1[2], base[3]]
}

/// The leaf shades: `params.shades` colours spread evenly across
/// `season ± season_variation`, each with its own lightness drawn from
/// `color_variation`. The lightness step is taken in sRGB, where a step
/// looks the same on a dark leaf as on a light one; in linear light ±0.08
/// turned a dark green into chartreuse.
fn leaf_palette(params: &LeafParams, rng: &mut SeededRng) -> Vec<[f32; 4]> {
    let n = params.shades.max(1);
    (0..n)
        .map(|k| {
            let spread = if n == 1 {
                0.0
            } else {
                k as f32 / (n - 1) as f32 * 2.0 - 1.0
            };
            let season = params.season + spread * params.season_variation;
            let [r, g, b, a] = season_tint(params.color_base, season);
            let srgb = [linear_to_srgb(r), linear_to_srgb(g), linear_to_srgb(b), a];
            let [r, g, b, a] = rng.next_color_variation_hsl(srgb, params.color_variation);
            [srgb_to_linear(r), srgb_to_linear(g), srgb_to_linear(b), a]
        })
        .collect()
}

/// Convert a single linear channel value to sRGB (inverse of [`srgb_to_linear`]).
fn linear_to_srgb(c: f32) -> f32 {
    if c <= 0.0031308 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

/// Normalize a Vec3, returning `fallback` if zero-length.
fn safe_normalize(v: Vec3, fallback: Vec3) -> Vec3 {
    let n = v.normalized();
    if n.length() < 1e-6 {
        fallback
    } else {
        n
    }
}

/// Sample leaf placement positions along branch segments.
///
/// Filters segments by `min_depth`, then probabilistically samples each one
/// using `density`. Places leaves at the segment midpoint (or at multiple
/// evenly-spaced positions for long segments relative to `spread_radius`).
fn sample_along_branch_positions(
    segments: &[BranchSegment],
    params: &LeafParams,
    rng: &mut SeededRng,
) -> (Vec<Vec3>, Vec<Vec3>) {
    let mut positions = Vec::new();
    let mut directions = Vec::new();

    if params.leaf_along_branch_density <= 0.0 {
        return (positions, directions);
    }

    for seg in segments {
        if seg.depth < params.leaf_along_branch_min_depth {
            continue;
        }

        if !rng.next_bool(params.leaf_along_branch_density as f64) {
            continue;
        }

        let seg_len = seg.length();
        if seg_len < 1e-6 {
            continue;
        }

        let dir = seg.direction();
        let num_points = along_branch_clusters(seg_len, params);

        for j in 0..num_points {
            let t = if num_points == 1 {
                0.5
            } else {
                (j as f32 + 0.5) / num_points as f32
            };
            let pos = seg.start + (seg.end - seg.start) * t;
            positions.push(pos);
            directions.push(dir);
        }
    }

    (positions, directions)
}

/// Uniform in `[-radius, radius)`, or 0 for no radius (an empty range panics).
fn jitter(rng: &mut SeededRng, radius: f32) -> f32 {
    if radius > 0.0 {
        rng.next_range(-radius, radius)
    } else {
        0.0
    }
}

/// Pick the shade a leaf wears.
fn pick_shade(rng: &mut SeededRng, shades: usize) -> usize {
    ((rng.next_f32() * shades as f32) as usize).min(shades - 1)
}

/// Add one leaf card to `builder`: the front face as given (counter-clockwise
/// about `normal`), then the same corners again with the normal flipped and
/// the winding reversed for the back.
fn add_card(
    builder: &mut MeshBuilder,
    corners: &[(Vec3, [f32; 2])],
    triangles: &[[u32; 3]],
    normal: Vec3,
    tangent: Vec3,
) {
    for front in [true, false] {
        let sign = if front { 1.0 } else { -1.0 };
        let mut base = None;
        for &(p, uv) in corners {
            let i = builder.add_vertex(Vertex {
                position: p.to_array(),
                normal: (normal * sign).to_array(),
                tangent: [tangent.x, tangent.y, tangent.z, sign],
                uv,
            });
            base.get_or_insert(i);
        }
        let base = base.unwrap_or(0);
        for t in triangles {
            if front {
                builder.add_triangle(base + t[0], base + t[1], base + t[2]);
            } else {
                builder.add_triangle(base + t[0], base + t[2], base + t[1]);
            }
        }
    }
}

/// One flat quad leaf facing along the branch with a random spin.
fn billboard_card(
    tip: Vec3,
    dir: Vec3,
    params: &LeafParams,
    rng: &mut SeededRng,
    shades: &mut [MeshBuilder],
) {
    let spread = params.leaf_spread_radius;

    // Find a perpendicular basis
    let perp = perpendicular_to(dir);
    let bitangent = safe_normalize(dir.cross(&perp), Vec3::new(0.0, 0.0, 1.0));

    let scale = params.leaf_size
        * (1.0 + rng.next_range(-params.leaf_size_variation, params.leaf_size_variation));
    let half = scale * 0.5;

    // Random rotation around the branch direction
    let angle = rng.next_range(0.0, std::f32::consts::TAU);
    let u = rotate_around_axis(perp, dir, angle);
    let v = rotate_around_axis(bitangent, dir, angle);

    // Random offset within the spread radius sphere
    let offset = dir * jitter(rng, spread) + u * jitter(rng, spread) + v * jitter(rng, spread);
    let center = tip + offset;

    let shade = pick_shade(rng, shades.len());
    // The quad spans u and v, so it faces along u × v (the branch direction).
    let normal = safe_normalize(u.cross(&v), dir);
    add_card(
        &mut shades[shade],
        &[
            (center - u * half - v * half, [0.0, 0.0]),
            (center + u * half - v * half, [1.0, 0.0]),
            (center + u * half + v * half, [1.0, 1.0]),
            (center - u * half + v * half, [0.0, 1.0]),
        ],
        &[[0, 1, 2], [0, 2, 3]],
        normal,
        u,
    );
}

/// One diamond leaf, tilted off the branch direction by a random pitch/yaw.
fn diamond_card(
    tip: Vec3,
    dir: Vec3,
    params: &LeafParams,
    rng: &mut SeededRng,
    shades: &mut [MeshBuilder],
) {
    let spread = params.leaf_spread_radius;

    let scale = params.leaf_size
        * (1.0 + rng.next_range(-params.leaf_size_variation, params.leaf_size_variation));
    let half = scale * 0.5;

    // Random orientation offset
    let pitch_offset = rng.next_range(-0.5, 0.5);
    let yaw_offset = rng.next_range(-0.5, 0.5);

    let perp = perpendicular_to(dir);
    let bitangent = safe_normalize(dir.cross(&perp), Vec3::new(0.0, 0.0, 1.0));

    // Apply random pitch/yaw to the leaf direction
    let leaf_dir = safe_normalize(dir + perp * pitch_offset + bitangent * yaw_offset, dir);
    let leaf_perp = perpendicular_to(leaf_dir);
    let leaf_bi = safe_normalize(leaf_dir.cross(&leaf_perp), Vec3::new(0.0, 0.0, 1.0));

    // Random offset within the spread radius sphere
    let offset =
        dir * jitter(rng, spread) + perp * jitter(rng, spread) + bitangent * jitter(rng, spread);
    let center = tip + offset;

    let shade = pick_shade(rng, shades.len());
    // Diamond: center, +X, +Y, -X, -Y in leaf space. perp × bi = leaf_dir, so
    // the fan below winds counter-clockwise about the leaf's direction.
    add_card(
        &mut shades[shade],
        &[
            (center, [0.5, 0.5]),
            (center + leaf_perp * half, [1.0, 0.5]),
            (center + leaf_bi * half, [0.5, 0.0]),
            (center - leaf_perp * half, [0.0, 0.5]),
            (center - leaf_bi * half, [0.5, 1.0]),
        ],
        &[[0, 1, 2], [0, 2, 3], [0, 3, 4], [0, 4, 1]],
        leaf_dir,
        leaf_perp,
    );
}

/// Find a vector perpendicular to `v`.
fn perpendicular_to(v: Vec3) -> Vec3 {
    let candidate = if v.x.abs() < 0.9 {
        Vec3::RIGHT
    } else {
        Vec3::UP
    };
    let result = v.cross(&candidate).normalized();
    if result.length() < 1e-6 {
        Vec3::RIGHT
    } else {
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_tips() -> (Vec<Vec3>, Vec<Vec3>) {
        let tips = vec![
            Vec3::new(0.0, 5.0, 0.0),
            Vec3::new(1.0, 4.0, 0.5),
            Vec3::new(-0.5, 4.5, -0.3),
            Vec3::new(0.3, 3.5, 0.8),
            Vec3::new(-0.8, 4.0, 0.2),
            Vec3::new(0.5, 4.8, -0.5),
            Vec3::new(0.0, 3.8, 0.6),
            Vec3::new(-0.3, 4.2, 0.1),
            Vec3::new(0.7, 3.9, -0.2),
            Vec3::new(-0.6, 4.1, 0.4),
        ];
        let dirs = tips
            .iter()
            .map(|t| (*t - Vec3::new(0.0, 2.0, 0.0)).normalized())
            .collect();
        (tips, dirs)
    }

    /// A ring of `n` tips around a trunk at y = 4, each pointing outward.
    fn ring_of_tips(n: usize) -> (Vec<Vec3>, Vec<Vec3>) {
        (0..n)
            .map(|i| {
                let a = i as f32 / n as f32 * std::f32::consts::TAU;
                let out = Vec3::new(a.cos(), 0.3, a.sin()).normalized();
                (Vec3::new(0.0, 4.0, 0.0) + out * 2.0, out)
            })
            .unzip()
    }

    fn cards(mesh: &MeshData, style: LeafStyle) -> usize {
        mesh.indices.len() / card_index_count(style)
    }

    #[test]
    fn billboard_leaves_produce_valid_mesh() {
        let (tips, dirs) = sample_tips();
        let params = LeafParams {
            style: LeafStyle::BillboardCluster,
            density: 1.0,
            leaves_per_tip: 3,
            ..LeafParams::default()
        };
        let mut rng = SeededRng::new(42);
        let mesh = generate_leaves(&tips, &dirs, &[], &params, &mut rng).unwrap();

        assert!(mesh.validate().is_ok());
        // Each leaf is a two-sided quad: 8 verts, 4 tris
        assert_eq!(mesh.vertex_count(), 30 * 8);
        assert_eq!(mesh.triangle_count(), 30 * 4);
    }

    #[test]
    fn mesh_cluster_leaves_produce_valid_mesh() {
        let (tips, dirs) = sample_tips();
        let params = LeafParams {
            style: LeafStyle::MeshCluster,
            density: 1.0,
            leaves_per_tip: 3,
            ..LeafParams::default()
        };
        let mut rng = SeededRng::new(42);
        let mesh = generate_leaves(&tips, &dirs, &[], &params, &mut rng).unwrap();

        assert!(mesh.validate().is_ok());
        // Each diamond is two-sided: 10 verts, 8 tris
        assert_eq!(mesh.vertex_count(), 30 * 10);
        assert_eq!(mesh.triangle_count(), 30 * 8);
    }

    #[test]
    fn none_style_returns_empty_mesh() {
        let (tips, dirs) = sample_tips();
        let params = LeafParams {
            style: LeafStyle::None,
            ..LeafParams::default()
        };
        let mut rng = SeededRng::new(42);
        let mesh = generate_leaves(&tips, &dirs, &[], &params, &mut rng).unwrap();
        assert_eq!(mesh.vertex_count(), 0);
        assert_eq!(mesh.triangle_count(), 0);
    }

    #[test]
    fn leaf_count_tracks_density() {
        // 400 tips at density 0.25 should leaf ~100 of them, and doubling the
        // density should roughly double the count.
        let (tips, dirs) = ring_of_tips(400);
        let count = |density: f32| {
            let params = LeafParams {
                density,
                leaves_per_tip: 1,
                ..LeafParams::default()
            };
            let mesh = generate_leaves(&tips, &dirs, &[], &params, &mut SeededRng::new(7)).unwrap();
            cards(&mesh, params.style) as f32
        };
        let quarter = count(0.25);
        let half = count(0.5);
        assert!(
            (80.0..=120.0).contains(&quarter),
            "density 0.25 leafed {quarter} of 400"
        );
        let ratio = half / quarter;
        assert!(
            (1.6..=2.4).contains(&ratio),
            "2x density gave {ratio:.2}x leaves"
        );
    }

    #[test]
    fn billboard_uvs_in_unit_square() {
        let (tips, dirs) = sample_tips();
        let mesh = generate_leaves(
            &tips,
            &dirs,
            &[],
            &LeafParams::default(),
            &mut SeededRng::new(3),
        )
        .unwrap();
        for v in &mesh.vertices {
            assert!((0.0..=1.0).contains(&v.uv[0]) && (0.0..=1.0).contains(&v.uv[1]));
        }
    }

    #[test]
    fn cards_face_both_ways_and_normals_match_winding() {
        // Each card's front faces along its branch (outward from the trunk),
        // the back faces the other way, and every triangle's winding agrees
        // with its vertices' normal, so lighting and culling see the same side.
        for style in [LeafStyle::BillboardCluster, LeafStyle::MeshCluster] {
            let (tips, dirs) = ring_of_tips(24);
            let params = LeafParams {
                style,
                density: 1.0,
                leaves_per_tip: 2,
                leaf_spread_radius: 0.0,
                ..LeafParams::default()
            };
            let mesh =
                generate_leaves(&tips, &dirs, &[], &params, &mut SeededRng::new(11)).unwrap();
            let pos = |i: u32| Vec3::from_array(mesh.vertices[i as usize].position);
            let per_card = card_index_count(style);
            let mut outward = 0;
            for card in mesh.indices.chunks(per_card) {
                let (front, back) = card.split_at(per_card / 2);
                let nf = Vec3::from_array(mesh.vertices[front[0] as usize].normal);
                let nb = Vec3::from_array(mesh.vertices[back[0] as usize].normal);
                assert!(
                    nf.dot(&nb) < -0.99,
                    "{style:?}: back normal should oppose front"
                );
                for tri in card.chunks(3) {
                    let face = (pos(tri[1]) - pos(tri[0])).cross(&(pos(tri[2]) - pos(tri[0])));
                    let n = Vec3::from_array(mesh.vertices[tri[0] as usize].normal);
                    assert!(
                        face.dot(&n) > 0.0,
                        "{style:?}: winding disagrees with normal"
                    );
                }
                let centre = pos(front[0]);
                let away = (centre - Vec3::new(0.0, centre.y, 0.0)).normalized();
                if nf.dot(&away) > 0.0 {
                    outward += 1;
                }
            }
            let total = mesh.indices.len() / per_card;
            assert!(
                outward * 10 >= total * 9,
                "{style:?}: only {outward}/{total} fronts face away from the trunk"
            );
        }
    }

    #[test]
    fn shades_are_materials_near_the_base() {
        let (tips, dirs) = ring_of_tips(100);
        let params = LeafParams {
            shades: 5,
            color_variation: 0.08,
            season_variation: 0.0,
            ..LeafParams::default()
        };
        let mesh = generate_leaves(&tips, &dirs, &[], &params, &mut SeededRng::new(5)).unwrap();
        assert_eq!(
            mesh.submeshes.len(),
            5,
            "every shade should be worn by some leaf"
        );
        let covered: u32 = mesh.submeshes.iter().map(|s| s.index_count).sum();
        assert_eq!(covered as usize, mesh.indices.len());

        let colours: Vec<[f32; 4]> = mesh.materials.iter().map(|m| m.base_color).collect();
        for (i, a) in colours.iter().enumerate() {
            for b in &colours[i + 1..] {
                assert_ne!(a, b, "shades should differ");
            }
            // Lightness ±0.08 and saturation ±0.04 keep every channel close.
            for c in 0..3 {
                assert!(
                    (a[c] - params.color_base[c]).abs() <= 0.12,
                    "shade {a:?} strays from base {:?}",
                    params.color_base
                );
            }
        }
    }

    #[test]
    fn season_runs_green_to_red() {
        let green = [0.03, 0.26, 0.004, 1.0];
        assert_eq!(season_tint(green, 0.0), green);
        let yellow = season_tint(green, 0.4);
        let orange = season_tint(green, 0.7);
        let red = season_tint(green, 1.0);
        assert!(
            yellow[0] > yellow[2] && yellow[1] > 0.3,
            "yellow: {yellow:?}"
        );
        assert!(orange[0] > 2.0 * orange[1], "orange: {orange:?}");
        assert!(red[0] > 10.0 * red[1], "red: {red:?}");
        // Red's share of the colour climbs monotonically through the year.
        let mut last = 0.0;
        for i in 0..=20 {
            let c = season_tint(green, i as f32 / 20.0);
            let share = c[0] / (c[0] + c[1] + c[2]);
            assert!(
                share >= last - 1e-6,
                "red share fell at season {}",
                i as f32 / 20.0
            );
            last = share;
        }
    }

    #[test]
    fn determinism_same_seed_same_output() {
        let (tips, dirs) = sample_tips();
        let params = LeafParams::default();

        let mut rng_a = SeededRng::new(123);
        let mesh_a = generate_leaves(&tips, &dirs, &[], &params, &mut rng_a).unwrap();

        let mut rng_b = SeededRng::new(123);
        let mesh_b = generate_leaves(&tips, &dirs, &[], &params, &mut rng_b).unwrap();

        assert_eq!(mesh_a.vertex_count(), mesh_b.vertex_count());
        assert_eq!(mesh_a.triangle_count(), mesh_b.triangle_count());
        for (va, vb) in mesh_a.vertices.iter().zip(mesh_b.vertices.iter()) {
            assert_eq!(va.position, vb.position);
        }
    }

    #[test]
    fn billboard_leaves_per_tip_count() {
        // With density=1.0 and a single tip, we should get exactly leaves_per_tip cards
        let tips = vec![Vec3::new(0.0, 5.0, 0.0)];
        let dirs = vec![Vec3::UP];
        let params = LeafParams {
            style: LeafStyle::BillboardCluster,
            density: 1.0,
            leaves_per_tip: 6,
            ..LeafParams::default()
        };
        let mut rng = SeededRng::new(42);
        let mesh = generate_leaves(&tips, &dirs, &[], &params, &mut rng).unwrap();

        // 6 two-sided quads × 8 verts = 48 verts, 6 × 4 tris = 24 tris
        assert_eq!(mesh.vertex_count(), 48);
        assert_eq!(mesh.triangle_count(), 24);
    }

    #[test]
    fn along_branch_placement_adds_leaves() {
        let tips = vec![Vec3::new(0.0, 5.0, 0.0)];
        let dirs = vec![Vec3::UP];
        let segments = vec![
            BranchSegment {
                start: Vec3::new(0.0, 3.0, 0.0),
                end: Vec3::new(1.0, 4.0, 0.0),
                radius_start: 0.08,
                radius_end: 0.05,
                depth: 2,
            },
            BranchSegment {
                start: Vec3::new(1.0, 4.0, 0.0),
                end: Vec3::new(2.0, 4.5, 0.5),
                radius_start: 0.05,
                radius_end: 0.03,
                depth: 3,
            },
        ];

        // Without along-branch
        let params_no_along = LeafParams {
            density: 1.0,
            leaf_along_branch_density: 0.0,
            ..LeafParams::default()
        };
        let mut rng1 = SeededRng::new(42);
        let mesh_no =
            generate_leaves(&tips, &dirs, &segments, &params_no_along, &mut rng1).unwrap();

        // With along-branch
        let params_along = LeafParams {
            density: 1.0,
            leaf_along_branch_density: 1.0,
            leaf_along_branch_min_depth: 2,
            ..LeafParams::default()
        };
        let mut rng2 = SeededRng::new(42);
        let mesh_yes = generate_leaves(&tips, &dirs, &segments, &params_along, &mut rng2).unwrap();

        assert!(
            mesh_yes.vertex_count() > mesh_no.vertex_count(),
            "along-branch placement ({}) should produce more vertices than tip-only ({})",
            mesh_yes.vertex_count(),
            mesh_no.vertex_count()
        );
    }

    #[test]
    fn along_branch_respects_min_depth() {
        let tips: Vec<Vec3> = vec![];
        let dirs: Vec<Vec3> = vec![];
        let segments = vec![BranchSegment {
            start: Vec3::new(0.0, 3.0, 0.0),
            end: Vec3::new(1.0, 4.0, 0.0),
            radius_start: 0.1,
            radius_end: 0.08,
            depth: 1, // below min_depth=2
        }];

        let params = LeafParams {
            density: 1.0,
            leaf_along_branch_density: 1.0,
            leaf_along_branch_min_depth: 2,
            ..LeafParams::default()
        };
        let mut rng = SeededRng::new(42);
        let mesh = generate_leaves(&tips, &dirs, &segments, &params, &mut rng).unwrap();

        // No tips and segment depth < min_depth, so no leaves
        assert_eq!(mesh.vertex_count(), 0);
    }
}
