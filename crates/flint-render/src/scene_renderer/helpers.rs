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
