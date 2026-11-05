//! Living props at run time (#211): each prop's window of poses, kept full a
//! few steps ahead of its clock within the procgen frame budget.
//!
//! A lineage is one spec and lineage seed: a [`LivingSkeleton`] and a
//! [`LivingRig`], shared by every prop that grows from it. A prop is one entity
//! on a lineage, with its own stream of shape seeds and its own [`PoseRing`].
//!
//! Poses don't go through [`ProcGenQueue`](crate::ProcGenQueue). That queue
//! dedupes by spec name and caches by content hash, so a ring of poses of one
//! spec would collapse into one request, and its cache would have poses evict
//! meshes. They share its frame budget instead: this gets whatever the queue
//! left of the frame.
//!
//! What a prop looks like never depends on when its poses were made. A shape
//! seed comes from the entity's *name* and the step index, never from an
//! entity id or the wall clock, so the budget only decides which frame a pose
//! is ready on, and never which pose it is.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use flint_core::wooze::{Cycle, WoozeField, WoozeSample};
use flint_core::{ContentHash, EntityId};
use web_time::Instant;

use crate::generator::LivingGenerator;
use crate::living::{
    boil_bones, mix64, LivingClock, LivingJitter, LivingPose, LivingRig, LivingSkeleton,
    SegmentPose, WarpedTime, BOIL_BONES, JITTER_CHANNELS,
};
use crate::registry::GeneratorRegistry;
use crate::spec::{EchoConfig, EchoCopy, ProcGenSpec, Stagger};
use crate::types::BranchSegment;

/// How many bytes of poses the cache keeps once no ring holds them. A pose of
/// the oak is under 6 KB, so this is about 1,400 of them: a grove's rings four
/// times over.
pub const POSE_CACHE_BYTES: usize = 8 * 1024 * 1024;

/// How fast the pose estimate forgets its slowest pose, each pose. Poses vary
/// by a factor of two, so an estimate that forgets within a frame or two lets a
/// slow one overrun the budget. At this rate it takes about a second of a grove's
/// poses to halve.
const POSE_ESTIMATE_DECAY: f64 = 0.99;

/// What a scene's `living_prop` component asks for. Anything left `None` comes
/// from the spec's `[living]` table, except the phase offset, which comes from
/// the entity's name so that a grove doesn't move in step.
#[derive(Debug, Clone, PartialEq)]
pub struct LivingPropRequest {
    /// The entity this prop is.
    pub entity: EntityId,
    /// Its name, which seeds its stream of poses.
    pub entity_name: String,
    /// Where it stands in the world, which is where it reads the wooze field.
    pub position: [f32; 3],
    /// The spec's `meta.name`.
    pub spec_name: String,
    /// Which tree this is. `None` for the spec's `lineage_seed`.
    pub lineage_seed: Option<u64>,
    /// How many poses to keep ready. `None` for the spec's `pose_ring`.
    pub pose_ring: Option<u32>,
    /// How far into its step cycle this prop runs, as a fraction of a step.
    pub phase_offset: Option<f32>,
    /// How its parts fall out of phase. `None` for the spec's `stagger`.
    pub stagger: Option<Stagger>,
}

/// What the shadows of a scene's living props follow, which is its own choice:
/// a whole tree's shadow boiling can read as life or as noise.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum LivingShadows {
    /// Exactly what's drawn, stagger and leaf jitter included. No extra cost.
    #[default]
    Blended,
    /// A slower pose with no stagger and no jitter, in bones of its own.
    Drift,
}

/// What a scene says about its living props, ahead of any one spec. How woozy
/// they are is the wooze field's to say (#213).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct LivingDefaults {
    /// What the shadows follow.
    pub shadows: LivingShadows,
    /// The scene's word on echoes, over each spec's, a field at a time.
    pub echoes: EchoOverride,
}

/// A scene's `[living] echoes`: whichever of a spec's fields it sets. The wooze
/// field (#213) has no knob for these yet.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct EchoOverride {
    /// How many copies. `None`: the spec's.
    pub count: Option<u32>,
    /// How far the farthest copy is from now, in steps. `None`: the spec's.
    pub spread: Option<f32>,
    /// The opacity of a copy at now. `None`: the spec's.
    pub alpha: Option<f32>,
}

impl EchoOverride {
    fn over(&self, spec: EchoConfig) -> EchoConfig {
        EchoConfig {
            count: self.count.unwrap_or(spec.count),
            spread: self.spread.unwrap_or(spec.spread),
            alpha: self.alpha.unwrap_or(spec.alpha),
        }
    }
}

/// Room for one prop's blend at a time, kept across props and frames so that
/// blending a grove allocates nothing.
#[derive(Debug, Default)]
pub struct BlendFrame {
    times: Vec<WarpedTime>,
    pose: Option<LivingPose>,
    posed: Vec<BranchSegment>,
    /// The bone matrices to write, from [`PropBlend::first_bone`].
    pub bones: Vec<[[f32; 4]; 4]>,
    /// The shadow's bone matrices, whole, if [`PropBlend::shadow_changed`].
    pub shadow_bones: Vec<[[f32; 4]; 4]>,
    /// Each leaf part whose tint has changed, with the tint.
    pub tints: Vec<(usize, [f32; 3])>,
}

/// What one prop needs uploaded this frame; see [`LivingProps::blend_prop`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PropBlend {
    /// The entity whose buffers these are.
    pub entity: EntityId,
    /// Where [`BlendFrame::bones`] goes; `None` if the bones haven't changed.
    pub first_bone: Option<usize>,
    /// Whether to write [`BlendFrame::shadow_bones`].
    pub shadow_changed: bool,
}

/// What a prop's bones and materials last held, so that nothing moved means
/// nothing computed and nothing uploaded: a prop holding still is free.
#[derive(Debug, Clone, Default)]
struct DrawnState {
    bones: DrawnBones,
    tints: Vec<i32>,
    shadow: Option<(u64, f32)>,
}

/// What one bone buffer last held: a prop's, or one of its echoes'.
#[derive(Debug, Clone, Default)]
struct DrawnBones {
    times: Vec<WarpedTime>,
    turns: [f32; JITTER_CHANNELS],
    /// Each part's cycle, by segment id. Holds what the part started it with, for
    /// this buffer alone: an echo reaches each beat at a moment of its own.
    parts: Vec<Cycle>,
}

/// One faint copy of a prop, and what its own bones last held.
#[derive(Debug, Clone)]
struct PropEcho {
    copy: EchoCopy,
    drawn: DrawnBones,
}

/// A prop's place in [`LivingProps`]; stays valid until the props are cleared.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LivingPropId(pub usize);

/// A lineage that has just finished building, for the caller to upload.
#[derive(Debug, Clone)]
pub struct LineageDone {
    /// The name its mesh goes under, the one every prop's model now asks for.
    pub asset_name: String,
    /// Its rig, whose `mesh` is the skinned rest mesh; or why it couldn't be made.
    pub rig: std::result::Result<Arc<LivingRig>, String>,
    /// The rest pose as bone matrices, for the props' own bone buffers.
    pub rest_palette: Vec<[[f32; 4]; 4]>,
    /// The props drawn from it.
    pub entities: Vec<EntityId>,
}

/// How the rings have fared, for `--exit-after`'s report.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct LivingStats {
    /// Frames ticked.
    pub frames: u64,
    /// Frames that ended with every ring full.
    pub frames_full: u64,
    /// The most any ring has been short at the end of a frame.
    pub most_short: usize,
    /// Frames that ended with some prop missing a pose its clock is using now.
    pub frames_starved: u64,
    /// Poses generated.
    pub poses_made: u64,
    /// Poses found in the cache.
    pub poses_from_cache: u64,
    /// Poses that couldn't be made, which hold the rest pose instead.
    pub pose_failures: u64,
    /// Frames a prop kept its last bones, for want of a pose. Counted per prop.
    pub blends_starved: u64,
    /// Frames blending has run.
    pub blend_frames: u64,
    /// Milliseconds spent blending and uploading, over every frame.
    pub blend_total_ms: f64,
    /// The most milliseconds spent blending and uploading in one frame.
    pub blend_worst_ms: f64,
    /// Frames that spent over 1 ms blending and uploading.
    pub blend_frames_over_1ms: u64,
    /// Bone buffer writes, over every frame.
    pub bone_writes: u64,
    /// Milliseconds spent over every frame.
    pub total_ms: f64,
    /// The most milliseconds spent in one frame.
    pub worst_ms: f64,
    /// Frames that spent more than the budget left to them.
    pub frames_over_budget: u64,
}

/// A window onto a prop's endless stream of poses: `first` and the steps after
/// it, in fixed slots, so sliding it along allocates nothing.
///
/// There's one slot more than the ring is asked to hold: a spare on deck, made
/// whenever the frame has room. So a beat, which takes the oldest pose away,
/// leaves a full ring and not one that is short until the queue catches up, and
/// poses get made across the quiet frames between beats.
#[derive(Debug, Clone)]
struct PoseRing {
    first: u64,
    slots: Vec<Option<Arc<LivingPose>>>,
    ready: usize,
}

impl PoseRing {
    /// A ring that holds `len` poses, with its spare.
    fn new(first: u64, len: usize) -> Self {
        Self {
            first,
            slots: vec![None; len + 1],
            ready: 0,
        }
    }

    /// How many poses it holds when full, not counting the spare.
    fn len(&self) -> usize {
        self.slots.len() - 1
    }

    fn slot(&self, index: u64) -> usize {
        (index % self.slots.len() as u64) as usize
    }

    fn contains(&self, index: u64) -> bool {
        index >= self.first && index - self.first < self.slots.len() as u64
    }

    fn get(&self, index: u64) -> Option<&Arc<LivingPose>> {
        if !self.contains(index) {
            return None;
        }
        self.slots[self.slot(index)].as_ref()
    }

    /// Move the window forward, dropping the poses it leaves behind.
    fn slide_to(&mut self, first: u64) {
        if first <= self.first {
            return;
        }
        let leaving = (first - self.first).min(self.slots.len() as u64);
        for index in self.first..self.first + leaving {
            let slot = self.slot(index);
            if self.slots[slot].take().is_some() {
                self.ready -= 1;
            }
        }
        self.first = first;
    }

    /// The earliest step in the window with no pose yet.
    fn next_missing(&self) -> Option<u64> {
        if self.ready == self.slots.len() {
            return None;
        }
        (self.first..self.first + self.slots.len() as u64).find(|&i| self.get(i).is_none())
    }

    fn put(&mut self, index: u64, pose: Arc<LivingPose>) {
        let slot = self.slot(index);
        if self.slots[slot].replace(pose).is_none() {
            self.ready += 1;
        }
    }

    /// How many poses it's missing of what it should hold. The spare doesn't
    /// count: it's what stops this going above zero.
    fn short(&self) -> usize {
        self.len().saturating_sub(self.ready)
    }
}

/// One prop's runtime state, the other half of its `living_prop` component.
#[derive(Debug, Clone)]
pub struct LivingProp {
    entity: EntityId,
    entity_name: String,
    asset_name: String,
    lineage: usize,
    /// Added to a step index to give that step's shape seed.
    stream: u64,
    phase_offset: f32,
    clock: LivingClock,
    pose_ring: u32,
    /// `None` until its lineage is built, since its size depends on the depth.
    ring: Option<PoseRing>,
    jitter: LivingJitter,
    /// Seconds its jitter runs ahead of the props' clock, fixed when it's added:
    /// each value already has its own phase, and has no need to bend with the
    /// step. Only the slow layer does.
    jitter_ahead: f64,
    /// The step when it was added, which puts its echoes' jitter in seconds.
    jitter_step: f64,
    /// See [`Self::beats`].
    beats: f64,
    position: [f32; 3],
    wooze: WoozeSample,
    /// The same as [`DrawnBones::parts`] for the drift-only shadow, whose cycle is
    /// two steps long.
    shadow_cycle: Cycle,
    drawn: DrawnState,
    /// Farthest from now first, which is the order they draw in.
    echoes: Vec<PropEcho>,
}

impl LivingProp {
    /// The entity this prop is.
    pub fn entity(&self) -> EntityId {
        self.entity
    }

    /// The entity's name.
    pub fn entity_name(&self) -> &str {
        &self.entity_name
    }

    /// The mesh it draws: one per spec and lineage.
    pub fn asset_name(&self) -> &str {
        &self.asset_name
    }

    /// The clock its parts drift on, read at [`Self::beats`]: as the spec says,
    /// or as the wooze field last said.
    pub fn clock(&self) -> LivingClock {
        self.clock
    }

    /// The clock its leaf cards and shades boil on.
    pub fn jitter(&self) -> LivingJitter {
        self.jitter
    }

    /// How far into its step cycle it runs, as a fraction of a step.
    pub fn phase_offset(&self) -> f32 {
        self.phase_offset
    }

    /// Where this prop's clock stands, in steps it has run through. It moves on
    /// by `dt / step` each frame, so a new step changes how fast it runs from
    /// here and never where it is.
    pub fn beats(&self) -> f64 {
        self.beats
    }

    /// Where it stands in the world.
    pub fn position(&self) -> [f32; 3] {
        self.position
    }

    /// What the wooze field last said about its place, or what its spec says in
    /// a scene with no field.
    pub fn wooze(&self) -> WoozeSample {
        self.wooze
    }

    /// How many echoes it draws.
    pub fn echo_count(&self) -> usize {
        self.echoes.len()
    }

    /// Echo `index`'s place on the clock and its opacity.
    pub fn echo(&self, index: usize) -> EchoCopy {
        self.echoes[index].copy
    }

    /// Where echo `index`'s clock stands: an offset counted in steps, so it's the
    /// same many steps from the prop whatever the step is.
    pub fn echo_beats(&self, index: usize) -> f64 {
        self.beats + f64::from(self.echoes[index].copy.offset)
    }

    /// The seconds echo `index`'s jitter reads when this prop's reads `t`. By the
    /// step it was added on, since jitter doesn't bend with the step.
    fn echo_jitter_time(&self, index: usize, t: f64) -> f64 {
        t + f64::from(self.echoes[index].copy.offset) * self.jitter_step
    }

    /// How many steps behind now the farthest echo is. A ring's window starts
    /// there, so a pose stays until the last echo has finished with it.
    fn steps_behind(&self) -> f64 {
        let behind = self
            .echoes
            .iter()
            .map(|e| -e.copy.offset)
            .fold(0.0, f32::max);
        f64::from(behind)
    }

    /// How many steps the prop's clocks span, behind now to ahead of it.
    fn echo_span(&self) -> f64 {
        let ahead = self
            .echoes
            .iter()
            .map(|e| e.copy.offset)
            .fold(0.0, f32::max);
        self.steps_behind() + f64::from(ahead)
    }

    /// The first step its ring's window needs now.
    fn window_start(&self) -> u64 {
        (self.beats - self.steps_behind()).max(0.0).floor() as u64
    }

    /// The shape seed of step `index`. Two props of a lineage share a
    /// structure and never a stream.
    pub fn shape_seed(&self, index: u64) -> u64 {
        self.stream.wrapping_add(index)
    }

    /// The pose for step `index`, if it's in the window and ready.
    pub fn pose(&self, index: u64) -> Option<&Arc<LivingPose>> {
        self.ring.as_ref()?.get(index)
    }

    /// The first step in the window: the step the trunk side is leaving.
    pub fn first_step(&self) -> Option<u64> {
        self.ring.as_ref().map(|ring| ring.first)
    }

    /// How many poses its ring holds when full. It can be more than
    /// `pose_ring`: never fewer than the clock's stagger can reach.
    pub fn ring_len(&self) -> usize {
        self.ring.as_ref().map_or(0, PoseRing::len)
    }

    /// How many poses of the ring are ready now. A spare beyond it doesn't count.
    pub fn ready(&self) -> usize {
        self.ring
            .as_ref()
            .map_or(0, |ring| ring.ready.min(ring.len()))
    }
}

/// What a lineage is once it's built.
#[derive(Debug, Clone)]
struct BuiltLineage {
    skeleton: Arc<LivingSkeleton>,
    rig: Arc<LivingRig>,
}

#[derive(Debug, Clone)]
struct Lineage {
    spec: Arc<ProcGenSpec>,
    seed: u64,
    asset_name: String,
    /// Names the spec and the generator's revision, for the pose cache.
    identity: ContentHash,
    /// `None` until it's built, and for good if it can't be.
    built: Option<BuiltLineage>,
    tried: bool,
}

/// Poses that have been generated, apart from the meshes in
/// [`ProcGenCache`](crate::ProcGenCache): they're tiny, and a grove makes a
/// hundred a second, so they'd push the meshes out. First in, first out, since
/// a prop only moves forward through its stream, and a pose in a ring stays
/// alive whatever happens to it here.
#[derive(Debug)]
struct PoseCache {
    poses: HashMap<(ContentHash, u64), Arc<LivingPose>>,
    order: VecDeque<(ContentHash, u64)>,
    bytes: usize,
    max_bytes: usize,
}

impl PoseCache {
    fn new(max_bytes: usize) -> Self {
        Self {
            poses: HashMap::new(),
            order: VecDeque::new(),
            bytes: 0,
            max_bytes,
        }
    }

    fn size_of(pose: &LivingPose) -> usize {
        pose.segments.len() * std::mem::size_of::<SegmentPose>()
    }

    fn get(&self, identity: ContentHash, shape_seed: u64) -> Option<Arc<LivingPose>> {
        self.poses.get(&(identity, shape_seed)).cloned()
    }

    fn insert(&mut self, identity: ContentHash, shape_seed: u64, pose: Arc<LivingPose>) {
        self.bytes += Self::size_of(&pose);
        self.order.push_back((identity, shape_seed));
        self.poses.insert((identity, shape_seed), pose);
        while self.bytes > self.max_bytes {
            let Some(oldest) = self.order.pop_front() else {
                break;
            };
            if let Some(gone) = self.poses.remove(&oldest) {
                self.bytes -= Self::size_of(&gone);
            }
        }
    }
}

/// The scene's living props, their lineages and the poses they've been given.
#[derive(Debug)]
pub struct LivingProps {
    lineages: Vec<Lineage>,
    props: Vec<LivingProp>,
    cache: PoseCache,
    /// The clock every prop reads, in seconds. It stops when the game pauses.
    time: f64,
    /// How long the next pose is likely to take: the slowest one lately.
    pose_estimate_ms: f64,
    stats: LivingStats,
    defaults: LivingDefaults,
}

impl LivingProps {
    /// No props, and a pose cache that keeps `pose_cache_bytes`.
    pub fn new(pose_cache_bytes: usize) -> Self {
        Self {
            lineages: Vec::new(),
            props: Vec::new(),
            cache: PoseCache::new(pose_cache_bytes),
            time: 0.0,
            pose_estimate_ms: 0.0,
            stats: LivingStats::default(),
            defaults: LivingDefaults::default(),
        }
    }

    /// What the scene says about its props, for the props added after this.
    pub fn set_defaults(&mut self, defaults: LivingDefaults) {
        self.defaults = defaults;
    }

    /// Whether there's nothing here to do, as in any scene that doesn't opt in.
    pub fn is_empty(&self) -> bool {
        self.props.is_empty()
    }

    /// The props, in the order they were added.
    pub fn props(&self) -> &[LivingProp] {
        &self.props
    }

    /// One prop.
    pub fn prop(&self, id: LivingPropId) -> &LivingProp {
        &self.props[id.0]
    }

    /// How many distinct trees the props grow from.
    pub fn lineage_count(&self) -> usize {
        self.lineages.len()
    }

    /// The skeleton and rig a prop's poses apply to, once they're built.
    pub fn rig_of(&self, id: LivingPropId) -> Option<(&Arc<LivingSkeleton>, &Arc<LivingRig>)> {
        let built = self.lineages[self.props[id.0].lineage].built.as_ref()?;
        Some((&built.skeleton, &built.rig))
    }

    /// The clock every prop reads, in seconds of unpaused play.
    pub fn time(&self) -> f64 {
        self.time
    }

    /// How the rings have fared so far.
    pub fn stats(&self) -> LivingStats {
        self.stats
    }

    /// Forget the scene's props and lineages, keeping the poses: coming back
    /// to a scene shouldn't have to regenerate them.
    pub fn clear_props(&mut self) {
        self.lineages.clear();
        self.props.clear();
        self.time = 0.0;
        self.stats = LivingStats::default();
    }

    /// Add a prop whose spec has been checked. Returns its id and the name of
    /// its mesh.
    pub(crate) fn add(
        &mut self,
        request: LivingPropRequest,
        spec: &ProcGenSpec,
        identity: &str,
    ) -> (LivingPropId, String) {
        let living = spec
            .living
            .as_ref()
            .expect("the resolver checks for a [living] table");
        let seed = request.lineage_seed.unwrap_or(living.lineage_seed);
        let asset_name = format!("{}@{seed}", request.spec_name);

        let existing = self
            .lineages
            .iter()
            .position(|l| l.asset_name == asset_name);
        let lineage = existing.unwrap_or_else(|| {
            self.lineages.push(Lineage {
                spec: Arc::new(spec.clone()),
                seed,
                asset_name: asset_name.clone(),
                identity: ContentHash::from_str(&format!("{identity}\n{seed}")),
                built: None,
                tried: false,
            });
            self.lineages.len() - 1
        });

        let stream = name_hash(&request.entity_name);
        let phase_offset = request
            .phase_offset
            .map_or_else(|| name_phase(stream), |phase| phase.rem_euclid(1.0));
        let mut clock = LivingClock::from(living);
        if let Some(stagger) = request.stagger {
            clock.stagger = stagger;
        }
        self.props.push(LivingProp {
            entity: request.entity,
            entity_name: request.entity_name,
            asset_name: asset_name.clone(),
            lineage,
            stream,
            phase_offset,
            clock,
            pose_ring: request.pose_ring.unwrap_or(living.pose_ring),
            ring: None,
            jitter: LivingJitter::from(living),
            jitter_ahead: f64::from(phase_offset) * f64::from(clock.step),
            jitter_step: f64::from(clock.step),
            beats: f64::from(phase_offset),
            position: request.position,
            wooze: WoozeSample {
                amplitude: 1.0,
                step: clock.step,
                glide: clock.glide,
                jitter: 1.0,
            },
            shadow_cycle: Cycle::default(),
            drawn: DrawnState::default(),
            echoes: self
                .defaults
                .echoes
                .over(living.echoes)
                .copies()
                .into_iter()
                .map(|copy| PropEcho {
                    copy,
                    drawn: DrawnBones::default(),
                })
                .collect(),
        });
        (LivingPropId(self.props.len() - 1), asset_name)
    }

    /// One prop's blend this frame: fills `frame` with what has changed and
    /// says where it goes. Call after [`tick`](Self::tick) has moved the clock.
    ///
    /// A part reads its own place on the warped clock, and the prop's jitter
    /// bones and leaf tints read theirs. If none of that has moved, as when
    /// the game is paused or every part is holding, this computes no bones and
    /// asks for no upload.
    ///
    /// A pose the clock needs that isn't ready leaves the prop as it was, and
    /// [`LivingStats::blends_starved`] counts it. It can only happen if the
    /// budget has run behind or poses are still coming in under `--procgen-defer`.
    ///
    /// Each part reads the field's glide and amplitude when its own cycle begins,
    /// through its [`Cycle`], so a change reaches a staggered tree a part at a
    /// time and nothing has to be redrawn until it does: a prop holding through a
    /// change costs no more than one holding through nothing.
    pub fn blend_prop(&mut self, id: LivingPropId, frame: &mut BlendFrame) -> PropBlend {
        let prop = &mut self.props[id.0];
        let mut blend = PropBlend {
            entity: prop.entity,
            first_bone: None,
            shadow_changed: false,
        };
        frame.tints.clear();
        let (Some(ring), Some(built)) = (&prop.ring, &self.lineages[prop.lineage].built) else {
            return blend;
        };
        let (skeleton, rig) = (&built.skeleton, &built.rig);
        let t = self.time + prop.jitter_ahead;
        let jitter = prop.jitter.scaled(prop.wooze.jitter);

        let moved = blend_bones(
            &prop.clock,
            &jitter,
            prop.stream,
            ring,
            skeleton,
            rig,
            t,
            prop.beats,
            &prop.wooze,
            &mut prop.drawn.bones,
            frame,
        );
        blend.first_bone = moved.first_bone;
        self.stats.blends_starved += u64::from(moved.starved);

        // Each part's own beat, and what's new goes up as a material change.
        prop.drawn.tints.resize(rig.leaf_parts.len(), 0);
        for (i, part) in rig.leaf_parts.iter().enumerate() {
            let step = jitter.tint_step(prop.stream, i, t);
            if step != prop.drawn.tints[i] {
                prop.drawn.tints[i] = step;
                frame.tints.push((*part, prop.jitter.tint(step)));
            }
        }

        if self.defaults.shadows == LivingShadows::Drift {
            let slow = prop.shadow_cycle.read(prop.beats / 2.0, &prop.wooze);
            let (from, to) = LivingClock::slow_pair(slow.index);
            let weight = slow.weight;
            let shown = prop.shadow_cycle.shown(weight);
            if prop.drawn.shadow != Some((from, weight)) {
                let pose = frame.pose.get_or_insert_with(|| LivingPose {
                    shape_seed: 0,
                    segments: Vec::new(),
                });
                frame.times.clear();
                frame
                    .times
                    .resize(skeleton.segments.len(), WarpedTime { index: 0, weight });
                let found = pose.blend_from(&frame.times, |index| {
                    let wanted = if index == 0 { from } else { to };
                    ring.get(wanted).map(|p| &**p)
                });
                if found {
                    for segment in &mut pose.segments {
                        *segment = segment.pulled(shown);
                    }
                }
                if found
                    && rig
                        .palette_into(skeleton, pose, &mut frame.posed, &mut frame.shadow_bones)
                        .is_ok()
                {
                    frame
                        .shadow_bones
                        .extend_from_slice(&[[[0.0; 4]; 4]; BOIL_BONES]);
                    prop.drawn.shadow = Some((from, weight));
                    blend.shadow_changed = true;
                }
            }
        }
        blend
    }

    /// One echo's blend this frame: fills `frame.bones` if its bones have moved
    /// and returns where they go. It's the prop's own blend, at its own place
    /// on the clock, so a leaf card comes along with its bones and turns on the
    /// echo's beat. Its tints are the prop's.
    ///
    /// An echo has no rest pose to fall back on: its bones are written whole the
    /// first time, or not at all.
    pub fn blend_echo(
        &mut self,
        id: LivingPropId,
        echo: usize,
        frame: &mut BlendFrame,
    ) -> Option<usize> {
        let prop = &mut self.props[id.0];
        let (Some(ring), Some(built)) = (&prop.ring, &self.lineages[prop.lineage].built) else {
            return None;
        };
        let t = prop.echo_jitter_time(echo, self.time + prop.jitter_ahead);
        let beats = prop.echo_beats(echo);
        let jitter = prop.jitter.scaled(prop.wooze.jitter);
        let drawn = &mut prop.echoes[echo].drawn;
        let never_posed = drawn.times.is_empty();
        let moved = blend_bones(
            &prop.clock,
            &jitter,
            prop.stream,
            ring,
            &built.skeleton,
            &built.rig,
            t,
            beats,
            &prop.wooze,
            drawn,
            frame,
        );
        self.stats.blends_starved += u64::from(moved.starved);
        moved.first_bone.filter(|first| *first == 0 || !never_posed)
    }

    /// Note one frame's blending: how long it took and how many buffer writes.
    pub fn record_blend(&mut self, spent_ms: f64, writes: u64) {
        let stats = &mut self.stats;
        stats.blend_frames += 1;
        stats.blend_total_ms += spent_ms;
        stats.blend_worst_ms = stats.blend_worst_ms.max(spent_ms);
        stats.blend_frames_over_1ms += u64::from(spent_ms > 1.0);
        stats.bone_writes += writes;
    }

    /// Build every lineage and fill every ring, with no budget: scene load,
    /// so the first frame finds them full.
    pub(crate) fn make_all_now(&mut self, registry: &GeneratorRegistry) -> Vec<LineageDone> {
        let mut done = Vec::new();
        while let Some(built) = self.build_next_lineage(registry) {
            done.push(built);
        }
        while self.fill_one(registry).is_some() {}
        self.pose_estimate_ms = 0.0;
        // What load made wasn't made in play: the report is about the frames.
        self.stats = LivingStats::default();
        done
    }

    /// Tell the props where they stand now, for a scene with a wooze field, in
    /// which a prop that moves reads the field where it's got to.
    pub fn track_positions(&mut self, position_of: impl Fn(EntityId) -> Option<[f32; 3]>) {
        for prop in &mut self.props {
            if let Some(position) = position_of(prop.entity) {
                prop.position = position;
            }
        }
    }

    /// One frame: read the wooze field if the scene has one, move the clocks on
    /// by `dt`, slide the rings to them, and generate what's missing within
    /// `budget_ms`. Lineages come first, since nothing can pose before them; one
    /// can't be cut short, so a frame builds at most one and then stops.
    pub(crate) fn tick(
        &mut self,
        dt: f64,
        budget_ms: f64,
        registry: &GeneratorRegistry,
        wooze: Option<(&WoozeField, f64)>,
    ) -> Vec<LineageDone> {
        self.time += dt;
        let start = Instant::now();
        for prop in &mut self.props {
            if let Some((field, time)) = wooze {
                prop.wooze = field.sample(prop.position, time);
                prop.clock.step = prop.wooze.step;
                prop.clock.glide = prop.wooze.glide;
            }
            prop.beats += dt / f64::from(prop.clock.step);
            let first = prop.window_start();
            if let Some(ring) = &mut prop.ring {
                ring.slide_to(first);
            }
        }

        let done = if budget_ms > 0.0 {
            self.build_next_lineage(registry)
        } else {
            None
        };
        let worked = done.is_some() || self.fill_within(budget_ms, start, registry);

        let spent = start.elapsed().as_secs_f64() * 1000.0;
        self.record_frame(spent, budget_ms, worked);
        done.into_iter().collect()
    }

    /// Generate poses, emptiest ring first, until the next would pass the
    /// budget. A budget too small for even one pose (a debug build) still gets
    /// one a frame, or the rings would never fill at all. Returns whether it
    /// generated anything.
    fn fill_within(
        &mut self,
        budget_ms: f64,
        start: Instant,
        registry: &GeneratorRegistry,
    ) -> bool {
        let mut made = false;
        loop {
            let used = start.elapsed().as_secs_f64() * 1000.0;
            let estimate = self.pose_estimate_ms;
            let a_later_frame_fits_it = estimate > 0.0 && estimate <= budget_ms;
            if used + estimate > budget_ms && (made || budget_ms <= 0.0 || a_later_frame_fits_it) {
                return made;
            }
            let began = Instant::now();
            let Some(generated) = self.fill_one(registry) else {
                return made;
            };
            if generated {
                made = true;
                let took = began.elapsed().as_secs_f64() * 1000.0;
                self.pose_estimate_ms = (self.pose_estimate_ms * POSE_ESTIMATE_DECAY).max(took);
            }
        }
    }

    /// Give the emptiest ring its next pose. Returns whether that took
    /// generating (false for a cache hit), or `None` if nothing is missing.
    fn fill_one(&mut self, registry: &GeneratorRegistry) -> Option<bool> {
        let (index, step) = self
            .props
            .iter()
            .enumerate()
            .filter_map(|(i, prop)| {
                let ring = prop.ring.as_ref()?;
                Some((i, ring.next_missing()?, ring.ready))
            })
            .min_by_key(|&(_, _, ready)| ready)
            .map(|(i, step, _)| (i, step))?;

        let prop = &self.props[index];
        let lineage = &self.lineages[prop.lineage];
        let shape_seed = prop.shape_seed(step);
        let identity = lineage.identity;

        let (pose, generated) = if let Some(pose) = self.cache.get(identity, shape_seed) {
            self.stats.poses_from_cache += 1;
            (pose, false)
        } else {
            let made = lineage
                .built
                .as_ref()
                .zip(living_side(registry, &lineage.spec))
                .and_then(|(built, generator)| {
                    generator
                        .pose(&lineage.spec, &built.skeleton, shape_seed)
                        .ok()
                });
            // Its checks passed when it was added and its lineage built, so
            // this shouldn't happen; the rest pose keeps the ring from
            // retrying forever if it does, and the report counts it.
            if made.is_none() {
                self.stats.pose_failures += 1;
            }
            let pose = made.unwrap_or_else(|| {
                let segments = lineage
                    .built
                    .as_ref()
                    .map_or(0, |built| built.skeleton.segments.len());
                LivingPose {
                    shape_seed,
                    segments: vec![SegmentPose::IDENTITY; segments],
                }
            });
            let pose = Arc::new(pose);
            self.cache.insert(identity, shape_seed, pose.clone());
            self.stats.poses_made += 1;
            (pose, true)
        };

        self.props[index]
            .ring
            .as_mut()
            .expect("only a prop with a ring has a missing pose")
            .put(step, pose);
        Some(generated)
    }

    /// Build the first lineage not yet tried and give its props their rings.
    fn build_next_lineage(&mut self, registry: &GeneratorRegistry) -> Option<LineageDone> {
        let index = self.lineages.iter().position(|l| !l.tried)?;
        let lineage = &mut self.lineages[index];
        lineage.tried = true;
        let built = build_lineage(registry, &lineage.spec, lineage.seed);
        let asset_name = lineage.asset_name.clone();

        let mut rest_palette = Vec::new();
        let rig = match built {
            Ok(built) => {
                let rest = LivingPose {
                    shape_seed: 0,
                    segments: vec![SegmentPose::IDENTITY; built.skeleton.segments.len()],
                };
                // A rest pose has one transform per segment by construction.
                let _ = built.rig.palette(&built.skeleton, &rest, &mut rest_palette);
                // Still jitter bones, or the mesh's shared buffer would have
                // them at its identity, which is a long way from nothing.
                boil_bones(&[0.0; JITTER_CHANNELS], &mut rest_palette);
                let rig = built.rig.clone();
                let max_depth = built.skeleton.max_depth();
                lineage.built = Some(built);
                for prop in self.props.iter_mut().filter(|p| p.lineage == index) {
                    let first = prop.window_start();
                    // Every pose a staggered part of it or its echoes can be
                    // using has to fit.
                    let reach = prop.clock.reach_across(max_depth, prop.echo_span());
                    let mut len = u64::from(prop.pose_ring).max(reach + 1);
                    // The slow pair is up to three steps ahead of now.
                    if self.defaults.shadows == LivingShadows::Drift {
                        len = len.max(4 + prop.steps_behind().ceil() as u64);
                    }
                    prop.ring = Some(PoseRing::new(first, len as usize));
                }
                Ok(rig)
            }
            Err(reason) => Err(reason),
        };
        let entities = self
            .props
            .iter()
            .filter(|p| p.lineage == index)
            .map(|p| p.entity)
            .collect();
        Some(LineageDone {
            asset_name,
            rig,
            rest_palette,
            entities,
        })
    }

    fn record_frame(&mut self, spent_ms: f64, budget_ms: f64, worked: bool) {
        let mut all_full = true;
        let mut starved = false;
        for prop in &self.props {
            let Some(ring) = &prop.ring else {
                continue;
            };
            let short = ring.short();
            all_full &= short == 0;
            self.stats.most_short = self.stats.most_short.max(short);
            starved |= ring.get(ring.first).is_none() || ring.get(ring.first + 1).is_none();
        }
        let stats = &mut self.stats;
        stats.frames += 1;
        stats.frames_full += u64::from(all_full);
        stats.frames_starved += u64::from(starved);
        stats.total_ms += spent_ms;
        stats.worst_ms = stats.worst_ms.max(spent_ms);
        // Sliding the rings is always cheap, so a frame that generated nothing
        // can't have overrun, even one the asset queue had already spent.
        stats.frames_over_budget += u64::from(worked && spent_ms > budget_ms.max(0.0));
    }
}

/// What [`blend_bones`] did.
struct MovedBones {
    /// Where [`BlendFrame::bones`] goes; `None` if the bones haven't changed.
    first_bone: Option<usize>,
    /// Whether a pose it needed wasn't ready, which left the bones as they were.
    starved: bool,
}

/// One bone buffer's blend, a prop's or an echo's: every part reads its own place
/// on the warped clock at `beats`, and the jitter reads `t` in seconds. If none of
/// that has moved, this computes nothing.
#[allow(clippy::too_many_arguments)]
fn blend_bones(
    clock: &LivingClock,
    jitter: &LivingJitter,
    stream: u64,
    ring: &PoseRing,
    skeleton: &LivingSkeleton,
    rig: &LivingRig,
    t: f64,
    beats: f64,
    wooze: &WoozeSample,
    drawn: &mut DrawnBones,
    frame: &mut BlendFrame,
) -> MovedBones {
    let mut moved = MovedBones {
        first_bone: None,
        starved: false,
    };
    let mut turns = [0.0; JITTER_CHANNELS];
    for (channel, turn) in turns.iter_mut().enumerate() {
        *turn = jitter.card_turn(stream, channel, t);
    }
    drawn
        .parts
        .resize(skeleton.segments.len(), Cycle::default());
    frame.times.clear();
    for (segment, part) in skeleton.segments.iter().zip(&mut drawn.parts) {
        let now = part.read(beats + clock.phase(segment), wooze);
        frame.times.push(WarpedTime {
            index: now.index,
            weight: now.weight,
        });
    }
    let turned = turns != drawn.turns;
    let drifted = frame.times != drawn.times;

    frame.bones.clear();
    if drifted {
        let pose = frame.pose.get_or_insert_with(|| LivingPose {
            shape_seed: 0,
            segments: Vec::new(),
        });
        let found = pose.blend_from(&frame.times, |index| ring.get(index).map(|p| &**p));
        if found {
            for ((segment, part), now) in
                pose.segments.iter_mut().zip(&drawn.parts).zip(&frame.times)
            {
                let shown = part.shown(now.weight);
                if shown < 1.0 {
                    *segment = segment.pulled(shown);
                }
            }
        }
        let posed = found
            && rig
                .palette_into(skeleton, pose, &mut frame.posed, &mut frame.bones)
                .is_ok();
        if posed {
            std::mem::swap(&mut drawn.times, &mut frame.times);
            boil_bones(&turns, &mut frame.bones);
            drawn.turns = turns;
            moved.first_bone = Some(0);
        } else {
            frame.bones.clear();
            moved.starved = true;
        }
    }
    if moved.first_bone.is_none() && turned {
        boil_bones(&turns, &mut frame.bones);
        drawn.turns = turns;
        moved.first_bone = Some(rig.bone_segments.len());
    }
    moved
}

fn living_side<'a>(
    registry: &'a GeneratorRegistry,
    spec: &ProcGenSpec,
) -> Option<&'a dyn LivingGenerator> {
    registry.get(&spec.generator)?.as_living()
}

fn build_lineage(
    registry: &GeneratorRegistry,
    spec: &ProcGenSpec,
    seed: u64,
) -> std::result::Result<BuiltLineage, String> {
    let generator = living_side(registry, spec)
        .ok_or_else(|| format!("its generator '{}' can't live", spec.generator))?;
    let skeleton = generator.structure(spec, seed).map_err(|e| e.to_string())?;
    let rig = generator.rig(spec, seed).map_err(|e| e.to_string())?;
    Ok(BuiltLineage {
        skeleton: Arc::new(skeleton),
        rig: Arc::new(rig),
    })
}

/// A name as 64 well-mixed bits, stable across runs and platforms, unlike the
/// standard library's hasher. FNV-1a alone won't do: names that differ in their
/// last characters, as `oak_01` and `oak_02` do, come out the same in its high
/// bits, which put a whole grove on one phase. The finalizer (MurmurHash3's)
/// spreads every input bit across all of them.
fn name_hash(name: &str) -> u64 {
    mix64(name.bytes().fold(0xcbf2_9ce4_8422_2325u64, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3)
    }))
}

/// A phase offset in [0, 1) from a name's hash. The same name always gets the
/// same offset.
fn name_phase(hash: u64) -> f32 {
    ((hash >> 40) as f64 / (1u64 << 24) as f64) as f32
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cache::ProcGenCacheConfig;
    use crate::ProcGenResolver;
    use std::path::Path;

    const OAK: &str = "../../specs/examples/oak_living.procgen.toml";

    /// A small tree, so tests can make many poses even in a debug build.
    fn sapling(extra: &str) -> String {
        format!(
            r#"
generator = "tree_v1"
[meta]
name = "sapling"
version = "1.0.0"
[seed]
mode = "fixed"
value = 1
[params]
branch_method = "lsystem"
lsystem_iterations = 2
leaf_density = 0.0
[living]
lineage_seed = 7
pose_ring = 4
variance = {{ amount = 0.5, steady = 0.5, keep_size = false }}
{extra}
{echoes}
"#,
            // These tests are about the prop itself, except those that ask.
            echoes = if extra.contains("echoes") {
                ""
            } else {
                "echoes = { count = 0 }"
            },
        )
    }

    fn resolver_with(spec: &str) -> ProcGenResolver {
        let mut resolver = ProcGenResolver::new();
        resolver.index_spec(ProcGenSpec::parse(spec).unwrap());
        resolver
    }

    fn request(n: u64, name: &str) -> LivingPropRequest {
        LivingPropRequest {
            entity: EntityId::from_raw(n),
            entity_name: name.to_string(),
            position: [0.0; 3],
            spec_name: "sapling".to_string(),
            lineage_seed: None,
            pose_ring: None,
            phase_offset: None,
            stagger: None,
        }
    }

    fn pose_seeds(props: &LivingProps, id: LivingPropId) -> Vec<u64> {
        let prop = props.prop(id);
        let first = prop.first_step().unwrap();
        (first..first + prop.ring_len() as u64)
            .map(|i| prop.pose(i).unwrap().shape_seed)
            .collect()
    }

    /// 40 props ticked over `seconds` with a budget wide enough to never bind.
    fn run(resolver: &mut ProcGenResolver, seconds: f64, dt: f64) {
        for _ in 0..(seconds / dt).round() as usize {
            resolver.process_living(dt);
        }
    }

    #[test]
    fn a_ring_slides_without_losing_what_is_ahead() {
        let mut ring = PoseRing::new(10, 3);
        let pose = |seed| {
            Arc::new(LivingPose {
                shape_seed: seed,
                segments: Vec::new(),
            })
        };
        for i in 10..14 {
            assert_eq!(ring.next_missing(), Some(i));
            ring.put(i, pose(i));
        }
        assert_eq!(ring.next_missing(), None);
        assert_eq!(ring.short(), 0);

        // A beat spends the spare, and the ring is still full.
        ring.slide_to(11);
        assert_eq!(ring.short(), 0);
        assert_eq!(ring.next_missing(), Some(14));

        ring.slide_to(12);
        assert_eq!(ring.short(), 1);
        assert_eq!(ring.ready, 2);
        assert!(ring.get(11).is_none(), "behind the window");
        assert_eq!(ring.get(13).unwrap().shape_seed, 13);
        assert!(ring.get(14).is_none());
        assert_eq!(ring.next_missing(), Some(14));

        // A slot is reused without its old pose answering for the new step.
        ring.put(14, pose(14));
        assert_eq!(ring.get(14).unwrap().shape_seed, 14);
        assert_eq!(ring.next_missing(), Some(15));

        // Sliding further than the window empties it.
        ring.slide_to(100);
        assert_eq!(ring.ready, 0);
        assert_eq!(ring.next_missing(), Some(100));
    }

    #[test]
    fn sliding_backwards_changes_nothing() {
        let mut ring = PoseRing::new(10, 2);
        ring.slide_to(5);
        assert_eq!(ring.first, 10);
    }

    #[test]
    fn a_ring_of_one_spec_does_not_collapse_to_one_request() {
        let mut resolver = resolver_with(&sapling(""));
        let id = resolver.add_living_prop(request(1, "oak")).unwrap().0;
        resolver.make_living_now();
        let props = resolver.living_props();
        let seeds = pose_seeds(props, id);
        assert_eq!(seeds.len(), 4);
        let mut unique = seeds.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), 4, "{seeds:?}");
        assert_ne!(
            props.prop(id).pose(0).unwrap().segments,
            props.prop(id).pose(1).unwrap().segments
        );
        // None of it reached the mesh cache.
        assert_eq!(resolver.cache_stats().entry_count, 0);
    }

    #[test]
    fn props_share_a_lineage_and_not_a_stream() {
        let mut resolver = resolver_with(&sapling(""));
        let (a, asset_a) = resolver.add_living_prop(request(1, "a")).unwrap();
        let (b, asset_b) = resolver.add_living_prop(request(2, "b")).unwrap();
        let mut other = request(3, "c");
        other.lineage_seed = Some(8);
        let (_, asset_c) = resolver.add_living_prop(other).unwrap();

        assert_eq!(asset_a, "sapling@7");
        assert_eq!(asset_b, "sapling@7");
        assert_eq!(asset_c, "sapling@8");

        let done = resolver.make_living_now();
        assert_eq!(done.len(), 2, "one build a lineage");
        assert_eq!(done[0].entities.len(), 2);
        assert_eq!(done[1].entities.len(), 1);
        let props = resolver.living_props();
        assert_eq!(props.lineage_count(), 2);
        assert_ne!(pose_seeds(props, a), pose_seeds(props, b));
    }

    #[test]
    fn a_rest_palette_is_the_identity() {
        let mut resolver = resolver_with(&sapling(""));
        resolver.add_living_prop(request(1, "a")).unwrap();
        let done = resolver.make_living_now();
        let palette = &done[0].rest_palette;
        let bones = done[0].rig.as_ref().unwrap().bone_segments.len();
        assert_eq!(palette.len(), bones + BOIL_BONES);
        for bone in &palette[bones..] {
            assert_eq!(*bone, [[0.0; 4]; 4], "still jitter bones");
        }
        for bone in &palette[..bones] {
            for (c, column) in bone.iter().enumerate() {
                for (r, value) in column.iter().enumerate() {
                    let want = if c == r { 1.0 } else { 0.0 };
                    assert!((value - want).abs() < 1e-3, "{bone:?}");
                }
            }
        }
    }

    #[test]
    fn the_component_overrides_the_spec() {
        let mut resolver = resolver_with(&sapling("stagger = \"per_level\""));
        let mut custom = request(1, "a");
        custom.pose_ring = Some(6);
        custom.phase_offset = Some(1.25);
        custom.stagger = Some(Stagger::None);
        let id = resolver.add_living_prop(custom).unwrap().0;
        resolver.make_living_now();
        let prop = resolver.living_props().prop(id);
        assert_eq!(prop.ring_len(), 6);
        assert_eq!(prop.phase_offset(), 0.25, "wraps into one step");
        assert_eq!(prop.clock().stagger, Stagger::None);
    }

    #[test]
    fn a_name_sets_the_phase_the_same_way_every_time() {
        let mut resolver = resolver_with(&sapling(""));
        let a = resolver.add_living_prop(request(1, "tree_a")).unwrap().0;
        let b = resolver.add_living_prop(request(2, "tree_b")).unwrap().0;
        // Whatever the id: entity ids differ run to run.
        let again = resolver.add_living_prop(request(99, "tree_a")).unwrap().0;
        let props = resolver.living_props();
        assert_eq!(
            props.prop(a).phase_offset(),
            props.prop(again).phase_offset()
        );
        assert_ne!(props.prop(a).phase_offset(), props.prop(b).phase_offset());
        assert_eq!(props.prop(a).shape_seed(3), props.prop(again).shape_seed(3));
        let phase = props.prop(a).phase_offset();
        assert!((0.0..1.0).contains(&phase), "{phase}");
    }

    /// Names that differ only in their last characters, as a scene's do, have
    /// to land all over the cycle: in step, a grove would be one big pop, and
    /// ask for all its poses on one frame.
    #[test]
    fn a_grove_of_numbered_names_spreads_across_the_cycle() {
        let phases: Vec<f32> = (0..40)
            .map(|i| name_phase(name_hash(&format!("oak_{i:02}"))))
            .collect();
        let mut buckets = [0; 10];
        for phase in &phases {
            buckets[(*phase * 10.0) as usize] += 1;
        }
        assert!(
            buckets.iter().all(|b| (1..=10).contains(b)),
            "{buckets:?} from {phases:?}"
        );
    }

    #[test]
    fn neighbouring_names_do_not_share_poses() {
        // A stream is its start plus the step, so two that started close would
        // be the same poses a few steps apart.
        let mut starts: Vec<u64> = (0..200)
            .map(|i| name_hash(&format!("oak_{i:02}")))
            .collect();
        starts.sort_unstable();
        for pair in starts.windows(2) {
            assert!(pair[1] - pair[0] > 1 << 32, "{pair:?}");
        }
    }

    #[test]
    fn a_small_ring_still_reaches_every_staggered_part() {
        // Two poses is valid in a spec, and per-branch stagger needs three.
        let mut resolver = resolver_with(&sapling("stagger = \"per_branch\""));
        let mut small = request(1, "a");
        small.pose_ring = Some(2);
        let id = resolver.add_living_prop(small).unwrap().0;
        resolver.make_living_now();

        let props = resolver.living_props();
        let (skeleton, _) = props.rig_of(id).unwrap();
        let prop = props.prop(id);
        assert_eq!(prop.ring_len(), 3);
        for i in 0..200 {
            let t = f64::from(i) * 0.013;
            let first = prop.clock().at(t).index;
            for segment in &skeleton.segments {
                let now = prop.clock().for_segment(t, segment).index;
                assert!(now + 1 < first + prop.ring_len() as u64, "at {t}");
            }
        }
    }

    #[test]
    fn rings_stay_full_as_the_clock_runs() {
        let mut resolver = resolver_with(&sapling(""));
        let ids: Vec<_> = (0..8)
            .map(|n| {
                let name = format!("tree_{n}");
                resolver.add_living_prop(request(n, &name)).unwrap().0
            })
            .collect();
        resolver.make_living_now();
        run(&mut resolver, 5.0, 1.0 / 60.0);

        let props = resolver.living_props();
        for id in &ids {
            let prop = props.prop(*id);
            assert_eq!(prop.ready(), prop.ring_len());
            assert_eq!(prop.first_step(), Some(prop.beats().floor() as u64));
        }
        let stats = props.stats();
        assert_eq!(stats.frames, 300);
        assert_eq!(stats.frames_starved, 0);
        // 5 s at 2 steps a second: about 10 new poses a prop, with no extras.
        let made = stats.poses_made;
        assert!((70..=90).contains(&made), "{made}");
        assert_eq!(stats.most_short, 0, "{stats:?}");
        assert_eq!(stats.frames_full, 300, "{stats:?}");
    }

    #[test]
    fn a_paused_clock_makes_nothing() {
        let mut resolver = resolver_with(&sapling(""));
        resolver.add_living_prop(request(1, "a")).unwrap();
        resolver.make_living_now();
        for _ in 0..600 {
            resolver.process_living(0.0);
        }
        assert_eq!(resolver.living_props().stats().poses_made, 0);
        assert_eq!(resolver.living_props().time(), 0.0);
    }

    #[test]
    fn poses_do_not_depend_on_the_frame_rate_or_the_budget() {
        let seeds_after = |dts: &[f64], budget: f64| {
            let mut resolver = ProcGenResolver::with_config(ProcGenCacheConfig {
                runtime_max_generation_ms_per_frame: budget,
                ..Default::default()
            });
            resolver.index_spec(ProcGenSpec::parse(&sapling("")).unwrap());
            let id = resolver.add_living_prop(request(5, "the_tree")).unwrap().0;
            resolver.make_living_now();
            for dt in dts {
                resolver.process_living(*dt);
            }
            // Give it whatever it still owes, with the clock stopped.
            for _ in 0..200 {
                resolver.process_living(0.0);
            }
            let props = resolver.living_props();
            let prop = props.prop(id);
            let first = prop.first_step().unwrap();
            (
                first,
                pose_seeds(props, id),
                prop.pose(first).unwrap().clone(),
            )
        };
        let smooth = seeds_after(&[1.0 / 60.0; 240], 4.0);
        let lumpy = seeds_after(&[0.25, 1.0, 0.5, 0.25, 2.0 - 1e-9], 0.001);
        assert_eq!(smooth, lumpy);
    }

    #[test]
    fn a_tiny_budget_still_makes_one_pose_a_frame() {
        let mut resolver = ProcGenResolver::with_config(ProcGenCacheConfig {
            runtime_max_generation_ms_per_frame: 1e-6,
            ..Default::default()
        });
        resolver.index_spec(ProcGenSpec::parse(&sapling("")).unwrap());
        resolver.add_living_prop(request(1, "a")).unwrap();

        // No make_living_now: the first frame builds, then it's a pose a frame.
        assert_eq!(resolver.process_living(0.0).len(), 1);
        let filled = |r: &ProcGenResolver| r.living_props().props()[0].ready();
        assert_eq!(filled(&resolver), 0);
        resolver.process_living(0.0);
        assert_eq!(filled(&resolver), 1);
        resolver.process_living(0.0);
        assert_eq!(filled(&resolver), 2);
    }

    #[test]
    fn a_spec_that_cannot_live_says_why() {
        let mut resolver = resolver_with(
            r#"
generator = "tree_v1"
[meta]
name = "still"
version = "1.0.0"
[seed]
mode = "fixed"
value = 1
"#,
        );
        let mut no_table = request(1, "a");
        no_table.spec_name = "still".into();
        let reason = resolver.add_living_prop(no_table).unwrap_err();
        assert!(reason.contains("[living]"), "{reason}");

        let mut unknown = request(2, "b");
        unknown.spec_name = "nothing".into();
        assert!(resolver.add_living_prop(unknown).is_err());

        let mut resolver =
            resolver_with(&sapling("").replace("\"lsystem\"", "\"space_colonization\""));
        let reason = resolver.add_living_prop(request(3, "c")).unwrap_err();
        assert!(reason.contains("space"), "{reason}");
        assert!(resolver.living_props().is_empty());
    }

    #[test]
    fn coming_back_to_a_scene_reads_its_poses_from_the_cache() {
        let mut resolver = resolver_with(&sapling(""));
        let id = resolver.add_living_prop(request(1, "a")).unwrap().0;
        resolver.make_living_now();
        let before = pose_seeds(resolver.living_props(), id);

        resolver.clear_living_props();
        assert!(resolver.living_props().is_empty());

        let id = resolver.add_living_prop(request(2, "a")).unwrap().0;
        for _ in 0..8 {
            resolver.process_living(0.0);
        }
        let props = resolver.living_props();
        assert_eq!(pose_seeds(props, id), before);
        let stats = props.stats();
        // Four in the ring and the spare.
        assert_eq!((stats.poses_made, stats.poses_from_cache), (0, 5));
    }

    #[test]
    fn the_pose_cache_keeps_to_its_size_and_rings_keep_their_poses() {
        let pose = |seed| {
            Arc::new(LivingPose {
                shape_seed: seed,
                segments: vec![SegmentPose::IDENTITY; 10],
            })
        };
        let one = PoseCache::size_of(&pose(0));
        let mut cache = PoseCache::new(one * 3);
        let identity = ContentHash::from_str("x");
        let held = pose(0);
        cache.insert(identity, 0, held.clone());
        for seed in 1..5 {
            cache.insert(identity, seed, pose(seed));
        }
        assert!(cache.bytes <= one * 3);
        assert_eq!(cache.poses.len(), 3);
        assert!(cache.get(identity, 0).is_none(), "the oldest went first");
        assert!(cache.get(identity, 4).is_some());
        assert_eq!(held.shape_seed, 0, "what a ring holds outlives the cache");
    }

    /// The figures #211 was sized on, with the real oak: run it with
    /// `cargo test --release -p flint-procgen a_grove -- --ignored --nocapture`.
    /// A debug build is too slow to say anything about a budget.
    #[test]
    #[ignore = "wants a release build to mean anything"]
    fn a_grove_of_forty_keeps_full_within_the_budget() {
        let mut resolver = ProcGenResolver::new();
        resolver.index_spec(ProcGenSpec::from_file(Path::new(OAK)).unwrap());
        for n in 0..40u64 {
            let mut prop = request(n, &format!("oak_{n:02}"));
            prop.spec_name = "oak_living".into();
            prop.lineage_seed = Some(n % 5);
            resolver.add_living_prop(prop).unwrap();
        }
        let started = Instant::now();
        resolver.make_living_now();
        println!("load: {:.0} ms", started.elapsed().as_secs_f64() * 1e3);

        run(&mut resolver, 60.0, 1.0 / 60.0);
        let stats = resolver.living_props().stats();
        println!("{stats:#?}");
        assert_eq!(stats.frames, 3600);
        assert_eq!(stats.frames_starved, 0);
        assert_eq!(stats.most_short, 0, "{stats:?}");
        // A pose can't be cut short, so the odd slow one runs over.
        assert!(stats.frames_over_budget <= 36, "{stats:?}");
        assert!(stats.worst_ms < 8.0, "{stats:?}");
    }

    /// A full ring, one sapling prop on `extra`'s settings, and a place to blend.
    fn one_prop(extra: &str, defaults: LivingDefaults) -> (ProcGenResolver, LivingPropId) {
        let mut resolver = resolver_with(&sapling(extra));
        resolver.set_living_defaults(defaults);
        let id = resolver.add_living_prop(request(1, "oak")).unwrap().0;
        resolver.make_living_now();
        (resolver, id)
    }

    const NO_JITTER: &str = "jitter = { amplitude = 0.0 }";

    /// Everything the bones say about a tree: each segment's far end.
    fn tips(
        resolver: &ProcGenResolver,
        id: LivingPropId,
        bones: &[[[f32; 4]; 4]],
    ) -> Vec<[f32; 3]> {
        let (skeleton, rig) = resolver.living_props().rig_of(id).unwrap();
        skeleton
            .segments
            .iter()
            .map(|s| {
                let m = &bones[rig.segment_bones[s.id as usize] as usize];
                let e = s.end.to_array();
                std::array::from_fn(|r| m[0][r] * e[0] + m[1][r] * e[1] + m[2][r] * e[2] + m[3][r])
            })
            .collect()
    }

    fn farthest(a: &[[f32; 3]], b: &[[f32; 3]]) -> f32 {
        a.iter()
            .zip(b)
            .map(|(a, b)| {
                let d: f32 = (0..3).map(|i| (a[i] - b[i]).powi(2)).sum();
                d.sqrt()
            })
            .fold(0.0, f32::max)
    }

    /// `seconds` of play at 60 fps: each frame's tips and whether its bones changed.
    fn play(
        resolver: &mut ProcGenResolver,
        id: LivingPropId,
        seconds: f64,
    ) -> Vec<(Vec<[f32; 3]>, bool)> {
        let mut frame = BlendFrame::default();
        let first = resolver.living_props_mut().blend_prop(id, &mut frame);
        assert_eq!(first.first_bone, Some(0), "the first blend writes it all");
        let mut bones = frame.bones.clone();
        let mut out = vec![(tips(resolver, id, &bones), true)];
        for _ in 0..(seconds * 60.0) as usize {
            resolver.process_living(1.0 / 60.0);
            let blend = resolver.living_props_mut().blend_prop(id, &mut frame);
            let moved = blend.first_bone == Some(0);
            if moved {
                bones.clone_from(&frame.bones);
            }
            out.push((tips(resolver, id, &bones), moved));
        }
        out
    }

    /// The most the tree moves between two poses, measured with the bones.
    fn pose_gap(resolver: &ProcGenResolver, id: LivingPropId) -> f32 {
        let props = resolver.living_props();
        let prop = props.prop(id);
        let (skeleton, rig) = props.rig_of(id).unwrap();
        let first = prop.first_step().unwrap();
        (first..first + 3)
            .map(|step| {
                let mut a = Vec::new();
                let mut b = Vec::new();
                rig.palette(skeleton, prop.pose(step).unwrap(), &mut a)
                    .unwrap();
                rig.palette(skeleton, prop.pose(step + 1).unwrap(), &mut b)
                    .unwrap();
                farthest(&tips(resolver, id, &a), &tips(resolver, id, &b))
            })
            .fold(0.0, f32::max)
    }

    #[test]
    fn at_glide_point_three_nothing_pops_even_across_a_beat() {
        let extra = format!("glide = 0.3\nstep = 0.5\nstagger = \"per_level\"\n{NO_JITTER}");
        let (mut resolver, id) = one_prop(&extra, LivingDefaults::default());
        let gap = pose_gap(&resolver, id);
        assert!(
            gap > 0.01,
            "a sapling that doesn't move tests nothing: {gap}"
        );

        let frames = play(&mut resolver, id, 6.0);
        let worst = frames
            .windows(2)
            .map(|w| farthest(&w[0].0, &w[1].0))
            .fold(0.0, f32::max);
        // A glide takes 0.15 s, 9 frames, and a smoothstep peaks at 1.5 times
        // its mean speed: about a sixth of a pose's distance in a frame. Parents
        // and children glide at different times, so a tip can add a few levels' moves.
        assert!(worst < gap * 0.45, "moved {worst} in a frame of {gap}");
    }

    #[test]
    fn at_glide_zero_it_holds_still_and_then_snaps_on_the_beat() {
        let extra = format!("glide = 0.0\nstep = 0.5\nstagger = \"none\"\n{NO_JITTER}");
        let (mut resolver, id) = one_prop(&extra, LivingDefaults::default());
        let phase = resolver.living_props().prop(id).phase_offset();
        let frames = play(&mut resolver, id, 6.0);

        let mut snaps = 0;
        for (i, w) in frames.windows(2).enumerate() {
            let moved = farthest(&w[0].0, &w[1].0);
            if w[1].1 {
                snaps += 1;
                assert!(moved > 0.0, "an upload that changed nothing, frame {i}");
            } else {
                assert_eq!(moved, 0.0, "frame {i} moved without a beat");
            }
        }
        // 6 s at two beats a second, starting at a phase of {phase}.
        assert!((11..=13).contains(&snaps), "{snaps} snaps at phase {phase}");
    }

    #[test]
    fn per_level_stagger_puts_each_level_at_its_own_place_in_the_cycle() {
        let extra = format!("glide = 1.0\nstagger = \"per_level\"\n{NO_JITTER}");
        let (mut resolver, id) = one_prop(&extra, LivingDefaults::default());
        let mut frame = BlendFrame::default();
        resolver.process_living(0.1);
        resolver.living_props_mut().blend_prop(id, &mut frame);

        let props = resolver.living_props();
        let (skeleton, _) = props.rig_of(id).unwrap();
        let drawn = &props.prop(id).drawn.bones.times;
        let by_depth = |depth: u32| {
            let at = skeleton.segments.iter().position(|s| s.depth == depth);
            drawn[at.expect("the sapling has this level")]
        };
        let shallowest = skeleton.segments.iter().map(|s| s.depth).min().unwrap();
        assert!(skeleton.max_depth() > shallowest, "needs two levels");
        assert_ne!(by_depth(shallowest), by_depth(skeleton.max_depth()));
        // Same depth, same place: only the level matters.
        for (s, time) in skeleton.segments.iter().zip(drawn) {
            assert_eq!(*time, by_depth(s.depth));
        }
    }

    #[test]
    fn the_runtime_and_the_strip_agree_on_every_bone() {
        // The strip blends a stream with `PoseStream`, the player a ring, and
        // both through `LivingPose::blend_from`, so the same poses and clock must
        // give the same bones.
        let extra = format!("stagger = \"per_level\"\n{NO_JITTER}");
        let (mut resolver, id) = one_prop(&extra, LivingDefaults::default());
        let mut frame = BlendFrame::default();
        let mut checked = 0;
        for _ in 0..25 {
            resolver.process_living(0.03);
            let blend = resolver.living_props_mut().blend_prop(id, &mut frame);
            if blend.first_bone != Some(0) {
                continue;
            }
            checked += 1;
            let props = resolver.living_props();
            let prop = props.prop(id);
            let (skeleton, rig) = props.rig_of(id).unwrap();

            let segments = skeleton
                .segments
                .iter()
                .enumerate()
                .map(|(i, s)| {
                    let (index, weight) = flint_core::wooze::warp(
                        prop.beats() + prop.clock.phase(s),
                        prop.clock.glide,
                    );
                    SegmentPose::blend(
                        &prop.pose(index).unwrap().segments[i],
                        &prop.pose(index + 1).unwrap().segments[i],
                        weight,
                    )
                })
                .collect();
            let by_hand = LivingPose {
                shape_seed: 0,
                segments,
            };
            let mut expected = Vec::new();
            rig.palette(skeleton, &by_hand, &mut expected).unwrap();
            assert_eq!(&frame.bones[..expected.len()], &expected[..]);
        }
        assert!(checked > 5, "only {checked} frames moved");
    }

    #[test]
    fn a_blend_uploads_the_whole_palette_with_still_jitter_bones_at_the_end() {
        let (mut resolver, id) = one_prop(NO_JITTER, LivingDefaults::default());
        let mut frame = BlendFrame::default();
        resolver.living_props_mut().blend_prop(id, &mut frame);
        let (_, rig) = resolver.living_props().rig_of(id).unwrap();
        assert_eq!(frame.bones.len(), rig.bone_segments.len() + BOIL_BONES);
        for bone in &frame.bones[rig.bone_segments.len()..] {
            assert_eq!(*bone, [[0.0; 4]; 4], "a still jitter bone adds nothing");
        }
    }

    #[test]
    fn a_paused_clock_costs_no_uploads() {
        let (mut resolver, id) = one_prop("", LivingDefaults::default());
        let mut frame = BlendFrame::default();
        resolver.process_living(0.3);
        assert_eq!(
            resolver
                .living_props_mut()
                .blend_prop(id, &mut frame)
                .first_bone,
            Some(0)
        );
        for _ in 0..10 {
            resolver.process_living(0.0);
            let blend = resolver.living_props_mut().blend_prop(id, &mut frame);
            assert_eq!(blend.first_bone, None);
            assert!(frame.tints.is_empty());
            assert!(!blend.shadow_changed);
        }
    }

    #[test]
    fn jitter_alone_writes_only_its_own_bones() {
        let extra = "glide = 0.0\nstep = 100.0\nstagger = \"none\"\njitter = { amplitude = 0.05 }";
        let (mut resolver, id) = one_prop(extra, LivingDefaults::default());
        let mut frame = BlendFrame::default();
        resolver.living_props_mut().blend_prop(id, &mut frame);
        let tail = resolver
            .living_props()
            .rig_of(id)
            .unwrap()
            .1
            .bone_segments
            .len();

        let mut writes = 0;
        for _ in 0..120 {
            resolver.process_living(1.0 / 60.0);
            if let Some(first) = resolver
                .living_props_mut()
                .blend_prop(id, &mut frame)
                .first_bone
            {
                assert_eq!(first, tail, "a holding pose isn't written again");
                assert_eq!(frame.bones.len(), BOIL_BONES);
                writes += 1;
            }
        }
        // 2 s of 4 channels at 8 beats a second, some of them on one frame.
        assert!((16..=64).contains(&writes), "{writes}");
    }

    #[test]
    fn a_missing_pose_leaves_the_prop_as_it_was_and_is_counted() {
        let (mut resolver, id) = one_prop(NO_JITTER, LivingDefaults::default());
        let mut frame = BlendFrame::default();
        resolver.living_props_mut().blend_prop(id, &mut frame);

        // Past the ring, with no frame budget to have filled it.
        let props = resolver.living_props_mut();
        for prop in &mut props.props {
            prop.beats += 1000.0;
            prop.ring.as_mut().unwrap().slide_to(100_000);
        }
        let blend = props.blend_prop(id, &mut frame);
        assert_eq!(blend.first_bone, None);
        assert_eq!(props.stats().blends_starved, 1);
    }

    #[test]
    fn drift_shadows_move_slower_than_the_prop_and_never_run_out_of_poses() {
        let defaults = LivingDefaults {
            shadows: LivingShadows::Drift,
            ..Default::default()
        };
        let extra = "glide = 0.3\nstep = 0.5\nstagger = \"none\"";
        let mut resolver = resolver_with(&sapling(extra));
        resolver.set_living_defaults(defaults);
        // The smallest ring it can be asked for, to prove the slow pair fits.
        let mut small = request(1, "oak");
        small.pose_ring = Some(2);
        let id = resolver.add_living_prop(small).unwrap().0;
        resolver.make_living_now();
        let (bone_count, tail) = {
            let rig = resolver.living_props().rig_of(id).unwrap().1;
            (
                rig.bone_segments.len() + BOIL_BONES,
                rig.bone_segments.len(),
            )
        };
        let mut frame = BlendFrame::default();
        let (mut shadow_writes, mut pose_writes) = (0, 0);
        for _ in 0..600 {
            resolver.process_living(1.0 / 60.0);
            let blend = resolver.living_props_mut().blend_prop(id, &mut frame);
            pose_writes += usize::from(blend.first_bone == Some(0));
            if blend.shadow_changed {
                shadow_writes += 1;
                assert_eq!(frame.shadow_bones.len(), bone_count);
                for bone in &frame.shadow_bones[tail..] {
                    assert_eq!(*bone, [[0.0; 4]; 4], "shadows don't jitter");
                }
            }
        }
        // Half the beats, each with a glide twice as long: about as many writes,
        // but a tree's shadow moves half as often.
        assert!(shadow_writes > 0);
        assert!(
            shadow_writes <= pose_writes + 20,
            "{shadow_writes} {pose_writes}"
        );
        assert_eq!(resolver.living_props().stats().blends_starved, 0);
    }

    #[test]
    fn blended_shadows_cost_nothing_extra() {
        let (mut resolver, id) = one_prop("", LivingDefaults::default());
        let mut frame = BlendFrame::default();
        for _ in 0..60 {
            resolver.process_living(1.0 / 60.0);
            let blend = resolver.living_props_mut().blend_prop(id, &mut frame);
            assert!(!blend.shadow_changed);
        }
    }

    #[test]
    fn an_echo_is_the_props_own_blend_at_an_offset_time() {
        let extra = format!("stagger = \"per_level\"\nechoes = {{ count = 6 }}\n{NO_JITTER}");
        let (mut resolver, id) = one_prop(&extra, LivingDefaults::default());
        assert_eq!(resolver.living_props().prop(id).echo_count(), 6);
        let mut frame = BlendFrame::default();
        let mut checked = 0;
        for _ in 0..40 {
            resolver.process_living(0.03);
            for echo in 0..6 {
                let first = resolver.living_props_mut().blend_echo(id, echo, &mut frame);
                if first != Some(0) {
                    continue;
                }
                checked += 1;
                let props = resolver.living_props();
                let prop = props.prop(id);
                let (skeleton, rig) = props.rig_of(id).unwrap();
                let segments = skeleton
                    .segments
                    .iter()
                    .enumerate()
                    .map(|(i, s)| {
                        let (index, weight) = flint_core::wooze::warp(
                            prop.echo_beats(echo) + prop.clock.phase(s),
                            prop.clock.glide,
                        );
                        SegmentPose::blend(
                            &prop.pose(index).unwrap().segments[i],
                            &prop.pose(index + 1).unwrap().segments[i],
                            weight,
                        )
                    })
                    .collect();
                let by_hand = LivingPose {
                    shape_seed: 0,
                    segments,
                };
                let mut expected = Vec::new();
                rig.palette(skeleton, &by_hand, &mut expected).unwrap();
                assert_eq!(&frame.bones[..expected.len()], &expected[..]);
            }
        }
        assert!(checked > 30, "only {checked} echo blends moved");
    }

    #[test]
    fn a_ring_has_every_pose_every_echo_wants() {
        for stagger in ["none", "per_level", "per_branch"] {
            // A small ring and the widest spread there is.
            let extra = format!("stagger = \"{stagger}\"\nechoes = {{ count = 6, spread = 1.8 }}");
            let (mut resolver, id) = one_prop(&extra, LivingDefaults::default());
            let mut frame = BlendFrame::default();
            for _ in 0..600 {
                resolver.process_living(1.0 / 60.0);
                let props = resolver.living_props_mut();
                props.blend_prop(id, &mut frame);
                for echo in 0..6 {
                    props.blend_echo(id, echo, &mut frame);
                }
            }
            assert_eq!(
                resolver.living_props().stats().blends_starved,
                0,
                "with {stagger} stagger"
            );
        }
    }

    #[test]
    fn drift_shadows_still_find_their_poses_with_echoes_on() {
        let defaults = LivingDefaults {
            shadows: LivingShadows::Drift,
            ..Default::default()
        };
        let extra = "echoes = { count = 6, spread = 1.8 }";
        let (mut resolver, id) = one_prop(extra, defaults);
        let mut frame = BlendFrame::default();
        let mut wrote = 0;
        for _ in 0..600 {
            resolver.process_living(1.0 / 60.0);
            let props = resolver.living_props_mut();
            let prop = props.prop(id);
            let slow = flint_core::wooze::warp(prop.beats() / 2.0, prop.clock.glide).0;
            let (from, to) = LivingClock::slow_pair(slow);
            assert!(prop.pose(from).is_some() && prop.pose(to).is_some());
            wrote += u32::from(props.blend_prop(id, &mut frame).shadow_changed);
        }
        assert!(wrote > 30, "the shadow only moved {wrote} times");
    }

    #[test]
    fn with_no_echoes_a_ring_is_what_it_was() {
        let off = "echoes = { count = 0 }\nstagger = \"per_level\"";
        let on = "echoes = { count = 6 }\nstagger = \"per_level\"";
        let (mut resolver_off, id_off) = one_prop(off, LivingDefaults::default());
        let (mut resolver_on, id_on) = one_prop(on, LivingDefaults::default());
        run(&mut resolver_off, 3.0, 1.0 / 60.0);
        run(&mut resolver_on, 3.0, 1.0 / 60.0);

        let props = resolver_off.living_props();
        let prop = props.prop(id_off);
        assert_eq!(prop.echo_count(), 0);
        // The window starts at now, as it did before echoes.
        assert_eq!(prop.first_step(), Some(prop.beats().floor() as u64));
        assert_eq!(prop.ring_len(), 4);

        // With them it starts behind, for the farthest one to read.
        let props = resolver_on.living_props();
        let prop = props.prop(id_on);
        assert!(prop.first_step().unwrap() < prop.beats().floor() as u64);
    }

    #[test]
    fn an_echo_costs_nothing_while_the_clock_is_paused() {
        let (mut resolver, id) = one_prop("echoes = { count = 2 }", LivingDefaults::default());
        let mut frame = BlendFrame::default();
        let props = resolver.living_props_mut();
        assert_eq!(props.blend_echo(id, 0, &mut frame), Some(0));
        assert_eq!(props.blend_echo(id, 1, &mut frame), Some(0));
        resolver.process_living(0.0);
        let props = resolver.living_props_mut();
        assert_eq!(props.blend_echo(id, 0, &mut frame), None);
        assert_eq!(props.blend_echo(id, 1, &mut frame), None);
    }

    #[test]
    fn an_echo_with_no_pose_writes_nothing_and_never_part_of_its_bones() {
        // It has no rest pose under it, so jitter alone can't be its first write.
        let extra = "echoes = { count = 2 }\njitter = { amplitude = 0.3, step = 0.01 }";
        let mut resolver = resolver_with(&sapling(extra));
        let id = resolver.add_living_prop(request(1, "oak")).unwrap().0;
        let props = resolver.living_props_mut();
        let mut frame = BlendFrame::default();
        assert_eq!(props.blend_echo(id, 0, &mut frame), None, "no lineage yet");
        resolver.make_living_now();
        let props = resolver.living_props_mut();
        let ring = props.props[id.0].ring.as_mut().unwrap();
        ring.slots.fill(None);
        ring.ready = 0;
        props.time = 0.05;
        assert_eq!(props.blend_echo(id, 0, &mut frame), None);
        assert_eq!(props.stats().blends_starved, 1);
    }

    #[test]
    fn a_scene_overrides_echoes_a_field_at_a_time() {
        let defaults = LivingDefaults {
            echoes: EchoOverride {
                count: Some(2),
                alpha: Some(0.5),
                ..Default::default()
            },
            ..Default::default()
        };
        let (resolver, id) = one_prop("echoes = { count = 6, spread = 0.5 }", defaults);
        let prop = resolver.living_props().prop(id);
        assert_eq!(prop.echo_count(), 2);
        assert_eq!(prop.echo(0).offset, -0.5);
        assert!((prop.echo(0).opacity - 0.5 * (1.0 - 0.5 * 0.55)).abs() < 1e-5);

        let off = LivingDefaults {
            echoes: EchoOverride {
                count: Some(0),
                ..Default::default()
            },
            ..Default::default()
        };
        let (resolver, id) = one_prop("echoes = { count = 6 }", off);
        assert_eq!(resolver.living_props().prop(id).echo_count(), 0);
    }

    #[test]
    fn leaf_tints_are_few_and_only_change_on_their_beat() {
        let spec = std::fs::read_to_string(OAK).unwrap();
        let mut resolver = resolver_with(&spec);
        let mut req = request(1, "oak");
        req.spec_name = "oak_living".into();
        let id = resolver.add_living_prop(req).unwrap().0;
        resolver.make_living_now();

        let mut frame = BlendFrame::default();
        let mut seen = std::collections::HashSet::new();
        let mut changes = 0;
        for _ in 0..240 {
            resolver.process_living(1.0 / 60.0);
            resolver.living_props_mut().blend_prop(id, &mut frame);
            for (part, tint) in &frame.tints {
                assert_ne!(*part, 0, "the bark isn't a leaf");
                seen.insert(tint.map(f32::to_bits));
                changes += 1;
            }
        }
        // 4 s at 8 beats a second on five shades, if none repeats a level.
        assert!(changes > 30 && changes <= 160, "{changes}");
        // Each distinct tint costs a material per shade, for the whole grove.
        assert!(seen.len() <= 9, "{} tints", seen.len());
    }

    // ---- The wooze field (#213) ----

    fn field(global: &str, zones: &[&str]) -> WoozeField {
        let mut field = WoozeField::new(WoozeSample::NEUTRAL);
        field
            .set_global(&global.parse().unwrap(), 0.0, 0.0)
            .unwrap();
        for zone in zones {
            field.add_zone(&zone.parse().unwrap()).unwrap();
        }
        field
    }

    /// One frame on `field`, with the bones of each prop that moved.
    fn tick_with(resolver: &mut ProcGenResolver, field: &WoozeField, time: f64, dt: f64) {
        resolver.process_living_in(dt, Some((field, time)));
    }

    const DREAMY: &str = r#"
        name = "pocket"
        shape = "sphere"
        center = [0, 0, 0]
        radius = 3
        falloff = 4
        glide = 0.8
    "#;

    /// The Done When: props inside a `glide = 0.8` sphere glide dreamier than
    /// those outside, with a smooth falloff between. A prop at glide `g` moves
    /// for `g` of its time, so that's what to count.
    #[test]
    fn a_zone_makes_the_props_inside_it_glide_longer_with_a_falloff_between() {
        let extra = format!("stagger = \"none\"\n{NO_JITTER}");
        let mut resolver = resolver_with(&sapling(&extra));
        let places = [("in", 0.0), ("edge", 5.0), ("out", 12.0)];
        let ids: Vec<_> = places
            .iter()
            .enumerate()
            .map(|(n, (name, x))| {
                let mut prop = request(n as u64, name);
                prop.position = [*x, 0.0, 0.0];
                resolver.add_living_prop(prop).unwrap().0
            })
            .collect();
        resolver.make_living_now();
        let field = field("step = 0.5\nglide = 0.3", &[DREAMY]);

        let mut frame = BlendFrame::default();
        let mut moving = [0usize; 3];
        let frames = 600;
        for f in 0..frames {
            let time = f64::from(f) / 60.0;
            tick_with(&mut resolver, &field, time, 1.0 / 60.0);
            for (i, id) in ids.iter().enumerate() {
                let blend = resolver.living_props_mut().blend_prop(*id, &mut frame);
                moving[i] += usize::from(blend.first_bone == Some(0));
            }
        }
        // The frame of each beat moves too, one in the 30 of a step.
        let share = |i: usize| moving[i] as f32 / frames as f32 - 1.0 / 30.0;
        assert!((share(0) - 0.8).abs() < 0.04, "inside {}", share(0));
        // Halfway down the falloff, it's halfway between the two.
        assert!((share(1) - 0.55).abs() < 0.04, "falloff {}", share(1));
        assert!((share(2) - 0.3).abs() < 0.04, "outside {}", share(2));
    }

    /// The tips' largest move on any frame of `seconds` of play, where `change`
    /// runs on the frame it's given.
    fn largest_move(
        resolver: &mut ProcGenResolver,
        id: LivingPropId,
        field: &mut WoozeField,
        seconds: f64,
        change_at: usize,
        change: impl Fn(&mut WoozeField, f64),
    ) -> (f32, usize) {
        let mut frame = BlendFrame::default();
        let mut time = 0.0;
        tick_with(resolver, field, time, 1.0 / 60.0);
        resolver.living_props_mut().blend_prop(id, &mut frame);
        let mut bones = frame.bones.clone();
        let mut worst = (0.0_f32, 0);
        for f in 0..(seconds * 60.0) as usize {
            if f == change_at {
                change(field, time);
            }
            time += 1.0 / 60.0;
            tick_with(resolver, field, time, 1.0 / 60.0);
            let blend = resolver.living_props_mut().blend_prop(id, &mut frame);
            if blend.first_bone == Some(0) {
                let moved = farthest(
                    &tips(resolver, id, &bones),
                    &tips(resolver, id, &frame.bones),
                );
                if moved > worst.0 {
                    worst = (moved, f);
                }
                bones.clone_from(&frame.bones);
            }
        }
        worst
    }

    /// The risk in the ticket. Going from a long glide to a short one mid-cycle
    /// computed from absolute time would jump the eased weight, and a new step
    /// would jump t' itself. Neither shows here.
    #[test]
    fn a_change_of_step_or_glide_at_any_moment_never_pops() {
        let extra = format!("stagger = \"per_level\"\n{NO_JITTER}");
        let run = |change: fn(&mut WoozeField, f64), change_at: usize| {
            let (mut resolver, id) = one_prop(&extra, LivingDefaults::default());
            let mut field = field("step = 0.5\nglide = 0.5", &[]);
            let gap = pose_gap(&resolver, id);
            let worst = largest_move(&mut resolver, id, &mut field, 6.0, change_at, change);
            assert!(worst.0 < gap * 0.45, "moved {worst:?} of a gap of {gap}");
        };
        // A spread of moments, so one lands in a hold, a glide and on a beat.
        for change_at in [60, 71, 83, 95, 107, 119] {
            run(
                |f, t| {
                    f.set_global(&"step = 5.0".parse().unwrap(), t, 0.0)
                        .unwrap()
                },
                change_at,
            );
            run(
                |f, t| {
                    f.set_global(&"glide = 1.0".parse().unwrap(), t, 0.0)
                        .unwrap()
                },
                change_at,
            );
            run(
                |f, t| {
                    f.set_global(&"glide = 0.2".parse().unwrap(), t, 0.0)
                        .unwrap()
                },
                change_at,
            );
            run(
                |f, t| {
                    f.set_global(&"amplitude = 0.0".parse().unwrap(), t, 0.0)
                        .unwrap()
                },
                change_at,
            );
        }
    }

    /// The Done When: a script changes the global step at runtime and every prop
    /// follows on its next cycle, and the clock doesn't jump to get there.
    #[test]
    fn a_new_global_step_reaches_every_prop_by_its_next_cycle() {
        let mut resolver = resolver_with(&sapling(NO_JITTER));
        let ids: Vec<_> = (0..4)
            .map(|n| {
                let mut prop = request(n, &format!("tree_{n}"));
                prop.position = [n as f32 * 20.0, 0.0, 0.0];
                resolver.add_living_prop(prop).unwrap().0
            })
            .collect();
        resolver.make_living_now();
        let mut field = field("step = 0.5", &[]);

        let dt = 1.0 / 60.0;
        let mut time = 0.0;
        for _ in 0..60 {
            time += dt;
            tick_with(&mut resolver, &field, time, dt);
        }
        let before: Vec<f64> = ids
            .iter()
            .map(|id| resolver.living_props().prop(*id).beats())
            .collect();
        field
            .set_global(&"step = 2.0".parse().unwrap(), time, 0.0)
            .unwrap();
        time += dt;
        tick_with(&mut resolver, &field, time, dt);
        for (id, before) in ids.iter().zip(&before) {
            let prop = resolver.living_props().prop(*id);
            assert_eq!(prop.clock().step, 2.0);
            // One frame of the new step, from where it stood: bent, not jumped.
            let moved = prop.beats() - before;
            assert!((moved - dt / 2.0).abs() < 1e-9, "{moved}");
        }

        // A full cycle now takes the new step's 2 s, for every prop.
        let start: Vec<f64> = ids
            .iter()
            .map(|id| resolver.living_props().prop(*id).beats())
            .collect();
        for _ in 0..120 {
            time += dt;
            tick_with(&mut resolver, &field, time, dt);
        }
        for (id, start) in ids.iter().zip(&start) {
            let beats = resolver.living_props().prop(*id).beats();
            assert!((beats - start - 1.0).abs() < 1e-9, "{}", beats - start);
        }
    }

    /// An echo is a number of steps from its prop, so a new step moves them both
    /// the same way, and each of its parts has its own cycle to hold on to.
    #[test]
    fn an_echo_keeps_its_distance_and_never_pops_through_a_change() {
        let extra = format!("stagger = \"per_level\"\nechoes = {{ count = 4 }}\n{NO_JITTER}");
        let (mut resolver, id) = one_prop(&extra, LivingDefaults::default());
        let gap = pose_gap(&resolver, id);
        let mut field = field("step = 0.5\nglide = 0.5", &[]);
        let offset = |r: &ProcGenResolver| {
            let prop = r.living_props().prop(id);
            prop.echo_beats(3) - prop.beats()
        };
        let before = offset(&resolver);

        let dt = 1.0 / 60.0;
        let mut frame = BlendFrame::default();
        let mut time = 0.0;
        let mut bones: Option<Vec<[[f32; 4]; 4]>> = None;
        let mut worst = 0.0_f32;
        for f in 0..360 {
            if f == 95 {
                field
                    .set_global(
                        &"step = 2.0\nglide = 1.0\namplitude = 0.2".parse().unwrap(),
                        time,
                        0.0,
                    )
                    .unwrap();
            }
            time += dt;
            tick_with(&mut resolver, &field, time, dt);
            let props = resolver.living_props_mut();
            props.blend_prop(id, &mut frame);
            if props.blend_echo(id, 3, &mut frame) == Some(0) {
                if let Some(last) = &bones {
                    let moved = farthest(
                        &tips(&resolver, id, last),
                        &tips(&resolver, id, &frame.bones),
                    );
                    worst = worst.max(moved);
                }
                bones = Some(frame.bones.clone());
            }
        }
        assert!((offset(&resolver) - before).abs() < 1e-9);
        assert!(worst > 0.0, "the echo never moved");
        assert!(worst < gap * 0.45, "moved {worst} of a gap of {gap}");
        assert_eq!(resolver.living_props().stats().blends_starved, 0);
    }

    #[test]
    fn amplitude_is_how_far_from_rest_a_prop_strays() {
        let extra = format!("stagger = \"none\"\n{NO_JITTER}");
        let spread = |amplitude: &str| {
            let (mut resolver, id) = one_prop(&extra, LivingDefaults::default());
            let field = field(amplitude, &[]);
            tick_with(&mut resolver, &field, 0.0, 1.0 / 60.0);
            let mut frame = BlendFrame::default();
            resolver.living_props_mut().blend_prop(id, &mut frame);
            let (skeleton, rig) = resolver.living_props().rig_of(id).unwrap();
            let rest = LivingPose {
                shape_seed: 0,
                segments: vec![SegmentPose::IDENTITY; skeleton.segments.len()],
            };
            let mut rest_bones = Vec::new();
            rig.palette(skeleton, &rest, &mut rest_bones).unwrap();
            farthest(
                &tips(&resolver, id, &rest_bones),
                &tips(&resolver, id, &frame.bones),
            )
        };
        let none = spread("amplitude = 0.0");
        let half = spread("amplitude = 0.5");
        let all = spread("amplitude = 1.0");
        assert!(none < 1e-4, "{none}");
        assert!(half > none && all > half, "{none} {half} {all}");
    }

    #[test]
    fn the_jitter_knob_scales_the_leaves_and_keeps_to_the_same_few_tints() {
        let spec = std::fs::read_to_string(OAK).unwrap();
        let turning = |jitter: &str| {
            let mut resolver = resolver_with(&spec);
            let mut req = request(1, "oak");
            req.spec_name = "oak_living".into();
            let id = resolver.add_living_prop(req).unwrap().0;
            resolver.make_living_now();
            let field = field(jitter, &[]);
            let mut frame = BlendFrame::default();
            let mut seen = std::collections::HashSet::new();
            let mut biggest = 0.0_f32;
            for f in 0..120 {
                tick_with(&mut resolver, &field, f64::from(f) / 60.0, 1.0 / 60.0);
                resolver.living_props_mut().blend_prop(id, &mut frame);
                for (_, tint) in &frame.tints {
                    seen.insert(tint.map(f32::to_bits));
                }
                let t =
                    resolver.living_props().prop(id).jitter_ahead + resolver.living_props().time();
                let jitter_now = resolver
                    .living_props()
                    .prop(id)
                    .jitter()
                    .scaled(resolver.living_props().prop(id).wooze().jitter);
                biggest = biggest.max(jitter_now.card_turn(1, 0, t).abs());
            }
            (seen.len(), biggest)
        };
        let (none_tints, none_turn) = turning("jitter = 0.0");
        assert_eq!((none_tints, none_turn), (0, 0.0), "no jitter, no boil");
        let (one_tints, one_turn) = turning("jitter = 1.0");
        let (more_tints, more_turn) = turning("jitter = 4.0");
        assert!(one_tints > 0 && one_tints <= 9, "{one_tints}");
        assert!(more_tints <= 9, "{more_tints} would be a material each");
        assert!(more_turn > one_turn, "{more_turn} {one_turn}");
    }

    #[test]
    fn a_prop_that_moves_reads_the_field_where_it_has_got_to() {
        let (mut resolver, id) = one_prop(NO_JITTER, LivingDefaults::default());
        let far = [50.0, 0.0, 0.0];
        resolver.living_props_mut().track_positions(|_| Some(far));
        let field = field("glide = 0.3", &[DREAMY]);
        tick_with(&mut resolver, &field, 0.0, 1.0 / 60.0);
        assert_eq!(resolver.living_props().prop(id).wooze().glide, 0.3);

        resolver
            .living_props_mut()
            .track_positions(|_| Some([0.0; 3]));
        tick_with(&mut resolver, &field, 0.1, 1.0 / 60.0);
        assert_eq!(resolver.living_props().prop(id).wooze().glide, 0.8);
        assert_eq!(resolver.living_props().prop(id).clock().glide, 0.8);
    }

    #[test]
    fn with_no_field_a_prop_keeps_to_its_spec() {
        let (mut resolver, id) = one_prop("step = 0.25\nglide = 0.9", LivingDefaults::default());
        for _ in 0..30 {
            resolver.process_living(1.0 / 60.0);
        }
        let prop = resolver.living_props().prop(id);
        assert_eq!((prop.clock().step, prop.clock().glide), (0.25, 0.9));
        assert_eq!(prop.wooze().amplitude, 1.0);
    }

    #[test]
    fn a_field_beats_the_spec_wherever_it_covers() {
        let (mut resolver, id) = one_prop("step = 0.25\nglide = 0.9", LivingDefaults::default());
        let field = field("step = 2.0\nglide = 0.0", &[]);
        tick_with(&mut resolver, &field, 0.0, 1.0 / 60.0);
        let clock = resolver.living_props().prop(id).clock();
        assert_eq!((clock.step, clock.glide), (2.0, 0.0));
    }
}
