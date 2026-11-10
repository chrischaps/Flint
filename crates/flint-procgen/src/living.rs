//! What a [`LivingGenerator`](crate::LivingGenerator) makes: a skeleton that
//! holds a prop's identity, and poses that say how far one variant of it has
//! drifted.
//!
//! Poses are small and relative to the skeleton, so a ring of them can be
//! cached apart from any mesh and blended per segment without one.

use std::collections::BTreeMap;

use flint_core::quat::{quat_nlerp, quat_rotate_vec3};
use flint_core::Vec3;
use serde::{Deserialize, Serialize};

use crate::generator::LivingGenerator;
use crate::skinning::SkinnedMeshData;
use crate::spec::{LivingConfig, ProcGenSpec, Stagger, VarianceConfig, JITTER_TARGETS};
use crate::types::{BoundingBox, BranchSegment, MeshData};
use crate::{ProcGenError, Result};

/// How many times [`LivingSkeleton::with_variance`] corrects its size. A
/// fixed count, so the result doesn't depend on a tolerance. Limbs stay on the
/// trunk, so the box isn't quite proportional to the scale and one round isn't
/// enough, but each round leaves only a fraction of the error.
const KEEP_SIZE_ROUNDS: usize = 4;

/// How many leaf cards' worth of independent jitter a prop has: each card turns
/// with one of these, so they don't all jump at once, but a palette stays small.
pub const JITTER_CHANNELS: usize = 4;

/// The bones at the end of a runtime palette that carry the jitter layer: an
/// x, a y and a z for each channel. See [`CardBoil`].
pub const BOIL_BONES: usize = JITTER_CHANNELS * 3;

/// Radians a leaf card turns at most, per unit of `jitter.amplitude`. The
/// default amplitude of 0.05 gives about 11 degrees either way.
const CARD_TURN_PER_AMPLITUDE: f32 = 4.0;

/// How far a leaf shade's colour moves toward warm or cool at most, per unit
/// of `jitter.amplitude`: the default gives 10% either way.
const TINT_PER_AMPLITUDE: f32 = 2.0;

/// How many tints each side of neutral a shade can take. They're quantized so
/// that a draw's material stays one of a few, shared by every prop and created
/// once, and boil never makes a bind group per frame.
const TINT_STEPS: i32 = 4;

/// One segment's transform in a pose, relative to the skeleton.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SegmentPose {
    /// Turns the rest segment's direction to the pose's, in the prop's space
    /// and not its parent's, as a quaternion `[x, y, z, w]`. It doesn't
    /// compound down the tree: a child's rotation already includes whatever
    /// its parent did.
    pub rotation: [f32; 4],
    /// The pose's length over the rest length.
    pub length_scale: f32,
    /// The pose's mean radius over the rest mean radius.
    pub radius_scale: f32,
    /// How far the pose moves the segment, in metres in the prop's space,
    /// after it's turned and stretched. Like `rotation` it's absolute: it
    /// doesn't carry to the segment's children, which start where the unmoved
    /// chain puts them and take a slide of their own. So slides that change
    /// smoothly along a tube shear it without parting it, and a pose can move
    /// a form sideways rather than only bend it about its root. Zero for any
    /// generator that doesn't ask for it.
    #[serde(default, skip_serializing_if = "is_zero_slide")]
    pub slide: [f32; 3],
}

fn is_zero_slide(slide: &[f32; 3]) -> bool {
    *slide == [0.0; 3]
}

impl SegmentPose {
    /// The rest pose.
    pub const IDENTITY: Self = Self {
        rotation: [0.0, 0.0, 0.0, 1.0],
        length_scale: 1.0,
        radius_scale: 1.0,
        slide: [0.0; 3],
    };

    /// `a` at `weight` 0 and `b` at 1, exactly, so a held frame is its pose to
    /// the bit. Between them, rotation by nlerp along the short way round,
    /// which the #207 spike found enough at this size, and scales by lerp.
    pub fn blend(a: &Self, b: &Self, weight: f32) -> Self {
        if weight <= 0.0 {
            return *a;
        }
        if weight >= 1.0 {
            return *b;
        }
        let lerp = |x: f32, y: f32| x + (y - x) * weight;
        Self {
            rotation: quat_nlerp(&a.rotation, &b.rotation, weight),
            length_scale: lerp(a.length_scale, b.length_scale),
            radius_scale: lerp(a.radius_scale, b.radius_scale),
            slide: std::array::from_fn(|i| lerp(a.slide[i], b.slide[i])),
        }
    }

    /// This transform with only `weight` of it left: 0 is the rest pose and 1
    /// is exactly this one, bit for bit.
    pub fn pulled(&self, weight: f32) -> Self {
        if weight >= 1.0 {
            return *self;
        }
        let scale = |s: f32| 1.0 + (s - 1.0) * weight;
        Self {
            rotation: quat_nlerp(&Self::IDENTITY.rotation, &self.rotation, weight),
            length_scale: scale(self.length_scale),
            radius_scale: scale(self.radius_scale),
            slide: self.slide.map(|x| x * weight),
        }
    }
}

/// Where the hold-and-glide clock stands: `weight` of the way from pose
/// `index` to pose `index + 1`. A pose's index is its shape seed, so the
/// stream of poses has no end and a ring is only a window onto it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WarpedTime {
    /// The pose being held or left.
    pub index: u64,
    /// How far toward the next pose, 0 to 1.
    pub weight: f32,
}

impl WarpedTime {
    /// The design's t': the index plus the eased weight.
    pub fn value(self) -> f64 {
        self.index as f64 + f64::from(self.weight)
    }

    /// The design's formula, with time counted in steps. Before the start
    /// reads as the start.
    fn at_beat(beats: f64, glide: f32) -> Self {
        let (index, weight) = flint_core::wooze::warp(beats, glide);
        Self { index, weight }
    }
}

/// Each branch level's phase offset, as a fraction of a step. The #207 spike's
/// value.
pub const STAGGER_PER_LEVEL: f64 = 0.15;

/// The clock a living prop drifts on: hold, then glide to the next pose over
/// the last `glide` of each `step`, with its parts out of phase.
///
/// With a fixed step, as the offline tools have, a moment in seconds is enough
/// to say where it stands. A prop in a scene counts in beats instead and moves
/// on by `dt / step`, so that a new step bends the clock rather than jumping it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LivingClock {
    /// Seconds between pose targets.
    pub step: f32,
    /// How much of a step is spent easing, 0 to 1.
    pub glide: f32,
    /// How the parts fall out of phase.
    pub stagger: Stagger,
}

impl From<&LivingConfig> for LivingClock {
    fn from(config: &LivingConfig) -> Self {
        Self {
            step: config.step,
            glide: config.glide,
            stagger: config.stagger,
        }
    }
}

impl LivingClock {
    /// The clock at `t` seconds with no phase offset: the trunk side's.
    pub fn at(&self, t: f64) -> WarpedTime {
        WarpedTime::at_beat(t / f64::from(self.step), self.glide)
    }

    /// The clock at `t` seconds for one segment, with its phase offset.
    pub fn for_segment(&self, t: f64, segment: &BranchSegment) -> WarpedTime {
        WarpedTime::at_beat(t / f64::from(self.step) + self.phase(segment), self.glide)
    }

    /// How far ahead of the prop's own beat this segment runs, in beats.
    pub fn phase(&self, segment: &BranchSegment) -> f64 {
        match self.stagger {
            Stagger::None => 0.0,
            Stagger::PerLevel => f64::from(segment.depth) * STAGGER_PER_LEVEL,
            // The golden ratio spreads ids evenly around the cycle, so
            // neighbours are never in step and it doesn't depend on the count.
            Stagger::PerBranch => (f64::from(segment.id) * 0.618_033_988_749_894_9).fract(),
        }
    }

    /// [`for_segment`](Self::for_segment) for every segment, written over `out`.
    pub fn times_into(&self, t: f64, segments: &[BranchSegment], out: &mut Vec<WarpedTime>) {
        out.clear();
        out.extend(segments.iter().map(|s| self.for_segment(t, s)));
    }

    /// The drift-only pose shadows can follow (`[living] shadows = "drift"`): at
    /// half speed and all at once, it takes every other pose, and is a beat
    /// ahead so that what it leaves is never behind a ring's window. Given where
    /// the slow clock stands, a count of half beats, returns the two poses.
    pub fn slow_pair(slow_index: u64) -> (u64, u64) {
        let from = slow_index * 2 + 1;
        (from, from + 2)
    }

    /// How many steps past [`at`](Self::at)'s index a part of a tree this deep
    /// can be drawing on: ahead by its stagger, and blending toward the pose
    /// after that. A window of poses has to span this many steps and one more.
    pub fn reach(&self, max_depth: u32) -> u64 {
        self.reach_across(max_depth, 0.0)
    }

    /// [`reach`](Self::reach) for parts whose clocks stand as much as `span`
    /// steps apart: a prop's echoes, measured from the farthest behind.
    pub fn reach_across(&self, max_depth: u32, span: f64) -> u64 {
        let ahead = match self.stagger {
            Stagger::None => 0.0,
            Stagger::PerLevel => f64::from(max_depth) * STAGGER_PER_LEVEL,
            Stagger::PerBranch => 1.0,
        };
        (ahead + span).ceil() as u64 + 1
    }
}

/// A lineage's poses over time: generates each pose once, on first need, and
/// blends per segment. For offline tools, which can ask for any moment.
pub struct PoseStream<'a> {
    generator: &'a dyn LivingGenerator,
    spec: &'a ProcGenSpec,
    skeleton: &'a LivingSkeleton,
    clock: LivingClock,
    poses: BTreeMap<u64, LivingPose>,
}

impl<'a> PoseStream<'a> {
    /// A stream of `skeleton`'s poses on `clock`.
    pub fn new(
        generator: &'a dyn LivingGenerator,
        spec: &'a ProcGenSpec,
        skeleton: &'a LivingSkeleton,
        clock: LivingClock,
    ) -> Self {
        Self {
            generator,
            spec,
            skeleton,
            clock,
            poses: BTreeMap::new(),
        }
    }

    /// The prop at `t` seconds. It's a blend, so `shape_seed` names the pose
    /// the trunk side is leaving.
    pub fn at(&mut self, t: f64) -> Result<LivingPose> {
        let mut times = Vec::with_capacity(self.skeleton.segments.len());
        self.clock
            .times_into(t, &self.skeleton.segments, &mut times);
        for now in &times {
            self.ensure(now.index)?;
            self.ensure(now.index + 1)?;
        }
        let mut pose = LivingPose {
            shape_seed: self.clock.at(t).index,
            segments: Vec::new(),
        };
        let blended = pose.blend_from(&times, |index| self.poses.get(&index));
        debug_assert!(blended, "every pose was just made");
        Ok(pose)
    }

    fn ensure(&mut self, shape_seed: u64) -> Result<()> {
        if !self.poses.contains_key(&shape_seed) {
            let pose = self.generator.pose(self.spec, self.skeleton, shape_seed)?;
            self.poses.insert(shape_seed, pose);
        }
        Ok(())
    }
}

/// One variant of a prop: a transform per segment, in segment-id order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LivingPose {
    /// The shape seed this pose came from.
    pub shape_seed: u64,
    /// `segments[id]` is the transform of the skeleton's segment `id`.
    pub segments: Vec<SegmentPose>,
}

impl LivingPose {
    /// Become the blend `times` says, segment by segment: `times[id]` is how
    /// far segment `id` is between `poses(index)` and `poses(index + 1)`. This
    /// is the same in the runtime and in the strip, which rebuilds every frame
    /// on the CPU, so the strip is a check on what the bones draw.
    ///
    /// Returns false, with `self` of no use, if a pose it needs isn't there.
    pub fn blend_from<'p>(
        &mut self,
        times: &[WarpedTime],
        poses: impl Fn(u64) -> Option<&'p LivingPose>,
    ) -> bool {
        self.segments.clear();
        // Under any stagger a tree's parts are on a handful of steps, and a
        // ring's lookup isn't free when it's done twice a segment.
        let mut held: Option<(u64, &LivingPose, Option<&LivingPose>)> = None;
        for (i, now) in times.iter().enumerate() {
            let (from, to) = match held {
                Some((index, from, to)) if index == now.index => (from, to),
                _ => {
                    let Some(from) = poses(now.index) else {
                        return false;
                    };
                    let to = poses(now.index + 1);
                    held = Some((now.index, from, to));
                    (from, to)
                }
            };
            let segment = if now.weight <= 0.0 {
                from.segments[i]
            } else {
                let Some(to) = to else {
                    return false;
                };
                SegmentPose::blend(&from.segments[i], &to.segments[i], now.weight)
            };
            self.segments.push(segment);
        }
        true
    }
}

/// The fast layer: a leaf card's turn and a leaf shade's tint, which change on
/// a clock of their own and don't depend on the poses at all.
///
/// A value is a pure function of the prop's stream, what it's for and the step,
/// so nothing has to be generated ahead or kept, and it's the same on every run.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LivingJitter {
    /// Its clock; never staggered, since each value has its own phase.
    clock: LivingClock,
    /// `jitter.amplitude`, or 0 if `jitter.targets` left leaf cards out.
    amplitude: f32,
    /// How much of it this place shows: the wooze field's `jitter`.
    scale: f32,
}

impl From<&LivingConfig> for LivingJitter {
    fn from(config: &LivingConfig) -> Self {
        let on = config
            .jitter
            .targets
            .iter()
            .any(|target| target == JITTER_TARGETS[0]);
        Self {
            clock: LivingClock {
                step: config.jitter.step,
                glide: config.jitter.glide,
                stagger: Stagger::None,
            },
            amplitude: if on { config.jitter.amplitude } else { 0.0 },
            scale: 1.0,
        }
    }
}

impl LivingJitter {
    /// This jitter as a place with this `jitter` knob shows it.
    pub fn scaled(self, scale: f32) -> Self {
        Self { scale, ..self }
    }

    /// A value from -1 to 1. Each `channel` runs on its own phase: the golden
    /// ratio spreads them around the step, so a prop never jumps all at once.
    fn value(&self, stream: u64, channel: u64, t: f64) -> f32 {
        if self.amplitude <= 0.0 || self.scale <= 0.0 {
            return 0.0;
        }
        let phase = (channel as f64 * 0.618_033_988_749_894_9).fract();
        let now = self.clock.at(t + phase * f64::from(self.clock.step));
        let noise = |index: u64| {
            let hash = mix64(
                stream
                    ^ channel.wrapping_mul(0x9e37_79b9_7f4a_7c15)
                    ^ index.wrapping_mul(0xc2b2_ae3d_27d4_eb4f),
            );
            ((hash >> 32) as f64 / (1u64 << 31) as f64 - 1.0) as f32
        };
        let from = noise(now.index);
        if now.weight <= 0.0 {
            from
        } else {
            from + (noise(now.index + 1) - from) * now.weight
        }
    }

    /// The radians a jitter channel's cards are turned through at `t`.
    pub fn card_turn(&self, stream: u64, channel: usize, t: f64) -> f32 {
        self.value(stream, channel as u64, t)
            * self.amplitude
            * self.scale
            * CARD_TURN_PER_AMPLITUDE
    }

    /// A leaf shade's tint step at `t`, from `-TINT_STEPS` to `TINT_STEPS`.
    /// Its channels come after the cards', so a shade and a card never agree.
    ///
    /// The scale goes in before the rounding, so that a whole grove shares the
    /// same few materials wherever it stands in the field: more jitter means the
    /// far levels come up more often, not that there are more levels.
    pub fn tint_step(&self, stream: u64, part: usize, t: f64) -> i32 {
        let value = self.value(stream, (JITTER_CHANNELS + part) as u64, t) * self.scale;
        ((value * TINT_STEPS as f32).round() as i32).clamp(-TINT_STEPS, TINT_STEPS)
    }

    /// The multiplier a tint step stands for: warmer one way and cooler the
    /// other, so it reads as the colour shifting and not the light changing.
    pub fn tint(&self, step: i32) -> [f32; 3] {
        let shift = step as f32 / TINT_STEPS as f32 * self.amplitude * TINT_PER_AMPLITUDE;
        [1.0 + shift, 1.0 + shift * 0.25, 1.0 - shift]
    }
}

/// `turns` as the palette's jitter bones. A bone like this has no linear part
/// and a `w` of 0, so `M * p` is just its offset, whatever `p` is: added to a
/// segment bone under a vertex's weight, it moves the vertex without touching
/// its normal or the segment's pose.
pub fn boil_bones(turns: &[f32; JITTER_CHANNELS], out: &mut Vec<[[f32; 4]; 4]>) {
    for turn in turns {
        for axis in 0..3 {
            let mut offset = [0.0; 4];
            offset[axis] = *turn;
            out.push([[0.0; 4], [0.0; 4], [0.0; 4], offset]);
        }
    }
}

/// How one leaf-card vertex answers the jitter layer, at run time.
///
/// Turning a point `r` from its card's centre through a small angle `a` about
/// the card's normal `n` moves it by `a (n x r)`. That is a fixed vector times
/// one number that changes, and skinning is a weighted sum of matrices, so it
/// fits in the bone palette: the vector's three components are the vertex's
/// weights on a channel's three [`boil_bones`], whose offsets hold the angle.
/// So the cards turn with no new shader, no new vertex data and no per-card
/// state, and a card's corners keep their shape to within `a^2 / 2`.
///
/// These weights don't sum to 1, which glTF forbids, so they stay out of the
/// baked mesh and go up only to the renderer's own vertex buffer.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CardBoil {
    /// The channel's three bones.
    pub joints: [u16; 3],
    /// `n x r`, times how hard this card follows its channel.
    pub weights: [f32; 3],
}

/// A leaf card's place on the skeleton. It rides its segment rigidly.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct LeafBinding {
    /// The id of the segment nearest the card's centre.
    pub segment: u32,
    /// How far along that segment the nearest point is, 0 (start) to 1 (end).
    pub along: f32,
    /// From that point to the card's centre, in the rest pose.
    pub offset: Vec3,
}

/// What every pose of one lineage shares: the prop's identity.
#[derive(Debug, Clone, PartialEq)]
pub struct LivingSkeleton {
    /// The seed the structure grew from.
    pub lineage_seed: u64,
    /// The rest pose's segments, each with its stable id and parent. A
    /// parent always comes before its children.
    pub segments: Vec<BranchSegment>,
    /// The leaf cards in the rest pose, whole and in order: card `k` is
    /// `card_index_count` indices starting at `k * card_index_count`.
    pub leaf_mesh: MeshData,
    /// Indices one leaf card takes in `leaf_mesh`.
    pub card_index_count: usize,
    /// `leaf_bindings[k]` is where card `k` rides.
    pub leaf_bindings: Vec<LeafBinding>,
    /// The two axes, per segment and in the rest pose, that an echo's place
    /// is measured along (`[living] echo_places`): `form_v1`'s are along its
    /// lines of writing and up the form. Empty, as for a tree: every segment's
    /// are the prop's X and Y.
    pub place_axes: Vec<[[f32; 3]; 2]>,
}

impl LivingSkeleton {
    /// The vertices of leaf card `k`, each once.
    pub fn card_vertices(&self, k: usize) -> Vec<u32> {
        let first = k * self.card_index_count;
        let mut verts = self.leaf_mesh.indices[first..first + self.card_index_count].to_vec();
        verts.sort_unstable();
        verts.dedup();
        verts
    }

    /// How far segment `i` moves for an echo at `place` (`[along, up]` in
    /// metres along [`Self::place_axes`]), in the prop's space.
    pub fn place_offset(&self, i: usize, place: [f32; 2]) -> [f32; 3] {
        let [a, b] = self
            .place_axes
            .get(i)
            .copied()
            .unwrap_or([[1.0, 0.0, 0.0], [0.0, 1.0, 0.0]]);
        std::array::from_fn(|k| a[k] * place[0] + b[k] * place[1])
    }

    /// `pose` as an echo draws it: showing `slide` of its slides (see
    /// [`SegmentPose::slide`] and `[living] echo_slide`), and moved to
    /// `place` (`[living] echo_places`). A slide is absolute and doesn't
    /// carry, so a place that changes smoothly along the form shears it
    /// without parting it. The prop itself is drawn at its own slide and
    /// `[0, 0]`.
    pub fn show_at(&self, pose: &mut LivingPose, slide: f32, place: [f32; 2]) {
        if slide != 1.0 {
            for segment in &mut pose.segments {
                segment.slide = segment.slide.map(|x| x * slide);
            }
        }
        if place != [0.0; 2] {
            for (i, segment) in pose.segments.iter_mut().enumerate() {
                let by = self.place_offset(i, place);
                segment.slide = std::array::from_fn(|k| segment.slide[k] + by[k]);
            }
        }
    }

    /// The segments as `pose` holds them: each child starts at its parent's
    /// posed end, so the tree stays connected whatever the transforms say,
    /// and a segment off the trunk keeps its rest start.
    ///
    /// This also works on a blend of two poses, which is what keeps a blended
    /// tree whole.
    pub fn posed_segments(&self, pose: &LivingPose) -> Result<Vec<BranchSegment>> {
        let mut posed = Vec::with_capacity(self.segments.len());
        self.posed_segments_into(pose, &mut posed)?;
        Ok(posed)
    }

    /// [`posed_segments`](Self::posed_segments), written over `posed`.
    pub fn posed_segments_into(
        &self,
        pose: &LivingPose,
        posed: &mut Vec<BranchSegment>,
    ) -> Result<()> {
        self.check_pose(pose)?;
        posed.clear();
        for (rest, p) in self.segments.iter().zip(&pose.segments) {
            // A child starts at the end of its parent's unmoved chain: a slide
            // is the segment's own and doesn't carry.
            let start = match rest.parent {
                Some(parent) => {
                    let parent = parent as usize;
                    posed[parent].end - Vec3::from_array(pose.segments[parent].slide)
                }
                None => rest.start,
            };
            let dir = Vec3::from_array(quat_rotate_vec3(&p.rotation, rest.direction().to_array()));
            let end = start + dir * (rest.length() * p.length_scale);
            let slide = Vec3::from_array(p.slide);
            let (start, end) = (start + slide, end + slide);
            posed.push(BranchSegment {
                start,
                end,
                radius_start: rest.radius_start * p.radius_scale,
                radius_end: rest.radius_end * p.radius_scale,
                ..*rest
            });
        }
        Ok(())
    }

    /// How every leaf-card vertex answers the jitter layer, in vertex order,
    /// on the channels' bones from `first_bone`. A card takes one channel and
    /// one direction from its index, so neighbours don't turn together, and
    /// keeps one normal for all its vertices: a two-sided card's faces point
    /// opposite ways, and would otherwise turn apart.
    pub fn card_boil(&self, first_bone: u16) -> Vec<CardBoil> {
        let none = CardBoil {
            joints: [0; 3],
            weights: [0.0; 3],
        };
        let mut boil = vec![none; self.leaf_mesh.vertices.len()];
        for k in 0..self.leaf_bindings.len() {
            let verts = self.card_vertices(k);
            let Some(first) = verts.first() else {
                continue;
            };
            let at = |v: u32| Vec3::from_array(self.leaf_mesh.vertices[v as usize].position);
            let centre =
                verts.iter().fold(Vec3::ZERO, |sum, &v| sum + at(v)) * (1.0 / verts.len() as f32);
            let normal = Vec3::from_array(self.leaf_mesh.vertices[*first as usize].normal);

            let hash = mix64(k as u64);
            let channel = (hash % JITTER_CHANNELS as u64) as u16;
            let joints = std::array::from_fn(|axis| first_bone + channel * 3 + axis as u16);
            // From half to all of the channel's turn, either way round.
            let share = 0.5 + 0.5 * ((hash >> 8) & 0xff) as f32 / 255.0;
            let share = if hash & 0x10000 == 0 { share } else { -share };
            for &v in &verts {
                boil[v as usize] = CardBoil {
                    joints,
                    weights: (normal.cross(&(at(v) - centre)) * share).to_array(),
                };
            }
        }
        boil
    }

    /// The leaf mesh as `pose` holds it, each card carried rigidly by its
    /// segment. Vertex and card order match [`Self::leaf_mesh`].
    pub fn posed_leaf_mesh(&self, pose: &LivingPose) -> Result<MeshData> {
        let posed = self.posed_segments(pose)?;
        let mut mesh = self.leaf_mesh.clone();
        for k in 0..self.leaf_bindings.len() {
            let carry = self.card_carry(k, pose, &posed);
            for v in self.card_vertices(k) {
                let vertex = &mut mesh.vertices[v as usize];
                vertex.position = carry.place(Vec3::from_array(vertex.position)).to_array();
                vertex.normal = carry.turn(Vec3::from_array(vertex.normal)).to_array();
                let t = carry.turn(Vec3::new(
                    vertex.tangent[0],
                    vertex.tangent[1],
                    vertex.tangent[2],
                ));
                vertex.tangent = [t.x, t.y, t.z, vertex.tangent[3]];
            }
        }
        mesh.recompute_bounds();
        Ok(mesh)
    }

    /// The box around everything `pose` moves: the branches and the leaf
    /// cards. The trunk isn't in it, since a pose can't move the trunk.
    pub fn posed_bounds(&self, pose: &LivingPose) -> Result<BoundingBox> {
        let posed = self.posed_segments(pose)?;
        let mut points: Vec<[f32; 3]> =
            Vec::with_capacity(posed.len() * 2 + self.leaf_bindings.len() * self.card_index_count);
        for s in &posed {
            points.push(s.start.to_array());
            points.push(s.end.to_array());
        }
        for k in 0..self.leaf_bindings.len() {
            let carry = self.card_carry(k, pose, &posed);
            for v in self.card_vertices(k) {
                let at = Vec3::from_array(self.leaf_mesh.vertices[v as usize].position);
                points.push(carry.place(at).to_array());
            }
        }
        Ok(BoundingBox::from_positions(&points))
    }

    /// [`Self::posed_bounds`] for the rest pose, the size every pose is held to.
    pub fn rest_bounds(&self) -> BoundingBox {
        let rest = LivingPose {
            shape_seed: 0,
            segments: vec![SegmentPose::IDENTITY; self.segments.len()],
        };
        self.posed_bounds(&rest)
            .expect("the rest pose has one transform per segment")
    }

    /// The deepest level of the tree, which [`VarianceConfig::weight`] measures
    /// depth against.
    pub fn max_depth(&self) -> u32 {
        self.segments.iter().map(|s| s.depth).max().unwrap_or(0)
    }

    /// `full` held to `variance`: each segment pulled back toward the rest
    /// pose, the lineage's base, so only its weight of the difference is left.
    /// With `keep_size`, every segment's length then takes one shared scale
    /// that centres the box's sides on the rest pose's.
    ///
    /// One scale keeps the tree's proportions and leaves its limbs on the
    /// trunk, and that is why the box can't match on every side at once. Takes
    /// the pose from [`full_pose`](crate::LivingGenerator::full_pose).
    pub fn with_variance(
        &self,
        full: &LivingPose,
        variance: &VarianceConfig,
    ) -> Result<LivingPose> {
        self.check_pose(full)?;
        let max_depth = self.max_depth();
        let mut pose = LivingPose {
            shape_seed: full.shape_seed,
            segments: self
                .segments
                .iter()
                .zip(&full.segments)
                .map(|(rest, p)| p.pulled(variance.weight(rest.depth, max_depth)))
                .collect(),
        };
        if variance.keep_size {
            self.hold_size(&mut pose)?;
        }
        Ok(pose)
    }

    /// Scale `pose`'s lengths until its box's most stretched side is over the
    /// rest pose's by the same fraction its most squeezed side is under.
    ///
    /// A slide moves the form without changing its size, so the size is
    /// measured with the slides left out: a shear would otherwise widen the
    /// box and read as stretch.
    fn hold_size(&self, pose: &mut LivingPose) -> Result<()> {
        let rest = self.rest_bounds();
        // Only a pose with a slide pays for the copy.
        let mut unslid = pose.segments.iter().any(|s| s.slide != [0.0; 3]).then(|| {
            let mut unslid = pose.clone();
            for s in &mut unslid.segments {
                s.slide = [0.0; 3];
            }
            unslid
        });
        for _ in 0..KEEP_SIZE_ROUNDS {
            let measured = unslid.as_ref().unwrap_or(pose);
            let scale = balancing_scale(&rest, &self.posed_bounds(measured)?);
            for s in &mut pose.segments {
                s.length_scale *= scale;
            }
            for s in unslid.iter_mut().flat_map(|u| &mut u.segments) {
                s.length_scale *= scale;
            }
        }
        Ok(())
    }

    /// How `pose` carries leaf card `k`, given the segments it poses.
    fn card_carry(&self, k: usize, pose: &LivingPose, posed: &[BranchSegment]) -> CardCarry {
        let card = &self.leaf_bindings[k];
        let (rest, now) = (
            &self.segments[card.segment as usize],
            &posed[card.segment as usize],
        );
        let rotation = pose.segments[card.segment as usize].rotation;
        let turned = Vec3::from_array(quat_rotate_vec3(&rotation, card.offset.to_array()));
        CardCarry {
            rotation,
            rest_centre: rest.start + (rest.end - rest.start) * card.along + card.offset,
            centre: now.start + (now.end - now.start) * card.along + turned,
        }
    }

    fn check_pose(&self, pose: &LivingPose) -> Result<()> {
        if pose.segments.len() == self.segments.len() {
            return Ok(());
        }
        Err(ProcGenError::InvalidParameter {
            name: "pose".into(),
            reason: format!(
                "it has {} segments and the skeleton has {}: a pose of another lineage or spec",
                pose.segments.len(),
                self.segments.len()
            ),
        })
    }
}

/// The rigid move a pose gives one leaf card.
struct CardCarry {
    rotation: [f32; 4],
    rest_centre: Vec3,
    centre: Vec3,
}

impl CardCarry {
    fn turn(&self, v: Vec3) -> Vec3 {
        Vec3::from_array(quat_rotate_vec3(&self.rotation, v.to_array()))
    }

    /// Where a point of the card in the rest pose ends up.
    fn place(&self, rest: Vec3) -> Vec3 {
        self.centre + self.turn(rest - self.rest_centre)
    }
}

/// The scale that centres `now`'s sides on `rest`'s: dividing by the midpoint
/// of the sides' ratios puts the largest and smallest equally far from 1. A
/// side with no length in the rest pose has no ratio and doesn't count.
fn balancing_scale(rest: &BoundingBox, now: &BoundingBox) -> f32 {
    let (rest, now) = (
        (rest.max - rest.min).to_array(),
        (now.max - now.min).to_array(),
    );
    let (mut least, mut most) = (f32::INFINITY, 0.0f32);
    for (now, rest) in now.iter().zip(&rest) {
        if *rest > 1e-6 && *now > 1e-6 {
            let ratio = now / rest;
            least = least.min(ratio);
            most = most.max(ratio);
        }
    }
    if most > 0.0 {
        2.0 / (least + most)
    } else {
        1.0
    }
}

/// One lineage's skinned rest mesh and how its poses reach the bones.
///
/// Bone 0 is the trunk, which every pose leaves still. After it comes a bone
/// per segment, parents first, unless the tree has more segments than
/// [`MAX_SKINNED_BONES`](crate::MAX_SKINNED_BONES) leaves room for: then its
/// smallest twigs ride the bone of their nearest ancestor.
#[derive(Debug, Clone, PartialEq)]
pub struct LivingRig {
    /// The rest mesh, in the same place as the tree `generate` would bake.
    pub mesh: SkinnedMeshData,
    /// `segment_bones[id]` is the bone that moves segment `id`.
    pub segment_bones: Vec<u16>,
    /// `bone_segments[bone]` is the segment a bone follows; `None` for the trunk.
    pub bone_segments: Vec<Option<u32>>,
    /// The mesh's first leaf-card vertex; the bark comes before it.
    pub first_leaf_vertex: usize,
    /// `card_boil[v]` is how vertex `first_leaf_vertex + v` answers the jitter.
    pub card_boil: Vec<CardBoil>,
    /// Which parts are leaves, counting the sub-meshes that have any indices.
    pub leaf_parts: Vec<usize>,
}

impl LivingRig {
    /// The parts that aren't leaves (a tree's bark, a form's body), counting as
    /// [`Self::leaf_parts`] does: what a prop's softness (#582) feathers.
    pub fn body_parts(&self) -> impl Iterator<Item = usize> + '_ {
        let parts = self
            .mesh
            .submeshes
            .iter()
            .filter(|sub| sub.index_count > 0)
            .count()
            .max(1);
        (0..parts).filter(|part| !self.leaf_parts.contains(part))
    }

    /// `pose` as bone matrices (column-major, what the skinned pipeline
    /// uploads), written over `out`. Each takes a rest vertex to where the
    /// pose puts it, and all of a pose's scale is in here: the skeleton's
    /// local transforms can't hold it, since a parent's stretch would shear
    /// its children.
    ///
    /// A segment's bone turns it by the pose's rotation, stretches it along
    /// itself by the length scale and across by the radius scale, and puts
    /// its head on the posed end of its parent. So a tube stays joined under
    /// any pose, and blending two poses first keeps that true.
    pub fn palette(
        &self,
        skeleton: &LivingSkeleton,
        pose: &LivingPose,
        out: &mut Vec<[[f32; 4]; 4]>,
    ) -> Result<()> {
        self.palette_into(skeleton, pose, &mut Vec::new(), out)
    }

    /// [`palette`](Self::palette) with somewhere to keep the posed segments, so
    /// that a frame's worth of props allocates nothing.
    pub fn palette_into(
        &self,
        skeleton: &LivingSkeleton,
        pose: &LivingPose,
        posed: &mut Vec<BranchSegment>,
        out: &mut Vec<[[f32; 4]; 4]>,
    ) -> Result<()> {
        skeleton.posed_segments_into(pose, posed)?;
        out.clear();
        out.extend(self.bone_segments.iter().map(|segment| match segment {
            None => IDENTITY_BONE,
            Some(id) => {
                let id = *id as usize;
                let (rest, p) = (&skeleton.segments[id], &pose.segments[id]);
                segment_bone(rest.start, rest.direction(), p, posed[id].start)
            }
        }));
        Ok(())
    }

    /// A vertex's influences as the renderer should have them: its own, and on
    /// a leaf card, the jitter's as well. The card has one segment bone, so
    /// it has three slots free.
    pub fn runtime_joints(&self, vertex: usize) -> ([u16; 4], [f32; 4]) {
        let v = &self.mesh.vertices[vertex];
        match vertex
            .checked_sub(self.first_leaf_vertex)
            .and_then(|leaf| self.card_boil.get(leaf))
        {
            Some(boil) => (
                [
                    v.joint_indices[0],
                    boil.joints[0],
                    boil.joints[1],
                    boil.joints[2],
                ],
                [
                    v.joint_weights[0],
                    boil.weights[0],
                    boil.weights[1],
                    boil.weights[2],
                ],
            ),
            None => (v.joint_indices, v.joint_weights),
        }
    }

    /// The rest mesh moved by `palette` on the CPU, the way the GPU does it:
    /// a mesh to look at or measure, not a way to draw a pose every frame.
    pub fn skinned_mesh(&self, palette: &[[[f32; 4]; 4]]) -> MeshData {
        let mut mesh = self.mesh.rest_mesh();
        for (out, v) in mesh.vertices.iter_mut().zip(&self.mesh.vertices) {
            let mut blend = [[0.0f32; 4]; 4];
            for (joint, weight) in v.joint_indices.iter().zip(&v.joint_weights) {
                if *weight > 0.0 {
                    for (column, bone) in blend.iter_mut().zip(&palette[*joint as usize]) {
                        for (c, b) in column.iter_mut().zip(bone) {
                            *c += b * weight;
                        }
                    }
                }
            }
            let apply = |p: [f32; 3], w: f32| -> [f32; 3] {
                std::array::from_fn(|r| {
                    blend[0][r] * p[0] + blend[1][r] * p[1] + blend[2][r] * p[2] + blend[3][r] * w
                })
            };
            out.position = apply(v.position, 1.0);
            out.normal = Vec3::from_array(apply(v.normal, 0.0))
                .normalized()
                .to_array();
            let t = Vec3::from_array(apply([v.tangent[0], v.tangent[1], v.tangent[2]], 0.0))
                .normalized();
            out.tangent = [t.x, t.y, t.z, v.tangent[3]];
        }
        mesh.recompute_bounds();
        mesh
    }
}

/// Every bit of `h` spread across all of them (MurmurHash3's finalizer),
/// stable across runs and platforms.
pub(crate) fn mix64(h: u64) -> u64 {
    let mut h = h ^ (h >> 33);
    h = h.wrapping_mul(0xff51_afd7_ed55_8ccd);
    h ^= h >> 33;
    h = h.wrapping_mul(0xc4ce_b9fe_1a85_ec53);
    h ^ (h >> 33)
}

const IDENTITY_BONE: [[f32; 4]; 4] = [
    [1.0, 0.0, 0.0, 0.0],
    [0.0, 1.0, 0.0, 0.0],
    [0.0, 0.0, 1.0, 0.0],
    [0.0, 0.0, 0.0, 1.0],
];

/// The bone matrix for a segment leaving `head` along unit `dir` at rest, and
/// leaving `posed_head` under `p`: turn * stretch about the head, where a
/// stretch of `l` along `dir` and `r` across it is `r I + (l - r) dir dir^T`.
fn segment_bone(head: Vec3, dir: Vec3, p: &SegmentPose, posed_head: Vec3) -> [[f32; 4]; 4] {
    let (l, r) = (p.length_scale, p.radius_scale);
    let d = dir.to_array();
    let mut linear = [[0.0f32; 3]; 3];
    for (c, column) in linear.iter_mut().enumerate() {
        let mut stretched = [0.0f32; 3];
        for (row, s) in stretched.iter_mut().enumerate() {
            *s = (l - r) * d[row] * d[c] + if row == c { r } else { 0.0 };
        }
        *column = quat_rotate_vec3(&p.rotation, stretched);
    }
    let h = head.to_array();
    let moved: [f32; 3] =
        std::array::from_fn(|r| linear[0][r] * h[0] + linear[1][r] * h[1] + linear[2][r] * h[2]);
    let translation = posed_head.to_array();
    [
        [linear[0][0], linear[0][1], linear[0][2], 0.0],
        [linear[1][0], linear[1][1], linear[1][2], 0.0],
        [linear[2][0], linear[2][1], linear[2][2], 0.0],
        [
            translation[0] - moved[0],
            translation[1] - moved[1],
            translation[2] - moved[2],
            1.0,
        ],
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clock(glide: f32, stagger: Stagger) -> LivingClock {
        LivingClock {
            step: 0.5,
            glide,
            stagger,
        }
    }

    fn segment(id: u32, depth: u32) -> BranchSegment {
        BranchSegment {
            start: Vec3::ZERO,
            end: Vec3::UP,
            radius_start: 0.1,
            radius_end: 0.1,
            depth,
            id,
            parent: None,
        }
    }

    #[test]
    fn glide_zero_holds_for_the_whole_step_then_snaps() {
        let c = clock(0.0, Stagger::None);
        for t in [0.0, 0.1, 0.25, 0.49] {
            assert_eq!(
                c.at(t),
                WarpedTime {
                    index: 0,
                    weight: 0.0
                },
                "t = {t}"
            );
        }
        assert_eq!(
            c.at(0.5),
            WarpedTime {
                index: 1,
                weight: 0.0
            }
        );
        assert_eq!(
            c.at(1.75),
            WarpedTime {
                index: 3,
                weight: 0.0
            }
        );
    }

    #[test]
    fn glide_one_never_holds_and_eases_at_both_ends() {
        let c = clock(1.0, Stagger::None);
        assert_eq!(c.at(0.0).weight, 0.0);
        assert_eq!(c.at(0.25).weight, 0.5, "smoothstep's middle");
        // Slow, fast, slow: the first and last tenths move least.
        let w = |t: f64| f64::from(c.at(t).weight);
        let (start, middle, end) = (
            w(0.05) - w(0.0),
            w(0.275) - w(0.225),
            w(0.5 - 1e-9) - w(0.45),
        );
        assert!(start < middle && end < middle, "{start} {middle} {end}");
    }

    #[test]
    fn a_partial_glide_holds_first_and_arrives_on_the_beat() {
        // The design's formula with g = 0.3: a hold for 70% of the step.
        let c = clock(0.3, Stagger::None);
        assert_eq!(c.at(0.34).weight, 0.0);
        assert!(c.at(0.36).weight > 0.0);
        assert_eq!(c.at(0.425).weight, 0.5);
        assert!(c.at(0.5 - 1e-9).weight > 0.999);
    }

    #[test]
    fn warped_time_is_continuous_across_a_beat() {
        for glide in [0.1, 0.3, 0.6, 1.0] {
            let c = clock(glide, Stagger::None);
            for beat in 1..6 {
                let t = f64::from(beat) * 0.5;
                let (before, after) = (c.at(t - 1e-9).value(), c.at(t).value());
                assert!((before - after).abs() < 1e-3, "{glide} {beat}");
            }
        }
    }

    #[test]
    fn warped_time_never_runs_backwards() {
        for glide in [0.0, 0.3, 1.0] {
            let c = clock(glide, Stagger::None);
            let mut last = 0.0;
            for i in 0..2000 {
                let now = c.at(f64::from(i) * 0.001).value();
                assert!(now >= last, "{glide} at {i}");
                last = now;
            }
        }
    }

    #[test]
    fn stagger_offsets_by_depth_or_id_or_not_at_all() {
        let deep = segment(9, 3);

        let c = clock(1.0, Stagger::None);
        assert_eq!(c.for_segment(0.1, &deep), c.at(0.1));

        let c = clock(1.0, Stagger::PerLevel);
        let ahead = 3.0 * STAGGER_PER_LEVEL * 0.5;
        assert_eq!(c.for_segment(0.1, &deep), c.at(0.1 + ahead));
        assert_eq!(
            c.for_segment(0.1, &segment(0, 3)),
            c.for_segment(0.1, &deep)
        );
        assert_ne!(
            c.for_segment(0.1, &segment(0, 2)),
            c.for_segment(0.1, &deep)
        );

        // Each segment on its own phase, whatever its depth.
        let c = clock(1.0, Stagger::PerBranch);
        assert_ne!(
            c.for_segment(0.1, &segment(1, 3)),
            c.for_segment(0.1, &segment(2, 3))
        );
        assert_eq!(
            c.for_segment(0.1, &segment(1, 3)),
            c.for_segment(0.1, &segment(1, 0))
        );
    }

    #[test]
    fn the_clock_reads_its_settings_from_the_spec() {
        let spec = ProcGenSpec::parse(
            r#"
generator = "tree_v1"
[meta]
name = "t"
version = "1.0.0"
[seed]
mode = "fixed"
value = 1
[living]
lineage_seed = 7
step = 0.25
glide = 0.8
stagger = "none"
"#,
        )
        .unwrap();
        let c = LivingClock::from(spec.living.as_ref().unwrap());
        assert_eq!(
            c,
            LivingClock {
                step: 0.25,
                glide: 0.8,
                stagger: Stagger::None
            }
        );
    }

    #[test]
    fn blend_lands_on_each_end_and_stays_a_unit_rotation() {
        let a = SegmentPose::IDENTITY;
        let b = SegmentPose {
            rotation: flint_core::quat::quat_from_axis_angle([0.0, 0.0, 1.0], 1.2),
            length_scale: 1.4,
            radius_scale: 0.6,
            slide: [0.2, 0.0, -0.4],
        };
        assert_eq!(SegmentPose::blend(&a, &b, 0.0), a);
        assert_eq!(SegmentPose::blend(&a, &b, 1.0), b);

        let mid = SegmentPose::blend(&a, &b, 0.5);
        assert!((mid.length_scale - 1.2).abs() < 1e-6);
        assert!((mid.radius_scale - 0.8).abs() < 1e-6);
        assert!((mid.slide[0] - 0.1).abs() < 1e-6);
        assert!((mid.slide[2] + 0.2).abs() < 1e-6);
        let norm: f32 = mid.rotation.iter().map(|x| x * x).sum();
        assert!((norm - 1.0).abs() < 1e-5);
    }

    fn pose_of(seed: u64, turn: f32, count: usize) -> LivingPose {
        LivingPose {
            shape_seed: seed,
            segments: vec![
                SegmentPose {
                    rotation: flint_core::quat::quat_from_axis_angle([0.0, 0.0, 1.0], turn),
                    length_scale: 1.0 + turn,
                    radius_scale: 1.0,
                    slide: [0.0; 3],
                };
                count
            ],
        }
    }

    #[test]
    fn blend_from_reads_each_part_from_its_own_two_poses() {
        let poses = [pose_of(0, 0.1, 3), pose_of(1, 0.2, 3), pose_of(2, 0.3, 3)];
        let times = [
            WarpedTime {
                index: 0,
                weight: 0.0,
            },
            WarpedTime {
                index: 0,
                weight: 0.5,
            },
            WarpedTime {
                index: 1,
                weight: 1.0,
            },
        ];
        let mut out = pose_of(9, 0.0, 0);
        assert!(out.blend_from(&times, |i| poses.get(i as usize)));
        assert_eq!(out.segments[0], poses[0].segments[0], "a hold is its pose");
        assert_eq!(
            out.segments[1],
            SegmentPose::blend(&poses[0].segments[1], &poses[1].segments[1], 0.5)
        );
        assert_eq!(
            out.segments[2], poses[2].segments[2],
            "an arrival is its pose"
        );
    }

    #[test]
    fn blend_from_says_when_a_pose_it_needs_is_missing() {
        let poses = [pose_of(0, 0.1, 1)];
        let at = |weight| [WarpedTime { index: 0, weight }];
        let mut out = pose_of(9, 0.0, 0);
        assert!(
            out.blend_from(&at(0.0), |i| poses.get(i as usize)),
            "a hold doesn't need the next pose"
        );
        assert!(!out.blend_from(&at(0.5), |i| poses.get(i as usize)));
        assert!(!out.blend_from(&at(0.0), |_| None));
    }

    fn jitter(amplitude: f32, step: f32, glide: f32) -> LivingJitter {
        LivingJitter {
            clock: LivingClock {
                step,
                glide,
                stagger: Stagger::None,
            },
            amplitude,
            scale: 1.0,
        }
    }

    #[test]
    fn jitter_snaps_on_its_own_beat_and_each_channel_on_its_own_phase() {
        let j = jitter(0.05, 0.125, 0.0);
        let mut changes = [0usize; JITTER_CHANNELS];
        let mut when: Vec<Vec<usize>> = vec![Vec::new(); JITTER_CHANNELS];
        for frame in 1..240 {
            let (now, then) = (f64::from(frame) / 60.0, f64::from(frame - 1) / 60.0);
            for channel in 0..JITTER_CHANNELS {
                if j.card_turn(77, channel, now) != j.card_turn(77, channel, then) {
                    changes[channel] += 1;
                    when[channel].push(frame as usize);
                }
            }
        }
        // 4 s at 8 beats a second: it holds between them, and it does change.
        for count in changes {
            assert!((28..=32).contains(&count), "{changes:?}");
        }
        assert_ne!(when[0], when[1], "channels jumping together");
        assert_ne!(when[1], when[2], "channels jumping together");
    }

    #[test]
    fn jitter_is_bounded_repeatable_and_silent_at_no_amplitude() {
        let j = jitter(0.05, 0.125, 0.0);
        for i in 0..500 {
            let t = f64::from(i) * 0.01;
            let turn = j.card_turn(5, 2, t);
            assert!(turn.abs() <= 0.05 * CARD_TURN_PER_AMPLITUDE, "{turn}");
            assert_eq!(turn, j.card_turn(5, 2, t), "the same value every time");
            let step = j.tint_step(5, 1, t);
            assert!(step.abs() <= TINT_STEPS, "{step}");
        }
        assert_ne!(j.card_turn(5, 0, 1.0), j.card_turn(6, 0, 1.0), "by stream");

        let still = jitter(0.0, 0.125, 0.0);
        assert_eq!(still.card_turn(5, 2, 3.0), 0.0);
        assert_eq!(still.tint_step(5, 2, 3.0), 0);
    }

    #[test]
    fn jitter_can_glide_when_a_spec_asks() {
        let j = jitter(0.05, 0.125, 1.0);
        for frame in 1..400 {
            let (now, then) = (f64::from(frame) / 240.0, f64::from(frame - 1) / 240.0);
            let moved = (j.card_turn(5, 0, now) - j.card_turn(5, 0, then)).abs();
            // A full swing is twice the bound, over 30 samples, at a smoothstep's
            // peak of 1.5 times the mean.
            let bound = 2.0 * 0.05 * CARD_TURN_PER_AMPLITUDE * 1.5 / 30.0;
            assert!(moved <= bound * 1.01, "{moved} at {frame}");
        }
    }

    #[test]
    fn a_tint_shifts_warm_or_cool_and_neutral_is_exact() {
        let j = jitter(0.05, 0.125, 0.0);
        assert_eq!(j.tint(0), [1.0, 1.0, 1.0]);
        let warm = j.tint(TINT_STEPS);
        assert!((warm[0] - 1.1).abs() < 1e-6 && (warm[2] - 0.9).abs() < 1e-6);
        let cool = j.tint(-TINT_STEPS);
        assert!(cool[0] < 1.0 && cool[2] > 1.0);
    }

    #[test]
    fn a_boil_bone_is_an_offset_that_ignores_where_it_is_applied() {
        let mut bones = Vec::new();
        boil_bones(&[0.1, 0.2, 0.3, 0.4], &mut bones);
        assert_eq!(bones.len(), BOIL_BONES);
        // Channel 1's y bone: a turn of 0.2 along y, and nothing else.
        let m = &bones[4];
        for column in &m[..3] {
            assert_eq!(*column, [0.0; 4], "a linear part would bend the normals");
        }
        assert_eq!(m[3], [0.0, 0.2, 0.0, 0.0], "a w would change the point's w");
    }

    #[test]
    fn the_slow_pair_stays_ahead_of_the_window_and_arrives_where_it_next_leaves() {
        let c = clock(0.3, Stagger::PerLevel);
        let slow = |t: f64| {
            let now = WarpedTime::at_beat(t / (2.0 * f64::from(c.step)), c.glide);
            let (from, to) = LivingClock::slow_pair(now.index);
            (from, to, now.weight)
        };
        let mut last = slow(0.0);
        for frame in 1..3000 {
            let t = f64::from(frame) / 60.0;
            let (from, to, weight) = slow(t);
            let first = c.at(t).index;
            assert!(from >= first, "{from} left the window at {first}");
            assert!(to - first <= 3, "{to} is past a ring of 4 at {first}");
            if from != last.0 {
                assert_eq!(from, last.1, "takes up where it arrived");
                assert!(last.2 > 0.99, "and only once it has arrived: {}", last.2);
                assert_eq!(weight, 0.0);
            }
            last = (from, to, weight);
        }
    }
}
