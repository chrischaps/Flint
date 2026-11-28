//! The terrain's collider as a heightfield (#1219), walked: the character
//! crosses chunk seams on the heightfield as it did on the chunks' trimesh.
//! Moved from flint-physics' `heightfield_tests.rs` with the controller
//! (#1618); the rays and the rolling ball stayed there.

use super::tests::{spawn_from_toml, step};
use super::*;
use flint_physics::StaticHeightfield;
use flint_terrain::{Heightmap, Terrain, TerrainConfig};

const DT: f64 = 1.0 / 60.0;

const OFFSET: [f32; 3] = [-20.0, 1.5, 7.0];

/// Hills 64 x 48 m, 6 m high, in 4 x 3 chunks of 8 x 8 quads: a seam every
/// 16 m along both axes (flint-physics' `hills`).
fn hills() -> Terrain {
    let (w, d) = (33u32, 25u32);
    let heights = (0..w * d)
        .map(|i| {
            let (u, v) = (
                (i % w) as f32 / (w - 1) as f32,
                (i / w) as f32 / (d - 1) as f32,
            );
            0.5 + 0.3 * (6.0 * u).sin() * (5.0 * v).cos() + 0.05 * (23.0 * u + 17.0 * v).sin()
        })
        .collect();
    let cfg = TerrainConfig {
        heightmap_path: String::new(),
        width: 64.0,
        depth: 48.0,
        height_scale: 6.0,
        chunk_resolution: 8,
        texture_tile: 1.0,
        splat_map_path: String::new(),
        layer_textures: Default::default(),
        metallic: 0.0,
        roughness: 1.0,
        grass: None,
    };
    Terrain::generate(&Heightmap::from_raw(heights, w, d), &cfg)
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Collider {
    /// The chunks' triangles, as the player registered them before #1219
    Trimesh,
    Heightfield,
}

/// The character walking forward (+Z) for five seconds over the hills,
/// across two seams, and its feet each step.
fn walk(kind: Collider) -> Vec<([f32; 3], bool)> {
    let terrain = hills();
    let mut world = FlintWorld::new();
    let mut sys = PhysicsSystem::new();
    let ground = world.spawn("terrain").unwrap();
    let [x, y, z] = OFFSET;
    match kind {
        Collider::Trimesh => {
            let (verts, tris) = terrain.trimesh_data();
            let verts = verts
                .iter()
                .map(|v| [v[0] + x, v[1] + y, v[2] + z])
                .collect();
            sys.register_static_trimesh(ground, verts, tris, 0.8, 0.1);
        }
        Collider::Heightfield => {
            let grid = terrain.height_grid();
            let field = StaticHeightfield {
                columns: grid.columns,
                rows: grid.rows,
                heights: grid.heights,
                width: grid.width,
                depth: grid.depth,
                origin: OFFSET,
            };
            sys.register_static_heightfield(ground, &field, 0.8, 0.1);
        }
    }
    let id = spawn_from_toml(
        &mut world,
        "player",
        &format!(
            "[transform]\nposition = [{}, {}, {}]\n\
             [rigidbody]\nbody_type = \"kinematic\"\n\
             [collider]\nshape = \"capsule\"\nsize = [0.6, 1.8, 0.6]\n\
             [character_controller]\nstart_yaw = 20.0\n",
            OFFSET[0] + 13.0,
            OFFSET[1] + 9.0,
            OFFSET[2] + 10.0
        ),
    );
    sys.initialize(&mut world).unwrap();
    let mut cc = CharacterController::new();
    cc.enter(&world, &mut sys);
    // Rays see only what a step has put in the query pipeline
    step(&mut sys, &mut world, DT);

    let mut input = InputState::new();
    let mut feet = Vec::new();
    for n in 0..360 {
        if n == 60 {
            input.process_key_down(flint_runtime::parse_key_code("KeyW").unwrap());
        }
        cc.frame_input(&input, &world);
        cc.update(&input, &mut world, &mut sys, DT);
        step(&mut sys, &mut world, DT);
        let p = world.get_transform(id).unwrap().position;
        feet.push(([p.x, p.y, p.z], cc.grounded()));
    }
    feet
}

#[test]
fn the_character_walks_the_heightfield_as_it_walked_the_trimesh() {
    let mesh = walk(Collider::Trimesh);
    let field = walk(Collider::Heightfield);
    let (start, end) = (field[59].0, field.last().unwrap().0);
    // It walked from before the seam at z = 16 to past the one at z = 32
    assert!(
        start[2] < OFFSET[2] + 16.0 && end[2] > OFFSET[2] + 32.0,
        "{start:?} to {end:?}"
    );
    let (mut worst, mut grounded_apart) = (0.0f32, Vec::new());
    for (n, (a, b)) in mesh.iter().zip(&field).enumerate() {
        let gap = (0..3)
            .map(|i| (a.0[i] - b.0[i]).powi(2))
            .sum::<f32>()
            .sqrt();
        worst = worst.max(gap);
        assert!(gap < 0.01, "step {n}: trimesh {a:?}, heightfield {b:?}");
        if n > 30 && a.1 != b.1 {
            grounded_apart.push(n);
        }
    }
    // Walking downhill the controller's snap can land a millimetre either
    // side of its reach for one step: measured, 1 step in 330 (step 302)
    assert!(
        grounded_apart.len() <= 3,
        "grounded differs at steps {grounded_apart:?}"
    );
    eprintln!(
        "walked {:.1} m; worst gap {worst:e} m; grounded differs at {grounded_apart:?}",
        end[2] - start[2]
    );
}
