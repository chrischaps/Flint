//! Living props in the player (#211): a scene's `living_prop` components become
//! skinned meshes, each with its own bones, and rings of poses the resolver
//! keeps full.
//!
//! Each prop's `model.asset` is rewritten at load from the spec's name to its
//! lineage's mesh, `<spec>@<lineage seed>`, and marked `runtime`: the file
//! loader and the plain procgen path leave it alone from then on. An entity
//! whose spec can't live keeps its model and takes the plain path, as a still
//! tree, with a warning saying why.
//!
//! The mesh goes up through the same `load_skinned_model` a skinned GLB does,
//! and each prop's bones through `update_bone_matrices`, so each one has its
//! own bone buffer. The buffer is the prop's skeleton instance: a pose reaches
//! it as a whole palette from [`LivingRig::palette`], which holds scale the
//! animation system's joint hierarchy can't, so props don't register a
//! `Skeleton` there, and cost nothing per frame while they hold still.

use flint_core::components as comp;
use flint_core::toml_util::toml_f32;
use flint_ecs::FlintWorld;
use flint_import::{AlphaMode, ImportResult, ImportedMaterial, ImportedMesh, ImportedTexture};
use flint_procgen::{
    AssetKind, LineageDone, LivingPropRequest, LivingRig, LivingStats, ProcGenResolver, Stagger,
};
use flint_render::model_loader::ModelLoadConfig;

use super::procgen_assets::{AssetSink, ProcGenMode};

/// Read an entity's `living_prop` component into a request for `spec_name`.
/// A bad field is named in the error, rather than quietly meaning its default.
pub fn read_request(
    entity: flint_core::EntityId,
    entity_name: &str,
    spec_name: &str,
    component: &toml::Value,
) -> Result<LivingPropRequest, String> {
    let lineage_seed = match component.get("lineage_seed") {
        None => None,
        Some(v) => Some(
            v.as_integer()
                .and_then(|seed| u64::try_from(seed).ok())
                .ok_or_else(|| format!("lineage_seed {v} isn't a seed: it must be 0 or more"))?,
        ),
    };
    let pose_ring = match component.get("pose_ring") {
        None => None,
        Some(v) => Some(
            v.as_integer()
                .and_then(|ring| u32::try_from(ring).ok())
                .filter(|ring| *ring >= 2)
                .ok_or_else(|| {
                    format!("pose_ring {v} is too small to blend: it must be 2 or more")
                })?,
        ),
    };
    let phase_offset = match component.get("phase_offset") {
        None => None,
        Some(v) => Some(
            toml_f32(v)
                .filter(|phase| phase.is_finite())
                .ok_or_else(|| format!("phase_offset {v} isn't a number"))?,
        ),
    };
    let stagger = match component.get("stagger") {
        None => None,
        Some(v) => Some(v.clone().try_into::<Stagger>().map_err(|_| {
            format!("stagger {v} isn't one of \"none\", \"per_level\", \"per_branch\"")
        })?),
    };
    Ok(LivingPropRequest {
        entity,
        entity_name: entity_name.to_string(),
        spec_name: spec_name.to_string(),
        lineage_seed,
        pose_ring,
        phase_offset,
        stagger,
    })
}

/// Turn the world's `living_prop` entities into living props: register each
/// with the resolver, point its model at its lineage's mesh, and either build
/// everything now or queue it behind placeholders.
pub fn resolve(
    world: &mut FlintWorld,
    config: &ModelLoadConfig,
    resolver: &mut ProcGenResolver,
    sink: &mut dyn AssetSink,
    mode: ProcGenMode,
) {
    // Not a single scan per frame: this is load, and it's all the cost a scene
    // with no living props pays.
    let mut assets: Vec<(flint_core::EntityId, String)> = Vec::new();
    for entity in world.all_entities() {
        let Some(comps) = world.get_components(entity.id) else {
            continue;
        };
        let Some(component) = comps.get(comp::LIVING_PROP) else {
            continue;
        };
        let name = &entity.name;
        let Some(spec_name) = comps
            .get(comp::MODEL)
            .and_then(|m| m.get("asset"))
            .and_then(|a| a.as_str())
        else {
            tracing::warn!("Living prop '{name}' has no model.asset naming its spec; ignoring it");
            continue;
        };
        if config.overrides.contains_key(spec_name) {
            tracing::warn!(
                "Living prop '{name}': the catalog has '{spec_name}', and a catalog entry can't \
                 live, so it stays a still model"
            );
            continue;
        }
        let added = read_request(entity.id, name, spec_name, component)
            .and_then(|request| resolver.add_living_prop(request));
        match added {
            Ok((_, asset)) => assets.push((entity.id, asset)),
            Err(reason) => {
                tracing::warn!("Living prop '{name}' stays a still model: {reason}");
            }
        }
    }
    if assets.is_empty() {
        return;
    }

    for (id, asset) in &assets {
        let model = world
            .get_components_mut(*id)
            .and_then(|comps| comps.get_mut(comp::MODEL))
            .and_then(|model| model.as_table_mut());
        if let Some(model) = model {
            model.insert("asset".into(), toml::Value::String(asset.clone()));
            model.insert("runtime".into(), toml::Value::Boolean(true));
        }
    }

    let props = resolver.living_props();
    println!(
        "[living] {} prop(s) on {} lineage(s)",
        props.props().len(),
        props.lineage_count()
    );
    match mode {
        ProcGenMode::Immediate => {
            let started = web_time::Instant::now();
            let done = resolver.make_living_now();
            apply_lineages(&done, sink);
            let props = resolver.living_props();
            let poses: usize = props.props().iter().map(|p| p.ready()).sum();
            println!(
                "[living] {} lineage(s) and {poses} poses in {:.0} ms",
                done.len(),
                started.elapsed().as_secs_f64() * 1e3,
            );
        }
        ProcGenMode::Deferred => {
            let mut uploaded: Vec<&str> = Vec::new();
            for (_, asset) in &assets {
                if !uploaded.contains(&asset.as_str()) {
                    sink.upload_placeholder(asset, AssetKind::Model);
                    uploaded.push(asset);
                }
            }
        }
    }
}

/// Send up finished lineages: the mesh once, and rest bones for each prop. One
/// that couldn't be made leaves a placeholder under its name.
pub fn apply_lineages(done: &[LineageDone], sink: &mut dyn AssetSink) {
    for lineage in done {
        match &lineage.rig {
            Ok(rig) => {
                sink.upload_living(&lineage.asset_name, rig);
                for entity in &lineage.entities {
                    sink.upload_bones(*entity, &lineage.asset_name, &lineage.rest_palette);
                }
            }
            Err(reason) => {
                tracing::warn!(
                    "Couldn't build living lineage '{}': {reason}; showing a placeholder",
                    lineage.asset_name
                );
                sink.upload_placeholder(&lineage.asset_name, AssetKind::Model);
            }
        }
    }
}

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
    let fallback = flint_procgen::MaterialData::default();
    for (i, sub) in mesh.submeshes.iter().enumerate() {
        let start = (sub.index_start as usize).min(mesh.indices.len());
        let end = (start + sub.index_count as usize).min(mesh.indices.len());
        let indices = &mesh.indices[start..end];
        let (Some(lo), Some(hi)) = (indices.iter().min(), indices.iter().max()) else {
            continue;
        };
        let vertices = &mesh.vertices[*lo as usize..=*hi as usize];
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
            joint_indices: Some(vertices.iter().map(|v| v.joint_indices).collect()),
            joint_weights: Some(vertices.iter().map(|v| v.joint_weights).collect()),
            skin_index: Some(0),
        });
    }
    result
}

/// The `[living]` lines `--exit-after` prints, so a run can be read headlessly:
/// whether the rings stayed full and whether they kept to the budget.
pub fn report(
    stats: &LivingStats,
    prop_count: usize,
    lineages: usize,
    ring: usize,
    budget_ms: f64,
) -> [String; 2] {
    let avg = stats.total_ms / stats.frames.max(1) as f64;
    [
        format!(
            "[living] {prop_count} props on {lineages} lineages, {ring} poses a ring: {} of {} frames \
             ended full, never more than {} short, {} frames missing a pose in use",
            stats.frames_full, stats.frames, stats.most_short, stats.frames_starved,
        ),
        format!(
            "[living] {} poses made, {} from cache, {} failed: avg {avg:.2} ms/frame, worst {:.2} ms, \
             budget {budget_ms:.0} ms, over it on {} frames",
            stats.poses_made, stats.poses_from_cache, stats.pose_failures, stats.worst_ms,
            stats.frames_over_budget,
        ),
    ]
}

#[cfg(test)]
#[path = "living_props_tests.rs"]
mod tests;
