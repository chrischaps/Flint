//! `form_v1`: one abstract living form that isn't a tree.
//!
//! A body swept around a spine of bones, with small two-sided cards on its
//! surface. The lineage seed fixes the form's identity: its spine, its lobes and
//! every card. A pose bends, ripples, swells, turns and stretches the spine along
//! one smooth curve, so the silhouette holds while it breathes; the cards ride
//! their bones and boil through Living Props' jitter layer, as leaf cards do; and
//! `[living] echoes` draws its other selves around it. Built for Embodiment
//! (chrischaps/Embodiment#4), whose candidates are a held flame, a folded ribbon,
//! a standing stone and the like, after spike #3 found the `LivingGenerator`
//! contract general enough for a form with no branches.
//!
//! - `body.rs`: the spine, its frames, and the surface swept around it.
//! - `cards.rs`: the cards, and which segment each rides.
//! - `pose.rs`: a pose from a shape seed.
//!
//! The parameters fall in four groups, described in [`FormGenerator::param_schema`]:
//! the section (`radius`, `profile`, `aspect`, ...), the spine (`height`, `lean`,
//! `curl_deg`, `coil`, ...), the cards (`cards`, `card_tilt`, ...) and the pose
//! vocabulary (`bend_deg`, `ripple`, `swell`, `turn_deg`, `stretch`, `slide`).

mod body;
mod cards;
mod pose;

use flint_core::quat::{quat_conjugate, quat_mul, quat_normalize, quat_rotate_vec3};
use flint_core::Vec3;

use crate::generator::{GenerationCost, Generator, LivingGenerator};
use crate::generators::util::{parse_hex_color, toml_f32, toml_string, toml_u32};
use crate::living::{LivingPose, LivingRig, LivingSkeleton, SegmentPose, BOIL_BONES};
use crate::output::{GeneratorOutput, OutputKind};
use crate::skinning::{BoneDef, SkeletonDef, SkinnedMeshData, SkinnedVertex, MAX_SKINNED_BONES};
use crate::spec::ProcGenSpec;
use crate::types::{BoundingBox, BranchSegment, MeshData, SubMesh};
use crate::{ProcGenError, Result, SeededRng};

use body::{surface, Lineage, Spine};
use cards::{bind_cards, card_mesh, CARD_INDICES, CARD_VERTICES};

/// The form generator. See the module docs.
pub struct FormGenerator;

/// `[params]` of a `form_v1` spec. Lengths in metres, angles in degrees.
#[derive(Debug, Clone, PartialEq)]
pub struct FormParams {
    // The spine.
    pub height: f32,
    pub lift: f32,
    pub lean: f32,
    pub curl_deg: f32,
    pub curl_bias: f32,
    pub coil: f32,
    pub coil_turns: f32,
    pub facing_deg: f32,
    pub segments: u32,
    pub waves: u32,
    // The section.
    pub radius: f32,
    pub profile: Vec<f32>,
    pub round_base: f32,
    pub round_tip: f32,
    pub aspect: f32,
    pub section_deg: f32,
    pub twist_turns: f32,
    pub lobes: f32,
    pub rings: u32,
    pub sides: u32,
    pub body_color: [f32; 4],
    pub body_roughness: f32,
    pub body_opacity: f32,
    // The cards.
    pub cards: u32,
    pub card_layout: CardLayout,
    pub card_size: f32,
    pub card_aspect: f32,
    pub card_lift: f32,
    pub card_tilt: f32,
    pub card_turn_deg: f32,
    pub card_from: f32,
    pub card_to: f32,
    pub card_rise: f32,
    pub card_color: [f32; 4],
    pub card_shades: u32,
    pub card_roughness: f32,
    // Rows: the cards as lines of writing (`card_layout = "rows"`).
    pub row_spacing: f32,
    pub row_margin: f32,
    pub word_gap: f32,
    pub paragraph_lines: f32,
    pub row_faces: RowFaces,
    // The pose vocabulary.
    pub bend_deg: f32,
    pub ripple: f32,
    pub ripple_waves: f32,
    pub swell: f32,
    pub turn_deg: f32,
    pub stretch: f32,
    pub slide: f32,
    pub slide_reflow: f32,
}

impl Default for FormParams {
    fn default() -> Self {
        Self {
            height: 1.3,
            lift: 0.0,
            lean: 0.07,
            curl_deg: 0.0,
            curl_bias: 1.0,
            coil: 0.0,
            coil_turns: 1.0,
            facing_deg: 0.0,
            segments: 10,
            waves: 3,
            radius: 0.27,
            profile: DEFAULT_PROFILE.to_vec(),
            round_base: 0.06,
            round_tip: 0.12,
            aspect: 1.0,
            section_deg: 0.0,
            twist_turns: 0.0,
            lobes: 0.06,
            rings: 96,
            sides: 64,
            body_color: parse_hex_color(DEFAULT_BODY).expect("a valid colour"),
            body_roughness: 0.8,
            body_opacity: 1.0,
            cards: 520,
            card_layout: CardLayout::Scatter,
            card_size: 0.06,
            card_aspect: 1.5,
            card_lift: 0.012,
            card_tilt: 0.0,
            card_turn_deg: 35.0,
            card_from: 0.04,
            card_to: 0.97,
            card_rise: 0.0,
            card_color: parse_hex_color(DEFAULT_CARD).expect("a valid colour"),
            card_shades: 4,
            card_roughness: 0.7,
            row_spacing: 0.04,
            row_margin: 0.1,
            word_gap: 0.012,
            paragraph_lines: 6.0,
            row_faces: RowFaces::Both,
            bend_deg: 16.0,
            ripple: 0.5,
            ripple_waves: 1.4,
            swell: 0.14,
            turn_deg: 0.0,
            stretch: 0.05,
            slide: 0.0,
            slide_reflow: 0.5,
        }
    }
}

/// Where the cards lie: `card_layout`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CardLayout {
    /// Scattered over the whole surface by area, like scales or flakes.
    Scatter,
    /// In rows across the broad faces, a card a word, like lines of writing.
    Rows,
}

/// Which broad faces carry rows: `row_faces`. The front is the face turned
/// away from `facing_deg`, so a page curling away from you shows you its front.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowFaces {
    Front,
    Back,
    Both,
}

/// An enum key: its value must be one of `names`, read as the matching item.
fn one_of<T: Copy>(
    t: &toml::map::Map<String, toml::Value>,
    key: &str,
    names: &[(&str, T)],
) -> Result<Option<T>> {
    let Some(v) = t.get(key) else {
        return Ok(None);
    };
    let s = toml_string(v, key)?;
    names
        .iter()
        .find(|(name, _)| *name == s)
        .map(|(_, item)| Some(*item))
        .ok_or_else(|| {
            let all: Vec<String> = names.iter().map(|(n, _)| format!("{n:?}")).collect();
            invalid(key, format!("{s:?}: must be one of {}", all.join(", ")))
        })
}

const DEFAULT_PROFILE: [f32; 6] = [0.6, 0.95, 1.0, 0.85, 0.55, 0.15];
const DEFAULT_BODY: &str = "#3E382F";
const DEFAULT_CARD: &str = "#9C8E7A";

/// A number key's accepted range, inclusive. `open_min` makes the low end
/// exclusive.
struct Range {
    min: f32,
    max: f32,
    open_min: bool,
}

const fn closed(min: f32, max: f32) -> Range {
    Range {
        min,
        max,
        open_min: false,
    }
}

const fn above(min: f32, max: f32) -> Range {
    Range {
        min,
        max,
        open_min: true,
    }
}

fn invalid(name: &str, reason: impl Into<String>) -> ProcGenError {
    ProcGenError::InvalidParameter {
        name: name.into(),
        reason: reason.into(),
    }
}

fn check(name: &str, value: f32, range: &Range) -> Result<f32> {
    let low_ok = if range.open_min {
        value > range.min
    } else {
        value >= range.min
    };
    if value.is_finite() && low_ok && value <= range.max {
        return Ok(value);
    }
    let low = if range.open_min {
        "more than"
    } else {
        "at least"
    };
    Err(invalid(
        name,
        format!(
            "{value}: must be {low} {} and at most {}",
            range.min, range.max
        ),
    ))
}

impl FormParams {
    /// Read `[params]`: a missing key takes its default, and a bad one is an
    /// error naming it.
    pub fn from_toml(params: &toml::Value) -> Result<Self> {
        let mut p = Self::default();
        let Some(t) = params.as_table() else {
            return Ok(p);
        };
        macro_rules! num {
            ($key:literal, $field:ident, $range:expr) => {
                if let Some(v) = t.get($key) {
                    p.$field = check($key, toml_f32(v, $key)?, &$range)?;
                }
            };
        }
        macro_rules! int {
            ($key:literal, $field:ident, $min:expr, $max:expr) => {
                if let Some(v) = t.get($key) {
                    let n = toml_u32(v, $key)?;
                    if !($min..=$max).contains(&n) {
                        return Err(invalid($key, format!("{n}: must be {} to {}", $min, $max)));
                    }
                    p.$field = n;
                }
            };
        }
        macro_rules! colour {
            ($key:literal, $field:ident) => {
                if let Some(v) = t.get($key) {
                    p.$field = parse_hex_color(&toml_string(v, $key)?)
                        .map_err(|e| invalid($key, e.to_string()))?;
                }
            };
        }
        num!("height", height, above(0.0, 100.0));
        num!("lift", lift, closed(0.0, 100.0));
        num!("lean", lean, closed(-10.0, 10.0));
        num!("curl_deg", curl_deg, closed(-270.0, 270.0));
        num!("curl_bias", curl_bias, closed(0.2, 5.0));
        num!("coil", coil, closed(0.0, 10.0));
        num!("coil_turns", coil_turns, closed(0.0, 12.0));
        num!("facing_deg", facing_deg, closed(-360.0, 360.0));
        int!("segments", segments, 2, 64);
        int!("waves", waves, 1, 64);
        num!("radius", radius, above(0.0, 50.0));
        num!("round_base", round_base, closed(0.0, 0.5));
        num!("round_tip", round_tip, closed(0.0, 0.5));
        num!("aspect", aspect, above(0.0, 1.0));
        num!("section_deg", section_deg, closed(-360.0, 360.0));
        num!("twist_turns", twist_turns, closed(-8.0, 8.0));
        num!("lobes", lobes, closed(0.0, 0.5));
        int!("rings", rings, 4, 1024);
        int!("sides", sides, 6, 1024);
        colour!("body_color", body_color);
        num!("body_roughness", body_roughness, closed(0.0, 1.0));
        num!("body_opacity", body_opacity, closed(0.0, 1.0));
        int!("cards", cards, 0, 20_000);
        if let Some(layout) = one_of(
            t,
            "card_layout",
            &[("scatter", CardLayout::Scatter), ("rows", CardLayout::Rows)],
        )? {
            p.card_layout = layout;
        }
        num!("card_size", card_size, above(0.0, 10.0));
        num!("card_aspect", card_aspect, above(0.0, 10.0));
        num!("card_lift", card_lift, closed(0.0, 1.0));
        num!("card_tilt", card_tilt, closed(0.0, 1.0));
        num!("card_turn_deg", card_turn_deg, closed(0.0, 180.0));
        num!("card_from", card_from, closed(0.0, 1.0));
        num!("card_to", card_to, closed(0.0, 1.0));
        num!("card_rise", card_rise, closed(-0.9, 10.0));
        colour!("card_color", card_color);
        int!("card_shades", card_shades, 1, 8);
        num!("card_roughness", card_roughness, closed(0.0, 1.0));
        num!("row_spacing", row_spacing, above(0.0, 10.0));
        num!("row_margin", row_margin, closed(0.0, 0.9));
        num!("word_gap", word_gap, closed(0.0, 10.0));
        num!("paragraph_lines", paragraph_lines, closed(0.0, 1000.0));
        if let Some(faces) = one_of(
            t,
            "row_faces",
            &[
                ("front", RowFaces::Front),
                ("back", RowFaces::Back),
                ("both", RowFaces::Both),
            ],
        )? {
            p.row_faces = faces;
        }
        num!("bend_deg", bend_deg, closed(0.0, 90.0));
        num!("ripple", ripple, closed(0.0, 2.0));
        num!("ripple_waves", ripple_waves, closed(0.0, 6.0));
        num!("swell", swell, closed(0.0, 0.9));
        num!("turn_deg", turn_deg, closed(0.0, 180.0));
        num!("stretch", stretch, closed(0.0, 0.5));
        num!("slide", slide, closed(0.0, 1.0));
        num!("slide_reflow", slide_reflow, closed(0.0, 1.0));
        if let Some(v) = t.get("profile") {
            let Some(list) = v.as_array() else {
                return Err(invalid("profile", "must be an array of numbers"));
            };
            if !(2..=16).contains(&list.len()) {
                return Err(invalid(
                    "profile",
                    format!("{} values: must be 2 to 16, base to tip", list.len()),
                ));
            }
            p.profile = list
                .iter()
                .map(|x| check("profile", toml_f32(x, "profile")?, &closed(0.0, 10.0)))
                .collect::<Result<_>>()?;
            if p.profile.iter().all(|x| *x == 0.0) {
                return Err(invalid("profile", "all 0: the form would have no body"));
            }
        }
        if p.waves > p.segments {
            return Err(invalid(
                "waves",
                format!("{}: at most `segments` ({})", p.waves, p.segments),
            ));
        }
        if p.card_from >= p.card_to {
            return Err(invalid(
                "card_from",
                format!("{}: must be below card_to ({})", p.card_from, p.card_to),
            ));
        }
        Ok(p)
    }
}

/// Everything a lineage fixes: the params, the lineage's own draws, the spine
/// and its segments.
struct Built {
    p: FormParams,
    lin: Lineage,
    spine: Spine,
    segments: Vec<BranchSegment>,
}

fn build(spec: &ProcGenSpec, lineage_seed: u64) -> Result<Built> {
    let p = FormParams::from_toml(&spec.params)?;
    let mut rng = SeededRng::new(lineage_seed);
    let lin = Lineage::new(&mut rng.fork("lineage"));
    let spine = Spine::new(&p, &lin);
    let segments = spine.segments(&p, &lin);
    Ok(Built {
        p,
        lin,
        spine,
        segments,
    })
}

/// How large a pose is: `[living] drift.amplitude`, 1 being the pose params as
/// written. A spec with no `[living]` table poses at the drift's default.
fn amplitude(spec: &ProcGenSpec) -> f32 {
    spec.living.as_ref().map_or_else(
        || crate::spec::DriftConfig::default().amplitude,
        |l| l.drift.amplitude,
    )
}

/// The rotation turning `a` to `b`.
fn rotation_between(a: Vec3, b: Vec3) -> [f32; 4] {
    flint_core::quat::quat_from_two_vectors(a.to_array(), b.to_array())
}

impl FormGenerator {
    fn skeleton(&self, built: &Built, lineage_seed: u64) -> LivingSkeleton {
        let leaf_mesh = card_mesh(
            &built.p,
            &built.lin,
            &built.spine,
            &mut SeededRng::new(lineage_seed).fork("cards"),
        );
        let leaf_bindings = bind_cards(&leaf_mesh, &built.segments);
        // An echo's place (`[living] echo_places`) is measured along the
        // lines of writing, the section's broad axis, and up the form, each
        // segment's own way: the same frame the pose's slide moves along.
        let n = built.segments.len();
        let place_axes = built
            .segments
            .iter()
            .enumerate()
            .map(|(i, segment)| {
                let u = (i as f32 + 0.5) / n as f32;
                let along = body::broad_axis(&built.p, &built.spine, u);
                [along.to_array(), segment.direction().to_array()]
            })
            .collect();
        LivingSkeleton {
            lineage_seed,
            segments: built.segments.clone(),
            leaf_mesh,
            card_index_count: CARD_INDICES,
            leaf_bindings,
            place_axes,
        }
    }
}

impl LivingGenerator for FormGenerator {
    /// Every `form_v1` spec is living. Only its params can be wrong.
    fn check_living(&self, spec: &ProcGenSpec) -> Result<()> {
        FormParams::from_toml(&spec.params).map(|_| ())
    }

    fn structure(&self, spec: &ProcGenSpec, lineage_seed: u64) -> Result<LivingSkeleton> {
        let built = build(spec, lineage_seed)?;
        Ok(self.skeleton(&built, lineage_seed))
    }

    fn rig(&self, spec: &ProcGenSpec, lineage_seed: u64) -> Result<LivingRig> {
        let built = build(spec, lineage_seed)?;
        let skeleton = self.skeleton(&built, lineage_seed);
        let Built {
            p,
            lin,
            spine,
            segments,
        } = &built;
        let n = segments.len();
        // Bone 0 is the root, which no pose moves; segment i rides bone i + 1.
        let segment_bones: Vec<u16> = (0..n).map(|i| i as u16 + 1).collect();
        let bone_segments: Vec<Option<u32>> = std::iter::once(None)
            .chain((0..n).map(|i| Some(i as u32)))
            .collect();
        debug_assert!(bone_segments.len() + BOIL_BONES <= MAX_SKINNED_BONES);

        // The body: a ring at a time, each ring blended between the two bones
        // nearest it, so a bend is a curve and not a kink.
        let (rings, sides) = (p.rings as usize, p.sides as usize);
        let mut vertices =
            Vec::with_capacity((rings + 1) * (sides + 1) + skeleton.leaf_mesh.vertices.len());
        for r in 0..=rings {
            let u = r as f32 / rings as f32;
            let c = (u * n as f32 - 0.5).clamp(0.0, n as f32 - 1.0);
            let i0 = c.floor() as usize;
            let i1 = (i0 + 1).min(n - 1);
            let f = c - i0 as f32;
            let (joints, weights) = if i1 == i0 || f <= 0.0 {
                ([i0 as u16 + 1, 0, 0, 0], [1.0, 0.0, 0.0, 0.0])
            } else {
                ([i0 as u16 + 1, i1 as u16 + 1, 0, 0], [1.0 - f, f, 0.0, 0.0])
            };
            for s in 0..=sides {
                let a = s as f32 / sides as f32 * std::f32::consts::TAU;
                let (pos, nrm) = surface(p, lin, spine, u, a);
                let frame = spine.frame(u);
                let along = (frame.binormal * a.cos() - frame.normal * a.sin()).normalized();
                vertices.push(SkinnedVertex {
                    position: pos.to_array(),
                    normal: nrm.to_array(),
                    tangent: [along.x, along.y, along.z, 1.0],
                    uv: [s as f32 / sides as f32, u],
                    joint_indices: joints,
                    joint_weights: weights,
                });
            }
        }
        // Wound so each quad faces the way its vertices' normals do: which way
        // round the sweep goes depends on the section's turn and the curl.
        let mut indices = Vec::with_capacity(rings * sides * 6);
        for r in 0..rings {
            for s in 0..sides {
                let a = (r * (sides + 1) + s) as u32;
                let (b, c) = (a + 1, a + (sides + 1) as u32);
                let d = c + 1;
                let at = |i: u32| Vec3::from_array(vertices[i as usize].position);
                let normal = [a, b, c, d].iter().fold(Vec3::ZERO, |sum, &i| {
                    sum + Vec3::from_array(vertices[i as usize].normal)
                });
                let facing = (at(d) - at(a)).cross(&(at(c) - at(b)));
                if facing.dot(&normal) >= 0.0 {
                    indices.extend_from_slice(&[a, b, d, a, d, c]);
                } else {
                    indices.extend_from_slice(&[a, d, b, a, c, d]);
                }
            }
        }
        let body_index_count = indices.len() as u32;
        let first_leaf_vertex = vertices.len();

        // The cards, each wholly on its segment's bone.
        let cards = &skeleton.leaf_mesh;
        for (k, binding) in skeleton.leaf_bindings.iter().enumerate() {
            let bone = segment_bones[binding.segment as usize];
            for v in &cards.vertices[k * CARD_VERTICES..(k + 1) * CARD_VERTICES] {
                vertices.push(SkinnedVertex {
                    position: v.position,
                    normal: v.normal,
                    tangent: v.tangent,
                    uv: v.uv,
                    joint_indices: [bone, 0, 0, 0],
                    joint_weights: [1.0, 0.0, 0.0, 0.0],
                });
            }
        }
        let mut submeshes = vec![SubMesh {
            index_start: 0,
            index_count: body_index_count,
            material_index: 0,
        }];
        for sub in &cards.submeshes {
            submeshes.push(SubMesh {
                index_start: sub.index_start + body_index_count,
                index_count: sub.index_count,
                material_index: sub.material_index,
            });
        }
        indices.extend(cards.indices.iter().map(|i| i + first_leaf_vertex as u32));

        let frame = |s: &BranchSegment| rotation_between(Vec3::UP, s.direction());
        let bones = bone_segments
            .iter()
            .map(|bone| match bone {
                None => BoneDef {
                    name: "root".into(),
                    ..BoneDef::default()
                },
                Some(id) => {
                    let s = &segments[*id as usize];
                    let (parent_bone, parent_head, parent_frame) = match s.parent {
                        Some(pid) => {
                            let parent = &segments[pid as usize];
                            (
                                segment_bones[pid as usize] as usize,
                                parent.start,
                                frame(parent),
                            )
                        }
                        None => (0, Vec3::ZERO, SegmentPose::IDENTITY.rotation),
                    };
                    let to_parent = quat_conjugate(&parent_frame);
                    BoneDef {
                        name: format!("seg_{id:04}"),
                        parent: Some(parent_bone),
                        rest_translation: quat_rotate_vec3(
                            &to_parent,
                            (s.start - parent_head).to_array(),
                        ),
                        rest_rotation: quat_normalize(&quat_mul(&to_parent, &frame(s))),
                        rest_scale: [1.0, 1.0, 1.0],
                    }
                }
            })
            .collect();

        let positions: Vec<[f32; 3]> = vertices.iter().map(|v| v.position).collect();
        let materials = cards.materials.clone();
        let mesh = SkinnedMeshData {
            vertices,
            indices,
            materials,
            submeshes,
            bounding_box: BoundingBox::from_positions(&positions),
            skeleton: SkeletonDef { bones },
        };
        mesh.validate()?;
        Ok(LivingRig {
            card_boil: skeleton.card_boil(bone_segments.len() as u16),
            mesh,
            segment_bones,
            bone_segments,
            first_leaf_vertex,
            leaf_parts: (1..=cards.submeshes.len()).collect(),
        })
    }

    fn full_pose(
        &self,
        spec: &ProcGenSpec,
        skeleton: &LivingSkeleton,
        shape_seed: u64,
    ) -> Result<LivingPose> {
        let built = build(spec, skeleton.lineage_seed)?;
        if built.segments.len() != skeleton.segments.len() {
            return Err(invalid(
                "pose",
                format!(
                    "the skeleton has {} segments and this spec makes {}: a skeleton of another spec",
                    skeleton.segments.len(),
                    built.segments.len()
                ),
            ));
        }
        Ok(pose::full_pose(
            &built.p,
            &built.spine,
            skeleton,
            amplitude(spec),
            shape_seed,
        ))
    }

    fn posed_mesh(
        &self,
        spec: &ProcGenSpec,
        skeleton: &LivingSkeleton,
        pose: &LivingPose,
    ) -> Result<MeshData> {
        let rig = self.rig(spec, skeleton.lineage_seed)?;
        let mut palette = Vec::new();
        rig.palette(skeleton, pose, &mut palette)?;
        Ok(rig.skinned_mesh(&palette))
    }
}

impl Generator for FormGenerator {
    fn type_name(&self) -> &str {
        "form_v1"
    }

    fn revision(&self) -> u32 {
        1
    }

    fn output_kind(&self) -> OutputKind {
        OutputKind::Mesh
    }

    fn as_living(&self) -> Option<&dyn LivingGenerator> {
        Some(self)
    }

    /// With a `[living]` table, the skinned rest of the lineage its
    /// `lineage_seed` names, bones and all, as `tree_v1` bakes a living tree;
    /// without one, the plain rest mesh of the spec's seed.
    fn generate(&self, spec: &ProcGenSpec, rng: &mut SeededRng) -> Result<GeneratorOutput> {
        match &spec.living {
            Some(living) => Ok(GeneratorOutput::SkinnedMesh(
                self.rig(spec, living.lineage_seed)?.mesh,
            )),
            None => Ok(GeneratorOutput::Mesh(
                self.rig(spec, rng.seed())?.mesh.rest_mesh(),
            )),
        }
    }

    fn param_schema(&self) -> serde_json::Value {
        let d = FormParams::default();
        let hex = "^#?[0-9a-fA-F]{6,8}$";
        serde_json::json!({
            "type": "object",
            "description": "One abstract living form: a body swept around a spine of bones, with two-sided cards on its surface that boil. A pose bends, ripples, swells, turns and stretches the spine.",
            "properties": {
                "height": { "type": "number", "exclusiveMinimum": 0.0, "maximum": 100.0, "default": d.height,
                    "description": "Length of the spine from base to tip, metres (its height when it doesn't curl)" },
                "lift": { "type": "number", "minimum": 0.0, "maximum": 100.0, "default": d.lift,
                    "description": "How far the base stands above the prop's origin, metres: above 0 the form floats" },
                "lean": { "type": "number", "minimum": -10.0, "maximum": 10.0, "default": d.lean,
                    "description": "Sideways offset of the tip toward `facing_deg`, metres, growing with the square of the height" },
                "curl_deg": { "type": "number", "minimum": -270.0, "maximum": 270.0, "default": d.curl_deg,
                    "description": "Degrees the spine's direction turns through from base to tip, toward `facing_deg`: a hook, a wave, a fold" },
                "curl_bias": { "type": "number", "minimum": 0.2, "maximum": 5.0, "default": d.curl_bias,
                    "description": "Where the curl happens: 1 evenly, above 1 more toward the tip, below 1 more toward the base" },
                "coil": { "type": "number", "minimum": 0.0, "maximum": 10.0, "default": d.coil,
                    "description": "Radius of a helical wander of the spine at the tip, metres, from nothing at the base" },
                "coil_turns": { "type": "number", "minimum": 0.0, "maximum": 12.0, "default": d.coil_turns,
                    "description": "Turns the coil makes over the spine" },
                "facing_deg": { "type": "number", "minimum": -360.0, "maximum": 360.0, "default": d.facing_deg,
                    "description": "Direction of the lean and curl, degrees about up from +X (0 bends sideways to a camera on +Z); each lineage turns it by up to ±15°" },
                "segments": { "type": "integer", "minimum": 2, "maximum": 64, "default": d.segments,
                    "description": "Bones along the spine: more is a smoother bend and more palette per prop" },
                "waves": { "type": "integer", "minimum": 1, "maximum": 64, "default": d.waves,
                    "description": "Depth bands the segments fall into, base to tip, at most `segments`: per-level stagger ripples up the form in this many waves, and `[living] variance.steady` holds the low bands still" },
                "radius": { "type": "number", "exclusiveMinimum": 0.0, "maximum": 50.0, "default": d.radius,
                    "description": "The section's radius where `profile` is 1, metres" },
                "profile": { "type": "array", "items": { "type": "number", "minimum": 0.0, "maximum": 10.0 },
                    "minItems": 2, "maxItems": 16, "default": d.profile,
                    "description": "The section's radius as fractions of `radius`, evenly spaced from base to tip and joined smoothly: a flame narrows to 0 at the tip, a stone stays wide" },
                "round_base": { "type": "number", "minimum": 0.0, "maximum": 0.5, "default": d.round_base,
                    "description": "Fraction of the length over which the base rounds off to the spine: 0 is a flat base" },
                "round_tip": { "type": "number", "minimum": 0.0, "maximum": 0.5, "default": d.round_tip,
                    "description": "Fraction of the length over which the tip rounds off to the spine: 0 is a flat top" },
                "aspect": { "type": "number", "exclusiveMinimum": 0.0, "maximum": 1.0, "default": d.aspect,
                    "description": "The section's thin axis over its broad one: 1 round, 0.1 a ribbon" },
                "section_deg": { "type": "number", "minimum": -360.0, "maximum": 360.0, "default": d.section_deg,
                    "description": "Turn of the section's broad axis away from the curl's plane, degrees: 90 folds a ribbon across its width" },
                "twist_turns": { "type": "number", "minimum": -8.0, "maximum": 8.0, "default": d.twist_turns,
                    "description": "Turns the section makes about the spine from base to tip, at rest" },
                "lobes": { "type": "number", "minimum": 0.0, "maximum": 0.5, "default": d.lobes,
                    "description": "Depth of the slow lobes the lineage puts in the surface, as a fraction of the radius" },
                "rings": { "type": "integer", "minimum": 4, "maximum": 1024, "default": d.rings,
                    "description": "Rings of vertices along the body" },
                "sides": { "type": "integer", "minimum": 6, "maximum": 1024, "default": d.sides,
                    "description": "Vertices around each ring" },
                "body_color": { "type": "string", "pattern": hex, "default": DEFAULT_BODY,
                    "description": "The body's colour, sRGB hex" },
                "body_roughness": { "type": "number", "minimum": 0.0, "maximum": 1.0, "default": d.body_roughness,
                    "description": "The body's roughness, 0 glossy to 1 matte" },
                "body_opacity": { "type": "number", "minimum": 0.0, "maximum": 1.0, "default": d.body_opacity,
                    "description": "How much of the body shows: 1 solid; below 1 it's drawn translucent after everything opaque, and its echoes with it; 0 not at all, so the form is only its cards" },
                "cards": { "type": "integer", "minimum": 0, "maximum": 20000, "default": d.cards,
                    "description": "Cards on the surface: what boils. 0 is a smooth form. With `card_layout = \"rows\"` it's the most there can be, and the rows decide how many" },
                "card_layout": { "type": "string", "enum": ["scatter", "rows"], "default": "scatter",
                    "description": "Where cards lie: `scatter` over the whole surface by area; `rows` in lines across the broad faces, a card a word, so a flat form reads as a page of writing. In rows `card_size` is a stroke's thickness and `card_aspect` a typical word's length over it, the cards lie flat, and `card_tilt` and `card_rise` have nothing to do" },
                "card_size": { "type": "number", "exclusiveMinimum": 0.0, "maximum": 10.0, "default": d.card_size,
                    "description": "Typical card width, metres; each varies ±30%" },
                "card_aspect": { "type": "number", "exclusiveMinimum": 0.0, "maximum": 10.0, "default": d.card_aspect,
                    "description": "A card's length over its width: above 1 a feather or flake along the surface" },
                "card_lift": { "type": "number", "minimum": 0.0, "maximum": 1.0, "default": d.card_lift,
                    "description": "How far a card sits off the surface, metres" },
                "card_tilt": { "x-when": { "card_layout": ["scatter"] }, "type": "number", "minimum": 0.0, "maximum": 1.0, "default": d.card_tilt,
                    "description": "How far cards stand up off the surface toward the tip: 0 flat like scales, 1 upright like a fringe, which boils the outline's edge" },
                "card_turn_deg": { "type": "number", "minimum": 0.0, "maximum": 180.0, "default": d.card_turn_deg,
                    "description": "Largest turn of a card about its normal away from pointing up the surface, degrees" },
                "card_from": { "type": "number", "minimum": 0.0, "maximum": 1.0, "default": d.card_from,
                    "description": "Height fraction where cards begin, below `card_to`" },
                "card_to": { "type": "number", "minimum": 0.0, "maximum": 1.0, "default": d.card_to,
                    "description": "Height fraction where cards end" },
                "card_rise": { "x-when": { "card_layout": ["scatter"] }, "type": "number", "minimum": -0.9, "maximum": 10.0, "default": d.card_rise,
                    "description": "How cards crowd along the length: 0 by area alone, above 0 toward the tip, below 0 toward the base" },
                "card_color": { "type": "string", "pattern": hex, "default": DEFAULT_CARD,
                    "description": "The cards' middle colour, sRGB hex; shades spread warmer and cooler around it" },
                "card_shades": { "type": "integer", "minimum": 1, "maximum": 8, "default": d.card_shades,
                    "description": "Shades the cards are spread over, each a part of its own the jitter tints" },
                "card_roughness": { "type": "number", "minimum": 0.0, "maximum": 1.0, "default": d.card_roughness,
                    "description": "The cards' roughness, 0 glossy to 1 matte" },
                "row_spacing": { "x-when": { "card_layout": ["rows"] }, "type": "number", "exclusiveMinimum": 0.0, "maximum": 10.0, "default": d.row_spacing,
                    "description": "Rows: metres from one line to the next, up the surface" },
                "row_margin": { "x-when": { "card_layout": ["rows"] }, "type": "number", "minimum": 0.0, "maximum": 0.9, "default": d.row_margin,
                    "description": "Rows: the share of the section's half-width kept clear at each side, so lines stop short of the edge and never bristle at the outline" },
                "word_gap": { "x-when": { "card_layout": ["rows"] }, "type": "number", "minimum": 0.0, "maximum": 10.0, "default": d.word_gap,
                    "description": "Rows: metres between words; each gap varies ±30%" },
                "paragraph_lines": { "x-when": { "card_layout": ["rows"] }, "type": "number", "minimum": 0.0, "maximum": 1000.0, "default": d.paragraph_lines,
                    "description": "Rows: lines in a typical paragraph, whose last line stops short and whose next begins indented; 0 is one unbroken block" },
                "row_faces": { "x-when": { "card_layout": ["rows"] }, "type": "string", "enum": ["front", "back", "both"], "default": "both",
                    "description": "Rows: which broad faces carry lines. The front is the face turned away from `facing_deg`, so a page curling away from you shows you its front" },
                "bend_deg": { "type": "number", "minimum": 0.0, "maximum": 90.0, "default": d.bend_deg,
                    "description": "Pose: how far a pose leans the tip, degrees, at drift amplitude 1. The slow lean of the whole form" },
                "ripple": { "type": "number", "minimum": 0.0, "maximum": 2.0, "default": d.ripple,
                    "description": "Pose: a wave up the spine, as a share of `bend_deg`" },
                "ripple_waves": { "type": "number", "minimum": 0.0, "maximum": 6.0, "default": d.ripple_waves,
                    "description": "Pose: waves of the ripple along the length; each pose varies it ±25%" },
                "swell": { "type": "number", "minimum": 0.0, "maximum": 0.9, "default": d.swell,
                    "description": "Pose: how much the section breathes wider and narrower along the length, as a fraction of its radius" },
                "turn_deg": { "type": "number", "minimum": 0.0, "maximum": 180.0, "default": d.turn_deg,
                    "description": "Pose: how far a pose turns the section about the spine at the tip, degrees: what a flat form shows" },
                "stretch": { "type": "number", "minimum": 0.0, "maximum": 0.5, "default": d.stretch,
                    "description": "Pose: how much a pose lengthens or shortens the spine, as a fraction (`[living] variance.keep_size` evens the box out again)" },
                "slide": { "type": "number", "minimum": 0.0, "maximum": 1.0, "default": d.slide,
                    "description": "Pose: how far a pose moves the form along its section's broad axis at most, metres: along the lines of a page in rows. Unlike a bend it doesn't pivot at the base, so an echo can stand beside the form rather than fan from its root" },
                "slide_reflow": { "type": "number", "minimum": 0.0, "maximum": 1.0, "default": d.slide_reflow,
                    "description": "Pose: how much of the slide changes along the length rather than moving the whole form: 0 slides it as one, 1 lets lines at one height go one way and lines at another the other" }
            },
            "additionalProperties": false
        })
    }

    fn estimate_cost(&self, spec: &ProcGenSpec) -> GenerationCost {
        let p = FormParams::from_toml(&spec.params).unwrap_or_default();
        let body = (p.rings + 1) * (p.sides + 1);
        let cards = p.cards * CARD_VERTICES as u32;
        GenerationCost {
            estimated_vertices: body + cards,
            estimated_triangles: p.rings * p.sides * 2 + p.cards * 4,
            estimated_generation_ms: (body as f64 * 2.0 + cards as f64) * 4e-4 + 1.0,
            ..Default::default()
        }
    }
}

#[cfg(test)]
mod tests;
