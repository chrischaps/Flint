//! Builds a [`SkeletonDef`] from bone table + limb chain definitions.
//!
//! Handles bilateral symmetry by mirroring bones across the X axis.

use crate::generators::util::{toml_f32, toml_string};
use crate::skinning::{BoneDef, SkeletonDef};
use crate::{ProcGenError, Result};
use std::collections::HashMap;

/// Parsed limb chain definition from TOML.
#[derive(Debug, Clone)]
pub struct LimbChainDef {
    pub name: String,
    pub parent_bone: String,
    pub attach_offset: [f32; 3],
    pub mirror: bool,
    pub segments: Vec<LimbSegmentDef>,
    pub material: String,
}

/// A single segment within a limb chain.
#[derive(Debug, Clone)]
pub struct LimbSegmentDef {
    pub length: f32,
    pub radius_start: f32,
    pub radius_end: f32,
    /// Euler angles [pitch, yaw, roll] in degrees for the rest pose.
    pub rest_angles: [f32; 3],
}

/// Build a skeleton from TOML bone definitions and limb chains.
///
/// Returns the skeleton and a mapping from limb chain info for geometry generation.
pub fn build_skeleton(
    bone_defs: &[toml::Value],
    limb_chains: &[LimbChainDef],
) -> Result<(SkeletonDef, Vec<LimbChainBones>)> {
    let mut bones = Vec::new();
    let mut name_to_index: HashMap<String, usize> = HashMap::new();

    // 1. Add explicit bones from the bone table
    for (i, def) in bone_defs.iter().enumerate() {
        let table = def.as_table().ok_or_else(|| ProcGenError::InvalidParameter {
            name: format!("bones[{i}]"),
            reason: "expected a table".into(),
        })?;

        let name = table
            .get("name")
            .ok_or_else(|| ProcGenError::MissingParameter(format!("bones[{i}].name")))?;
        let name = toml_string(name, &format!("bones[{i}].name"))?;

        let parent = if let Some(parent_val) = table.get("parent") {
            let parent_name = toml_string(parent_val, &format!("bones[{i}].parent"))?;
            let idx = name_to_index.get(&parent_name).ok_or_else(|| {
                ProcGenError::InvalidParameter {
                    name: format!("bones[{i}].parent"),
                    reason: format!("bone '{parent_name}' not found (must be defined before use)"),
                }
            })?;
            Some(*idx)
        } else {
            None
        };

        let position = parse_vec3_opt(table, "position", &format!("bones[{i}]"))?;
        let rotation_deg = parse_vec3_opt(table, "rotation", &format!("bones[{i}]"))?;
        let rotation_quat = euler_deg_to_quat(&rotation_deg);

        let idx = bones.len();
        name_to_index.insert(name.clone(), idx);
        bones.push(BoneDef {
            name,
            parent,
            rest_translation: position,
            rest_rotation: rotation_quat,
            rest_scale: [1.0, 1.0, 1.0],
        });
    }

    // 2. Add bones from limb chains
    let mut chain_bones = Vec::new();

    for chain in limb_chains {
        let sides = if chain.mirror {
            vec![("_R", 1.0f32), ("_L", -1.0f32)]
        } else {
            vec![("", 1.0f32)]
        };

        for (suffix, mirror_sign) in &sides {
            let parent_idx = name_to_index.get(&chain.parent_bone).ok_or_else(|| {
                ProcGenError::InvalidParameter {
                    name: format!("limb_chain.{}.parent_bone", chain.name),
                    reason: format!("bone '{}' not found", chain.parent_bone),
                }
            })?;

            let mut segment_bone_indices = Vec::new();
            let mut current_parent = *parent_idx;

            // First bone: at attach offset
            let attach = [
                chain.attach_offset[0] * mirror_sign,
                chain.attach_offset[1],
                chain.attach_offset[2],
            ];

            for (seg_i, seg) in chain.segments.iter().enumerate() {
                let bone_name = format!("{}{suffix}_{seg_i}", chain.name);
                let translation = if seg_i == 0 {
                    attach
                } else {
                    // Each subsequent bone is offset along the local -Y axis (length of previous segment)
                    [0.0, -chain.segments[seg_i - 1].length, 0.0]
                };

                // Mirror: flip the yaw and roll for mirrored side
                let angles = if *mirror_sign < 0.0 {
                    [seg.rest_angles[0], -seg.rest_angles[1], -seg.rest_angles[2]]
                } else {
                    seg.rest_angles
                };
                let rotation = euler_deg_to_quat(&angles);

                let idx = bones.len();
                name_to_index.insert(bone_name.clone(), idx);
                bones.push(BoneDef {
                    name: bone_name,
                    parent: Some(current_parent),
                    rest_translation: translation,
                    rest_rotation: rotation,
                    rest_scale: [1.0, 1.0, 1.0],
                });
                segment_bone_indices.push(idx);
                current_parent = idx;
            }

            chain_bones.push(LimbChainBones {
                chain_name: format!("{}{suffix}", chain.name),
                bone_indices: segment_bone_indices,
                segments: chain.segments.clone(),
                material: chain.material.clone(),
            });
        }
    }

    Ok((SkeletonDef { bones }, chain_bones))
}

/// Info about the bones generated for a limb chain (for geometry generation).
#[derive(Debug, Clone)]
pub struct LimbChainBones {
    pub chain_name: String,
    pub bone_indices: Vec<usize>,
    pub segments: Vec<LimbSegmentDef>,
    pub material: String,
}

/// Parse a limb chain definition from a TOML table.
pub fn parse_limb_chain(table: &toml::value::Table, idx: usize) -> Result<LimbChainDef> {
    let prefix = format!("limb_chains[{idx}]");

    let name = table
        .get("name")
        .ok_or_else(|| ProcGenError::MissingParameter(format!("{prefix}.name")))?;
    let name = toml_string(name, &format!("{prefix}.name"))?;

    let parent_bone = table
        .get("parent_bone")
        .ok_or_else(|| ProcGenError::MissingParameter(format!("{prefix}.parent_bone")))?;
    let parent_bone = toml_string(parent_bone, &format!("{prefix}.parent_bone"))?;

    let attach_offset = if let Some(v) = table.get("attach_offset") {
        parse_f32_array3(v, &format!("{prefix}.attach_offset"))?
    } else {
        [0.0, 0.0, 0.0]
    };

    let mirror = table
        .get("mirror")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    let material = table
        .get("material")
        .and_then(|v| v.as_str())
        .unwrap_or("default")
        .to_string();

    let segments_val = table
        .get("segments")
        .ok_or_else(|| ProcGenError::MissingParameter(format!("{prefix}.segments")))?;
    let segments_arr = segments_val
        .as_array()
        .ok_or_else(|| ProcGenError::InvalidParameter {
            name: format!("{prefix}.segments"),
            reason: "expected an array".into(),
        })?;

    let mut segments = Vec::new();
    for (si, seg_val) in segments_arr.iter().enumerate() {
        let seg_table = seg_val
            .as_table()
            .ok_or_else(|| ProcGenError::InvalidParameter {
                name: format!("{prefix}.segments[{si}]"),
                reason: "expected a table".into(),
            })?;

        let length = seg_table
            .get("length")
            .ok_or_else(|| {
                ProcGenError::MissingParameter(format!("{prefix}.segments[{si}].length"))
            })
            .and_then(|v| toml_f32(v, &format!("{prefix}.segments[{si}].length")))?;

        let radius_start = seg_table
            .get("radius_start")
            .ok_or_else(|| {
                ProcGenError::MissingParameter(format!("{prefix}.segments[{si}].radius_start"))
            })
            .and_then(|v| toml_f32(v, &format!("{prefix}.segments[{si}].radius_start")))?;

        let radius_end = seg_table
            .get("radius_end")
            .ok_or_else(|| {
                ProcGenError::MissingParameter(format!("{prefix}.segments[{si}].radius_end"))
            })
            .and_then(|v| toml_f32(v, &format!("{prefix}.segments[{si}].radius_end")))?;

        let rest_angles = if let Some(v) = seg_table.get("rest_angles") {
            parse_f32_array3(v, &format!("{prefix}.segments[{si}].rest_angles"))?
        } else {
            [0.0, 0.0, 0.0]
        };

        segments.push(LimbSegmentDef {
            length,
            radius_start,
            radius_end,
            rest_angles,
        });
    }

    Ok(LimbChainDef {
        name,
        parent_bone,
        attach_offset,
        mirror,
        segments,
        material,
    })
}

// ─── Helpers ────────────────────────────────────────────────────────────────

fn parse_vec3_opt(table: &toml::value::Table, key: &str, prefix: &str) -> Result<[f32; 3]> {
    if let Some(v) = table.get(key) {
        parse_f32_array3(v, &format!("{prefix}.{key}"))
    } else {
        Ok([0.0, 0.0, 0.0])
    }
}

fn parse_f32_array3(v: &toml::Value, name: &str) -> Result<[f32; 3]> {
    let arr = v.as_array().ok_or_else(|| ProcGenError::InvalidParameter {
        name: name.into(),
        reason: "expected an array of 3 numbers".into(),
    })?;
    if arr.len() != 3 {
        return Err(ProcGenError::InvalidParameter {
            name: name.into(),
            reason: format!("expected 3 elements, got {}", arr.len()),
        });
    }
    Ok([
        toml_f32(&arr[0], name)?,
        toml_f32(&arr[1], name)?,
        toml_f32(&arr[2], name)?,
    ])
}

/// Convert Euler angles (degrees, pitch/yaw/roll = XYZ) to a quaternion [x, y, z, w].
pub fn euler_deg_to_quat(angles_deg: &[f32; 3]) -> [f32; 4] {
    let [pitch, yaw, roll] = *angles_deg;
    let (px, py, pz) = (
        pitch.to_radians() * 0.5,
        yaw.to_radians() * 0.5,
        roll.to_radians() * 0.5,
    );

    let (sp, cp) = px.sin_cos();
    let (sy, cy) = py.sin_cos();
    let (sr, cr) = pz.sin_cos();

    // XYZ rotation order
    let x = sp * cy * cr + cp * sy * sr;
    let y = cp * sy * cr - sp * cy * sr;
    let z = cp * cy * sr + sp * sy * cr;
    let w = cp * cy * cr - sp * sy * sr;

    [x, y, z, w]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_euler_to_quat() {
        let q = euler_deg_to_quat(&[0.0, 0.0, 0.0]);
        assert!((q[3] - 1.0).abs() < 1e-6, "w should be 1.0 for identity");
        assert!(q[0].abs() < 1e-6);
        assert!(q[1].abs() < 1e-6);
        assert!(q[2].abs() < 1e-6);
    }

    #[test]
    fn build_skeleton_basic() {
        let bones: Vec<toml::Value> = vec![
            toml::Value::Table(toml::map::Map::from_iter([
                ("name".to_string(), toml::Value::String("root".into())),
                (
                    "position".to_string(),
                    toml::Value::Array(vec![
                        toml::Value::Float(0.0),
                        toml::Value::Float(0.0),
                        toml::Value::Float(0.0),
                    ]),
                ),
            ])),
            toml::Value::Table(toml::map::Map::from_iter([
                ("name".to_string(), toml::Value::String("head".into())),
                ("parent".to_string(), toml::Value::String("root".into())),
                (
                    "position".to_string(),
                    toml::Value::Array(vec![
                        toml::Value::Float(0.0),
                        toml::Value::Float(1.0),
                        toml::Value::Float(0.0),
                    ]),
                ),
            ])),
        ];

        let (skel, chains) = build_skeleton(&bones, &[]).unwrap();
        assert_eq!(skel.bones.len(), 2);
        assert_eq!(skel.bones[0].name, "root");
        assert_eq!(skel.bones[1].name, "head");
        assert_eq!(skel.bones[1].parent, Some(0));
        assert!(chains.is_empty());
    }

    #[test]
    fn build_skeleton_with_mirrored_chain() {
        let bones: Vec<toml::Value> = vec![toml::Value::Table(toml::map::Map::from_iter([(
            "name".to_string(),
            toml::Value::String("root".into()),
        )]))];

        let chain = LimbChainDef {
            name: "leg".into(),
            parent_bone: "root".into(),
            attach_offset: [0.5, 0.0, 0.0],
            mirror: true,
            segments: vec![
                LimbSegmentDef {
                    length: 1.0,
                    radius_start: 0.1,
                    radius_end: 0.08,
                    rest_angles: [0.0, 0.0, 0.0],
                },
                LimbSegmentDef {
                    length: 0.8,
                    radius_start: 0.08,
                    radius_end: 0.05,
                    rest_angles: [0.0, 0.0, 0.0],
                },
            ],
            material: "default".into(),
        };

        let (skel, chain_bones) = build_skeleton(&bones, &[chain]).unwrap();

        // root + 2 segments * 2 sides = 5 bones
        assert_eq!(skel.bones.len(), 5);
        assert_eq!(chain_bones.len(), 2); // R and L

        // Right leg
        assert_eq!(skel.bones[1].name, "leg_R_0");
        assert_eq!(skel.bones[2].name, "leg_R_1");
        // Left leg - mirrored X offset
        assert_eq!(skel.bones[3].name, "leg_L_0");
        assert_eq!(skel.bones[3].rest_translation[0], -0.5); // mirrored X
        assert_eq!(skel.bones[4].name, "leg_L_1");
    }

    #[test]
    fn limb_chain_bones_extend_negative_y() {
        let bones: Vec<toml::Value> = vec![toml::Value::Table(toml::map::Map::from_iter([(
            "name".to_string(),
            toml::Value::String("root".into()),
        )]))];

        let chain = LimbChainDef {
            name: "arm".into(),
            parent_bone: "root".into(),
            attach_offset: [1.0, 0.0, 0.0],
            mirror: false,
            segments: vec![
                LimbSegmentDef {
                    length: 2.0,
                    radius_start: 0.1,
                    radius_end: 0.08,
                    rest_angles: [0.0, 0.0, 0.0],
                },
                LimbSegmentDef {
                    length: 1.5,
                    radius_start: 0.08,
                    radius_end: 0.05,
                    rest_angles: [0.0, 0.0, 0.0],
                },
            ],
            material: "default".into(),
        };

        let (skel, _) = build_skeleton(&bones, &[chain]).unwrap();

        // First segment bone: at attach offset
        assert_eq!(skel.bones[1].rest_translation, [1.0, 0.0, 0.0]);
        // Second segment bone: offset along -Y by length of first segment
        assert_eq!(skel.bones[2].rest_translation, [0.0, -2.0, 0.0]);
    }
}
