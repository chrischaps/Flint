//! An inline `particle_emitter` through the real scene load, with the
//! engine's schemas (#1051). The loader writes each schema field's
//! `default` into a component that leaves it out, and the scene pass warns
//! about each key the schema doesn't declare, so the schema file decides
//! what the emitter is handed and what every load says about it: a test on
//! a hand-built table misses both.

use flint_core::components::PARTICLE_EMITTER;
use flint_particles::{EmissionShape, EmitterConfig, EmitterDef, ResolveContext};
use flint_scene::{diagnose_scene, load_scene_string, SceneFile};
use flint_schema::SchemaRegistry;

fn schemas() -> SchemaRegistry {
    SchemaRegistry::load_from_directory("../../schemas").unwrap()
}

fn scene(emitter: &str) -> String {
    format!("[scene]\nname = \"t\"\n\n[entities.fire.particle_emitter]\n{emitter}\n")
}

/// The emitter the player builds from `emitter` once the scene has loaded.
fn loaded(emitter: &str) -> EmitterConfig {
    let (world, _) = load_scene_string(&scene(emitter), &schemas()).unwrap();
    let id = world.get_id("fire").unwrap();
    let table = world
        .get_components(id)
        .and_then(|c| c.get(PARTICLE_EMITTER))
        .and_then(|v| v.as_table())
        .unwrap()
        .clone();
    EmitterDef::from_component(&table)
        .unwrap()
        .resolve(ResolveContext::inline())
        .unwrap()
}

/// The same emitter read straight from the text, no schema in the way.
fn as_written(emitter: &str) -> EmitterConfig {
    let table: toml::Table = toml::from_str(emitter).unwrap();
    EmitterDef::from_component(&table)
        .unwrap()
        .resolve(ResolveContext::inline())
        .unwrap()
}

/// What the load says about the scene.
fn said(emitter: &str) -> Vec<String> {
    let file = SceneFile::parse(&scene(emitter)).unwrap();
    diagnose_scene(&file, None, &schemas())
        .iter()
        .map(|d| d.to_string())
        .collect()
}

/// Every modern key `EmitterDef` reads, in the forms the schema has no
/// plain type for: a `[min, max]` range, curves, a shape table, bursts,
/// forces and a sub-emitter.
const MODERN: &str = r#"
name = "flames"
lifetime = [0.4, 0.9]
speed = 2.0
size = { start = [0.3, 0.3], end = [0.0, 0.0] }
color = { keys = [{ t = 0.0, v = [1.0, 0.8, 0.3, 1.0] }, { t = 1.0, v = [0.6, 0.1, 0.0, 0.0] }] }
alpha = { start = 1.0, end = 0.0 }
size_scale = [0.8, 1.2]
brightness = [0.9, 1.1]
rotation = [0, 90]
angular_velocity = 180
speed_curve = { start = 1.0, end = 0.2 }
shape = { kind = "cone", radius = 0.2, angle = 20 }
bursts = [{ time = 0.0, count = [4, 8] }]
forces = [{ kind = "drag", coefficient = 0.5 }]
on_death = { emitter = "smoke", count = 1 }
lighting = 0.5
fog = true
"#;

#[test]
fn a_modern_inline_emitter_loads_without_a_word() {
    assert_eq!(said(MODERN), Vec::<String>::new());
}

#[test]
fn a_typo_in_an_inline_emitter_still_warns_at_load() {
    let file = SceneFile::parse(&scene("emision_rate = 5.0\n")).unwrap();
    let said = diagnose_scene(&file, None, &schemas());
    assert_eq!(said.len(), 1, "{said:?}");
    assert_eq!(said[0].code, "unknown-field");
    let hint = said[0].hint.as_deref().unwrap_or("");
    assert!(
        hint.contains("emission_rate"),
        "names the key meant: {hint}"
    );
}

#[test]
fn an_inline_rotation_and_spin_take_effect() {
    let fire = loaded("rotation = [0, 90]\nangular_velocity = 180\n");
    let deg = std::f32::consts::PI / 180.0;
    assert!((fire.rotation_max - 90.0 * deg).abs() < 1e-5, "{fire:?}");
    assert!((fire.angular_velocity_min - 180.0 * deg).abs() < 1e-5);
    assert!((fire.angular_velocity_max - 180.0 * deg).abs() < 1e-5);
}

#[test]
fn a_legacy_cone_with_no_radius_still_spawns_from_a_point() {
    let fire = loaded("shape = \"cone\"\nshape_angle = 25.0\n");
    assert_eq!(
        fire.shape,
        EmissionShape::Cone {
            radius: 0.0,
            angle: 25.0
        }
    );
}

/// Every emitter plays after a load as it does when read as written: what
/// the schema's defaults add changes nothing the reader would have done.
#[test]
fn the_schemas_defaults_change_nothing_the_reader_does() {
    for emitter in [
        "",
        MODERN,
        "shape = \"cone\"\n",
        "shape = \"sphere\"\n",
        "shape = \"box\"\n",
        "lifetime_min = 0.5\nspeed_max = 4.0\nsize_start = 0.2\nburst_count = 3\n",
        "rotation_min = 10.0\nangular_velocity_max = 30.0\n",
    ] {
        assert_eq!(loaded(emitter), as_written(emitter), "{emitter}");
    }
}
