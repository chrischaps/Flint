//! Helpers the script tests share: a `probe` component a script writes its
//! readings into, and an entity with a script on it.

use flint_core::components as comp;
use flint_ecs::FlintWorld;

pub(crate) fn probe_component(world: &mut FlintWorld, id: flint_core::EntityId) {
    world
        .set_component(
            id,
            "probe",
            toml::Value::Table({
                let mut m = toml::map::Map::new();
                for f in [
                    "coh",
                    "lx",
                    "ty",
                    "npulses",
                    "kind_ok",
                    "sec_ok",
                    "age",
                    "err",
                    "reassembly",
                    "preroll",
                ] {
                    m.insert(f.into(), toml::Value::Float(-1.0));
                }
                m.insert("bar".into(), toml::Value::Integer(-1));
                m
            }),
        )
        .unwrap();
}

pub(crate) fn probe_float(world: &FlintWorld, id: flint_core::EntityId, field: &str) -> f64 {
    world
        .get_components(id)
        .unwrap()
        .get_field("probe", field)
        .unwrap()
        .as_float()
        .unwrap()
}

/// An entity named `name` with the probe component and a script.
pub(crate) fn scripted(world: &mut FlintWorld, name: &str, source: &str) -> flint_core::EntityId {
    let id = world.spawn(name).unwrap();
    probe_component(world, id);
    world
        .set_component(
            id,
            comp::SCRIPT,
            toml::Value::Table({
                let mut m = toml::map::Map::new();
                m.insert("source".into(), toml::Value::String(source.into()));
                m
            }),
        )
        .unwrap();
    id
}
