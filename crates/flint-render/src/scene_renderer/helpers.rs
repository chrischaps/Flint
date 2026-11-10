//! Free helper functions used by the scene renderer.

use crate::pipeline::BlendMode;
use flint_core::toml_util::toml_vec3;

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

/// Extract both the size and center offset from bounds.
pub(super) fn extract_bounds_info(bounds: &toml::Value) -> Option<([f32; 3], [f32; 3])> {
    let min = bounds.get("min")?;
    let max = bounds.get("max")?;

    let min_arr = toml_vec3(min)?;
    let max_arr = toml_vec3(max)?;

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

    /// Fills the fields still unset from one override value: an array is a
    /// base colour, a table may hold `base_color`, `emissive` and
    /// `emissive_strength`. Anything malformed is skipped.
    pub(super) fn fill_from(&mut self, value: &toml::Value, gltf_alpha: f32) {
        use flint_core::toml_util::toml_f32;
        let colour = |v: &toml::Value| -> Option<[f32; 4]> {
            let arr = v.as_array()?;
            let f = |i: usize| arr.get(i).and_then(toml_f32);
            Some([f(0)?, f(1)?, f(2)?, f(3).unwrap_or(gltf_alpha)])
        };
        match value {
            toml::Value::Array(_) => {
                if self.base_color.is_none() {
                    self.base_color = colour(value);
                }
            }
            toml::Value::Table(t) => {
                if self.base_color.is_none() {
                    self.base_color = t.get("base_color").and_then(colour);
                }
                if self.emissive.is_none() {
                    self.emissive = t
                        .get("emissive")
                        .and_then(colour)
                        .map(|[r, g, b, _]| [r.max(0.0), g.max(0.0), b.max(0.0)]);
                }
                if self.emissive_strength.is_none() {
                    self.emissive_strength = t
                        .get("emissive_strength")
                        .and_then(toml_f32)
                        .map(|s| s.max(0.0));
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod material_override_tests {
    use super::MaterialOverride;

    fn parse(s: &str) -> toml::Value {
        toml::from_str::<toml::Table>(&format!("v = {s}")).unwrap()["v"].clone()
    }

    #[test]
    fn array_is_a_base_colour_and_keeps_the_gltf_alpha() {
        let mut o = MaterialOverride::default();
        o.fill_from(&parse("[0.2, 0.4, 0.6]"), 0.5);
        assert_eq!(o.base_color, Some([0.2, 0.4, 0.6, 0.5]));
        assert_eq!(o.emissive, None);
    }

    #[test]
    fn table_sets_each_field_and_the_nearest_wins() {
        let mut o = MaterialOverride::default();
        // The child's value is read first: it lights the material.
        o.fill_from(
            &parse("{ emissive = [0, 1, 0.8], emissive_strength = 4 }"),
            1.0,
        );
        // The root paints it and tries to light it red; only the paint lands.
        o.fill_from(
            &parse("{ base_color = [1, 0, 0, 1], emissive = [1, 0, 0] }"),
            1.0,
        );
        assert_eq!(o.base_color, Some([1.0, 0.0, 0.0, 1.0]));
        assert_eq!(o.emissive, Some([0.0, 1.0, 0.8]));
        assert_eq!(o.emissive_strength, Some(4.0));
        assert!(o.is_full());
    }

    #[test]
    fn negative_light_counts_as_none_and_junk_is_skipped() {
        let mut o = MaterialOverride::default();
        o.fill_from(
            &parse("{ emissive = [-1, 2, 0], emissive_strength = -3 }"),
            1.0,
        );
        assert_eq!(o.emissive, Some([0.0, 2.0, 0.0]));
        assert_eq!(o.emissive_strength, Some(0.0));
        let mut o = MaterialOverride::default();
        o.fill_from(&parse("\"red\""), 1.0);
        o.fill_from(&parse("{ emissive = [1] }"), 1.0);
        assert_eq!(o, MaterialOverride::default());
    }
}
