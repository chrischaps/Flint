//! A model read back from a glTF file, in the shape the validator checks.
//!
//! [`validate_output`](crate::validate_output) checks what a generator made
//! in memory; [`validate_model`](crate::validate::validate_model) checks a
//! [`Model`], which is what a file on disk holds. `flint gen --validate`
//! reads its own GLB back through here (so an exporter bug fails the
//! checks), and `flint asset validate` reads an imported or AI-generated
//! model through the same path, so one set of rules covers every model.
//!
//! The file is read with [`flint_import::import_gltf`], the importer the
//! renderer uses, so the validator sees what the game will load.

use std::path::Path;

use flint_import::{ImportResult, ImportedMesh};

use crate::error::{ProcGenError, Result};
use crate::types::{BoundingBox, ImageData, MaterialData, MeshData, SubMesh, Vertex};

/// One mesh of a [`Model`], with what the file left out.
///
/// glTF makes normals and UVs optional, and a [`MeshData`] can't say an
/// attribute is absent, so the absences are counted here: the missing
/// vertices hold zeros in `mesh`, and the checks report them as missing
/// instead of as broken.
#[derive(Debug, Clone)]
pub struct ModelMesh {
    pub mesh: MeshData,
    /// Vertices whose primitive has no `NORMAL` attribute.
    pub normals_missing: usize,
    /// Vertices whose primitive has no `TEXCOORD_0` attribute.
    pub uvs_missing: usize,
    /// Per material of `mesh`: whether it samples a texture (base colour,
    /// metallic-roughness or normal). UVs on a textured material may tile
    /// past [0,1]; glTF samplers repeat by default.
    pub textured: Vec<bool>,
}

/// How a model's meshes relate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelLayout {
    /// One mesh: every mesh node of the file, merged.
    Single,
    /// One mesh per level of detail (`LOD_0`, `LOD_1`, ... as
    /// [`mesh_lods_to_glb`](crate::mesh_lods_to_glb) writes them).
    Lods,
    /// One skinned mesh.
    Skinned,
}

/// The meshes of a model file, ready for
/// [`validate_model`](crate::validate::validate_model).
#[derive(Debug, Clone)]
pub struct Model {
    pub layout: ModelLayout,
    pub meshes: Vec<ModelMesh>,
    /// glTF materials in the file (a primitive with none uses glTF's
    /// default material, which isn't counted).
    pub material_count: usize,
}

impl Model {
    /// Triangles over every mesh (every level, for a LOD chain).
    pub fn triangle_count(&self) -> usize {
        self.meshes.iter().map(|m| m.mesh.triangle_count()).sum()
    }
}

/// Read a `.glb` / `.gltf` file into a [`Model`].
pub fn read_model(path: &Path) -> Result<Model> {
    let import = flint_import::import_gltf(path).map_err(|e| {
        ProcGenError::InvalidMesh(format!("could not import {}: {e}", path.display()))
    })?;
    model_from_import(&import)
}

/// Build a [`Model`] from an import.
///
/// Mesh nodes named `LOD_0`, `LOD_1`, ... in order (two or more) are a LOD
/// chain, one mesh each. Anything else is merged into one mesh, primitives
/// becoming submeshes, so the style's triangle ceiling and palette apply
/// to the whole model. Node transforms are not applied: every check reads
/// shape (relative sliver area, unit normals, indices), not placement.
pub fn model_from_import(import: &ImportResult) -> Result<Model> {
    let materials: Vec<MaterialData> = import
        .materials
        .iter()
        .map(|m| MaterialData {
            name: m.name.clone(),
            base_color: m.base_color,
            metallic: m.metallic,
            roughness: m.roughness,
            normal_map: None,
        })
        .collect();
    let textured: Vec<bool> = import
        .materials
        .iter()
        .map(|m| {
            m.base_color_texture.is_some()
                || m.metallic_roughness_texture.is_some()
                || m.normal_texture.is_some()
        })
        .collect();

    // Primitives grouped by the node that holds them, in file order.
    let nodes: Vec<(&str, Vec<&ImportedMesh>)> = import
        .nodes
        .iter()
        .filter(|n| !n.mesh_primitive_indices.is_empty())
        .map(|n| {
            let prims = n
                .mesh_primitive_indices
                .iter()
                .filter_map(|&i| import.meshes.get(i))
                .collect();
            (n.name.as_str(), prims)
        })
        .collect();
    if nodes.is_empty() {
        return Err(ProcGenError::InvalidMesh("the file holds no mesh".into()));
    }

    let is_lod_chain = nodes.len() > 1
        && nodes
            .iter()
            .enumerate()
            .all(|(i, (name, _))| *name == format!("LOD_{i}"));
    let skinned = import.meshes.iter().any(|m| m.skin_index.is_some());

    let (layout, meshes) = if is_lod_chain {
        let meshes = nodes
            .iter()
            .map(|(_, prims)| merge(prims, &materials, &textured))
            .collect();
        (ModelLayout::Lods, meshes)
    } else {
        let prims: Vec<&ImportedMesh> = nodes.into_iter().flat_map(|(_, p)| p).collect();
        let layout = if skinned {
            ModelLayout::Skinned
        } else {
            ModelLayout::Single
        };
        (layout, vec![merge(&prims, &materials, &textured)])
    };

    Ok(Model {
        layout,
        meshes,
        material_count: import.materials.len(),
    })
}

/// glTF's default material, for a primitive that names none: white, fully
/// metallic, fully rough (glTF 2.0 §3.9.6).
fn gltf_default_material() -> MaterialData {
    MaterialData {
        name: "default (glTF)".into(),
        base_color: [1.0; 4],
        metallic: 1.0,
        roughness: 1.0,
        normal_map: None,
    }
}

/// Merge primitives into one mesh, a submesh each. Primitives that share a
/// vertex buffer (the exporter's submeshes do) share it here too, so the
/// vertex count matches the mesh that was written.
fn merge(prims: &[&ImportedMesh], materials: &[MaterialData], textured: &[bool]) -> ModelMesh {
    let mut out = ModelMesh {
        mesh: MeshData {
            vertices: Vec::new(),
            indices: Vec::new(),
            materials: Vec::new(),
            submeshes: Vec::new(),
            bounding_box: BoundingBox::from_positions(&[]),
        },
        normals_missing: 0,
        uvs_missing: 0,
        textured: Vec::new(),
    };
    // The file's material index (None = glTF default) for each slot.
    let mut slots: Vec<Option<usize>> = Vec::new();
    let mut previous: Option<(&ImportedMesh, u32)> = None;

    for prim in prims {
        let base = match previous {
            Some((p, base)) if same_vertices(p, prim) => base,
            _ => {
                let base = out.mesh.vertices.len() as u32;
                append_vertices(&mut out, prim);
                base
            }
        };
        previous = Some((prim, base));

        let slot = match slots.iter().position(|s| *s == prim.material_index) {
            Some(slot) => slot,
            None => {
                slots.push(prim.material_index);
                let (mat, tex) = match prim.material_index.and_then(|i| materials.get(i)) {
                    Some(m) => (m.clone(), textured[prim.material_index.unwrap()]),
                    None => (gltf_default_material(), false),
                };
                out.mesh.materials.push(mat);
                out.textured.push(tex);
                slots.len() - 1
            }
        };

        let index_start = out.mesh.indices.len() as u32;
        if prim.indices.is_empty() {
            // Non-indexed: every three vertices are a triangle.
            out.mesh
                .indices
                .extend((0..prim.positions.len() as u32).map(|i| base + i));
        } else {
            out.mesh
                .indices
                .extend(prim.indices.iter().map(|&i| base.saturating_add(i)));
        }
        out.mesh.submeshes.push(SubMesh {
            index_start,
            index_count: out.mesh.indices.len() as u32 - index_start,
            material_index: slot,
        });
    }

    out.mesh.recompute_bounds();
    out
}

fn same_vertices(a: &ImportedMesh, b: &ImportedMesh) -> bool {
    a.positions == b.positions && a.normals == b.normals && a.uvs == b.uvs
}

fn append_vertices(out: &mut ModelMesh, prim: &ImportedMesh) {
    let n = prim.positions.len();
    let has_normals = prim.normals.len() == n;
    let has_uvs = prim.uvs.len() == n;
    if !has_normals {
        out.normals_missing += n;
    }
    if !has_uvs {
        out.uvs_missing += n;
    }
    out.mesh.vertices.extend(
        prim.positions
            .iter()
            .enumerate()
            .map(|(i, &position)| Vertex {
                position,
                normal: if has_normals {
                    prim.normals[i]
                } else {
                    [0.0; 3]
                },
                tangent: [0.0; 4],
                uv: if has_uvs { prim.uvs[i] } else { [0.0; 2] },
            }),
    );
}

/// Read a PNG back into an [`ImageData`] (RGBA8), so a written texture can
/// be validated as it lies on disk. The file can't say what the image
/// means, so the caller names its `semantics`.
pub fn read_png(path: &Path, semantics: crate::ChannelSemantics) -> Result<ImageData> {
    let bytes = flint_core::vfs::read(path)?;
    let img = image::load_from_memory(&bytes)
        .map_err(|e| ProcGenError::ImageError(format!("could not read {}: {e}", path.display())))?
        .into_rgba8();
    Ok(ImageData {
        width: img.width(),
        height: img.height(),
        pixels: img.into_raw(),
        channel_semantics: semantics,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::generator::Generator;
    use crate::output::GeneratorOutput;
    use crate::rng::SeededRng;
    use crate::spec::ProcGenSpec;
    use crate::validate::{validate_model, validate_output, ValidationConstraints};

    fn lines(report: &crate::ValidationReport) -> Vec<String> {
        report
            .checks
            .iter()
            .map(|c| format!("[{}] {}: {}", c.status, c.name, c.detail))
            .collect()
    }

    /// The GLB the exporter writes validates exactly as the output it was
    /// written from: every check, status and detail. A difference is an
    /// exporter (or importer) bug.
    fn assert_round_trip(output: &GeneratorOutput, spec: &ProcGenSpec, name: &str) -> Model {
        let glb = match output {
            GeneratorOutput::Mesh(m) => crate::mesh_to_glb(m),
            GeneratorOutput::MeshWithLods(l) => crate::mesh_lods_to_glb(l),
            GeneratorOutput::SkinnedMesh(m) => crate::skinned_mesh_to_glb(m),
            _ => unreachable!(),
        }
        .unwrap();
        let dir = std::env::temp_dir().join(format!("flint_model_{name}_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(format!("{name}.glb"));
        std::fs::write(&path, glb).unwrap();
        let model = read_model(&path).unwrap();
        std::fs::remove_dir_all(&dir).ok();

        let constraints = ValidationConstraints {
            palette: vec!["#5a3d22".into(), "#4f6b2a".into()],
            roughness_range: Some([0.5, 1.0]),
            ..Default::default()
        };
        let memory = validate_output(output, spec, &constraints);
        let disk = validate_model(&model, Some(spec), &constraints);
        assert_eq!(lines(&disk), lines(&memory));
        model
    }

    #[test]
    fn the_oak_validates_the_same_from_disk() {
        let spec =
            ProcGenSpec::parse(include_str!("../../../specs/oak_tree.procgen.toml")).unwrap();
        let output = crate::TreeGenerator
            .generate(&spec, &mut SeededRng::new(42))
            .unwrap();
        let lods = output.as_mesh_lods().unwrap().len();
        let model = assert_round_trip(&output, &spec, "oak");
        assert_eq!(model.layout, ModelLayout::Lods);
        assert_eq!(model.meshes.len(), lods);
        // The bark's normal map comes back as a texture, so its UVs tile.
        assert!(model.meshes[0].textured.iter().any(|&t| t));
    }

    #[test]
    fn the_beetle_validates_the_same_from_disk() {
        let spec = ProcGenSpec::parse(include_str!("../../../specs/beetle.procgen.toml")).unwrap();
        let output = crate::CreatureGenerator
            .generate(&spec, &mut SeededRng::new(7))
            .unwrap();
        let model = assert_round_trip(&output, &spec, "beetle");
        assert_eq!(model.layout, ModelLayout::Skinned);
    }
}
