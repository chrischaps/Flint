//! `sweep_v1`: a profile swept along a path (#750). A rail, a pipe, a
//! moulding, a gutter, a ribbon of road: any cross-section that follows a
//! Catmull-Rom curve.
//!
//! The geometry is `flint_geometry::sweep`'s, the same that draws a scene's
//! `spline_mesh`, so a spec and a component with the same path and profile
//! make the same triangles. The spec carries its path inline (`[[params.
//! control_points]]`, written as a `.spline.toml`'s are), so the spec's
//! bytes are the whole input and the procgen cache can't serve a stale mesh
//! after the path changes. A `spline_mesh` in a scene is the way to sweep
//! along a spline file, with a collider; this is the way to make one model
//! of a swept shape, for `flint gen`, the previewer and a scene's
//! `model.asset`.

use flint_core::Vec3;
use flint_geometry::spline::{self, SplineControlPoint};
use flint_geometry::sweep::{self, Profile, SweepPath, SweepStyle};

use crate::algorithms::mesh_builder::tangents::compute_tangents;
use crate::generator::{GenerationCost, Generator};
use crate::generators::util::{parse_hex_color, toml_f32, toml_string};
use crate::output::{GeneratorOutput, OutputKind};
use crate::spec::ProcGenSpec;
use crate::types::{BoundingBox, MaterialData, MeshData, SubMesh, Vertex};
use crate::{ProcGenError, Result, SeededRng};

/// The sweep generator. See the module docs.
pub struct SweepGenerator;

/// The most vertices one sweep may make, so a spacing typed a thousand
/// times too fine is refused rather than filling memory.
const MAX_VERTICES: usize = 4_000_000;

/// `[params]` of a `sweep_v1` spec, read.
#[derive(Debug, Clone)]
pub struct SweepParams {
    pub control_points: Vec<SplineControlPoint>,
    pub closed: bool,
    pub spacing: f32,
    pub profile: Profile,
    pub offset: [f32; 2],
    pub texture_length: f32,
    pub color: [f32; 4],
    pub metallic: f32,
    pub roughness: f32,
}

fn invalid(name: &str, reason: impl Into<String>) -> ProcGenError {
    ProcGenError::InvalidParameter {
        name: name.into(),
        reason: reason.into(),
    }
}

/// `[a, b, ...]` of numbers, exactly `n` of them.
fn numbers(v: &toml::Value, name: &str, n: usize) -> Result<Vec<f32>> {
    let arr = v
        .as_array()
        .ok_or_else(|| invalid(name, format!("expected [{}]", vec!["n"; n].join(", "))))?;
    if arr.len() != n {
        return Err(invalid(
            name,
            format!("has {} numbers, expected {n}", arr.len()),
        ));
    }
    arr.iter().map(|x| toml_f32(x, name)).collect()
}

impl SweepParams {
    /// Read `[params]`: a missing key takes its default, a bad one is an
    /// error naming it.
    pub fn from_toml(params: &toml::Value) -> Result<Self> {
        let empty = toml::map::Map::new();
        let t = params.as_table().unwrap_or(&empty);
        let num = |key: &str, default: f32| -> Result<f32> {
            t.get(key).map_or(Ok(default), |v| toml_f32(v, key))
        };
        let flag = |key: &str, default: bool| -> Result<bool> {
            match t.get(key) {
                None => Ok(default),
                Some(v) => v
                    .as_bool()
                    .ok_or_else(|| invalid(key, "expected true or false")),
            }
        };

        let points = t
            .get("control_points")
            .ok_or_else(|| ProcGenError::MissingParameter("control_points".into()))?
            .as_array()
            .ok_or_else(|| {
                invalid(
                    "control_points",
                    "expected [[params.control_points]] tables",
                )
            })?;
        let mut control_points = Vec::with_capacity(points.len());
        for (i, point) in points.iter().enumerate() {
            let name = format!("control_points[{i}]");
            let p = point
                .as_table()
                .ok_or_else(|| invalid(&name, "expected a table with a `position`"))?;
            let position = p
                .get("position")
                .ok_or_else(|| invalid(&name, "has no `position`"))?;
            let xyz = numbers(position, &format!("{name}.position"), 3)?;
            let twist = p
                .get("twist")
                .map_or(Ok(0.0), |v| toml_f32(v, &format!("{name}.twist")))?;
            let gap_after = match p.get("gap_after") {
                None => false,
                Some(v) => v.as_bool().ok_or_else(|| {
                    invalid(&format!("{name}.gap_after"), "expected true or false")
                })?,
            };
            control_points.push(SplineControlPoint {
                position: Vec3::new(xyz[0], xyz[1], xyz[2]),
                twist,
                gap_after,
            });
        }
        if control_points.len() < 2 {
            return Err(invalid(
                "control_points",
                format!("a path needs at least 2, got {}", control_points.len()),
            ));
        }

        let spacing = num("spacing", 0.5)?;
        if !(spacing.is_finite() && spacing >= 0.01) {
            return Err(invalid(
                "spacing",
                format!("{spacing}: must be 0.01 or more"),
            ));
        }

        let smooth = flag("smooth", false)?;
        let profile = match t.get("profile") {
            Some(v) => {
                let arr = v
                    .as_array()
                    .ok_or_else(|| invalid("profile", "expected [[right, up], ...]"))?;
                let points = arr
                    .iter()
                    .enumerate()
                    .map(|(i, p)| {
                        let xy = numbers(p, &format!("profile[{i}]"), 2)?;
                        Ok([xy[0], xy[1]])
                    })
                    .collect::<Result<Vec<[f32; 2]>>>()?;
                Profile::new(points, flag("profile_closed", true)?, smooth)
                    .map_err(|why| invalid("profile", why))?
            }
            None => {
                let (w, h) = (num("width", 0.2)?, num("height", 0.2)?);
                if !(w > 0.0 && h > 0.0) {
                    return Err(invalid("width", "a box's width and height must be above 0"));
                }
                Profile::new(Profile::rect(w, h).points().to_vec(), true, smooth)
                    .map_err(|why| invalid("width", why))?
            }
        };

        let offset = match t.get("offset") {
            Some(v) => {
                let xy = numbers(v, "offset", 2)?;
                [xy[0], xy[1]]
            }
            None => [0.0, 0.0],
        };
        let texture_length = match t.get("texture_length") {
            Some(v) => {
                let l = toml_f32(v, "texture_length")?;
                if l.is_nan() || l <= 0.0 {
                    return Err(invalid("texture_length", "must be above 0"));
                }
                l
            }
            None => profile.width().max(0.01),
        };
        let color = match t.get("color") {
            Some(v) => parse_hex_color(&toml_string(v, "color")?)?,
            None => [0.5, 0.5, 0.5, 1.0],
        };
        let unit = |key: &str, default: f32| -> Result<f32> {
            let x = num(key, default)?;
            if !(0.0..=1.0).contains(&x) {
                return Err(invalid(key, format!("{x}: must be 0 to 1")));
            }
            Ok(x)
        };

        Ok(SweepParams {
            control_points,
            closed: flag("closed", false)?,
            spacing,
            profile,
            offset,
            texture_length,
            color,
            metallic: unit("metallic", 0.0)?,
            roughness: unit("roughness", 0.5)?,
        })
    }

    /// The path's length along its control points, a little under the
    /// curve's: for the cost estimate.
    fn rough_length(&self) -> f32 {
        let pts = &self.control_points;
        let mut length: f32 = pts
            .windows(2)
            .map(|w| (w[1].position - w[0].position).length())
            .sum();
        if self.closed {
            length += (pts[0].position - pts[pts.len() - 1].position).length();
        }
        length
    }

    fn vertices_per_segment(&self) -> usize {
        let n = self.profile.points().len();
        let edges = if self.profile.is_closed() { n } else { n - 1 };
        edges * 4
    }
}

impl Generator for SweepGenerator {
    fn type_name(&self) -> &str {
        "sweep_v1"
    }

    fn revision(&self) -> u32 {
        1
    }

    fn output_kind(&self) -> OutputKind {
        OutputKind::Mesh
    }

    fn generate(&self, spec: &ProcGenSpec, _rng: &mut SeededRng) -> Result<GeneratorOutput> {
        let p = SweepParams::from_toml(&spec.params)?;
        let estimate = (p.rough_length() * 1.6 / p.spacing) as usize * p.vertices_per_segment();
        if estimate > MAX_VERTICES {
            return Err(ProcGenError::BudgetExceeded(format!(
                "about {estimate} vertices at spacing {}: more than {MAX_VERTICES}; \
                 raise `spacing` or simplify `profile`",
                p.spacing
            )));
        }
        let samples = if p.closed {
            spline::sample_closed_spline(&p.control_points, p.spacing)
        } else {
            spline::sample_open_spline(&p.control_points, p.spacing)
        };
        let gaps = spline::compute_gap_ranges(&p.control_points, p.closed);
        let swept = sweep::sweep(
            &SweepPath {
                samples: &samples,
                closed: p.closed,
                spacing: p.spacing,
                gaps: &gaps,
            },
            &p.profile,
            &SweepStyle {
                offset: p.offset,
                color: p.color,
                stripes: None,
                texture_length: p.texture_length,
            },
        );
        if swept.is_empty() {
            return Err(ProcGenError::InvalidMesh(
                "the sweep made no geometry: the whole path is in a gap".into(),
            ));
        }

        let mut vertices: Vec<Vertex> = swept
            .vertices
            .iter()
            .map(|v| Vertex {
                position: v.position,
                normal: v.normal,
                tangent: [1.0, 0.0, 0.0, 1.0],
                uv: v.uv,
            })
            .collect();
        compute_tangents(&mut vertices, &swept.indices);
        let positions: Vec<[f32; 3]> = vertices.iter().map(|v| v.position).collect();
        let index_count = swept.indices.len() as u32;
        Ok(GeneratorOutput::Mesh(MeshData {
            vertices,
            indices: swept.indices,
            materials: vec![MaterialData {
                name: spec.meta.name.clone(),
                base_color: p.color,
                metallic: p.metallic,
                roughness: p.roughness,
                normal_map: None,
            }],
            submeshes: vec![SubMesh {
                index_start: 0,
                index_count,
                material_index: 0,
            }],
            bounding_box: BoundingBox::from_positions(&positions),
        }))
    }

    fn estimate_cost(&self, spec: &ProcGenSpec) -> GenerationCost {
        let Ok(p) = SweepParams::from_toml(&spec.params) else {
            return GenerationCost::default();
        };
        let segments = (p.rough_length() / p.spacing).ceil() as u32;
        let per = p.vertices_per_segment() as u32;
        GenerationCost {
            estimated_vertices: segments * per,
            estimated_triangles: segments * per / 2,
            ..Default::default()
        }
    }

    fn param_schema(&self) -> serde_json::Value {
        let point = serde_json::json!({ "type": "array", "items": { "type": "number" },
            "minItems": 2, "maxItems": 2 });
        serde_json::json!({
            "type": "object",
            "description": "A profile swept along a Catmull-Rom path: a rail, a pipe, a moulding, a gutter. The same sweep a scene's `spline_mesh` draws, as one model.",
            "required": ["control_points"],
            "properties": {
                "control_points": { "type": "array", "minItems": 2,
                    "items": { "type": "object", "required": ["position"], "properties": {
                        "position": { "type": "array", "items": { "type": "number" }, "minItems": 3, "maxItems": 3,
                            "description": "[x, y, z] metres" },
                        "twist": { "type": "number", "default": 0.0,
                            "description": "Banking about the path here, degrees" },
                        "gap_after": { "type": "boolean", "default": false,
                            "description": "No geometry from this point to the next" } } },
                    "description": "The path, as `[[params.control_points]]` tables written as a `.spline.toml`'s are, at least two" },
                "closed": { "type": "boolean", "default": false,
                    "description": "A loop: the last control point joins the first, and there are no end caps. A spline file's `[spline] closed` defaults to true; this to false, since a swept model is more often a run with two ends" },
                "spacing": { "type": "number", "minimum": 0.01, "maximum": 100.0, "default": 0.5,
                    "description": "Metres between the path's samples: smaller is a smoother bend and more triangles" },
                "profile": { "type": "array", "items": point, "minItems": 2,
                    "description": "The cross-section as [right, up] points in metres from the path, in place of `width` and `height`: closed, a solid whose faces look outward whichever way round it goes; open, a surface facing the left of the way it is listed" },
                "profile_closed": { "type": "boolean", "default": true,
                    "description": "False makes `profile` an open surface, with no closing edge and no caps" },
                "smooth": { "type": "boolean", "default": false,
                    "description": "Shade the profile round instead of faceted, with texture u once around it: a pipe or a rail" },
                "width": { "type": "number", "exclusiveMinimum": 0.0, "maximum": 100.0, "default": 0.2,
                    "description": "Without a `profile`, the width of a box cross-section, metres" },
                "height": { "type": "number", "exclusiveMinimum": 0.0, "maximum": 100.0, "default": 0.2,
                    "description": "Without a `profile`, the height of a box cross-section, metres" },
                "offset": { "type": "array", "items": { "type": "number" }, "minItems": 2, "maxItems": 2,
                    "default": [0.0, 0.0],
                    "description": "[right, up] metres from the path to the profile's centre, in the path's own frame" },
                "texture_length": { "type": "number", "exclusiveMinimum": 0.0, "maximum": 1000.0,
                    "description": "Metres of sweep per texture repeat (v); on a loop nudged so the repeats meet at the seam. Unset: the profile's width" },
                "color": { "type": "string", "pattern": "^#?[0-9a-fA-F]{6,8}$", "default": "#808080",
                    "description": "Base colour, sRGB hex" },
                "metallic": { "type": "number", "minimum": 0.0, "maximum": 1.0, "default": 0.0,
                    "description": "PBR metallic" },
                "roughness": { "type": "number", "minimum": 0.0, "maximum": 1.0, "default": 0.5,
                    "description": "PBR roughness" }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::GeneratorRegistry;

    const RAIL: &str = r##"
generator = "sweep_v1"

[meta]
name = "rail"
version = "1.0.0"

[seed]
mode = "fixed"
value = 1

[params]
spacing = 0.25
smooth = true
profile = [[0.05, 0.0], [0.035, 0.035], [0.0, 0.05], [-0.035, 0.035], [-0.05, 0.0], [-0.035, -0.035], [0.0, -0.05], [0.035, -0.035]]
color = "#b08d57"
metallic = 1.0
roughness = 0.3

[[params.control_points]]
position = [0, 1, 0]

[[params.control_points]]
position = [2, 1.2, 0]

[[params.control_points]]
position = [4, 1, 1]
"##;

    fn generate(text: &str) -> Result<MeshData> {
        let mut reg = GeneratorRegistry::new();
        reg.register(Box::new(SweepGenerator));
        let spec = ProcGenSpec::parse(text).unwrap();
        match reg.generate_from_spec(&spec)? {
            GeneratorOutput::Mesh(mesh) => Ok(mesh),
            other => panic!("not a mesh: {:?}", other.kind()),
        }
    }

    #[test]
    fn a_tube_sweeps_into_a_valid_capped_mesh() {
        let mesh = generate(RAIL).unwrap();
        mesh.validate().unwrap();
        assert!(mesh.triangle_count() > 100);
        assert_eq!(mesh.materials[0].metallic, 1.0);
        // the rail runs from x = 0 to 4, its 5 cm radius about the path
        let b = mesh.bounding_box;
        assert!(
            b.min.x.abs() < 0.06 && (b.max.x - 4.0).abs() < 0.06,
            "{b:?}"
        );
        assert!(b.min.y > 0.9 && b.max.y < 1.3, "{b:?}");
        // tangents are unit and lie across the normals (an open path's
        // last ring has no frame yet, #1016: its vertices are left out)
        for v in mesh.vertices.iter().filter(|v| v.normal != [0.0; 3]) {
            let t = Vec3::new(v.tangent[0], v.tangent[1], v.tangent[2]);
            let n = Vec3::from_array(v.normal);
            assert!((t.length() - 1.0).abs() < 1e-3, "{v:?}");
            assert!(t.dot(&n).abs() < 1e-3, "{v:?}");
        }
    }

    #[test]
    fn the_same_spec_makes_the_same_bytes() {
        assert_eq!(generate(RAIL).unwrap(), generate(RAIL).unwrap());
    }

    #[test]
    fn a_spec_without_a_profile_sweeps_a_box() {
        let text = RAIL.replace("profile =", "unused_profile =");
        let mesh = generate(&text).unwrap();
        let b = mesh.bounding_box;
        assert!((b.max.y - 1.3).abs() < 0.05, "{b:?}");
    }

    #[test]
    fn bad_params_name_themselves() {
        let one_point = RAIL.replace(
            "[[params.control_points]]\nposition = [2, 1.2, 0]\n\n[[params.control_points]]\nposition = [4, 1, 1]\n",
            "",
        );
        let err = generate(&one_point).unwrap_err().to_string();
        assert!(err.contains("control_points"), "{err}");
        let short = RAIL.replace("position = [2, 1.2, 0]", "position = [2, 1.2]");
        let err = generate(&short).unwrap_err().to_string();
        assert!(err.contains("control_points[1].position"), "{err}");
        let flat = RAIL.replace(
            "profile = [[0.05",
            "profile = [[0, 0], [1, 0], [2, 0]]\nold = [[0.05",
        );
        let err = generate(&flat).unwrap_err().to_string();
        assert!(err.contains("profile") && err.contains("no area"), "{err}");
        let fine = RAIL.replace("spacing = 0.25", "spacing = 0.001");
        assert!(generate(&fine).unwrap_err().to_string().contains("spacing"));
    }
}
