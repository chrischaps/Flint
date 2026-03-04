//! Post-generation validation for procedural output.
//!
//! Checks meshes and images against budget constraints, PBR ranges, and
//! style guide parameters. The [`validate_output`] entry point dispatches
//! to per-variant checks and returns a [`ValidationReport`].

use std::fmt;

use crate::output::GeneratorOutput;
use crate::spec::ProcGenSpec;
use crate::types::{ImageData, MeshData, MaterialData, Vertex};

// ── Status / Check / Constraints ────────────────────────────────────────────

/// Outcome of a single validation check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckStatus {
    Pass,
    Warn,
    Fail,
}

impl fmt::Display for CheckStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Pass => write!(f, "PASS"),
            Self::Warn => write!(f, "WARN"),
            Self::Fail => write!(f, "FAIL"),
        }
    }
}

/// A single validation check result.
#[derive(Debug, Clone)]
pub struct ValidationCheck {
    pub name: String,
    pub status: CheckStatus,
    pub detail: String,
}

/// Plain-data struct carrying optional budget/style constraints.
///
/// Built in the CLI from spec LOD targets and style guide fields, so that
/// `flint-procgen` never depends on `flint-asset-gen`.
#[derive(Debug, Clone, Default)]
pub struct ValidationConstraints {
    pub max_triangles: Option<u32>,
    pub roughness_range: Option<[f64; 2]>,
    pub metallic_range: Option<[f64; 2]>,
    pub expected_texture_width: Option<u32>,
    pub expected_texture_height: Option<u32>,
    pub palette: Vec<String>,
}

/// Aggregated validation results.
#[derive(Debug, Clone)]
pub struct ValidationReport {
    pub checks: Vec<ValidationCheck>,
}

impl ValidationReport {
    pub fn count_passed(&self) -> usize {
        self.checks.iter().filter(|c| c.status == CheckStatus::Pass).count()
    }

    pub fn count_warnings(&self) -> usize {
        self.checks.iter().filter(|c| c.status == CheckStatus::Warn).count()
    }

    pub fn count_failed(&self) -> usize {
        self.checks.iter().filter(|c| c.status == CheckStatus::Fail).count()
    }

    pub fn has_failures(&self) -> bool {
        self.checks.iter().any(|c| c.status == CheckStatus::Fail)
    }

    pub fn has_warnings(&self) -> bool {
        self.checks.iter().any(|c| c.status == CheckStatus::Warn)
    }

    /// Print each check with a status indicator.
    pub fn print_checks(&self) {
        for c in &self.checks {
            let icon = match c.status {
                CheckStatus::Pass => " PASS",
                CheckStatus::Warn => " WARN",
                CheckStatus::Fail => " FAIL",
            };
            println!("  [{icon}] {}: {}", c.name, c.detail);
        }
    }

    /// Print a one-line summary.
    pub fn print_summary(&self) {
        println!(
            "Validation: {} passed, {} warnings, {} failed",
            self.count_passed(),
            self.count_warnings(),
            self.count_failed(),
        );
    }
}

// ── Entry point ─────────────────────────────────────────────────────────────

/// Validate a generator output against spec and constraints.
pub fn validate_output(
    output: &GeneratorOutput,
    spec: &ProcGenSpec,
    constraints: &ValidationConstraints,
) -> ValidationReport {
    let mut checks = Vec::new();

    match output {
        GeneratorOutput::Mesh(mesh) => {
            validate_mesh(mesh, constraints, &mut checks, None);
        }
        GeneratorOutput::MeshWithLods(lods) => {
            let lod_spec = spec.lod.as_deref().unwrap_or(&[]);
            for (i, mesh) in lods.iter().enumerate() {
                let per_lod_target = lod_spec
                    .iter()
                    .find(|l| l.level as usize == i)
                    .map(|l| l.target_triangles);
                let prefix = Some(format!("LOD{i}"));
                validate_mesh(mesh, constraints, &mut checks, prefix.as_deref());
                // Per-LOD triangle budget from spec
                if let Some(target) = per_lod_target {
                    check_triangle_budget_value(
                        mesh.triangle_count(),
                        target,
                        &mut checks,
                        &format!("LOD{i} spec target"),
                    );
                }
            }
        }
        GeneratorOutput::SkinnedMesh(skinned) => {
            // Extract positions/UVs from SkinnedVertex for shared checks
            let vertices: Vec<Vertex> = skinned
                .vertices
                .iter()
                .map(|sv| Vertex {
                    position: sv.position,
                    normal: sv.normal,
                    tangent: sv.tangent,
                    uv: sv.uv,
                })
                .collect();
            let proxy = MeshData {
                vertices,
                indices: skinned.indices.clone(),
                materials: skinned.materials.clone(),
                submeshes: skinned.submeshes.clone(),
                bounding_box: skinned.bounding_box,
            };
            validate_mesh(&proxy, constraints, &mut checks, Some("skinned"));
        }
        GeneratorOutput::Image(img) => {
            validate_image(img, constraints, &mut checks, None);
        }
        GeneratorOutput::ImageSet(images) => {
            for img in images {
                let prefix = Some(format!("{:?}", img.channel_semantics).to_lowercase());
                validate_image(img, constraints, &mut checks, prefix.as_deref());
            }
        }
        GeneratorOutput::Sound(_) => {
            checks.push(ValidationCheck {
                name: "sound output".into(),
                status: CheckStatus::Pass,
                detail: "no mesh/image checks applicable".into(),
            });
        }
    }

    ValidationReport { checks }
}

// ── Mesh checks ─────────────────────────────────────────────────────────────

fn validate_mesh(
    mesh: &MeshData,
    constraints: &ValidationConstraints,
    checks: &mut Vec<ValidationCheck>,
    prefix: Option<&str>,
) {
    let label = |name: &str| match prefix {
        Some(p) => format!("{p} {name}"),
        None => name.to_string(),
    };

    // 1. Triangle budget
    if let Some(max_tris) = constraints.max_triangles {
        check_triangle_budget_value(mesh.triangle_count(), max_tris, checks, &label("triangle budget"));
    } else {
        checks.push(ValidationCheck {
            name: label("triangle budget"),
            status: CheckStatus::Pass,
            detail: format!("{} triangles (no budget set)", mesh.triangle_count()),
        });
    }

    // 2. UV range [0,1]
    check_uv_range(&mesh.vertices, checks, &label("UV range"));

    // 3 & 4. PBR roughness & metallic
    check_pbr_roughness(&mesh.materials, constraints, checks, &label("PBR roughness"));
    check_pbr_metallic(&mesh.materials, constraints, checks, &label("PBR metallic"));

    // 5. Degenerate triangles
    check_degenerate_triangles(&mesh.vertices, &mesh.indices, checks, &label("degenerate triangles"));
}

fn check_triangle_budget_value(
    actual: usize,
    budget: u32,
    checks: &mut Vec<ValidationCheck>,
    name: &str,
) {
    let budget = budget as usize;
    if actual <= budget {
        checks.push(ValidationCheck {
            name: name.into(),
            status: CheckStatus::Pass,
            detail: format!("{actual} triangles <= {budget} budget"),
        });
    } else if actual <= budget * 2 {
        checks.push(ValidationCheck {
            name: name.into(),
            status: CheckStatus::Warn,
            detail: format!("{actual} triangles > {budget} budget (within 2x tolerance)"),
        });
    } else {
        checks.push(ValidationCheck {
            name: name.into(),
            status: CheckStatus::Fail,
            detail: format!("{actual} triangles > {budget} budget (exceeds 2x)"),
        });
    }
}

fn check_uv_range(
    vertices: &[Vertex],
    checks: &mut Vec<ValidationCheck>,
    name: &str,
) {
    let mut bad = 0usize;
    for v in vertices {
        if v.uv[0] < 0.0 || v.uv[0] > 1.0 || v.uv[1] < 0.0 || v.uv[1] > 1.0 {
            bad += 1;
        }
    }
    if bad == 0 {
        checks.push(ValidationCheck {
            name: name.into(),
            status: CheckStatus::Pass,
            detail: format!("all {} vertices in [0,1]", vertices.len()),
        });
    } else {
        checks.push(ValidationCheck {
            name: name.into(),
            status: CheckStatus::Fail,
            detail: format!("{bad}/{} vertices outside [0,1]", vertices.len()),
        });
    }
}

fn check_pbr_roughness(
    materials: &[MaterialData],
    constraints: &ValidationConstraints,
    checks: &mut Vec<ValidationCheck>,
    name: &str,
) {
    let mut fail = false;
    let mut warn = false;
    let mut details = Vec::new();

    for mat in materials {
        let r = mat.roughness as f64;
        if r < 0.0 || r > 1.0 {
            fail = true;
            details.push(format!("'{}' roughness {:.2} outside [0,1]", mat.name, r));
        } else if let Some([lo, hi]) = constraints.roughness_range {
            if r < lo || r > hi {
                warn = true;
                details.push(format!(
                    "'{}' roughness {:.2} outside style range [{:.2},{:.2}]",
                    mat.name, r, lo, hi
                ));
            }
        }
    }

    if fail {
        checks.push(ValidationCheck {
            name: name.into(),
            status: CheckStatus::Fail,
            detail: details.join("; "),
        });
    } else if warn {
        checks.push(ValidationCheck {
            name: name.into(),
            status: CheckStatus::Warn,
            detail: details.join("; "),
        });
    } else {
        checks.push(ValidationCheck {
            name: name.into(),
            status: CheckStatus::Pass,
            detail: format!("all {} materials in range", materials.len()),
        });
    }
}

fn check_pbr_metallic(
    materials: &[MaterialData],
    constraints: &ValidationConstraints,
    checks: &mut Vec<ValidationCheck>,
    name: &str,
) {
    let mut fail = false;
    let mut warn = false;
    let mut details = Vec::new();

    for mat in materials {
        let m = mat.metallic as f64;
        if m < 0.0 || m > 1.0 {
            fail = true;
            details.push(format!("'{}' metallic {:.2} outside [0,1]", mat.name, m));
        } else if let Some([lo, hi]) = constraints.metallic_range {
            if m < lo || m > hi {
                warn = true;
                details.push(format!(
                    "'{}' metallic {:.2} outside style range [{:.2},{:.2}]",
                    mat.name, m, lo, hi
                ));
            }
        }
    }

    if fail {
        checks.push(ValidationCheck {
            name: name.into(),
            status: CheckStatus::Fail,
            detail: details.join("; "),
        });
    } else if warn {
        checks.push(ValidationCheck {
            name: name.into(),
            status: CheckStatus::Warn,
            detail: details.join("; "),
        });
    } else {
        checks.push(ValidationCheck {
            name: name.into(),
            status: CheckStatus::Pass,
            detail: format!("all {} materials in range", materials.len()),
        });
    }
}

fn check_degenerate_triangles(
    vertices: &[Vertex],
    indices: &[u32],
    checks: &mut Vec<ValidationCheck>,
    name: &str,
) {
    let tri_count = indices.len() / 3;
    let mut degenerate = 0usize;

    for i in 0..tri_count {
        let i0 = indices[i * 3] as usize;
        let i1 = indices[i * 3 + 1] as usize;
        let i2 = indices[i * 3 + 2] as usize;

        if i0 >= vertices.len() || i1 >= vertices.len() || i2 >= vertices.len() {
            continue; // out-of-range indices are a separate concern
        }

        let p0 = vertices[i0].position;
        let p1 = vertices[i1].position;
        let p2 = vertices[i2].position;

        // Edge vectors
        let e1 = [p1[0] - p0[0], p1[1] - p0[1], p1[2] - p0[2]];
        let e2 = [p2[0] - p0[0], p2[1] - p0[1], p2[2] - p0[2]];

        // Cross product
        let cx = e1[1] * e2[2] - e1[2] * e2[1];
        let cy = e1[2] * e2[0] - e1[0] * e2[2];
        let cz = e1[0] * e2[1] - e1[1] * e2[0];

        // Area = 0.5 * |cross|, but we compare area > 1e-10
        let area = 0.5 * (cx * cx + cy * cy + cz * cz).sqrt();
        if area <= 1e-10 {
            degenerate += 1;
        }
    }

    if degenerate == 0 {
        checks.push(ValidationCheck {
            name: name.into(),
            status: CheckStatus::Pass,
            detail: format!("all {tri_count} triangles have non-zero area"),
        });
    } else {
        checks.push(ValidationCheck {
            name: name.into(),
            status: CheckStatus::Fail,
            detail: format!("{degenerate}/{tri_count} degenerate triangles found"),
        });
    }
}

// ── Image checks ────────────────────────────────────────────────────────────

fn validate_image(
    img: &ImageData,
    constraints: &ValidationConstraints,
    checks: &mut Vec<ValidationCheck>,
    prefix: Option<&str>,
) {
    let label = |name: &str| match prefix {
        Some(p) => format!("{p} {name}"),
        None => name.to_string(),
    };

    // Texture dimensions — check expected size
    let mut dim_status = CheckStatus::Pass;
    let mut dim_detail = format!("{}x{}", img.width, img.height);

    if let Some(ew) = constraints.expected_texture_width {
        if img.width != ew {
            dim_status = CheckStatus::Fail;
            dim_detail = format!("width {} != expected {}", img.width, ew);
        }
    }
    if let Some(eh) = constraints.expected_texture_height {
        if img.height != eh {
            dim_status = if dim_status == CheckStatus::Fail {
                CheckStatus::Fail
            } else {
                CheckStatus::Fail
            };
            dim_detail = if dim_status == CheckStatus::Fail && constraints.expected_texture_width.is_some() {
                format!(
                    "{}x{} != expected {}x{}",
                    img.width,
                    img.height,
                    constraints.expected_texture_width.unwrap_or(img.width),
                    eh
                )
            } else {
                format!("height {} != expected {}", img.height, eh)
            };
        }
    }

    // Warn if not power-of-two (only if not already failing)
    if dim_status == CheckStatus::Pass {
        if !img.width.is_power_of_two() || !img.height.is_power_of_two() {
            dim_status = CheckStatus::Warn;
            dim_detail = format!(
                "{}x{} — not power-of-two",
                img.width, img.height
            );
        }
    }

    checks.push(ValidationCheck {
        name: label("texture dimensions"),
        status: dim_status,
        detail: dim_detail,
    });
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::*;

    fn make_triangle() -> MeshData {
        MeshData {
            vertices: vec![
                Vertex {
                    position: [0.0, 0.0, 0.0],
                    normal: [0.0, 0.0, 1.0],
                    tangent: [1.0, 0.0, 0.0, 1.0],
                    uv: [0.0, 0.0],
                },
                Vertex {
                    position: [1.0, 0.0, 0.0],
                    normal: [0.0, 0.0, 1.0],
                    tangent: [1.0, 0.0, 0.0, 1.0],
                    uv: [1.0, 0.0],
                },
                Vertex {
                    position: [0.0, 1.0, 0.0],
                    normal: [0.0, 0.0, 1.0],
                    tangent: [1.0, 0.0, 0.0, 1.0],
                    uv: [0.0, 1.0],
                },
            ],
            indices: vec![0, 1, 2],
            materials: vec![MaterialData::default()],
            submeshes: vec![],
            bounding_box: BoundingBox::from_positions(&[
                [0.0, 0.0, 0.0],
                [1.0, 0.0, 0.0],
                [0.0, 1.0, 0.0],
            ]),
        }
    }

    fn make_spec() -> ProcGenSpec {
        ProcGenSpec::parse(
            r#"
generator = "mock"
[meta]
name = "test"
version = "1.0.0"
[seed]
mode = "fixed"
value = 42
"#,
        )
        .unwrap()
    }

    // 1. Valid mesh → all checks pass
    #[test]
    fn valid_mesh_all_pass() {
        let mesh = make_triangle();
        let output = GeneratorOutput::Mesh(mesh);
        let spec = make_spec();
        let constraints = ValidationConstraints {
            max_triangles: Some(10),
            ..Default::default()
        };
        let report = validate_output(&output, &spec, &constraints);
        assert!(!report.has_failures());
        assert!(!report.has_warnings());
        assert!(report.count_passed() >= 4); // budget, uv, roughness, metallic, degenerate
    }

    // 2. UV outside [0,1] → Fail
    #[test]
    fn uv_outside_range_fails() {
        let mut mesh = make_triangle();
        mesh.vertices[0].uv = [1.5, 0.0];
        let output = GeneratorOutput::Mesh(mesh);
        let spec = make_spec();
        let constraints = ValidationConstraints::default();
        let report = validate_output(&output, &spec, &constraints);
        let uv_check = report.checks.iter().find(|c| c.name.contains("UV range")).unwrap();
        assert_eq!(uv_check.status, CheckStatus::Fail);
        assert!(uv_check.detail.contains("1/3"));
    }

    // 3. Triangle count over budget → Warn (under 2x), Fail (over 2x)
    #[test]
    fn triangle_over_budget_warn() {
        // Make 3 triangles, budget 2 → 3 > 2, 3 <= 4 → Warn
        let spec = make_spec();
        let mut mesh2 = make_triangle();
        // Add two more triangles
        mesh2.vertices.push(Vertex {
            position: [2.0, 0.0, 0.0],
            normal: [0.0, 0.0, 1.0],
            tangent: [1.0, 0.0, 0.0, 1.0],
            uv: [0.5, 0.5],
        });
        mesh2.vertices.push(Vertex {
            position: [2.0, 1.0, 0.0],
            normal: [0.0, 0.0, 1.0],
            tangent: [1.0, 0.0, 0.0, 1.0],
            uv: [0.5, 0.5],
        });
        mesh2.indices.extend_from_slice(&[0, 3, 4, 1, 3, 4]); // 3 triangles total
        let output2 = GeneratorOutput::Mesh(mesh2);
        let constraints = ValidationConstraints {
            max_triangles: Some(2),
            ..Default::default()
        };
        let report = validate_output(&output2, &spec, &constraints);
        let budget_check = report.checks.iter().find(|c| c.name.contains("triangle budget")).unwrap();
        assert_eq!(budget_check.status, CheckStatus::Warn);
    }

    #[test]
    fn triangle_over_budget_fail() {
        let mut mesh = make_triangle();
        // Make 5 triangles, budget 2 → 5 > 4 → Fail
        for _ in 0..4 {
            let base = mesh.vertices.len() as u32;
            mesh.vertices.push(Vertex {
                position: [0.0, 0.0, 0.0],
                normal: [0.0, 0.0, 1.0],
                tangent: [1.0, 0.0, 0.0, 1.0],
                uv: [0.5, 0.5],
            });
            mesh.vertices.push(Vertex {
                position: [1.0, 0.0, 0.0],
                normal: [0.0, 0.0, 1.0],
                tangent: [1.0, 0.0, 0.0, 1.0],
                uv: [0.5, 0.5],
            });
            mesh.vertices.push(Vertex {
                position: [0.0, 1.0, 0.0],
                normal: [0.0, 0.0, 1.0],
                tangent: [1.0, 0.0, 0.0, 1.0],
                uv: [0.5, 0.5],
            });
            mesh.indices.extend_from_slice(&[base, base + 1, base + 2]);
        }
        // Now 5 triangles total
        let output = GeneratorOutput::Mesh(mesh);
        let spec = make_spec();
        let constraints = ValidationConstraints {
            max_triangles: Some(2),
            ..Default::default()
        };
        let report = validate_output(&output, &spec, &constraints);
        let budget_check = report.checks.iter().find(|c| c.name.contains("triangle budget")).unwrap();
        assert_eq!(budget_check.status, CheckStatus::Fail);
    }

    // 4. Roughness > 1.0 → Fail (PBR range)
    #[test]
    fn roughness_out_of_pbr_range_fails() {
        let mut mesh = make_triangle();
        mesh.materials[0].roughness = 1.5;
        let output = GeneratorOutput::Mesh(mesh);
        let spec = make_spec();
        let constraints = ValidationConstraints::default();
        let report = validate_output(&output, &spec, &constraints);
        let check = report.checks.iter().find(|c| c.name.contains("roughness")).unwrap();
        assert_eq!(check.status, CheckStatus::Fail);
    }

    // 5. Roughness outside style range but in [0,1] → Warn
    #[test]
    fn roughness_outside_style_range_warns() {
        let mut mesh = make_triangle();
        mesh.materials[0].roughness = 0.3; // valid PBR, but outside style range
        let output = GeneratorOutput::Mesh(mesh);
        let spec = make_spec();
        let constraints = ValidationConstraints {
            roughness_range: Some([0.6, 0.95]),
            ..Default::default()
        };
        let report = validate_output(&output, &spec, &constraints);
        let check = report.checks.iter().find(|c| c.name.contains("roughness")).unwrap();
        assert_eq!(check.status, CheckStatus::Warn);
    }

    // 6. Degenerate triangle (coincident vertices) → Fail
    #[test]
    fn degenerate_triangle_fails() {
        let mut mesh = make_triangle();
        // Make all three vertices coincident
        mesh.vertices[1].position = [0.0, 0.0, 0.0];
        mesh.vertices[2].position = [0.0, 0.0, 0.0];
        let output = GeneratorOutput::Mesh(mesh);
        let spec = make_spec();
        let constraints = ValidationConstraints::default();
        let report = validate_output(&output, &spec, &constraints);
        let check = report.checks.iter().find(|c| c.name.contains("degenerate")).unwrap();
        assert_eq!(check.status, CheckStatus::Fail);
        assert!(check.detail.contains("1/1"));
    }

    // 7. Texture dimensions mismatch → Fail
    #[test]
    fn texture_dimensions_mismatch_fails() {
        let img = ImageData::new(256, 256, ChannelSemantics::Color);
        let output = GeneratorOutput::Image(img);
        let spec = make_spec();
        let constraints = ValidationConstraints {
            expected_texture_width: Some(512),
            expected_texture_height: Some(512),
            ..Default::default()
        };
        let report = validate_output(&output, &spec, &constraints);
        let check = report.checks.iter().find(|c| c.name.contains("texture")).unwrap();
        assert_eq!(check.status, CheckStatus::Fail);
    }

    // 8. Non-power-of-two texture → Warn
    #[test]
    fn non_power_of_two_texture_warns() {
        let img = ImageData::new(300, 300, ChannelSemantics::Color);
        let output = GeneratorOutput::Image(img);
        let spec = make_spec();
        let constraints = ValidationConstraints::default();
        let report = validate_output(&output, &spec, &constraints);
        let check = report.checks.iter().find(|c| c.name.contains("texture")).unwrap();
        assert_eq!(check.status, CheckStatus::Warn);
    }

    // 9. Valid texture → all pass
    #[test]
    fn valid_texture_all_pass() {
        let img = ImageData::new(256, 256, ChannelSemantics::Color);
        let output = GeneratorOutput::Image(img);
        let spec = make_spec();
        let constraints = ValidationConstraints::default();
        let report = validate_output(&output, &spec, &constraints);
        assert!(!report.has_failures());
        assert!(!report.has_warnings());
    }

    // 10. No constraints, valid mesh → all pass
    #[test]
    fn no_constraints_valid_mesh_all_pass() {
        let mesh = make_triangle();
        let output = GeneratorOutput::Mesh(mesh);
        let spec = make_spec();
        let constraints = ValidationConstraints::default();
        let report = validate_output(&output, &spec, &constraints);
        assert!(!report.has_failures());
        assert!(!report.has_warnings());
    }

    // 11. MeshWithLods → per-LOD checks
    #[test]
    fn mesh_with_lods_per_lod_checks() {
        let lod0 = make_triangle();
        let lod1 = make_triangle();
        let output = GeneratorOutput::MeshWithLods(vec![lod0, lod1]);
        let mut spec = make_spec();
        spec.lod = Some(vec![
            crate::LodLevel { level: 0, target_triangles: 10 },
            crate::LodLevel { level: 1, target_triangles: 1 },
        ]);
        let constraints = ValidationConstraints::default();
        let report = validate_output(&output, &spec, &constraints);
        assert!(!report.has_failures());
        // Should have checks prefixed with LOD0 and LOD1
        let lod0_checks: Vec<_> = report.checks.iter().filter(|c| c.name.starts_with("LOD0")).collect();
        let lod1_checks: Vec<_> = report.checks.iter().filter(|c| c.name.starts_with("LOD1")).collect();
        assert!(!lod0_checks.is_empty());
        assert!(!lod1_checks.is_empty());
    }

    // 12. ImageSet → each image checked
    #[test]
    fn image_set_each_checked() {
        let img1 = ImageData::new(256, 256, ChannelSemantics::Color);
        let img2 = ImageData::new(256, 256, ChannelSemantics::Normal);
        let output = GeneratorOutput::ImageSet(vec![img1, img2]);
        let spec = make_spec();
        let constraints = ValidationConstraints::default();
        let report = validate_output(&output, &spec, &constraints);
        assert_eq!(report.checks.len(), 2);
        assert!(report.checks.iter().any(|c| c.name.contains("color")));
        assert!(report.checks.iter().any(|c| c.name.contains("normal")));
    }

    // 13. Summary format
    #[test]
    fn summary_format() {
        let report = ValidationReport {
            checks: vec![
                ValidationCheck {
                    name: "a".into(),
                    status: CheckStatus::Pass,
                    detail: "ok".into(),
                },
                ValidationCheck {
                    name: "b".into(),
                    status: CheckStatus::Warn,
                    detail: "meh".into(),
                },
                ValidationCheck {
                    name: "c".into(),
                    status: CheckStatus::Fail,
                    detail: "bad".into(),
                },
            ],
        };
        assert_eq!(report.count_passed(), 1);
        assert_eq!(report.count_warnings(), 1);
        assert_eq!(report.count_failed(), 1);
        assert!(report.has_failures());
        assert!(report.has_warnings());
    }

    // Sound output → pass
    #[test]
    fn sound_output_passes() {
        let output = GeneratorOutput::Sound(vec![0u8; 16]);
        let spec = make_spec();
        let constraints = ValidationConstraints::default();
        let report = validate_output(&output, &spec, &constraints);
        assert!(!report.has_failures());
        assert_eq!(report.count_passed(), 1);
    }

    // Metallic outside PBR range → Fail
    #[test]
    fn metallic_out_of_pbr_range_fails() {
        let mut mesh = make_triangle();
        mesh.materials[0].metallic = -0.1;
        let output = GeneratorOutput::Mesh(mesh);
        let spec = make_spec();
        let constraints = ValidationConstraints::default();
        let report = validate_output(&output, &spec, &constraints);
        let check = report.checks.iter().find(|c| c.name.contains("metallic")).unwrap();
        assert_eq!(check.status, CheckStatus::Fail);
    }

    // Metallic outside style range → Warn
    #[test]
    fn metallic_outside_style_range_warns() {
        let mut mesh = make_triangle();
        mesh.materials[0].metallic = 0.5; // valid PBR, outside style
        let output = GeneratorOutput::Mesh(mesh);
        let spec = make_spec();
        let constraints = ValidationConstraints {
            metallic_range: Some([0.0, 0.15]),
            ..Default::default()
        };
        let report = validate_output(&output, &spec, &constraints);
        let check = report.checks.iter().find(|c| c.name.contains("metallic")).unwrap();
        assert_eq!(check.status, CheckStatus::Warn);
    }

    // CheckStatus Display
    #[test]
    fn check_status_display() {
        assert_eq!(format!("{}", CheckStatus::Pass), "PASS");
        assert_eq!(format!("{}", CheckStatus::Warn), "WARN");
        assert_eq!(format!("{}", CheckStatus::Fail), "FAIL");
    }
}
