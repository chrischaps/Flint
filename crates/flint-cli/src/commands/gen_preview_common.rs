//! Shared UI helpers for `gen-preview` and `tex-edit` commands.
//!
//! Extracted from `gen_preview.rs` so both the general procgen previewer
//! and the texture pipeline node editor can reuse parameter editing widgets.

use flint_procgen::{ParamFieldSpec, ParamFieldType};

// ─── Parameter UI helpers ───────────────────────────────────────────────────

/// Group fields by underscore prefix (e.g. "trunk_height" -> "Trunk").
/// Fields without an underscore go into "General".
pub fn group_fields<'a>(fields: &'a [ParamFieldSpec]) -> Vec<(String, Vec<&'a ParamFieldSpec>)> {
    let mut groups: Vec<(String, Vec<&ParamFieldSpec>)> = Vec::new();

    for field in fields {
        let group_name = if let Some(pos) = field.name.find('_') {
            let prefix = &field.name[..pos];
            let mut chars = prefix.chars();
            match chars.next() {
                Some(c) => {
                    let upper: String = c.to_uppercase().collect::<String>() + chars.as_str();
                    upper
                }
                None => "General".to_string(),
            }
        } else {
            "General".to_string()
        };

        if let Some(group) = groups.iter_mut().find(|(name, _)| name == &group_name) {
            group.1.push(field);
        } else {
            groups.push((group_name, vec![field]));
        }
    }

    // Move "General" to the front if it exists
    if let Some(pos) = groups.iter().position(|(name, _)| name == "General") {
        if pos > 0 {
            let general = groups.remove(pos);
            groups.insert(0, general);
        }
    }

    groups
}

/// Render a single parameter field widget. Returns true if the value changed.
///
/// When `strip_prefix` is true, the group prefix (text before the first `_`)
/// is stripped from the label — useful when the field is displayed under a
/// collapsing group header that already shows the prefix.  Pass `false` to
/// keep the full field name (e.g. in inline node bodies).
pub fn render_param_field(
    ui: &mut egui::Ui,
    field: &ParamFieldSpec,
    params: &mut toml::map::Map<String, toml::Value>,
) -> bool {
    render_param_field_inner(ui, field, params, true)
}

/// Like [`render_param_field`] but with explicit control over prefix stripping.
pub fn render_param_field_full(
    ui: &mut egui::Ui,
    field: &ParamFieldSpec,
    params: &mut toml::map::Map<String, toml::Value>,
    strip_prefix: bool,
) -> bool {
    render_param_field_inner(ui, field, params, strip_prefix)
}

fn render_param_field_inner(
    ui: &mut egui::Ui,
    field: &ParamFieldSpec,
    params: &mut toml::map::Map<String, toml::Value>,
    strip_prefix: bool,
) -> bool {
    let display_name = if strip_prefix {
        if let Some(pos) = field.name.find('_') {
            &field.name[pos + 1..]
        } else {
            &field.name
        }
    } else {
        &field.name
    };

    match &field.field_type {
        ParamFieldType::Float { min, max } => {
            let current = get_param_f64(params, &field.name, &field.default);
            let mut val = current;

            ui.horizontal(|ui| {
                ui.label(format!("{}:", display_name));
                let mut drag = egui::DragValue::new(&mut val).speed(0.01).max_decimals(4);
                if let (Some(lo), Some(hi)) = (min, max) {
                    drag = drag.range(*lo..=*hi);
                } else if let Some(lo) = min {
                    drag = drag.range(*lo..=f64::MAX);
                }
                ui.add(drag);
            });

            if (val - current).abs() > f64::EPSILON {
                params.insert(field.name.clone(), toml::Value::Float(val));
                return true;
            }
        }

        ParamFieldType::Integer { min, max } => {
            let current = get_param_i64(params, &field.name, &field.default);
            let mut val = current;

            ui.horizontal(|ui| {
                ui.label(format!("{}:", display_name));
                let mut drag = egui::DragValue::new(&mut val).speed(1);
                if let (Some(lo), Some(hi)) = (min, max) {
                    drag = drag.range(*lo..=*hi);
                } else if let Some(lo) = min {
                    drag = drag.range(*lo..=i64::MAX);
                }
                ui.add(drag);
            });

            if val != current {
                params.insert(field.name.clone(), toml::Value::Integer(val));
                return true;
            }
        }

        ParamFieldType::Bool => {
            let current = get_param_bool(params, &field.name, &field.default);
            let mut val = current;

            ui.checkbox(&mut val, display_name);

            if val != current {
                params.insert(field.name.clone(), toml::Value::Boolean(val));
                return true;
            }
        }

        ParamFieldType::HexColor => {
            let current_hex = get_param_string(params, &field.name, &field.default);
            let mut rgba = hex_to_rgba(&current_hex);

            let mut changed = false;
            ui.horizontal(|ui| {
                ui.label(format!("{}:", display_name));
                changed = ui
                    .color_edit_button_srgba_unmultiplied(&mut rgba)
                    .changed();
                ui.monospace(&current_hex);
            });

            if changed {
                let new_hex = rgba_to_hex(&rgba);
                params.insert(field.name.clone(), toml::Value::String(new_hex));
                return true;
            }
        }

        ParamFieldType::Enum { values } => {
            let current = get_param_string(params, &field.name, &field.default);
            let mut selected = current.clone();

            ui.horizontal(|ui| {
                ui.label(format!("{}:", display_name));
                egui::ComboBox::from_id_salt(&field.name)
                    .selected_text(&selected)
                    .show_ui(ui, |ui| {
                        for v in values {
                            ui.selectable_value(&mut selected, v.clone(), v);
                        }
                    });
            });

            if selected != current {
                params.insert(field.name.clone(), toml::Value::String(selected));
                return true;
            }
        }

        ParamFieldType::String => {
            let current = get_param_string(params, &field.name, &field.default);
            let mut val = current.clone();

            ui.horizontal(|ui| {
                ui.label(format!("{}:", display_name));
                ui.text_edit_singleline(&mut val);
            });

            if val != current {
                params.insert(field.name.clone(), toml::Value::String(val));
                return true;
            }
        }

        ParamFieldType::StringArray { item_enum } => {
            let current_arr = get_param_string_array(params, &field.name, &field.default);
            let mut changed = false;

            if let Some(choices) = item_enum {
                // Multi-checkbox
                ui.label(format!("{}:", display_name));
                let mut selected: Vec<String> = current_arr.clone();
                for choice in choices {
                    let mut checked = selected.contains(choice);
                    if ui.checkbox(&mut checked, choice).changed() {
                        if checked && !selected.contains(choice) {
                            selected.push(choice.clone());
                        } else if !checked {
                            selected.retain(|s| s != choice);
                        }
                        changed = true;
                    }
                }
                if changed {
                    let arr = selected
                        .into_iter()
                        .map(toml::Value::String)
                        .collect::<Vec<_>>();
                    params.insert(field.name.clone(), toml::Value::Array(arr));
                }
            } else {
                // Display as read-only for now
                ui.horizontal(|ui| {
                    ui.label(format!("{}:", display_name));
                    ui.monospace(format!("{:?}", current_arr));
                });
            }

            return changed;
        }

        ParamFieldType::ObjectArray => {
            if let Some(toml::Value::Array(arr)) = params.get(&field.name) {
                let count = arr.len();
                egui::CollapsingHeader::new(format!("{} ({})", display_name, count))
                    .default_open(false)
                    .show(ui, |ui| {
                        for (i, item) in arr.iter().enumerate() {
                            let label = if let Some(toml::Value::String(name)) =
                                item.as_table().and_then(|t| t.get("name"))
                            {
                                name.clone()
                            } else {
                                format!("[{}]", i)
                            };
                            egui::CollapsingHeader::new(&label)
                                .id_salt(format!("{}_{}", field.name, i))
                                .default_open(false)
                                .show(ui, |ui| {
                                    if let Some(table) = item.as_table() {
                                        egui::Grid::new(format!("{}_{}_{}", field.name, i, "grid"))
                                            .num_columns(2)
                                            .spacing([8.0, 2.0])
                                            .show(ui, |ui| {
                                                for (k, v) in table {
                                                    ui.label(k);
                                                    ui.monospace(format_toml_value_compact(v));
                                                    ui.end_row();
                                                }
                                            });
                                    }
                                });
                        }
                    });
            } else {
                ui.horizontal(|ui| {
                    ui.label(format!("{}:", display_name));
                    ui.monospace("[]");
                });
            }
        }
    }

    false
}

// ─── Param value extraction helpers ─────────────────────────────────────────

pub fn get_param_f64(
    params: &toml::map::Map<String, toml::Value>,
    key: &str,
    default: &Option<serde_json::Value>,
) -> f64 {
    if let Some(v) = params.get(key) {
        match v {
            toml::Value::Float(f) => return *f,
            toml::Value::Integer(i) => return *i as f64,
            _ => {}
        }
    }
    default
        .as_ref()
        .and_then(|d| d.as_f64())
        .unwrap_or(0.0)
}

pub fn get_param_i64(
    params: &toml::map::Map<String, toml::Value>,
    key: &str,
    default: &Option<serde_json::Value>,
) -> i64 {
    if let Some(v) = params.get(key) {
        match v {
            toml::Value::Integer(i) => return *i,
            toml::Value::Float(f) => return *f as i64,
            _ => {}
        }
    }
    default
        .as_ref()
        .and_then(|d| d.as_i64())
        .unwrap_or(0)
}

pub fn get_param_bool(
    params: &toml::map::Map<String, toml::Value>,
    key: &str,
    default: &Option<serde_json::Value>,
) -> bool {
    if let Some(toml::Value::Boolean(b)) = params.get(key) {
        return *b;
    }
    default
        .as_ref()
        .and_then(|d| d.as_bool())
        .unwrap_or(false)
}

pub fn get_param_string(
    params: &toml::map::Map<String, toml::Value>,
    key: &str,
    default: &Option<serde_json::Value>,
) -> String {
    if let Some(toml::Value::String(s)) = params.get(key) {
        return s.clone();
    }
    default
        .as_ref()
        .and_then(|d| d.as_str())
        .unwrap_or("")
        .to_string()
}

pub fn get_param_string_array(
    params: &toml::map::Map<String, toml::Value>,
    key: &str,
    default: &Option<serde_json::Value>,
) -> Vec<String> {
    if let Some(toml::Value::Array(arr)) = params.get(key) {
        return arr
            .iter()
            .filter_map(|v| {
                if let toml::Value::String(s) = v {
                    Some(s.clone())
                } else {
                    None
                }
            })
            .collect();
    }
    default
        .as_ref()
        .and_then(|d| d.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default()
}

// ─── Color conversion helpers ───────────────────────────────────────────────

pub fn hex_to_rgba(hex: &str) -> [u8; 4] {
    let hex = hex.trim_start_matches('#');
    let r = u8::from_str_radix(hex.get(0..2).unwrap_or("80"), 16).unwrap_or(128);
    let g = u8::from_str_radix(hex.get(2..4).unwrap_or("80"), 16).unwrap_or(128);
    let b = u8::from_str_radix(hex.get(4..6).unwrap_or("80"), 16).unwrap_or(128);
    let a = if hex.len() > 6 {
        u8::from_str_radix(hex.get(6..8).unwrap_or("FF"), 16).unwrap_or(255)
    } else {
        255
    };
    [r, g, b, a]
}

pub fn rgba_to_hex(rgba: &[u8; 4]) -> String {
    if rgba[3] == 255 {
        format!("#{:02X}{:02X}{:02X}", rgba[0], rgba[1], rgba[2])
    } else {
        format!(
            "#{:02X}{:02X}{:02X}{:02X}",
            rgba[0], rgba[1], rgba[2], rgba[3]
        )
    }
}

/// Generate a pseudo-random seed from system time.
pub fn random_seed() -> u64 {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos() as u64;
    // Mix with a larger time component for better spread
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    (nanos ^ millis.wrapping_mul(6364136223846793005)) % 1_000_000
}

/// Format a TOML value compactly for display in the object array UI.
pub fn format_toml_value_compact(v: &toml::Value) -> String {
    match v {
        toml::Value::String(s) => s.clone(),
        toml::Value::Integer(i) => i.to_string(),
        toml::Value::Float(f) => {
            // Trim trailing zeros but keep at least one decimal
            let s = format!("{:.4}", f);
            let s = s.trim_end_matches('0');
            let s = s.trim_end_matches('.');
            s.to_string()
        }
        toml::Value::Boolean(b) => b.to_string(),
        toml::Value::Array(arr) => {
            let items: Vec<String> = arr.iter().map(format_toml_value_compact).collect();
            format!("[{}]", items.join(", "))
        }
        toml::Value::Table(t) => {
            let items: Vec<String> = t
                .iter()
                .map(|(k, v)| format!("{}: {}", k, format_toml_value_compact(v)))
                .collect();
            format!("{{{}}}", items.join(", "))
        }
        toml::Value::Datetime(dt) => dt.to_string(),
    }
}

/// Human-friendly count with commas.
pub fn format_count(n: u64) -> String {
    if n < 1000 {
        return n.to_string();
    }
    let s = n.to_string();
    let mut result = String::new();
    for (i, c) in s.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            result.push(',');
        }
        result.push(c);
    }
    result.chars().rev().collect()
}
