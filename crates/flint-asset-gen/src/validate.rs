//! Model validation against style constraints
//!
//! Validates generated GLB models by importing them and checking against
//! style guide constraints (triangle count, UVs, normals, material ranges).

use crate::style::StyleGuide;
use flint_core::{FlintError, Result};
use flint_import::import_gltf;
use serde::{Deserialize, Serialize};
use std::path::Path;

/// A single validation check result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidationCheck {
    pub name: String,
    pub status: CheckStatus,
    pub detail: String,
}

/// Status of a validation check
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CheckStatus {
    Pass,
    Warn,
    Fail,
}

/// Full validation report for a model
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidationReport {
    pub path: String,
    pub checks: Vec<ValidationCheck>,
    pub mesh_count: usize,
    pub material_count: usize,
    pub total_triangles: u32,
    pub passed: bool,
}

impl ValidationReport {
    /// Count checks by status
    pub fn count_by_status(&self, status: CheckStatus) -> usize {
        self.checks.iter().filter(|c| c.status == status).count()
    }

    /// Print a formatted summary
    pub fn print_summary(&self) {
        println!("Validation: {}", self.path);
        println!(
            "  Meshes: {}, Materials: {}, Triangles: {}",
            self.mesh_count, self.material_count, self.total_triangles
        );
        for check in &self.checks {
            let icon = match check.status {
                CheckStatus::Pass => "OK",
                CheckStatus::Warn => "WARN",
                CheckStatus::Fail => "FAIL",
            };
            println!("  {}: {}  {}", check.name, check.detail, icon);
        }
        if self.passed {
            println!("  Result: PASSED");
        } else {
            println!(
                "  Result: FAILED ({} issues)",
                self.count_by_status(CheckStatus::Fail)
            );
        }
    }
}

/// Validate a GLB model against optional style constraints
pub fn validate_model(path: &Path, style: Option<&StyleGuide>) -> Result<ValidationReport> {
    let path_str = path.to_string_lossy().to_string();

    // Import the model
    let result = import_gltf(&path_str).map_err(|e| {
        FlintError::GenerationError(format!("Failed to import model for validation: {}", e))
    })?;

    let mut checks = Vec::new();
    let mut total_triangles: u32 = 0;

    // Check each mesh
    for mesh in &result.meshes {
        let tri_count = (mesh.indices.len() / 3) as u32;
        total_triangles += tri_count;

        // UV check
        if mesh.uvs.is_empty() {
            let should_fail = style
                .and_then(|s| s.geometry.require_uvs)
                .unwrap_or(false);
            checks.push(ValidationCheck {
                name: format!("{}: UVs", mesh.name),
                status: if should_fail {
                    CheckStatus::Fail
                } else {
                    CheckStatus::Warn
                },
                detail: "missing".to_string(),
            });
        } else {
            checks.push(ValidationCheck {
                name: format!("{}: UVs", mesh.name),
                status: CheckStatus::Pass,
                detail: "present".to_string(),
            });
        }

        // Normal check
        if mesh.normals.is_empty() {
            let should_fail = style
                .and_then(|s| s.geometry.require_normals)
                .unwrap_or(false);
            checks.push(ValidationCheck {
                name: format!("{}: Normals", mesh.name),
                status: if should_fail {
                    CheckStatus::Fail
                } else {
                    CheckStatus::Warn
                },
                detail: "missing".to_string(),
            });
        } else {
            checks.push(ValidationCheck {
                name: format!("{}: Normals", mesh.name),
                status: CheckStatus::Pass,
                detail: "present".to_string(),
            });
        }
    }

    // Total triangle count check
    if let Some(max_tris) = style.and_then(|s| s.geometry.max_triangles) {
        if total_triangles > max_tris {
            checks.push(ValidationCheck {
                name: "Triangles".to_string(),
                status: CheckStatus::Fail,
                detail: format!("{} / {} max", total_triangles, max_tris),
            });
        } else {
            checks.push(ValidationCheck {
                name: "Triangles".to_string(),
                status: CheckStatus::Pass,
                detail: format!("{} / {} max", total_triangles, max_tris),
            });
        }
    } else {
        checks.push(ValidationCheck {
            name: "Triangles".to_string(),
            status: CheckStatus::Pass,
            detail: format!("{}", total_triangles),
        });
    }

    // Material checks
    for mat in &result.materials {
        if let Some(roughness_range) = style.and_then(|s| s.materials.roughness_range) {
            let in_range = mat.roughness >= roughness_range[0] as f32
                && mat.roughness <= roughness_range[1] as f32;
            checks.push(ValidationCheck {
                name: format!("{}: roughness", mat.name),
                status: if in_range {
                    CheckStatus::Pass
                } else {
                    CheckStatus::Warn
                },
                detail: format!(
                    "{:.2} (range: {:.1}-{:.1})",
                    mat.roughness, roughness_range[0], roughness_range[1]
                ),
            });
        }

        if let Some(metallic_range) = style.and_then(|s| s.materials.metallic_range) {
            let in_range = mat.metallic >= metallic_range[0] as f32
                && mat.metallic <= metallic_range[1] as f32;
            checks.push(ValidationCheck {
                name: format!("{}: metallic", mat.name),
                status: if in_range {
                    CheckStatus::Pass
                } else {
                    CheckStatus::Warn
                },
                detail: format!(
                    "{:.2} (range: {:.1}-{:.1})",
                    mat.metallic, metallic_range[0], metallic_range[1]
                ),
            });
        }
    }

    let passed = !checks.iter().any(|c| c.status == CheckStatus::Fail);

    Ok(ValidationReport {
        path: path_str,
        checks,
        mesh_count: result.meshes.len(),
        material_count: result.materials.len(),
        total_triangles,
        passed,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::GenerationProvider;

    #[test]
    fn test_validate_mock_model() {
        // Generate a mock GLB first, then validate it
        let dir = std::env::temp_dir().join(format!(
            "flint_validate_test_{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&dir).unwrap();

        let provider = crate::providers::mock::MockProvider::new();
        let request = crate::provider::GenerateRequest {
            name: "test_model".to_string(),
            description: "test cube".to_string(),
            kind: crate::provider::AssetKind::Model,
            texture_params: None,
            model_params: None,
            audio_params: None,
            tags: vec![],
        };

        let result = provider
            .generate(&request, None, &dir)
            .unwrap();

        let report = validate_model(Path::new(&result.output_path), None).unwrap();
        assert!(report.passed);
        assert!(report.mesh_count > 0);
        assert!(report.total_triangles > 0);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn test_validate_with_style_constraints() {
        let dir = std::env::temp_dir().join(format!(
            "flint_validate_style_test_{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&dir).unwrap();

        let provider = crate::providers::mock::MockProvider::new();
        let request = crate::provider::GenerateRequest {
            name: "test_model2".to_string(),
            description: "test cube".to_string(),
            kind: crate::provider::AssetKind::Model,
            texture_params: None,
            model_params: None,
            audio_params: None,
            tags: vec![],
        };

        let result = provider
            .generate(&request, None, &dir)
            .unwrap();

        // Style with very low triangle limit to force a failure
        let strict_style = StyleGuide {
            name: "strict_test".to_string(),
            description: None,
            prompt_prefix: None,
            prompt_suffix: None,
            negative_prompt: None,
            palette: vec![],
            materials: crate::style::MaterialConstraints::default(),
            geometry: crate::style::GeometryConstraints {
                max_triangles: Some(0),
                require_uvs: Some(true),
                require_normals: Some(true),
            },
        };

        let report = validate_model(Path::new(&result.output_path), Some(&strict_style)).unwrap();
        // Should fail due to triangle count > 0 and missing UVs on mock model
        assert!(!report.passed);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn test_validation_report_counts() {
        let report = ValidationReport {
            path: "test.glb".to_string(),
            checks: vec![
                ValidationCheck {
                    name: "UVs".to_string(),
                    status: CheckStatus::Pass,
                    detail: "present".to_string(),
                },
                ValidationCheck {
                    name: "Triangles".to_string(),
                    status: CheckStatus::Fail,
                    detail: "10000 / 5000 max".to_string(),
                },
                ValidationCheck {
                    name: "Normals".to_string(),
                    status: CheckStatus::Warn,
                    detail: "missing".to_string(),
                },
            ],
            mesh_count: 1,
            material_count: 1,
            total_triangles: 10000,
            passed: false,
        };

        assert_eq!(report.count_by_status(CheckStatus::Pass), 1);
        assert_eq!(report.count_by_status(CheckStatus::Fail), 1);
        assert_eq!(report.count_by_status(CheckStatus::Warn), 1);
    }
}
