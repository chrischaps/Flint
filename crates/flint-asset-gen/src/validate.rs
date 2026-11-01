//! Model validation against style constraints.
//!
//! A thin wrapper over the procgen validator: the model is read back with
//! [`flint_procgen::read_model`] and checked by
//! [`flint_procgen::validate_model`], the checks `flint gen --validate` runs
//! on the GLB it writes. One set of rules covers every model, generated,
//! imported or AI-made: mesh integrity, triangle budget, UVs, normals, PBR
//! ranges, palette and degenerate triangles.

use crate::style::StyleGuide;
use flint_core::{FlintError, Result};
use std::path::Path;

pub use flint_procgen::{CheckStatus, ValidationCheck};

/// Full validation report for a model file.
#[derive(Debug, Clone)]
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

    /// Print the model's header, each check as `flint gen --validate`
    /// prints it, and the result.
    pub fn print_summary(&self) {
        println!("Validation: {}", self.path);
        println!(
            "  Meshes: {}, Materials: {}, Triangles: {}",
            self.mesh_count, self.material_count, self.total_triangles
        );
        let report = flint_procgen::ValidationReport {
            checks: self.checks.clone(),
        };
        report.print_checks();
        println!("  {}", report.summary());
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

/// Validate a glTF/GLB model against optional style constraints.
pub fn validate_model(path: &Path, style: Option<&StyleGuide>) -> Result<ValidationReport> {
    let model = flint_procgen::read_model(path).map_err(|e| {
        FlintError::GenerationError(format!("Failed to import model for validation: {e}"))
    })?;
    let constraints = style
        .map(StyleGuide::validation_constraints)
        .unwrap_or_default();
    let report = flint_procgen::validate_model(&model, None, &constraints);

    Ok(ValidationReport {
        path: path.to_string_lossy().to_string(),
        passed: !report.has_failures(),
        checks: report.checks,
        mesh_count: model.meshes.len(),
        material_count: model.material_count,
        total_triangles: model.triangle_count() as u32,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::GenerationProvider;

    fn mock_model(name: &str) -> (std::path::PathBuf, std::path::PathBuf) {
        let dir =
            std::env::temp_dir().join(format!("flint_validate_{name}_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let provider = crate::providers::mock::MockProvider::new();
        let request = crate::provider::GenerateRequest {
            name: name.to_string(),
            description: "test cube".to_string(),
            kind: crate::provider::AssetKind::Model,
            texture_params: None,
            model_params: None,
            audio_params: None,
            tags: vec![],
        };
        let result = provider.generate(&request, None, &dir).unwrap();
        (dir, result.output_path.into())
    }

    fn strict_style() -> StyleGuide {
        StyleGuide {
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
        }
    }

    fn check<'a>(report: &'a ValidationReport, name: &str) -> &'a ValidationCheck {
        report
            .checks
            .iter()
            .find(|c| c.name == name)
            .unwrap_or_else(|| panic!("no '{name}' check in {:#?}", report.checks))
    }

    #[test]
    fn test_validate_mock_model() {
        let (dir, path) = mock_model("test_model");
        let report = validate_model(&path, None).unwrap();
        // The mock triangle has no normals or UVs: missing, so they warn.
        assert!(report.passed, "{:#?}", report.checks);
        assert_eq!(report.mesh_count, 1);
        assert_eq!(report.total_triangles, 1);
        assert_eq!(check(&report, "normals").status, CheckStatus::Warn);
        assert_eq!(check(&report, "UV range").status, CheckStatus::Warn);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn test_validate_with_style_constraints() {
        let (dir, path) = mock_model("test_model2");
        let report = validate_model(&path, Some(&strict_style())).unwrap();
        assert!(!report.passed);
        // Each requirement fails under its own name.
        assert_eq!(check(&report, "UV range").status, CheckStatus::Fail);
        assert_eq!(check(&report, "normals").status, CheckStatus::Fail);
        assert_eq!(
            check(&report, "style triangle budget").status,
            CheckStatus::Fail
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    /// `flint asset validate` reports the check names `flint gen
    /// --validate` does, because they are the same checks.
    #[test]
    fn names_match_the_procgen_validator() {
        let v = |x: f32, y: f32| flint_procgen::Vertex {
            position: [x, y, 0.0],
            normal: [0.0, 0.0, 1.0],
            tangent: [1.0, 0.0, 0.0, 1.0],
            uv: [x, y],
        };
        let mut mesh = flint_procgen::MeshData {
            vertices: vec![v(0.0, 0.0), v(1.0, 0.0), v(1.0, 1.0), v(0.0, 1.0)],
            indices: vec![0, 1, 2, 0, 2, 3],
            materials: vec![flint_procgen::MaterialData::default()],
            submeshes: vec![],
            bounding_box: flint_procgen::BoundingBox::from_positions(&[]),
        };
        mesh.recompute_bounds();
        let glb = flint_procgen::mesh_to_glb(&mesh).unwrap();
        let dir =
            std::env::temp_dir().join(format!("flint_validate_names_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("cube.glb");
        std::fs::write(&path, glb).unwrap();

        let style = StyleGuide {
            palette: vec!["#808080".into()],
            ..strict_style()
        };
        let report = validate_model(&path, Some(&style)).unwrap();
        let spec = flint_procgen::ProcGenSpec::parse(
            "generator = \"mock\"\n[meta]\nname = \"quad\"\nversion = \"1.0.0\"\n\
             [seed]\nmode = \"fixed\"\nvalue = 0\n",
        )
        .unwrap();
        let in_memory = flint_procgen::validate_output(
            &flint_procgen::GeneratorOutput::Mesh(mesh),
            &spec,
            &style.validation_constraints(),
        );
        let names = |checks: &[ValidationCheck]| -> Vec<String> {
            checks.iter().map(|c| c.name.clone()).collect()
        };
        assert_eq!(names(&report.checks), names(&in_memory.checks));
        std::fs::remove_dir_all(&dir).ok();
    }
}
