//! The Vehicles panel (ADR 0140 section 7): each vehicle's speed, slip and
//! yaw rate against the assist's target, each wheel's contact and spring,
//! and sliders for the tuning, written back to the `vehicle` component so
//! the next step drives with them.
//!
//! Made at the kernel's point for a scene's panels (`ScenePanels`), which
//! the player fills only where it has the debug HUD: a build without the
//! debug-hud package has no panel and pays nothing for it.

use crate::{VEHICLE, VEHICLE_STATE, VEHICLE_WHEEL};
use flint_core::EntityId;
use flint_ecs::FlintWorld;
use flint_package::egui;
use flint_package::panels::{DebugPanel, PanelCtx};

/// The panel's name, its title in F3.
pub const PANEL: &str = "Vehicles";

/// The tuning the sliders cover: field, range, its schema default.
const SLIDERS: [(&str, f32, f32, f32); 11] = [
    ("top_speed", 0.0, 80.0, 30.0),
    ("accel", 0.0, 60.0, 15.0),
    ("drag", 0.0, 3.0, 0.5),
    ("grip", 0.0, 30.0, 9.0),
    ("yaw_assist", 0.0, 30.0, 10.0),
    ("upright", 0.0, 200.0, 60.0),
    ("scrub", 0.0, 5.0, 0.0),
    ("air_control", 0.0, 1.0, 0.0),
    ("tyre_grip", 0.0, 5.0, 1.8),
    ("suspension_stiffness", 0.0, 200.0, 40.0),
    ("suspension_damping", 0.0, 20.0, 3.5),
];

/// One wheel as the panel shows it.
#[derive(Debug, Clone, Default)]
struct WheelRow {
    name: String,
    contact: bool,
    compression: f32,
    slip: f32,
}

/// One vehicle as the panel shows it.
#[derive(Debug, Clone)]
struct Row {
    entity: EntityId,
    name: String,
    speed: f32,
    slip: f32,
    yaw_rate: f32,
    yaw_target: f32,
    airborne: f32,
    contact: bool,
    tuning: Vec<f32>,
    wheels: Vec<WheelRow>,
}

pub struct VehiclesPanel {
    open: bool,
    rows: Vec<Row>,
    /// Slider moves since the last sync: (vehicle, field, value).
    edits: Vec<(EntityId, &'static str, f32)>,
}

/// The panel, in a scene with a vehicle.
pub fn for_scene(world: &FlintWorld) -> Option<Box<dyn DebugPanel>> {
    (!world.entities_with_component(VEHICLE).is_empty()).then(|| {
        Box::new(VehiclesPanel {
            open: false,
            rows: Vec::new(),
            edits: Vec::new(),
        }) as Box<dyn DebugPanel>
    })
}

impl VehiclesPanel {
    fn read(world: &FlintWorld) -> Vec<Row> {
        world
            .entities_with_component(VEHICLE)
            .iter()
            .map(|&entity| {
                let s = world.view(entity, VEHICLE_STATE);
                let t = world.view(entity, VEHICLE);
                let wheels = world
                    .children(entity)
                    .iter()
                    .filter(|c| world.get_component(**c, VEHICLE_WHEEL).is_some())
                    .map(|&c| {
                        let w = world.view(c, VEHICLE_WHEEL);
                        WheelRow {
                            name: world.get_name(c).unwrap_or("?").to_string(),
                            contact: w.bool("contact", false),
                            compression: w.f32("compression", 0.0),
                            slip: w.f32("slip", 0.0),
                        }
                    })
                    .collect();
                Row {
                    entity,
                    name: world.get_name(entity).unwrap_or("?").to_string(),
                    speed: s.f32("speed", 0.0),
                    slip: s.f32("slip_angle", 0.0),
                    yaw_rate: s.f32("yaw_rate", 0.0),
                    yaw_target: s.f32("yaw_target", 0.0),
                    airborne: s.f32("airborne_time", 0.0),
                    contact: s.bool("contact", false),
                    tuning: SLIDERS.iter().map(|(f, _, _, d)| t.f32(f, *d)).collect(),
                    wheels,
                }
            })
            .collect()
    }
}

impl DebugPanel for VehiclesPanel {
    fn name(&self) -> &str {
        PANEL
    }

    fn ui(&mut self, ui: &mut egui::Ui) {
        if self.rows.is_empty() {
            ui.label("No vehicles");
            return;
        }
        for row in &mut self.rows {
            egui::CollapsingHeader::new(&row.name)
                .id_salt(("vehicle", row.entity.raw()))
                .default_open(true)
                .show(ui, |ui| {
                    ui.monospace(format!(
                        "{:6.1} m/s  slip {:5.1}°{}{}",
                        row.speed,
                        row.slip,
                        if row.airborne > 0.0 {
                            format!("  air {:.2} s", row.airborne)
                        } else {
                            String::new()
                        },
                        if row.contact { "  contact" } else { "" },
                    ));
                    ui.monospace(format!(
                        "yaw {:6.1}°/s  target {:6.1}°/s",
                        row.yaw_rate, row.yaw_target
                    ));
                    for w in &row.wheels {
                        ui.horizontal(|ui| {
                            ui.monospace(format!(
                                "{} {:<10} {:3.0}%  slip {:5.1}°",
                                if w.contact { "●" } else { "○" },
                                w.name,
                                w.compression * 100.0,
                                w.slip
                            ));
                        });
                    }
                    for (i, (field, lo, hi, _)) in SLIDERS.iter().enumerate() {
                        let value = &mut row.tuning[i];
                        if ui
                            .add(egui::Slider::new(value, *lo..=*hi).text(*field))
                            .changed()
                        {
                            self.edits.push((row.entity, field, *value));
                        }
                    }
                });
        }
    }

    fn is_open(&self) -> bool {
        self.open
    }

    fn toggle(&mut self) {
        self.open = !self.open;
    }

    fn weight(&self) -> u32 {
        4 + self
            .rows
            .iter()
            .map(|r| 12 + r.wheels.len() as u32)
            .sum::<u32>()
    }

    /// The sliders' moves into the `vehicle` components, then the state as
    /// it now stands.
    fn sync(&mut self, ctx: &mut PanelCtx) {
        for (entity, field, value) in self.edits.drain(..) {
            let _ = ctx
                .world
                .set_field(entity, VEHICLE, field, toml::Value::Float(value as f64));
        }
        self.rows = Self::read(ctx.world);
    }

    fn is_dirty(&self) -> bool {
        false
    }

    fn clear_dirty(&mut self) {}

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }
}
