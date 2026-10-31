//! Post-generation validation for procedural output.
//!
//! Checks meshes and images against budget constraints, PBR ranges, and
//! style guide parameters. The [`validate_output`] entry point dispatches
//! to per-variant checks and returns a [`ValidationReport`].
//!
//! Every check runs on the generated output, never on the spec: a spec can
//! promise 8000 triangles and a palette of browns, and only the mesh and the
//! pixels say whether it kept the promise.

use std::fmt;

use crate::model::{Model, ModelLayout, ModelMesh};
use crate::output::GeneratorOutput;
use crate::spec::ProcGenSpec;
use crate::types::{ChannelSemantics, ImageData, MaterialData, MeshData, Vertex};

/// How far (CIE76 ΔE) a colour may sit from its nearest palette entry and
/// still count as "on palette". 2.3 is the just-noticeable difference; 20 is
/// a clearly different shade of the same family (light oak against sienna),
/// beyond it the eye names a different colour.
pub const PALETTE_DELTA_E: f32 = 20.0;

/// The share of a colour texture's texels that must be on palette.
pub const PALETTE_TEXEL_SHARE: f32 = 0.5;

/// The share of a roughness or metallic map's texels that must fall inside
/// the style's range for it.
pub const MAP_TEXEL_SHARE: f32 = 0.9;

/// Slack on range comparisons: material factors are `f32` and style ranges
/// `f64`, so `0.15_f32` reads as 0.15000000596 and must not fall outside
/// `[0.0, 0.15]`.
const RANGE_EPSILON: f64 = 1e-6;

/// A triangle is degenerate when its area is at most this times its longest
/// edge squared: its height is a two-millionth of its length. Relative, so
/// a pole sliver on a metre-wide ellipsoid (vertices 5e-8 apart, because
/// `sin(PI)` is not 0 in f32) counts, and a millimetre-scale detail doesn't.
/// A blade of grass a thousand times longer than it is wide scores 5e-4.
const SLIVER_RATIO: f32 = 1e-6;

/// A normal whose length is further than this from 1 is not unit length.
const NORMAL_LENGTH_TOLERANCE: f32 = 0.01;

/// Cap on texels sampled per texture for the palette and roughness checks,
/// so a 4K map validates as fast as a 256² one.
const MAX_TEXEL_SAMPLES: usize = 65_536;

// ── Status / Check / Constraints ────────────────────────────────────────────

/// Outcome of a single validation check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckStatus {
    Pass,
    Warn,
    Fail,
}

impl fmt::Display for CheckStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Pass => write!(f, "PASS"),
            Self::Warn => write!(f, "WARN"),
            Self::Fail => write!(f, "FAIL"),
        }
    }
}

/// A single validation check result.
#[derive(Debug, Clone)]
pub struct ValidationCheck {
    pub name: String,
    pub status: CheckStatus,
    pub detail: String,
}

/// Plain-data struct carrying optional budget/style constraints.
///
/// Built in the CLI from spec fields and the style guide, so that
/// `flint-procgen` never depends on `flint-asset-gen`. The spec's own LOD
/// targets are read from the spec by [`validate_output`], not from here.
#[derive(Debug, Clone, Default)]
pub struct ValidationConstraints {
    /// A ceiling on every mesh's triangle count (the style guide's
    /// `geometry.max_triangles`).
    pub max_triangles: Option<u32>,
    pub roughness_range: Option<[f64; 2]>,
    pub metallic_range: Option<[f64; 2]>,
    pub expected_texture_width: Option<u32>,
    pub expected_texture_height: Option<u32>,
    /// Hex colours (`#RRGGBB`, `RRGGBB` or `#RGB`). Materials' base colours
    /// and colour textures are checked against them.
    pub palette: Vec<String>,
    /// The style guide's `geometry.require_uvs`: a mesh whose vertices all
    /// share one UV, or that has none, fails.
    pub require_uvs: bool,
    /// The style guide's `geometry.require_normals`: a mesh read from a
    /// file without normals fails (without it, missing normals warn; glTF
    /// renderers derive flat ones). Normals that are present are always
    /// checked.
    pub require_normals: bool,
}

/// Aggregated validation results.
#[derive(Debug, Clone)]
pub struct ValidationReport {
    pub checks: Vec<ValidationCheck>,
}

impl ValidationReport {
    pub fn count_passed(&self) -> usize {
        self.count(CheckStatus::Pass)
    }

    pub fn count_warnings(&self) -> usize {
        self.count(CheckStatus::Warn)
    }

    pub fn count_failed(&self) -> usize {
        self.count(CheckStatus::Fail)
    }

    fn count(&self, status: CheckStatus) -> usize {
        self.checks.iter().filter(|c| c.status == status).count()
    }

    pub fn has_failures(&self) -> bool {
        self.checks.iter().any(|c| c.status == CheckStatus::Fail)
    }

    pub fn has_warnings(&self) -> bool {
        self.checks.iter().any(|c| c.status == CheckStatus::Warn)
    }

    /// Print each check with a status indicator.
    pub fn print_checks(&self) {
        for c in &self.checks {
            println!("  [{:>5}] {}: {}", c.status.to_string(), c.name, c.detail);
        }
    }

    /// The one-line summary, e.g. `Validation: 5 passed, 1 warning, 0 failed`.
    pub fn summary(&self) -> String {
        let warnings = self.count_warnings();
        format!(
            "Validation: {} passed, {} {}, {} failed",
            self.count_passed(),
            warnings,
            if warnings == 1 { "warning" } else { "warnings" },
            self.count_failed(),
        )
    }

    /// Print the one-line summary.
    pub fn print_summary(&self) {
        println!("{}", self.summary());
    }
}

// ── Entry point ─────────────────────────────────────────────────────────────

/// Validate a generator output against spec and constraints.
///
/// This checks the output in memory. To check what was written to disk,
/// read it back with [`read_model`](crate::model::read_model) and call
/// [`validate_model`]: the same checks, under the same names.
pub fn validate_output(
    output: &GeneratorOutput,
    spec: &ProcGenSpec,
    constraints: &ValidationConstraints,
) -> ValidationReport {
    let mut checks = Vec::new();
    let palette = parse_palette(&constraints.palette, &mut checks);
    let spec_target = |level: usize| spec_target(Some(spec), level);

    match output {
        GeneratorOutput::Mesh(mesh) => {
            validate_mesh(
                &MeshView::generated(mesh),
                constraints,
                &palette,
                spec_target(0),
                &mut checks,
                None,
            );
        }
        GeneratorOutput::MeshWithLods(lods) => {
            for (i, mesh) in lods.iter().enumerate() {
                let prefix = lod_prefix(i);
                validate_mesh(
                    &MeshView::generated(mesh),
                    constraints,
                    &palette,
                    spec_target(i),
                    &mut checks,
                    Some(&prefix),
                );
            }
        }
        GeneratorOutput::SkinnedMesh(skinned) => {
            // Extract positions/UVs from SkinnedVertex for shared checks
            let vertices: Vec<Vertex> = skinned
                .vertices
                .iter()
                .map(|sv| Vertex {
                    position: sv.position,
                    normal: sv.normal,
                    tangent: sv.tangent,
                    uv: sv.uv,
                })
                .collect();
            let proxy = MeshData {
                vertices,
                indices: skinned.indices.clone(),
                materials: skinned.materials.clone(),
                submeshes: skinned.submeshes.clone(),
                bounding_box: skinned.bounding_box,
            };
            validate_mesh(
                &MeshView::generated(&proxy),
                constraints,
                &palette,
                spec_target(0),
                &mut checks,
                Some(SKINNED_PREFIX),
            );
        }
        GeneratorOutput::Image(img) => {
            validate_image(img, constraints, &palette, &mut checks, None);
        }
        GeneratorOutput::ImageSet(images) => {
            for img in images {
                let prefix = format!("{:?}", img.channel_semantics).to_lowercase();
                validate_image(img, constraints, &palette, &mut checks, Some(&prefix));
            }
        }
        GeneratorOutput::Sound(_) => {
            checks.push(ValidationCheck {
                name: "sound output".into(),
                status: CheckStatus::Pass,
                detail: "no mesh/image checks applicable".into(),
            });
        }
    }

    ValidationReport { checks }
}

/// Validate a model read from a file (see [`crate::model`]) against an
/// optional spec (for its LOD targets) and constraints.
///
/// The checks and their names are [`validate_output`]'s: a GLB that
/// `flint gen` wrote reports the lines its in-memory output did, and an
/// imported model gets the procgen rules (integrity, slivers, normals,
/// palette) too. Attributes the file leaves out are reported as missing.
pub fn validate_model(
    model: &Model,
    spec: Option<&ProcGenSpec>,
    constraints: &ValidationConstraints,
) -> ValidationReport {
    let mut checks = Vec::new();
    let palette = parse_palette(&constraints.palette, &mut checks);
    for (i, mesh) in model.meshes.iter().enumerate() {
        let prefix = match model.layout {
            ModelLayout::Single => None,
            ModelLayout::Lods => Some(lod_prefix(i)),
            ModelLayout::Skinned => Some(SKINNED_PREFIX.to_string()),
        };
        validate_mesh(
            &MeshView::read(mesh),
            constraints,
            &palette,
            spec_target(spec, i),
            &mut checks,
            prefix.as_deref(),
        );
    }
    ValidationReport { checks }
}

const SKINNED_PREFIX: &str = "skinned";

fn lod_prefix(level: usize) -> String {
    format!("LOD{level}")
}

/// The spec's triangle target for one LOD level.
fn spec_target(spec: Option<&ProcGenSpec>, level: usize) -> Option<u32> {
    spec?
        .lod
        .as_deref()
        .unwrap_or(&[])
        .iter()
        .find(|l| l.level as usize == level)
        .map(|l| l.target_triangles)
}

/// What the mesh checks read: the mesh, and what its source left out.
struct MeshView<'a> {
    mesh: &'a MeshData,
    normals_missing: usize,
    uvs_missing: usize,
    /// Per material: whether it samples a texture, so its UVs may tile.
    textured: Vec<bool>,
}

impl<'a> MeshView<'a> {
    /// A mesh made in memory: every attribute present, and a material
    /// textured when it carries a normal map (the only texture a generated
    /// material holds; the exporter embeds it, so the file agrees).
    fn generated(mesh: &'a MeshData) -> Self {
        Self {
            mesh,
            normals_missing: 0,
            uvs_missing: 0,
            textured: mesh
                .materials
                .iter()
                .map(|m| m.normal_map.is_some())
                .collect(),
        }
    }

    fn read(m: &'a ModelMesh) -> Self {
        Self {
            mesh: &m.mesh,
            normals_missing: m.normals_missing,
            uvs_missing: m.uvs_missing,
            textured: m.textured.clone(),
        }
    }
}

fn push(checks: &mut Vec<ValidationCheck>, name: String, status: CheckStatus, detail: String) {
    checks.push(ValidationCheck {
        name,
        status,
        detail,
    });
}

fn labeller(prefix: Option<&str>) -> impl Fn(&str) -> String + '_ {
    move |name: &str| match prefix {
        Some(p) => format!("{p} {name}"),
        None => name.to_string(),
    }
}

// ── Mesh checks ─────────────────────────────────────────────────────────────

fn validate_mesh(
    view: &MeshView,
    constraints: &ValidationConstraints,
    palette: &[Palette],
    spec_target: Option<u32>,
    checks: &mut Vec<ValidationCheck>,
    prefix: Option<&str>,
) {
    let label = labeller(prefix);
    let mesh = view.mesh;

    // Indices and positions first: every later check reads through them.
    let sound = check_integrity(mesh, checks, &label("mesh integrity"));

    // Triangle budgets: the spec's target for this level, then the style's
    // ceiling. With neither, report the count so the line still says it.
    let tris = mesh.triangle_count();
    if let Some(target) = spec_target {
        check_triangle_budget_value(tris, target, checks, &label("spec triangle target"));
    }
    if let Some(max_tris) = constraints.max_triangles {
        check_triangle_budget_value(tris, max_tris, checks, &label("style triangle budget"));
    }
    if spec_target.is_none() && constraints.max_triangles.is_none() {
        push(
            checks,
            label("triangle budget"),
            CheckStatus::Pass,
            format!("{tris} triangles (no budget set)"),
        );
    }

    check_uv_range(view, constraints.require_uvs, checks, &label("UV range"));
    check_normals(view, constraints.require_normals, checks, &label("normals"));

    check_pbr_factor(
        &mesh.materials,
        "roughness",
        |m| m.roughness,
        constraints.roughness_range,
        checks,
        &label("PBR roughness"),
    );
    check_pbr_factor(
        &mesh.materials,
        "metallic",
        |m| m.metallic,
        constraints.metallic_range,
        checks,
        &label("PBR metallic"),
    );
    if !palette.is_empty() {
        check_material_palette(&mesh.materials, palette, checks, &label("palette"));
    }

    if sound {
        check_degenerate_triangles(
            &mesh.vertices,
            &mesh.indices,
            checks,
            &label("degenerate triangles"),
        );
    }
}

/// Index count a multiple of three, every index inside the vertex buffer,
/// every position finite. Returns whether the triangles can be read.
fn check_integrity(mesh: &MeshData, checks: &mut Vec<ValidationCheck>, name: &str) -> bool {
    let n = mesh.vertices.len();
    let mut problems = Vec::new();
    if !mesh.indices.len().is_multiple_of(3) {
        problems.push(format!(
            "{} indices is not a whole number of triangles",
            mesh.indices.len()
        ));
    }
    let out_of_range = mesh.indices.iter().filter(|&&i| i as usize >= n).count();
    if out_of_range > 0 {
        let max = mesh.indices.iter().max().copied().unwrap_or(0);
        problems.push(format!(
            "{out_of_range} indices past the {n} vertices (highest {max})"
        ));
    }
    let non_finite = mesh
        .vertices
        .iter()
        .filter(|v| v.position.iter().any(|c| !c.is_finite()))
        .count();
    if non_finite > 0 {
        problems.push(format!("{non_finite}/{n} positions not finite"));
    }

    if problems.is_empty() {
        push(
            checks,
            name.into(),
            CheckStatus::Pass,
            format!("{} triangles over {n} vertices", mesh.triangle_count()),
        );
        true
    } else {
        push(checks, name.into(), CheckStatus::Fail, problems.join("; "));
        // A non-finite position still leaves the indices readable.
        mesh.indices.len().is_multiple_of(3) && out_of_range == 0
    }
}

fn check_triangle_budget_value(
    actual: usize,
    budget: u32,
    checks: &mut Vec<ValidationCheck>,
    name: &str,
) {
    let budget = budget as usize;
    let (status, detail) = if actual <= budget {
        (CheckStatus::Pass, format!("{actual} triangles <= {budget}"))
    } else if actual <= budget * 2 {
        (
            CheckStatus::Warn,
            format!("{actual} triangles > {budget} (within 2x tolerance)"),
        )
    } else {
        (
            CheckStatus::Fail,
            format!(
                "{actual} triangles > {budget} ({:.1}x, over the 2x tolerance)",
                actual as f64 / budget.max(1) as f64
            ),
        )
    };
    push(checks, name.into(), status, detail);
}

/// UVs must lie in [0,1], except on a submesh whose material carries a
/// texture: glTF samplers repeat by default, so a tiling texture (the tree's
/// bark, #22) wraps on purpose. Non-finite UVs always fail. With
/// `require_uvs`, a mesh whose vertices all share one UV has no layout, and
/// one its file gave no UVs fails (without it, missing UVs warn).
fn check_uv_range(
    view: &MeshView,
    require_uvs: bool,
    checks: &mut Vec<ValidationCheck>,
    name: &str,
) {
    let mesh = view.mesh;
    let vertices = &mesh.vertices;
    if view.uvs_missing > 0 {
        let (status, why) = if require_uvs {
            (CheckStatus::Fail, ", and the style requires UVs")
        } else {
            (CheckStatus::Warn, "")
        };
        push(
            checks,
            name.into(),
            status,
            format!(
                "{}/{} vertices have no UVs{why}",
                view.uvs_missing,
                vertices.len()
            ),
        );
        return;
    }
    let mut tiles = vec![false; vertices.len()];
    for sub in &mesh.submeshes {
        let textured = view
            .textured
            .get(sub.material_index)
            .copied()
            .unwrap_or(false);
        if !textured {
            continue;
        }
        let start = sub.index_start as usize;
        let end = (start + sub.index_count as usize).min(mesh.indices.len());
        for &i in &mesh.indices[start.min(end)..end] {
            if let Some(t) = tiles.get_mut(i as usize) {
                *t = true;
            }
        }
    }

    let mut bad = 0usize;
    let mut tiled = 0usize;
    for (v, &tiling) in vertices.iter().zip(&tiles) {
        let [u, w] = v.uv;
        let outside = !(0.0..=1.0).contains(&u) || !(0.0..=1.0).contains(&w);
        if !u.is_finite() || !w.is_finite() || (outside && !tiling) {
            bad += 1;
        } else if outside {
            tiled += 1;
        }
    }

    if bad > 0 {
        push(
            checks,
            name.into(),
            CheckStatus::Fail,
            format!("{bad}/{} vertices outside [0,1]", vertices.len()),
        );
        return;
    }
    let flat = vertices.len() > 1 && vertices.iter().all(|v| v.uv == vertices[0].uv);
    if require_uvs && flat {
        push(
            checks,
            name.into(),
            CheckStatus::Fail,
            format!(
                "every vertex at UV {:?}: no layout, and the style requires UVs",
                vertices[0].uv
            ),
        );
        return;
    }
    let detail = if tiled == 0 {
        format!("all {} vertices in [0,1]", vertices.len())
    } else {
        format!(
            "all {} vertices in [0,1] or tiling a textured material ({tiled} wrap)",
            vertices.len()
        )
    };
    push(checks, name.into(), CheckStatus::Pass, detail);
}

/// Normals must be finite and non-zero (Fail: lighting breaks), and should
/// be unit length (Warn: shading darkens or blows out where they are not).
/// A file may leave them out, and glTF renderers then derive flat normals:
/// that warns, or fails under the style's `require_normals`.
fn check_normals(
    view: &MeshView,
    require_normals: bool,
    checks: &mut Vec<ValidationCheck>,
    name: &str,
) {
    let vertices = &view.mesh.vertices;
    if view.normals_missing > 0 {
        let (status, why) = if require_normals {
            (CheckStatus::Fail, ", and the style requires normals")
        } else {
            (CheckStatus::Warn, " (renderers derive flat ones)")
        };
        push(
            checks,
            name.into(),
            status,
            format!(
                "{}/{} vertices have no normals{why}",
                view.normals_missing,
                vertices.len()
            ),
        );
        return;
    }
    let mut broken = 0usize;
    let mut unnormalised = 0usize;
    for v in vertices {
        let [x, y, z] = v.normal;
        let len = (x * x + y * y + z * z).sqrt();
        if !len.is_finite() || len < 1e-6 {
            broken += 1;
        } else if (len - 1.0).abs() > NORMAL_LENGTH_TOLERANCE {
            unnormalised += 1;
        }
    }
    let n = vertices.len();
    let (status, detail) = if broken > 0 {
        (
            CheckStatus::Fail,
            format!("{broken}/{n} normals zero or not finite"),
        )
    } else if unnormalised > 0 {
        (
            CheckStatus::Warn,
            format!("{unnormalised}/{n} normals not unit length"),
        )
    } else {
        (CheckStatus::Pass, format!("all {n} unit length"))
    };
    push(checks, name.into(), status, detail);
}

/// A PBR factor must lie in [0,1] (Fail) and inside the style's range when
/// one is set (Warn).
fn check_pbr_factor(
    materials: &[MaterialData],
    factor: &str,
    get: impl Fn(&MaterialData) -> f32,
    style_range: Option<[f64; 2]>,
    checks: &mut Vec<ValidationCheck>,
    name: &str,
) {
    let mut fail = Vec::new();
    let mut warn = Vec::new();
    for mat in materials {
        let v = get(mat) as f64;
        if !(0.0..=1.0).contains(&v) {
            fail.push(format!("'{}' {factor} {v:.2} outside [0,1]", mat.name));
        } else if let Some([lo, hi]) = style_range {
            if v < lo - RANGE_EPSILON || v > hi + RANGE_EPSILON {
                warn.push(format!(
                    "'{}' {factor} {v:.2} outside style range [{lo:.2},{hi:.2}]",
                    mat.name
                ));
            }
        }
    }
    let (status, detail) = if !fail.is_empty() {
        (CheckStatus::Fail, fail.join("; "))
    } else if !warn.is_empty() {
        (CheckStatus::Warn, warn.join("; "))
    } else if let Some([lo, hi]) = style_range {
        (
            CheckStatus::Pass,
            format!(
                "all {} materials in style range [{lo:.2},{hi:.2}]",
                materials.len()
            ),
        )
    } else {
        (
            CheckStatus::Pass,
            format!("all {} materials in [0,1]", materials.len()),
        )
    };
    push(checks, name.into(), status, detail);
}

fn check_degenerate_triangles(
    vertices: &[Vertex],
    indices: &[u32],
    checks: &mut Vec<ValidationCheck>,
    name: &str,
) {
    let tri_count = indices.len() / 3;
    let mut degenerate = 0usize;

    for tri in indices.as_chunks::<3>().0 {
        let [p0, p1, p2] = [0, 1, 2].map(|k| vertices[tri[k] as usize].position);
        let e1 = [p1[0] - p0[0], p1[1] - p0[1], p1[2] - p0[2]];
        let e2 = [p2[0] - p0[0], p2[1] - p0[1], p2[2] - p0[2]];
        let cx = e1[1] * e2[2] - e1[2] * e2[1];
        let cy = e1[2] * e2[0] - e1[0] * e2[2];
        let cz = e1[0] * e2[1] - e1[1] * e2[0];
        let area = 0.5 * (cx * cx + cy * cy + cz * cz).sqrt();
        let e3 = [p2[0] - p1[0], p2[1] - p1[1], p2[2] - p1[2]];
        let longest = [e1, e2, e3]
            .map(|e| e[0] * e[0] + e[1] * e[1] + e[2] * e[2])
            .into_iter()
            .fold(0.0f32, f32::max);
        if area <= SLIVER_RATIO * longest {
            degenerate += 1;
        }
    }

    let (status, detail) = if degenerate == 0 {
        (
            CheckStatus::Pass,
            format!("all {tri_count} triangles have non-zero area"),
        )
    } else {
        (
            CheckStatus::Fail,
            format!("{degenerate}/{tri_count} degenerate triangles found"),
        )
    };
    push(checks, name.into(), status, detail);
}

// ── Palette ─────────────────────────────────────────────────────────────────

/// One palette entry: its hex as written and its CIELAB coordinates.
#[derive(Debug, Clone)]
struct Palette {
    hex: String,
    lab: [f32; 3],
}

/// Parse the style's hex palette. An entry that isn't a colour is a failed
/// check of its own (the guide is wrong), and the rest are still used.
fn parse_palette(hexes: &[String], checks: &mut Vec<ValidationCheck>) -> Vec<Palette> {
    let mut bad = Vec::new();
    let palette = hexes
        .iter()
        .filter_map(|hex| match parse_hex(hex) {
            Some(rgb) => Some(Palette {
                hex: hex.clone(),
                lab: srgb_to_lab(rgb),
            }),
            None => {
                bad.push(format!("'{hex}'"));
                None
            }
        })
        .collect();
    if !bad.is_empty() {
        push(
            checks,
            "style palette".into(),
            CheckStatus::Fail,
            format!("not hex colours: {}", bad.join(", ")),
        );
    }
    palette
}

/// `#RRGGBB`, `RRGGBB` or `#RGB` to sRGB in [0,1].
fn parse_hex(hex: &str) -> Option<[f32; 3]> {
    let h = hex.trim().trim_start_matches('#');
    let digits: Vec<u8> = match h.len() {
        6 => (0..3)
            .map(|i| u8::from_str_radix(h.get(i * 2..i * 2 + 2)?, 16).ok())
            .collect::<Option<_>>()?,
        3 => h
            .chars()
            .map(|c| c.to_digit(16).map(|d| (d * 17) as u8))
            .collect::<Option<_>>()?,
        _ => return None,
    };
    Some([0, 1, 2].map(|i| digits[i] as f32 / 255.0))
}

fn linear_to_srgb(c: f32) -> f32 {
    let c = c.clamp(0.0, 1.0);
    if c <= 0.003_130_8 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.040_45 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// sRGB (D65) to CIELAB.
fn srgb_to_lab(rgb: [f32; 3]) -> [f32; 3] {
    let [r, g, b] = rgb.map(srgb_to_linear);
    let x = (0.412_456_4 * r + 0.357_576_1 * g + 0.180_437_5 * b) / 0.950_47;
    let y = 0.212_672_9 * r + 0.715_152_2 * g + 0.072_175 * b;
    let z = (0.019_333_9 * r + 0.119_192 * g + 0.950_304_1 * b) / 1.088_83;
    let f = |t: f32| {
        if t > 216.0 / 24389.0 {
            t.cbrt()
        } else {
            (24389.0 / 27.0 * t + 16.0) / 116.0
        }
    };
    let (fx, fy, fz) = (f(x), f(y), f(z));
    [116.0 * fy - 16.0, 500.0 * (fx - fy), 200.0 * (fy - fz)]
}

/// The nearest palette entry and its CIE76 ΔE.
fn nearest(palette: &[Palette], lab: [f32; 3]) -> (&Palette, f32) {
    palette
        .iter()
        .map(|p| {
            let d = ((p.lab[0] - lab[0]).powi(2)
                + (p.lab[1] - lab[1]).powi(2)
                + (p.lab[2] - lab[2]).powi(2))
            .sqrt();
            (p, d)
        })
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .expect("palette is non-empty")
}

/// Each material's base colour within [`PALETTE_DELTA_E`] of a palette entry.
fn check_material_palette(
    materials: &[MaterialData],
    palette: &[Palette],
    checks: &mut Vec<ValidationCheck>,
    name: &str,
) {
    let mut off = Vec::new();
    let mut worst = 0.0f32;
    for mat in materials {
        let [r, g, b, _] = mat.base_color;
        let srgb = [r, g, b].map(linear_to_srgb);
        let (p, d) = nearest(palette, srgb_to_lab(srgb));
        worst = worst.max(d);
        if d > PALETTE_DELTA_E {
            off.push(format!(
                "'{}' {} is ΔE {d:.0} from nearest {}",
                mat.name,
                hex_of(srgb),
                p.hex
            ));
        }
    }
    let (status, detail) = if off.is_empty() {
        (
            CheckStatus::Pass,
            format!(
                "all {} materials within ΔE {PALETTE_DELTA_E:.0} (worst {worst:.1})",
                materials.len()
            ),
        )
    } else {
        (CheckStatus::Warn, off.join("; "))
    };
    push(checks, name.into(), status, detail);
}

fn hex_of(srgb: [f32; 3]) -> String {
    let [r, g, b] = srgb.map(|c| (c.clamp(0.0, 1.0) * 255.0).round() as u8);
    format!("#{r:02X}{g:02X}{b:02X}")
}

// ── Image checks ────────────────────────────────────────────────────────────

fn validate_image(
    img: &ImageData,
    constraints: &ValidationConstraints,
    palette: &[Palette],
    checks: &mut Vec<ValidationCheck>,
    prefix: Option<&str>,
) {
    let label = labeller(prefix);
    check_texture_dimensions(img, constraints, checks, &label("texture dimensions"));

    match img.channel_semantics {
        ChannelSemantics::Color if !palette.is_empty() => {
            check_texture_palette(img, palette, checks, &label("palette"));
        }
        ChannelSemantics::Roughness => {
            if let Some(range) = constraints.roughness_range {
                check_texture_range(img, "roughness", range, checks, &label("style range"));
            }
        }
        ChannelSemantics::Metallic => {
            if let Some(range) = constraints.metallic_range {
                check_texture_range(img, "metallic", range, checks, &label("style range"));
            }
        }
        _ => {}
    }
}

/// The size the spec asked for (Fail when it differs), in powers of two
/// (Warn when not: mipmaps and block compression want them).
fn check_texture_dimensions(
    img: &ImageData,
    constraints: &ValidationConstraints,
    checks: &mut Vec<ValidationCheck>,
    name: &str,
) {
    let (w, h) = (img.width, img.height);
    let ew = constraints.expected_texture_width.unwrap_or(w);
    let eh = constraints.expected_texture_height.unwrap_or(h);
    let (status, detail) = if (w, h) != (ew, eh) {
        (
            CheckStatus::Fail,
            format!("{w}x{h} != {ew}x{eh} the spec asked for"),
        )
    } else if !w.is_power_of_two() || !h.is_power_of_two() {
        (CheckStatus::Warn, format!("{w}x{h}: not power-of-two"))
    } else {
        (CheckStatus::Pass, format!("{w}x{h}"))
    };
    push(checks, name.into(), status, detail);
}

/// Up to [`MAX_TEXEL_SAMPLES`] RGBA texels, spread evenly over the image.
fn sample_texels(img: &ImageData) -> impl Iterator<Item = &[u8]> {
    let count = img.pixels.len() / 4;
    let step = count.div_ceil(MAX_TEXEL_SAMPLES).max(1);
    img.pixels.chunks_exact(4).step_by(step)
}

/// The share of a colour map's texels within [`PALETTE_DELTA_E`] of the
/// palette must reach [`PALETTE_TEXEL_SHARE`]. Texels are sRGB, as written.
fn check_texture_palette(
    img: &ImageData,
    palette: &[Palette],
    checks: &mut Vec<ValidationCheck>,
    name: &str,
) {
    let mut near = 0usize;
    let mut total = 0usize;
    let mut sum_de = 0.0f64;
    for px in sample_texels(img) {
        let srgb = [px[0], px[1], px[2]].map(|c| c as f32 / 255.0);
        let (_, d) = nearest(palette, srgb_to_lab(srgb));
        sum_de += d as f64;
        total += 1;
        if d <= PALETTE_DELTA_E {
            near += 1;
        }
    }
    let share = near as f32 / total.max(1) as f32;
    let mean = sum_de / total.max(1) as f64;
    let detail = format!(
        "{:.0}% of texels within ΔE {PALETTE_DELTA_E:.0} of the palette (mean ΔE {mean:.1})",
        share * 100.0
    );
    let status = if share >= PALETTE_TEXEL_SHARE {
        CheckStatus::Pass
    } else {
        CheckStatus::Warn
    };
    push(checks, name.into(), status, detail);
}

/// The share of a grayscale map's texels inside the style's range must
/// reach [`MAP_TEXEL_SHARE`]. The value is the red channel over 255.
fn check_texture_range(
    img: &ImageData,
    factor: &str,
    [lo, hi]: [f64; 2],
    checks: &mut Vec<ValidationCheck>,
    name: &str,
) {
    let mut inside = 0usize;
    let mut total = 0usize;
    let mut sum = 0.0f64;
    for px in sample_texels(img) {
        let v = px[0] as f64 / 255.0;
        sum += v;
        total += 1;
        // Half a byte of slack: 0.6 is byte 153, and 153/255 is 0.6 exactly,
        // but 0.95 lands between bytes.
        if v >= lo - 0.5 / 255.0 && v <= hi + 0.5 / 255.0 {
            inside += 1;
        }
    }
    let share = inside as f64 / total.max(1) as f64;
    let mean = sum / total.max(1) as f64;
    let detail = format!(
        "{:.0}% of texels in style range [{lo:.2},{hi:.2}] (mean {factor} {mean:.2})",
        share * 100.0
    );
    let status = if share >= MAP_TEXEL_SHARE as f64 {
        CheckStatus::Pass
    } else {
        CheckStatus::Warn
    };
    push(checks, name.into(), status, detail);
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::*;

    fn make_triangle() -> MeshData {
        MeshData {
            vertices: vec![
                Vertex {
                    position: [0.0, 0.0, 0.0],
                    normal: [0.0, 0.0, 1.0],
                    tangent: [1.0, 0.0, 0.0, 1.0],
                    uv: [0.0, 0.0],
                },
                Vertex {
                    position: [1.0, 0.0, 0.0],
                    normal: [0.0, 0.0, 1.0],
                    tangent: [1.0, 0.0, 0.0, 1.0],
                    uv: [1.0, 0.0],
                },
                Vertex {
                    position: [0.0, 1.0, 0.0],
                    normal: [0.0, 0.0, 1.0],
                    tangent: [1.0, 0.0, 0.0, 1.0],
                    uv: [0.0, 1.0],
                },
            ],
            indices: vec![0, 1, 2],
            materials: vec![MaterialData::default()],
            submeshes: vec![],
            bounding_box: BoundingBox::from_positions(&[
                [0.0, 0.0, 0.0],
                [1.0, 0.0, 0.0],
                [0.0, 1.0, 0.0],
            ]),
        }
    }

    fn make_spec() -> ProcGenSpec {
        ProcGenSpec::parse(
            r#"
generator = "mock"
[meta]
name = "test"
version = "1.0.0"
[seed]
mode = "fixed"
value = 42
"#,
        )
        .unwrap()
    }

    // 1. Valid mesh → all checks pass
    #[test]
    fn valid_mesh_all_pass() {
        let mesh = make_triangle();
        let output = GeneratorOutput::Mesh(mesh);
        let spec = make_spec();
        let constraints = ValidationConstraints {
            max_triangles: Some(10),
            ..Default::default()
        };
        let report = validate_output(&output, &spec, &constraints);
        assert!(!report.has_failures());
        assert!(!report.has_warnings());
        assert!(report.count_passed() >= 4); // budget, uv, roughness, metallic, degenerate
    }

    // 2. UV outside [0,1] → Fail
    #[test]
    fn uv_outside_range_fails() {
        let mut mesh = make_triangle();
        mesh.vertices[0].uv = [1.5, 0.0];
        let output = GeneratorOutput::Mesh(mesh);
        let spec = make_spec();
        let constraints = ValidationConstraints::default();
        let report = validate_output(&output, &spec, &constraints);
        let uv_check = report
            .checks
            .iter()
            .find(|c| c.name.contains("UV range"))
            .unwrap();
        assert_eq!(uv_check.status, CheckStatus::Fail);
        assert!(uv_check.detail.contains("1/3"));
    }

    // 2b. UVs that tile a textured submesh (bark) pass; NaN never does
    #[test]
    fn uv_tiling_a_textured_material_passes() {
        use crate::types::{ChannelSemantics, SubMesh};
        let mut mesh = make_triangle();
        mesh.vertices[0].uv = [1.5, 3.0];
        mesh.materials = vec![MaterialData {
            normal_map: Some(ImageData::new(4, 4, ChannelSemantics::Normal)),
            ..MaterialData::default()
        }];
        mesh.submeshes = vec![SubMesh {
            index_start: 0,
            index_count: 3,
            material_index: 0,
        }];
        let uv_status = |mesh: &MeshData| {
            let report = validate_output(
                &GeneratorOutput::Mesh(mesh.clone()),
                &make_spec(),
                &ValidationConstraints::default(),
            );
            report
                .checks
                .iter()
                .find(|c| c.name.contains("UV range"))
                .unwrap()
                .status
        };
        assert_eq!(uv_status(&mesh), CheckStatus::Pass);
        mesh.vertices[1].uv = [f32::NAN, 0.0];
        assert_eq!(uv_status(&mesh), CheckStatus::Fail);
    }

    // 3. Triangle count over budget → Warn (under 2x), Fail (over 2x)
    #[test]
    fn triangle_over_budget_warn() {
        // Make 3 triangles, budget 2 → 3 > 2, 3 <= 4 → Warn
        let spec = make_spec();
        let mut mesh2 = make_triangle();
        // Add two more triangles
        mesh2.vertices.push(Vertex {
            position: [2.0, 0.0, 0.0],
            normal: [0.0, 0.0, 1.0],
            tangent: [1.0, 0.0, 0.0, 1.0],
            uv: [0.5, 0.5],
        });
        mesh2.vertices.push(Vertex {
            position: [2.0, 1.0, 0.0],
            normal: [0.0, 0.0, 1.0],
            tangent: [1.0, 0.0, 0.0, 1.0],
            uv: [0.5, 0.5],
        });
        mesh2.indices.extend_from_slice(&[0, 3, 4, 1, 3, 4]); // 3 triangles total
        let output2 = GeneratorOutput::Mesh(mesh2);
        let constraints = ValidationConstraints {
            max_triangles: Some(2),
            ..Default::default()
        };
        let report = validate_output(&output2, &spec, &constraints);
        let budget_check = report
            .checks
            .iter()
            .find(|c| c.name.contains("triangle budget"))
            .unwrap();
        assert_eq!(budget_check.status, CheckStatus::Warn);
    }

    #[test]
    fn triangle_over_budget_fail() {
        let mut mesh = make_triangle();
        // Make 5 triangles, budget 2 → 5 > 4 → Fail
        for _ in 0..4 {
            let base = mesh.vertices.len() as u32;
            mesh.vertices.push(Vertex {
                position: [0.0, 0.0, 0.0],
                normal: [0.0, 0.0, 1.0],
                tangent: [1.0, 0.0, 0.0, 1.0],
                uv: [0.5, 0.5],
            });
            mesh.vertices.push(Vertex {
                position: [1.0, 0.0, 0.0],
                normal: [0.0, 0.0, 1.0],
                tangent: [1.0, 0.0, 0.0, 1.0],
                uv: [0.5, 0.5],
            });
            mesh.vertices.push(Vertex {
                position: [0.0, 1.0, 0.0],
                normal: [0.0, 0.0, 1.0],
                tangent: [1.0, 0.0, 0.0, 1.0],
                uv: [0.5, 0.5],
            });
            mesh.indices.extend_from_slice(&[base, base + 1, base + 2]);
        }
        // Now 5 triangles total
        let output = GeneratorOutput::Mesh(mesh);
        let spec = make_spec();
        let constraints = ValidationConstraints {
            max_triangles: Some(2),
            ..Default::default()
        };
        let report = validate_output(&output, &spec, &constraints);
        let budget_check = report
            .checks
            .iter()
            .find(|c| c.name.contains("triangle budget"))
            .unwrap();
        assert_eq!(budget_check.status, CheckStatus::Fail);
    }

    // 4. Roughness > 1.0 → Fail (PBR range)
    #[test]
    fn roughness_out_of_pbr_range_fails() {
        let mut mesh = make_triangle();
        mesh.materials[0].roughness = 1.5;
        let output = GeneratorOutput::Mesh(mesh);
        let spec = make_spec();
        let constraints = ValidationConstraints::default();
        let report = validate_output(&output, &spec, &constraints);
        let check = report
            .checks
            .iter()
            .find(|c| c.name.contains("roughness"))
            .unwrap();
        assert_eq!(check.status, CheckStatus::Fail);
    }

    // 5. Roughness outside style range but in [0,1] → Warn
    #[test]
    fn roughness_outside_style_range_warns() {
        let mut mesh = make_triangle();
        mesh.materials[0].roughness = 0.3; // valid PBR, but outside style range
        let output = GeneratorOutput::Mesh(mesh);
        let spec = make_spec();
        let constraints = ValidationConstraints {
            roughness_range: Some([0.6, 0.95]),
            ..Default::default()
        };
        let report = validate_output(&output, &spec, &constraints);
        let check = report
            .checks
            .iter()
            .find(|c| c.name.contains("roughness"))
            .unwrap();
        assert_eq!(check.status, CheckStatus::Warn);
    }

    // 6. Degenerate triangle (coincident vertices) → Fail
    #[test]
    fn degenerate_triangle_fails() {
        let mut mesh = make_triangle();
        // Make all three vertices coincident
        mesh.vertices[1].position = [0.0, 0.0, 0.0];
        mesh.vertices[2].position = [0.0, 0.0, 0.0];
        let output = GeneratorOutput::Mesh(mesh);
        let spec = make_spec();
        let constraints = ValidationConstraints::default();
        let report = validate_output(&output, &spec, &constraints);
        let check = report
            .checks
            .iter()
            .find(|c| c.name.contains("degenerate"))
            .unwrap();
        assert_eq!(check.status, CheckStatus::Fail);
        assert!(check.detail.contains("1/1"));
    }

    // 7. Texture dimensions mismatch → Fail
    #[test]
    fn texture_dimensions_mismatch_fails() {
        let img = ImageData::new(256, 256, ChannelSemantics::Color);
        let output = GeneratorOutput::Image(img);
        let spec = make_spec();
        let constraints = ValidationConstraints {
            expected_texture_width: Some(512),
            expected_texture_height: Some(512),
            ..Default::default()
        };
        let report = validate_output(&output, &spec, &constraints);
        let check = report
            .checks
            .iter()
            .find(|c| c.name.contains("texture"))
            .unwrap();
        assert_eq!(check.status, CheckStatus::Fail);
    }

    // 8. Non-power-of-two texture → Warn
    #[test]
    fn non_power_of_two_texture_warns() {
        let img = ImageData::new(300, 300, ChannelSemantics::Color);
        let output = GeneratorOutput::Image(img);
        let spec = make_spec();
        let constraints = ValidationConstraints::default();
        let report = validate_output(&output, &spec, &constraints);
        let check = report
            .checks
            .iter()
            .find(|c| c.name.contains("texture"))
            .unwrap();
        assert_eq!(check.status, CheckStatus::Warn);
    }

    // 9. Valid texture → all pass
    #[test]
    fn valid_texture_all_pass() {
        let img = ImageData::new(256, 256, ChannelSemantics::Color);
        let output = GeneratorOutput::Image(img);
        let spec = make_spec();
        let constraints = ValidationConstraints::default();
        let report = validate_output(&output, &spec, &constraints);
        assert!(!report.has_failures());
        assert!(!report.has_warnings());
    }

    // 10. No constraints, valid mesh → all pass
    #[test]
    fn no_constraints_valid_mesh_all_pass() {
        let mesh = make_triangle();
        let output = GeneratorOutput::Mesh(mesh);
        let spec = make_spec();
        let constraints = ValidationConstraints::default();
        let report = validate_output(&output, &spec, &constraints);
        assert!(!report.has_failures());
        assert!(!report.has_warnings());
    }

    // 11. MeshWithLods → per-LOD checks
    #[test]
    fn mesh_with_lods_per_lod_checks() {
        let lod0 = make_triangle();
        let lod1 = make_triangle();
        let output = GeneratorOutput::MeshWithLods(vec![lod0, lod1]);
        let mut spec = make_spec();
        spec.lod = Some(vec![
            crate::LodLevel {
                level: 0,
                target_triangles: 10,
            },
            crate::LodLevel {
                level: 1,
                target_triangles: 1,
            },
        ]);
        let constraints = ValidationConstraints::default();
        let report = validate_output(&output, &spec, &constraints);
        assert!(!report.has_failures());
        // Should have checks prefixed with LOD0 and LOD1
        let lod0_checks: Vec<_> = report
            .checks
            .iter()
            .filter(|c| c.name.starts_with("LOD0"))
            .collect();
        let lod1_checks: Vec<_> = report
            .checks
            .iter()
            .filter(|c| c.name.starts_with("LOD1"))
            .collect();
        assert!(!lod0_checks.is_empty());
        assert!(!lod1_checks.is_empty());
    }

    // 12. ImageSet → each image checked
    #[test]
    fn image_set_each_checked() {
        let img1 = ImageData::new(256, 256, ChannelSemantics::Color);
        let img2 = ImageData::new(256, 256, ChannelSemantics::Normal);
        let output = GeneratorOutput::ImageSet(vec![img1, img2]);
        let spec = make_spec();
        let constraints = ValidationConstraints::default();
        let report = validate_output(&output, &spec, &constraints);
        assert_eq!(report.checks.len(), 2);
        assert!(report.checks.iter().any(|c| c.name.contains("color")));
        assert!(report.checks.iter().any(|c| c.name.contains("normal")));
    }

    // 13. Summary format
    #[test]
    fn summary_format() {
        let report = ValidationReport {
            checks: vec![
                ValidationCheck {
                    name: "a".into(),
                    status: CheckStatus::Pass,
                    detail: "ok".into(),
                },
                ValidationCheck {
                    name: "b".into(),
                    status: CheckStatus::Warn,
                    detail: "meh".into(),
                },
                ValidationCheck {
                    name: "c".into(),
                    status: CheckStatus::Fail,
                    detail: "bad".into(),
                },
            ],
        };
        assert_eq!(report.count_passed(), 1);
        assert_eq!(report.count_warnings(), 1);
        assert_eq!(report.count_failed(), 1);
        assert!(report.has_failures());
        assert!(report.has_warnings());
    }

    // Sound output → pass
    #[test]
    fn sound_output_passes() {
        let output = GeneratorOutput::Sound(vec![0u8; 16]);
        let spec = make_spec();
        let constraints = ValidationConstraints::default();
        let report = validate_output(&output, &spec, &constraints);
        assert!(!report.has_failures());
        assert_eq!(report.count_passed(), 1);
    }

    // Metallic outside PBR range → Fail
    #[test]
    fn metallic_out_of_pbr_range_fails() {
        let mut mesh = make_triangle();
        mesh.materials[0].metallic = -0.1;
        let output = GeneratorOutput::Mesh(mesh);
        let spec = make_spec();
        let constraints = ValidationConstraints::default();
        let report = validate_output(&output, &spec, &constraints);
        let check = report
            .checks
            .iter()
            .find(|c| c.name.contains("metallic"))
            .unwrap();
        assert_eq!(check.status, CheckStatus::Fail);
    }

    // Metallic outside style range → Warn
    #[test]
    fn metallic_outside_style_range_warns() {
        let mut mesh = make_triangle();
        mesh.materials[0].metallic = 0.5; // valid PBR, outside style
        let output = GeneratorOutput::Mesh(mesh);
        let spec = make_spec();
        let constraints = ValidationConstraints {
            metallic_range: Some([0.0, 0.15]),
            ..Default::default()
        };
        let report = validate_output(&output, &spec, &constraints);
        let check = report
            .checks
            .iter()
            .find(|c| c.name.contains("metallic"))
            .unwrap();
        assert_eq!(check.status, CheckStatus::Warn);
    }

    // CheckStatus Display
    #[test]
    fn check_status_display() {
        assert_eq!(format!("{}", CheckStatus::Pass), "PASS");
        assert_eq!(format!("{}", CheckStatus::Warn), "WARN");
        assert_eq!(format!("{}", CheckStatus::Fail), "FAIL");
    }

    // ── #30: the checks the audit found missing ─────────────────────────────

    fn check<'a>(report: &'a ValidationReport, name: &str) -> &'a ValidationCheck {
        report
            .checks
            .iter()
            .find(|c| c.name.contains(name))
            .unwrap_or_else(|| panic!("no '{name}' check in {:#?}", report.checks))
    }

    #[test]
    fn summary_line_says_warning_for_one() {
        let mk = |status| ValidationCheck {
            name: "x".into(),
            status,
            detail: String::new(),
        };
        let one = ValidationReport {
            checks: vec![mk(CheckStatus::Pass), mk(CheckStatus::Warn)],
        };
        assert_eq!(one.summary(), "Validation: 1 passed, 1 warning, 0 failed");
        let two = ValidationReport {
            checks: vec![mk(CheckStatus::Warn), mk(CheckStatus::Warn)],
        };
        assert_eq!(two.summary(), "Validation: 0 passed, 2 warnings, 0 failed");
    }

    // The beetle's elytra are metallic 0.15 and the tavern allows [0, 0.15]:
    // f32 0.15 widened to f64 is 0.15000000596, which used to warn.
    #[test]
    fn style_range_edge_is_inside() {
        let mut mesh = make_triangle();
        mesh.materials[0].metallic = 0.15;
        mesh.materials[0].roughness = 0.95;
        let report = validate_output(
            &GeneratorOutput::Mesh(mesh),
            &make_spec(),
            &ValidationConstraints {
                metallic_range: Some([0.0, 0.15]),
                roughness_range: Some([0.6, 0.95]),
                ..Default::default()
            },
        );
        assert_eq!(check(&report, "metallic").status, CheckStatus::Pass);
        assert_eq!(check(&report, "roughness").status, CheckStatus::Pass);
    }

    #[test]
    fn out_of_range_index_fails_integrity_without_panicking() {
        let mut mesh = make_triangle();
        mesh.indices = vec![0, 1, 7];
        let report = validate_output(
            &GeneratorOutput::Mesh(mesh),
            &make_spec(),
            &ValidationConstraints::default(),
        );
        let c = check(&report, "mesh integrity");
        assert_eq!(c.status, CheckStatus::Fail);
        assert!(c.detail.contains("highest 7"), "{}", c.detail);
        // The degenerate check can't read the triangle, so it doesn't run.
        assert!(!report.checks.iter().any(|c| c.name.contains("degenerate")));
    }

    #[test]
    fn nan_position_fails_integrity() {
        let mut mesh = make_triangle();
        mesh.vertices[2].position[1] = f32::NAN;
        let report = validate_output(
            &GeneratorOutput::Mesh(mesh),
            &make_spec(),
            &ValidationConstraints::default(),
        );
        assert_eq!(check(&report, "mesh integrity").status, CheckStatus::Fail);
    }

    #[test]
    fn zero_normal_fails_and_long_normal_warns() {
        let status = |n: [f32; 3]| {
            let mut mesh = make_triangle();
            mesh.vertices[1].normal = n;
            check(
                &validate_output(
                    &GeneratorOutput::Mesh(mesh),
                    &make_spec(),
                    &ValidationConstraints::default(),
                ),
                "normals",
            )
            .status
        };
        assert_eq!(status([0.0, 0.0, 1.0]), CheckStatus::Pass);
        assert_eq!(status([0.0, 0.0, 2.0]), CheckStatus::Warn);
        assert_eq!(status([0.0, 0.0, 0.0]), CheckStatus::Fail);
        assert_eq!(status([f32::NAN, 0.0, 1.0]), CheckStatus::Fail);
    }

    #[test]
    fn require_uvs_fails_a_mesh_with_no_layout() {
        let mut mesh = make_triangle();
        for v in &mut mesh.vertices {
            v.uv = [0.0, 0.0];
        }
        let run = |require_uvs| {
            check(
                &validate_output(
                    &GeneratorOutput::Mesh(mesh.clone()),
                    &make_spec(),
                    &ValidationConstraints {
                        require_uvs,
                        ..Default::default()
                    },
                ),
                "UV range",
            )
            .status
        };
        assert_eq!(run(false), CheckStatus::Pass);
        assert_eq!(run(true), CheckStatus::Fail);
    }

    fn tavern() -> Vec<String> {
        ["#8B4513", "#A0522D", "#D4A574", "#4A4A4A"]
            .map(String::from)
            .to_vec()
    }

    #[test]
    fn material_palette_compliance() {
        let run = |srgb: [f32; 3]| {
            let mut mesh = make_triangle();
            let [r, g, b] = srgb.map(srgb_to_linear);
            mesh.materials[0].base_color = [r, g, b, 1.0];
            validate_output(
                &GeneratorOutput::Mesh(mesh),
                &make_spec(),
                &ValidationConstraints {
                    palette: tavern(),
                    ..Default::default()
                },
            )
        };
        // Sienna itself, and a slightly lighter sienna, are on palette.
        let on = run([
            0xA0 as f32 / 255.0,
            0x52 as f32 / 255.0,
            0x2D as f32 / 255.0,
        ]);
        assert_eq!(check(&on, "palette").status, CheckStatus::Pass);
        let near = run([0.66, 0.35, 0.20]);
        assert_eq!(check(&near, "palette").status, CheckStatus::Pass);
        // Leaf green is not a tavern colour.
        let off = run([0.25, 0.55, 0.15]);
        let c = check(&off, "palette");
        assert_eq!(c.status, CheckStatus::Warn);
        assert!(c.detail.contains("nearest"), "{}", c.detail);
    }

    #[test]
    fn no_palette_check_without_a_palette() {
        let report = validate_output(
            &GeneratorOutput::Mesh(make_triangle()),
            &make_spec(),
            &ValidationConstraints::default(),
        );
        assert!(!report.checks.iter().any(|c| c.name.contains("palette")));
    }

    #[test]
    fn a_bad_palette_entry_is_reported() {
        let report = validate_output(
            &GeneratorOutput::Mesh(make_triangle()),
            &make_spec(),
            &ValidationConstraints {
                palette: vec!["#8B4513".into(), "brownish".into()],
                ..Default::default()
            },
        );
        let c = check(&report, "style palette");
        assert_eq!(c.status, CheckStatus::Fail);
        assert!(c.detail.contains("brownish"));
        // The good entry is still used.
        assert!(report.checks.iter().any(|c| c.name == "palette"));
    }

    fn filled(
        w: u32,
        h: u32,
        sem: ChannelSemantics,
        rgb: impl Fn(u32, u32) -> [u8; 3],
    ) -> ImageData {
        let mut img = ImageData::new(w, h, sem);
        for y in 0..h {
            for x in 0..w {
                let o = ((y * w + x) * 4) as usize;
                let [r, g, b] = rgb(x, y);
                img.pixels[o..o + 4].copy_from_slice(&[r, g, b, 255]);
            }
        }
        img
    }

    #[test]
    fn texture_palette_counts_texels() {
        let run = |img: ImageData| {
            check(
                &validate_output(
                    &GeneratorOutput::Image(img),
                    &make_spec(),
                    &ValidationConstraints {
                        palette: tavern(),
                        ..Default::default()
                    },
                ),
                "palette",
            )
            .clone()
        };
        let wood = run(filled(16, 16, ChannelSemantics::Color, |_, _| {
            [0x8B, 0x45, 0x13]
        }));
        assert_eq!(wood.status, CheckStatus::Pass);
        assert!(wood.detail.starts_with("100%"), "{}", wood.detail);
        // Three quarters cyan: a quarter on palette is not enough.
        let cyan = run(filled(16, 16, ChannelSemantics::Color, |x, _| {
            if x < 4 {
                [0x8B, 0x45, 0x13]
            } else {
                [0, 200, 220]
            }
        }));
        assert_eq!(cyan.status, CheckStatus::Warn);
        assert!(cyan.detail.starts_with("25%"), "{}", cyan.detail);
    }

    #[test]
    fn roughness_map_against_style_range() {
        let run = |byte: u8| {
            check(
                &validate_output(
                    &GeneratorOutput::ImageSet(vec![filled(
                        8,
                        8,
                        ChannelSemantics::Roughness,
                        |_, _| [byte; 3],
                    )]),
                    &make_spec(),
                    &ValidationConstraints {
                        roughness_range: Some([0.6, 0.95]),
                        ..Default::default()
                    },
                ),
                "roughness style range",
            )
            .status
        };
        assert_eq!(run(200), CheckStatus::Pass); // 0.78
        assert_eq!(run(153), CheckStatus::Pass); // 0.60, the edge
        assert_eq!(run(242), CheckStatus::Pass); // 0.949, rounds to the edge
        assert_eq!(run(60), CheckStatus::Warn); // 0.24, glossy
    }

    #[test]
    fn texture_dimension_messages() {
        let run = |w, h, ew, eh| {
            check(
                &validate_output(
                    &GeneratorOutput::Image(ImageData::new(w, h, ChannelSemantics::Color)),
                    &make_spec(),
                    &ValidationConstraints {
                        expected_texture_width: ew,
                        expected_texture_height: eh,
                        ..Default::default()
                    },
                ),
                "texture dimensions",
            )
            .clone()
        };
        let c = run(256, 128, Some(256), Some(256));
        assert_eq!(c.status, CheckStatus::Fail);
        assert_eq!(c.detail, "256x128 != 256x256 the spec asked for");
        let c = run(256, 128, None, Some(256));
        assert_eq!(c.detail, "256x128 != 256x256 the spec asked for");
        assert_eq!(run(256, 128, None, None).status, CheckStatus::Pass);
    }

    // The spec's LOD target and the style's ceiling are separate lines, and
    // LOD0 is no longer checked against the same number twice.
    #[test]
    fn lod_targets_and_style_ceiling_are_separate() {
        let output = GeneratorOutput::MeshWithLods(vec![make_triangle(), make_triangle()]);
        let mut spec = make_spec();
        spec.lod = Some(vec![crate::LodLevel {
            level: 0,
            target_triangles: 10,
        }]);
        let report = validate_output(
            &output,
            &spec,
            &ValidationConstraints {
                max_triangles: Some(5),
                ..Default::default()
            },
        );
        let names: Vec<_> = report
            .checks
            .iter()
            .filter(|c| c.name.contains("triangle target") || c.name.contains("triangle budget"))
            .map(|c| c.name.as_str())
            .collect();
        assert_eq!(
            names,
            [
                "LOD0 spec triangle target",
                "LOD0 style triangle budget",
                "LOD1 style triangle budget"
            ]
        );
    }

    #[test]
    fn degenerate_is_relative_to_size() {
        let status = |p: [[f32; 3]; 3]| {
            let mut mesh = make_triangle();
            for (v, p) in mesh.vertices.iter_mut().zip(p) {
                v.position = p;
            }
            check(
                &validate_output(
                    &GeneratorOutput::Mesh(mesh),
                    &make_spec(),
                    &ValidationConstraints::default(),
                ),
                "degenerate",
            )
            .status
        };
        // A pole sliver: two corners 5e-8 apart on a half-metre shape. Its
        // area (~1e-8) is over the old absolute 1e-10, but it is a sliver.
        assert_eq!(
            status([[5e-8, 0.55, 0.0], [0.0, 0.55, 0.0], [0.14, 0.53, 0.0]]),
            CheckStatus::Fail
        );
        // A millimetre triangle (area 5e-7) is fine, and so is a grass blade.
        assert_eq!(
            status([[0.0, 0.0, 0.0], [0.001, 0.0, 0.0], [0.0, 0.001, 0.0]]),
            CheckStatus::Pass
        );
        assert_eq!(
            status([[0.0, 0.0, 0.0], [0.001, 0.0, 0.0], [0.0, 1.0, 0.0]]),
            CheckStatus::Pass
        );
    }

    // The UV sphere and ellipsoid no longer emit their pole slivers.
    #[test]
    fn sphere_and_ellipsoid_have_no_slivers() {
        use crate::algorithms::mesh_builder::primitives::{
            ellipsoid, sphere, EllipsoidParams, SphereParams,
        };
        let meshes = [
            sphere(&SphereParams {
                radius: 1.0,
                segments: 16,
                rings: 12,
            }),
            ellipsoid(&EllipsoidParams {
                radius_x: 0.35,
                radius_y: 0.25,
                radius_z: 0.55,
                segments: 16,
                rings: 12,
            }),
        ];
        for mesh in meshes {
            let report = validate_output(
                &GeneratorOutput::Mesh(mesh),
                &make_spec(),
                &ValidationConstraints::default(),
            );
            let c = check(&report, "degenerate");
            assert_eq!(c.status, CheckStatus::Pass, "{}", c.detail);
        }
    }

    #[test]
    fn lab_matches_reference_values() {
        // White, and sRGB red: L*a*b* (53.24, 80.09, 67.20).
        let w = srgb_to_lab([1.0, 1.0, 1.0]);
        assert!((w[0] - 100.0).abs() < 0.05 && w[1].abs() < 0.05 && w[2].abs() < 0.05);
        let r = srgb_to_lab([1.0, 0.0, 0.0]);
        assert!((r[0] - 53.24).abs() < 0.1, "{r:?}");
        assert!((r[1] - 80.09).abs() < 0.2, "{r:?}");
        assert!((r[2] - 67.20).abs() < 0.2, "{r:?}");
        assert_eq!(parse_hex("#fff"), Some([1.0, 1.0, 1.0]));
        assert_eq!(parse_hex("8B4513").map(hex_of).as_deref(), Some("#8B4513"));
        assert_eq!(parse_hex("#12345"), None);
    }
}
