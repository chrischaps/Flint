//! TOML loading for .ui.toml layout files and .style.toml style files

use super::element::{Anchor, ElementType, UiElement};
use super::style::StyleClass;
use super::tokens::{toml_to_style_value, TokenSet};
use std::collections::HashMap;
use std::path::Path;

/// Parse a .ui.toml layout file into elements + style path
pub fn load_layout(path: &Path) -> Result<(Vec<UiElement>, String), String> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| format!("Failed to read layout {}: {}", path.display(), e))?;

    let doc: toml::Value = content
        .parse()
        .map_err(|e| format!("Failed to parse layout {}: {}", path.display(), e))?;

    let table = doc
        .as_table()
        .ok_or_else(|| format!("Layout {} is not a TOML table", path.display()))?;

    // Read [ui] section for metadata
    let style_path = table
        .get("ui")
        .and_then(|ui| ui.get("style"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    // Read [elements.*] sections
    let elements_table = match table.get("elements") {
        Some(toml::Value::Table(t)) => t,
        _ => return Ok((Vec::new(), style_path)),
    };

    let mut elements = Vec::new();

    for (id, value) in elements_table {
        let elem_table = match value.as_table() {
            Some(t) => t,
            None => continue,
        };

        let type_str = elem_table
            .get("type")
            .and_then(|v| v.as_str())
            .unwrap_or("panel");

        let element_type = ElementType::from_str(type_str).unwrap_or(ElementType::Panel);

        let mut elem = UiElement::new(id.clone(), element_type);

        if let Some(anchor_str) = elem_table.get("anchor").and_then(|v| v.as_str()) {
            elem.anchor = Anchor::from_str(anchor_str).unwrap_or_default();
        }

        if let Some(parent) = elem_table.get("parent").and_then(|v| v.as_str()) {
            elem.parent_id = Some(parent.to_string());
        }

        if let Some(class) = elem_table.get("class").and_then(|v| v.as_str()) {
            elem.class = class.to_string();
        }

        if let Some(text) = elem_table.get("text").and_then(|v| v.as_str()) {
            elem.text = text.to_string();
        }

        if let Some(src) = elem_table.get("src").and_then(|v| v.as_str()) {
            elem.src = src.to_string();
        }

        if let Some(visible) = elem_table.get("visible").and_then(|v| v.as_bool()) {
            elem.visible = visible;
        }

        elements.push(elem);
    }

    // Build parent-child relationships
    let ids: Vec<String> = elements.iter().map(|e| e.id.clone()).collect();
    let parents: Vec<Option<String>> = elements.iter().map(|e| e.parent_id.clone()).collect();

    for (i, parent_id) in parents.iter().enumerate() {
        if let Some(pid) = parent_id {
            // Find parent index and add child
            if let Some(parent_idx) = ids.iter().position(|id| id == pid) {
                let child_id = ids[i].clone();
                elements[parent_idx].children.push(child_id);
            }
        }
    }

    Ok((elements, style_path))
}

/// A parsed `.style.toml`: named classes plus the token tables that
/// were used to resolve `"$name"` references inside them.
pub struct StyleSheet {
    pub classes: HashMap<String, StyleClass>,
    pub tokens: TokenSet,
}

/// Parse a .style.toml file into named style classes.
///
/// `root_dir` is the project root, where a top-level `tokens = "ui/theme.toml"`
/// resolves. String property values starting with `$` are looked up in the
/// local `[tokens]` table and the shared file; unknown tokens warn once and
/// leave the property unset.
pub fn load_styles(path: &Path, root_dir: &Path) -> Result<StyleSheet, String> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| format!("Failed to read style {}: {}", path.display(), e))?;

    let doc: toml::Value = content
        .parse()
        .map_err(|e| format!("Failed to parse style {}: {}", path.display(), e))?;

    let table = doc
        .as_table()
        .ok_or_else(|| format!("Style {} is not a TOML table", path.display()))?;

    let style_dir = path.parent().unwrap_or(root_dir);
    let mut tokens = TokenSet::from_style_table(table, root_dir, style_dir);

    let styles_table = match table.get("styles") {
        Some(toml::Value::Table(t)) => t,
        _ => {
            return Ok(StyleSheet {
                classes: HashMap::new(),
                tokens,
            })
        }
    };

    let mut classes = HashMap::new();

    for (name, value) in styles_table {
        let props_table = match value.as_table() {
            Some(t) => t,
            None => continue,
        };

        let mut properties = HashMap::new();

        for (key, val) in props_table {
            let resolved = match val {
                toml::Value::String(s) if TokenSet::is_reference(s) => tokens.resolve_style(key, s),
                other => toml_to_style_value(other),
            };
            if let Some(sv) = resolved {
                properties.insert(key.clone(), sv);
            }
        }

        classes.insert(
            name.clone(),
            StyleClass {
                name: name.clone(),
                properties,
            },
        );
    }

    Ok(StyleSheet { classes, tokens })
}
