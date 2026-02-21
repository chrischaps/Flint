//! glTF/GLB file importer

use crate::types::{
    AlphaMode, ImportResult, ImportedChannel, ImportedJoint, ImportedKeyframe, ImportedMaterial,
    ImportedMesh, ImportedNode, ImportedSkeleton, ImportedSkeletalClip, ImportedTexture,
    JointProperty,
};
use flint_asset::{AssetMeta, AssetType};
use flint_core::{ContentHash, FlintError, Result};
use std::collections::HashMap;
use std::path::Path;

/// Import a glTF or GLB file
pub fn import_gltf<P: AsRef<Path>>(path: P) -> Result<ImportResult> {
    let path = path.as_ref();
    let (document, buffers, images) = gltf::import(path).map_err(|e| {
        FlintError::ImportError(format!("Failed to import glTF: {}", e))
    })?;

    let hash = ContentHash::from_file(path)
        .map_err(|e| FlintError::ImportError(format!("Failed to hash file: {}", e)))?;

    let file_name = path
        .file_stem()
        .and_then(|n| n.to_str())
        .unwrap_or("unnamed")
        .to_string();

    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("glb")
        .to_string();

    // Build a map from glTF node index -> skin index for mesh-skin association
    let mut node_skin_map: HashMap<usize, usize> = HashMap::new();
    for node in document.nodes() {
        if let Some(skin) = node.skin() {
            // Associate the mesh node with its skin
            if node.mesh().is_some() {
                node_skin_map.insert(node.mesh().unwrap().index(), skin.index());
            }
        }
    }

    let mut meshes = Vec::new();
    let mut total_vertices = 0u64;
    let mut imported_nodes: Vec<ImportedNode> = Vec::new();
    let mut root_nodes: Vec<usize> = Vec::new();

    // Track which glTF node index maps to which ImportedNode index
    let mut gltf_node_to_imported: HashMap<usize, usize> = HashMap::new();

    // Walk the scene graph via scenes → root nodes → recursive children.
    // This extracts meshes per-node (preserving transforms) instead of per-mesh.
    let scene_root_nodes: Vec<gltf::Node> = document
        .scenes()
        .flat_map(|scene| scene.nodes())
        .collect();

    // Recursive helper: extract a node and all its children
    fn walk_node(
        node: &gltf::Node,
        buffers: &[gltf::buffer::Data],
        node_skin_map: &HashMap<usize, usize>,
        meshes: &mut Vec<ImportedMesh>,
        imported_nodes: &mut Vec<ImportedNode>,
        gltf_node_to_imported: &mut HashMap<usize, usize>,
        total_vertices: &mut u64,
    ) -> usize {
        let (translation, rotation, scale) = node.transform().decomposed();

        let node_name = node
            .name()
            .map(String::from)
            .unwrap_or_else(|| format!("node_{}", node.index()));

        let skin_index = if node.skin().is_some() {
            node.skin().map(|s| s.index())
        } else {
            // Check if this node's mesh is associated with a skin via another node
            node.mesh()
                .and_then(|m| node_skin_map.get(&m.index()).copied())
        };

        // Extract mesh primitives if this node has a mesh
        let mut mesh_primitive_indices = Vec::new();
        if let Some(mesh) = node.mesh() {
            let mesh_name = mesh
                .name()
                .map(String::from)
                .unwrap_or_else(|| node_name.clone());

            for primitive in mesh.primitives() {
                let reader = primitive.reader(|buffer| Some(&buffers[buffer.index()]));

                let positions: Vec<[f32; 3]> = reader
                    .read_positions()
                    .map(|iter| iter.collect())
                    .unwrap_or_default();

                let normals: Vec<[f32; 3]> = reader
                    .read_normals()
                    .map(|iter| iter.collect())
                    .unwrap_or_default();

                let uvs: Vec<[f32; 2]> = reader
                    .read_tex_coords(0)
                    .map(|iter| iter.into_f32().collect())
                    .unwrap_or_default();

                let indices: Vec<u32> = reader
                    .read_indices()
                    .map(|iter| iter.into_u32().collect())
                    .unwrap_or_default();

                let material_index = primitive.material().index();

                let joint_indices: Option<Vec<[u16; 4]>> = reader
                    .read_joints(0)
                    .map(|iter| iter.into_u16().collect());

                let joint_weights: Option<Vec<[f32; 4]>> = reader
                    .read_weights(0)
                    .map(|iter| iter.into_f32().collect());

                *total_vertices += positions.len() as u64;

                let prim_index = meshes.len();
                meshes.push(ImportedMesh {
                    name: mesh_name.clone(),
                    positions,
                    normals,
                    uvs,
                    indices,
                    material_index,
                    joint_indices,
                    joint_weights,
                    skin_index,
                });
                mesh_primitive_indices.push(prim_index);
            }
        }

        // Reserve this node's index
        let node_index = imported_nodes.len();
        gltf_node_to_imported.insert(node.index(), node_index);
        imported_nodes.push(ImportedNode {
            name: node_name,
            translation,
            rotation,
            scale,
            mesh_primitive_indices,
            children: Vec::new(), // filled in below
            skin_index,
        });

        // Recurse into children
        let child_indices: Vec<usize> = node
            .children()
            .map(|child| {
                walk_node(
                    &child,
                    buffers,
                    node_skin_map,
                    meshes,
                    imported_nodes,
                    gltf_node_to_imported,
                    total_vertices,
                )
            })
            .collect();

        imported_nodes[node_index].children = child_indices;

        node_index
    }

    for root_node in &scene_root_nodes {
        let idx = walk_node(
            root_node,
            &buffers,
            &node_skin_map,
            &mut meshes,
            &mut imported_nodes,
            &mut gltf_node_to_imported,
            &mut total_vertices,
        );
        root_nodes.push(idx);
    }

    let mut textures = Vec::new();
    for (i, image) in images.iter().enumerate() {
        let tex_name = document
            .textures()
            .nth(i)
            .and_then(|t| t.name().map(String::from))
            .unwrap_or_else(|| format!("texture_{}", i));

        let format = match image.format {
            gltf::image::Format::R8 => "r8",
            gltf::image::Format::R8G8 => "rg8",
            gltf::image::Format::R8G8B8 => "rgb8",
            gltf::image::Format::R8G8B8A8 => "rgba8",
            gltf::image::Format::R16 => "r16",
            gltf::image::Format::R16G16 => "rg16",
            gltf::image::Format::R16G16B16 => "rgb16",
            gltf::image::Format::R16G16B16A16 => "rgba16",
            gltf::image::Format::R32G32B32FLOAT => "rgb32f",
            gltf::image::Format::R32G32B32A32FLOAT => "rgba32f",
        };

        textures.push(ImportedTexture {
            name: tex_name,
            width: image.width,
            height: image.height,
            format: format.to_string(),
            data: image.pixels.clone(),
        });
    }

    let mut materials = Vec::new();
    for material in document.materials() {
        let mat_name = material
            .name()
            .map(String::from)
            .unwrap_or_else(|| format!("material_{}", material.index().unwrap_or(0)));

        let pbr = material.pbr_metallic_roughness();
        let base_color = pbr.base_color_factor();
        let metallic = pbr.metallic_factor();
        let roughness = pbr.roughness_factor();

        let base_color_texture = pbr
            .base_color_texture()
            .map(|info| {
                info.texture()
                    .name()
                    .map(String::from)
                    .unwrap_or_else(|| format!("texture_{}", info.texture().index()))
            });

        let normal_texture = material
            .normal_texture()
            .map(|info| {
                info.texture()
                    .name()
                    .map(String::from)
                    .unwrap_or_else(|| format!("texture_{}", info.texture().index()))
            });

        let metallic_roughness_texture = pbr
            .metallic_roughness_texture()
            .map(|info| {
                info.texture()
                    .name()
                    .map(String::from)
                    .unwrap_or_else(|| format!("texture_{}", info.texture().index()))
            });

        let alpha_mode = match material.alpha_mode() {
            gltf::material::AlphaMode::Opaque => AlphaMode::Opaque,
            gltf::material::AlphaMode::Mask => AlphaMode::Mask,
            gltf::material::AlphaMode::Blend => AlphaMode::Blend,
        };
        let alpha_cutoff = material.alpha_cutoff().unwrap_or(0.5);

        materials.push(ImportedMaterial {
            name: mat_name,
            base_color,
            metallic,
            roughness,
            base_color_texture,
            normal_texture,
            metallic_roughness_texture,
            use_vertex_color: false,
            alpha_mode,
            alpha_cutoff,
        });
    }

    // --- Extract skeletons (skins) ---
    let skeletons = extract_skeletons(&document, &buffers);

    // --- Extract skeletal animation clips ---
    let skeletal_clips = extract_skeletal_clips(&document, &buffers, &skeletons);

    let mut properties = HashMap::new();
    properties.insert(
        "vertex_count".to_string(),
        toml::Value::Integer(total_vertices as i64),
    );
    properties.insert(
        "mesh_count".to_string(),
        toml::Value::Integer(meshes.len() as i64),
    );
    properties.insert(
        "material_count".to_string(),
        toml::Value::Integer(materials.len() as i64),
    );
    if !skeletons.is_empty() {
        properties.insert(
            "skeleton_count".to_string(),
            toml::Value::Integer(skeletons.len() as i64),
        );
    }
    if !skeletal_clips.is_empty() {
        properties.insert(
            "skeletal_clip_count".to_string(),
            toml::Value::Integer(skeletal_clips.len() as i64),
        );
    }

    let asset_meta = AssetMeta {
        name: file_name,
        asset_type: AssetType::Mesh,
        hash: hash.to_prefixed_hex(),
        source_path: Some(path.to_string_lossy().to_string()),
        format: Some(ext),
        properties,
        tags: vec![],
    };

    Ok(ImportResult {
        asset_meta,
        meshes,
        textures,
        materials,
        skeletons,
        skeletal_clips,
        nodes: imported_nodes,
        root_nodes,
    })
}

/// Extract skeleton data from glTF skins
fn extract_skeletons(
    document: &gltf::Document,
    buffers: &[gltf::buffer::Data],
) -> Vec<ImportedSkeleton> {
    let mut skeletons = Vec::new();

    for skin in document.skins() {
        let skin_name = skin
            .name()
            .map(String::from)
            .unwrap_or_else(|| format!("skin_{}", skin.index()));

        let joint_nodes: Vec<gltf::Node> = skin.joints().collect();

        // Build a map from glTF node index -> joint index within this skin
        let mut node_to_joint: HashMap<usize, usize> = HashMap::new();
        for (joint_idx, node) in joint_nodes.iter().enumerate() {
            node_to_joint.insert(node.index(), joint_idx);
        }

        // Read inverse bind matrices
        let reader = skin.reader(|buf| Some(&buffers[buf.index()]));
        let ibms: Vec<[[f32; 4]; 4]> = reader
            .read_inverse_bind_matrices()
            .map(|iter| iter.collect())
            .unwrap_or_else(|| vec![identity_4x4(); joint_nodes.len()]);

        // Build joint hierarchy
        let mut joints = Vec::with_capacity(joint_nodes.len());
        for (joint_idx, node) in joint_nodes.iter().enumerate() {
            let joint_name = node
                .name()
                .map(String::from)
                .unwrap_or_else(|| format!("joint_{}", joint_idx));

            // Find parent: walk up the glTF node tree and check if the parent is also a joint
            let parent = find_parent_joint(document, node.index(), &node_to_joint);

            let ibm = ibms.get(joint_idx).copied().unwrap_or_else(identity_4x4);

            joints.push(ImportedJoint {
                name: joint_name,
                index: joint_idx,
                parent,
                inverse_bind_matrix: ibm,
            });
        }

        skeletons.push(ImportedSkeleton {
            name: skin_name,
            joints,
        });
    }

    skeletons
}

/// Find the parent joint index for a given node, if the parent is within the skin's joint set
fn find_parent_joint(
    document: &gltf::Document,
    node_index: usize,
    node_to_joint: &HashMap<usize, usize>,
) -> Option<usize> {
    // Walk all nodes to find the parent of node_index
    for node in document.nodes() {
        for child in node.children() {
            if child.index() == node_index {
                // Found the parent node — check if it's a joint
                return node_to_joint.get(&node.index()).copied();
            }
        }
    }
    None
}

/// Extract skeletal animation clips from glTF animations
fn extract_skeletal_clips(
    document: &gltf::Document,
    buffers: &[gltf::buffer::Data],
    skeletons: &[ImportedSkeleton],
) -> Vec<ImportedSkeletalClip> {
    if skeletons.is_empty() {
        return Vec::new();
    }

    // Build a map from glTF node index -> (skeleton_index, joint_index)
    // We rebuild from skeletons since each skeleton was built from skin joints
    let mut node_to_skeleton_joint: HashMap<usize, (usize, usize)> = HashMap::new();
    for skin in document.skins() {
        let skel_idx = skin.index();
        if skel_idx >= skeletons.len() {
            continue;
        }
        for (joint_idx, node) in skin.joints().enumerate() {
            node_to_skeleton_joint.insert(node.index(), (skel_idx, joint_idx));
        }
    }

    let mut clips = Vec::new();

    for animation in document.animations() {
        let clip_name = animation
            .name()
            .map(String::from)
            .unwrap_or_else(|| format!("clip_{}", animation.index()));

        let mut channels = Vec::new();
        let mut max_time: f32 = 0.0;

        for channel in animation.channels() {
            let target_node = channel.target().node().index();

            // Only process channels that target skeleton joints
            let Some(&(_skel_idx, joint_idx)) = node_to_skeleton_joint.get(&target_node) else {
                continue;
            };

            let property = match channel.target().property() {
                gltf::animation::Property::Translation => JointProperty::Translation,
                gltf::animation::Property::Rotation => JointProperty::Rotation,
                gltf::animation::Property::Scale => JointProperty::Scale,
                _ => continue, // Skip morph targets etc.
            };

            let reader = channel.reader(|buf| Some(&buffers[buf.index()]));

            let timestamps: Vec<f32> = reader
                .read_inputs()
                .map(|iter| iter.collect())
                .unwrap_or_default();

            let interpolation = match channel.sampler().interpolation() {
                gltf::animation::Interpolation::Step => "STEP",
                gltf::animation::Interpolation::Linear => "LINEAR",
                gltf::animation::Interpolation::CubicSpline => "CUBICSPLINE",
            };

            let outputs: Vec<Vec<f32>> = match &property {
                JointProperty::Translation | JointProperty::Scale => {
                    reader
                        .read_outputs()
                        .map(|out| match out {
                            gltf::animation::util::ReadOutputs::Translations(iter) => {
                                iter.map(|v| vec![v[0], v[1], v[2]]).collect()
                            }
                            gltf::animation::util::ReadOutputs::Scales(iter) => {
                                iter.map(|v| vec![v[0], v[1], v[2]]).collect()
                            }
                            _ => Vec::new(),
                        })
                        .unwrap_or_default()
                }
                JointProperty::Rotation => {
                    reader
                        .read_outputs()
                        .map(|out| match out {
                            gltf::animation::util::ReadOutputs::Rotations(iter) => {
                                iter.into_f32()
                                    .map(|v| vec![v[0], v[1], v[2], v[3]])
                                    .collect()
                            }
                            _ => Vec::new(),
                        })
                        .unwrap_or_default()
                }
            };

            let keyframes: Vec<ImportedKeyframe> = timestamps
                .iter()
                .zip(outputs.iter())
                .map(|(&time, value)| {
                    if time > max_time {
                        max_time = time;
                    }
                    ImportedKeyframe {
                        time,
                        value: value.clone(),
                    }
                })
                .collect();

            channels.push(ImportedChannel {
                joint_index: joint_idx,
                property,
                interpolation: interpolation.to_string(),
                keyframes,
            });
        }

        if !channels.is_empty() {
            clips.push(ImportedSkeletalClip {
                name: clip_name,
                duration: max_time,
                channels,
            });
        }
    }

    clips
}

fn identity_4x4() -> [[f32; 4]; 4] {
    [
        [1.0, 0.0, 0.0, 0.0],
        [0.0, 1.0, 0.0, 0.0],
        [0.0, 0.0, 1.0, 0.0],
        [0.0, 0.0, 0.0, 1.0],
    ]
}

