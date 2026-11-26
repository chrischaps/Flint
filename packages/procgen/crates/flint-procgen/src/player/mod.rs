//! The procgen package's part of the player (#1557): what flint-player
//! did behind its `procgen` feature, built against the Package API
//! (`flint-package`) instead, so the player names none of it.
//!
//! - [`PACKAGE`]: what a launcher lists. It puts the run's
//!   [`ProcGenResolver`](crate::ProcGenResolver) in the resources and
//!   registers the [`AssetMaker`](flint_package::assets::AssetMaker) the
//!   kernel calls at a scene's load, for spawned entities, each frame and
//!   at a teardown, and the [`LoadSteps`] point another package's load
//!   step goes in (living's props).
//! - [`AssetSink`], [`GpuSink`], the resolution order (catalog > procgen >
//!   file > placeholder) and the upload of what a generator made.
//! - [`resolver`] / [`resolver_mut`] / [`generators_mut`]: the run's
//!   resolver in a context's resources, for living's code and a game's
//!   generators (ADR 0119).

mod assets;
mod maker;
pub mod recorder;

pub use assets::{
    apply_completed, apply_failures, collect_requests, collect_requests_of, discover_specs,
    image_set_name, resolve, upload_output, AssetRequest, AssetSink, GpuSink, Outcome, ProcGenMode,
    Recording, PLACEHOLDER_RGBA, PLACEHOLDER_TEXEL,
};
pub use maker::{
    disk_config, generators_mut, resolver, resolver_mut, use_disk, LoadCtx, LoadStep, LoadSteps,
    Procgen, PACKAGE,
};

use crate::LivingRig;
use flint_import::{AlphaMode, ImportResult, ImportedMaterial, ImportedMesh, ImportedTexture};

/// A rig's rest mesh as a model import, split by sub-mesh, so a procgen skeleton
/// goes up the same path as a skinned GLB. Each part keeps only the vertices it
/// uses, not the whole shared buffer: a tree has six parts.
pub fn import_of(name: &str, rig: &LivingRig) -> ImportResult {
    let mesh = &rig.mesh;
    let mut result = ImportResult {
        asset_meta: flint_asset::AssetMeta {
            name: name.to_string(),
            asset_type: flint_asset::AssetType::Mesh,
            hash: String::new(),
            source_path: None,
            format: None,
            properties: Default::default(),
            tags: Vec::new(),
        },
        meshes: Vec::with_capacity(mesh.submeshes.len()),
        textures: Vec::new(),
        materials: Vec::with_capacity(mesh.submeshes.len()),
        skeletons: Vec::new(),
        skeletal_clips: Vec::new(),
        node_clips: Vec::new(),
        nodes: Vec::new(),
        root_nodes: Vec::new(),
    };
    let fallback = crate::MaterialData::default();
    for (i, sub) in mesh.submeshes.iter().enumerate() {
        let start = (sub.index_start as usize).min(mesh.indices.len());
        let end = (start + sub.index_count as usize).min(mesh.indices.len());
        let indices = &mesh.indices[start..end];
        let (Some(lo), Some(hi)) = (indices.iter().min(), indices.iter().max()) else {
            continue;
        };
        let span = *lo as usize..=*hi as usize;
        let vertices = &mesh.vertices[span.clone()];
        let source = mesh.materials.get(sub.material_index).unwrap_or(&fallback);

        let mut material = ImportedMaterial {
            name: source.name.clone(),
            base_color: source.base_color,
            metallic: source.metallic,
            roughness: source.roughness,
            use_vertex_color: true,
            alpha_mode: AlphaMode::Opaque,
            alpha_cutoff: 0.5,
            ..Default::default()
        };
        if let Some(normal) = &source.normal_map {
            let texture = format!("part{i}_normal");
            result.textures.push(ImportedTexture {
                name: texture.clone(),
                width: normal.width,
                height: normal.height,
                format: "rgba8".to_string(),
                data: normal.pixels.clone(),
            });
            material.normal_texture = Some(texture);
        }
        result.materials.push(material);

        result.meshes.push(ImportedMesh {
            name: format!("{name}_part{i}"),
            positions: vertices.iter().map(|v| v.position).collect(),
            normals: vertices.iter().map(|v| v.normal).collect(),
            uvs: vertices.iter().map(|v| v.uv).collect(),
            indices: indices.iter().map(|ix| ix - lo).collect(),
            material_index: Some(result.materials.len() - 1),
            joint_indices: Some(span.clone().map(|v| rig.runtime_joints(v).0).collect()),
            joint_weights: Some(span.map(|v| rig.runtime_joints(v).1).collect()),
            skin_index: Some(0),
            lod: None,
        });
    }
    result
}

#[cfg(test)]
mod tests {
    /// The renderer's bone limit, which a living rig's palette must fit:
    /// held here, where the package's player side sees both (#1557; it was
    /// flint-cli's `preview.rs` test).
    #[test]
    fn procgen_knows_the_renderers_bone_limit() {
        assert_eq!(crate::MAX_SKINNED_BONES, flint_render::MAX_BONES);
    }
}
