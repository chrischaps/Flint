//! Space colonization trees, one per crown volume: sphere, ellipsoid,
//! hemisphere and box, side by side, each grown from the same seed.
//!
//! ```bash
//! cargo run -p flint-procgen --example colonization_gallery -- gallery.glb
//! cargo run -p flint-procgen --example colonization_gallery -- gallery.glb 7 3.0
//! flint edit gallery.glb --render gallery.png
//! ```
//!
//! The optional arguments are the seed and the pipe model exponent.

use flint_core::{Transform, Vec3};
use flint_procgen::algorithms::mesh_builder::{cuboid, MeshBuilder};
use flint_procgen::algorithms::space_colonization::{
    branch_graph_to_segments, CrownVolume, SpaceColonization, SpaceColonizationParams,
};
use flint_procgen::generators::tree::branches::segments_to_mesh;
use flint_procgen::{MaterialData, SeededRng};

fn material(name: &str, rgb: [f32; 3], roughness: f32) -> MaterialData {
    MaterialData {
        name: name.into(),
        base_color: [rgb[0], rgb[1], rgb[2], 1.0],
        metallic: 0.0,
        roughness,
        normal_map: None,
    }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let out = args.next().unwrap_or_else(|| "gallery.glb".into());
    let seed: u64 = args.next().map_or(1, |s| s.parse().expect("seed"));
    let pipe_exponent: f32 = args.next().map_or(2.0, |s| s.parse().expect("exponent"));

    // Every crown sits above a 1.2 m trunk and spans roughly 2.4 m.
    let crowns = [
        (
            "sphere",
            [0.36, 0.25, 0.16],
            CrownVolume::Sphere {
                center: Vec3::new(0.0, 2.4, 0.0),
                radius: 1.2,
            },
        ),
        (
            "ellipsoid",
            [0.30, 0.27, 0.18],
            CrownVolume::Ellipsoid {
                center: Vec3::new(0.0, 2.8, 0.0),
                radii: Vec3::new(0.7, 1.6, 0.7),
            },
        ),
        (
            "hemisphere",
            [0.40, 0.28, 0.17],
            CrownVolume::Hemisphere {
                center: Vec3::new(0.0, 1.5, 0.0),
                radius: 1.4,
            },
        ),
        (
            "box",
            [0.27, 0.24, 0.17],
            CrownVolume::Box {
                min: Vec3::new(-1.1, 1.2, -0.4),
                max: Vec3::new(1.1, 2.6, 0.4),
            },
        ),
    ];

    let spacing = 3.2;
    let width = spacing * crowns.len() as f32;
    let mut scene = MeshBuilder::new();
    let plinth = scene.add_material(material("plinth", [0.15, 0.14, 0.13], 0.85));
    let mut base = MeshBuilder::new();
    base.merge_transformed(
        &cuboid(width, 0.12, 3.0),
        &Transform::from_position(Vec3::new(0.0, -0.06, 0.0)).to_matrix(),
    );
    scene.merge_as_submesh(&base.build(), plinth);

    for (i, (name, rgb, crown)) in crowns.iter().enumerate() {
        let gen = SpaceColonization::new(SpaceColonizationParams {
            crown: *crown,
            num_attraction_points: 600,
            kill_distance: 0.12,
            influence_distance: 0.9,
            step_size: 0.08,
            trunk_steps: 12,
            max_iterations: 400,
            max_nodes: 20_000,
            base_radius: 0.07,
            pipe_exponent,
            ..Default::default()
        })
        .expect("valid params");
        let started = std::time::Instant::now();
        let grown = gen.grow(&mut SeededRng::new(seed)).expect("grows");
        let ms = started.elapsed().as_secs_f32() * 1000.0;
        let forks = grown.nodes.iter().map(|n| n.depth).max().unwrap_or(0);
        println!(
            "{name:>10}: {} nodes, {} iterations, {} of 600 points unconsumed, depth {forks}, {ms:.1} ms",
            grown.nodes.len(),
            grown.iterations,
            grown.unconsumed,
        );

        let index = scene.add_material(material(name, *rgb, 0.75));
        let mesh = segments_to_mesh(&branch_graph_to_segments(&grown.nodes), 6).expect("mesh");
        let x = (i as f32 + 0.5) * spacing - width / 2.0;
        let mut placed = MeshBuilder::new();
        placed.merge_transformed(
            &mesh,
            &Transform::from_position(Vec3::new(x, 0.0, 0.0)).to_matrix(),
        );
        scene.merge_as_submesh(&placed.build(), index);
    }

    scene.compute_tangents();
    let mesh = scene.build();
    mesh.validate().expect("gallery mesh is valid");
    let glb = flint_procgen::mesh_to_glb(&mesh).expect("export");
    std::fs::write(&out, glb).expect("write glb");
    println!(
        "wrote {out}: {} vertices, {} triangles",
        mesh.vertex_count(),
        mesh.triangle_count()
    );
}
