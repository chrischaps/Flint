//! The terrain's collider as a heightfield (#1219): the same answers as
//! the chunks' trimesh it replaced, and no hop at a chunk seam.

use super::*;
use flint_terrain::{Heightmap, Terrain, TerrainConfig};

const DT: f64 = 1.0 / 60.0;

fn config(width: f32, depth: f32, height_scale: f32, res: u32) -> TerrainConfig {
    TerrainConfig {
        heightmap_path: String::new(),
        width,
        depth,
        height_scale,
        chunk_resolution: res,
        texture_tile: 1.0,
        splat_map_path: String::new(),
        layer_textures: Default::default(),
        metallic: 0.0,
        roughness: 1.0,
        grass: None,
    }
}

/// Hills 64 x 48 m, 6 m high, in 4 x 3 chunks of 8 x 8 quads: a seam every
/// 16 m along both axes.
fn hills() -> (Terrain, TerrainConfig) {
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
    let cfg = config(64.0, 48.0, 6.0, 8);
    (
        Terrain::generate(&Heightmap::from_raw(heights, w, d), &cfg),
        cfg,
    )
}

/// A slope falling 1 m every 4 m along +X, flat along Z, in 4 x 1 chunks.
fn slope() -> (Terrain, TerrainConfig) {
    let (w, d) = (33u32, 9u32);
    let heights = (0..w * d)
        .map(|i| 1.0 - (i % w) as f32 / (w - 1) as f32)
        .collect();
    let cfg = config(64.0, 16.0, 16.0, 8);
    (
        Terrain::generate(&Heightmap::from_raw(heights, w, d), &cfg),
        cfg,
    )
}

const OFFSET: [f32; 3] = [-20.0, 1.5, 7.0];

#[derive(Clone, Copy, Debug, PartialEq)]
enum Collider {
    /// The chunks' triangles, as the player registered them before #1219
    Trimesh,
    Heightfield,
}

/// A world holding `terrain` at [`OFFSET`] as `kind`; `then` adds the rest
/// before the physics is initialized.
fn with_terrain(
    terrain: &Terrain,
    kind: Collider,
    then: impl FnOnce(&mut FlintWorld),
) -> (FlintWorld, PhysicsSystem, EntityId) {
    let mut world = FlintWorld::new();
    let mut sys = PhysicsSystem::new();
    let id = world.spawn("terrain").unwrap();
    let [x, y, z] = OFFSET;
    match kind {
        Collider::Trimesh => {
            let (verts, tris) = terrain.trimesh_data();
            let verts = verts
                .iter()
                .map(|v| [v[0] + x, v[1] + y, v[2] + z])
                .collect();
            sys.register_static_trimesh(id, verts, tris, 0.8, 0.1);
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
            sys.register_static_heightfield(id, &field, 0.8, 0.1);
        }
    }
    then(&mut world);
    sys.initialize(&mut world).unwrap();
    // Rays see only what a step has put in the query pipeline
    sys.step(&mut world, DT);
    (world, sys, id)
}

fn spawn(world: &mut FlintWorld, name: &str, text: &str) -> EntityId {
    let id = world.spawn(name).unwrap();
    let table: toml::Table = toml::from_str(text).unwrap();
    for (component, data) in table {
        world.set_component(id, &component, data).unwrap();
    }
    id
}

/// Rays straight down, slanted, and grazing from the side, through cell
/// edges, cell diagonals and the chunk seams.
fn rays() -> Vec<([f32; 3], [f32; 3])> {
    let mut rays = Vec::new();
    let dirs = [
        [0.0, -1.0, 0.0],
        [0.4, -1.0, 0.25],
        [-0.7, -0.5, 0.3],
        [1.0, -0.08, 0.6],
    ];
    for i in 0..=41 {
        for k in 0..=31 {
            // Steps of 1.6 m and 1.6 m: on every seam (x, z = 16, 32 ..)
            // and every other cell edge, and between them
            let (x, z) = (
                OFFSET[0] - 1.6 + i as f32 * 1.6,
                OFFSET[2] - 1.6 + k as f32 * 1.6,
            );
            let d = dirs[(i + k) % dirs.len()];
            let start = if d[1] > -0.1 {
                [x - 30.0 * d[0], OFFSET[1] + 4.0, z - 30.0 * d[2]]
            } else {
                [x, OFFSET[1] + 20.0, z]
            };
            rays.push((start, d));
        }
    }
    rays
}

/// Whether a point of [`hills`] is on a cell's edge or diagonal (cells are
/// 2 m square), where two triangles meet.
fn on_an_edge(p: [f32; 3]) -> bool {
    let (fx, fz) = ((p[0] - OFFSET[0]) / 2.0, (p[2] - OFFSET[2]) / 2.0);
    let near = |v: f32| (v - v.round()).abs() < 1e-3;
    near(fx) || near(fz) || near(fx - fz)
}

#[test]
fn raycasts_on_the_heightfield_answer_as_the_trimesh_did() {
    let (terrain, _) = hills();
    let (_, mesh, mesh_id) = with_terrain(&terrain, Collider::Trimesh, |_| {});
    let (_, field, field_id) = with_terrain(&terrain, Collider::Heightfield, |_| {});
    assert_eq!(mesh_id, field_id);

    let (mut hits, mut misses, mut faces, mut worst) = (0, 0, 0, 0.0f32);
    for (origin, dir) in rays() {
        let a = mesh.raycast(origin, dir, 200.0, None);
        let b = field.raycast(origin, dir, 200.0, None);
        match (a, b) {
            (Some(a), Some(b)) => {
                hits += 1;
                assert_eq!(a.entity_id, b.entity_id);
                let gap = (a.distance - b.distance).abs();
                worst = worst.max(gap);
                assert!(
                    gap < 2e-3,
                    "{origin:?} {dir:?}: {} vs {}",
                    a.distance,
                    b.distance
                );
                // On an edge either triangle's normal is the answer
                if !on_an_edge(b.point) {
                    let dot: f32 = (0..3).map(|i| a.normal[i] * b.normal[i]).sum();
                    assert!(
                        dot > 0.9999,
                        "{origin:?} {dir:?}: {:?} vs {:?}",
                        a.normal,
                        b.normal
                    );
                    faces += 1;
                }
            }
            (None, None) => misses += 1,
            (a, b) => panic!("{origin:?} {dir:?}: trimesh {a:?}, heightfield {b:?}"),
        }
    }
    assert!(
        hits > 800 && misses > 50 && faces > 500,
        "{hits} hits, {misses} misses, {faces} faces"
    );
    eprintln!("{hits} rays hit ({faces} inside a face), {misses} missed both; worst distance gap {worst:e} m");
}

#[test]
fn the_heightfield_reports_its_extents_where_the_terrain_is() {
    let (terrain, _) = hills();
    let (_, sys, id) = with_terrain(&terrain, Collider::Heightfield, |_| {});
    let Some(ColliderExtents::Heightfield {
        half_extents,
        centre,
    }) = sys.get_entity_collider_extents(id)
    else {
        panic!("{:?}", sys.get_entity_collider_extents(id));
    };
    assert_eq!([half_extents[0], half_extents[2]], [32.0, 24.0]);
    assert_eq!([centre[0], centre[2]], [OFFSET[0] + 32.0, OFFSET[2] + 24.0]);
}

/// The character walking forward (+Z) for five seconds over the hills,
/// across two seams, and its feet each step.
fn walk(kind: Collider) -> Vec<([f32; 3], bool)> {
    let (terrain, _) = hills();
    let mut player = None;
    let (mut world, mut sys, _) = with_terrain(&terrain, kind, |world| {
        player = Some(spawn(
            world,
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
        ));
    });
    let id = player.unwrap();
    let mut input = InputState::new();
    let mut feet = Vec::new();
    for step in 0..360 {
        if step == 60 {
            input.process_key_down(flint_runtime::parse_key_code("KeyW").unwrap());
        }
        sys.character_frame_input(&input, &world);
        sys.update_character(&input, &mut world, DT);
        sys.step(&mut world, DT);
        let p = world.get_transform(id).unwrap().position;
        feet.push(([p.x, p.y, p.z], sys.character.grounded));
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
    for (step, (a, b)) in mesh.iter().zip(&field).enumerate() {
        let gap = (0..3)
            .map(|i| (a.0[i] - b.0[i]).powi(2))
            .sum::<f32>()
            .sqrt();
        worst = worst.max(gap);
        assert!(gap < 0.01, "step {step}: trimesh {a:?}, heightfield {b:?}");
        if step > 30 && a.1 != b.1 {
            grounded_apart.push(step);
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

/// A ball rolled down [`slope`] across the seams at x = 16, 32 and 48, and
/// the most it rose off the slope (metres, along the slope's normal) with
/// where. The slope falls 1 m in 4.
fn roll(kind: Collider) -> (f32, f32) {
    let (terrain, _) = slope();
    let r = 0.25;
    let fall = 0.25f32; // metres down per metre along +X
    let surface = |x: f32| OFFSET[1] + 16.0 - (x - OFFSET[0]) * fall;
    let along = (1.0 + fall * fall).sqrt();
    let x0 = OFFSET[0] + 6.0;
    let mut ball = None;
    let (mut world, mut sys, _) = with_terrain(&terrain, kind, |world| {
        ball = Some(spawn(
            world,
            "ball",
            &format!(
                "[transform]\nposition = [{x0}, {}, {}]\n\
                 [rigidbody]\nbody_type = \"dynamic\"\nmass = 1.0\n\
                 [collider]\nshape = \"sphere\"\nsize = [{d}, {d}, {d}]\nfriction = 0.8\n",
                surface(x0) + r * along + 0.001,
                OFFSET[2] + 8.0,
                d = 2.0 * r
            ),
        ));
    });
    let id = ball.unwrap();
    let (mut worst, mut at) = (0.0f32, 0.0f32);
    for _ in 0..600 {
        sys.step(&mut world, DT);
        let p = world.get_transform(id).unwrap().position;
        if p.x > OFFSET[0] + 60.0 {
            break;
        }
        // Height of the centre over the slope, along its normal, less r
        let off = (p.y - surface(p.x)) / along - r;
        if off > worst {
            (worst, at) = (off, p.x - OFFSET[0]);
        }
    }
    let p = world.get_transform(id).unwrap().position;
    assert!(p.x > OFFSET[0] + 50.0, "it rolled across the seams: {p:?}");
    (worst, at)
}

#[test]
fn a_ball_rolled_across_a_chunk_seam_does_not_hop() {
    let (field, field_at) = roll(Collider::Heightfield);
    let (mesh, mesh_at) = roll(Collider::Trimesh);
    eprintln!(
        "rose off the slope: heightfield {field} m at x = {field_at}, \
         trimesh {mesh} m at x = {mesh_at}"
    );
    assert!(
        field < 0.005,
        "the heightfield's ball rose {field} m at x = {field_at}"
    );
    // What the test is for: the chunks' trimesh threw it 0.19 m up off a
    // cell edge at x = 42 (measured on Rapier 0.22)
    assert!(mesh > 0.05, "the trimesh's ball rose only {mesh} m");
}
