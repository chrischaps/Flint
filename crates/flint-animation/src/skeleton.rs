//! Runtime skeleton representation with joint hierarchy and bone matrix computation

/// A single joint's local-space pose (translation, rotation, scale)
#[derive(Debug, Clone)]
pub struct JointPose {
    pub translation: [f32; 3],
    pub rotation: [f32; 4], // quaternion xyzw
    pub scale: [f32; 3],
}

impl Default for JointPose {
    fn default() -> Self {
        Self {
            translation: [0.0, 0.0, 0.0],
            rotation: [0.0, 0.0, 0.0, 1.0], // identity quaternion
            scale: [1.0, 1.0, 1.0],
        }
    }
}

/// Runtime skeleton with joint hierarchy, inverse bind matrices, and computed bone matrices.
///
/// The bone matrix pipeline:
/// 1. Animation writes into `local_poses` (per-joint TRS)
/// 2. `compute_bone_matrices()` walks the hierarchy root-to-leaf
/// 3. Accumulates `global[i] = global[parent[i]] * local_to_mat4(local_poses[i])`
/// 4. Final: `bone_matrices[i] = global[i] * inverse_bind_matrices[i]`
/// 5. `bone_matrices` is uploaded to GPU for vertex skinning
pub struct Skeleton {
    pub joint_names: Vec<String>,
    pub parents: Vec<Option<usize>>,
    pub inverse_bind_matrices: Vec<[[f32; 4]; 4]>,
    pub local_poses: Vec<JointPose>,
    /// GPU-ready bone matrices (global * inverse_bind)
    pub bone_matrices: Vec<[[f32; 4]; 4]>,
}

impl Skeleton {
    /// Create a new skeleton from imported data
    pub fn from_imported(imported: &flint_import::ImportedSkeleton) -> Self {
        let joint_count = imported.joints.len();

        let joint_names: Vec<String> = imported.joints.iter().map(|j| j.name.clone()).collect();
        let parents: Vec<Option<usize>> = imported.joints.iter().map(|j| j.parent).collect();
        let inverse_bind_matrices: Vec<[[f32; 4]; 4]> = imported
            .joints
            .iter()
            .map(|j| j.inverse_bind_matrix)
            .collect();
        let local_poses = vec![JointPose::default(); joint_count];
        let bone_matrices = vec![IDENTITY_4X4; joint_count];

        Self {
            joint_names,
            parents,
            inverse_bind_matrices,
            local_poses,
            bone_matrices,
        }
    }

    pub fn joint_count(&self) -> usize {
        self.joint_names.len()
    }

    /// Compute final bone matrices by walking the hierarchy root-to-leaf.
    ///
    /// glTF guarantees that joint arrays are in topological order (parents before children),
    /// so a single forward pass suffices.
    pub fn compute_bone_matrices(&mut self) {
        let count = self.joint_count();
        let mut globals = vec![IDENTITY_4X4; count];

        for i in 0..count {
            let local = pose_to_mat4(&self.local_poses[i]);

            globals[i] = match self.parents[i] {
                Some(parent_idx) => mat4_mul(&globals[parent_idx], &local),
                None => local,
            };

            self.bone_matrices[i] = mat4_mul(&globals[i], &self.inverse_bind_matrices[i]);
        }
    }
}

const IDENTITY_4X4: [[f32; 4]; 4] = [
    [1.0, 0.0, 0.0, 0.0],
    [0.0, 1.0, 0.0, 0.0],
    [0.0, 0.0, 1.0, 0.0],
    [0.0, 0.0, 0.0, 1.0],
];

/// Convert a JointPose (TRS) to a column-major 4x4 matrix
fn pose_to_mat4(pose: &JointPose) -> [[f32; 4]; 4] {
    let [tx, ty, tz] = pose.translation;
    let [qx, qy, qz, qw] = pose.rotation;
    let [sx, sy, sz] = pose.scale;

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

    // Column-major: m[col][row]
    [
        [(1.0 - (yy + zz)) * sx, (xy + wz) * sx, (xz - wy) * sx, 0.0],
        [(xy - wz) * sy, (1.0 - (xx + zz)) * sy, (yz + wx) * sy, 0.0],
        [(xz + wy) * sz, (yz - wx) * sz, (1.0 - (xx + yy)) * sz, 0.0],
        [tx, ty, tz, 1.0],
    ]
}

/// Multiply two column-major 4x4 matrices
fn mat4_mul(a: &[[f32; 4]; 4], b: &[[f32; 4]; 4]) -> [[f32; 4]; 4] {
    let mut out = [[0.0f32; 4]; 4];
    for col in 0..4 {
        for row in 0..4 {
            out[col][row] = (0..4).map(|k| a[k][row] * b[col][k]).sum();
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_pose_produces_identity_matrix() {
        let pose = JointPose::default();
        let m = pose_to_mat4(&pose);
        for col in 0..4 {
            for row in 0..4 {
                let expected = if col == row { 1.0 } else { 0.0 };
                assert!(
                    (m[col][row] - expected).abs() < 1e-6,
                    "m[{}][{}] = {}, expected {}",
                    col,
                    row,
                    m[col][row],
                    expected
                );
            }
        }
    }

    #[test]
    fn translation_pose_sets_last_column() {
        let pose = JointPose {
            translation: [3.0, 5.0, 7.0],
            ..Default::default()
        };
        let m = pose_to_mat4(&pose);
        assert!((m[3][0] - 3.0).abs() < 1e-6);
        assert!((m[3][1] - 5.0).abs() < 1e-6);
        assert!((m[3][2] - 7.0).abs() < 1e-6);
        assert!((m[3][3] - 1.0).abs() < 1e-6);
    }

    #[test]
    fn scale_pose_scales_diagonal() {
        let pose = JointPose {
            scale: [2.0, 3.0, 4.0],
            ..Default::default()
        };
        let m = pose_to_mat4(&pose);
        assert!((m[0][0] - 2.0).abs() < 1e-6);
        assert!((m[1][1] - 3.0).abs() < 1e-6);
        assert!((m[2][2] - 4.0).abs() < 1e-6);
    }

    #[test]
    fn bone_matrix_identity_skeleton() {
        // A two-joint skeleton where joint 1 is child of joint 0
        // With identity poses and identity IBMs, bone matrices should be identity
        let imported = flint_import::ImportedSkeleton {
            name: "test".to_string(),
            joints: vec![
                flint_import::ImportedJoint {
                    name: "root".to_string(),
                    index: 0,
                    parent: None,
                    inverse_bind_matrix: IDENTITY_4X4,
                },
                flint_import::ImportedJoint {
                    name: "child".to_string(),
                    index: 1,
                    parent: Some(0),
                    inverse_bind_matrix: IDENTITY_4X4,
                },
            ],
        };

        let mut skel = Skeleton::from_imported(&imported);
        skel.compute_bone_matrices();

        for (i, bm) in skel.bone_matrices.iter().enumerate() {
            for col in 0..4 {
                for row in 0..4 {
                    let expected = if col == row { 1.0 } else { 0.0 };
                    assert!(
                        (bm[col][row] - expected).abs() < 1e-5,
                        "bone_matrices[{}][{}][{}] = {}, expected {}",
                        i,
                        col,
                        row,
                        bm[col][row],
                        expected
                    );
                }
            }
        }
    }

    #[test]
    fn bone_matrix_with_translation() {
        // Root translated by (1,0,0), child translated by (0,2,0) relative to root
        // With identity IBMs, child's global should be (1,2,0)
        let imported = flint_import::ImportedSkeleton {
            name: "test".to_string(),
            joints: vec![
                flint_import::ImportedJoint {
                    name: "root".to_string(),
                    index: 0,
                    parent: None,
                    inverse_bind_matrix: IDENTITY_4X4,
                },
                flint_import::ImportedJoint {
                    name: "child".to_string(),
                    index: 1,
                    parent: Some(0),
                    inverse_bind_matrix: IDENTITY_4X4,
                },
            ],
        };

        let mut skel = Skeleton::from_imported(&imported);
        skel.local_poses[0].translation = [1.0, 0.0, 0.0];
        skel.local_poses[1].translation = [0.0, 2.0, 0.0];
        skel.compute_bone_matrices();

        // Root bone matrix should have translation (1,0,0)
        assert!((skel.bone_matrices[0][3][0] - 1.0).abs() < 1e-5);
        assert!((skel.bone_matrices[0][3][1] - 0.0).abs() < 1e-5);

        // Child bone matrix should have translation (1,2,0) — accumulated from parent
        assert!((skel.bone_matrices[1][3][0] - 1.0).abs() < 1e-5);
        assert!((skel.bone_matrices[1][3][1] - 2.0).abs() < 1e-5);
    }
}
