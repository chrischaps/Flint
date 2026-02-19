//! Entity inspector panel — displays components and provides editable transform fields

use flint_core::EntityId;
use flint_ecs::FlintWorld;

/// Action returned from the inspector when a transform is edited.
pub enum InspectorAction {
    None,
    TransformChanged {
        entity_id: EntityId,
        entity_name: String,
        old_position: [f32; 3],
        new_position: [f32; 3],
    },
}

/// Entity inspector — displays all components for a selected entity,
/// with editable DragValue fields for the transform.
pub struct EntityInspector {
    // Cached values for DragValue editing (egui needs mutable references)
    pos: [f32; 3],
    rot: [f32; 3],
    scale: [f32; 3],
    cached_entity: Option<EntityId>,
}

impl Default for EntityInspector {
    fn default() -> Self {
        Self::new()
    }
}

impl EntityInspector {
    pub fn new() -> Self {
        Self {
            pos: [0.0; 3],
            rot: [0.0; 3],
            scale: [1.0, 1.0, 1.0],
            cached_entity: None,
        }
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, world: &FlintWorld, entity_id: EntityId) -> InspectorAction {
        ui.heading("Entity Inspector");
        ui.separator();

        // Entity info
        let entities = world.all_entities();
        let entity = entities.iter().find(|e| e.id == entity_id);

        let Some(entity) = entity else {
            ui.label("Entity not found");
            return InspectorAction::None;
        };

        let entity_name = entity.name.clone();

        ui.label(format!("Name: {}", entity.name));
        ui.label(format!(
            "Archetype: {}",
            entity.archetype.as_deref().unwrap_or("(none)")
        ));
        ui.label(format!("ID: {}", entity.id.raw()));

        // Refresh cached values when entity changes or on first load
        let transform = world.get_transform(entity_id).unwrap_or_default();
        if self.cached_entity != Some(entity_id) {
            self.pos = [transform.position.x, transform.position.y, transform.position.z];
            self.rot = [transform.rotation.x, transform.rotation.y, transform.rotation.z];
            self.scale = [transform.scale.x, transform.scale.y, transform.scale.z];
            self.cached_entity = Some(entity_id);
        }

        // Sync from world if not actively dragging (handles external changes like gizmo drags)
        // We detect "not dragging" by comparing — if values match world, keep ours; if world changed, sync.
        let world_pos = [transform.position.x, transform.position.y, transform.position.z];
        let world_rot = [transform.rotation.x, transform.rotation.y, transform.rotation.z];
        let world_scale = [transform.scale.x, transform.scale.y, transform.scale.z];

        // Transform section with editable DragValues
        let mut action = InspectorAction::None;

        ui.separator();
        egui::CollapsingHeader::new("Transform").default_open(true).show(ui, |ui| {
            let old_pos = self.pos;

            // Sync position from world when it changes externally
            if !ui.ctx().is_using_pointer() {
                self.pos = world_pos;
            }

            ui.horizontal(|ui| {
                ui.label("Position:");
                ui.colored_label(egui::Color32::from_rgb(214, 67, 67), "X");
                ui.add(egui::DragValue::new(&mut self.pos[0]).speed(0.1).range(f32::MIN..=f32::MAX));
                ui.colored_label(egui::Color32::from_rgb(67, 172, 67), "Y");
                ui.add(egui::DragValue::new(&mut self.pos[1]).speed(0.1).range(f32::MIN..=f32::MAX));
                ui.colored_label(egui::Color32::from_rgb(67, 118, 214), "Z");
                ui.add(egui::DragValue::new(&mut self.pos[2]).speed(0.1).range(f32::MIN..=f32::MAX));
            });

            // Detect if position changed via DragValue
            if (self.pos[0] - old_pos[0]).abs() > 1e-6
                || (self.pos[1] - old_pos[1]).abs() > 1e-6
                || (self.pos[2] - old_pos[2]).abs() > 1e-6
            {
                action = InspectorAction::TransformChanged {
                    entity_id,
                    entity_name: entity_name.clone(),
                    old_position: world_pos,
                    new_position: self.pos,
                };
            }

            // Rotation (read-only for now — editable rotation comes later)
            self.rot = world_rot;
            ui.horizontal(|ui| {
                ui.label("Rotation:");
                ui.monospace(format!(
                    "[{:.2}, {:.2}, {:.2}]",
                    self.rot[0], self.rot[1], self.rot[2]
                ));
            });

            // Scale (read-only for now)
            self.scale = world_scale;
            ui.horizontal(|ui| {
                ui.label("Scale:");
                ui.monospace(format!(
                    "[{:.2}, {:.2}, {:.2}]",
                    self.scale[0], self.scale[1], self.scale[2]
                ));
            });
        });

        // Dynamic components
        if let Some(components) = world.get_components(entity_id) {
            for (comp_name, comp_value) in components.data.iter() {
                ui.separator();
                egui::CollapsingHeader::new(comp_name).default_open(true).show(ui, |ui| {
                    display_toml_value(ui, comp_value);
                });
            }
        }

        action
    }

    /// Force-refresh cached values from the world (after undo/redo or gizmo drag).
    pub fn invalidate_cache(&mut self) {
        self.cached_entity = None;
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
