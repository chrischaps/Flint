//! Generates geometry for body parts attached to bones.
//!
//! Each body part specifies a bone, a shape, dimensions, and a material.
//! The geometry is positioned in the bone's local space and rigidly skinned.

use crate::algorithms::mesh_builder::primitives::{
    ellipsoid, sphere, tapered_cylinder, EllipsoidParams, SphereParams, TaperedCylinderParams,
};
use crate::generators::util::{toml_f32, toml_string};
use crate::skinning::{SkeletonDef, SkinnedVertex};
use crate::types::MeshData;
use crate::{ProcGenError, Result};

/// Parsed body part definition.
#[derive(Debug, Clone)]
pub struct BodyPartDef {
    pub bone: String,
    pub shape: String,
    pub dimensions: [f32; 3],
    pub offset: [f32; 3],
    pub material: String,
}

/// Parse a body part definition from a TOML table.
pub fn parse_body_part(table: &toml::value::Table, idx: usize) -> Result<BodyPartDef> {
    let prefix = format!("body_parts[{idx}]");

    let bone = table
        .get("bone")
        .ok_or_else(|| ProcGenError::MissingParameter(format!("{prefix}.bone")))?;
    let bone = toml_string(bone, &format!("{prefix}.bone"))?;

    let shape = table
        .get("shape")
        .ok_or_else(|| ProcGenError::MissingParameter(format!("{prefix}.shape")))?;
    let shape = toml_string(shape, &format!("{prefix}.shape"))?;

    let dimensions = if let Some(v) = table.get("dimensions") {
        let arr = v.as_array().ok_or_else(|| ProcGenError::InvalidParameter {
            name: format!("{prefix}.dimensions"),
            reason: "expected an array of 3 numbers".into(),
        })?;
        if arr.len() != 3 {
            return Err(ProcGenError::InvalidParameter {
                name: format!("{prefix}.dimensions"),
                reason: format!("expected 3 elements, got {}", arr.len()),
            });
        }
        [
            toml_f32(&arr[0], &format!("{prefix}.dimensions[0]"))?,
            toml_f32(&arr[1], &format!("{prefix}.dimensions[1]"))?,
            toml_f32(&arr[2], &format!("{prefix}.dimensions[2]"))?,
        ]
    } else {
        [1.0, 1.0, 1.0]
    };

    let offset = if let Some(v) = table.get("offset") {
        let arr = v.as_array().ok_or_else(|| ProcGenError::InvalidParameter {
            name: format!("{prefix}.offset"),
            reason: "expected an array of 3 numbers".into(),
        })?;
        if arr.len() != 3 {
            return Err(ProcGenError::InvalidParameter {
                name: format!("{prefix}.offset"),
                reason: format!("expected 3 elements, got {}", arr.len()),
            });
        }
        [
            toml_f32(&arr[0], &format!("{prefix}.offset[0]"))?,
            toml_f32(&arr[1], &format!("{prefix}.offset[1]"))?,
            toml_f32(&arr[2], &format!("{prefix}.offset[2]"))?,
        ]
    } else {
        [0.0, 0.0, 0.0]
    };

    let material = table
        .get("material")
        .and_then(|v| v.as_str())
        .unwrap_or("default")
        .to_string();

    Ok(BodyPartDef {
        bone,
        shape,
        dimensions,
        offset,
        material,
    })
}

/// Generate geometry for a body part, returning skinned vertices and indices.
///
/// The geometry is generated centered at origin, then offset. Bone transforms
/// are applied via the skeleton (not baked into vertex positions), so vertices
/// are in the bone's local space.
pub fn generate_body_part(
    part: &BodyPartDef,
    skeleton: &SkeletonDef,
    world_transforms: &[[f32; 16]],
) -> Result<(Vec<SkinnedVertex>, Vec<u32>, String)> {
    let bone_idx = skeleton.find_bone(&part.bone).ok_or_else(|| {
        ProcGenError::InvalidParameter {
            name: "body_part.bone".into(),
            reason: format!("bone '{}' not found in skeleton", part.bone),
        }
    })?;

    let mesh = generate_shape(&part.shape, &part.dimensions)?;

    // Transform vertices to world space (baked) and assign rigid skinning to this bone
    let world = &world_transforms[bone_idx];
    let skinned_verts: Vec<SkinnedVertex> = mesh
        .vertices
        .iter()
        .map(|v| {
            let local_pos = [
                v.position[0] + part.offset[0],
                v.position[1] + part.offset[1],
                v.position[2] + part.offset[2],
            ];
            let world_pos = transform_point(world, &local_pos);
            let world_norm = transform_normal(world, &v.normal);

            SkinnedVertex {
                position: world_pos,
                normal: world_norm,
                tangent: v.tangent,
                uv: v.uv,
                joint_indices: [bone_idx as u16, 0, 0, 0],
                joint_weights: [1.0, 0.0, 0.0, 0.0],
            }
        })
        .collect();

    Ok((skinned_verts, mesh.indices, part.material.clone()))
}

fn generate_shape(shape: &str, dimensions: &[f32; 3]) -> Result<MeshData> {
    match shape {
        "ellipsoid" => Ok(ellipsoid(&EllipsoidParams {
            radius_x: dimensions[0],
            radius_y: dimensions[1],
            radius_z: dimensions[2],
            segments: 16,
            rings: 12,
        })),
        "sphere" => {
            let radius = dimensions[0];
            Ok(sphere(&SphereParams {
                radius,
                segments: 16,
                rings: 12,
            }))
        }
        "capsule" => {
            // Approximate capsule as tapered cylinder with hemispherical ends
            let radius = dimensions[0];
            let height = dimensions[1];
            Ok(tapered_cylinder(&TaperedCylinderParams {
                radius_bottom: radius,
                radius_top: radius,
                height,
                radial_segments: 12,
                height_segments: 4,
                caps: true,
            }))
        }
        "tapered_cylinder" => {
            let radius_bottom = dimensions[0];
            let radius_top = dimensions[1];
            let height = dimensions[2];
            Ok(tapered_cylinder(&TaperedCylinderParams {
                radius_bottom,
                radius_top,
                height,
                radial_segments: 12,
                height_segments: 4,
                caps: true,
            }))
        }
        _ => Err(ProcGenError::InvalidParameter {
            name: "body_part.shape".into(),
            reason: format!(
                "unknown shape '{shape}'; valid: ellipsoid, sphere, capsule, tapered_cylinder"
            ),
        }),
    }
}

/// Transform a point by a column-major 4x4 matrix.
pub(crate) fn transform_point(m: &[f32; 16], p: &[f32; 3]) -> [f32; 3] {
    [
        m[0] * p[0] + m[4] * p[1] + m[8] * p[2] + m[12],
        m[1] * p[0] + m[5] * p[1] + m[9] * p[2] + m[13],
        m[2] * p[0] + m[6] * p[1] + m[10] * p[2] + m[14],
    ]
}

/// Transform a normal by a column-major 4x4 matrix (using upper-left 3x3, then normalize).
pub(crate) fn transform_normal(m: &[f32; 16], n: &[f32; 3]) -> [f32; 3] {
    let x = m[0] * n[0] + m[4] * n[1] + m[8] * n[2];
    let y = m[1] * n[0] + m[5] * n[1] + m[9] * n[2];
    let z = m[2] * n[0] + m[6] * n[1] + m[10] * n[2];
    let len = (x * x + y * y + z * z).sqrt();
    if len > 1e-10 {
        [x / len, y / len, z / len]
    } else {
        [0.0, 1.0, 0.0]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skinning::BoneDef;

    #[test]
    fn generate_ellipsoid_body_part() {
        let skeleton = SkeletonDef {
            bones: vec![BoneDef {
                name: "root".into(),
                parent: None,
                rest_translation: [0.0, 0.0, 0.0],
                rest_rotation: [0.0, 0.0, 0.0, 1.0],
                rest_scale: [1.0, 1.0, 1.0],
            }],
        };
        let world_transforms = skeleton.compute_world_transforms();

        let part = BodyPartDef {
            bone: "root".into(),
            shape: "ellipsoid".into(),
            dimensions: [0.5, 0.3, 0.4],
            offset: [0.0, 0.0, 0.0],
            material: "test".into(),
        };

        let (verts, indices, mat) = generate_body_part(&part, &skeleton, &world_transforms).unwrap();
        assert!(!verts.is_empty());
        assert!(!indices.is_empty());
        assert_eq!(mat, "test");

        // All vertices should be skinned to bone 0
        for v in &verts {
            assert_eq!(v.joint_indices[0], 0);
            assert_eq!(v.joint_weights[0], 1.0);
        }
    }
}
