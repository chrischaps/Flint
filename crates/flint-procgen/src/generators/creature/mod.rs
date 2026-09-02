//! Skeleton-based creature generator.
//!
//! Generates skinned meshes from a data-driven bone hierarchy, body parts,
//! and limb chains. All anatomy-specific details live in the TOML spec;
//! the generator only knows about generic concepts: bones, shapes, and chains.

pub mod body_parts;
pub mod limb_chains;
pub mod skeleton_builder;

use std::collections::HashMap;

use crate::generator::{GenerationCost, Generator};
use crate::generators::util::{parse_hex_color, toml_f32};
use crate::output::{GeneratorOutput, OutputKind};
use crate::skinning::{SkinnedMeshData, SkinnedVertex};
use crate::spec::ProcGenSpec;
use crate::types::{BoundingBox, MaterialData, SubMesh};
use crate::{ProcGenError, Result, SeededRng};

use self::body_parts::{generate_body_part, parse_body_part, BodyPartDef};
use self::limb_chains::generate_limb_chain;
use self::skeleton_builder::{build_skeleton, parse_limb_chain, LimbChainDef};

pub struct CreatureGenerator;

impl Generator for CreatureGenerator {
    fn type_name(&self) -> &str {
        "creature_v1"
    }

    fn output_kind(&self) -> OutputKind {
        OutputKind::Mesh
    }

    fn generate(&self, spec: &ProcGenSpec, _rng: &mut SeededRng) -> Result<GeneratorOutput> {
        let params = spec
            .params
            .as_table()
            .ok_or_else(|| ProcGenError::MissingParameter("params".into()))?;

        // ─── Parse materials ────────────────────────────────────────────
        let materials = parse_materials(params)?;

        // ─── Parse bones ────────────────────────────────────────────────
        let bone_defs: Vec<toml::Value> = if let Some(v) = params.get("bones") {
            v.as_array()
                .ok_or_else(|| ProcGenError::InvalidParameter {
                    name: "bones".into(),
                    reason: "expected an array".into(),
                })?
                .clone()
        } else {
            vec![]
        };

        // ─── Parse limb chains ──────────────────────────────────────────
        let limb_chain_defs: Vec<LimbChainDef> = if let Some(v) = params.get("limb_chains") {
            let arr = v.as_array().ok_or_else(|| ProcGenError::InvalidParameter {
                name: "limb_chains".into(),
                reason: "expected an array".into(),
            })?;
            arr.iter()
                .enumerate()
                .map(|(i, val)| {
                    let table = val
                        .as_table()
                        .ok_or_else(|| ProcGenError::InvalidParameter {
                            name: format!("limb_chains[{i}]"),
                            reason: "expected a table".into(),
                        })?;
                    parse_limb_chain(table, i)
                })
                .collect::<Result<Vec<_>>>()?
        } else {
            vec![]
        };

        // ─── Parse body parts ───────────────────────────────────────────
        let body_part_defs: Vec<BodyPartDef> = if let Some(v) = params.get("body_parts") {
            let arr = v.as_array().ok_or_else(|| ProcGenError::InvalidParameter {
                name: "body_parts".into(),
                reason: "expected an array".into(),
            })?;
            arr.iter()
                .enumerate()
                .map(|(i, val)| {
                    let table = val
                        .as_table()
                        .ok_or_else(|| ProcGenError::InvalidParameter {
                            name: format!("body_parts[{i}]"),
                            reason: "expected a table".into(),
                        })?;
                    parse_body_part(table, i)
                })
                .collect::<Result<Vec<_>>>()?
        } else {
            vec![]
        };

        // ─── Build skeleton ─────────────────────────────────────────────
        let (skeleton, chain_bones) = build_skeleton(&bone_defs, &limb_chain_defs)?;
        let world_transforms = skeleton.compute_world_transforms();

        // ─── Generate geometry ──────────────────────────────────────────
        let mut all_vertices: Vec<SkinnedVertex> = Vec::new();
        let mut all_indices: Vec<u32> = Vec::new();
        let mut all_materials: Vec<MaterialData> = Vec::new();
        let mut all_submeshes: Vec<SubMesh> = Vec::new();
        let mut material_index_map: HashMap<String, usize> = HashMap::new();

        // Helper to get or create material index
        let get_material_index = |mat_name: &str,
                                  materials: &HashMap<String, MaterialData>,
                                  all_mats: &mut Vec<MaterialData>,
                                  mat_map: &mut HashMap<String, usize>|
         -> usize {
            if let Some(&idx) = mat_map.get(mat_name) {
                return idx;
            }
            let mat = materials
                .get(mat_name)
                .cloned()
                .unwrap_or_else(|| MaterialData {
                    name: mat_name.to_string(),
                    ..Default::default()
                });
            let idx = all_mats.len();
            all_mats.push(mat);
            mat_map.insert(mat_name.to_string(), idx);
            idx
        };

        // Body parts
        for part in &body_part_defs {
            let (verts, indices, mat_name) =
                generate_body_part(part, &skeleton, &world_transforms)?;

            let mat_idx = get_material_index(
                &mat_name,
                &materials,
                &mut all_materials,
                &mut material_index_map,
            );

            let index_start = all_indices.len() as u32;
            let base_vertex = all_vertices.len() as u32;

            all_vertices.extend(verts);
            all_indices.extend(indices.iter().map(|&i| i + base_vertex));

            all_submeshes.push(SubMesh {
                index_start,
                index_count: indices.len() as u32,
                material_index: mat_idx,
            });
        }

        // Limb chains
        for chain in &chain_bones {
            let (verts, indices) = generate_limb_chain(chain, &skeleton, &world_transforms);

            let mat_idx = get_material_index(
                &chain.material,
                &materials,
                &mut all_materials,
                &mut material_index_map,
            );

            let index_start = all_indices.len() as u32;
            let base_vertex = all_vertices.len() as u32;

            all_vertices.extend(verts);
            all_indices.extend(indices.iter().map(|&i| i + base_vertex));

            all_submeshes.push(SubMesh {
                index_start,
                index_count: indices.len() as u32,
                material_index: mat_idx,
            });
        }

        // ─── Compute bounding box ───────────────────────────────────────
        let positions: Vec<[f32; 3]> = all_vertices.iter().map(|v| v.position).collect();
        let bounding_box = BoundingBox::from_positions(&positions);

        let mesh = SkinnedMeshData {
            vertices: all_vertices,
            indices: all_indices,
            materials: all_materials,
            submeshes: all_submeshes,
            bounding_box,
            skeleton,
        };

        mesh.validate()?;

        Ok(GeneratorOutput::SkinnedMesh(mesh))
    }

    fn param_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "symmetry": {
                    "type": "string",
                    "enum": ["bilateral", "none"],
                    "default": "none",
                    "description": "Symmetry mode for mirroring"
                },
                "bones": {
                    "type": "array",
                    "description": "Bone hierarchy definitions",
                    "items": {
                        "type": "object",
                        "properties": {
                            "name": { "type": "string" },
                            "parent": { "type": "string" },
                            "position": {
                                "type": "array",
                                "items": { "type": "number" },
                                "minItems": 3,
                                "maxItems": 3
                            },
                            "rotation": {
                                "type": "array",
                                "items": { "type": "number" },
                                "minItems": 3,
                                "maxItems": 3,
                                "description": "Euler angles in degrees [pitch, yaw, roll]"
                            }
                        },
                        "required": ["name"]
                    }
                },
                "body_parts": {
                    "type": "array",
                    "description": "Geometry attached to bones",
                    "items": {
                        "type": "object",
                        "properties": {
                            "bone": { "type": "string" },
                            "shape": {
                                "type": "string",
                                "enum": ["ellipsoid", "sphere", "capsule", "tapered_cylinder"]
                            },
                            "dimensions": {
                                "type": "array",
                                "items": { "type": "number" },
                                "minItems": 3,
                                "maxItems": 3
                            },
                            "material": { "type": "string" }
                        },
                        "required": ["bone", "shape"]
                    }
                },
                "limb_chains": {
                    "type": "array",
                    "description": "Auto-generated bone chains with tapered geometry",
                    "items": {
                        "type": "object",
                        "properties": {
                            "name": { "type": "string" },
                            "parent_bone": { "type": "string" },
                            "mirror": { "type": "boolean", "default": false },
                            "segments": {
                                "type": "array",
                                "items": {
                                    "type": "object",
                                    "properties": {
                                        "length": { "type": "number" },
                                        "radius_start": { "type": "number" },
                                        "radius_end": { "type": "number" }
                                    }
                                }
                            }
                        },
                        "required": ["name", "parent_bone", "segments"]
                    }
                }
            }
        })
    }

    fn estimate_cost(&self, spec: &ProcGenSpec) -> GenerationCost {
        let params = spec.params.as_table();

        let body_part_count = params
            .and_then(|p| p.get("body_parts"))
            .and_then(|v| v.as_array())
            .map(|a| a.len())
            .unwrap_or(0);

        let limb_segment_count: usize = params
            .and_then(|p| p.get("limb_chains"))
            .and_then(|v| v.as_array())
            .map(|chains| {
                chains
                    .iter()
                    .map(|c| {
                        let seg_count = c
                            .get("segments")
                            .and_then(|s| s.as_array())
                            .map(|a| a.len())
                            .unwrap_or(0);
                        let mirror = c.get("mirror").and_then(|m| m.as_bool()).unwrap_or(false);
                        if mirror {
                            seg_count * 2
                        } else {
                            seg_count
                        }
                    })
                    .sum()
            })
            .unwrap_or(0);

        // Rough estimates: ~220 verts per body part (16x12 ellipsoid), ~90 per limb segment
        let est_verts = (body_part_count * 220 + limb_segment_count * 90) as u32;
        let est_tris = est_verts; // rough 1:1 ratio for these primitives

        GenerationCost {
            estimated_vertices: est_verts,
            estimated_triangles: est_tris,
            estimated_texture_bytes: 0,
            estimated_generation_ms: 5.0,
        }
    }
}

/// Parse the `[params.materials.*]` table into a name->MaterialData map.
fn parse_materials(
    params: &toml::map::Map<String, toml::Value>,
) -> Result<HashMap<String, MaterialData>> {
    let mut map = HashMap::new();

    let materials_table = match params.get("materials") {
        Some(v) => match v.as_table() {
            Some(t) => t,
            None => return Ok(map),
        },
        None => return Ok(map),
    };

    for (name, val) in materials_table {
        let table = val
            .as_table()
            .ok_or_else(|| ProcGenError::InvalidParameter {
                name: format!("materials.{name}"),
                reason: "expected a table".into(),
            })?;

        let base_color = if let Some(hex) = table.get("base_color").and_then(|v| v.as_str()) {
            parse_hex_color(hex)?
        } else {
            [0.5, 0.5, 0.5, 1.0]
        };

        let roughness = table
            .get("roughness")
            .map(|v| toml_f32(v, &format!("materials.{name}.roughness")))
            .transpose()?
            .unwrap_or(0.5);

        let metallic = table
            .get("metallic")
            .map(|v| toml_f32(v, &format!("materials.{name}.metallic")))
            .transpose()?
            .unwrap_or(0.0);

        map.insert(
            name.clone(),
            MaterialData {
                name: name.clone(),
                base_color,
                metallic,
                roughness,
            },
        );
    }

    Ok(map)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creature_generator_type_name() {
        let gen = CreatureGenerator;
        assert_eq!(gen.type_name(), "creature_v1");
        assert_eq!(gen.output_kind(), OutputKind::Mesh);
    }

    #[test]
    fn creature_generator_minimal_spec() {
        let spec = ProcGenSpec::parse(
            r#"
generator = "creature_v1"

[meta]
name = "test_creature"
version = "1.0.0"

[seed]
mode = "fixed"
value = 42

[params]

[[params.bones]]
name = "root"
position = [0, 0, 0]

[[params.body_parts]]
bone = "root"
shape = "ellipsoid"
dimensions = [0.5, 0.3, 0.4]
"#,
        )
        .unwrap();

        let mut rng = SeededRng::new(42);
        let output = CreatureGenerator.generate(&spec, &mut rng).unwrap();

        let mesh = output.as_skinned_mesh().unwrap();
        assert!(mesh.vertex_count() > 0);
        assert!(mesh.triangle_count() > 0);
        assert_eq!(mesh.bone_count(), 1);
        assert!(mesh.validate().is_ok());
    }

    #[test]
    fn creature_generator_with_limb_chain() {
        let spec = ProcGenSpec::parse(
            r#"
generator = "creature_v1"

[meta]
name = "test_creature_limb"
version = "1.0.0"

[seed]
mode = "fixed"
value = 42

[params]

[[params.bones]]
name = "root"
position = [0, 0.5, 0]

[[params.body_parts]]
bone = "root"
shape = "sphere"
dimensions = [0.3, 0.3, 0.3]

[[params.limb_chains]]
name = "leg"
parent_bone = "root"
attach_offset = [0.2, -0.1, 0]
mirror = true
segments = [
  { length = 0.3, radius_start = 0.04, radius_end = 0.03, rest_angles = [-30, 0, 0] },
  { length = 0.2, radius_start = 0.03, radius_end = 0.015, rest_angles = [45, 0, 0] },
]
material = "default"
"#,
        )
        .unwrap();

        let mut rng = SeededRng::new(42);
        let output = CreatureGenerator.generate(&spec, &mut rng).unwrap();

        let mesh = output.as_skinned_mesh().unwrap();
        assert!(mesh.vertex_count() > 0);
        // root + 2 segments * 2 sides (mirrored) = 5 bones
        assert_eq!(mesh.bone_count(), 5);
        assert!(mesh.validate().is_ok());
    }

    #[test]
    fn creature_generator_with_materials() {
        let spec = ProcGenSpec::parse(
            r##"
generator = "creature_v1"

[meta]
name = "test_materials"
version = "1.0.0"

[seed]
mode = "fixed"
value = 42

[params]

[[params.bones]]
name = "root"

[[params.body_parts]]
bone = "root"
shape = "ellipsoid"
dimensions = [0.5, 0.3, 0.4]
material = "shell"

[params.materials.shell]
base_color = "#8B4513"
roughness = 0.3
metallic = 0.1
"##,
        )
        .unwrap();

        let mut rng = SeededRng::new(42);
        let output = CreatureGenerator.generate(&spec, &mut rng).unwrap();

        let mesh = output.as_skinned_mesh().unwrap();
        assert_eq!(mesh.materials.len(), 1);
        assert_eq!(mesh.materials[0].name, "shell");
        assert!((mesh.materials[0].roughness - 0.3).abs() < 1e-3);
    }
}
