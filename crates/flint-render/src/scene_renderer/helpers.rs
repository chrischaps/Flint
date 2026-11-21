//! Free helper functions used by the scene renderer.

use crate::pipeline::BlendMode;
use flint_core::diagnostic::{codes, Diagnostic};
use flint_ecs::ComponentView;

pub(super) fn identity_matrix() -> [[f32; 4]; 4] {
    [
        [1.0, 0.0, 0.0, 0.0],
        [0.0, 1.0, 0.0, 0.0],
        [0.0, 0.0, 1.0, 0.0],
        [0.0, 0.0, 0.0, 1.0],
    ]
}

/// Compute the inverse-transpose of a 4x4 model matrix (for correct normal transformation).
/// Only the upper 3x3 matters for normals; we embed it in a 4x4 for GPU upload.
pub(super) fn mat4_inv_transpose(m: &[[f32; 4]; 4]) -> [[f32; 4]; 4] {
    // Extract upper-left 3x3
    let a = m[0][0];
    let b = m[1][0];
    let c = m[2][0];
    let d = m[0][1];
    let e = m[1][1];
    let f = m[2][1];
    let g = m[0][2];
    let h = m[1][2];
    let i = m[2][2];

    let det = a * (e * i - f * h) - b * (d * i - f * g) + c * (d * h - e * g);

    if det.abs() < 1e-10 {
        return identity_matrix();
    }

    let inv_det = 1.0 / det;

    // Cofactor matrix: cof(i,j) / det gives the inverse-transpose entries.
    // In column-major storage m[col][row], column c needs [cof(0,c), cof(1,c), cof(2,c)] / det.
    //
    // Row 0 cofactors:
    let cof00 = (e * i - f * h) * inv_det;
    let cof01 = (f * g - d * i) * inv_det;
    let cof02 = (d * h - e * g) * inv_det;
    // Row 1 cofactors:
    let cof10 = (c * h - b * i) * inv_det;
    let cof11 = (a * i - c * g) * inv_det;
    let cof12 = (b * g - a * h) * inv_det;
    // Row 2 cofactors:
    let cof20 = (b * f - c * e) * inv_det;
    let cof21 = (c * d - a * f) * inv_det;
    let cof22 = (a * e - b * d) * inv_det;

    // Column-major: column j = [cof(0,j), cof(1,j), cof(2,j)]
    [
        [cof00, cof10, cof20, 0.0],
        [cof01, cof11, cof21, 0.0],
        [cof02, cof12, cof22, 0.0],
        [0.0, 0.0, 0.0, 1.0],
    ]
}

/// Extract both the size and center offset from a `bounds` component, read
/// through the typed view (ADR 0115: a corner that is no vector is reported
/// once). `None` without both corners.
pub(super) fn extract_bounds_info(
    bounds: &flint_ecs::ComponentView<'_>,
) -> Option<([f32; 3], [f32; 3])> {
    let min_arr = bounds.opt_vec3("min")?;
    let max_arr = bounds.opt_vec3("max")?;

    let size = [
        max_arr[0] - min_arr[0],
        max_arr[1] - min_arr[1],
        max_arr[2] - min_arr[2],
    ];

    let center = [
        (min_arr[0] + max_arr[0]) / 2.0,
        (min_arr[1] + max_arr[1]) / 2.0,
        (min_arr[2] + max_arr[2]) / 2.0,
    ];

    Some((size, center))
}

/// `material.blend_mode`'s values, the schema's list too (a test holds the
/// two together). Read with `ComponentView::choice`, so an unknown one is
/// reported and reads as `alpha`.
pub(super) const BLEND_MODES: &[&str] = &["alpha", "additive", "multiply"];

/// Parse a blend mode string from TOML into the BlendMode enum
pub(super) fn parse_blend_mode(s: &str) -> BlendMode {
    match s {
        "additive" => BlendMode::Additive,
        "multiply" => BlendMode::Multiply,
        _ => BlendMode::Alpha,
    }
}

/// What `material_overrides.<name>` says about one glTF material (#624),
/// gathered field by field from the drawn entity up through its ancestors.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) struct MaterialOverride {
    /// Replaces the material's base colour (and `material.base_color_*`).
    pub base_color: Option<[f32; 4]>,
    /// Replaces the material's `emissive` colour, linear RGB.
    pub emissive: Option<[f32; 3]>,
    /// Replaces the material's `emissive_strength`.
    pub emissive_strength: Option<f32>,
}

impl MaterialOverride {
    pub(super) fn is_full(&self) -> bool {
        self.base_color.is_some() && self.emissive.is_some() && self.emissive_strength.is_some()
    }

    /// Fills the fields still unset from one entity's
    /// `material_overrides.<material>`, read through the typed view
    /// (ADR 0115, #1068): an array is a base colour, a table may hold
    /// `base_color`, `emissive` and `emissive_strength`. A value of the
    /// wrong shape is skipped, as before, and now reported once by entity,
    /// component and field (`material_overrides.Glass.emissive`). A field
    /// the nearer entity already set is not read here, so not reported.
    pub(super) fn fill_from<'a>(
        &mut self,
        overrides: &ComponentView<'a>,
        material: &'a str,
        gltf_alpha: f32,
    ) {
        let Some(value) = overrides.raw(material) else {
            return;
        };
        match value {
            toml::Value::Array(_) => {
                if self.base_color.is_none() {
                    self.base_color = overrides
                        .opt_color(material)
                        .map(|c| keep_alpha(c, value, gltf_alpha));
                }
            }
            toml::Value::Table(_) => {
                let entry = overrides.table(material);
                if self.base_color.is_none() {
                    self.base_color = entry.opt_color(OVERRIDE_BASE_COLOR).map(|c| {
                        let written = entry.raw(OVERRIDE_BASE_COLOR).expect("read just above");
                        keep_alpha(c, written, gltf_alpha)
                    });
                }
                if self.emissive.is_none() {
                    self.emissive = entry
                        .opt_color(OVERRIDE_EMISSIVE)
                        .map(|[r, g, b, _]| [r.max(0.0), g.max(0.0), b.max(0.0)]);
                }
                if self.emissive_strength.is_none() {
                    self.emissive_strength = entry
                        .opt_f32(OVERRIDE_EMISSIVE_STRENGTH)
                        .map(|s| s.max(0.0));
                }
            }
            other => overrides.report(material, || {
                Diagnostic::error(
                    codes::BAD_TYPE,
                    format!(
                        "expected a colour, [r, g, b] or [r, g, b, a], or a table of \
                         {OVERRIDE_BASE_COLOR}, {OVERRIDE_EMISSIVE} and \
                         {OVERRIDE_EMISSIVE_STRENGTH}, got {} {other}",
                        other.type_str()
                    ),
                )
                .hint("the material keeps its own colour and light")
            }),
        }
    }
}

/// The keys of a `material_overrides` table entry (#624).
const OVERRIDE_BASE_COLOR: &str = "base_color";
const OVERRIDE_EMISSIVE: &str = "emissive";
const OVERRIDE_EMISSIVE_STRENGTH: &str = "emissive_strength";

/// A three-number colour keeps the glTF material's alpha, so a tint on a
/// blended glass material stays translucent; a fourth number is its own.
fn keep_alpha(colour: [f32; 4], written: &toml::Value, gltf_alpha: f32) -> [f32; 4] {
    let three = written.as_array().is_some_and(|a| a.len() < 4);
    if three {
        [colour[0], colour[1], colour[2], gltf_alpha]
    } else {
        colour
    }
}

#[cfg(test)]
mod material_override_tests {
    use super::MaterialOverride;
    use flint_core::components as comp;
    use flint_ecs::FlintWorld;

    /// Fill `o` from an entity `lamp` whose `material_overrides.Glass` is
    /// `value`, then read it `times - 1` more times into fresh overrides
    /// (as the frames after would); what the view reported, as lines.
    fn fill(o: &mut MaterialOverride, value: &str, gltf_alpha: f32, times: usize) -> Vec<String> {
        let mut world = FlintWorld::new();
        let id = world.spawn("lamp").unwrap();
        let data: toml::Value = toml::from_str(&format!("Glass = {value}")).unwrap();
        world
            .set_component(id, comp::MATERIAL_OVERRIDES, data)
            .unwrap();
        let view = world.view(id, comp::MATERIAL_OVERRIDES);
        o.fill_from(&view, "Glass", gltf_alpha);
        for _ in 1..times {
            MaterialOverride::default().fill_from(&view, "Glass", gltf_alpha);
        }
        world
            .read_reports()
            .snapshot()
            .iter()
            .map(|d| d.to_string())
            .collect()
    }

    #[test]
    fn array_is_a_base_colour_and_keeps_the_gltf_alpha() {
        let mut o = MaterialOverride::default();
        assert!(fill(&mut o, "[0.2, 0.4, 0.6]", 0.5, 1).is_empty());
        assert_eq!(o.base_color, Some([0.2, 0.4, 0.6, 0.5]));
        assert_eq!(o.emissive, None);
        let mut o = MaterialOverride::default();
        fill(&mut o, "[0.2, 0.4, 0.6, 0.9]", 0.5, 1);
        assert_eq!(o.base_color, Some([0.2, 0.4, 0.6, 0.9]));
        let mut o = MaterialOverride::default();
        fill(&mut o, "{ base_color = [1, 1, 0] }", 0.25, 1);
        assert_eq!(o.base_color, Some([1.0, 1.0, 0.0, 0.25]));
    }

    #[test]
    fn table_sets_each_field_and_the_nearest_wins() {
        let mut o = MaterialOverride::default();
        // The child's value is read first: it lights the material.
        fill(
            &mut o,
            "{ emissive = [0, 1, 0.8], emissive_strength = 4 }",
            1.0,
            1,
        );
        // The root paints it and tries to light it red; only the paint lands.
        fill(
            &mut o,
            "{ base_color = [1, 0, 0, 1], emissive = [1, 0, 0] }",
            1.0,
            1,
        );
        assert_eq!(o.base_color, Some([1.0, 0.0, 0.0, 1.0]));
        assert_eq!(o.emissive, Some([0.0, 1.0, 0.8]));
        assert_eq!(o.emissive_strength, Some(4.0));
        assert!(o.is_full());
    }

    #[test]
    fn negative_light_counts_as_none() {
        let mut o = MaterialOverride::default();
        fill(
            &mut o,
            "{ emissive = [-1, 2, 0], emissive_strength = -3 }",
            1.0,
            1,
        );
        assert_eq!(o.emissive, Some([0.0, 2.0, 0.0]));
        assert_eq!(o.emissive_strength, Some(0.0));
    }

    #[test]
    fn a_wrong_shape_is_skipped_and_reported_once_by_field() {
        let mut o = MaterialOverride::default();
        let said = fill(&mut o, "\"red\"", 1.0, 3);
        assert_eq!(o, MaterialOverride::default());
        assert_eq!(said.len(), 1, "{said:?}");
        assert!(
            said[0].starts_with(
                "error[bad-type] entity 'lamp' material_overrides.Glass: expected a colour"
            ),
            "{said:?}"
        );

        let mut o = MaterialOverride::default();
        let said = fill(
            &mut o,
            "{ emissive = [1], emissive_strength = \"lots\", base_color = [0, 0, 1] }",
            1.0,
            3,
        );
        assert_eq!(o.emissive, None);
        assert_eq!(o.emissive_strength, None);
        assert_eq!(
            o.base_color,
            Some([0.0, 0.0, 1.0, 1.0]),
            "the good field lands"
        );
        assert_eq!(
            said,
            [
                "error[bad-type] entity 'lamp' material_overrides.Glass.emissive: expected a \
                 colour, [r, g, b] or [r, g, b, a], got array [1]",
                "error[bad-type] entity 'lamp' material_overrides.Glass.emissive_strength: \
                 expected a number, got string \"lots\"",
            ]
        );
    }
}
