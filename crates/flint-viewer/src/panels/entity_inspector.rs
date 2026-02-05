//! Read-only entity inspector panel — displays components and their properties

use flint_core::EntityId;
use flint_ecs::FlintWorld;

/// Entity inspector — read-only display of all components for a selected entity
#[derive(Default)]
pub struct EntityInspector;

impl EntityInspector {
    pub fn new() -> Self {
        Self
    }

    pub fn ui(&self, ui: &mut egui::Ui, world: &FlintWorld, entity_id: EntityId) {
        ui.heading("Entity Inspector");
        ui.separator();

        // Entity info
        let entities = world.all_entities();
        let entity = entities.iter().find(|e| e.id == entity_id);

        let Some(entity) = entity else {
            ui.label("Entity not found");
            return;
        };

        ui.label(format!("Name: {}", entity.name));
        ui.label(format!(
            "Archetype: {}",
            entity.archetype.as_deref().unwrap_or("(none)")
        ));
        ui.label(format!("ID: {}", entity.id.raw()));

        // Transform
        let transform = world.get_transform(entity_id).unwrap_or_default();
        ui.separator();
        ui.collapsing("Transform", |ui| {
            ui.horizontal(|ui| {
                ui.label("Position:");
                ui.monospace(format!(
                    "[{:.2}, {:.2}, {:.2}]",
                    transform.position.x, transform.position.y, transform.position.z
                ));
            });
            ui.horizontal(|ui| {
                ui.label("Rotation:");
                ui.monospace(format!(
                    "[{:.2}, {:.2}, {:.2}]",
                    transform.rotation.x, transform.rotation.y, transform.rotation.z
                ));
            });
            ui.horizontal(|ui| {
                ui.label("Scale:");
                ui.monospace(format!(
                    "[{:.2}, {:.2}, {:.2}]",
                    transform.scale.x, transform.scale.y, transform.scale.z
                ));
            });
        });

        // Dynamic components
        if let Some(components) = world.get_components(entity_id) {
            for (comp_name, comp_value) in components.data.iter() {
                ui.separator();
                ui.collapsing(comp_name, |ui| {
                    display_toml_value(ui, comp_value);
                });
            }
        }
    }
}

/// Recursively display a TOML value as a read-only tree
fn display_toml_value(ui: &mut egui::Ui, value: &toml::Value) {
    match value {
        toml::Value::Table(table) => {
            for (key, val) in table {
                match val {
                    toml::Value::Table(_) | toml::Value::Array(_) => {
                        ui.collapsing(key, |ui| {
                            display_toml_value(ui, val);
                        });
                    }
                    _ => {
                        ui.horizontal(|ui| {
                            ui.label(format!("{}:", key));
                            ui.monospace(format_toml_leaf(val));
                        });
                    }
                }
            }
        }
        toml::Value::Array(arr) => {
            for (i, val) in arr.iter().enumerate() {
                match val {
                    toml::Value::Table(_) | toml::Value::Array(_) => {
                        ui.collapsing(format!("[{}]", i), |ui| {
                            display_toml_value(ui, val);
                        });
                    }
                    _ => {
                        ui.horizontal(|ui| {
                            ui.label(format!("[{}]:", i));
                            ui.monospace(format_toml_leaf(val));
                        });
                    }
                }
            }
        }
        _ => {
            ui.monospace(format_toml_leaf(value));
        }
    }
}

fn format_toml_leaf(value: &toml::Value) -> String {
    match value {
        toml::Value::String(s) => format!("\"{}\"", s),
        toml::Value::Integer(i) => i.to_string(),
        toml::Value::Float(f) => format!("{:.4}", f),
        toml::Value::Boolean(b) => b.to_string(),
        toml::Value::Datetime(d) => d.to_string(),
        _ => value.to_string(),
    }
}
