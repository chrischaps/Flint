//! Skinning data types for skeleton-based procedural generation.
//!
//! These types represent skinned meshes with bone hierarchies, suitable for
//! export as glTF skinned meshes. The vertex format extends [`Vertex`] with
//! joint indices and weights for skeletal animation.

use serde::{Deserialize, Serialize};

use crate::types::{BoundingBox, MaterialData, SubMesh};

/// A skinned vertex with joint influences for skeletal animation.
///
/// Extends the standard vertex layout with 4 bone influences per vertex.
/// Joint weights must be normalized (sum to 1.0).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SkinnedVertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub tangent: [f32; 4],
    pub uv: [f32; 2],
    /// Indices into the skeleton's bone array (up to 4 influences).
    pub joint_indices: [u16; 4],
    /// Blend weights for each joint (must sum to 1.0).
    pub joint_weights: [f32; 4],
}

/// A single bone in a skeleton hierarchy.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BoneDef {
    pub name: String,
    /// Parent bone index, or `None` for root bones.
    pub parent: Option<usize>,
    /// Rest-pose translation relative to parent.
    pub rest_translation: [f32; 3],
    /// Rest-pose rotation as quaternion [x, y, z, w].
    pub rest_rotation: [f32; 4],
    /// Rest-pose scale.
    pub rest_scale: [f32; 3],
}

impl Default for BoneDef {
    fn default() -> Self {
        Self {
            name: String::new(),
            parent: None,
            rest_translation: [0.0, 0.0, 0.0],
            rest_rotation: [0.0, 0.0, 0.0, 1.0], // identity quaternion
            rest_scale: [1.0, 1.0, 1.0],
        }
    }
}

/// A complete skeleton definition (bone hierarchy in rest pose).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SkeletonDef {
    pub bones: Vec<BoneDef>,
}

impl SkeletonDef {
    /// Find a bone index by name.
    pub fn find_bone(&self, name: &str) -> Option<usize> {
        self.bones.iter().position(|b| b.name == name)
    }

    /// Compute the world-space transform matrix for each bone (column-major 4x4).
    ///
    /// Returns one `[f32; 16]` per bone, suitable for computing inverse bind matrices.
    pub fn compute_world_transforms(&self) -> Vec<[f32; 16]> {
        let mut world = vec![[0.0f32; 16]; self.bones.len()];

        for (i, bone) in self.bones.iter().enumerate() {
            let local = compose_trs(
                &bone.rest_translation,
                &bone.rest_rotation,
                &bone.rest_scale,
            );

            world[i] = if let Some(parent) = bone.parent {
                mat4_mul(&world[parent], &local)
            } else {
                local
            };
        }

        world
    }

    /// Compute inverse bind matrices for all bones (column-major 4x4).
    pub fn compute_inverse_bind_matrices(&self) -> Vec<[f32; 16]> {
        self.compute_world_transforms()
            .into_iter()
            .map(|w| mat4_inverse(&w))
            .collect()
    }
}

/// A skinned mesh with geometry, materials, and skeleton.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SkinnedMeshData {
    pub vertices: Vec<SkinnedVertex>,
    pub indices: Vec<u32>,
    pub materials: Vec<MaterialData>,
    #[serde(default)]
    pub submeshes: Vec<SubMesh>,
    pub bounding_box: BoundingBox,
    pub skeleton: SkeletonDef,
}

impl SkinnedMeshData {
    pub fn triangle_count(&self) -> usize {
        self.indices.len() / 3
    }

    pub fn vertex_count(&self) -> usize {
        self.vertices.len()
    }

    pub fn bone_count(&self) -> usize {
        self.skeleton.bones.len()
    }

    pub fn validate(&self) -> crate::Result<()> {
        if !self.indices.len().is_multiple_of(3) {
            return Err(crate::ProcGenError::InvalidMesh(format!(
                "index count {} is not a multiple of 3",
                self.indices.len()
            )));
        }
        let vert_count = self.vertices.len() as u32;
        for (i, &idx) in self.indices.iter().enumerate() {
            if idx >= vert_count {
                return Err(crate::ProcGenError::InvalidMesh(format!(
                    "index [{}] = {} is out of range (vertex count: {})",
                    i, idx, vert_count
                )));
            }
        }
        let bone_count = self.skeleton.bones.len() as u16;
        for (i, v) in self.vertices.iter().enumerate() {
            for k in 0..4 {
                if v.joint_weights[k] > 0.0 && v.joint_indices[k] >= bone_count {
                    return Err(crate::ProcGenError::InvalidMesh(format!(
                        "vertex {} joint_indices[{}] = {} out of range (bone count: {})",
                        i, k, v.joint_indices[k], bone_count
                    )));
                }
            }
        }
        Ok(())
    }
}

// ─── Matrix math helpers (column-major 4x4) ────────────────────────────────

/// Compose a TRS (Translation * Rotation * Scale) matrix in column-major order.
fn compose_trs(t: &[f32; 3], q: &[f32; 4], s: &[f32; 3]) -> [f32; 16] {
    let [qx, qy, qz, qw] = *q;

    // Rotation matrix from quaternion
    let x2 = qx + qx;
    let y2 = qy + qy;
    let z2 = qz + qz;
    let xx = qx * x2;
    let xy = qx * y2;
    let xz = qx * z2;
    let yy = qy * y2;
    let yz = qy * z2;
    let zz = qz * z2;
    let wx = qw * x2;
    let wy = qw * y2;
    let wz = qw * z2;

    // Column-major: m[col * 4 + row]
    [
        (1.0 - yy - zz) * s[0], // col 0
        (xy + wz) * s[0],
        (xz - wy) * s[0],
        0.0,
        (xy - wz) * s[1], // col 1
        (1.0 - xx - zz) * s[1],
        (yz + wx) * s[1],
        0.0,
        (xz + wy) * s[2], // col 2
        (yz - wx) * s[2],
        (1.0 - xx - yy) * s[2],
        0.0,
        t[0], // col 3
        t[1],
        t[2],
        1.0,
    ]
}

/// Multiply two column-major 4x4 matrices: result = a * b.
fn mat4_mul(a: &[f32; 16], b: &[f32; 16]) -> [f32; 16] {
    let mut out = [0.0f32; 16];
    for col in 0..4 {
        for row in 0..4 {
            let mut sum = 0.0;
            for k in 0..4 {
                sum += a[k * 4 + row] * b[col * 4 + k];
            }
            out[col * 4 + row] = sum;
        }
    }
    out
}

/// Invert a column-major 4x4 matrix using cofactor expansion.
fn mat4_inverse(m: &[f32; 16]) -> [f32; 16] {
    // Index helper: m[col][row] = m[col * 4 + row]
    let m = |c: usize, r: usize| m[c * 4 + r];

    let mut inv = [0.0f32; 16];

    inv[0] = m(1, 1) * m(2, 2) * m(3, 3) - m(1, 1) * m(2, 3) * m(3, 2)
        - m(2, 1) * m(1, 2) * m(3, 3)
        + m(2, 1) * m(1, 3) * m(3, 2)
        + m(3, 1) * m(1, 2) * m(2, 3)
        - m(3, 1) * m(1, 3) * m(2, 2);

    inv[4] = -m(1, 0) * m(2, 2) * m(3, 3) + m(1, 0) * m(2, 3) * m(3, 2)
        + m(2, 0) * m(1, 2) * m(3, 3)
        - m(2, 0) * m(1, 3) * m(3, 2)
        - m(3, 0) * m(1, 2) * m(2, 3)
        + m(3, 0) * m(1, 3) * m(2, 2);

    inv[8] = m(1, 0) * m(2, 1) * m(3, 3) - m(1, 0) * m(2, 3) * m(3, 1)
        - m(2, 0) * m(1, 1) * m(3, 3)
        + m(2, 0) * m(1, 3) * m(3, 1)
        + m(3, 0) * m(1, 1) * m(2, 3)
        - m(3, 0) * m(1, 3) * m(2, 1);

    inv[12] = -m(1, 0) * m(2, 1) * m(3, 2) + m(1, 0) * m(2, 2) * m(3, 1)
        + m(2, 0) * m(1, 1) * m(3, 2)
        - m(2, 0) * m(1, 2) * m(3, 1)
        - m(3, 0) * m(1, 1) * m(2, 2)
        + m(3, 0) * m(1, 2) * m(2, 1);

    inv[1] = -m(0, 1) * m(2, 2) * m(3, 3) + m(0, 1) * m(2, 3) * m(3, 2)
        + m(2, 1) * m(0, 2) * m(3, 3)
        - m(2, 1) * m(0, 3) * m(3, 2)
        - m(3, 1) * m(0, 2) * m(2, 3)
        + m(3, 1) * m(0, 3) * m(2, 2);

    inv[5] = m(0, 0) * m(2, 2) * m(3, 3) - m(0, 0) * m(2, 3) * m(3, 2)
        - m(2, 0) * m(0, 2) * m(3, 3)
        + m(2, 0) * m(0, 3) * m(3, 2)
        + m(3, 0) * m(0, 2) * m(2, 3)
        - m(3, 0) * m(0, 3) * m(2, 2);

    inv[9] = -m(0, 0) * m(2, 1) * m(3, 3) + m(0, 0) * m(2, 3) * m(3, 1)
        + m(2, 0) * m(0, 1) * m(3, 3)
        - m(2, 0) * m(0, 3) * m(3, 1)
        - m(3, 0) * m(0, 1) * m(2, 3)
        + m(3, 0) * m(0, 3) * m(2, 1);

    inv[13] = m(0, 0) * m(2, 1) * m(3, 2) - m(0, 0) * m(2, 2) * m(3, 1)
        - m(2, 0) * m(0, 1) * m(3, 2)
        + m(2, 0) * m(0, 2) * m(3, 1)
        + m(3, 0) * m(0, 1) * m(2, 2)
        - m(3, 0) * m(0, 2) * m(2, 1);

    inv[2] = m(0, 1) * m(1, 2) * m(3, 3) - m(0, 1) * m(1, 3) * m(3, 2)
        - m(1, 1) * m(0, 2) * m(3, 3)
        + m(1, 1) * m(0, 3) * m(3, 2)
        + m(3, 1) * m(0, 2) * m(1, 3)
        - m(3, 1) * m(0, 3) * m(1, 2);

    inv[6] = -m(0, 0) * m(1, 2) * m(3, 3) + m(0, 0) * m(1, 3) * m(3, 2)
        + m(1, 0) * m(0, 2) * m(3, 3)
        - m(1, 0) * m(0, 3) * m(3, 2)
        - m(3, 0) * m(0, 2) * m(1, 3)
        + m(3, 0) * m(0, 3) * m(1, 2);

    inv[10] = m(0, 0) * m(1, 1) * m(3, 3) - m(0, 0) * m(1, 3) * m(3, 1)
        - m(1, 0) * m(0, 1) * m(3, 3)
        + m(1, 0) * m(0, 3) * m(3, 1)
        + m(3, 0) * m(0, 1) * m(1, 3)
        - m(3, 0) * m(0, 3) * m(1, 1);

    inv[14] = -m(0, 0) * m(1, 1) * m(3, 2) + m(0, 0) * m(1, 2) * m(3, 1)
        + m(1, 0) * m(0, 1) * m(3, 2)
        - m(1, 0) * m(0, 2) * m(3, 1)
        - m(3, 0) * m(0, 1) * m(1, 2)
        + m(3, 0) * m(0, 2) * m(1, 1);

    inv[3] = -m(0, 1) * m(1, 2) * m(2, 3) + m(0, 1) * m(1, 3) * m(2, 2)
        + m(1, 1) * m(0, 2) * m(2, 3)
        - m(1, 1) * m(0, 3) * m(2, 2)
        - m(2, 1) * m(0, 2) * m(1, 3)
        + m(2, 1) * m(0, 3) * m(1, 2);

    inv[7] = m(0, 0) * m(1, 2) * m(2, 3) - m(0, 0) * m(1, 3) * m(2, 2)
        - m(1, 0) * m(0, 2) * m(2, 3)
        + m(1, 0) * m(0, 3) * m(2, 2)
        + m(2, 0) * m(0, 2) * m(1, 3)
        - m(2, 0) * m(0, 3) * m(1, 2);

    inv[11] = -m(0, 0) * m(1, 1) * m(2, 3) + m(0, 0) * m(1, 3) * m(2, 1)
        + m(1, 0) * m(0, 1) * m(2, 3)
        - m(1, 0) * m(0, 3) * m(2, 1)
        - m(2, 0) * m(0, 1) * m(1, 3)
        + m(2, 0) * m(0, 3) * m(1, 1);

    inv[15] = m(0, 0) * m(1, 1) * m(2, 2) - m(0, 0) * m(1, 2) * m(2, 1)
        - m(1, 0) * m(0, 1) * m(2, 2)
        + m(1, 0) * m(0, 2) * m(2, 1)
        + m(2, 0) * m(0, 1) * m(1, 2)
        - m(2, 0) * m(0, 2) * m(1, 1);

    let det = m(0, 0) * inv[0] + m(1, 0) * inv[1] + m(2, 0) * inv[2] + m(3, 0) * inv[3];

    if det.abs() < 1e-10 {
        return identity_mat4();
    }

    let inv_det = 1.0 / det;
    let mut result = [0.0f32; 16];
    for i in 0..16 {
        result[i] = inv[i] * inv_det;
    }
    result
}

/// 4x4 identity matrix (column-major).
fn identity_mat4() -> [f32; 16] {
    [
        1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use flint_core::Vec3;

    #[test]
    fn identity_trs() {
        let m = compose_trs(&[0.0, 0.0, 0.0], &[0.0, 0.0, 0.0, 1.0], &[1.0, 1.0, 1.0]);
        let id = identity_mat4();
        for i in 0..16 {
            assert!(
                (m[i] - id[i]).abs() < 1e-6,
                "identity TRS mismatch at index {}: {} vs {}",
                i,
                m[i],
                id[i]
            );
        }
    }

    #[test]
    fn translation_trs() {
        let m = compose_trs(&[1.0, 2.0, 3.0], &[0.0, 0.0, 0.0, 1.0], &[1.0, 1.0, 1.0]);
        assert!((m[12] - 1.0).abs() < 1e-6);
        assert!((m[13] - 2.0).abs() < 1e-6);
        assert!((m[14] - 3.0).abs() < 1e-6);
    }

    #[test]
    fn inverse_of_identity() {
        let id = identity_mat4();
        let inv = mat4_inverse(&id);
        for i in 0..16 {
            assert!(
                (inv[i] - id[i]).abs() < 1e-6,
                "inverse of identity mismatch at {}",
                i
            );
        }
    }

    #[test]
    fn inverse_of_translation() {
        let m = compose_trs(&[5.0, -3.0, 7.0], &[0.0, 0.0, 0.0, 1.0], &[1.0, 1.0, 1.0]);
        let inv = mat4_inverse(&m);
        let product = mat4_mul(&m, &inv);
        let id = identity_mat4();
        for i in 0..16 {
            assert!(
                (product[i] - id[i]).abs() < 1e-4,
                "M * M^-1 should be identity, mismatch at {}: {}",
                i,
                product[i]
            );
        }
    }

    #[test]
    fn skeleton_find_bone() {
        let skel = SkeletonDef {
            bones: vec![
                BoneDef {
                    name: "root".into(),
                    parent: None,
                    ..Default::default()
                },
                BoneDef {
                    name: "spine".into(),
                    parent: Some(0),
                    ..Default::default()
                },
            ],
        };
        assert_eq!(skel.find_bone("root"), Some(0));
        assert_eq!(skel.find_bone("spine"), Some(1));
        assert_eq!(skel.find_bone("missing"), None);
    }

    #[test]
    fn skinned_mesh_validate_ok() {
        let mesh = SkinnedMeshData {
            vertices: vec![
                SkinnedVertex {
                    position: [0.0, 0.0, 0.0],
                    normal: [0.0, 1.0, 0.0],
                    tangent: [1.0, 0.0, 0.0, 1.0],
                    uv: [0.0, 0.0],
                    joint_indices: [0, 0, 0, 0],
                    joint_weights: [1.0, 0.0, 0.0, 0.0],
                },
                SkinnedVertex {
                    position: [1.0, 0.0, 0.0],
                    normal: [0.0, 1.0, 0.0],
                    tangent: [1.0, 0.0, 0.0, 1.0],
                    uv: [1.0, 0.0],
                    joint_indices: [0, 0, 0, 0],
                    joint_weights: [1.0, 0.0, 0.0, 0.0],
                },
                SkinnedVertex {
                    position: [0.0, 1.0, 0.0],
                    normal: [0.0, 1.0, 0.0],
                    tangent: [1.0, 0.0, 0.0, 1.0],
                    uv: [0.0, 1.0],
                    joint_indices: [0, 0, 0, 0],
                    joint_weights: [1.0, 0.0, 0.0, 0.0],
                },
            ],
            indices: vec![0, 1, 2],
            materials: vec![MaterialData::default()],
            submeshes: vec![],
            bounding_box: BoundingBox {
                min: Vec3::ZERO,
                max: Vec3::new(1.0, 1.0, 0.0),
            },
            skeleton: SkeletonDef {
                bones: vec![BoneDef {
                    name: "root".into(),
                    parent: None,
                    ..Default::default()
                }],
            },
        };
        assert!(mesh.validate().is_ok());
        assert_eq!(mesh.vertex_count(), 3);
        assert_eq!(mesh.triangle_count(), 1);
        assert_eq!(mesh.bone_count(), 1);
    }

    #[test]
    fn skinned_mesh_validate_bad_joint_index() {
        let mesh = SkinnedMeshData {
            vertices: vec![SkinnedVertex {
                position: [0.0, 0.0, 0.0],
                normal: [0.0, 1.0, 0.0],
                tangent: [1.0, 0.0, 0.0, 1.0],
                uv: [0.0, 0.0],
                joint_indices: [5, 0, 0, 0], // bone 5 doesn't exist
                joint_weights: [1.0, 0.0, 0.0, 0.0],
            }],
            indices: vec![],
            materials: vec![],
            submeshes: vec![],
            bounding_box: BoundingBox {
                min: Vec3::ZERO,
                max: Vec3::ZERO,
            },
            skeleton: SkeletonDef {
                bones: vec![BoneDef {
                    name: "root".into(),
                    parent: None,
                    ..Default::default()
                }],
            },
        };
        assert!(mesh.validate().is_err());
    }

    #[test]
    fn world_transforms_chain() {
        let skel = SkeletonDef {
            bones: vec![
                BoneDef {
                    name: "root".into(),
                    parent: None,
                    rest_translation: [0.0, 1.0, 0.0],
                    rest_rotation: [0.0, 0.0, 0.0, 1.0],
                    rest_scale: [1.0, 1.0, 1.0],
                },
                BoneDef {
                    name: "child".into(),
                    parent: Some(0),
                    rest_translation: [0.0, 2.0, 0.0],
                    rest_rotation: [0.0, 0.0, 0.0, 1.0],
                    rest_scale: [1.0, 1.0, 1.0],
                },
            ],
        };

        let world = skel.compute_world_transforms();
        // Root at Y=1
        assert!((world[0][13] - 1.0).abs() < 1e-6);
        // Child at Y=1+2=3
        assert!((world[1][13] - 3.0).abs() < 1e-6);
    }
}
