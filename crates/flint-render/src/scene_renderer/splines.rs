//! A scene's splines and the meshes swept along them (#750), loaded the
//! one way the player and `flint render` both use. The geometry is
//! `flint_geometry::sweep`'s; this is the component adapter: it reads
//! `spline` and `spline_mesh`, stores each path's samples on its entity as
//! `spline_data` (what the script spline functions read), uploads the
//! meshes and hands back their colliders for a host with physics.
//!
//! A `spline_mesh` sweeps a box `width` by `height`, or the polygon in
//! `profile` (`profile_closed`, `smooth`). `chunk_size` splits a long sweep
//! into child entities `<name>__chunk_<i>`, each with a `spline_chunk`.
//! Mistakes are warned once each: a spline file's with its file, line and
//! column, a component's with its entity and field.

use super::SceneRenderer;
use crate::primitives::Vertex;
use flint_core::components as comp;
use flint_core::diagnostic::{codes, Diagnostic};
use flint_core::project::Project;
use flint_core::toml_util::toml_f32_slice;
use flint_core::EntityId;
use flint_ecs::FlintWorld;
use flint_geometry::components;
use flint_geometry::spline::{Span, SplineSample};
use flint_geometry::spline_file::SplineFile;
use flint_geometry::sweep::{self, Profile, Stripes, SweepPath, SweepStyle, SweptMesh};
use flint_import::ImportedMaterial;
use std::collections::{BTreeMap, HashMap};
use std::path::Path;

/// A swept mesh's triangles, for a host with physics to make a static
/// trimesh collider of.
pub struct SplineCollider {
    /// The entity that holds the mesh: the `spline_mesh` entity, or one of
    /// its chunks.
    pub entity: EntityId,
    pub vertices: Vec<[f32; 3]>,
    pub triangles: Vec<[u32; 3]>,
    pub friction: f32,
    pub restitution: f32,
}

/// What [`load_scene_splines`] made.
#[derive(Default)]
pub struct LoadedSplines {
    /// Paths read and sampled.
    pub splines: usize,
    /// `spline_mesh` components swept.
    pub meshes: usize,
    /// One for each mesh (or chunk) whose `collider` is not false.
    pub colliders: Vec<SplineCollider>,
}

/// A spline as sampled, ready to sweep along.
struct SampledSpline {
    samples: Vec<SplineSample>,
    closed: bool,
    spacing: f32,
    gaps: Vec<(f32, f32)>,
}

/// A `spline_mesh` component as read.
struct MeshDef {
    spline: String,
    profile: Profile,
    style: SweepStyle,
    metallic: f32,
    roughness: f32,
    friction: f32,
    restitution: f32,
    chunk_size: usize,
    texture: Option<String>,
    normal_texture: Option<String>,
    collider: bool,
}

/// Load every `spline` of the scene and sweep every `spline_mesh` along
/// its spline, uploading the meshes to `renderer` and giving each entity
/// (or chunk) a `model.asset` naming its mesh. Files are found by the
/// project's search from the scene's folder (ADR 0114).
pub fn load_scene_splines(
    scene_path: &str,
    world: &mut FlintWorld,
    renderer: &mut SceneRenderer,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
) -> LoadedSplines {
    let scene_dir = Path::new(scene_path)
        .parent()
        .unwrap_or_else(|| Path::new("."));
    let project = Project::for_scene(Path::new(scene_path));
    let mut loaded = LoadedSplines::default();

    // ── The paths ──
    let mut splines: HashMap<String, SampledSpline> = HashMap::new();
    let entities: Vec<(EntityId, String)> = world
        .all_entities()
        .into_iter()
        .map(|e| (e.id, e.name))
        .collect();
    for (id, name) in &entities {
        let Some(file_rel) = world
            .view(*id, components::SPLINE)
            .opt_str("file")
            .map(String::from)
        else {
            continue;
        };
        let path = project.file_or_beside("spline file", &file_rel, scene_dir);
        let file = match SplineFile::read(&path) {
            Ok(file) => file,
            Err(e) => {
                tracing::warn!("spline '{name}': {e}");
                continue;
            }
        };
        let samples = file.samples();
        let gaps = file.gap_ranges();
        tracing::debug!(
            "spline '{name}': {} control points, {} samples ({})",
            file.control_points.len(),
            samples.len(),
            if file.closed { "closed" } else { "open" }
        );
        store_spline_data(world, *id, &samples, file.closed, &file.attr_spans());
        splines.insert(
            name.clone(),
            SampledSpline {
                samples,
                closed: file.closed,
                spacing: file.spacing,
                gaps,
            },
        );
        loaded.splines += 1;
    }

    // ── The meshes swept along them ──
    for (id, name) in &entities {
        if world.get_component(*id, components::SPLINE_MESH).is_none() {
            continue;
        }
        let Some(def) = mesh_def(world, *id) else {
            continue;
        };
        let Some(spline) = splines.get(&def.spline) else {
            tracing::warn!(
                "spline_mesh '{name}': its spline '{}' is no entity with a spline that loaded",
                def.spline
            );
            continue;
        };
        let path = SweepPath {
            samples: &spline.samples,
            closed: spline.closed,
            spacing: spline.spacing,
            gaps: &spline.gaps,
        };
        let mut texture = |rel: &Option<String>| {
            let rel = rel.as_deref()?;
            let file = project.file_or_beside("spline_mesh texture", rel, scene_dir);
            match renderer.load_texture_file_mipmapped(device, queue, rel, &file) {
                Ok(_) => Some(rel.to_string()),
                Err(e) => {
                    tracing::warn!("spline_mesh '{name}' texture '{rel}': {e}");
                    None
                }
            }
        };
        let base_color_texture = texture(&def.texture);
        let normal_texture = texture(&def.normal_texture);
        let material = |mesh_name: &str| ImportedMaterial {
            name: mesh_name.to_string(),
            base_color: def.style.color,
            metallic: def.metallic,
            roughness: def.roughness,
            base_color_texture: base_color_texture.clone(),
            normal_texture: normal_texture.clone(),
            metallic_roughness_texture: None,
            use_vertex_color: def.style.stripes.is_some(),
            alpha_mode: flint_import::AlphaMode::Opaque,
            alpha_cutoff: 0.5,
            ..Default::default()
        };
        let collider = |entity: EntityId, mesh: &SweptMesh| {
            def.collider.then(|| SplineCollider {
                entity,
                vertices: mesh.positions(),
                triangles: mesh.triangles(),
                friction: def.friction,
                restitution: def.restitution,
            })
        };

        if def.chunk_size > 0 {
            let chunks = sweep::sweep_chunks(&path, &def.profile, &def.style, def.chunk_size);
            tracing::debug!("spline_mesh '{name}': {} chunks", chunks.len());
            for chunk in chunks {
                let chunk_name = format!("{name}__chunk_{}", chunk.index);
                let chunk_id = match world.spawn(&chunk_name) {
                    Ok(id) => id,
                    Err(e) => {
                        tracing::error!("spline_mesh '{name}': chunk '{chunk_name}': {e}");
                        continue;
                    }
                };
                let _ = world.set_parent(chunk_id, *id);
                // The geometry is in world space: an identity transform
                let mut transform = toml::map::Map::new();
                transform.insert(
                    "position".into(),
                    toml::Value::Array(vec![toml::Value::Float(0.0); 3]),
                );
                let _ = world.set_component(chunk_id, comp::TRANSFORM, transform.into());
                let mut material_table = toml::map::Map::new();
                material_table.insert("opacity".into(), toml::Value::Float(1.0));
                let _ = world.set_component(chunk_id, comp::MATERIAL, material_table.into());
                let mut chunk_table = toml::map::Map::new();
                chunk_table.insert(
                    "chunk_index".into(),
                    toml::Value::Integer(chunk.index as i64),
                );
                chunk_table.insert("t_start".into(), toml::Value::Float(chunk.t_start as f64));
                chunk_table.insert("t_end".into(), toml::Value::Float(chunk.t_end as f64));
                let _ = world.set_component(chunk_id, components::SPLINE_CHUNK, chunk_table.into());

                upload(
                    renderer,
                    device,
                    &chunk_name,
                    &chunk.mesh,
                    material(&chunk_name),
                );
                loaded.colliders.extend(collider(chunk_id, &chunk.mesh));
                set_model_asset(world, chunk_id, &chunk_name);
            }
            // The parent draws nothing of its own
        } else {
            let mesh = sweep::sweep(&path, &def.profile, &def.style);
            if mesh.is_empty() {
                continue;
            }
            tracing::debug!(
                "spline_mesh '{name}': {} vertices, {} triangles",
                mesh.vertices.len(),
                mesh.indices.len() / 3
            );
            upload(renderer, device, name, &mesh, material(name));
            loaded.colliders.extend(collider(*id, &mesh));
            set_model_asset(world, *id, name);
        }
        loaded.meshes += 1;
    }

    if loaded.splines > 0 || loaded.meshes > 0 {
        tracing::info!(
            "splines: {} paths, {} meshes swept",
            loaded.splines,
            loaded.meshes
        );
    }
    loaded
}

/// Read a `spline_mesh`, reporting each mistake once (ADR 0115). `None`
/// when it can't be swept.
fn mesh_def(world: &FlintWorld, id: EntityId) -> Option<MeshDef> {
    let view = world.view(id, components::SPLINE_MESH);
    let required = |field: &str, why: &str| {
        view.report(field, || {
            Diagnostic::error(codes::MISSING_FIELD, format!("`{field}` is required{why}"))
                .hint("the mesh is skipped")
        });
    };
    let Some(spline) = view.opt_str("spline").map(String::from) else {
        required("spline", ": the entity whose path this follows");
        return None;
    };

    let profile = if view.has("profile") {
        let points = view
            .raw("profile")
            .and_then(|v| v.as_array())
            .and_then(|points| {
                points
                    .iter()
                    .map(|p| {
                        let xy = toml_f32_slice(p.as_array()?)?;
                        (xy.len() == 2).then(|| [xy[0], xy[1]])
                    })
                    .collect::<Option<Vec<[f32; 2]>>>()
            });
        let Some(points) = points else {
            view.report("profile", || {
                Diagnostic::error(
                    codes::BAD_TYPE,
                    "`profile` is a list of [right, up] points, in metres",
                )
                .hint("the mesh is skipped")
            });
            return None;
        };
        let closed = view.bool("profile_closed", true);
        match Profile::new(points, closed, view.bool("smooth", false)) {
            Ok(profile) => profile,
            Err(why) => {
                view.report("profile", || {
                    Diagnostic::error(codes::BAD_SPLINE, why).hint("the mesh is skipped")
                });
                return None;
            }
        }
    } else {
        let (Some(width), Some(height)) = (view.opt_f32("width"), view.opt_f32("height")) else {
            let field = if view.has("width") { "height" } else { "width" };
            required(field, " (or a `profile`)");
            return None;
        };
        let rect = Profile::rect(width, height);
        if view.bool("smooth", false) {
            Profile::new(rect.points().to_vec(), true, true).unwrap_or(rect)
        } else {
            rect
        }
    };

    let offset = match view.raw("offset") {
        None => [0.0, 0.0],
        Some(value) => match value.as_array().and_then(|arr| toml_f32_slice(arr)) {
            Some(vals) if vals.len() >= 2 => [vals[0], vals[1]],
            _ => {
                view.report("offset", || {
                    Diagnostic::error(
                        codes::BAD_TYPE,
                        format!("expected [right, up], 2 numbers, got {value}"),
                    )
                    .hint("the default [0, 0] is used")
                });
                [0.0, 0.0]
            }
        },
    };
    let stripes = view.opt_color("stripe_color").map(|color| Stripes {
        color,
        width: view.f32("stripe_width", 2.0),
    });
    let texture_length = view
        .opt_f32("texture_length")
        .filter(|l| *l > 0.0)
        .unwrap_or(profile.width().max(0.01));
    let file = |field: &str| {
        view.opt_str(field)
            .filter(|s| !s.is_empty())
            .map(String::from)
    };

    Some(MeshDef {
        spline,
        style: SweepStyle {
            offset,
            color: view.color("color", [0.5, 0.5, 0.5, 1.0]),
            stripes,
            texture_length,
        },
        profile,
        metallic: view.f32("metallic", 0.0),
        roughness: view.f32("roughness", 0.5),
        friction: view.f32("friction", 0.5),
        restitution: view.f32("restitution", 0.1),
        chunk_size: view.i64("chunk_size", 0).max(0) as usize,
        texture: file("texture"),
        normal_texture: file("normal_texture"),
        collider: view.bool("collider", true),
    })
}

fn upload(
    renderer: &mut SceneRenderer,
    device: &wgpu::Device,
    name: &str,
    mesh: &SweptMesh,
    material: ImportedMaterial,
) {
    let vertices: Vec<Vertex> = mesh
        .vertices
        .iter()
        .map(|v| Vertex {
            position: v.position,
            normal: v.normal,
            color: v.color,
            uv: v.uv,
        })
        .collect();
    renderer.load_procedural_mesh(device, name, &vertices, &mesh.indices, material);
}

/// Name the uploaded mesh in the entity's `model.asset`, so it is drawn.
fn set_model_asset(world: &mut FlintWorld, id: EntityId, asset: &str) {
    if let Some(mut comps) = world.get_components_mut(id) {
        comps.set_field(comp::MODEL, "asset", toml::Value::String(asset.into()));
    } else {
        let mut model = toml::map::Map::new();
        model.insert("asset".into(), toml::Value::String(asset.into()));
        let _ = world.set_component(id, comp::MODEL, model.into());
    }
}

/// Store a path's samples as flat arrays in its entity's `spline_data`,
/// which the script spline functions read, and its control points'
/// attributes as spans: `attrs.<name> = { starts, ends, values }` (#855;
/// `gap_after` is one of them, `true` over each gap).
fn store_spline_data(
    world: &mut FlintWorld,
    id: EntityId,
    samples: &[SplineSample],
    closed: bool,
    attrs: &BTreeMap<String, Vec<Span<toml::Value>>>,
) {
    let floats = |f: &dyn Fn(&SplineSample) -> f32| {
        toml::Value::Array(
            samples
                .iter()
                .map(|s| toml::Value::Float(f(s) as f64))
                .collect(),
        )
    };
    let mut table = toml::map::Map::new();
    table.insert("positions_x".into(), floats(&|s| s.position.x));
    table.insert("positions_y".into(), floats(&|s| s.position.y));
    table.insert("positions_z".into(), floats(&|s| s.position.z));
    table.insert("forwards_x".into(), floats(&|s| s.forward.x));
    table.insert("forwards_y".into(), floats(&|s| s.forward.y));
    table.insert("forwards_z".into(), floats(&|s| s.forward.z));
    table.insert("rights_x".into(), floats(&|s| s.right.x));
    table.insert("rights_y".into(), floats(&|s| s.right.y));
    table.insert("rights_z".into(), floats(&|s| s.right.z));
    table.insert("ups_x".into(), floats(&|s| s.up.x));
    table.insert("ups_y".into(), floats(&|s| s.up.y));
    table.insert("ups_z".into(), floats(&|s| s.up.z));
    table.insert("twists".into(), floats(&|s| s.twist));
    table.insert("t_values".into(), floats(&|s| s.t));
    table.insert(
        "sample_count".into(),
        toml::Value::Integer(samples.len() as i64),
    );
    table.insert("closed".into(), toml::Value::Boolean(closed));
    let mut attr_table = toml::map::Map::new();
    for (name, spans) in attrs {
        let ends = |f: fn(&Span<toml::Value>) -> f32| {
            toml::Value::Array(
                spans
                    .iter()
                    .map(|s| toml::Value::Float(f(s) as f64))
                    .collect(),
            )
        };
        let mut runs = toml::map::Map::new();
        runs.insert("starts".into(), ends(|s| s.start));
        runs.insert("ends".into(), ends(|s| s.end));
        runs.insert(
            "values".into(),
            toml::Value::Array(spans.iter().map(|s| s.value.clone()).collect()),
        );
        attr_table.insert(name.clone(), toml::Value::Table(runs));
    }
    table.insert("attrs".into(), toml::Value::Table(attr_table));
    let _ = world.set_component(id, components::SPLINE_DATA, table.into());
}
