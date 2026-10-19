// crates/flint-render/src/water_body.rs
//! Water on any mesh (ADR 0094): lakes at their own level, rivers that run
//! and fall, each with its own tint, drawn with the ocean's shader.
//!
//! A body is a mesh of [`WaterVertex`]es, each carrying its rest position,
//! its surface normal and a flow (XZ, m/s). The ocean's vertex shader has a
//! second entry point, `vs_body`, that displaces such a mesh with the
//! ocean's wave spectrum (scaled by the body's `wave_scale`) and advects the
//! waves and foam along the flow. The fragment shader is the ocean's own, so
//! Fresnel, refraction, the cel ramp and the shoreline foam (ADR 0088/0091)
//! all carry over. Fast water foams in streaks along its flow.
//!
//! Bodies come from two places: the `water_body` scene component (a lake's
//! outline or a river's path), and [`crate::SceneRenderer::set_water_body`]
//! for a game's own mesh. Both draw after the sea, depth-tested, so a lake
//! above sea level hides the sea under it.

use bytemuck::{Pod, Zeroable};
use flint_core::toml_util::{toml_color, toml_f32};
use wgpu::util::DeviceExt;

/// A water mesh vertex: where the surface rests, which way is up for it,
/// and where the water there is going.
#[repr(C)]
#[derive(Copy, Clone, Debug, PartialEq, Pod, Zeroable)]
pub struct WaterVertex {
    /// The undisplaced surface point (world space).
    pub position: [f32; 3],
    /// The surface's normal at rest: up for a lake, tilted down a fall.
    pub normal: [f32; 3],
    /// The water's velocity in XZ, m/s. Zero for still water.
    pub flow: [f32; 2],
}

impl WaterVertex {
    const ATTRIBS: [wgpu::VertexAttribute; 3] =
        wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32x2];

    pub fn desc() -> wgpu::VertexBufferLayout<'static> {
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<WaterVertex>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &Self::ATTRIBS,
        }
    }
}

/// A water body's triangles.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct WaterBodyMesh {
    pub vertices: Vec<WaterVertex>,
    pub indices: Vec<u32>,
}

/// One point on a river's centre line.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RiverPoint {
    /// The surface on the centre line (world space).
    pub position: [f32; 3],
    /// Bank to bank, m.
    pub width: f32,
    /// How fast the water runs here mid-stream, m/s.
    pub speed: f32,
}

/// Columns of vertices across a river (so the banks can run slower).
const RIVER_ACROSS: u32 = 4;
/// A river's banks run at this share of its mid-stream speed.
const BANK_SPEED: f32 = 0.35;

impl WaterBodyMesh {
    /// Check the indices name real vertices and make whole triangles.
    pub fn validate(&self) -> Result<(), String> {
        if !self.indices.len().is_multiple_of(3) {
            return Err(format!(
                "water body has {} indices, not a whole number of triangles",
                self.indices.len()
            ));
        }
        if let Some(&i) = self
            .indices
            .iter()
            .find(|&&i| i as usize >= self.vertices.len())
        {
            return Err(format!(
                "water body index {i} is past its {} vertices",
                self.vertices.len()
            ));
        }
        Ok(())
    }

    /// Still water filling a simple outline (no holes, not crossing itself)
    /// at `level`. The outline is XZ points in either winding, not closed
    /// (the last point joins the first).
    pub fn lake(outline: &[[f32; 2]], level: f32) -> Result<Self, String> {
        let triangles = triangulate(outline)?;
        let vertices = outline
            .iter()
            .map(|p| WaterVertex {
                position: [p[0], level, p[1]],
                normal: [0.0, 1.0, 0.0],
                flow: [0.0, 0.0],
            })
            .collect();
        Ok(Self {
            vertices,
            indices: triangles,
        })
    }

    /// A ribbon down a river's centre line, flowing from the first point to
    /// the last. Where the line drops steeply the ribbon falls with it, and
    /// its normals tilt down the fall. The banks run slower than mid-stream.
    pub fn river(path: &[RiverPoint]) -> Result<Self, String> {
        if path.len() < 2 {
            return Err("a river needs at least two points".into());
        }
        // Resample so the ribbon is fine enough to carry the waves.
        let mut samples: Vec<RiverPoint> = Vec::new();
        for pair in path.windows(2) {
            let (a, b) = (pair[0], pair[1]);
            let len = dist3(a.position, b.position);
            let spacing = (a.width.min(b.width) / RIVER_ACROSS as f32).max(0.5);
            let steps = ((len / spacing).ceil() as usize).clamp(1, 4096);
            for s in 0..steps {
                let t = s as f32 / steps as f32;
                samples.push(RiverPoint {
                    position: lerp3(a.position, b.position, t),
                    width: a.width + (b.width - a.width) * t,
                    speed: a.speed + (b.speed - a.speed) * t,
                });
            }
        }
        samples.push(*path.last().unwrap());
        samples.dedup_by(|b, a| dist3(a.position, b.position) < 1e-4);
        if samples.len() < 2 {
            return Err("a river's points are all in one place".into());
        }

        let cols = RIVER_ACROSS + 1;
        let mut vertices = Vec::with_capacity(samples.len() * cols as usize);
        for (i, s) in samples.iter().enumerate() {
            let prev = samples[i.saturating_sub(1)].position;
            let next = samples[(i + 1).min(samples.len() - 1)].position;
            let along = sub3(next, prev);
            let horiz = [along[0], along[2]];
            let hl = (horiz[0] * horiz[0] + horiz[1] * horiz[1]).sqrt();
            if hl < 1e-5 {
                return Err(format!(
                    "river point {i} falls straight down; give it some run"
                ));
            }
            let dir = [horiz[0] / hl, horiz[1] / hl];
            // Across, to the river's left looking downstream.
            let across = [dir[1], 0.0, -dir[0]];
            let tangent = normalize3(along);
            let mut normal = normalize3(cross3(across, tangent));
            if normal[1] < 0.0 {
                normal = [-normal[0], -normal[1], -normal[2]];
            }
            for c in 0..cols {
                let u = c as f32 / RIVER_ACROSS as f32 * 2.0 - 1.0; // -1 bank, +1 bank
                let half = s.width * 0.5 * u;
                let speed = s.speed * (BANK_SPEED + (1.0 - BANK_SPEED) * (1.0 - u * u));
                vertices.push(WaterVertex {
                    position: [
                        s.position[0] + across[0] * half,
                        s.position[1],
                        s.position[2] + across[2] * half,
                    ],
                    normal,
                    flow: [dir[0] * speed, dir[1] * speed],
                });
            }
        }
        let mut indices = Vec::with_capacity((samples.len() - 1) * RIVER_ACROSS as usize * 6);
        for i in 0..samples.len() as u32 - 1 {
            for c in 0..RIVER_ACROSS {
                let a = i * cols + c;
                let b = a + 1;
                let d = a + cols;
                let e = d + 1;
                indices.extend_from_slice(&[a, d, b, b, d, e]);
            }
        }
        Ok(Self { vertices, indices })
    }
}

/// A river's points from a centre line with one width and speed, the speed
/// rising at the foot of a drop: `speed × (1 + fall_speedup × slope)`, the
/// slope being the drop over the run of the segment upstream of the point.
/// So water is calm at a fall's lip, fastest at its foot, and slows again
/// over the next segment.
pub fn river_points(
    path: &[[f32; 3]],
    width: f32,
    speed: f32,
    fall_speedup: f32,
) -> Vec<RiverPoint> {
    let slope = |a: [f32; 3], b: [f32; 3]| {
        let run = ((b[0] - a[0]).powi(2) + (b[2] - a[2]).powi(2))
            .sqrt()
            .max(1e-3);
        ((a[1] - b[1]).max(0.0) / run).min(10.0)
    };
    (0..path.len())
        .map(|i| {
            let upstream = if i > 0 {
                slope(path[i - 1], path[i])
            } else {
                0.0
            };
            RiverPoint {
                position: path[i],
                width,
                speed: speed * (1.0 + fall_speedup * upstream),
            }
        })
        .collect()
}

/// How a body looks where it differs from the sea. Everything else (foam,
/// refraction, sky reflection, the shoreline band) is the `ocean`
/// component's, or the defaults when the scene has none.
#[derive(Debug, Clone, PartialEq)]
pub struct WaterBodyLook {
    /// Water in wave shadow; `None` takes the ocean's.
    pub deep_color: Option<[f32; 4]>,
    /// Water in full light; `None` takes the ocean's.
    pub shallow_color: Option<[f32; 4]>,
    /// The ocean's waves, scaled: 0 is a mirror, 1 the open sea.
    pub wave_scale: f32,
    /// White water where the flow is fast: 0 none, 1 full.
    pub flow_foam: f32,
    /// Seconds between the two flow phases' resets. Longer moves the waves
    /// further before they blend away and pulse less; shorter stretches
    /// less on fast water.
    pub flow_period: f32,
}

impl Default for WaterBodyLook {
    fn default() -> Self {
        Self {
            deep_color: None,
            shallow_color: None,
            wave_scale: 0.3,
            flow_foam: 1.0,
            flow_period: 2.0,
        }
    }
}

impl WaterBodyLook {
    /// Read the look fields of a `water_body` component.
    pub fn from_component(value: &toml::Value) -> Self {
        let d = Self::default();
        let f = |name: &str, dv: f32| value.get(name).and_then(toml_f32).unwrap_or(dv);
        Self {
            deep_color: value.get("deep_color").and_then(toml_color),
            shallow_color: value.get("shallow_color").and_then(toml_color),
            wave_scale: f("wave_scale", d.wave_scale).clamp(0.0, 2.0),
            flow_foam: f("flow_foam", d.flow_foam).clamp(0.0, 2.0),
            flow_period: f("flow_period", d.flow_period).clamp(0.25, 30.0),
        }
    }

    /// The uniform block for this body, its missing colours taken from the
    /// sea's. `flowing` says whether any vertex moves (still water skips the
    /// flow's second wave evaluation).
    pub fn to_gpu(&self, sea: &crate::OceanVisuals, flowing: bool) -> WaterBodyUniformsGpu {
        WaterBodyUniformsGpu {
            deep_color: self.deep_color.unwrap_or(sea.deep_color),
            shallow_color: self.shallow_color.unwrap_or(sea.shallow_color),
            params: [
                self.wave_scale,
                if flowing { self.flow_foam } else { 0.0 },
                if flowing { self.flow_period } else { 0.0 },
                0.0,
            ],
        }
    }
}

/// Per-body uniforms: layout must match `BodyUniforms` in ocean_shader.wgsl.
#[repr(C)]
#[derive(Copy, Clone, Debug, PartialEq, Pod, Zeroable)]
pub struct WaterBodyUniformsGpu {
    pub deep_color: [f32; 4],
    pub shallow_color: [f32; 4],
    /// [wave_scale, flow_foam, flow_period (0 = still), 0].
    pub params: [f32; 4],
}

impl WaterBodyUniformsGpu {
    /// The sea itself, as a body: its own colours, full waves, no flow.
    pub fn sea(sea: &crate::OceanVisuals) -> Self {
        Self {
            deep_color: sea.deep_color,
            shallow_color: sea.shallow_color,
            params: [1.0, 0.0, 0.0, 0.0],
        }
    }
}

/// A body on the GPU.
pub(crate) struct GpuWaterBody {
    pub vertex_buffer: wgpu::Buffer,
    pub index_buffer: wgpu::Buffer,
    pub index_count: u32,
    pub uniform_buffer: wgpu::Buffer,
    /// Group 1: the shared ocean uniforms and this body's.
    pub bind_group: wgpu::BindGroup,
    pub look: WaterBodyLook,
    pub flowing: bool,
}

impl GpuWaterBody {
    pub fn new(
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        ocean_uniforms: &wgpu::Buffer,
        label: &str,
        mesh: &WaterBodyMesh,
        look: WaterBodyLook,
    ) -> Self {
        let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some(&format!("Water Body {label} Vertices")),
            contents: bytemuck::cast_slice(&mesh.vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some(&format!("Water Body {label} Indices")),
            contents: bytemuck::cast_slice(&mesh.indices),
            usage: wgpu::BufferUsages::INDEX,
        });
        let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(&format!("Water Body {label} Uniforms")),
            size: std::mem::size_of::<WaterBodyUniformsGpu>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: ocean_uniforms.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: uniform_buffer.as_entire_binding(),
                },
            ],
            label: Some(&format!("Water Body {label} Bind Group")),
        });
        let flowing = mesh
            .vertices
            .iter()
            .any(|v| v.flow[0] != 0.0 || v.flow[1] != 0.0);
        Self {
            vertex_buffer,
            index_buffer,
            index_count: mesh.indices.len() as u32,
            uniform_buffer,
            bind_group,
            look,
            flowing,
        }
    }
}

/// Build the mesh a `water_body` component describes: `shape = "lake"` with
/// an `outline` of [x, z] points and a `level`, or `shape = "river"` with a
/// `path` of [x, y, z] points, a `width`, a `speed` and a `fall_speedup`.
pub fn mesh_from_component(value: &toml::Value) -> Result<WaterBodyMesh, String> {
    let f = |name: &str, dv: f32| value.get(name).and_then(toml_f32).unwrap_or(dv);
    let points = |name: &str, n: usize| -> Result<Vec<Vec<f32>>, String> {
        let arr = value
            .get(name)
            .and_then(|v| v.as_array())
            .ok_or_else(|| format!("water_body needs a `{name}` array"))?;
        arr.iter()
            .map(|p| {
                p.as_array()
                    .and_then(|a| flint_core::toml_util::toml_f32_slice(a))
                    .filter(|a| a.len() == n)
                    .ok_or_else(|| format!("each `{name}` point needs {n} numbers"))
            })
            .collect()
    };
    match value
        .get("shape")
        .and_then(|v| v.as_str())
        .unwrap_or("lake")
    {
        "lake" => {
            let outline: Vec<[f32; 2]> =
                points("outline", 2)?.iter().map(|p| [p[0], p[1]]).collect();
            WaterBodyMesh::lake(&outline, f("level", 0.0))
        }
        "river" => {
            let path: Vec<[f32; 3]> = points("path", 3)?
                .iter()
                .map(|p| [p[0], p[1], p[2]])
                .collect();
            WaterBodyMesh::river(&river_points(
                &path,
                f("width", 4.0).max(0.05),
                f("speed", 1.0).max(0.0),
                f("fall_speedup", 3.0).max(0.0),
            ))
        }
        other => Err(format!(
            "water_body shape `{other}` is neither lake nor river"
        )),
    }
}

// ── Triangulation (ear clipping, for a lake's simple outline) ───────────

fn triangulate(ring: &[[f32; 2]]) -> Result<Vec<u32>, String> {
    let n = ring.len();
    if n < 3 {
        return Err("a lake's outline needs at least three points".into());
    }
    let area: f32 = (0..n)
        .map(|i| {
            let (a, b) = (ring[i], ring[(i + 1) % n]);
            a[0] * b[1] - b[0] * a[1]
        })
        .sum::<f32>()
        * 0.5;
    if area.abs() < 1e-6 {
        return Err("a lake's outline encloses no area".into());
    }
    // Work counter-clockwise in (x, z).
    let mut idx: Vec<usize> = (0..n).collect();
    if area < 0.0 {
        idx.reverse();
    }
    let cross = |o: [f32; 2], a: [f32; 2], b: [f32; 2]| {
        (a[0] - o[0]) * (b[1] - o[1]) - (a[1] - o[1]) * (b[0] - o[0])
    };
    let mut out = Vec::with_capacity((n - 2) * 3);
    let mut guard = 0usize;
    while idx.len() > 3 {
        let m = idx.len();
        let mut clipped = false;
        for i in 0..m {
            let (ia, ib, ic) = (idx[(i + m - 1) % m], idx[i], idx[(i + 1) % m]);
            let (a, b, c) = (ring[ia], ring[ib], ring[ic]);
            if cross(a, b, c) <= 0.0 {
                continue; // reflex or flat: not an ear
            }
            let inside = idx.iter().any(|&j| {
                if j == ia || j == ib || j == ic {
                    return false;
                }
                let p = ring[j];
                cross(a, b, p) >= 0.0 && cross(b, c, p) >= 0.0 && cross(c, a, p) >= 0.0
            });
            if inside {
                continue;
            }
            out.extend_from_slice(&[ia as u32, ib as u32, ic as u32]);
            idx.remove(i);
            clipped = true;
            break;
        }
        if !clipped {
            guard += 1;
            // Only collinear runs are left: drop a flat vertex and go on.
            let m = idx.len();
            let flat = (0..m).find(|&i| {
                let (a, b, c) = (
                    ring[idx[(i + m - 1) % m]],
                    ring[idx[i]],
                    ring[idx[(i + 1) % m]],
                );
                cross(a, b, c).abs() < 1e-9
            });
            match flat {
                Some(i) if guard < n => {
                    idx.remove(i);
                }
                _ => return Err("a lake's outline crosses itself".into()),
            }
        }
    }
    out.extend(idx.iter().map(|&i| i as u32));
    Ok(out)
}

fn sub3(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn lerp3(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
    ]
}

fn dist3(a: [f32; 3], b: [f32; 3]) -> f32 {
    let d = sub3(a, b);
    (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt()
}

fn cross3(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn normalize3(a: [f32; 3]) -> [f32; 3] {
    let l = (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt().max(1e-12);
    [a[0] / l, a[1] / l, a[2] / l]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tri_area(m: &WaterBodyMesh) -> f32 {
        m.indices
            .chunks(3)
            .map(|t| {
                let p = |i: u32| m.vertices[i as usize].position;
                let (a, b, c) = (p(t[0]), p(t[1]), p(t[2]));
                ((b[0] - a[0]) * (c[2] - a[2]) - (b[2] - a[2]) * (c[0] - a[0])).abs() * 0.5
            })
            .sum()
    }

    #[test]
    fn uniform_struct_matches_wgsl_layout() {
        assert_eq!(std::mem::size_of::<WaterBodyUniformsGpu>(), 48);
        assert_eq!(std::mem::size_of::<WaterVertex>(), 32);
    }

    #[test]
    fn a_lake_fills_its_outline_at_its_level() {
        // An L, concave, wound clockwise: area 3.
        let l = [
            [0.0, 0.0],
            [0.0, 2.0],
            [1.0, 2.0],
            [1.0, 1.0],
            [2.0, 1.0],
            [2.0, 0.0],
        ];
        let m = WaterBodyMesh::lake(&l, 7.5).unwrap();
        m.validate().unwrap();
        assert_eq!(m.indices.len(), 4 * 3);
        assert!((tri_area(&m) - 3.0).abs() < 1e-4);
        assert!(m
            .vertices
            .iter()
            .all(|v| v.position[1] == 7.5 && v.flow == [0.0, 0.0]));
    }

    #[test]
    fn a_lake_outline_with_collinear_points_still_fills() {
        let sq = [
            [0.0, 0.0],
            [1.0, 0.0],
            [2.0, 0.0],
            [2.0, 2.0],
            [0.0, 2.0],
            [0.0, 1.0],
        ];
        let m = WaterBodyMesh::lake(&sq, 0.0).unwrap();
        assert!((tri_area(&m) - 4.0).abs() < 1e-4);
    }

    #[test]
    fn a_degenerate_lake_is_refused() {
        assert!(WaterBodyMesh::lake(&[[0.0, 0.0], [1.0, 1.0]], 0.0).is_err());
        assert!(WaterBodyMesh::lake(&[[0.0, 0.0], [1.0, 1.0], [2.0, 2.0]], 0.0).is_err());
    }

    #[test]
    fn a_river_flows_downstream_slower_at_its_banks() {
        let pts = river_points(&[[0.0, 5.0, 0.0], [0.0, 5.0, 20.0]], 4.0, 1.5, 3.0);
        let m = WaterBodyMesh::river(&pts).unwrap();
        m.validate().unwrap();
        // 4 m wide and 20 long, as a ribbon.
        assert!((tri_area(&m) - 80.0).abs() < 1e-2);
        for v in &m.vertices {
            assert!(
                v.flow[1] > 0.0 && v.flow[0].abs() < 1e-5,
                "flows +Z: {:?}",
                v.flow
            );
            assert!((v.position[1] - 5.0).abs() < 1e-5);
            assert!(v.position[0].abs() <= 2.0 + 1e-4);
        }
        let mid = m.vertices.iter().map(|v| v.flow[1]).fold(0.0, f32::max);
        let bank = m
            .vertices
            .iter()
            .map(|v| v.flow[1])
            .fold(f32::MAX, f32::min);
        assert!((mid - 1.5).abs() < 1e-4);
        assert!((bank - 1.5 * BANK_SPEED).abs() < 1e-4);
    }

    #[test]
    fn a_river_falls_faster_and_tilts_down_its_step() {
        // Level, a 2 m drop over 1 m, level again.
        let path = [
            [0.0, 6.0, 0.0],
            [0.0, 6.0, 10.0],
            [0.0, 4.0, 11.0],
            [0.0, 4.0, 20.0],
        ];
        let pts = river_points(&path, 3.0, 1.0, 3.0);
        assert_eq!(pts[0].speed, 1.0);
        assert_eq!(pts[1].speed, 1.0, "calm at the lip");
        assert!(
            (pts[2].speed - 7.0).abs() < 1e-4,
            "slope 2 at the foot → 1 + 3×2"
        );
        assert_eq!(pts[3].speed, 1.0, "slow again downstream");
        let m = WaterBodyMesh::river(&pts).unwrap();
        // On the step the normal leans downstream (+Z) and is still up-ish.
        let on_step = m
            .vertices
            .iter()
            .find(|v| v.position[1] < 5.5 && v.position[1] > 4.5)
            .expect("a vertex on the step");
        assert!(
            on_step.normal[2] > 0.5 && on_step.normal[1] > 0.0,
            "{:?}",
            on_step.normal
        );
        // Away from it, flat water is up.
        let flat = m.vertices.iter().find(|v| v.position[2] > 15.0).unwrap();
        assert!((flat.normal[1] - 1.0).abs() < 1e-4);
    }

    #[test]
    fn a_component_builds_either_shape() {
        let lake: toml::Value = toml::from_str(
            "shape = 'lake'\nlevel = 3.0\noutline = [[0, 0], [4, 0], [4, 4], [0, 4]]",
        )
        .unwrap();
        let m = mesh_from_component(&lake).unwrap();
        assert!((tri_area(&m) - 16.0).abs() < 1e-4);
        let river: toml::Value =
            toml::from_str("shape = 'river'\nwidth = 2\npath = [[0, 1, 0], [10, 1, 0]]").unwrap();
        assert!(mesh_from_component(&river).unwrap().vertices.len() > 10);
        let bad: toml::Value = toml::from_str("shape = 'pond'").unwrap();
        assert!(mesh_from_component(&bad).is_err());
    }

    #[test]
    fn still_water_needs_no_flow_pass() {
        let sea = crate::OceanVisuals::default();
        let look = WaterBodyLook {
            deep_color: Some([0.1, 0.2, 0.3, 1.0]),
            ..Default::default()
        };
        let still = look.to_gpu(&sea, false);
        assert_eq!(still.deep_color, [0.1, 0.2, 0.3, 1.0]);
        assert_eq!(still.shallow_color, sea.shallow_color);
        assert_eq!(still.params[1..3], [0.0, 0.0]);
        assert_eq!(look.to_gpu(&sea, true).params[2], 2.0);
    }
}
