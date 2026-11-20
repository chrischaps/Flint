//! Coincident emitters draw in name order, in every process (#858).
//!
//! The `style-veil` golden (`tests/fixtures/scenes/style_veil.scene.toml`)
//! has two particle entities on one spot: `fire` (the campfire effect, whose
//! `puff` and `smoke` emitters blend by alpha) and `blaze` (an inline alpha
//! emitter). Order-dependent blends draw far-to-near by the emitter
//! origin's view distance (`sort_particle_draws`), and two emitters on one
//! origin tie exactly, so the order between them is the order the sync
//! hands them over in: its instances by `EntityId`.
//!
//! Before #688 a scene spawned in `HashMap` order, so `blaze` and `fire`
//! swapped ids from one process to the next and the golden drew one of two
//! images (the main binary before #688, ten runs: 7 of one PNG, 3 of the
//! other; at #688 and since, ten of one). Ids are the names' ranks now, so
//! the tie resolves by name. This test pins both halves: the tie is there
//! (if the fixture stops tying, the hazard moved and this says so) and it
//! resolves `blaze` before `fire`, whatever process loads the scene.

use std::path::{Path, PathBuf};

use flint_particles::{ParticleBlendMode, ParticleSystem};
use flint_runtime::RuntimeSystem;

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

/// One emitter's draw as the renderer is handed it.
#[derive(Debug, Clone, PartialEq)]
struct Draw {
    entity: String,
    emitter: String,
    blend: ParticleBlendMode,
    sort_key: f32,
}

/// The style-veil scene's particle draws at `--particle-time 2`, set up as
/// `flint render` sets them up (fixed step, the scene's camera).
fn veil_draws() -> Vec<Draw> {
    let root = repo();
    let scene = root.join("tests/fixtures/scenes/style_veil.scene.toml");
    let registry = flint_schema::SchemaRegistry::load_from_directories(&[
        root.join("schemas"),
        root.join("schemas/examples"),
    ])
    .unwrap();
    let (mut world, file) = flint_scene::load_scene(&scene, &registry).unwrap();
    let camera = file
        .camera
        .as_ref()
        .and_then(|c| c.position)
        .expect("the fixture has a camera position");

    let project = flint_core::project::Project::for_scene(&scene);
    let mut particles = ParticleSystem::new();
    flint_particles::load_particle_effects_from_world(
        &project,
        scene.to_str().unwrap(),
        &mut particles,
    );
    particles.initialize(&mut world).unwrap();
    particles.simulate_to(&world, 2.0, 1.0 / 60.0);
    particles.pack(Some(camera));

    particles
        .sync
        .draw_data()
        .iter()
        .map(|d| Draw {
            entity: d
                .entity_id
                .and_then(|id| world.get_name(id))
                .unwrap_or("(detached)")
                .to_string(),
            emitter: d.emitter_name.to_string(),
            blend: d.blend_mode,
            sort_key: d.sort_key,
        })
        .collect()
}

#[test]
fn coincident_alpha_emitters_tie_and_draw_in_name_order() {
    let draws = veil_draws();
    for d in &draws {
        eprintln!(
            "{:>6} / {:<6} {:?} at {:.6}",
            d.entity, d.emitter, d.blend, d.sort_key
        );
    }

    // The hazard: an alpha draw of each entity at the same distance, so
    // nothing but their order in this list decides which blends over which
    let alpha = |name: &str| {
        draws
            .iter()
            .find(|d| d.entity == name && d.blend == ParticleBlendMode::Alpha)
            .unwrap_or_else(|| panic!("no alpha draw from '{name}' in {draws:#?}"))
            .clone()
    };
    let (blaze, fire) = (alpha("blaze"), alpha("fire"));
    assert_eq!(
        blaze.sort_key.to_bits(),
        fire.sort_key.to_bits(),
        "blaze and fire no longer tie: the style-veil golden's order hazard \
         (#858) moved; update this test and the case's comment"
    );

    // The tie resolves by name: every draw of an entity is together, and
    // the entities come in name order
    let mut seen: Vec<&str> = Vec::new();
    for d in &draws {
        if seen.last() != Some(&d.entity.as_str()) {
            assert!(
                !seen.contains(&d.entity.as_str()),
                "'{}' draws in two runs: {draws:#?}",
                d.entity
            );
            seen.push(&d.entity);
        }
    }
    assert_eq!(seen, ["blaze", "fire"]);
}

#[test]
fn two_loads_hand_the_renderer_the_same_draws() {
    // Two worlds in one process count their ids alike (#688); with
    // spawn order out of the picture the whole list, order included, is
    // the scene's own
    assert_eq!(veil_draws(), veil_draws());
}
