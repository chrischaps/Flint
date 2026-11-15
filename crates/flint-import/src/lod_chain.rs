//! Levels of detail written as sibling nodes (#193).
//!
//! `flint gen` writes a spec with `[[lod]]` tables as one GLB holding a mesh
//! node per level, named `LOD_0`, `LOD_1`, ... at the scene's root
//! (`flint_procgen::mesh_lods_to_glb`). Imported as they stand, the levels
//! are ordinary nodes, each expanded into a child entity, and every one of
//! them draws: the low levels' thinned, enlarged leaf cards show as a crown
//! of oversized leaves over the real tree.
//!
//! [`band_lod_chain`] recognises the chain and gives each level a
//! [`LodBand`], so exactly one draws at any distance. The file names no
//! distances, so they come from the model: every level shares one reference
//! box (the union of their bounds, which the renderer measures the camera
//! to), `LOD_0` hands over at [`FIRST_SWITCH_RADII`] times that box's
//! radius, and each later switch is where the level before it would have
//! as many triangles per pixel of screen as this one: the square root of
//! their triangle ratio further on, at least [`MIN_SWITCH_STEP`] times the
//! last switch. A chain any of whose meshes already carries an authored
//! `extras.flint_lod` is left alone.

use crate::types::{ImportedMesh, ImportedNode, LodBand, MeshBounds};

/// `LOD_0`'s far edge, in radii of the chain's shared box (half its
/// diagonal), measured from the box's surface. At 8 radii a 60° camera
/// sees the model at about a fifth of the screen's height.
pub const FIRST_SWITCH_RADII: f32 = 8.0;

/// Each later switch is at least this many times the one before it, so a
/// level that barely saves triangles still gets a band worth drawing.
pub const MIN_SWITCH_STEP: f32 = 1.5;

/// The node indices of a LOD chain at the scene's root, in level order: two
/// or more root nodes holding meshes, named exactly `LOD_0`, `LOD_1`, ...
/// Other root nodes (an empty, a light) don't break it, but a root mesh
/// node with another name does: then the file is a model of several parts.
pub fn lod_chain(nodes: &[ImportedNode], root_nodes: &[usize]) -> Option<Vec<usize>> {
    let chain: Vec<usize> = root_nodes
        .iter()
        .copied()
        .filter(|&i| {
            nodes
                .get(i)
                .is_some_and(|n| !n.mesh_primitive_indices.is_empty())
        })
        .collect();
    let in_order = chain
        .iter()
        .enumerate()
        .all(|(level, &i)| nodes[i].name == format!("LOD_{level}"));
    (chain.len() >= 2 && in_order).then_some(chain)
}

/// Give each level of a LOD chain its band (see the module docs). Returns
/// the bands set, one per level, or `None` when the file holds no chain or
/// its meshes already name their own.
pub fn band_lod_chain(
    meshes: &mut [ImportedMesh],
    nodes: &[ImportedNode],
    root_nodes: &[usize],
) -> Option<Vec<LodBand>> {
    let chain = lod_chain(nodes, root_nodes)?;
    let levels: Vec<&[usize]> = chain
        .iter()
        .map(|&i| nodes[i].mesh_primitive_indices.as_slice())
        .collect();
    let prims = || levels.iter().flat_map(|l| l.iter().copied());
    if prims().any(|p| meshes.get(p).is_none_or(|m| m.lod.is_some())) {
        return None;
    }

    let bounds = prims()
        .filter_map(|p| meshes[p].bounds())
        .reduce(|a, b| a.union(&b))?;
    let switches = switch_distances(&bounds, &levels, meshes);

    let bands: Vec<LodBand> = (0..levels.len())
        .map(|level| LodBand {
            near: if level == 0 { 0.0 } else { switches[level - 1] },
            far: switches.get(level).copied().unwrap_or(f32::INFINITY),
            draw_distance: f32::INFINITY,
            ref_box: Some((bounds.min, bounds.max)),
        })
        .collect();
    for (level, prims) in levels.iter().enumerate() {
        for &p in prims.iter() {
            meshes[p].lod = Some(bands[level]);
        }
    }
    Some(bands)
}

/// The distances where level `i` hands over to `i + 1`, one fewer than the
/// levels.
fn switch_distances(bounds: &MeshBounds, levels: &[&[usize]], meshes: &[ImportedMesh]) -> Vec<f32> {
    let size = bounds.size();
    let radius = 0.5 * (size[0] * size[0] + size[1] * size[1] + size[2] * size[2]).sqrt();
    let triangles: Vec<f32> = levels
        .iter()
        .map(|l| {
            l.iter()
                .map(|&p| meshes[p].indices.len() / 3)
                .sum::<usize>() as f32
        })
        .collect();

    let mut switches = Vec::with_capacity(levels.len() - 1);
    let mut d = FIRST_SWITCH_RADII * radius.max(1e-3);
    switches.push(d);
    for i in 2..levels.len() {
        let (before, this) = (triangles[i - 1], triangles[i]);
        let ratio = if this > 0.0 {
            (before / this).sqrt()
        } else {
            0.0
        };
        d *= ratio.max(MIN_SWITCH_STEP);
        switches.push(d);
    }
    switches
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A level: one primitive, a box of `half` metres around the origin,
    /// with `triangles` triangles.
    fn level(half: f32, triangles: usize) -> ImportedMesh {
        ImportedMesh {
            name: String::new(),
            positions: vec![[-half; 3], [half; 3], [half, -half, half]],
            normals: vec![],
            uvs: vec![],
            indices: vec![0; triangles * 3],
            material_index: None,
            joint_indices: None,
            joint_weights: None,
            skin_index: None,
            lod: None,
        }
    }

    fn node(name: &str, prims: Vec<usize>) -> ImportedNode {
        ImportedNode {
            name: name.into(),
            translation: [0.0; 3],
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [1.0; 3],
            mesh_primitive_indices: prims,
            children: vec![],
            skin_index: None,
        }
    }

    #[test]
    fn a_chain_draws_one_level_at_every_distance() {
        // The oak's shape: 9000, 2200 and 500 triangles, the low levels a
        // little larger (bigger leaf cards).
        let mut meshes = vec![level(4.0, 9000), level(4.2, 2200), level(4.5, 500)];
        let nodes = vec![
            node("LOD_0", vec![0]),
            node("LOD_1", vec![1]),
            node("LOD_2", vec![2]),
        ];
        let bands = band_lod_chain(&mut meshes, &nodes, &[0, 1, 2]).expect("a chain");

        // One shared box, the union: 4.5 m on each side.
        for b in &bands {
            assert_eq!(b.ref_box, Some(([-4.5; 3], [4.5; 3])));
        }
        let radius = 4.5 * 3f32.sqrt();
        assert!((bands[0].far - 8.0 * radius).abs() < 1e-3);
        assert!((bands[2].near / bands[1].near - (2200f32 / 500.0).sqrt()).abs() < 1e-4);
        assert!(bands[2].far.is_infinite());

        for d in [0.0, 1.0, 30.0, bands[0].far, 80.0, bands[1].far, 500.0, 1e6] {
            let drawn = bands.iter().filter(|b| b.visible_at(d, 1.0, 1.0)).count();
            assert_eq!(drawn, 1, "at {d} m");
        }
        assert!(meshes.iter().zip(&bands).all(|(m, b)| m.lod == Some(*b)));
    }

    #[test]
    fn levels_that_barely_save_triangles_still_step_out() {
        let mut meshes = vec![level(1.0, 100), level(1.0, 90), level(1.0, 95)];
        let nodes = vec![
            node("LOD_0", vec![0]),
            node("LOD_1", vec![1]),
            node("LOD_2", vec![2]),
        ];
        let bands = band_lod_chain(&mut meshes, &nodes, &[0, 1, 2]).unwrap();
        assert!((bands[2].near / bands[1].near - MIN_SWITCH_STEP).abs() < 1e-5);
    }

    #[test]
    fn other_files_are_left_alone() {
        let mut meshes = vec![level(1.0, 10), level(1.0, 10)];
        // One level is not a chain.
        let one = vec![node("LOD_0", vec![0])];
        assert!(band_lod_chain(&mut meshes, &one, &[0]).is_none());
        // Out of order, or a part with another name.
        let swapped = vec![node("LOD_1", vec![0]), node("LOD_0", vec![1])];
        assert!(band_lod_chain(&mut meshes, &swapped, &[0, 1]).is_none());
        let parts = vec![node("LOD_0", vec![0]), node("Wheel", vec![1])];
        assert!(band_lod_chain(&mut meshes, &parts, &[0, 1]).is_none());
        // Levels below the root are some other model's business.
        let nested = vec![node("LOD_0", vec![0]), node("LOD_1", vec![1])];
        assert!(band_lod_chain(&mut meshes, &nested, &[]).is_none());
        assert!(meshes.iter().all(|m| m.lod.is_none()));

        // An authored band wins: the chain is not re-banded.
        let authored = LodBand {
            far: 12.0,
            ..LodBand::default()
        };
        meshes[0].lod = Some(authored);
        let chain = vec![node("LOD_0", vec![0]), node("LOD_1", vec![1])];
        assert!(band_lod_chain(&mut meshes, &chain, &[0, 1]).is_none());
        assert_eq!(meshes[0].lod, Some(authored));
        assert!(meshes[1].lod.is_none());
    }

    #[test]
    fn an_empty_root_does_not_break_the_chain() {
        let mut meshes = vec![level(1.0, 10), level(1.0, 5)];
        let nodes = vec![
            node("Camera", vec![]),
            node("LOD_0", vec![0]),
            node("LOD_1", vec![1]),
        ];
        assert_eq!(lod_chain(&nodes, &[0, 1, 2]), Some(vec![1, 2]));
        assert!(band_lod_chain(&mut meshes, &nodes, &[0, 1, 2]).is_some());
    }
}
