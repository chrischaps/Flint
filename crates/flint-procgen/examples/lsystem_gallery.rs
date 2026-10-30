//! L-system plants grown from text grammars: the four built-in presets in a
//! row, or one preset at a given seed.
//!
//! ```bash
//! cargo run -p flint-procgen --example lsystem_gallery -- gallery.glb
//! cargo run -p flint-procgen --example lsystem_gallery -- weed.glb weed
//! cargo run -p flint-procgen --example lsystem_gallery -- plant7.glb stochastic_plant 7
//! flint edit gallery.glb --render gallery.png
//! ```

use flint_core::{Transform, Vec3};
use flint_procgen::algorithms::lsystem::grammar::{preset, Preset, PRESETS};
use flint_procgen::algorithms::lsystem::TurtleConfig;
use flint_procgen::algorithms::mesh_builder::{cuboid, MeshBuilder};
use flint_procgen::generators::tree::branches::segments_to_mesh;
use flint_procgen::{BranchSegment, MaterialData, SeededRng};

/// Every plant is scaled to this height, in metres.
const HEIGHT: f32 = 2.4;

fn material(name: &str, rgb: [f32; 3], roughness: f32) -> MaterialData {
    MaterialData {
        name: name.into(),
        base_color: [rgb[0], rgb[1], rgb[2], 1.0],
        metallic: 0.0,
        roughness,
    }
}

/// Grow a preset, scaled to `HEIGHT`.
fn grow(p: &Preset, seed: u64) -> Vec<BranchSegment> {
    let config = TurtleConfig {
        radius: 0.05,
        radius_decay: 0.72,
        tropism_strength: if p.name == "bush" { 0.0 } else { 0.06 },
        ..TurtleConfig::default().with_angle(p.angle)
    };
    let mut segments = p
        .grammar()
        .generate(p.iterations, &config, &mut SeededRng::new(seed))
        .expect("preset grows");
    let top = segments.iter().map(|s| s.end.y).fold(0.0f32, f32::max);
    let scale = HEIGHT / top;
    for s in &mut segments {
        s.start = s.start * scale;
        s.end = s.end * scale;
    }
    println!("{:>16}: {} segments", p.name, segments.len());
    segments
}

fn main() {
    let mut args = std::env::args().skip(1);
    let out = args.next().unwrap_or_else(|| "gallery.glb".into());
    let only = args.next();
    let seed: u64 = args.next().map_or(1, |s| s.parse().expect("seed"));

    let palette = [
        ("fractal_plant", [0.30, 0.42, 0.18]),
        ("weed", [0.42, 0.36, 0.16]),
        ("stochastic_plant", [0.20, 0.36, 0.26]),
        ("bush", [0.36, 0.24, 0.14]),
    ];
    let plants: Vec<&Preset> = match &only {
        Some(name) => vec![preset(name).unwrap_or_else(|| {
            let names: Vec<_> = PRESETS.iter().map(|p| p.name).collect();
            panic!("unknown preset {name}; expected one of {names:?}")
        })],
        None => PRESETS.iter().collect(),
    };

    let spacing = 2.6;
    let width = spacing * plants.len() as f32;
    let mut scene = MeshBuilder::new();
    let plinth = scene.add_material(material("plinth", [0.15, 0.14, 0.13], 0.85));
    let mut base = MeshBuilder::new();
    base.merge_transformed(
        &cuboid(width, 0.12, 2.2),
        &Transform::from_position(Vec3::new(0.0, -0.06, 0.0)).to_matrix(),
    );
    scene.merge_as_submesh(&base.build(), plinth);

    for (i, p) in plants.iter().enumerate() {
        let rgb = palette
            .iter()
            .find(|(n, _)| *n == p.name)
            .map_or([0.3, 0.3, 0.2], |c| c.1);
        let index = scene.add_material(material(p.name, rgb, 0.7));
        let mesh = segments_to_mesh(&grow(p, seed), 6).expect("mesh");
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
