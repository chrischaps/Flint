//! glTF/GLB file importer

use crate::types::{ImportResult, ImportedMaterial, ImportedMesh, ImportedTexture};
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

    let mut meshes = Vec::new();
    let mut total_vertices = 0u64;

    for mesh in document.meshes() {
        let mesh_name = mesh
            .name()
            .map(String::from)
            .unwrap_or_else(|| format!("mesh_{}", mesh.index()));

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

            total_vertices += positions.len() as u64;

            meshes.push(ImportedMesh {
                name: mesh_name.clone(),
                positions,
                normals,
                uvs,
                indices,
                material_index,
            });
        }
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

        materials.push(ImportedMaterial {
            name: mat_name,
            base_color,
            metallic,
            roughness,
            base_color_texture,
            normal_texture,
            metallic_roughness_texture,
        });
    }

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
    })
}
