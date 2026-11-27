//! The vehicle package's player part: what a launcher lists
//! ([`PACKAGE`]). Its subsystem drives every `vehicle` in a scene, and its
//! script module and panel are registered with it.
//!
//! The frame, as ADR 0140 section 6 lays it out:
//!
//! | when | what |
//! |---|---|
//! | `scene_loaded` | one [`model::Vehicle`] per `vehicle`, its wheels its children with `vehicle_wheel` |
//! | `Stage::PreScripts` | `vehicle_state` and each wheel's state from the last step; a player's driving input from the actions |
//! | `fixed_update`, each step | the tuning and `vehicle_input` read, Rapier's raycast wheels updated, the assists and the push applied |
//! | `Stage::Physics` | the wheel children's poses; the modifiers back to neutral |

pub mod model;
mod module;
mod panel;
mod subsystem;

use crate::{PACKAGE_NAME, VEHICLE, VEHICLE_INPUT, VEHICLE_STATE, VEHICLE_WHEEL};
use flint_package::{Package, Registry, SceneCtx};

pub use subsystem::Vehicles;

/// The `vehicle` package (`packages/vehicle/package.toml`).
pub static PACKAGE: VehiclePackage = VehiclePackage;

pub struct VehiclePackage;

impl Package for VehiclePackage {
    fn name(&self) -> &'static str {
        PACKAGE_NAME
    }

    /// Its builder, its line for a project that leaves it out, its script
    /// module, and its panel at the kernel's point for a scene's panels
    /// (made only where the player has the debug HUD, so absent without
    /// the debug-hud package).
    fn register(&self, registry: &mut Registry) {
        registry
            .scene_subsystem(subsystem::for_scene)
            .when_left_out(left_out)
            .script_modules(&[&module::VEHICLE_MODULE]);
        if let Some(panels) = registry.point_mut::<flint_package::panels::ScenePanels>() {
            panels.0.push((PACKAGE_NAME, panel::for_scene));
        }
    }
}

/// What a scene with vehicles hears when the project's `[packages]` leaves
/// `vehicle` out.
fn left_out(ctx: &SceneCtx) -> Option<String> {
    let in_use = ctx.packages?;
    let uses: Vec<&str> = [VEHICLE, VEHICLE_WHEEL, VEHICLE_INPUT, VEHICLE_STATE]
        .into_iter()
        .filter(|c| !ctx.world.entities_with_component(c).is_empty())
        .collect();
    flint_package::left_out_line(in_use, PACKAGE_NAME, &uses, "sit undriven")
}

#[cfg(test)]
mod tests;
