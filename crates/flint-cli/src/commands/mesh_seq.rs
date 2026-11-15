//! Mesh sequences for `flint render --mesh <entity>=<files>` (#222).
//!
//! A procgen look test writes its frames as mesh files: one GLB per pose, per
//! seed, per setting. `--mesh` binds an entity of the scene to such a list, and
//! the render draws one frame per file on one renderer, each with the entity
//! showing that file's mesh, so a sequence costs one start-up, not one each.
//!
//! The files are matched here, before a scene loads, and each one is flattened
//! on the way in: a GLB of several mesh nodes, or one moved by its node, becomes
//! one mesh with every node's transform baked into its vertices. That is what
//! lets a frame be a single upload under the binding's own asset name, with no
//! child entities to make and unmake between frames.

use anyhow::{bail, Context, Result};
use flint_import::{ImportResult, ImportedMesh, ImportedNode};
use glam::{Mat3, Mat4, Quat, Vec3};
use std::path::{Path, PathBuf};

/// `--mesh tree_a=out/f*.glb` as typed: an entity and where its frames are.
#[derive(Debug, Clone, PartialEq)]
pub struct MeshArg {
    pub entity: String,
    pub pattern: String,
}

/// Parse one `--mesh`: `<entity>=<file, glob or folder>`.
pub fn parse_mesh_arg(text: &str) -> Result<MeshArg, String> {
    let Some((entity, pattern)) = text.split_once('=') else {
        return Err(format!(
            "'{text}' names no entity: it's <entity>=<files>, as tree_a=out/f*.glb"
        ));
    };
    let (entity, pattern) = (entity.trim(), pattern.trim());
    if entity.is_empty() || pattern.is_empty() {
        return Err(format!(
            "'{text}' needs both sides: <entity>=<files>, as tree_a=out/f*.glb"
        ));
    }
    Ok(MeshArg {
        entity: entity.to_string(),
        pattern: pattern.to_string(),
    })
}

/// An entity and its frames, in the order they're drawn.
#[derive(Debug, Clone)]
pub struct MeshBinding {
    pub entity: String,
    pub files: Vec<PathBuf>,
}

impl MeshBinding {
    /// The asset name the binding's mesh is uploaded under: one no scene names,
    /// so the entity's own asset (which other entities may share) is untouched.
    pub fn asset(&self) -> String {
        format!("mesh-seq/{}", self.entity)
    }
}

/// Every `--mesh`, its files found, and all of them the same length.
pub fn resolve(args: &[MeshArg]) -> Result<Vec<MeshBinding>> {
    let mut bindings: Vec<MeshBinding> = Vec::new();
    for arg in args {
        if bindings.iter().any(|b| b.entity == arg.entity) {
            bail!("--mesh binds '{}' twice", arg.entity);
        }
        let files = match_files(&arg.pattern)?;
        bindings.push(MeshBinding {
            entity: arg.entity.clone(),
            files,
        });
    }
    if let Some(first) = bindings.first() {
        for b in &bindings[1..] {
            if b.files.len() != first.files.len() {
                bail!(
                    "--mesh sequences differ in length: '{}' has {} files, '{}' has {}; \
                     each frame takes one file from every binding",
                    first.entity,
                    first.files.len(),
                    b.entity,
                    b.files.len()
                );
            }
        }
    }
    Ok(bindings)
}

/// What a `--mesh` pattern names: one file, a folder's `.glb`/`.gltf` files, or
/// a `*` / `?` wildcard in the file name (`out/f*.glb`), in natural order, so
/// `f2` comes before `f10`.
pub fn match_files(pattern: &str) -> Result<Vec<PathBuf>> {
    let path = Path::new(pattern);
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut files: Vec<PathBuf> = if name.contains(['*', '?']) {
        let dir = match path.parent() {
            Some(d) if !d.as_os_str().is_empty() => d.to_path_buf(),
            _ => PathBuf::from("."),
        };
        if dir.to_string_lossy().contains(['*', '?']) {
            bail!("--mesh {pattern}: only the file name can hold a wildcard, not a folder");
        }
        read_dir(&dir, pattern)?
            .into_iter()
            .filter(|p| {
                p.file_name()
                    .is_some_and(|n| wildcard(&name, &n.to_string_lossy()))
            })
            .collect()
    } else if path.is_dir() {
        read_dir(path, pattern)?
            .into_iter()
            .filter(|p| is_mesh_file(p))
            .collect()
    } else if path.is_file() {
        vec![path.to_path_buf()]
    } else {
        bail!("--mesh {pattern}: no such file or folder");
    };
    files.sort_by(|a, b| natural_cmp(&a.to_string_lossy(), &b.to_string_lossy()));
    if files.is_empty() {
        bail!("--mesh {pattern} matches no files");
    }
    Ok(files)
}

fn read_dir(dir: &Path, pattern: &str) -> Result<Vec<PathBuf>> {
    let entries = std::fs::read_dir(dir)
        .with_context(|| format!("--mesh {pattern}: reading {}", dir.display()))?;
    Ok(entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_file())
        .collect())
}

fn is_mesh_file(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("glb") || e.eq_ignore_ascii_case("gltf"))
}

/// `*` any run of characters, `?` any one; everything else matches itself.
fn wildcard(pattern: &str, name: &str) -> bool {
    let (p, n): (Vec<char>, Vec<char>) = (pattern.chars().collect(), name.chars().collect());
    let (mut pi, mut ni) = (0, 0);
    let (mut star, mut mark) = (None, 0);
    while ni < n.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == n[ni]) {
            pi += 1;
            ni += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            mark = ni;
            pi += 1;
        } else if let Some(s) = star {
            pi = s + 1;
            mark += 1;
            ni = mark;
        } else {
            return false;
        }
    }
    p[pi..].iter().all(|&c| c == '*')
}

/// Names compared with their runs of digits as numbers: `f2` < `f10`.
fn natural_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    let (mut a, mut b) = (a.as_bytes(), b.as_bytes());
    loop {
        match (a.first(), b.first()) {
            (None, None) => return Ordering::Equal,
            (None, _) => return Ordering::Less,
            (_, None) => return Ordering::Greater,
            (Some(x), Some(y)) if x.is_ascii_digit() && y.is_ascii_digit() => {
                let da = a.iter().take_while(|c| c.is_ascii_digit()).count();
                let db = b.iter().take_while(|c| c.is_ascii_digit()).count();
                let (na, nb) = (&a[..da], &b[..db]);
                let trim = |s: &[u8]| -> usize { s.iter().take_while(|&&c| c == b'0').count() };
                let (ta, tb) = (&na[trim(na)..], &nb[trim(nb)..]);
                let ord = ta
                    .len()
                    .cmp(&tb.len())
                    .then_with(|| ta.cmp(tb))
                    .then(da.cmp(&db));
                if ord != Ordering::Equal {
                    return ord;
                }
                a = &a[da..];
                b = &b[db..];
            }
            (Some(x), Some(y)) => {
                if x != y {
                    return x.cmp(y);
                }
                a = &a[1..];
                b = &b[1..];
            }
        }
    }
}

/// The frame's label on a contact sheet: its file's name without the extension.
pub fn label(path: &Path) -> String {
    path.file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// `import`, with every mesh node's transform baked into its vertices and the
/// whole thing one node at the origin. A file that's already that is returned
/// as it was. Skinned meshes keep their vertices (glTF places a skinned mesh
/// by its joints, never its node), and a node mirrored by a negative scale has
/// its triangles turned so they still face out.
pub fn flatten(import: ImportResult) -> ImportResult {
    if !import.needs_expansion() {
        return import;
    }
    let mut meshes = Vec::new();
    let mut stack: Vec<(usize, Mat4)> = import
        .root_nodes
        .iter()
        .rev()
        .map(|&n| (n, Mat4::IDENTITY))
        .collect();
    let mut seen = vec![false; import.nodes.len()];
    while let Some((index, parent)) = stack.pop() {
        let Some(node) = import.nodes.get(index) else {
            continue;
        };
        if std::mem::replace(&mut seen[index], true) {
            continue; // a cycle or a shared child: glTF allows neither
        }
        let world = parent * local(node);
        for &p in &node.mesh_primitive_indices {
            if let Some(mesh) = import.meshes.get(p) {
                meshes.push(if mesh.skin_index.is_some() || node.skin_index.is_some() {
                    mesh.clone()
                } else {
                    transformed(mesh, world)
                });
            }
        }
        for &child in node.children.iter().rev() {
            stack.push((child, world));
        }
    }
    let node = ImportedNode {
        name: "flattened".into(),
        translation: [0.0; 3],
        rotation: [0.0, 0.0, 0.0, 1.0],
        scale: [1.0; 3],
        mesh_primitive_indices: (0..meshes.len()).collect(),
        children: Vec::new(),
        skin_index: None,
    };
    ImportResult {
        meshes,
        nodes: vec![node],
        root_nodes: vec![0],
        node_clips: Vec::new(),
        ..import
    }
}

fn local(node: &ImportedNode) -> Mat4 {
    Mat4::from_scale_rotation_translation(
        Vec3::from(node.scale),
        Quat::from_array(node.rotation).normalize(),
        Vec3::from(node.translation),
    )
}

fn transformed(mesh: &ImportedMesh, world: Mat4) -> ImportedMesh {
    let normal_matrix = Mat3::from_mat4(world).inverse().transpose();
    let mut out = mesh.clone();
    for p in &mut out.positions {
        *p = world.transform_point3(Vec3::from(*p)).to_array();
    }
    for n in &mut out.normals {
        *n = (normal_matrix * Vec3::from(*n))
            .normalize_or_zero()
            .to_array();
    }
    if world.determinant() < 0.0 {
        for tri in out.indices.as_chunks_mut::<3>().0 {
            tri.swap(1, 2);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_binding_is_an_entity_and_its_files() {
        assert_eq!(
            parse_mesh_arg("tree_a=out/f*.glb"),
            Ok(MeshArg {
                entity: "tree_a".into(),
                pattern: "out/f*.glb".into()
            })
        );
        assert!(parse_mesh_arg("out/f*.glb")
            .unwrap_err()
            .contains("names no entity"));
        assert!(parse_mesh_arg("tree_a=")
            .unwrap_err()
            .contains("both sides"));
    }

    #[test]
    fn wildcards_match_a_file_name() {
        assert!(wildcard("f*.glb", "f001.glb"));
        assert!(wildcard("f???.glb", "f001.glb"));
        assert!(!wildcard("f???.glb", "f01.glb"));
        assert!(!wildcard("f*.glb", "f001.png"));
        assert!(wildcard("*", "anything"));
        assert!(wildcard("a*b*c", "aXXbYYc"));
        assert!(!wildcard("a*b*c", "aXXbYY"));
    }

    #[test]
    fn numbers_sort_as_numbers() {
        let mut names = vec!["f10.glb", "f2.glb", "f1.glb", "g.glb", "f02.glb"];
        names.sort_by(|a, b| natural_cmp(a, b));
        assert_eq!(names, ["f1.glb", "f2.glb", "f02.glb", "f10.glb", "g.glb"]);
    }

    #[test]
    fn folders_and_globs_find_their_files_in_order() {
        let dir = std::env::temp_dir().join(format!("flint-mesh-seq-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for name in ["f10.glb", "f9.glb", "f1.glb", "notes.txt"] {
            std::fs::write(dir.join(name), b"").unwrap();
        }
        let names =
            |files: Vec<PathBuf>| -> Vec<String> { files.iter().map(|p| label(p)).collect() };
        let folder = match_files(&dir.to_string_lossy()).unwrap();
        assert_eq!(names(folder), ["f1", "f9", "f10"]);
        let glob = match_files(&dir.join("f1*.glb").to_string_lossy()).unwrap();
        assert_eq!(names(glob), ["f1", "f10"]);
        assert!(match_files(&dir.join("z*.glb").to_string_lossy())
            .unwrap_err()
            .to_string()
            .contains("matches no files"));
        let args = [
            parse_mesh_arg(&format!("a={}", dir.display())).unwrap(),
            parse_mesh_arg(&format!("b={}", dir.join("f1*.glb").display())).unwrap(),
        ];
        assert!(resolve(&args)
            .unwrap_err()
            .to_string()
            .contains("differ in length"));
        std::fs::remove_dir_all(&dir).ok();
    }

    fn triangle(name: &str) -> ImportedMesh {
        ImportedMesh {
            name: name.into(),
            positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            normals: vec![[0.0, 0.0, 1.0]; 3],
            uvs: Vec::new(),
            indices: vec![0, 1, 2],
            material_index: None,
            joint_indices: None,
            joint_weights: None,
            skin_index: None,
            lod: None,
        }
    }

    fn node(
        name: &str,
        t: [f32; 3],
        s: [f32; 3],
        meshes: Vec<usize>,
        children: Vec<usize>,
    ) -> ImportedNode {
        ImportedNode {
            name: name.into(),
            translation: t,
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: s,
            mesh_primitive_indices: meshes,
            children,
            skin_index: None,
        }
    }

    fn import(
        meshes: Vec<ImportedMesh>,
        nodes: Vec<ImportedNode>,
        roots: Vec<usize>,
    ) -> ImportResult {
        ImportResult {
            asset_meta: flint_asset::AssetMeta {
                name: "t".into(),
                asset_type: flint_asset::AssetType::Mesh,
                hash: "0".into(),
                source_path: None,
                format: None,
                properties: Default::default(),
                tags: Vec::new(),
            },
            meshes,
            textures: Vec::new(),
            materials: Vec::new(),
            skeletons: Vec::new(),
            skeletal_clips: Vec::new(),
            node_clips: Vec::new(),
            nodes,
            root_nodes: roots,
        }
    }

    #[test]
    fn nodes_bake_into_one_mesh_node() {
        // A parent moved up 2 m holding a child moved 1 m along x and
        // mirrored in x: the child's triangle lands at (1, 2, 0) facing out.
        let flat = flatten(import(
            vec![triangle("a"), triangle("b")],
            vec![
                node("parent", [0.0, 2.0, 0.0], [1.0; 3], vec![0], vec![1]),
                node("child", [1.0, 0.0, 0.0], [-1.0, 1.0, 1.0], vec![1], vec![]),
            ],
            vec![0],
        ));
        assert!(!flat.needs_expansion());
        assert_eq!(flat.nodes.len(), 1);
        assert_eq!(flat.nodes[0].mesh_primitive_indices, [0, 1]);
        assert_eq!(flat.meshes[0].positions[1], [1.0, 2.0, 0.0]);
        assert_eq!(flat.meshes[1].positions[0], [1.0, 2.0, 0.0]);
        assert_eq!(flat.meshes[1].positions[1], [0.0, 2.0, 0.0]);
        assert_eq!(flat.meshes[1].indices, [0, 2, 1]);
        assert_eq!(flat.meshes[1].normals[0], [0.0, 0.0, 1.0]);
    }

    #[test]
    fn a_plain_mesh_is_left_alone() {
        let one = import(
            vec![triangle("a")],
            vec![node("only", [0.0; 3], [1.0; 3], vec![0], vec![])],
            vec![0],
        );
        let flat = flatten(one.clone());
        assert_eq!(flat.nodes[0].name, "only");
        assert_eq!(flat.meshes[0].positions, one.meshes[0].positions);
    }
}
