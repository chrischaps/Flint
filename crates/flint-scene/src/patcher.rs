//! Structure-preserving TOML scene patcher
//!
//! Uses `toml_edit` to modify only the changed fields in a scene file,
//! preserving comments, formatting, and ordering of unchanged content.
//! This produces minimal diffs when saving from the viewer.

use std::collections::HashSet;
use std::fs;
use std::path::Path;

/// A parsed scene document that can be patched field-by-field
pub struct SceneDocument {
    doc: toml_edit::DocumentMut,
}

/// A dirty field identifier: (entity_name, component_name, field_name)
pub type DirtyField = (String, String, String);

impl SceneDocument {
    /// Parse a scene file into an editable document
    pub fn from_file<P: AsRef<Path>>(path: P) -> Result<Self, String> {
        let content = fs::read_to_string(path.as_ref())
            .map_err(|e| format!("Failed to read scene file: {}", e))?;
        Self::from_str(&content)
    }

    /// Parse a TOML string into an editable document
    pub fn from_str(content: &str) -> Result<Self, String> {
        let doc: toml_edit::DocumentMut = content
            .parse()
            .map_err(|e| format!("Failed to parse TOML: {}", e))?;
        Ok(Self { doc })
    }

    /// Patch a single field in the document.
    /// Navigates to `entities.<entity_name>.<component>.<field>` and sets the value.
    pub fn patch_field(
        &mut self,
        entity_name: &str,
        component: &str,
        field: &str,
        value: &toml::Value,
    ) -> Result<(), String> {
        // Ensure entities table exists
        let entities = self.doc.get_mut("entities")
            .and_then(|v| v.as_table_like_mut())
            .ok_or_else(|| "No [entities] table in scene file".to_string())?;

        // Ensure entity table exists
        let entity = entities.get_mut(entity_name)
            .and_then(|v| v.as_table_like_mut())
            .ok_or_else(|| format!("Entity '{}' not found in scene file", entity_name))?;

        // Ensure component table exists (create if needed)
        if entity.get(component).is_none() {
            entity.insert(component, toml_edit::Item::Table(toml_edit::Table::new()));
        }

        let comp = entity.get_mut(component)
            .and_then(|v| v.as_table_like_mut())
            .ok_or_else(|| format!("Component '{}' is not a table", component))?;

        // Set the field value
        let edit_value = toml_to_edit_value(value);
        comp.insert(field, toml_edit::Item::Value(edit_value));

        Ok(())
    }

    /// Patch multiple dirty fields at once
    pub fn patch_fields(
        &mut self,
        dirty: &HashSet<DirtyField>,
        get_value: impl Fn(&str, &str, &str) -> Option<toml::Value>,
    ) -> Vec<String> {
        let mut errors = Vec::new();

        for (entity_name, component, field) in dirty {
            if let Some(value) = get_value(entity_name, component, field) {
                if let Err(e) = self.patch_field(entity_name, component, field, &value) {
                    errors.push(e);
                }
            }
        }

        errors
    }

    /// Serialize the document back to a string (preserving formatting)
    pub fn to_string(&self) -> String {
        self.doc.to_string()
    }

    /// Write the document to a file
    pub fn save<P: AsRef<Path>>(&self, path: P) -> Result<(), String> {
        fs::write(path, self.to_string())
            .map_err(|e| format!("Failed to write scene file: {}", e))
    }
}

/// Convert a `toml::Value` to a `toml_edit::Value`, using inline formatting
/// for arrays (Vec3, Color) to keep them compact on a single line.
fn toml_to_edit_value(value: &toml::Value) -> toml_edit::Value {
    match value {
        toml::Value::String(s) => toml_edit::Value::from(s.as_str()),
        toml::Value::Integer(i) => toml_edit::Value::from(*i),
        toml::Value::Float(f) => {
            // Ensure float renders with decimal point
            let formatted = format_float(*f);
            // Parse as a toml_edit value to get correct repr
            let s = format!("x = {}", formatted);
            if let Ok(doc) = s.parse::<toml_edit::DocumentMut>() {
                if let Some(v) = doc.get("x") {
                    return v.as_value().cloned().unwrap_or_else(|| toml_edit::Value::from(*f));
                }
            }
            toml_edit::Value::from(*f)
        }
        toml::Value::Boolean(b) => toml_edit::Value::from(*b),
        toml::Value::Array(arr) => {
            let mut edit_arr = toml_edit::Array::new();
            for item in arr {
                edit_arr.push(toml_to_edit_value(item));
            }
            // Force inline formatting for compact Vec3/Color arrays
            edit_arr.set_trailing("");
            edit_arr.set_trailing_comma(false);
            toml_edit::Value::Array(edit_arr)
        }
        toml::Value::Table(table) => {
            let mut edit_table = toml_edit::InlineTable::new();
            for (key, val) in table {
                edit_table.insert(key, toml_to_edit_value(val));
            }
            toml_edit::Value::InlineTable(edit_table)
        }
        toml::Value::Datetime(dt) => {
            // Convert datetime to string representation
            toml_edit::Value::from(dt.to_string())
        }
    }
}

/// Format a float value to always include a decimal point
fn format_float(f: f64) -> String {
    if f.fract() == 0.0 {
        format!("{:.1}", f)
    } else {
        // Trim trailing zeros but keep at least one decimal digit
        let s = format!("{}", f);
        if s.contains('.') {
            s
        } else {
            format!("{:.1}", f)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_patch_preserves_comments() {
        let original = r#"# Scene header comment
[scene]
name = "Test Scene"

# Player entity
[entities.player]
archetype = "player"

[entities.player.transform]
position = [0, 1, 0]
rotation = [0, 0, 0]
scale = [1, 1, 1]

# Room entity
[entities.room]
archetype = "room"

[entities.room.bounds]
min = [0, 0, 0]
max = [10, 4, 8]
"#;

        let mut doc = SceneDocument::from_str(original).unwrap();

        // Patch the player position
        let new_pos = toml::Value::Array(vec![
            toml::Value::Float(5.0),
            toml::Value::Float(2.0),
            toml::Value::Float(3.0),
        ]);
        doc.patch_field("player", "transform", "position", &new_pos).unwrap();

        let result = doc.to_string();

        // Comments should be preserved
        assert!(result.contains("# Scene header comment"));
        assert!(result.contains("# Player entity"));
        assert!(result.contains("# Room entity"));

        // The position should be updated
        assert!(result.contains("5.0"));
        assert!(result.contains("2.0"));
        assert!(result.contains("3.0"));

        // Room should be unchanged
        assert!(result.contains("[entities.room.bounds]"));
    }

    #[test]
    fn test_patch_single_field() {
        let original = r#"[scene]
name = "Test"

[entities.box]
archetype = "furniture"

[entities.box.transform]
position = [0, 0, 0]
rotation = [0, 0, 0]
scale = [1, 1, 1]

[entities.box.material]
color = [0.5, 0.5, 0.5, 1.0]
roughness = 0.8
"#;

        let mut doc = SceneDocument::from_str(original).unwrap();
        doc.patch_field("box", "material", "roughness", &toml::Value::Float(0.3)).unwrap();

        let result = doc.to_string();
        assert!(result.contains("0.3"));
        // Other fields should still be present
        assert!(result.contains("color = [0.5, 0.5, 0.5, 1.0]"));
    }
}
