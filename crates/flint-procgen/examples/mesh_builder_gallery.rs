//! A small gallery built from the mesh builder: a fluted column (a smooth
//! custom cross-section), a spiral tendril (a lobed profile swept along a
//! helix), and a crystal cluster (faceted hexagons placed with
//! `merge_transformed`, its left half a mirror image of its right).
//!
//! ```bash
//! cargo run -p flint-procgen --example mesh_builder_gallery -- gallery.glb
//! flint edit gallery.glb --render gallery.png
//! ```

use flint_core::{Transform, Vec3};
use flint_procgen::algorithms::mesh_builder::{
    build_spine_from_positions, cuboid, extrude_profile_along_curve, CrossSection,
    ExtrudeParams, ExtrudePoint, MeshBuilder,
};
use flint_procgen::{MaterialData, MeshData};
use std::f32::consts::TAU;

fn material(name: &str, rgb: [f32; 3], roughness: f32) -> MaterialData {
    MaterialData {
        name: name.into(),
        base_color: [rgb[0], rgb[1], rgb[2], 1.0],
        metallic: 0.0,
        roughness,
    }
}

fn place(position: [f32; 3], rotation_deg: [f32; 3], scale: [f32; 3]) -> [[f32; 4]; 4] {
    Transform::default()
        .with_position(Vec3::new(position[0], position[1], position[2]))
        .with_rotation(Vec3::new(rotation_deg[0], rotation_deg[1], rotation_deg[2]))
        .with_scale(Vec3::new(scale[0], scale[1], scale[2]))
        .to_matrix()
}

/// A closed loop whose radius ripples `lobes` times around: `1 + depth * cos`.
fn rippled_loop(points: usize, lobes: f32, depth: f32) -> Vec<[f32; 2]> {
    (0..points)
        .map(|i| {
            let t = i as f32 / points as f32 * TAU;
            let r = 1.0 + depth * (lobes * t).cos();
            [t.cos() * r, t.sin() * r]
        })
        .collect()
}

fn column() -> MeshData {
    let mut b = MeshBuilder::new();
    let shaft: Vec<Vec3> = (0..=12)
        .map(|j| Vec3::new(0.0, 0.3 + 2.3 * j as f32 / 12.0, 0.0))
        .collect();
    let flutes = CrossSection::smooth(&rippled_loop(240, 20.0, 0.06));
    let tube = extrude_profile_along_curve(
        &ExtrudeParams {
            spine: build_spine_from_positions(&shaft, 0.32, 0.27),
            radial_segments: 0,
            caps: false,
            uv_length_scale: 1.0,
        },
        &flutes,
    )
    .expect("column shaft");
    b.merge(&tube);
    b.merge_transformed(&cuboid(0.9, 0.3, 0.9), &place([0.0, 0.15, 0.0], [0.0; 3], [1.0; 3]));
    b.merge_transformed(&cuboid(0.8, 0.16, 0.8), &place([0.0, 2.68, 0.0], [0.0; 3], [1.0; 3]));
    b.build()
}

fn tendril() -> MeshData {
    let turns = 2.6;
    let path: Vec<Vec3> = (0..=160)
        .map(|j| {
            let t = j as f32 / 160.0;
            let a = t * turns * TAU;
            let r = 0.42 * (1.0 - 0.55 * t);
            Vec3::new(a.cos() * r, 0.08 + 2.5 * t, a.sin() * r)
        })
        .collect();
    let lobed = CrossSection::smooth(&rippled_loop(72, 4.0, 0.22));
    extrude_profile_along_curve(
        &ExtrudeParams {
            spine: build_spine_from_positions(&path, 0.13, 0.025),
            radial_segments: 0,
            caps: true,
            uv_length_scale: 4.0,
        },
        &lobed,
    )
    .expect("tendril")
}

/// One hexagonal crystal standing on the origin, pointing up +Y.
fn crystal(height: f32, radius: f32) -> MeshData {
    let up = Vec3::new(0.0, 1.0, 0.0);
    let side = Vec3::new(0.0, 0.0, 1.0);
    let ring = |y: f32, r: f32| ExtrudePoint {
        position: Vec3::new(0.0, y, 0.0),
        forward: up,
        up: side,
        radius: r,
    };
    let hexagon: Vec<[f32; 2]> = (0..6)
        .map(|i| {
            let t = i as f32 / 6.0 * TAU;
            [t.cos(), t.sin()]
        })
        .collect();
    extrude_profile_along_curve(
        &ExtrudeParams {
            spine: vec![
                ring(-0.1, radius),
                ring(height * 0.72, radius),
                ring(height, radius * 0.02),
            ],
            radial_segments: 0,
            caps: true,
            uv_length_scale: 1.0,
        },
        &CrossSection::faceted(&hexagon),
    )
    .expect("crystal")
}

fn crystal_cluster() -> MeshData {
    // (height, radius, tilt toward +X, turn about Y, offset x, offset z)
    let half = [
        (1.25, 0.15, 14.0, 10.0, 0.08, 0.02),
        (0.85, 0.11, 34.0, -25.0, 0.22, 0.18),
        (0.62, 0.09, 52.0, 40.0, 0.28, -0.16),
        (0.45, 0.07, 66.0, 5.0, 0.42, 0.02),
    ];
    let mut right = MeshBuilder::new();
    for (h, r, tilt, turn, x, z) in half {
        // Tilt about Z leans the crystal toward -X; negate so it leans out.
        right.merge_transformed(&crystal(h, r), &place([x, 0.0, z], [0.0, turn, -tilt], [1.0; 3]));
    }
    let right = right.build();

    let mut b = MeshBuilder::new();
    b.merge(&right);
    // The left half is the same crystals through a mirror: winding and
    // tangent handedness flip with it, so the copy still faces outward.
    b.merge_transformed(&right, &place([0.0; 3], [0.0, 30.0, 0.0], [-0.9, 0.9, 0.9]));
    b.merge_transformed(&crystal(1.5, 0.17), &place([0.0, 0.0, -0.05], [-6.0, 0.0, 0.0], [1.0; 3]));
    b.build()
}

fn main() {
    let out = std::env::args().nth(1).unwrap_or_else(|| "gallery.glb".into());

    let mut scene = MeshBuilder::new();
    let pieces = [
        (
            material("plinth", [0.16, 0.15, 0.14], 0.8),
            {
                let mut b = MeshBuilder::new();
                b.merge_transformed(&cuboid(7.6, 0.16, 2.4), &place([0.0, -0.08, 0.0], [0.0; 3], [1.0; 3]));
                b.build()
            },
            [0.0, 0.0, 0.0],
        ),
        (material("marble", [0.86, 0.83, 0.77], 0.35), column(), [-2.6, 0.0, 0.0]),
        (material("tendril", [0.22, 0.52, 0.38], 0.55), tendril(), [0.0, 0.0, 0.0]),
        (material("amethyst", [0.55, 0.30, 0.78], 0.15), crystal_cluster(), [2.6, 0.0, 0.0]),
    ];
    for (mat, mesh, at) in pieces {
        let index = scene.add_material(mat);
        let mut placed = MeshBuilder::new();
        placed.merge_transformed(&mesh, &place(at, [0.0; 3], [1.0; 3]));
        scene.merge_as_submesh(&placed.build(), index);
    }
    scene.compute_tangents();
    let mesh = scene.build();
    mesh.validate().expect("gallery mesh is valid");

    let glb = flint_procgen::mesh_to_glb(&mesh).expect("export");
    std::fs::write(&out, glb).expect("write glb");
    println!(
        "wrote {out}: {} vertices, {} triangles, {} materials",
        mesh.vertex_count(),
        mesh.triangle_count(),
        mesh.materials.len()
    );
}
