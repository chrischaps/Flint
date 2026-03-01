//! Spline-based procedural geometry.
//!
//! Two-pass initialization:
//! 1. Load `.spline.toml` files for entities with a `spline` component,
//!    sample the path, and store `spline_data` on each entity.
//! 2. For entities with a `spline_mesh` component, sweep a rectangular
//!    cross-section along the referenced spline and upload the resulting
//!    mesh + trimesh collider.

use flint_core::components as comp;
use flint_core::spline::{self, SplineControlPoint, SplineSample};
use flint_core::toml_util::{toml_f32, toml_f32_slice};
use flint_core::Vec3;
use flint_ecs::FlintWorld;
use flint_import::ImportedMaterial;
use flint_physics::PhysicsSystem;
use flint_render::{SceneRenderer, Vertex};
use std::collections::HashMap;
use std::path::Path;

// ─── Pass 1: load spline data ────────────────────────────

struct SplineDef {
    closed: bool,
    spacing: f32,
    control_points: Vec<SplineControlPoint>,
    gap_ranges: Vec<(f32, f32)>,
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
        let vals = toml_f32_slice(pos)?;
        let twist = pt.get("twist").and_then(toml_f32).unwrap_or(0.0);

        let gap_after = pt
            .get("gap_after")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        control_points.push(SplineControlPoint {
            position: Vec3::new(vals[0], vals[1], vals[2]),
            twist,
            gap_after,
        });
    }

    if control_points.len() < 2 {
        eprintln!(
            "Spline needs at least 2 control points, got {}",
            control_points.len()
        );
        return None;
    }

    let gap_ranges = spline::compute_gap_ranges(&control_points, closed);

    Some(SplineDef {
        closed,
        spacing,
        control_points,
        gap_ranges,
    })
}

/// Store sampled spline data as flat arrays in a `spline_data` ECS component.
fn store_spline_data(
    world: &mut FlintWorld,
    entity_id: flint_core::EntityId,
    samples: &[SplineSample],
    closed: bool,
    gap_ranges: &[(f32, f32)],
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

    // Gap ranges
    let gap_starts: Vec<toml::Value> = gap_ranges.iter().map(|(s, _)| to_val(*s)).collect();
    let gap_ends: Vec<toml::Value> = gap_ranges.iter().map(|(_, e)| to_val(*e)).collect();
    table.insert("gap_starts".into(), toml::Value::Array(gap_starts));
    table.insert("gap_ends".into(), toml::Value::Array(gap_ends));
    table.insert(
        "gap_count".into(),
        toml::Value::Integer(gap_ranges.len() as i64),
    );

    let _ = world.set_component(entity_id, comp::SPLINE_DATA, toml::Value::Table(table));
}

// ─── Pass 2: generate cross-section meshes ───────────────

struct SplineMeshDef {
    spline_entity: String,
    width: f32,
    height: f32,
    offset: [f32; 2], // [right, up]
    color: [f32; 4],
    stripe_color: Option<[f32; 4]>,
    stripe_width: f32,
    friction: f32,
    restitution: f32,
    metallic: f32,
    roughness: f32,
    chunk_size: u32, // 0 = no chunking (single mesh), >0 = segments per chunk
}

fn parse_spline_mesh_component(comp: &toml::Value) -> Option<SplineMeshDef> {
    let spline_entity = comp.get("spline")?.as_str()?.to_string();
    let width = comp.get("width").and_then(toml_f32)?;
    let height = comp.get("height").and_then(toml_f32)?;

    let offset = if let Some(arr) = comp.get("offset").and_then(|v| v.as_array()) {
        let vals = toml_f32_slice(arr)?;
        if vals.len() >= 2 {
            [vals[0], vals[1]]
        } else {
            [0.0, 0.0]
        }
    } else {
        [0.0, 0.0]
    };

    let color = if let Some(arr) = comp.get("color").and_then(|v| v.as_array()) {
        let vals = toml_f32_slice(arr)?;
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

    let stripe_color = if let Some(arr) = comp.get("stripe_color").and_then(|v| v.as_array()) {
        let vals = toml_f32_slice(arr)?;
        if vals.len() >= 4 {
            Some([vals[0], vals[1], vals[2], vals[3]])
        } else if vals.len() >= 3 {
            Some([vals[0], vals[1], vals[2], 1.0])
        } else {
            None
        }
    } else {
        None
    };

    let stripe_width = comp.get("stripe_width").and_then(toml_f32).unwrap_or(2.0);

    let friction = comp.get("friction").and_then(toml_f32).unwrap_or(0.5);
    let restitution = comp.get("restitution").and_then(toml_f32).unwrap_or(0.1);
    let metallic = comp.get("metallic").and_then(toml_f32).unwrap_or(0.0);
    let roughness = comp.get("roughness").and_then(toml_f32).unwrap_or(0.5);
    let chunk_size = comp
        .get("chunk_size")
        .and_then(|v| v.as_integer())
        .unwrap_or(0) as u32;

    Some(SplineMeshDef {
        spline_entity,
        width,
        height,
        offset,
        color,
        stripe_color,
        stripe_width,
        friction,
        restitution,
        metallic,
        roughness,
        chunk_size,
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
    stripe_color: Option<[f32; 4]>,
    stripe_width: f32,
    spacing: f32,
    gap_ranges: &[(f32, f32)],
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

    // Helper closure to generate an end cap at a given sample index.
    // `facing_forward = true` → cap faces +forward (back cap / departure cap)
    // `facing_forward = false` → cap faces -forward (front cap / arrival cap)
    let generate_end_cap = |idx: usize,
                            facing_forward: bool,
                            vertices: &mut Vec<Vertex>,
                            indices: &mut Vec<u32>,
                            phys_verts: &mut Vec<[f32; 3]>,
                            phys_tris: &mut Vec<[u32; 3]>| {
        let cap_fwd = if facing_forward {
            samples[idx].forward
        } else {
            samples[idx].forward * -1.0
        };
        let cap_normal = cap_fwd.to_array();
        let [c_bl, c_br, c_tr, c_tl] = corners[idx];
        let base = vertices.len() as u32;
        vertices.extend_from_slice(&[
            Vertex {
                position: c_bl.to_array(),
                normal: cap_normal,
                color,
                uv: [0.0, 0.0],
            },
            Vertex {
                position: c_br.to_array(),
                normal: cap_normal,
                color,
                uv: [1.0, 0.0],
            },
            Vertex {
                position: c_tr.to_array(),
                normal: cap_normal,
                color,
                uv: [1.0, 1.0],
            },
            Vertex {
                position: c_tl.to_array(),
                normal: cap_normal,
                color,
                uv: [0.0, 1.0],
            },
        ]);
        if facing_forward {
            // Back cap winding
            indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
        } else {
            // Front cap winding (CCW viewed from outside)
            indices.extend_from_slice(&[base, base + 2, base + 1, base, base + 3, base + 2]);
        }
        let pb = phys_verts.len() as u32;
        phys_verts.extend_from_slice(&[
            c_bl.to_array(),
            c_br.to_array(),
            c_tr.to_array(),
            c_tl.to_array(),
        ]);
        if facing_forward {
            phys_tris.push([pb, pb + 1, pb + 2]);
            phys_tris.push([pb, pb + 2, pb + 3]);
        } else {
            phys_tris.push([pb, pb + 2, pb + 1]);
            phys_tris.push([pb, pb + 3, pb + 2]);
        }
    };

    for seg in 0..num_segs {
        let next = if closed { (seg + 1) % n } else { seg + 1 };

        let seg_in_gap = spline::is_in_gap(samples[seg].t, gap_ranges);
        let next_in_gap = spline::is_in_gap(samples[next].t, gap_ranges);

        // Generate departure cap when entering a gap
        if !seg_in_gap && next_in_gap {
            generate_end_cap(
                seg,
                true,
                &mut vertices,
                &mut indices,
                &mut phys_verts,
                &mut phys_tris,
            );
        }

        // Generate arrival cap when exiting a gap
        if seg_in_gap && !next_in_gap {
            generate_end_cap(
                next,
                false,
                &mut vertices,
                &mut indices,
                &mut phys_verts,
                &mut phys_tris,
            );
        }

        // Skip geometry for segments inside gaps
        if seg_in_gap {
            continue;
        }

        let [c_bl, c_br, c_tr, c_tl] = corners[seg];
        let [n_bl, n_br, n_tr, n_tl] = corners[next];

        let u0 = seg as f32 / num_segs as f32;
        let u1 = (seg + 1) as f32 / num_segs as f32;

        // Per-segment color: alternate between primary and stripe color
        let seg_color = if let Some(sc) = stripe_color {
            let dist = seg as f32 * spacing;
            let stripe_index = (dist / stripe_width).floor() as i32;
            if stripe_index % 2 == 0 {
                color
            } else {
                sc
            }
        } else {
            color
        };

        // --- Top face (TL, TR → NTL, NTR) ---
        // Normal: average of up vectors
        let top_normal = ((samples[seg].up + samples[next].up) * 0.5).normalized();
        let base = vertices.len() as u32;
        vertices.extend_from_slice(&[
            Vertex {
                position: c_tl.to_array(),
                normal: top_normal.to_array(),
                color: seg_color,
                uv: [0.0, u0],
            },
            Vertex {
                position: c_tr.to_array(),
                normal: top_normal.to_array(),
                color: seg_color,
                uv: [1.0, u0],
            },
            Vertex {
                position: n_tl.to_array(),
                normal: top_normal.to_array(),
                color: seg_color,
                uv: [0.0, u1],
            },
            Vertex {
                position: n_tr.to_array(),
                normal: top_normal.to_array(),
                color: seg_color,
                uv: [1.0, u1],
            },
        ]);
        indices.extend_from_slice(&[base, base + 1, base + 2, base + 1, base + 3, base + 2]);
        let pb = phys_verts.len() as u32;
        phys_verts.extend_from_slice(&[
            c_tl.to_array(),
            c_tr.to_array(),
            n_tl.to_array(),
            n_tr.to_array(),
        ]);
        phys_tris.push([pb, pb + 1, pb + 2]);
        phys_tris.push([pb + 1, pb + 3, pb + 2]);

        // --- Bottom face (BL, BR → NBL, NBR) ---
        let bot_normal = ((samples[seg].up + samples[next].up) * -0.5).normalized();
        let base = vertices.len() as u32;
        vertices.extend_from_slice(&[
            Vertex {
                position: c_br.to_array(),
                normal: bot_normal.to_array(),
                color: seg_color,
                uv: [0.0, u0],
            },
            Vertex {
                position: c_bl.to_array(),
                normal: bot_normal.to_array(),
                color: seg_color,
                uv: [1.0, u0],
            },
            Vertex {
                position: n_br.to_array(),
                normal: bot_normal.to_array(),
                color: seg_color,
                uv: [0.0, u1],
            },
            Vertex {
                position: n_bl.to_array(),
                normal: bot_normal.to_array(),
                color: seg_color,
                uv: [1.0, u1],
            },
        ]);
        indices.extend_from_slice(&[base, base + 1, base + 2, base + 1, base + 3, base + 2]);
        let pb = phys_verts.len() as u32;
        phys_verts.extend_from_slice(&[
            c_br.to_array(),
            c_bl.to_array(),
            n_br.to_array(),
            n_bl.to_array(),
        ]);
        phys_tris.push([pb, pb + 1, pb + 2]);
        phys_tris.push([pb + 1, pb + 3, pb + 2]);

        // --- Left side face (TL, BL → NTL, NBL) ---
        let left_normal = ((samples[seg].right + samples[next].right) * -0.5).normalized();
        let base = vertices.len() as u32;
        vertices.extend_from_slice(&[
            Vertex {
                position: c_bl.to_array(),
                normal: left_normal.to_array(),
                color: seg_color,
                uv: [0.0, u0],
            },
            Vertex {
                position: c_tl.to_array(),
                normal: left_normal.to_array(),
                color: seg_color,
                uv: [1.0, u0],
            },
            Vertex {
                position: n_bl.to_array(),
                normal: left_normal.to_array(),
                color: seg_color,
                uv: [0.0, u1],
            },
            Vertex {
                position: n_tl.to_array(),
                normal: left_normal.to_array(),
                color: seg_color,
                uv: [1.0, u1],
            },
        ]);
        indices.extend_from_slice(&[base, base + 1, base + 2, base + 1, base + 3, base + 2]);
        let pb = phys_verts.len() as u32;
        phys_verts.extend_from_slice(&[
            c_bl.to_array(),
            c_tl.to_array(),
            n_bl.to_array(),
            n_tl.to_array(),
        ]);
        phys_tris.push([pb, pb + 1, pb + 2]);
        phys_tris.push([pb + 1, pb + 3, pb + 2]);

        // --- Right side face (BR, TR → NBR, NTR) ---
        let right_normal = ((samples[seg].right + samples[next].right) * 0.5).normalized();
        let base = vertices.len() as u32;
        vertices.extend_from_slice(&[
            Vertex {
                position: c_tr.to_array(),
                normal: right_normal.to_array(),
                color: seg_color,
                uv: [0.0, u0],
            },
            Vertex {
                position: c_br.to_array(),
                normal: right_normal.to_array(),
                color: seg_color,
                uv: [1.0, u0],
            },
            Vertex {
                position: n_tr.to_array(),
                normal: right_normal.to_array(),
                color: seg_color,
                uv: [0.0, u1],
            },
            Vertex {
                position: n_br.to_array(),
                normal: right_normal.to_array(),
                color: seg_color,
                uv: [1.0, u1],
            },
        ]);
        indices.extend_from_slice(&[base, base + 1, base + 2, base + 1, base + 3, base + 2]);
        let pb = phys_verts.len() as u32;
        phys_verts.extend_from_slice(&[
            c_tr.to_array(),
            c_br.to_array(),
            n_tr.to_array(),
            n_br.to_array(),
        ]);
        phys_tris.push([pb, pb + 1, pb + 2]);
        phys_tris.push([pb + 1, pb + 3, pb + 2]);
    }

    // End caps for open splines (only at actual spline endpoints, not gap edges)
    if !closed && n >= 2 {
        // Front cap at sample 0 (if not in a gap)
        if !spline::is_in_gap(samples[0].t, gap_ranges) {
            generate_end_cap(
                0,
                false,
                &mut vertices,
                &mut indices,
                &mut phys_verts,
                &mut phys_tris,
            );
        }

        // Back cap at last sample (if not in a gap)
        let last = n - 1;
        if !spline::is_in_gap(samples[last].t, gap_ranges) {
            generate_end_cap(
                last,
                true,
                &mut vertices,
                &mut indices,
                &mut phys_verts,
                &mut phys_tris,
            );
        }
    }

    (vertices, indices, phys_verts, phys_tris)
}

/// Helper to set model.asset on an entity so the renderer draws it.
fn set_model_asset(world: &mut FlintWorld, entity_id: flint_core::EntityId, asset_name: &str) {
    if let Some(comps) = world.get_components_mut(entity_id) {
        comps.set_field(
            comp::MODEL,
            "asset",
            toml::Value::String(asset_name.to_string()),
        );
    } else {
        let mut model_table = toml::map::Map::new();
        model_table.insert("asset".into(), toml::Value::String(asset_name.to_string()));
        let _ = world.set_component(entity_id, comp::MODEL, toml::Value::Table(model_table));
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
        spacing: f32,
        gap_ranges: Vec<(f32, f32)>,
    }
    let mut spline_infos: Vec<SplineInfo> = Vec::new();

    for entity in world.all_entities() {
        let spline_file = world
            .get_components(entity.id)
            .and_then(|c| c.get(comp::SPLINE).cloned())
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
                        spacing: def.spacing,
                        gap_ranges: def.gap_ranges,
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
        store_spline_data(
            world,
            info.entity_id,
            &info.samples,
            info.closed,
            &info.gap_ranges,
        );
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
            .and_then(|c| c.get(comp::SPLINE_MESH).cloned());

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
                    eprintln!("Failed to parse spline_mesh on entity '{}'", entity.name);
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

        if job.def.chunk_size > 0 {
            // ── Chunked mesh generation ──
            let chunk_sz = job.def.chunk_size as usize;
            let n = info.samples.len();
            let num_segs = if info.closed { n } else { n - 1 };
            let num_chunks = (num_segs + chunk_sz - 1) / chunk_sz;

            println!(
                "  Chunked mesh '{}': {}x{} @ offset [{:.1}, {:.1}], {} segments → {} chunks of {}",
                job.entity_name,
                job.def.width,
                job.def.height,
                job.def.offset[0],
                job.def.offset[1],
                num_segs,
                num_chunks,
                chunk_sz,
            );

            for chunk_i in 0..num_chunks {
                let seg_start = chunk_i * chunk_sz;
                let seg_end = ((chunk_i + 1) * chunk_sz).min(num_segs);

                // Sample range: include one extra sample for the last segment boundary
                let sample_start = seg_start;
                let sample_end = if info.closed {
                    // For closed splines, the last segment wraps around
                    seg_end + 1 // We'll handle wrapping below
                } else {
                    (seg_end + 1).min(n)
                };

                // Build sub-sample slice for this chunk, handling closed spline wrapping
                let chunk_samples: Vec<SplineSample> = if info.closed {
                    (sample_start..sample_end)
                        .map(|i| info.samples[i % n].clone())
                        .collect()
                } else {
                    info.samples[sample_start..sample_end].to_vec()
                };

                if chunk_samples.len() < 2 {
                    continue;
                }

                // Check if this entire chunk falls within a gap
                let chunk_all_in_gap = (seg_start..seg_end).all(|seg| {
                    let sample_idx = if info.closed { seg % n } else { seg };
                    spline::is_in_gap(info.samples[sample_idx].t, &info.gap_ranges)
                });
                if chunk_all_in_gap {
                    continue;
                }

                // Parametric t range for this chunk
                let t_start = chunk_samples.first().unwrap().t;
                let t_end = chunk_samples.last().unwrap().t;

                // Generate mesh for this chunk (treat as open — no wrapping within a chunk)
                let (verts, indices, phys_verts, phys_tris) = generate_cross_section_mesh(
                    &chunk_samples,
                    false, // chunks are always open segments
                    job.def.width,
                    job.def.height,
                    job.def.offset,
                    job.def.color,
                    job.def.stripe_color,
                    job.def.stripe_width,
                    info.spacing,
                    &info.gap_ranges,
                );

                if verts.is_empty() {
                    continue;
                }

                // Spawn child entity for this chunk
                let chunk_name = format!("{}__chunk_{}", job.entity_name, chunk_i);
                let chunk_id = match world.spawn(&chunk_name) {
                    Ok(id) => id,
                    Err(e) => {
                        eprintln!("Failed to spawn chunk entity '{}': {}", chunk_name, e);
                        continue;
                    }
                };

                // Set parent relationship
                let _ = world.set_parent(chunk_id, job.entity_id);

                // Set transform (identity — geometry is already in world space)
                let mut transform_table = toml::map::Map::new();
                transform_table.insert(
                    "position".into(),
                    toml::Value::Array(vec![
                        toml::Value::Float(0.0),
                        toml::Value::Float(0.0),
                        toml::Value::Float(0.0),
                    ]),
                );
                let _ = world.set_component(
                    chunk_id,
                    comp::TRANSFORM,
                    toml::Value::Table(transform_table),
                );

                // Set material component with opacity = 1.0
                let mut mat_table = toml::map::Map::new();
                mat_table.insert("opacity".into(), toml::Value::Float(1.0));
                let _ =
                    world.set_component(chunk_id, comp::MATERIAL, toml::Value::Table(mat_table));

                // Set spline_chunk component
                let mut chunk_table = toml::map::Map::new();
                chunk_table.insert("chunk_index".into(), toml::Value::Integer(chunk_i as i64));
                chunk_table.insert("t_start".into(), toml::Value::Float(t_start as f64));
                chunk_table.insert("t_end".into(), toml::Value::Float(t_end as f64));
                let _ = world.set_component(
                    chunk_id,
                    comp::SPLINE_CHUNK,
                    toml::Value::Table(chunk_table),
                );

                // Upload render mesh
                let material = ImportedMaterial {
                    name: chunk_name.clone(),
                    base_color: job.def.color,
                    metallic: job.def.metallic,
                    roughness: job.def.roughness,
                    base_color_texture: None,
                    normal_texture: None,
                    metallic_roughness_texture: None,
                    use_vertex_color: job.def.stripe_color.is_some(),
                    alpha_mode: flint_import::AlphaMode::Opaque,
                    alpha_cutoff: 0.5,
                };
                renderer.load_procedural_mesh(device, &chunk_name, &verts, &indices, material);

                // Register trimesh collider
                if let Some(ref mut phys) = physics {
                    phys.sync.register_static_trimesh(
                        chunk_id,
                        &mut phys.physics_world,
                        phys_verts,
                        phys_tris,
                        job.def.friction,
                        job.def.restitution,
                    );
                }

                // Set model.asset so renderer draws it
                set_model_asset(world, chunk_id, &chunk_name);
            }
            // Parent entity gets no mesh when chunking is active
        } else {
            // ── Original single-mesh path ──
            let (verts, indices, phys_verts, phys_tris) = generate_cross_section_mesh(
                &info.samples,
                info.closed,
                job.def.width,
                job.def.height,
                job.def.offset,
                job.def.color,
                job.def.stripe_color,
                job.def.stripe_width,
                info.spacing,
                &info.gap_ranges,
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
                use_vertex_color: job.def.stripe_color.is_some(),
                alpha_mode: flint_import::AlphaMode::Opaque,
                alpha_cutoff: 0.5,
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
    }

    if !spline_infos.is_empty() || !mesh_jobs.is_empty() {
        println!(
            "Spline generation complete: {} splines, {} meshes.",
            spline_infos.len(),
            mesh_jobs.len(),
        );
    }
}
