//! The `vehicle` script module (ADR 0140 section 5): what a component
//! can't say. Most of a vehicle is read and written through its
//! components (`vehicle_state`, `vehicle_input`) with `get_field` and
//! `set_field`; these add a respawn, the wheels in one call and the list.
//!
//! Enabled where a scene has a `vehicle` (the subsystem runs); elsewhere a
//! call is the "not enabled" error (#698).

use crate::{VEHICLE, VEHICLE_WHEEL};
use flint_core::EntityId;
use flint_script::api::builder::kind::{Array, Entity, Num};
use flint_script::api::{Call, Module};
use flint_script::context::ScriptCommand;
use flint_script::{ModuleCommand, ScriptModule};
use rhai::{Dynamic, Map};

/// The module's name.
pub const MODULE: &str = "vehicle";

/// `vehicle_place(id, x, y, z, yaw)`: what the next step carries out.
#[derive(Debug, Clone, PartialEq)]
pub struct Place {
    pub entity: EntityId,
    pub position: [f32; 3],
    pub yaw: f32,
}

pub struct VehicleModule;

pub static VEHICLE_MODULE: VehicleModule = VehicleModule;

fn id_dynamic(id: i64) -> Dynamic {
    Dynamic::from(id)
}

/// A wheel's state as a map: the entity, and the fields the package writes.
fn wheel_map(call: &Call<'_>, wheel: EntityId) -> Option<Map> {
    let world = call.world().ok()?;
    let v = world.view(wheel, VEHICLE_WHEEL);
    if !v.exists() {
        return None;
    }
    let mut map = Map::new();
    map.insert("entity".into(), id_dynamic(wheel.raw() as i64));
    map.insert("contact".into(), Dynamic::from(v.bool("contact", false)));
    map.insert("ground".into(), id_dynamic(v.i64("ground", -1)));
    let n = v.vec3("normal", [0.0, 1.0, 0.0]);
    let normal: rhai::Array = n.iter().map(|c| Dynamic::from(*c as f64)).collect();
    map.insert("normal".into(), Dynamic::from(normal));
    map.insert(
        "compression".into(),
        Dynamic::from(v.f32("compression", 0.0) as f64),
    );
    map.insert("slip".into(), Dynamic::from(v.f32("slip", 0.0) as f64));
    map.insert("steer".into(), Dynamic::from(v.bool("steer", false)));
    map.insert("drive".into(), Dynamic::from(v.bool("drive", false)));
    Some(map)
}

impl ScriptModule for VehicleModule {
    fn name(&self) -> &'static str {
        MODULE
    }

    fn enabled_when(&self) -> &'static str {
        "the scene has a vehicle (the vehicle package)"
    }

    fn declare(&self, m: &mut Module<'_>) {
        m.func(
            "vehicle_place",
            "Move a vehicle there, facing yaw degrees (0 faces -Z, positive left), at rest: a respawn or a restart, carried out by the next fixed step.",
        )
        .arg("id", Entity)
        .arg("x", Num)
        .arg("y", Num)
        .arg("z", Num)
        .arg("yaw", Num)
        .run(|call, (entity, x, y, z, yaw)| {
            let place = Place {
                entity,
                position: [x as f32, y as f32, z as f32],
                yaw: yaw as f32,
            };
            let function = call.name();
            call.ctx()
                .commands
                .push(ScriptCommand::Module(ModuleCommand::new(function, place)));
        });

        m.func(
            "vehicle_wheels",
            "A vehicle's wheels, each a map of its entity, contact, ground (an id or -1), normal, compression (0 to 1), slip (degrees), steer and drive.",
        )
        .arg("id", Entity)
        .returns(Array, "one map a wheel, in the children's order; empty for an entity that isn't a vehicle")
        .run(|call, (id,)| {
            let children = match call.world() {
                Ok(world) if world.get_component(id, VEHICLE).is_some() => world.children(id).to_vec(),
                _ => Vec::new(),
            };
            children
                .into_iter()
                .filter_map(|c| wheel_map(call, c))
                .map(Dynamic::from)
                .collect::<rhai::Array>()
        });

        m.func("vehicle_ids", "The scene's vehicles.")
            .returns(Array, "their ids, in id order")
            .run(|call, ()| match call.world() {
                Ok(world) => world
                    .entities_with_component(VEHICLE)
                    .iter()
                    .map(|e| id_dynamic(e.raw() as i64))
                    .collect::<rhai::Array>(),
                Err(_) => rhai::Array::new(),
            });
    }
}
