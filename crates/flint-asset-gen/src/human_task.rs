//! Human task file generation
//!
//! Creates `.task.toml` files with structured specs for human artists
//! to create assets that match the project's style guide.

use crate::provider::AssetKind;
use crate::style::StyleGuide;
use flint_core::Result;
use serde::{Deserialize, Serialize};
use std::path::Path;

/// A task for a human artist
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HumanTask {
    pub name: String,
    pub asset_type: String,
    pub description: String,
    #[serde(default)]
    pub style: Option<String>,
    #[serde(default)]
    pub reference_prompt: Option<String>,
    #[serde(default = "default_status")]
    pub status: String,
    pub created_at: String,
    #[serde(default)]
    pub constraints: TaskConstraints,
}

fn default_status() -> String {
    "open".to_string()
}

/// Constraints for the task (derived from style guide)
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TaskConstraints {
    #[serde(default)]
    pub max_triangles: Option<u32>,
    #[serde(default)]
    pub format: Option<String>,
    #[serde(default)]
    pub roughness_range: Option<[f64; 2]>,
    #[serde(default)]
    pub metallic_range: Option<[f64; 2]>,
    #[serde(default)]
    pub palette_colors: Vec<String>,
}

/// TOML wrapper for task files
#[derive(Debug, Serialize, Deserialize)]
struct TaskFile {
    task: HumanTask,
}

/// Generate a human task file for a missing asset
pub fn generate_task_file(
    name: &str,
    kind: AssetKind,
    description: &str,
    style: Option<&StyleGuide>,
    output_dir: &Path,
) -> Result<std::path::PathBuf> {
    std::fs::create_dir_all(output_dir)?;

    let mut constraints = TaskConstraints::default();
    let mut style_name = None;
    let mut reference_prompt = None;

    if let Some(s) = style {
        style_name = Some(s.name.clone());
        reference_prompt = Some(s.enrich_prompt(description));
        constraints.palette_colors = s.palette.clone();
        constraints.roughness_range = s.materials.roughness_range;
        constraints.metallic_range = s.materials.metallic_range;
        constraints.max_triangles = s.geometry.max_triangles;
    }

    if kind == AssetKind::Model {
        constraints.format = Some("glb".to_string());
    }

    let task = HumanTask {
        name: name.to_string(),
        asset_type: kind.to_string(),
        description: description.to_string(),
        style: style_name,
        reference_prompt,
        status: "open".to_string(),
        created_at: now_iso8601(),
        constraints,
    };

    let file = TaskFile { task };
    let content = toml::to_string_pretty(&file).map_err(|e| {
        flint_core::FlintError::GenerationError(format!("Failed to serialize task: {}", e))
    })?;

    let path = output_dir.join(format!("{}.task.toml", name));
    std::fs::write(&path, content)?;

    Ok(path)
}

fn now_iso8601() -> String {
    let dur = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = dur.as_secs();
    let days = secs / 86400;
    let time_secs = secs % 86400;
    let hours = time_secs / 3600;
    let mins = (time_secs % 3600) / 60;
    let s = time_secs % 60;

    let mut y = 1970i64;
    let mut remaining_days = days as i64;
    loop {
        let days_in_year = if y % 4 == 0 && (y % 100 != 0 || y % 400 == 0) {
            366
        } else {
            365
        };
        if remaining_days < days_in_year {
            break;
        }
        remaining_days -= days_in_year;
        y += 1;
    }
    let leap = y % 4 == 0 && (y % 100 != 0 || y % 400 == 0);
    let month_days = [
        31,
        if leap { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    let mut m = 0usize;
    for (i, &md) in month_days.iter().enumerate() {
        if remaining_days < md as i64 {
            m = i;
            break;
        }
        remaining_days -= md as i64;
    }

    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        y,
        m + 1,
        remaining_days + 1,
        hours,
        mins,
        s
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir() -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "flint_task_test_{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn test_generate_task_file_no_style() {
        let dir = temp_dir();
        let path = generate_task_file("tavern_chair", AssetKind::Model, "A sturdy wooden chair", None, &dir).unwrap();

        assert!(path.exists());
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("tavern_chair"));
        assert!(content.contains("model"));
        assert!(content.contains("A sturdy wooden chair"));
        assert!(content.contains("open"));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn test_generate_task_file_with_style() {
        let dir = temp_dir();
        let style = StyleGuide {
            name: "medieval".to_string(),
            description: None,
            prompt_prefix: Some("Medieval fantasy".to_string()),
            prompt_suffix: None,
            negative_prompt: None,
            palette: vec!["#8B4513".to_string(), "#A0522D".to_string()],
            materials: crate::style::MaterialConstraints {
                roughness_range: Some([0.6, 0.9]),
                metallic_range: Some([0.0, 0.1]),
                preferred_materials: vec![],
            },
            geometry: crate::style::GeometryConstraints {
                max_triangles: Some(5000),
                require_uvs: None,
                require_normals: None,
            },
        };

        let path = generate_task_file(
            "oak_table",
            AssetKind::Model,
            "A large oak table",
            Some(&style),
            &dir,
        )
        .unwrap();

        assert!(path.exists());
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("medieval"));
        assert!(content.contains("5000"));
        assert!(content.contains("8B4513"));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn test_task_file_roundtrip() {
        let dir = temp_dir();
        let path = generate_task_file(
            "test_asset",
            AssetKind::Texture,
            "A test texture",
            None,
            &dir,
        )
        .unwrap();

        let content = std::fs::read_to_string(&path).unwrap();
        let file: TaskFile = toml::from_str(&content).unwrap();
        assert_eq!(file.task.name, "test_asset");
        assert_eq!(file.task.asset_type, "texture");

        std::fs::remove_dir_all(&dir).ok();
    }
}
