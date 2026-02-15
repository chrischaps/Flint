//! Spline-based procedural geometry.
//!
//! Two-pass initialization:
//! 1. Load `.spline.toml` files for entities with a `spline` component,
//!    sample the path, and store `spline_data` on each entity.
//! 2. For entities with a `spline_mesh` component, sweep a rectangular
//!    cross-section along the referenced spline and upload the resulting
//!    mesh + trimesh collider.

use flint_core::spline::{
    self, SplineControlPoint, SplineSample,
};
use flint_core::Vec3;
use flint_ecs::FlintWorld;
use flint_import::ImportedMaterial;
use flint_physics::PhysicsSystem;
use flint_render::{SceneRenderer, Vertex};
use std::collections::HashMap;
use std::path::Path;

// ─── TOML parsing helpers ────────────────────────────────

fn toml_f32(v: &toml::Value) -> Option<f32> {
    v.as_float()
        .map(|f| f as f32)
        .or_else(|| v.as_integer().map(|i| i as f32))
}

fn toml_f32_array(arr: &[toml::Value]) -> Option<Vec<f32>> {
    arr.iter().map(toml_f32).collect()
}

// ─── Pass 1: load spline data ────────────────────────────

struct SplineDef {
    closed: bool,
    spacing: f32,
    control_points: Vec<SplineControlPoint>,
}

fn parse_spline_file(path: &Path) -> Option<SplineDef> {
    let content = std::fs::read_to_string(path).ok()?;
    let value: toml::Value = toml::from_str(&content).ok()?;

    let spline_section = value.get("spline")?;
    let closed = spline_section
        .get("closed")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);

    let spacing = value
        .get("sampling")
        .and_then(|s| s.get("spacing"))
        .and_then(toml_f32)
        .unwrap_or(2.0);

    let points_arr = value.get("control_points")?.as_array()?;
    let mut control_points = Vec::new();

    for pt in points_arr {
        let pos = pt.get("position")?.as_array()?;
        if pos.len() < 3 {
            continue;
        }
        let vals = toml_f32_array(pos)?;
        let twist = pt.get("twist").and_then(toml_f32).unwrap_or(0.0);

        control_points.push(SplineControlPoint {
            position: Vec3::new(vals[0], vals[1], vals[2]),
            twist,
        });
    }

    if control_points.len() < 2 {
        eprintln!(
            "Spline needs at least 2 control points, got {}",
            control_points.len()
        );
        return None;
    }

    Some(SplineDef {
        closed,
        spacing,
        control_points,
    })
}

/// Store sampled spline data as flat arrays in a `spline_data` ECS component.
fn store_spline_data(
    world: &mut FlintWorld,
    entity_id: flint_core::EntityId,
    samples: &[SplineSample],
    closed: bool,
) {
    let to_val = |f: f32| toml::Value::Float(f as f64);

    let positions_x: Vec<toml::Value> = samples.iter().map(|s| to_val(s.position.x)).collect();
    let positions_y: Vec<toml::Value> = samples.iter().map(|s| to_val(s.position.y)).collect();
    let positions_z: Vec<toml::Value> = samples.iter().map(|s| to_val(s.position.z)).collect();
    let forwards_x: Vec<toml::Value> = samples.iter().map(|s| to_val(s.forward.x)).collect();
    let forwards_y: Vec<toml::Value> = samples.iter().map(|s| to_val(s.forward.y)).collect();
    let forwards_z: Vec<toml::Value> = samples.iter().map(|s| to_val(s.forward.z)).collect();
    let rights_x: Vec<toml::Value> = samples.iter().map(|s| to_val(s.right.x)).collect();
    let rights_y: Vec<toml::Value> = samples.iter().map(|s| to_val(s.right.y)).collect();
    let rights_z: Vec<toml::Value> = samples.iter().map(|s| to_val(s.right.z)).collect();
    let ups_x: Vec<toml::Value> = samples.iter().map(|s| to_val(s.up.x)).collect();
    let ups_y: Vec<toml::Value> = samples.iter().map(|s| to_val(s.up.y)).collect();
    let ups_z: Vec<toml::Value> = samples.iter().map(|s| to_val(s.up.z)).collect();
    let twists: Vec<toml::Value> = samples.iter().map(|s| to_val(s.twist)).collect();
    let t_values: Vec<toml::Value> = samples.iter().map(|s| to_val(s.t)).collect();

    let mut table = toml::map::Map::new();
    table.insert("positions_x".into(), toml::Value::Array(positions_x));
    table.insert("positions_y".into(), toml::Value::Array(positions_y));
    table.insert("positions_z".into(), toml::Value::Array(positions_z));
    table.insert("forwards_x".into(), toml::Value::Array(forwards_x));
    table.insert("forwards_y".into(), toml::Value::Array(forwards_y));
    table.insert("forwards_z".into(), toml::Value::Array(forwards_z));
    table.insert("rights_x".into(), toml::Value::Array(rights_x));
    table.insert("rights_y".into(), toml::Value::Array(rights_y));
    table.insert("rights_z".into(), toml::Value::Array(rights_z));
    table.insert("ups_x".into(), toml::Value::Array(ups_x));
    table.insert("ups_y".into(), toml::Value::Array(ups_y));
    table.insert("ups_z".into(), toml::Value::Array(ups_z));
    table.insert("twists".into(), toml::Value::Array(twists));
    table.insert("t_values".into(), toml::Value::Array(t_values));
    table.insert(
        "sample_count".into(),
        toml::Value::Integer(samples.len() as i64),
    );
    table.insert("closed".into(), toml::Value::Boolean(closed));

    let _ = world.set_component(entity_id, "spline_data", toml::Value::Table(table));
}

// ─── Pass 2: generate cross-section meshes ───────────────

struct SplineMeshDef {
    spline_entity: String,
    width: f32,
    height: f32,
    offset: [f32; 2], // [right, up]
    color: [f32; 4],
    friction: f32,
    restitution: f32,
    metallic: f32,
    roughness: f32,
}

fn parse_spline_mesh_component(comp: &toml::Value) -> Option<SplineMeshDef> {
    let spline_entity = comp.get("spline")?.as_str()?.to_string();
    let width = comp.get("width").and_then(toml_f32)?;
    let height = comp.get("height").and_then(toml_f32)?;

    let offset = if let Some(arr) = comp.get("offset").and_then(|v| v.as_array()) {
        let vals = toml_f32_array(arr)?;
        if vals.len() >= 2 {
            [vals[0], vals[1]]
        } else {
            [0.0, 0.0]
        }
    } else {
        [0.0, 0.0]
    };

    let color = if let Some(arr) = comp.get("color").and_then(|v| v.as_array()) {
        let vals = toml_f32_array(arr)?;
        if vals.len() >= 4 {
            [vals[0], vals[1], vals[2], vals[3]]
        } else if vals.len() >= 3 {
            [vals[0], vals[1], vals[2], 1.0]
        } else {
            [0.5, 0.5, 0.5, 1.0]
        }
    } else {
        [0.5, 0.5, 0.5, 1.0]
    };

    let friction = comp.get("friction").and_then(toml_f32).unwrap_or(0.5);
    let restitution = comp.get("restitution").and_then(toml_f32).unwrap_or(0.1);
    let metallic = comp.get("metallic").and_then(toml_f32).unwrap_or(0.0);
    let roughness = comp.get("roughness").and_then(toml_f32).unwrap_or(0.5);

    Some(SplineMeshDef {
        spline_entity,
        width,
        height,
        offset,
        color,
        friction,
        restitution,
        metallic,
        roughness,
    })
}

/// Generate a rectangular cross-section mesh swept along spline samples.
///
/// At each sample, 4 corners are computed:
///   TL ──── TR
///   │        │
///   BL ──── BR
///
/// Between consecutive samples, 4 quad faces are generated
/// (top, bottom, left-side, right-side). For open splines,
/// 2 end-cap quads are added.
fn generate_cross_section_mesh(
    samples: &[SplineSample],
    closed: bool,
    width: f32,
    height: f32,
    offset: [f32; 2],
    color: [f32; 4],
) -> (Vec<Vertex>, Vec<u32>, Vec<[f32; 3]>, Vec<[u32; 3]>) {
    let n = samples.len();
    if n < 2 {
        return (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    }

    let hw = width / 2.0;
    let hh = height / 2.0;

    // Pre-compute 4 corners at each sample in world space
    // Order: BL, BR, TR, TL (bottom-left, bottom-right, top-right, top-left)
    let mut corners: Vec<[Vec3; 4]> = Vec::with_capacity(n);
    for s in samples {
        let center = s.position + s.right * offset[0] + s.up * offset[1];
        let bl = center + s.right * (-hw) + s.up * (-hh);
        let br = center + s.right * hw + s.up * (-hh);
        let tr = center + s.right * hw + s.up * hh;
        let tl = center + s.right * (-hw) + s.up * hh;
        corners.push([bl, br, tr, tl]);
    }

    // Each longitudinal segment contributes 4 quads (8 triangles, 8 vertices unique per face)
    // We use per-face vertices for correct normals.
    let num_segs = if closed { n } else { n - 1 };
    // 4 faces * 4 verts each = 16 verts per segment
    let vert_cap = num_segs * 16 + if !closed { 8 } else { 0 };
    let tri_cap = num_segs * 8 + if !closed { 4 } else { 0 };

    let mut vertices = Vec::with_capacity(vert_cap);
    let mut indices = Vec::with_capacity(tri_cap * 3);
    let mut phys_verts = Vec::with_capacity(vert_cap);
    let mut phys_tris = Vec::with_capacity(tri_cap);

    for seg in 0..num_segs {
        let next = if closed { (seg + 1) % n } else { seg + 1 };
        let [c_bl, c_br, c_tr, c_tl] = corners[seg];
        let [n_bl, n_br, n_tr, n_tl] = corners[next];

        let u0 = seg as f32 / num_segs as f32;
        let u1 = (seg + 1) as f32 / num_segs as f32;

        // --- Top face (TL, TR → NTL, NTR) ---
        // Normal: average of up vectors
        let top_normal = ((samples[seg].up + samples[next].up) * 0.5).normalized();
        let base = vertices.len() as u32;
        vertices.extend_from_slice(&[
            Vertex { position: c_tl.to_array(), normal: top_normal.to_array(), color, uv: [0.0, u0] },
            Vertex { position: c_tr.to_array(), normal: top_normal.to_array(), color, uv: [1.0, u0] },
            Vertex { position: n_tl.to_array(), normal: top_normal.to_array(), color, uv: [0.0, u1] },
            Vertex { position: n_tr.to_array(), normal: top_normal.to_array(), color, uv: [1.0, u1] },
        ]);
        indices.extend_from_slice(&[base, base + 1, base + 2, base + 1, base + 3, base + 2]);
        let pb = phys_verts.len() as u32;
        phys_verts.extend_from_slice(&[c_tl.to_array(), c_tr.to_array(), n_tl.to_array(), n_tr.to_array()]);
        phys_tris.push([pb, pb + 1, pb + 2]);
        phys_tris.push([pb + 1, pb + 3, pb + 2]);

        // --- Bottom face (BL, BR → NBL, NBR) ---
        let bot_normal = ((samples[seg].up + samples[next].up) * -0.5).normalized();
        let base = vertices.len() as u32;
        vertices.extend_from_slice(&[
            Vertex { position: c_br.to_array(), normal: bot_normal.to_array(), color, uv: [0.0, u0] },
            Vertex { position: c_bl.to_array(), normal: bot_normal.to_array(), color, uv: [1.0, u0] },
            Vertex { position: n_br.to_array(), normal: bot_normal.to_array(), color, uv: [0.0, u1] },
            Vertex { position: n_bl.to_array(), normal: bot_normal.to_array(), color, uv: [1.0, u1] },
        ]);
        indices.extend_from_slice(&[base, base + 1, base + 2, base + 1, base + 3, base + 2]);
        let pb = phys_verts.len() as u32;
        phys_verts.extend_from_slice(&[c_br.to_array(), c_bl.to_array(), n_br.to_array(), n_bl.to_array()]);
        phys_tris.push([pb, pb + 1, pb + 2]);
        phys_tris.push([pb + 1, pb + 3, pb + 2]);

        // --- Left side face (TL, BL → NTL, NBL) ---
        let left_normal = ((samples[seg].right + samples[next].right) * -0.5).normalized();
        let base = vertices.len() as u32;
        vertices.extend_from_slice(&[
            Vertex { position: c_bl.to_array(), normal: left_normal.to_array(), color, uv: [0.0, u0] },
            Vertex { position: c_tl.to_array(), normal: left_normal.to_array(), color, uv: [1.0, u0] },
            Vertex { position: n_bl.to_array(), normal: left_normal.to_array(), color, uv: [0.0, u1] },
            Vertex { position: n_tl.to_array(), normal: left_normal.to_array(), color, uv: [1.0, u1] },
        ]);
        indices.extend_from_slice(&[base, base + 1, base + 2, base + 1, base + 3, base + 2]);
        let pb = phys_verts.len() as u32;
        phys_verts.extend_from_slice(&[c_bl.to_array(), c_tl.to_array(), n_bl.to_array(), n_tl.to_array()]);
        phys_tris.push([pb, pb + 1, pb + 2]);
        phys_tris.push([pb + 1, pb + 3, pb + 2]);

        // --- Right side face (BR, TR → NBR, NTR) ---
        let right_normal = ((samples[seg].right + samples[next].right) * 0.5).normalized();
        let base = vertices.len() as u32;
        vertices.extend_from_slice(&[
            Vertex { position: c_tr.to_array(), normal: right_normal.to_array(), color, uv: [0.0, u0] },
            Vertex { position: c_br.to_array(), normal: right_normal.to_array(), color, uv: [1.0, u0] },
            Vertex { position: n_tr.to_array(), normal: right_normal.to_array(), color, uv: [0.0, u1] },
            Vertex { position: n_br.to_array(), normal: right_normal.to_array(), color, uv: [1.0, u1] },
        ]);
        indices.extend_from_slice(&[base, base + 1, base + 2, base + 1, base + 3, base + 2]);
        let pb = phys_verts.len() as u32;
        phys_verts.extend_from_slice(&[c_tr.to_array(), c_br.to_array(), n_tr.to_array(), n_br.to_array()]);
        phys_tris.push([pb, pb + 1, pb + 2]);
        phys_tris.push([pb + 1, pb + 3, pb + 2]);
    }

    // End caps for open splines
    if !closed && n >= 2 {
        // Front cap (at sample 0): BL, BR, TR, TL — normal = -forward
        let cap_normal = (samples[0].forward * -1.0).to_array();
        let [c_bl, c_br, c_tr, c_tl] = corners[0];
        let base = vertices.len() as u32;
        vertices.extend_from_slice(&[
            Vertex { position: c_bl.to_array(), normal: cap_normal, color, uv: [0.0, 0.0] },
            Vertex { position: c_br.to_array(), normal: cap_normal, color, uv: [1.0, 0.0] },
            Vertex { position: c_tr.to_array(), normal: cap_normal, color, uv: [1.0, 1.0] },
            Vertex { position: c_tl.to_array(), normal: cap_normal, color, uv: [0.0, 1.0] },
        ]);
        // Wind CCW when viewed from outside (facing -forward)
        indices.extend_from_slice(&[base, base + 2, base + 1, base, base + 3, base + 2]);
        let pb = phys_verts.len() as u32;
        phys_verts.extend_from_slice(&[c_bl.to_array(), c_br.to_array(), c_tr.to_array(), c_tl.to_array()]);
        phys_tris.push([pb, pb + 2, pb + 1]);
        phys_tris.push([pb, pb + 3, pb + 2]);

        // Back cap (at last sample): normal = +forward
        let last = n - 1;
        let cap_normal = samples[last].forward.to_array();
        let [c_bl, c_br, c_tr, c_tl] = corners[last];
        let base = vertices.len() as u32;
        vertices.extend_from_slice(&[
            Vertex { position: c_bl.to_array(), normal: cap_normal, color, uv: [0.0, 0.0] },
            Vertex { position: c_br.to_array(), normal: cap_normal, color, uv: [1.0, 0.0] },
            Vertex { position: c_tr.to_array(), normal: cap_normal, color, uv: [1.0, 1.0] },
            Vertex { position: c_tl.to_array(), normal: cap_normal, color, uv: [0.0, 1.0] },
        ]);
        indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
        let pb = phys_verts.len() as u32;
        phys_verts.extend_from_slice(&[c_bl.to_array(), c_br.to_array(), c_tr.to_array(), c_tl.to_array()]);
        phys_tris.push([pb, pb + 1, pb + 2]);
        phys_tris.push([pb, pb + 2, pb + 3]);
    }

    (vertices, indices, phys_verts, phys_tris)
}

/// Helper to set model.asset on an entity so the renderer draws it.
fn set_model_asset(world: &mut FlintWorld, entity_id: flint_core::EntityId, asset_name: &str) {
    if let Some(comps) = world.get_components_mut(entity_id) {
        comps.set_field("model", "asset", toml::Value::String(asset_name.to_string()));
    } else {
        let mut model_table = toml::map::Map::new();
        model_table.insert("asset".into(), toml::Value::String(asset_name.to_string()));
        let _ = world.set_component(entity_id, "model", toml::Value::Table(model_table));
    }
}

// ─── Main entry point ────────────────────────────────────

/// Load all spline data and generate geometry for spline_mesh entities.
///
/// Called from `PlayerApp::initialize()`. Scans the world for:
/// - Entities with a `spline` component (Pass 1: load + sample)
/// - Entities with a `spline_mesh` component (Pass 2: geometry)
pub fn load_splines(
    scene_path: &str,
    world: &mut FlintWorld,
    renderer: &mut SceneRenderer,
    mut physics: Option<&mut PhysicsSystem>,
    device: &wgpu::Device,
) {
    let scene_dir = Path::new(scene_path)
        .parent()
        .unwrap_or_else(|| Path::new("."));

    // ── Pass 1: Load spline data ──

    // Collect spline entity names and their sampled data
    // (we need to collect first to avoid borrowing world during iteration)
    struct SplineInfo {
        entity_id: flint_core::EntityId,
        entity_name: String,
        samples: Vec<SplineSample>,
        closed: bool,
    }
    let mut spline_infos: Vec<SplineInfo> = Vec::new();

    for entity in world.all_entities() {
        let spline_file = world
            .get_components(entity.id)
            .and_then(|c| c.get("spline").cloned())
            .and_then(|s| s.get("file").and_then(|v| v.as_str().map(String::from)));

        if let Some(file_rel) = spline_file {
            // Resolve path: scene dir, then parent
            let spline_path = {
                let p = scene_dir.join(&file_rel);
                if p.exists() {
                    p
                } else if let Some(parent) = scene_dir.parent() {
                    parent.join(&file_rel)
                } else {
                    p
                }
            };

            match parse_spline_file(&spline_path) {
                Some(def) => {
                    let samples = if def.closed {
                        spline::sample_closed_spline(&def.control_points, def.spacing)
                    } else {
                        spline::sample_open_spline(&def.control_points, def.spacing)
                    };

                    println!(
                        "Loaded spline '{}': {} control points → {} samples ({})",
                        entity.name,
                        def.control_points.len(),
                        samples.len(),
                        if def.closed { "closed" } else { "open" },
                    );

                    spline_infos.push(SplineInfo {
                        entity_id: entity.id,
                        entity_name: entity.name.clone(),
                        samples,
                        closed: def.closed,
                    });
                }
                None => {
                    eprintln!("Failed to parse spline file: {}", spline_path.display());
                }
            }
        }
    }

    // Store spline data on entities
    for info in &spline_infos {
        store_spline_data(world, info.entity_id, &info.samples, info.closed);
    }

    // Build lookup: entity name → index into spline_infos
    let spline_lookup: HashMap<&str, usize> = spline_infos
        .iter()
        .enumerate()
        .map(|(i, info)| (info.entity_name.as_str(), i))
        .collect();

    // ── Pass 2: Generate spline_mesh geometry ──

    // Collect mesh entities first
    struct MeshJob {
        entity_id: flint_core::EntityId,
        entity_name: String,
        def: SplineMeshDef,
    }
    let mut mesh_jobs: Vec<MeshJob> = Vec::new();

    for entity in world.all_entities() {
        let mesh_comp = world
            .get_components(entity.id)
            .and_then(|c| c.get("spline_mesh").cloned());

        if let Some(comp) = mesh_comp {
            match parse_spline_mesh_component(&comp) {
                Some(def) => {
                    mesh_jobs.push(MeshJob {
                        entity_id: entity.id,
                        entity_name: entity.name.clone(),
                        def,
                    });
                }
                None => {
                    eprintln!(
                        "Failed to parse spline_mesh on entity '{}'",
                        entity.name
                    );
                }
            }
        }
    }

    for job in &mesh_jobs {
        let spline_idx = match spline_lookup.get(job.def.spline_entity.as_str()) {
            Some(&idx) => idx,
            None => {
                eprintln!(
                    "spline_mesh '{}' references unknown spline entity '{}'",
                    job.entity_name, job.def.spline_entity
                );
                continue;
            }
        };
        let info = &spline_infos[spline_idx];

        let (verts, indices, phys_verts, phys_tris) = generate_cross_section_mesh(
            &info.samples,
            info.closed,
            job.def.width,
            job.def.height,
            job.def.offset,
            job.def.color,
        );

        if verts.is_empty() {
            continue;
        }

        println!(
            "  Mesh '{}': {}x{} @ offset [{:.1}, {:.1}] → {} verts, {} tris",
            job.entity_name,
            job.def.width,
            job.def.height,
            job.def.offset[0],
            job.def.offset[1],
            verts.len(),
            phys_tris.len(),
        );

        // Upload render mesh
        let material = ImportedMaterial {
            name: job.entity_name.clone(),
            base_color: job.def.color,
            metallic: job.def.metallic,
            roughness: job.def.roughness,
            base_color_texture: None,
            normal_texture: None,
            metallic_roughness_texture: None,
        };
        renderer.load_procedural_mesh(device, &job.entity_name, &verts, &indices, material);

        // Register trimesh collider (when physics is available)
        if let Some(ref mut phys) = physics {
            phys.sync.register_static_trimesh(
                job.entity_id,
                &mut phys.physics_world,
                phys_verts,
                phys_tris,
                job.def.friction,
                job.def.restitution,
            );
        }

        // Set model.asset so renderer draws it
        set_model_asset(world, job.entity_id, &job.entity_name);
    }

    if !spline_infos.is_empty() || !mesh_jobs.is_empty() {
        println!(
            "Spline generation complete: {} splines, {} meshes.",
            spline_infos.len(),
            mesh_jobs.len(),
        );
    }
}
