//! Editable entity inspector panel — schema-driven widgets for component properties

use crate::undo::EditAction;
use flint_core::EntityId;
use flint_ecs::FlintWorld;
use flint_schema::{FieldType, SchemaRegistry};

/// Entity inspector — editable display of all components for a selected entity
#[derive(Default)]
pub struct EntityInspector;

impl EntityInspector {
    pub fn new() -> Self {
        Self
    }

    /// Draw the interactive inspector, returning any edit actions produced
    pub fn edit_ui(
        &self,
        ui: &mut egui::Ui,
        world: &FlintWorld,
        registry: &SchemaRegistry,
        entity_id: EntityId,
    ) -> Vec<EditAction> {
        let mut actions = Vec::new();

        ui.heading("Entity Inspector");
        ui.separator();

        // Entity info
        let entities = world.all_entities();
        let entity = entities.iter().find(|e| e.id == entity_id);

        let Some(entity) = entity else {
            ui.label("Entity not found");
            return actions;
        };

        let entity_name = entity.name.clone();

        ui.label(format!("Name: {}", entity.name));
        ui.label(format!(
            "Archetype: {}",
            entity.archetype.as_deref().unwrap_or("(none)")
        ));
        ui.label(format!("ID: {}", entity.id.raw()));

        // Transform (always present, special handling)
        let transform = world.get_transform(entity_id).unwrap_or_default();

        ui.separator();
        egui::CollapsingHeader::new("Transform")
            .default_open(true)
            .show(ui, |ui| {
                // Position
                let pos = [transform.position.x, transform.position.y, transform.position.z];
                if let Some(new_pos) = vec3_edit(ui, "Position", pos, 0.1) {
                    let old_val = vec3_to_toml(pos);
                    let new_val = vec3_to_toml(new_pos);
                    actions.push(EditAction {
                        entity_id,
                        component: "transform".to_string(),
                        field: "position".to_string(),
                        old_value: old_val,
                        new_value: new_val,
                    });
                }

                // Rotation
                let rot = [transform.rotation.x, transform.rotation.y, transform.rotation.z];
                if let Some(new_rot) = vec3_edit(ui, "Rotation", rot, 1.0) {
                    let old_val = vec3_to_toml(rot);
                    let new_val = vec3_to_toml(new_rot);
                    actions.push(EditAction {
                        entity_id,
                        component: "transform".to_string(),
                        field: "rotation".to_string(),
                        old_value: old_val,
                        new_value: new_val,
                    });
                }

                // Scale
                let scl = [transform.scale.x, transform.scale.y, transform.scale.z];
                if let Some(new_scl) = vec3_edit(ui, "Scale", scl, 0.01) {
                    let old_val = vec3_to_toml(scl);
                    let new_val = vec3_to_toml(new_scl);
                    actions.push(EditAction {
                        entity_id,
                        component: "transform".to_string(),
                        field: "scale".to_string(),
                        old_value: old_val,
                        new_value: new_val,
                    });
                }
            });

        // Dynamic components
        if let Some(components) = world.get_components(entity_id) {
            for (comp_name, comp_value) in components.data.iter() {
                if comp_name == "transform" {
                    continue; // Already handled above
                }

                let schema = registry.get_component(comp_name);
                ui.separator();
                egui::CollapsingHeader::new(comp_name)
                    .default_open(true)
                    .show(ui, |ui| {
                        if let Some(table) = comp_value.as_table() {
                            for (field_name, field_value) in table {
                                let field_schema = schema.and_then(|s| s.get_field(field_name));

                                if let Some(fs) = field_schema {
                                    if let Some(new_val) = edit_field(
                                        ui,
                                        field_name,
                                        field_value,
                                        &fs.field_type,
                                        fs.min,
                                        fs.max,
                                    ) {
                                        actions.push(EditAction {
                                            entity_id,
                                            component: comp_name.clone(),
                                            field: field_name.clone(),
                                            old_value: field_value.clone(),
                                            new_value: new_val,
                                        });
                                    }
                                } else {
                                    // No schema — read-only fallback
                                    ui.horizontal(|ui| {
                                        ui.label(format!("{}:", field_name));
                                        ui.monospace(format_toml_leaf(field_value));
                                    });
                                }
                            }
                        } else {
                            display_toml_value(ui, comp_value);
                        }
                    });
            }
        }

        actions
    }
}

/// Edit a field based on its schema type. Returns Some(new_value) if changed.
fn edit_field(
    ui: &mut egui::Ui,
    name: &str,
    value: &toml::Value,
    field_type: &FieldType,
    min: Option<f64>,
    max: Option<f64>,
) -> Option<toml::Value> {
    match field_type {
        FieldType::Bool => {
            let mut b = value.as_bool().unwrap_or(false);
            let original = b;
            ui.horizontal(|ui| {
                ui.checkbox(&mut b, name);
            });
            if b != original {
                Some(toml::Value::Boolean(b))
            } else {
                None
            }
        }

        FieldType::I32 | FieldType::I64 => {
            let mut v = value.as_integer().unwrap_or(0);
            let original = v;
            ui.horizontal(|ui| {
                ui.label(format!("{}:", name));
                let mut drag = egui::DragValue::new(&mut v).speed(1);
                if let Some(lo) = min {
                    drag = drag.range(lo as i64..=max.unwrap_or(f64::MAX) as i64);
                }
                ui.add(drag);
            });
            if v != original {
                Some(toml::Value::Integer(v))
            } else {
                None
            }
        }

        FieldType::F32 | FieldType::F64 => {
            let mut v = value
                .as_float()
                .or_else(|| value.as_integer().map(|i| i as f64))
                .unwrap_or(0.0);
            let original = v;
            ui.horizontal(|ui| {
                ui.label(format!("{}:", name));
                let mut drag = egui::DragValue::new(&mut v).speed(0.01).max_decimals(4);
                if let Some(lo) = min {
                    drag = drag.range(lo..=max.unwrap_or(f64::MAX));
                }
                ui.add(drag);
            });
            if (v - original).abs() > f64::EPSILON {
                Some(toml::Value::Float(v))
            } else {
                None
            }
        }

        FieldType::String => {
            let mut s = value.as_str().unwrap_or("").to_string();
            let original = s.clone();
            ui.horizontal(|ui| {
                ui.label(format!("{}:", name));
                ui.text_edit_singleline(&mut s);
            });
            if s != original {
                Some(toml::Value::String(s))
            } else {
                None
            }
        }

        FieldType::Vec3 => {
            let arr = extract_vec3_f32(value).unwrap_or([0.0, 0.0, 0.0]);
            if let Some(new_arr) = vec3_edit(ui, name, arr, 0.1) {
                Some(vec3_to_toml(new_arr))
            } else {
                None
            }
        }

        FieldType::Color => {
            let arr = extract_color_f32(value).unwrap_or([1.0, 1.0, 1.0, 1.0]);
            let mut rgba = arr;
            let original = arr;
            ui.horizontal(|ui| {
                ui.label(format!("{}:", name));
                ui.color_edit_button_rgba_unmultiplied(&mut rgba);
            });
            let changed = (0..4).any(|i| (rgba[i] - original[i]).abs() > f32::EPSILON);
            if changed {
                Some(color_to_toml(rgba))
            } else {
                None
            }
        }

        FieldType::Enum { values } => {
            let mut current = value.as_str().unwrap_or("").to_string();
            let original = current.clone();
            ui.horizontal(|ui| {
                ui.label(format!("{}:", name));
                egui::ComboBox::from_id_salt(name)
                    .selected_text(&current)
                    .show_ui(ui, |ui| {
                        for v in values {
                            ui.selectable_value(&mut current, v.clone(), v);
                        }
                    });
            });
            if current != original {
                Some(toml::Value::String(current))
            } else {
                None
            }
        }

        FieldType::Transform => {
            // Transform sub-fields rendered as 3 vec3 rows
            ui.label(format!("{}:", name));
            None // Transform fields are handled by the top-level Transform section
        }

        FieldType::Array { .. } => {
            // Read-only for arrays
            ui.horizontal(|ui| {
                ui.label(format!("{}:", name));
                ui.monospace(format_toml_leaf(value));
            });
            None
        }
    }
}

/// Vec3 editor: returns Some([x, y, z]) if any component changed
fn vec3_edit(ui: &mut egui::Ui, label: &str, val: [f32; 3], speed: f64) -> Option<[f32; 3]> {
    let mut result = val;
    let mut changed = false;

    ui.horizontal(|ui| {
        ui.label(format!("{}:", label));

        // X (red tint)
        ui.visuals_mut().widgets.inactive.fg_stroke.color = egui::Color32::from_rgb(220, 80, 80);
        if ui
            .add(egui::DragValue::new(&mut result[0]).speed(speed).max_decimals(3).prefix("X ").custom_formatter(|v, _| format!("{:.3}", v)))
            .changed()
        {
            changed = true;
        }

        // Y (green tint)
        ui.visuals_mut().widgets.inactive.fg_stroke.color = egui::Color32::from_rgb(80, 180, 80);
        if ui
            .add(egui::DragValue::new(&mut result[1]).speed(speed).max_decimals(3).prefix("Y ").custom_formatter(|v, _| format!("{:.3}", v)))
            .changed()
        {
            changed = true;
        }

        // Z (blue tint)
        ui.visuals_mut().widgets.inactive.fg_stroke.color = egui::Color32::from_rgb(80, 120, 220);
        if ui
            .add(egui::DragValue::new(&mut result[2]).speed(speed).max_decimals(3).prefix("Z ").custom_formatter(|v, _| format!("{:.3}", v)))
            .changed()
        {
            changed = true;
        }

        // Reset visuals
        ui.visuals_mut().widgets.inactive.fg_stroke.color = egui::Color32::from_rgb(180, 180, 180);
    });

    if changed { Some(result) } else { None }
}

// --- Value conversion helpers ---

fn vec3_to_toml(v: [f32; 3]) -> toml::Value {
    toml::Value::Array(vec![
        toml::Value::Float(v[0] as f64),
        toml::Value::Float(v[1] as f64),
        toml::Value::Float(v[2] as f64),
    ])
}

fn color_to_toml(c: [f32; 4]) -> toml::Value {
    toml::Value::Array(vec![
        toml::Value::Float(c[0] as f64),
        toml::Value::Float(c[1] as f64),
        toml::Value::Float(c[2] as f64),
        toml::Value::Float(c[3] as f64),
    ])
}

fn extract_vec3_f32(value: &toml::Value) -> Option<[f32; 3]> {
    if let Some(arr) = value.as_array() {
        if arr.len() >= 3 {
            let x = arr[0].as_float().or_else(|| arr[0].as_integer().map(|i| i as f64))? as f32;
            let y = arr[1].as_float().or_else(|| arr[1].as_integer().map(|i| i as f64))? as f32;
            let z = arr[2].as_float().or_else(|| arr[2].as_integer().map(|i| i as f64))? as f32;
            return Some([x, y, z]);
        }
    }
    if let Some(table) = value.as_table() {
        let x = table.get("x").and_then(|v| v.as_float().or_else(|| v.as_integer().map(|i| i as f64)))? as f32;
        let y = table.get("y").and_then(|v| v.as_float().or_else(|| v.as_integer().map(|i| i as f64)))? as f32;
        let z = table.get("z").and_then(|v| v.as_float().or_else(|| v.as_integer().map(|i| i as f64)))? as f32;
        return Some([x, y, z]);
    }
    None
}

fn extract_color_f32(value: &toml::Value) -> Option<[f32; 4]> {
    let arr = value.as_array()?;
    if arr.len() < 3 {
        return None;
    }
    let r = arr[0].as_float().or_else(|| arr[0].as_integer().map(|i| i as f64))? as f32;
    let g = arr[1].as_float().or_else(|| arr[1].as_integer().map(|i| i as f64))? as f32;
    let b = arr[2].as_float().or_else(|| arr[2].as_integer().map(|i| i as f64))? as f32;
    let a = if arr.len() >= 4 {
        arr[3].as_float().or_else(|| arr[3].as_integer().map(|i| i as f64)).unwrap_or(1.0) as f32
    } else {
        1.0
    };
    Some([r, g, b, a])
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
