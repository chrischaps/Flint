//! GLB binary export for procedurally generated meshes.
//!
//! Converts [`MeshData`] into the GLB binary container format using the
//! `gltf::json` AST types. The output is a self-contained `.glb` file with
//! interleaved vertex data (position, normal, tangent, UV) and `u32` indices.

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::mem;

use gltf::json::accessor::{ComponentType, GenericComponentType, Type};
use gltf::json::buffer::Target;
use gltf::json::mesh::{Mode, Primitive, Semantic};
use gltf::json::validation::{Checked, USize64};
use gltf::json;

use crate::types::{MaterialData, MeshData};
use crate::Result;

/// Bytes per interleaved vertex: position(12) + normal(12) + tangent(16) + uv(8) = 48.
const VERTEX_STRIDE: usize = 48;

/// Convert a single [`MeshData`] into a self-contained GLB binary.
pub fn mesh_to_glb(mesh: &MeshData) -> Result<Vec<u8>> {
    meshes_to_glb_inner(&[mesh])
}

/// Convert multiple [`MeshData`] (e.g. LOD levels) into a single GLB binary.
///
/// Each mesh becomes a separate glTF mesh node. LOD 0 is the first mesh.
pub fn mesh_lods_to_glb(meshes: &[MeshData]) -> Result<Vec<u8>> {
    let refs: Vec<&MeshData> = meshes.iter().collect();
    meshes_to_glb_inner(&refs)
}

/// Internal: build a GLB from one or more meshes sharing a single binary buffer.
fn meshes_to_glb_inner(meshes: &[&MeshData]) -> Result<Vec<u8>> {
    if meshes.is_empty() {
        return Err(crate::ProcGenError::InvalidMesh(
            "no meshes provided for GLB export".into(),
        ));
    }

    for (i, mesh) in meshes.iter().enumerate() {
        mesh.validate().map_err(|e| {
            crate::ProcGenError::InvalidMesh(format!("mesh {i} validation failed: {e}"))
        })?;
    }

    let bin_data = build_buffer(meshes);
    let root = build_gltf_root(meshes, bin_data.len());
    write_glb(&root, &bin_data)
}

/// Pack all mesh vertex + index data into a single binary buffer.
///
/// Layout per mesh: [vertices (interleaved)] [padding to 4-byte] [indices (u32)]
/// Each mesh section is appended sequentially.
fn build_buffer(meshes: &[&MeshData]) -> Vec<u8> {
    let mut buf = Vec::new();

    for mesh in meshes {
        // Vertices: interleaved pos(3) + normal(3) + tangent(4) + uv(2) = 12 floats = 48 bytes
        for v in &mesh.vertices {
            buf.extend_from_slice(bytemuck_f32_slice(&v.position));
            buf.extend_from_slice(bytemuck_f32_slice(&v.normal));
            buf.extend_from_slice(bytemuck_f32_slice(&v.tangent));
            buf.extend_from_slice(bytemuck_f32_slice(&v.uv));
        }

        // Pad vertex section to 4-byte alignment (already aligned for f32, but be safe)
        while buf.len() % 4 != 0 {
            buf.push(0);
        }

        // Indices: u32
        for &idx in &mesh.indices {
            buf.extend_from_slice(&idx.to_le_bytes());
        }

        // Pad to 4-byte alignment after indices
        while buf.len() % 4 != 0 {
            buf.push(0);
        }
    }

    buf
}

/// Reinterpret an `f32` slice as little-endian bytes.
fn bytemuck_f32_slice(data: &[f32]) -> &[u8] {
    let ptr = data.as_ptr() as *const u8;
    let len = data.len() * mem::size_of::<f32>();
    // SAFETY: f32 has no alignment requirements stricter than u8, and the
    // original slice is valid for len bytes.
    unsafe { std::slice::from_raw_parts(ptr, len) }
}

/// Build the glTF JSON root with buffer views, accessors, meshes, materials,
/// nodes, and a single scene.
fn build_gltf_root(meshes: &[&MeshData], buffer_byte_length: usize) -> json::Root {
    let mut root = json::Root::default();
    root.asset.generator = Some("Flint Procgen".into());

    // Single buffer covering all mesh data
    let buffer_idx = root.push(json::Buffer {
        byte_length: USize64::from(buffer_byte_length),
        name: None,
        uri: None,
        extensions: None,
        extras: Default::default(),
    });

    let mut byte_offset: usize = 0;
    let mut node_indices = Vec::new();

    for (mesh_i, mesh) in meshes.iter().enumerate() {
        let vertex_bytes = mesh.vertices.len() * VERTEX_STRIDE;
        let vertex_offset = byte_offset;

        // Pad vertex section to 4-byte alignment
        let padded_vertex_bytes = (vertex_bytes + 3) & !3;

        let index_bytes = mesh.indices.len() * mem::size_of::<u32>();
        let index_offset = vertex_offset + padded_vertex_bytes;

        let padded_index_bytes = (index_bytes + 3) & !3;
        byte_offset = index_offset + padded_index_bytes;

        // Buffer view for vertex data (interleaved)
        let vtx_view_idx = root.push(json::buffer::View {
            buffer: buffer_idx,
            byte_length: USize64::from(vertex_bytes),
            byte_offset: Some(USize64::from(vertex_offset)),
            byte_stride: Some(json::buffer::Stride(VERTEX_STRIDE)),
            name: None,
            target: Some(Checked::Valid(Target::ArrayBuffer)),
            extensions: None,
            extras: Default::default(),
        });

        // Buffer view for index data
        let idx_view_idx = root.push(json::buffer::View {
            buffer: buffer_idx,
            byte_length: USize64::from(index_bytes),
            byte_offset: Some(USize64::from(index_offset)),
            byte_stride: None,
            name: None,
            target: Some(Checked::Valid(Target::ElementArrayBuffer)),
            extensions: None,
            extras: Default::default(),
        });

        // Compute bounding box for POSITION accessor
        let bb = &mesh.bounding_box;
        let pos_min = serde_json::json!([bb.min.x, bb.min.y, bb.min.z]);
        let pos_max = serde_json::json!([bb.max.x, bb.max.y, bb.max.z]);
        let vertex_count = mesh.vertices.len();

        // Accessor: POSITION (offset 0 in stride)
        let pos_acc = root.push(json::Accessor {
            buffer_view: Some(vtx_view_idx),
            byte_offset: Some(USize64::from(0u64)),
            count: USize64::from(vertex_count),
            component_type: Checked::Valid(GenericComponentType(ComponentType::F32)),
            type_: Checked::Valid(Type::Vec3),
            min: Some(pos_min),
            max: Some(pos_max),
            name: None,
            normalized: false,
            sparse: None,
            extensions: None,
            extras: Default::default(),
        });

        // Accessor: NORMAL (offset 12)
        let norm_acc = root.push(json::Accessor {
            buffer_view: Some(vtx_view_idx),
            byte_offset: Some(USize64::from(12u64)),
            count: USize64::from(vertex_count),
            component_type: Checked::Valid(GenericComponentType(ComponentType::F32)),
            type_: Checked::Valid(Type::Vec3),
            min: None,
            max: None,
            name: None,
            normalized: false,
            sparse: None,
            extensions: None,
            extras: Default::default(),
        });

        // Accessor: TANGENT (offset 24)
        let tang_acc = root.push(json::Accessor {
            buffer_view: Some(vtx_view_idx),
            byte_offset: Some(USize64::from(24u64)),
            count: USize64::from(vertex_count),
            component_type: Checked::Valid(GenericComponentType(ComponentType::F32)),
            type_: Checked::Valid(Type::Vec4),
            min: None,
            max: None,
            name: None,
            normalized: false,
            sparse: None,
            extensions: None,
            extras: Default::default(),
        });

        // Accessor: TEXCOORD_0 (offset 40)
        let uv_acc = root.push(json::Accessor {
            buffer_view: Some(vtx_view_idx),
            byte_offset: Some(USize64::from(40u64)),
            count: USize64::from(vertex_count),
            component_type: Checked::Valid(GenericComponentType(ComponentType::F32)),
            type_: Checked::Valid(Type::Vec2),
            min: None,
            max: None,
            name: None,
            normalized: false,
            sparse: None,
            extensions: None,
            extras: Default::default(),
        });

        // Accessor: indices
        let idx_acc = root.push(json::Accessor {
            buffer_view: Some(idx_view_idx),
            byte_offset: Some(USize64::from(0u64)),
            count: USize64::from(mesh.indices.len()),
            component_type: Checked::Valid(GenericComponentType(ComponentType::U32)),
            type_: Checked::Valid(Type::Scalar),
            min: None,
            max: None,
            name: None,
            normalized: false,
            sparse: None,
            extensions: None,
            extras: Default::default(),
        });

        // Shared vertex attributes for all primitives in this mesh
        let mut base_attributes = BTreeMap::new();
        base_attributes.insert(Checked::Valid(Semantic::Positions), pos_acc);
        base_attributes.insert(Checked::Valid(Semantic::Normals), norm_acc);
        base_attributes.insert(Checked::Valid(Semantic::Tangents), tang_acc);
        base_attributes.insert(Checked::Valid(Semantic::TexCoords(0)), uv_acc);

        let primitives = if mesh.submeshes.is_empty() {
            // Single primitive using full index buffer (legacy path)
            let mat_data = mesh.materials.first().cloned().unwrap_or_default();
            let mat_idx = root.push(material_from_data(&mat_data));

            vec![Primitive {
                attributes: base_attributes,
                extensions: None,
                extras: Default::default(),
                indices: Some(idx_acc),
                material: Some(mat_idx),
                mode: Checked::Valid(Mode::Triangles),
                targets: None,
            }]
        } else {
            // One primitive per submesh, each with its own index accessor
            mesh.submeshes
                .iter()
                .map(|sub| {
                    let mat_data = mesh
                        .materials
                        .get(sub.material_index)
                        .cloned()
                        .unwrap_or_default();
                    let mat_idx = root.push(material_from_data(&mat_data));

                    // Accessor into a sub-range of the shared index buffer view
                    let sub_idx_acc = root.push(json::Accessor {
                        buffer_view: Some(idx_view_idx),
                        byte_offset: Some(USize64::from(
                            sub.index_start as usize * mem::size_of::<u32>(),
                        )),
                        count: USize64::from(sub.index_count as usize),
                        component_type: Checked::Valid(GenericComponentType(ComponentType::U32)),
                        type_: Checked::Valid(Type::Scalar),
                        min: None,
                        max: None,
                        name: None,
                        normalized: false,
                        sparse: None,
                        extensions: None,
                        extras: Default::default(),
                    });

                    Primitive {
                        attributes: base_attributes.clone(),
                        extensions: None,
                        extras: Default::default(),
                        indices: Some(sub_idx_acc),
                        material: Some(mat_idx),
                        mode: Checked::Valid(Mode::Triangles),
                        targets: None,
                    }
                })
                .collect()
        };

        let mesh_name = if meshes.len() > 1 {
            Some(format!("LOD_{mesh_i}"))
        } else {
            Some("Mesh".into())
        };

        let mesh_idx = root.push(json::Mesh {
            extensions: None,
            extras: Default::default(),
            name: mesh_name.clone(),
            primitives,
            weights: None,
        });

        let node_idx = root.push(json::Node {
            mesh: Some(mesh_idx),
            name: mesh_name,
            ..Default::default()
        });
        node_indices.push(node_idx);
    }

    // Scene
    let scene_idx = root.push(json::Scene {
        extensions: None,
        extras: Default::default(),
        name: Some("Scene".into()),
        nodes: node_indices,
    });
    root.scene = Some(scene_idx);

    root
}

/// Convert a [`MaterialData`] into a glTF JSON material.
fn material_from_data(mat: &MaterialData) -> json::Material {
    use gltf::json::material::{PbrBaseColorFactor, PbrMetallicRoughness, StrengthFactor};

    json::Material {
        name: Some(mat.name.clone()),
        pbr_metallic_roughness: PbrMetallicRoughness {
            base_color_factor: PbrBaseColorFactor(mat.base_color),
            metallic_factor: StrengthFactor(mat.metallic),
            roughness_factor: StrengthFactor(mat.roughness),
            ..Default::default()
        },
        ..Default::default()
    }
}

/// Assemble the final GLB binary from the JSON root and binary buffer.
fn write_glb(root: &json::Root, bin_data: &[u8]) -> Result<Vec<u8>> {
    let json_bytes = json::serialize::to_vec(root).map_err(|e| {
        crate::ProcGenError::InvalidMesh(format!("failed to serialize glTF JSON: {e}"))
    })?;

    let glb = gltf::binary::Glb {
        header: gltf::binary::Header {
            magic: *b"glTF",
            version: 2,
            length: 0, // overwritten by to_vec()
        },
        json: Cow::Borrowed(&json_bytes),
        bin: Some(Cow::Borrowed(bin_data)),
    };

    glb.to_vec().map_err(|e| {
        crate::ProcGenError::InvalidMesh(format!("failed to write GLB container: {e}"))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{BoundingBox, MaterialData, MeshData, Vertex};
    use flint_core::Vec3;

    fn triangle_mesh() -> MeshData {
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
            materials: vec![MaterialData {
                name: "test_mat".into(),
                base_color: [0.8, 0.2, 0.1, 1.0],
                metallic: 0.0,
                roughness: 0.8,
            }],
            submeshes: vec![],
            bounding_box: BoundingBox {
                min: Vec3::new(0.0, 0.0, 0.0),
                max: Vec3::new(1.0, 1.0, 0.0),
            },
        }
    }

    #[test]
    fn glb_has_valid_header() {
        let mesh = triangle_mesh();
        let glb_bytes = mesh_to_glb(&mesh).unwrap();

        // GLB magic: "glTF"
        assert_eq!(&glb_bytes[0..4], b"glTF");
        // Version 2
        assert_eq!(u32::from_le_bytes([glb_bytes[4], glb_bytes[5], glb_bytes[6], glb_bytes[7]]), 2);
        // Total length matches actual bytes
        let total_len = u32::from_le_bytes([glb_bytes[8], glb_bytes[9], glb_bytes[10], glb_bytes[11]]);
        assert_eq!(total_len as usize, glb_bytes.len());
    }

    #[test]
    fn glb_roundtrip_triangle() {
        let mesh = triangle_mesh();
        let glb_bytes = mesh_to_glb(&mesh).unwrap();

        // Parse back with the gltf crate
        let gltf = gltf::Glb::from_slice(&glb_bytes).unwrap();
        let json: json::Root = json::deserialize::from_slice(&gltf.json).unwrap();

        assert_eq!(json.meshes.len(), 1);
        assert_eq!(json.meshes[0].primitives.len(), 1);
        assert_eq!(json.nodes.len(), 1);
        assert_eq!(json.scenes.len(), 1);
        assert_eq!(json.materials.len(), 1);

        // Check vertex count via accessor
        let prim = &json.meshes[0].primitives[0];
        let pos_idx = prim.attributes[&Checked::Valid(Semantic::Positions)];
        let pos_acc = &json.accessors[pos_idx.value()];
        assert_eq!(pos_acc.count.0, 3); // 3 vertices

        // Check index count
        let idx_acc = &json.accessors[prim.indices.unwrap().value()];
        assert_eq!(idx_acc.count.0, 3); // 3 indices
    }

    #[test]
    fn glb_multi_lod() {
        let mesh_hi = triangle_mesh();
        let mesh_lo = MeshData {
            vertices: vec![
                Vertex {
                    position: [0.0, 0.0, 0.0],
                    normal: [0.0, 1.0, 0.0],
                    tangent: [1.0, 0.0, 0.0, 1.0],
                    uv: [0.0, 0.0],
                },
                Vertex {
                    position: [1.0, 0.0, 0.0],
                    normal: [0.0, 1.0, 0.0],
                    tangent: [1.0, 0.0, 0.0, 1.0],
                    uv: [1.0, 0.0],
                },
                Vertex {
                    position: [0.5, 1.0, 0.0],
                    normal: [0.0, 1.0, 0.0],
                    tangent: [1.0, 0.0, 0.0, 1.0],
                    uv: [0.5, 1.0],
                },
            ],
            indices: vec![0, 1, 2],
            materials: vec![MaterialData::default()],
            submeshes: vec![],
            bounding_box: BoundingBox {
                min: Vec3::new(0.0, 0.0, 0.0),
                max: Vec3::new(1.0, 1.0, 0.0),
            },
        };

        let glb_bytes = mesh_lods_to_glb(&[mesh_hi, mesh_lo]).unwrap();
        let gltf = gltf::Glb::from_slice(&glb_bytes).unwrap();
        let json: json::Root = json::deserialize::from_slice(&gltf.json).unwrap();

        assert_eq!(json.meshes.len(), 2);
        assert_eq!(json.nodes.len(), 2);
        assert!(json.meshes[0].name.as_deref() == Some("LOD_0"));
        assert!(json.meshes[1].name.as_deref() == Some("LOD_1"));
    }

    #[test]
    fn glb_material_preserved() {
        let mesh = triangle_mesh();
        let glb_bytes = mesh_to_glb(&mesh).unwrap();
        let gltf = gltf::Glb::from_slice(&glb_bytes).unwrap();
        let json: json::Root = json::deserialize::from_slice(&gltf.json).unwrap();

        let mat = &json.materials[0];
        assert_eq!(mat.name.as_deref(), Some("test_mat"));
        let pbr = &mat.pbr_metallic_roughness;
        assert!((pbr.base_color_factor.0[0] - 0.8).abs() < 1e-6);
        assert!((pbr.metallic_factor.0).abs() < 1e-6);
        assert!((pbr.roughness_factor.0 - 0.8).abs() < 1e-6);
    }

    #[test]
    fn glb_empty_meshes_error() {
        let result = meshes_to_glb_inner(&[]);
        assert!(result.is_err());
    }

    #[test]
    fn glb_default_material_when_none() {
        let mut mesh = triangle_mesh();
        mesh.materials.clear();
        let glb_bytes = mesh_to_glb(&mesh).unwrap();
        let gltf = gltf::Glb::from_slice(&glb_bytes).unwrap();
        let json: json::Root = json::deserialize::from_slice(&gltf.json).unwrap();
        assert_eq!(json.materials.len(), 1);
        assert_eq!(json.materials[0].name.as_deref(), Some("default"));
    }

    #[test]
    fn vertex_stride_is_48() {
        // Verify our constant matches the actual data layout
        let float_size = std::mem::size_of::<f32>();
        let expected = float_size * (3 + 3 + 4 + 2); // pos + normal + tangent + uv
        assert_eq!(VERTEX_STRIDE, expected);
    }
}
