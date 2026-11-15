//! `rock_v1`: the cave game's own generator, which the engine doesn't ship.
//!
//! A boulder is a squashed sphere chiselled by a handful of planes. Each
//! cut is a half-space: every point past the plane is pressed back onto
//! it, so the rock keeps a convex body with flat, struck faces, and the
//! last cut always takes the bottom off so it sits on the ground. The mesh
//! is unwelded, one normal per face, which is what makes the facets read
//! under a lantern.

use flint_procgen::generators::util::{parse_hex_color, toml_f32, toml_string, toml_u32};
use flint_procgen::{
    BoundingBox, GenerationCost, Generator, GeneratorOutput, MaterialData, MeshData, OutputKind,
    ProcGenError, ProcGenSpec, Result, SeededRng, Vertex,
};

pub struct RockGenerator;

/// What a `rock_v1` spec's `[params]` say.
struct RockParams {
    /// Half the rock's width, in metres, before it is cut.
    radius: f32,
    /// How many planes chisel it, besides the one that flattens the base.
    cuts: u32,
    /// How deep a cut may go, as a fraction of the radius (0: none).
    depth: f32,
    /// Height over width: 1 is round, 0.5 a slab.
    squash: f32,
    /// The stone's colour, sRGB hex.
    color: [f32; 4],
    /// Surface roughness, 0 (polished) to 1 (chalk).
    roughness: f32,
}

impl Default for RockParams {
    fn default() -> Self {
        Self {
            radius: 0.5,
            cuts: 9,
            depth: 0.3,
            squash: 0.7,
            color: parse_hex_color("#6E675E").unwrap(),
            roughness: 0.9,
        }
    }
}

fn out_of_range(name: &str, reason: &str) -> ProcGenError {
    ProcGenError::InvalidParameter {
        name: name.into(),
        reason: reason.into(),
    }
}

impl RockParams {
    fn from_toml(params: &toml::Value) -> Result<Self> {
        let mut p = Self::default();
        let Some(t) = params.as_table() else {
            return Ok(p);
        };
        if let Some(v) = t.get("radius") {
            p.radius = toml_f32(v, "radius")?;
            if !(0.01..=50.0).contains(&p.radius) {
                return Err(out_of_range("radius", "must be 0.01 to 50 metres"));
            }
        }
        if let Some(v) = t.get("cuts") {
            p.cuts = toml_u32(v, "cuts")?;
            if p.cuts > 64 {
                return Err(out_of_range("cuts", "must be 0 to 64"));
            }
        }
        if let Some(v) = t.get("depth") {
            p.depth = toml_f32(v, "depth")?;
            if !(0.0..=0.8).contains(&p.depth) {
                return Err(out_of_range("depth", "must be 0 to 0.8"));
            }
        }
        if let Some(v) = t.get("squash") {
            p.squash = toml_f32(v, "squash")?;
            if !(0.2..=2.0).contains(&p.squash) {
                return Err(out_of_range("squash", "must be 0.2 to 2"));
            }
        }
        if let Some(v) = t.get("color") {
            p.color = parse_hex_color(&toml_string(v, "color")?)?;
        }
        if let Some(v) = t.get("roughness") {
            p.roughness = toml_f32(v, "roughness")?;
            if !(0.0..=1.0).contains(&p.roughness) {
                return Err(out_of_range("roughness", "must be 0 to 1"));
            }
        }
        Ok(p)
    }
}

// Coarse on purpose: big triangles between the cuts keep it low-poly.
const SEGMENTS: usize = 18;
const RINGS: usize = 12;
/// How many broad swellings break up the sphere before it is cut.
const LUMPS: usize = 5;

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

impl Generator for RockGenerator {
    fn type_name(&self) -> &str {
        "rock_v1"
    }

    fn revision(&self) -> u32 {
        1
    }

    fn output_kind(&self) -> OutputKind {
        OutputKind::Mesh
    }

    fn generate(&self, spec: &ProcGenSpec, rng: &mut SeededRng) -> Result<GeneratorOutput> {
        let p = RockParams::from_toml(&spec.params)?;
        let mut shape = rng.fork("shape");
        let mut chisel = rng.fork("chisel");

        // A sphere of unit radius, stretched a little differently on each
        // horizontal axis so no two rocks share a footprint.
        let stretch_x = shape.next_range(0.8, 1.2);
        let stretch_z = shape.next_range(0.8, 1.2);
        // A few broad swellings, so the body isn't a sphere before the cuts.
        let lumps: Vec<([f32; 3], f32)> = (0..LUMPS)
            .map(|_| (shape.next_vec3_unit(), shape.next_range(0.05, 0.22)))
            .collect();
        let mut points = Vec::with_capacity((RINGS + 1) * (SEGMENTS + 1));
        for j in 0..=RINGS {
            let phi = std::f32::consts::PI * j as f32 / RINGS as f32;
            for i in 0..=SEGMENTS {
                let theta = std::f32::consts::TAU * i as f32 / SEGMENTS as f32;
                let n = [phi.sin() * theta.cos(), phi.cos(), phi.sin() * theta.sin()];
                let swell: f32 = lumps
                    .iter()
                    .map(|(d, amount)| amount * dot(n, *d).max(0.0).powi(2))
                    .sum();
                let r = 1.0 + swell;
                points.push([
                    n[0] * r * stretch_x,
                    n[1] * r * p.squash,
                    n[2] * r * stretch_z,
                ]);
            }
        }

        // The cuts: a direction, and how far out along it the plane sits.
        // Leaning them upward a little keeps struck faces where light falls.
        let mut planes = Vec::with_capacity(p.cuts as usize + 1);
        for _ in 0..p.cuts {
            let mut d = chisel.next_vec3_unit();
            d[1] = d[1].abs() * 0.7 + 0.1;
            let len = dot(d, d).sqrt().max(1e-6);
            let d = [d[0] / len, d[1] / len, d[2] / len];
            let offset = 1.0 - p.depth * chisel.next_range(0.5, 1.0);
            planes.push((d, offset));
        }
        planes.push(([0.0, -1.0, 0.0], p.squash * 0.55));
        for point in &mut points {
            for (d, offset) in &planes {
                let past = dot(*point, *d) - offset;
                if past > 0.0 {
                    *point = sub(*point, [d[0] * past, d[1] * past, d[2] * past]);
                }
            }
        }

        // Sit it on the ground, and size it.
        let floor = points.iter().map(|q| q[1]).fold(f32::INFINITY, f32::min);
        for q in &mut points {
            *q = [q[0] * p.radius, (q[1] - floor) * p.radius, q[2] * p.radius];
        }

        // Unweld: three vertices a triangle, each with the face's normal.
        // Cuts collapse some triangles (near the poles, along a crease):
        // those are dropped.
        let mut vertices = Vec::new();
        let mut indices = Vec::new();
        let row = SEGMENTS + 1;
        let mut face = |a: [f32; 3], b: [f32; 3], c: [f32; 3]| {
            let n = cross(sub(b, a), sub(c, a));
            let len = dot(n, n).sqrt();
            if len < 1e-7 * p.radius * p.radius {
                return;
            }
            let n = [n[0] / len, n[1] / len, n[2] / len];
            let t = sub(b, a);
            let tl = dot(t, t).sqrt().max(1e-9);
            let tangent = [t[0] / tl, t[1] / tl, t[2] / tl, 1.0];
            for q in [a, b, c] {
                indices.push(vertices.len() as u32);
                vertices.push(Vertex {
                    position: q,
                    normal: n,
                    tangent,
                    uv: [q[0] + q[1], q[2] + q[1]],
                });
            }
        };
        for j in 0..RINGS {
            for i in 0..SEGMENTS {
                let a = points[j * row + i];
                let b = points[j * row + i + 1];
                let c = points[(j + 1) * row + i];
                let d = points[(j + 1) * row + i + 1];
                face(a, b, d);
                face(a, d, c);
            }
        }

        let positions: Vec<[f32; 3]> = vertices.iter().map(|v| v.position).collect();
        Ok(GeneratorOutput::Mesh(MeshData {
            vertices,
            indices,
            materials: vec![MaterialData {
                name: "stone".into(),
                base_color: p.color,
                metallic: 0.0,
                roughness: p.roughness,
                normal_map: None,
            }],
            submeshes: vec![],
            bounding_box: BoundingBox::from_positions(&positions),
        }))
    }

    fn param_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "radius": { "type": "number", "minimum": 0.01, "maximum": 50.0, "default": 0.5,
                    "description": "Half the rock's width in metres, before it is cut" },
                "cuts": { "type": "integer", "minimum": 0, "maximum": 64, "default": 9,
                    "description": "Planes that chisel it, besides the one that flattens the base" },
                "depth": { "type": "number", "minimum": 0.0, "maximum": 0.8, "default": 0.3,
                    "description": "How deep a cut may go, as a fraction of the radius" },
                "squash": { "type": "number", "minimum": 0.2, "maximum": 2.0, "default": 0.7,
                    "description": "Height over width: 1 is round, 0.5 a slab" },
                "color": { "type": "string", "default": "#6E675E",
                    "description": "The stone's colour, sRGB hex" },
                "roughness": { "type": "number", "minimum": 0.0, "maximum": 1.0, "default": 0.9,
                    "description": "Surface roughness, 0 polished to 1 chalk" }
            },
            "additionalProperties": false
        })
    }

    fn estimate_cost(&self, _spec: &ProcGenSpec) -> GenerationCost {
        let triangles = (SEGMENTS * RINGS * 2) as u32;
        GenerationCost {
            estimated_vertices: triangles * 3,
            estimated_triangles: triangles,
            estimated_texture_bytes: 0,
            estimated_generation_ms: 0.2,
        }
    }
}
