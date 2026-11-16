//! A scene's simulation is the same every load (#688).
//!
//! Bodies are given to Rapier in the order `entities_with_component` walks
//! them. That walk was a `HashSet`, so two loads of one scene could hand
//! Rapier its bodies in two orders, and a pile's solver results (contacts
//! are solved in that order) could differ from run to run. The walk is in id
//! order now and a scene's ids come from its names, so a pile loaded twice
//! settles to the same bits.

use crate::PhysicsSystem;
use flint_ecs::FlintWorld;
use flint_schema::SchemaRegistry;
use std::fmt::Write as _;

const DT: f64 = 1.0 / 60.0;

/// A floor and `n` crates dropped in a jittered column so they tumble into
/// each other, written in a scrambled order as a hand-edited file would be.
fn pile_scene(n: usize) -> String {
    let mut text = String::from("[scene]\nname = \"pile\"\n\n");
    text.push_str(
        "[entities.floor]\n[entities.floor.transform]\nposition = [0.0, -0.5, 0.0]\n\
         [entities.floor.rigidbody]\nbody_type = \"static\"\n\
         [entities.floor.collider]\nshape = \"box\"\nsize = [20.0, 1.0, 20.0]\n\n",
    );
    // A fixed permutation of 0..n: the file's order is not the names' order
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by_key(|i| (i * 37 + 11) % n);
    for i in order {
        let x = ((i * 7) % 5) as f64 * 0.23 - 0.46;
        let z = ((i * 3) % 4) as f64 * 0.19 - 0.29;
        let y = 0.6 + i as f64 * 0.55;
        let shape = if i % 3 == 0 { "sphere" } else { "box" };
        let _ = write!(
            text,
            "[entities.crate_{i:02}]\n[entities.crate_{i:02}.transform]\n\
             position = [{x:.3}, {y:.3}, {z:.3}]\nrotation = [{a:.1}, {b:.1}, 0.0]\n\
             [entities.crate_{i:02}.rigidbody]\nbody_type = \"dynamic\"\n\
             [entities.crate_{i:02}.collider]\nshape = \"{shape}\"\nsize = [0.5, 0.5, 0.5]\n\n",
            a = (i * 13 % 90) as f64,
            b = (i * 29 % 90) as f64,
        );
    }
    text
}

/// Load the pile and step it for `steps` frames; every crate's transform as
/// written back, by name.
fn settle(scene: &str, steps: usize) -> String {
    let (mut world, _) = flint_scene::load_scene_string(scene, &SchemaRegistry::new()).unwrap();
    let mut physics = PhysicsSystem::new();
    physics.initialize(&mut world).unwrap();
    for _ in 0..steps {
        physics.step(&mut world, DT);
    }
    poses(&world)
}

fn poses(world: &FlintWorld) -> String {
    let mut out = String::new();
    for info in world.all_entities() {
        let transform = world.get_component(info.id, "transform");
        let _ = writeln!(out, "{} {:?}", info.name, transform);
    }
    out
}

#[test]
fn a_pile_loaded_twice_settles_to_the_same_bits() {
    let scene = pile_scene(40);
    let first = settle(&scene, 300);
    // The pile moved: this compares a simulation, not two rest poses
    let (rest, _) = flint_scene::load_scene_string(&scene, &SchemaRegistry::new()).unwrap();
    assert_ne!(first, poses(&rest), "the crates should have fallen");
    for run in 2..=5 {
        let again = settle(&scene, 300);
        if let Some((a, b)) = first.lines().zip(again.lines()).find(|(a, b)| a != b) {
            panic!("load {run} settled differently from load 1:\n  1: {a}\n  {run}: {b}");
        }
    }
}
