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
//!
//! # Turning knobs while it runs (#218)
//!
//! A ring keeps each pose as its shape seed grew it, beside the same pose held
//! to the lineage's variance. A new variance re-holds what's there, a few poses
//! a frame out of the same budget, with no regrowing, and the cache keeps only
//! the first form, so a change reaches a cached pose on its way out. With
//! `keep_size` a re-hold costs nearly what growing a pose does, so the poses in
//! use go first, across the whole grove: that's the part anyone can see.
//!
//! New echoes or a new stagger can need poses a ring hasn't got: a wider spread
//! wants steps it has already left behind. So the ring grows first, to cover what
//! the prop is doing and what it's about to do, and the prop keeps its old clocks
//! until the new ones have every pose they'd read. No frame goes without one.

use std::collections::{BTreeMap, HashMap, VecDeque};
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
use crate::spec::{EchoConfig, EchoCopy, ProcGenSpec, Stagger, VarianceConfig};
use crate::types::BranchSegment;

/// How many bytes of poses the cache keeps once no ring holds them. A pose of
/// the oak is under 6 KB, so this is about 1,400 of them: a grove's rings four
/// times over.
pub const POSE_CACHE_BYTES: usize = 8 * 1024 * 1024;

/// The re-hold pass that reaches poses nothing is reading yet, which take turns
/// with making new ones. See [`LivingProp::rehold_span`].
const REHOLD_PASS_REST: u8 = 2;

/// How fast the pose estimate forgets its slowest pose, each pose. Poses vary
/// by a factor of two, so an estimate that forgets within a frame or two lets a
/// slow one overrun the budget. At this rate it takes about a second of a grove's
/// poses to halve.
const POSE_ESTIMATE_DECAY: f64 = 0.99;

/// A leaf part's tint as no tint step can be: one that has to be sent again.
const TINT_UNSENT: i32 = i32::MIN;

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
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LivingDefaults {
    /// What the shadows follow.
    pub shadows: LivingShadows,
    /// The scene's word on echoes, over each spec's, a field at a time.
    pub echoes: EchoOverride,
    /// The scene's word on stagger, over each spec's and each prop's own: it's
    /// what a tuned preset holds, and has to look the same when it loads.
    pub stagger: Option<Stagger>,
    /// The scene's word on variance, over each spec's, a field at a time.
    pub variance: VarianceOverride,
    /// `[living.species.<spec>]` (#246): each species' own values, over all of
    /// the above and over the wooze field, zones included. Keyed by spec name.
    pub species: BTreeMap<String, SpeciesOverride>,
}

/// One species' own values (#246): a layer over everything the scene says for
/// every prop, so a global knob moved live leaves them standing. A species is a
/// spec, with all its lineage seeds. Unset fields are whatever's below.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct SpeciesOverride {
    /// Its clock's knobs, over what the field says where each prop stands, or
    /// over its spec's in a scene with no field.
    pub clock: ClockOverride,
    /// Its stagger, over the scene's, each prop's own and the spec's.
    pub stagger: Option<Stagger>,
    /// Its echoes, a field at a time.
    pub echoes: EchoOverride,
    /// Its variance, a field at a time.
    pub variance: VarianceOverride,
}

impl SpeciesOverride {
    /// These values over `under`'s, a field at a time.
    pub fn merged(self, under: SpeciesOverride) -> SpeciesOverride {
        SpeciesOverride {
            clock: self.clock.merged(under.clock),
            stagger: self.stagger.or(under.stagger),
            echoes: self.echoes.merged(under.echoes),
            variance: self.variance.merged(under.variance),
        }
    }

    /// Whether it says nothing, and the species is as the scene would have it.
    pub fn is_empty(&self) -> bool {
        *self == SpeciesOverride::default()
    }
}

/// A species' word on the four knobs a wooze field has (#246).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct ClockOverride {
    /// How much of its slow drift shows.
    pub amplitude: Option<f32>,
    /// Seconds between pose targets.
    pub step: Option<f32>,
    /// How much of a step is spent easing.
    pub glide: Option<f32>,
    /// How much of its fast jitter shows.
    pub jitter: Option<f32>,
}

impl ClockOverride {
    /// What a prop reads: these over what the layers below it said.
    pub fn over(&self, below: WoozeSample) -> WoozeSample {
        WoozeSample {
            amplitude: self.amplitude.unwrap_or(below.amplitude),
            step: self.step.unwrap_or(below.step),
            glide: self.glide.unwrap_or(below.glide),
            jitter: self.jitter.unwrap_or(below.jitter),
        }
    }

    /// These over `under`, a field at a time.
    pub fn merged(self, under: ClockOverride) -> ClockOverride {
        ClockOverride {
            amplitude: self.amplitude.or(under.amplitude),
            step: self.step.or(under.step),
            glide: self.glide.or(under.glide),
            jitter: self.jitter.or(under.jitter),
        }
    }
}

/// Which props a live change reaches (#246): every one, or one species'.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LivingScope<'a> {
    /// The global layer: every prop, under any species' own value.
    All,
    /// One species' layer, by spec name, which beats the global one.
    Species(&'a str),
}

/// Where a prop's value came from (#246), lowest first. A layer beats every one
/// before it in this list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LivingLayer {
    /// The spec's `[living]` table, or for amplitude and jitter, as authored.
    Spec,
    /// The entity's `living_prop` component.
    Entity,
    /// The scene's `[living]` or `[wooze]` globals, or a knob moved live for the
    /// whole grove.
    Global,
    /// A wooze zone, by name, where the prop stands.
    Zone(String),
    /// `[living.species.<spec>]`, or a knob moved live for that species.
    Species,
}

impl LivingLayer {
    /// How a report names it: `spec`, `entity`, `global`, `zone:<name>` or
    /// `species`.
    pub fn label(&self) -> String {
        match self {
            LivingLayer::Spec => "spec".to_string(),
            LivingLayer::Entity => "entity".to_string(),
            LivingLayer::Global => "global".to_string(),
            LivingLayer::Zone(name) => format!("zone:{name}"),
            LivingLayer::Species => "species".to_string(),
        }
    }
}

/// A value and the layer that set it.
#[derive(Debug, Clone, PartialEq)]
pub struct Resolved<T> {
    /// What the prop runs on.
    pub value: T,
    /// Who said so.
    pub layer: LivingLayer,
}

/// Every value a species can set on one prop, resolved, each with its layer.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedProp {
    /// Its spec's name.
    pub species: String,
    pub amplitude: Resolved<f32>,
    pub step: Resolved<f32>,
    pub glide: Resolved<f32>,
    pub jitter: Resolved<f32>,
    pub stagger: Resolved<Stagger>,
    pub echo_count: Resolved<u32>,
    pub echo_spread: Resolved<f32>,
    pub echo_alpha: Resolved<f32>,
    pub variance_amount: Resolved<f32>,
    pub variance_steady: Resolved<f32>,
    pub keep_size: Resolved<bool>,
}

/// Every layer the grove's values are made of, apart from the field, to put back
/// with [`LivingProps::restore_layers`]: what "As loaded" goes back to.
#[derive(Debug, Clone, PartialEq)]
pub struct LivingLayers {
    echoes: Vec<EchoOverride>,
    staggers: Vec<Option<Stagger>>,
    clocks: Vec<(WoozeSample, bool)>,
    variances: Vec<VarianceOverride>,
    species: Vec<(String, SpeciesOverride)>,
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

/// A scene's `[living] variance`: whichever of a spec's fields it sets.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct VarianceOverride {
    /// How much of each pose's difference is left. `None`: the spec's.
    pub amount: Option<f32>,
    /// How much of that is held back from the trunk. `None`: the spec's.
    pub steady: Option<f32>,
    /// Whether poses keep the base's size. `None`: the spec's.
    pub keep_size: Option<bool>,
}

impl VarianceOverride {
    /// These fields over a whole config.
    pub fn over(&self, spec: VarianceConfig) -> VarianceConfig {
        VarianceConfig {
            amount: self.amount.unwrap_or(spec.amount),
            steady: self.steady.unwrap_or(spec.steady),
            keep_size: self.keep_size.unwrap_or(spec.keep_size),
        }
    }

    /// Every field set, to these.
    pub fn full(variance: VarianceConfig) -> Self {
        Self {
            amount: Some(variance.amount),
            steady: Some(variance.steady),
            keep_size: Some(variance.keep_size),
        }
    }

    /// These over `under`, a field at a time.
    pub fn merged(self, under: Self) -> Self {
        Self {
            amount: self.amount.or(under.amount),
            steady: self.steady.or(under.steady),
            keep_size: self.keep_size.or(under.keep_size),
        }
    }
}

impl EchoOverride {
    /// These fields over a whole config.
    pub fn over(&self, spec: EchoConfig) -> EchoConfig {
        EchoConfig {
            count: self.count.unwrap_or(spec.count),
            spread: self.spread.unwrap_or(spec.spread),
            alpha: self.alpha.unwrap_or(spec.alpha),
        }
    }

    /// Every field set, to these.
    pub fn full(echoes: EchoConfig) -> Self {
        Self {
            count: Some(echoes.count),
            spread: Some(echoes.spread),
            alpha: Some(echoes.alpha),
        }
    }

    /// These over `under`, a field at a time.
    pub fn merged(self, under: Self) -> Self {
        Self {
            count: self.count.or(under.count),
            spread: self.spread.or(under.spread),
            alpha: self.alpha.or(under.alpha),
        }
    }
}

/// The layer a field came from, out of a species', the global one and the spec.
fn layer_of(species: bool, global: bool) -> LivingLayer {
    if species {
        LivingLayer::Species
    } else if global {
        LivingLayer::Global
    } else {
        LivingLayer::Spec
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
    /// How many echoes it draws now, if that may have changed since last frame.
    pub echo_count: Option<usize>,
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
    /// Poses held again at a new variance, with no regrowing (#218).
    pub poses_reheld: u64,
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
    slots: Vec<Option<RingPose>>,
    ready: usize,
}

/// One pose in a ring, in both of its forms.
#[derive(Debug, Clone)]
struct RingPose {
    /// As its shape seed grew it. It never changes, so a new variance starts here.
    full: Arc<LivingPose>,
    /// Held to the lineage's variance: what a prop draws.
    held: Arc<LivingPose>,
    /// Which of the lineage's variances `held` is at.
    generation: u64,
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
        self.slots[self.slot(index)].as_ref().map(|pose| &pose.held)
    }

    /// Whether it holds every pose from `first` for `count` steps.
    fn holds(&self, first: u64, count: u64) -> bool {
        (first..first + count).all(|index| self.get(index).is_some())
    }

    /// A window at `first` of `len` poses and the spare, keeping whatever both
    /// windows have. Unlike [`slide_to`](Self::slide_to) this can move back, and
    /// what's now missing behind is what gets made next.
    fn reshape(&mut self, first: u64, len: usize) {
        if first == self.first && len == self.len() {
            return;
        }
        let mut new = Self::new(first, len);
        for index in self.first..self.first + self.slots.len() as u64 {
            if new.contains(index) {
                let slot = self.slot(index);
                if let Some(pose) = self.slots[slot].take() {
                    new.put(index, pose);
                }
            }
        }
        *self = new;
    }

    /// The earliest of `count` steps from `first` whose pose isn't at `generation`.
    fn next_stale(&self, first: u64, count: u64, generation: u64) -> Option<u64> {
        (first..first + count).find(|&index| {
            self.contains(index)
                && self.slots[self.slot(index)]
                    .as_ref()
                    .is_some_and(|pose| pose.generation != generation)
        })
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

    fn put(&mut self, index: u64, pose: RingPose) {
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
    /// What its echoes are set to, or are about to be, in the units a person
    /// turns: [`Self::echoes`] is what those work out to.
    echo_config: EchoConfig,
    /// How deep its lineage's tree is, once built. A ring's size depends on it.
    max_depth: u32,
    /// Clocks it's been asked for and is waiting on poses to start.
    pending: Option<PendingClocks>,
    /// Whether its echoes have changed since it last blended.
    echoes_changed: bool,
    /// Its species, as an index into [`LivingProps::species`].
    species: usize,
    /// What the layers under its species say of its clock: the field where it
    /// stands, or in a scene with none its spec's, or what was set live.
    wooze_below: WoozeSample,
    /// Whether `wooze_below` was set live in a scene with no field, which makes
    /// it the global layer's and not the spec's.
    clock_live: bool,
    /// Its spec's echoes, the lowest layer.
    echo_spec: EchoConfig,
    /// The global layer's word on its echoes: the scene's `[living] echoes` and
    /// whatever was moved live for every prop.
    echo_global: EchoOverride,
    /// Its own stagger, the entity's or else the spec's, and whether it's the
    /// entity's.
    stagger_own: Stagger,
    stagger_entity: bool,
    /// The global layer's word on its stagger.
    stagger_global: Option<Stagger>,
}

/// New clocks for a prop, held back until its ring has what they read.
#[derive(Debug, Clone)]
struct PendingClocks {
    stagger: Stagger,
    copies: Vec<EchoCopy>,
}

/// What a set of clocks asks of a prop's stream of poses, at this moment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RingNeeds {
    /// The earliest step any part of it can be reading.
    first: u64,
    /// How many steps from there it can read: without these it would starve.
    reads: u64,
    /// How many poses a ring for it keeps. At least `reads`, and the prop's
    /// `pose_ring`.
    len: u64,
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

    /// What its clock runs on: what the wooze field last said about its place, or
    /// what its spec says in a scene with no field, with its species' own values
    /// over either.
    pub fn wooze(&self) -> WoozeSample {
        self.wooze
    }

    /// What its clock would run on without its species' own values (#246): what
    /// a global knob starts from.
    pub fn wooze_below(&self) -> WoozeSample {
        self.wooze_below
    }

    /// Each part's place on the clock as of its last blend, in the skeleton's
    /// order, trunk first: which poses it was really drawn between. Empty until
    /// it has posed.
    pub fn drawn_times(&self) -> &[WarpedTime] {
        &self.drawn.bones.times
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

    /// The first step its current clocks can be reading: the farthest echo's.
    /// A pose stays until the last echo has finished with it.
    fn in_use_start(&self) -> u64 {
        let behind = self
            .echoes
            .iter()
            .map(|e| -e.copy.offset)
            .fold(0.0, f32::max);
        (self.beats - f64::from(behind)).max(0.0).floor() as u64
    }

    /// The first step its ring's window needs now: what's in use, and what's
    /// waiting to be.
    fn window_start(&self) -> u64 {
        let start = self.in_use_start();
        match &self.pending {
            None => start,
            Some(pending) => start.min(
                self.needs_of(pending.stagger, pending.copies.iter().copied(), false)
                    .first,
            ),
        }
    }

    /// What this stagger and these echoes ask of the ring now. `drift`: the
    /// scene's shadows follow the slow pair, up to three steps ahead of now.
    fn needs_of(
        &self,
        stagger: Stagger,
        copies: impl Iterator<Item = EchoCopy>,
        drift: bool,
    ) -> RingNeeds {
        let (mut behind, mut ahead) = (0.0f32, 0.0f32);
        for copy in copies {
            behind = behind.max(-copy.offset);
            ahead = ahead.max(copy.offset);
        }
        let (behind, ahead) = (f64::from(behind), f64::from(ahead));
        let clock = LivingClock {
            stagger,
            ..self.clock
        };
        let mut reads = clock.reach_across(self.max_depth, behind + ahead) + 1;
        if drift {
            reads = reads.max(4 + behind.ceil() as u64);
        }
        RingNeeds {
            first: (self.beats - behind).max(0.0).floor() as u64,
            reads,
            len: u64::from(self.pose_ring).max(reads),
        }
    }

    /// The steps a re-hold looks at, in its pass: what the prop itself is drawing,
    /// then what its echoes are too, then everything else in the ring.
    fn rehold_span(&self, pass: u8, drift: bool) -> (u64, u64) {
        match pass {
            0 => (
                self.beats.max(0.0).floor() as u64,
                self.clock.reach(self.max_depth) + 1,
            ),
            1 => {
                let needs = self.needs_now(drift);
                (needs.first, needs.reads)
            }
            _ => self
                .ring
                .as_ref()
                .map_or((0, 0), |ring| (ring.first, ring.slots.len() as u64)),
        }
    }

    /// What its current clocks ask of the ring.
    fn needs_now(&self, drift: bool) -> RingNeeds {
        self.needs_of(
            self.clock.stagger,
            self.echoes.iter().map(|e| e.copy),
            drift,
        )
    }

    /// The first pose its current clocks read that isn't there, or will be read
    /// next, and how many steps into what they read it is: the fewer, the sooner.
    /// Only within the ring's window: a step past it has nowhere to go yet.
    fn next_needed(&self, drift: bool) -> Option<(u64, u64)> {
        let ring = self.ring.as_ref()?;
        let needs = self.needs_now(drift);
        (0..needs.reads + 1)
            .find(|steps| {
                let step = needs.first + steps;
                ring.contains(step) && ring.get(step).is_none()
            })
            .map(|steps| (needs.first + steps, steps))
    }

    /// The window that covers what's in use and what's waiting.
    fn window(&self, drift: bool) -> (u64, usize) {
        let now = self.needs_now(drift);
        let (first, end) = match &self.pending {
            None => (now.first, now.first + now.len),
            Some(pending) => {
                let next = self.needs_of(pending.stagger, pending.copies.iter().copied(), drift);
                (
                    now.first.min(next.first),
                    (now.first + now.len).max(next.first + next.len),
                )
            }
        };
        (first, (end - first) as usize)
    }

    /// Make the ring's window the one its clocks want now.
    fn fit_ring(&mut self, drift: bool) {
        let (first, len) = self.window(drift);
        if let Some(ring) = &mut self.ring {
            ring.reshape(first, len);
        }
    }

    /// Start the waiting clocks. An echo that stays where it was keeps its
    /// cycles, so a change of opacity alone moves nothing.
    fn start_pending(&mut self) {
        let Some(pending) = self.pending.take() else {
            return;
        };
        self.clock.stagger = pending.stagger;
        let old = std::mem::take(&mut self.echoes);
        self.echoes = pending
            .copies
            .into_iter()
            .enumerate()
            .map(|(i, copy)| PropEcho {
                copy,
                drawn: old
                    .get(i)
                    .filter(|was| was.copy.offset == copy.offset)
                    .map(|was| DrawnBones {
                        times: Vec::new(),
                        ..was.drawn.clone()
                    })
                    .unwrap_or_default(),
            })
            .collect();
        self.echoes_changed = true;
    }

    /// Whatever it last wrote is out of date, though no clock has moved.
    fn redraw(&mut self) {
        self.drawn.bones.times.clear();
        self.drawn.shadow = None;
        for echo in &mut self.echoes {
            echo.drawn.times.clear();
        }
    }

    /// What its echoes are set to, as a person would turn them.
    pub fn echo_config(&self) -> EchoConfig {
        self.echo_config
    }

    /// Whether it's still on its old clocks, waiting for poses for new ones.
    pub fn is_waiting(&self) -> bool {
        self.pending.is_some()
    }

    /// The stagger it's on, or about to be.
    pub fn stagger(&self) -> Stagger {
        self.pending
            .as_ref()
            .map_or(self.clock.stagger, |pending| pending.stagger)
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

    /// Step `index`'s pose as its shape seed grew it, before any variance.
    pub fn full_pose(&self, index: u64) -> Option<&Arc<LivingPose>> {
        let ring = self.ring.as_ref()?;
        if !ring.contains(index) {
            return None;
        }
        ring.slots[ring.slot(index)].as_ref().map(|pose| &pose.full)
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
    /// How tall it stands at rest. Kept, since working it out poses the whole tree.
    rest_height: f32,
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
    /// What its poses are held to. Starts as the spec's, with the scene's word.
    variance: VarianceConfig,
    /// Counts the changes to `variance`, so a ring can tell what's out of date.
    generation: u64,
    /// Its species, as an index into [`LivingProps::species`].
    species: usize,
    /// The global layer's word on its variance, over the spec's.
    variance_global: VarianceOverride,
}

/// A species and its own values (#246).
#[derive(Debug, Clone)]
struct Species {
    name: String,
    over: SpeciesOverride,
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
    /// Every species a prop has been added from, or a value set for, by spec
    /// name. Never shrinks while the scene runs, so an index stays good.
    species: Vec<Species>,
    cache: PoseCache,
    /// The clock every prop reads, in seconds. It stops when the game pauses.
    time: f64,
    /// How long the next pose is likely to take: the slowest one lately.
    pose_estimate_ms: f64,
    stats: LivingStats,
    defaults: LivingDefaults,
    /// Whether any ring may hold poses at an old variance.
    reholding: bool,
    /// The prop a re-hold has got to, this pass. Those before it are done.
    rehold_cursor: usize,
    /// How much of a ring it's on: see [`LivingProp::rehold_span`].
    rehold_pass: u8,
    /// Takes turns with filling, once what's in use is done, so that neither can
    /// starve the other.
    rehold_turn: bool,
}

impl LivingProps {
    /// No props, and a pose cache that keeps `pose_cache_bytes`.
    pub fn new(pose_cache_bytes: usize) -> Self {
        Self {
            lineages: Vec::new(),
            props: Vec::new(),
            species: Vec::new(),
            cache: PoseCache::new(pose_cache_bytes),
            time: 0.0,
            pose_estimate_ms: 0.0,
            stats: LivingStats::default(),
            defaults: LivingDefaults::default(),
            reholding: false,
            rehold_cursor: 0,
            rehold_pass: 0,
            rehold_turn: false,
        }
    }

    /// What the scene says about its props, for the props added after this.
    pub fn set_defaults(&mut self, defaults: LivingDefaults) {
        self.defaults = defaults;
    }

    /// The index of a species by spec name, made if it's new, with whatever the
    /// scene says for it.
    fn species_index(&mut self, name: &str) -> usize {
        if let Some(i) = self.species.iter().position(|s| s.name == name) {
            return i;
        }
        self.species.push(Species {
            name: name.to_string(),
            over: self.defaults.species.get(name).copied().unwrap_or_default(),
        });
        self.species.len() - 1
    }

    /// What the scene's living shadows follow. One for the whole scene: a
    /// species can't set it (#247).
    pub fn shadows(&self) -> LivingShadows {
        self.defaults.shadows
    }

    /// Every species by spec name, in the order they were first met: each one a
    /// prop grows from, and any a value was set for.
    pub fn species_names(&self) -> Vec<&str> {
        self.species.iter().map(|s| s.name.as_str()).collect()
    }

    /// A species' own values, if it's one this has met.
    pub fn species_override(&self, name: &str) -> Option<SpeciesOverride> {
        self.species.iter().find(|s| s.name == name).map(|s| s.over)
    }

    /// Whether any prop grows from this spec.
    pub fn has_species(&self, name: &str) -> bool {
        self.props
            .iter()
            .any(|prop| self.species[prop.species].name == name)
    }

    /// A prop's species: its spec's name.
    pub fn species_of(&self, id: LivingPropId) -> &str {
        &self.species[self.props[id.0].species].name
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

    /// Which tree a prop grows from.
    pub fn lineage_seed(&self, id: LivingPropId) -> u64 {
        self.lineages[self.props[id.0].lineage].seed
    }

    /// How tall a prop stands at rest, once its lineage is built.
    pub fn rest_height(&self, id: LivingPropId) -> Option<f32> {
        let built = self.lineages[self.props[id.0].lineage].built.as_ref()?;
        Some(built.rest_height)
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
        self.species.clear();
        self.time = 0.0;
        self.stats = LivingStats::default();
        self.reholding = false;
        self.rehold_cursor = 0;
        self.rehold_pass = 0;
    }

    /// Give every prop these echoes, as soon as its ring can show them. Until
    /// then it keeps the ones it has, and a change that needs no new pose takes
    /// effect on the next tick. This is the global layer: a species' own echoes
    /// (#246) still beat it, a field at a time.
    pub fn set_echoes(&mut self, echoes: EchoConfig) {
        for i in 0..self.props.len() {
            self.set_echoes_of(LivingPropId(i), echoes);
        }
    }

    /// Give one prop these echoes in the global layer, as [`Self::set_echoes`]
    /// does every prop: a grove of several species keeps each one's own.
    pub fn set_echoes_of(&mut self, id: LivingPropId, echoes: EchoConfig) {
        self.props[id.0].echo_global = EchoOverride::full(echoes);
        self.refresh_echoes(id.0);
    }

    /// Set whichever echo fields `echoes` sets, for every prop in the global
    /// layer or in one species' own (#246). Returns whether any prop has them:
    /// a species no prop grows from keeps them, for none.
    pub fn tune_echoes(&mut self, scope: LivingScope, echoes: EchoOverride) -> bool {
        match scope {
            LivingScope::All => {
                for prop in &mut self.props {
                    prop.echo_global = echoes.merged(prop.echo_global);
                }
            }
            LivingScope::Species(name) => {
                let species = self.species_index(name);
                let over = &mut self.species[species].over;
                over.echoes = echoes.merged(over.echoes);
            }
        }
        self.refresh_props(scope)
    }

    /// Put every prop on this stagger, as soon as its ring can show it. A part
    /// jumps to its new place on the clock: no stagger can change without that.
    /// The global layer: a species' own stagger still beats it.
    pub fn set_stagger(&mut self, stagger: Stagger) {
        for i in 0..self.props.len() {
            self.set_stagger_of(LivingPropId(i), stagger);
        }
    }

    /// Put one prop on this stagger in the global layer, as [`Self::set_stagger`]
    /// does every prop.
    pub fn set_stagger_of(&mut self, id: LivingPropId, stagger: Stagger) {
        self.props[id.0].stagger_global = Some(stagger);
        self.refresh_stagger(id.0);
    }

    /// Put every prop on this stagger in the global layer, or one species in its
    /// own (#246). Returns whether any prop is of it.
    pub fn tune_stagger(&mut self, scope: LivingScope, stagger: Stagger) -> bool {
        match scope {
            LivingScope::All => {
                for prop in &mut self.props {
                    prop.stagger_global = Some(stagger);
                }
            }
            LivingScope::Species(name) => {
                let species = self.species_index(name);
                self.species[species].over.stagger = Some(stagger);
            }
        }
        self.refresh_props(scope)
    }

    /// Put one prop's clock where `wooze` says, as the field would in a scene
    /// that has one. For giving a scene with no field its specs' clocks back:
    /// a field, while there is one, says again each tick. Its species' own clock
    /// values (#246) still beat this.
    pub fn set_wooze_of(&mut self, id: LivingPropId, wooze: WoozeSample) {
        let prop = &mut self.props[id.0];
        prop.wooze_below = wooze;
        prop.clock_live = true;
        self.refresh_clock(id.0);
    }

    /// Set whichever clock knobs `clock` sets for one species (#246), over the
    /// field and its zones. A species is the only scope this has: the global
    /// clock is the field's. Returns whether any prop is of it.
    pub fn tune_clock(&mut self, species: &str, clock: ClockOverride) -> bool {
        let index = self.species_index(species);
        let over = &mut self.species[index].over;
        over.clock = clock.merged(over.clock);
        self.refresh_props(LivingScope::Species(species))
    }

    /// Replace one species' own values whole (#246), which is how one is cleared,
    /// a knob or all of them at a time. Returns whether any prop is of it.
    pub fn set_species(&mut self, species: &str, over: SpeciesOverride) -> bool {
        let index = self.species_index(species);
        self.species[index].over = over;
        self.refresh_props(LivingScope::Species(species))
    }

    /// Hold every lineage's poses to this variance: what the rings have is
    /// re-held from its full form, a few poses a frame, and what's made from
    /// here on starts at it. The global layer: a species' own variance beats it.
    pub fn set_variance(&mut self, variance: VarianceConfig) {
        for lineage in 0..self.lineages.len() {
            self.set_variance_of(lineage, variance);
        }
    }

    /// Hold one lineage's poses to this variance in the global layer, as
    /// [`Self::set_variance`] does every lineage's. `lineage` counts as
    /// [`Self::variances`] does.
    pub fn set_variance_of(&mut self, lineage: usize, variance: VarianceConfig) {
        self.lineages[lineage].variance_global = VarianceOverride::full(variance);
        self.refresh_variance(lineage);
    }

    /// Set whichever variance fields `variance` sets, for every lineage in the
    /// global layer or one species' own (#246). A lineage belongs to one spec, so
    /// a species' change re-holds only its lineages. Returns whether any prop is
    /// of it.
    pub fn tune_variance(&mut self, scope: LivingScope, variance: VarianceOverride) -> bool {
        match scope {
            LivingScope::All => {
                for lineage in &mut self.lineages {
                    lineage.variance_global = variance.merged(lineage.variance_global);
                }
            }
            LivingScope::Species(name) => {
                let species = self.species_index(name);
                let over = &mut self.species[species].over;
                over.variance = variance.merged(over.variance);
            }
        }
        self.refresh_props(scope)
    }

    /// Every layer as it stands, apart from the field, to go back to.
    pub fn layers(&self) -> LivingLayers {
        LivingLayers {
            echoes: self.props.iter().map(|p| p.echo_global).collect(),
            staggers: self.props.iter().map(|p| p.stagger_global).collect(),
            clocks: (self.props.iter())
                .map(|p| (p.wooze_below, p.clock_live))
                .collect(),
            variances: (self.lineages.iter()).map(|l| l.variance_global).collect(),
            species: (self.species.iter())
                .map(|s| (s.name.clone(), s.over))
                .collect(),
        }
    }

    /// Put every layer back as [`Self::layers`] found it. In a scene with a
    /// field the clocks come from it again on the next tick.
    pub fn restore_layers(&mut self, layers: &LivingLayers) {
        for (prop, echoes) in self.props.iter_mut().zip(&layers.echoes) {
            prop.echo_global = *echoes;
        }
        for (prop, stagger) in self.props.iter_mut().zip(&layers.staggers) {
            prop.stagger_global = *stagger;
        }
        for (prop, (below, live)) in self.props.iter_mut().zip(&layers.clocks) {
            prop.wooze_below = *below;
            prop.clock_live = *live;
        }
        for (lineage, variance) in self.lineages.iter_mut().zip(&layers.variances) {
            lineage.variance_global = *variance;
        }
        for species in &mut self.species {
            species.over = (layers.species.iter())
                .find(|(name, _)| *name == species.name)
                .map_or_else(SpeciesOverride::default, |(_, over)| *over);
        }
        self.refresh_props(LivingScope::All);
    }

    /// Bring every prop in `scope`, and its lineage, up to what its layers say
    /// now. Returns whether there were any.
    fn refresh_props(&mut self, scope: LivingScope) -> bool {
        let wanted = match scope {
            LivingScope::All => None,
            LivingScope::Species(name) => match self.species.iter().position(|s| s.name == name) {
                Some(index) => Some(index),
                None => return false,
            },
        };
        let mut any = false;
        for i in 0..self.props.len() {
            if wanted.is_some_and(|species| self.props[i].species != species) {
                continue;
            }
            any = true;
            self.refresh_echoes(i);
            self.refresh_stagger(i);
            self.refresh_clock(i);
        }
        for lineage in 0..self.lineages.len() {
            if wanted.is_some_and(|species| self.lineages[lineage].species != species) {
                continue;
            }
            self.refresh_variance(lineage);
        }
        any
    }

    /// One prop's echoes, from its layers.
    fn refresh_echoes(&mut self, i: usize) {
        let prop = &self.props[i];
        let species = &self.species[prop.species].over;
        let echoes = species.echoes.over(prop.echo_global.over(prop.echo_spec));
        let drift = self.defaults.shadows == LivingShadows::Drift;
        let prop = &mut self.props[i];
        if prop.echo_config == echoes {
            return;
        }
        prop.echo_config = echoes;
        let stagger = prop.stagger();
        prop.pending = Some(PendingClocks {
            stagger,
            copies: echoes.copies(),
        });
        if prop.ring.is_none() {
            prop.start_pending();
        }
        prop.fit_ring(drift);
    }

    /// One prop's stagger, from its layers.
    fn refresh_stagger(&mut self, i: usize) {
        let prop = &self.props[i];
        let stagger = self.species[prop.species]
            .over
            .stagger
            .or(prop.stagger_global)
            .unwrap_or(prop.stagger_own);
        let drift = self.defaults.shadows == LivingShadows::Drift;
        let prop = &mut self.props[i];
        if prop.stagger() == stagger {
            return;
        }
        let copies = match &prop.pending {
            Some(pending) => pending.copies.clone(),
            None => prop.echoes.iter().map(|e| e.copy).collect(),
        };
        prop.pending = Some(PendingClocks { stagger, copies });
        if prop.ring.is_none() {
            prop.start_pending();
        }
        prop.fit_ring(drift);
    }

    /// One prop's clock, from what's below its species and its species' own.
    fn refresh_clock(&mut self, i: usize) {
        let clock = self.species[self.props[i].species].over.clock;
        let prop = &mut self.props[i];
        prop.wooze = clock.over(prop.wooze_below);
        prop.clock.step = prop.wooze.step;
        prop.clock.glide = prop.wooze.glide;
    }

    /// One lineage's variance, from its layers. A change re-holds its poses.
    fn refresh_variance(&mut self, index: usize) {
        let lineage = &self.lineages[index];
        let spec = lineage
            .spec
            .living
            .as_ref()
            .map_or_else(VarianceConfig::default, |living| living.variance);
        let variance = self.species[lineage.species]
            .over
            .variance
            .over(lineage.variance_global.over(spec));
        let lineage = &mut self.lineages[index];
        if lineage.variance == variance {
            return;
        }
        lineage.variance = variance;
        lineage.generation += 1;
        self.reholding = true;
        self.rehold_cursor = 0;
        self.rehold_pass = 0;
    }

    /// Every value a species can set on one prop, with the layer that set it
    /// (#246). `field` is the wooze field and the moment the props last read it,
    /// in a scene that has one, to name a zone where one has the last word.
    pub fn resolved(&self, id: LivingPropId, field: Option<(&WoozeField, f64)>) -> ResolvedProp {
        let prop = &self.props[id.0];
        let species = &self.species[prop.species];
        let over = species.over;
        let lineage = &self.lineages[prop.lineage];
        let zones = field.map(|(field, time)| field.zones_at(prop.position, time));
        let clock_layer = |knob: usize, set: Option<f32>| {
            if set.is_some() {
                LivingLayer::Species
            } else if let Some(zones) = &zones {
                zones[knob].map_or(LivingLayer::Global, |zone| LivingLayer::Zone(zone.into()))
            } else if prop.clock_live {
                LivingLayer::Global
            } else {
                LivingLayer::Spec
            }
        };
        fn resolved<T>(value: T, layer: LivingLayer) -> Resolved<T> {
            Resolved { value, layer }
        }
        let stagger_layer = if over.stagger.is_some() {
            LivingLayer::Species
        } else if prop.stagger_global.is_some() {
            LivingLayer::Global
        } else if prop.stagger_entity {
            LivingLayer::Entity
        } else {
            LivingLayer::Spec
        };
        let (echoes, global) = (prop.echo_config, prop.echo_global);
        let (variance, global_variance) = (lineage.variance, lineage.variance_global);
        ResolvedProp {
            species: species.name.clone(),
            amplitude: resolved(prop.wooze.amplitude, clock_layer(0, over.clock.amplitude)),
            step: resolved(prop.wooze.step, clock_layer(1, over.clock.step)),
            glide: resolved(prop.wooze.glide, clock_layer(2, over.clock.glide)),
            jitter: resolved(prop.wooze.jitter, clock_layer(3, over.clock.jitter)),
            stagger: resolved(prop.stagger(), stagger_layer),
            echo_count: resolved(
                echoes.count,
                layer_of(over.echoes.count.is_some(), global.count.is_some()),
            ),
            echo_spread: resolved(
                echoes.spread,
                layer_of(over.echoes.spread.is_some(), global.spread.is_some()),
            ),
            echo_alpha: resolved(
                echoes.alpha,
                layer_of(over.echoes.alpha.is_some(), global.alpha.is_some()),
            ),
            variance_amount: resolved(
                variance.amount,
                layer_of(
                    over.variance.amount.is_some(),
                    global_variance.amount.is_some(),
                ),
            ),
            variance_steady: resolved(
                variance.steady,
                layer_of(
                    over.variance.steady.is_some(),
                    global_variance.steady.is_some(),
                ),
            ),
            keep_size: resolved(
                variance.keep_size,
                layer_of(
                    over.variance.keep_size.is_some(),
                    global_variance.keep_size.is_some(),
                ),
            ),
        }
    }

    /// What the first lineage's poses are held to. Every lineage has this once a
    /// knob has set it; until then each has its spec's: see [`Self::variances`].
    pub fn variance(&self) -> Option<VarianceConfig> {
        self.lineages.first().map(|lineage| lineage.variance)
    }

    /// What each lineage's poses are held to, in the order they were first
    /// named.
    pub fn variances(&self) -> Vec<VarianceConfig> {
        self.lineages
            .iter()
            .map(|lineage| lineage.variance)
            .collect()
    }

    /// Whether poses are still being held to a new variance.
    pub fn is_reholding(&self) -> bool {
        self.reholding
    }

    /// The same props on the same clocks, for a second look at them beside these
    /// (#238): the knobs of one can then turn without the other's. Its lineages,
    /// rings and poses are these, and shared, since a pose is an `Arc`. Its pose
    /// cache starts empty: the two take turns with one, through
    /// [`ProcGenResolver::process_living_beside`](crate::ProcGenResolver::process_living_beside),
    /// since a pose doesn't depend on any knob and either side can use the other's.
    pub fn fork(&self) -> Self {
        Self {
            lineages: self.lineages.clone(),
            props: self.props.clone(),
            // Each side keeps its own species values (#246), as it does the rest.
            species: self.species.clone(),
            cache: PoseCache::new(self.cache.max_bytes),
            time: self.time,
            pose_estimate_ms: self.pose_estimate_ms,
            stats: LivingStats::default(),
            defaults: self.defaults.clone(),
            reholding: self.reholding,
            rehold_cursor: self.rehold_cursor,
            rehold_pass: self.rehold_pass,
            rehold_turn: self.rehold_turn,
        }
    }

    /// Lend each other a pose cache: what one side made, the other finds.
    pub fn swap_cache(&mut self, other: &mut LivingProps) {
        std::mem::swap(&mut self.cache, &mut other.cache);
    }

    /// Forget what every bone buffer, echo and tint was last given, so that the
    /// next blend writes all of it: the buffers now hold something else, such as
    /// the other side of an A/B (#238). No clock moves and no cycle starts over, so
    /// what's written is what would have been anyway.
    pub fn redraw_all(&mut self) {
        for prop in &mut self.props {
            prop.redraw();
            // Not cleared: a cleared tint reads as step 0, and a part really on
            // step 0 would then never be sent, keeping the other side's.
            prop.drawn.tints.fill(TINT_UNSENT);
            prop.echoes_changed = true;
        }
    }

    /// Start every part's cycle over at the beat it's on, reading the field as it
    /// is from the next tick: what a part keeps of the glide and drift it started
    /// its cycle with is gone, and it shows the knobs as they are now. For a look
    /// that has to be what its preset says at once, not from each part's next
    /// beat (#238). Where the knobs haven't changed since a cycle began, nothing
    /// it draws changes.
    pub fn restart_cycles(&mut self) {
        for prop in &mut self.props {
            prop.drawn.bones.parts.clear();
            prop.shadow_cycle = Cycle::default();
            for echo in &mut prop.echoes {
                echo.drawn.parts.clear();
            }
            prop.redraw();
        }
    }

    /// Finish what knobs have started, with no budget: every pose held to the
    /// current variance, every waiting clock started and every ring full. For a
    /// look that has to be shown as it is now, not as it'll be in a second.
    pub(crate) fn settle_now(&mut self, registry: &GeneratorRegistry) {
        // Each round can only start clocks and reshape rings toward what they
        // already want, so a few rounds always do; the bound is for safety.
        for _ in 0..16 {
            while self.reholding && self.rehold_one() {}
            while self.fill_one(registry).is_some() {}
            self.start_what_is_ready();
            let waiting = self.props.iter().any(LivingProp::is_waiting);
            let missing = self
                .props
                .iter()
                .any(|p| p.ring.as_ref().is_some_and(|r| r.next_missing().is_some()));
            if !self.reholding && !waiting && !missing {
                break;
            }
        }
        self.pose_estimate_ms = 0.0;
    }

    /// Move every prop's clock on by exactly `beats` of its own steps, however
    /// long those are, and the seconds its jitter reads by `seconds`. For
    /// single-stepping: with `dt` at 0 it's the only thing moving the clock.
    pub fn advance_beats(&mut self, beats: f64, seconds: f64) {
        self.time += seconds;
        for prop in &mut self.props {
            prop.beats += beats;
        }
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
        let species = self.species_index(&request.spec_name);

        let existing = self
            .lineages
            .iter()
            .position(|l| l.asset_name == asset_name);
        let lineage = existing.unwrap_or_else(|| {
            let global = self.defaults.variance;
            self.lineages.push(Lineage {
                spec: Arc::new(spec.clone()),
                seed,
                asset_name: asset_name.clone(),
                identity: ContentHash::from_str(&format!("{identity}\n{seed}")),
                built: None,
                tried: false,
                variance: (self.species[species].over.variance).over(global.over(living.variance)),
                generation: 0,
                species,
                variance_global: global,
            });
            self.lineages.len() - 1
        });

        let over = self.species[species].over;
        let stream = name_hash(&request.entity_name);
        let phase_offset = request
            .phase_offset
            .map_or_else(|| name_phase(stream), |phase| phase.rem_euclid(1.0));
        let mut clock = LivingClock::from(living);
        let stagger_own = request.stagger.unwrap_or(clock.stagger);
        clock.stagger = over
            .stagger
            .or(self.defaults.stagger)
            .unwrap_or(stagger_own);
        let echo_global = self.defaults.echoes;
        let echo_config = over.echoes.over(echo_global.over(living.echoes));
        let wooze_below = WoozeSample {
            amplitude: 1.0,
            step: clock.step,
            glide: clock.glide,
            jitter: 1.0,
        };
        // Its jitter's place is fixed by the spec's step, as it is in a scene with
        // a field, whose step arrives on the first tick.
        let jitter_step = f64::from(clock.step);
        let wooze = over.clock.over(wooze_below);
        clock.step = wooze.step;
        clock.glide = wooze.glide;
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
            jitter_ahead: f64::from(phase_offset) * jitter_step,
            jitter_step,
            beats: f64::from(phase_offset),
            position: request.position,
            wooze,
            shadow_cycle: Cycle::default(),
            drawn: DrawnState::default(),
            echoes: echo_config
                .copies()
                .into_iter()
                .map(|copy| PropEcho {
                    copy,
                    drawn: DrawnBones::default(),
                })
                .collect(),
            echo_config,
            max_depth: 0,
            pending: None,
            echoes_changed: false,
            species,
            wooze_below,
            clock_live: false,
            echo_spec: living.echoes,
            echo_global,
            stagger_own,
            stagger_entity: request.stagger.is_some(),
            stagger_global: self.defaults.stagger,
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
            echo_count: std::mem::take(&mut prop.echoes_changed).then_some(prop.echoes.len()),
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

        // Each part's own beat, and what's new goes up as a material change. A
        // part's first blend always sends its tint, so nothing is assumed of
        // what the renderer holds: after an A/B flip (#238) it's the other side's.
        prop.drawn.tints.resize(rig.leaf_parts.len(), TINT_UNSENT);
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
        let species = &self.species;
        for prop in &mut self.props {
            if let Some((field, time)) = wooze {
                // A species' own values go over the field, zones and all (#246).
                prop.wooze_below = field.sample(prop.position, time);
                prop.wooze = species[prop.species].over.clock.over(prop.wooze_below);
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
        self.start_what_is_ready();

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
                // Only making a pose teaches the estimate anything, so one that came
                // down to just under the budget would be turned away by every frame
                // after it, and the rings would run dry with the budget unspent.
                if !made {
                    self.pose_estimate_ms *= POSE_ESTIMATE_DECAY;
                }
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
        // While a knob is moving, what's being read comes before anything else:
        // poses a waiting clock wants behind the prop can use up a whole budget
        // without helping the prop stay ahead. At rest this is never looked at,
        // and rings fill as they always have.
        let waiting = self.props.iter().any(LivingProp::is_waiting);
        let urgent = if self.reholding || waiting {
            self.pose_needed_soonest()
        } else {
            None
        };
        if self.reholding && urgent.is_none() {
            // What's in use goes first, unless clocks are waiting on poses, which
            // would never start under a knob that keeps moving.
            let goes_first = if self.rehold_pass < REHOLD_PASS_REST && !waiting {
                true
            } else {
                self.rehold_turn = !self.rehold_turn;
                self.rehold_turn
            };
            if goes_first && self.rehold_one() {
                return Some(true);
            }
        }
        let missing = urgent.or_else(|| {
            self.props
                .iter()
                .enumerate()
                .filter_map(|(i, prop)| {
                    let ring = prop.ring.as_ref()?;
                    Some((i, ring.next_missing()?, ring.ready))
                })
                .min_by_key(|&(_, _, ready)| ready)
                .map(|(i, step, _)| (i, step))
        });
        let Some((index, step)) = missing else {
            // Nothing is missing, so the frame's room goes to what's left of a
            // re-hold.
            return (self.reholding && self.rehold_one()).then_some(true);
        };

        let prop = &self.props[index];
        let lineage = &self.lineages[prop.lineage];
        let shape_seed = prop.shape_seed(step);
        let identity = lineage.identity;

        // The cache keeps poses as they grew, so a new variance reaches them too.
        let (full, generated) = if let Some(pose) = self.cache.get(identity, shape_seed) {
            self.stats.poses_from_cache += 1;
            (pose, false)
        } else {
            let made = lineage
                .built
                .as_ref()
                .zip(living_side(registry, &lineage.spec))
                .and_then(|(built, generator)| {
                    generator
                        .full_pose(&lineage.spec, &built.skeleton, shape_seed)
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

        let lineage = &self.lineages[self.props[index].lineage];
        let pose = RingPose {
            held: hold(lineage, &full),
            full,
            generation: lineage.generation,
        };
        self.props[index]
            .ring
            .as_mut()
            .expect("only a prop with a ring has a missing pose")
            .put(step, pose);
        Some(generated)
    }

    /// The pose some prop will run out of first: its place, and the step.
    fn pose_needed_soonest(&self) -> Option<(usize, u64)> {
        let drift = self.defaults.shadows == LivingShadows::Drift;
        self.props
            .iter()
            .enumerate()
            .filter_map(|(i, prop)| {
                let (step, steps_in) = prop.next_needed(drift)?;
                Some((i, step, steps_in))
            })
            .min_by_key(|&(_, _, steps_in)| steps_in)
            .map(|(i, step, _)| (i, step))
    }

    /// Hold the next pose that's at an old variance to the current one, starting
    /// with what's in use. Returns whether there was one; when there are none
    /// left, a re-hold is over.
    fn rehold_one(&mut self) -> bool {
        let drift = self.defaults.shadows == LivingShadows::Drift;
        while self.rehold_pass <= REHOLD_PASS_REST {
            while let Some(prop) = self.props.get_mut(self.rehold_cursor) {
                let (first, count) = prop.rehold_span(self.rehold_pass, drift);
                let lineage = &self.lineages[prop.lineage];
                let stale = prop
                    .ring
                    .as_ref()
                    .and_then(|ring| ring.next_stale(first, count, lineage.generation));
                let (Some(index), Some(ring)) = (stale, &mut prop.ring) else {
                    self.rehold_cursor += 1;
                    continue;
                };
                let slot = ring.slot(index);
                if let Some(pose) = &mut ring.slots[slot] {
                    pose.held = hold(lineage, &pose.full);
                    pose.generation = lineage.generation;
                }
                // Nothing on screen reads what the last pass reaches.
                if self.rehold_pass < REHOLD_PASS_REST {
                    prop.redraw();
                }
                self.stats.poses_reheld += 1;
                return true;
            }
            self.rehold_cursor = 0;
            self.rehold_pass += 1;
        }
        self.reholding = false;
        false
    }

    /// Start any waiting clocks whose poses have all arrived, and shrink each
    /// ring back to what its prop now reads.
    fn start_what_is_ready(&mut self) {
        let drift = self.defaults.shadows == LivingShadows::Drift;
        for prop in &mut self.props {
            let (Some(pending), Some(ring)) = (&prop.pending, &prop.ring) else {
                continue;
            };
            let needs = prop.needs_of(pending.stagger, pending.copies.iter().copied(), drift);
            if ring.holds(needs.first, needs.reads) {
                prop.start_pending();
                prop.fit_ring(drift);
            }
        }
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
                let drift = self.defaults.shadows == LivingShadows::Drift;
                for prop in self.props.iter_mut().filter(|p| p.lineage == index) {
                    prop.max_depth = max_depth;
                    // Every pose a staggered part of it or its echoes can be
                    // using has to fit.
                    let (first, len) = prop.window(drift);
                    prop.ring = Some(PoseRing::new(first, len));
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
            // What its clocks read, which isn't the window's start while the ring
            // reaches back for clocks that are still waiting on it.
            let start = prop.in_use_start();
            starved |= ring.get(start).is_none() || ring.get(start + 1).is_none();
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

/// `full` held to its lineage's variance. A pose of its skeleton can always be
/// held; one that somehow couldn't keeps its full form, rather than a hole.
fn hold(lineage: &Lineage, full: &Arc<LivingPose>) -> Arc<LivingPose> {
    lineage
        .built
        .as_ref()
        .and_then(|built| built.skeleton.with_variance(full, &lineage.variance).ok())
        .map_or_else(|| full.clone(), Arc::new)
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
    let rest_height = skeleton.rest_bounds().max.y;
    Ok(BuiltLineage {
        skeleton: Arc::new(skeleton),
        rig: Arc::new(rig),
        rest_height,
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

    #[test]
    fn a_capture_step_leaves_no_pose_missing_however_far_it_goes() {
        let mut resolver = resolver_with(&sapling(""));
        resolver.add_living_prop(request(1, "first")).unwrap();
        resolver.make_living_now();
        // Far past the whole ring in one step, as a capture may be.
        resolver.step_living_now(30.0, None);
        let props = resolver.living_props();
        let prop = &props.props()[0];
        assert!(prop.ring_len() > 0);
        assert_eq!(prop.ready(), prop.ring_len());
        assert!(prop.pose(prop.first_step().unwrap()).is_some());
        assert_eq!(props.lineage_seed(LivingPropId(0)), 7);
    }

    #[test]
    fn what_was_drawn_is_what_it_reports() {
        let mut resolver = resolver_with(&sapling(""));
        resolver.add_living_prop(request(1, "first")).unwrap();
        resolver.make_living_now();
        resolver.step_living_now(1.0, None);
        assert!(resolver.living_props().props()[0].drawn_times().is_empty());

        let mut frame = BlendFrame::default();
        resolver
            .living_props_mut()
            .blend_prop(LivingPropId(0), &mut frame);
        let prop = &resolver.living_props().props()[0];
        let trunk = prop.drawn_times()[0];
        assert_eq!(trunk.index, prop.beats().floor() as u64);
        assert!(prop.pose(trunk.index).is_some());
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
            let pose = Arc::new(LivingPose {
                shape_seed: seed,
                segments: Vec::new(),
            });
            RingPose {
                full: pose.clone(),
                held: pose,
                generation: 0,
            }
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

    fn plain(seed: u64) -> RingPose {
        let pose = Arc::new(LivingPose {
            shape_seed: seed,
            segments: Vec::new(),
        });
        RingPose {
            full: pose.clone(),
            held: pose,
            generation: 0,
        }
    }

    #[test]
    fn a_ring_can_reach_back_and_keeps_what_both_windows_share() {
        let mut ring = PoseRing::new(10, 3);
        for i in 10..14 {
            ring.put(i, plain(i));
        }
        ring.reshape(8, 6);
        assert_eq!((ring.first, ring.len()), (8, 6));
        assert_eq!(ring.ready, 4, "kept all four");
        assert_eq!(ring.next_missing(), Some(8), "what's behind comes first");
        assert_eq!(ring.get(12).unwrap().shape_seed, 12);
        assert!(!ring.holds(8, 2));
        ring.put(8, plain(8));
        ring.put(9, plain(9));
        assert!(ring.holds(8, 6));

        // And in again, keeping only what's still in the window.
        ring.reshape(11, 2);
        assert_eq!((ring.first, ring.len()), (11, 2));
        assert_eq!(ring.ready, 3);
        assert!(ring.get(10).is_none());
        assert_eq!(ring.get(13).unwrap().shape_seed, 13);
    }

    fn with_echoes(echoes: &str) -> ProcGenResolver {
        resolver_with(&sapling(&format!("echoes = {{ {echoes} }}")))
    }

    /// One prop with its rings full, a little way into its stream so that what's
    /// behind it matters. Its clock is on a fixed step of half a second.
    fn running(echoes: &str, seconds: f64) -> (ProcGenResolver, LivingPropId) {
        let mut resolver = with_echoes(echoes);
        let id = resolver.add_living_prop(request(1, "oak")).unwrap().0;
        resolver.make_living_now();
        run(&mut resolver, seconds, 1.0 / 60.0);
        (resolver, id)
    }

    fn no_pose_missed(resolver: &ProcGenResolver) {
        let stats = resolver.living_props().stats();
        assert_eq!(stats.frames_starved, 0, "{stats:?}");
        assert_eq!(stats.blends_starved, 0, "{stats:?}");
    }

    fn blend_all(
        resolver: &mut ProcGenResolver,
        frame: &mut BlendFrame,
        id: LivingPropId,
    ) -> usize {
        let props = resolver.living_props_mut();
        props.blend_prop(id, frame);
        let mut wrote = 0;
        for echo in 0..props.prop(id).echo_count() {
            wrote += usize::from(props.blend_echo(id, echo, frame).is_some());
        }
        wrote
    }

    /// A prop's bone buffers, its own first, and the tints it sent.
    type Drawn = (Vec<Vec<[[f32; 4]; 4]>>, Vec<(usize, [f32; 3])>);

    /// Everything a prop's buffers would hold now, written whole: its bones, then
    /// each echo's, then its tints.
    fn everything_drawn(props: &mut LivingProps, id: LivingPropId) -> Drawn {
        props.redraw_all();
        let mut frame = BlendFrame::default();
        let blend = props.blend_prop(id, &mut frame);
        assert_eq!(
            blend.first_bone,
            Some(0),
            "a redraw writes the whole palette"
        );
        assert_eq!(blend.echo_count, Some(props.prop(id).echo_count()));
        let tints = frame.tints.clone();
        let mut bones = vec![frame.bones.clone()];
        for echo in 0..props.prop(id).echo_count() {
            assert_eq!(props.blend_echo(id, echo, &mut frame), Some(0));
            bones.push(frame.bones.clone());
        }
        (bones, tints)
    }

    #[test]
    fn a_redraw_writes_everything_and_moves_nothing() {
        let (mut resolver, id) = running("count = 3", 5.3);
        let props = resolver.living_props_mut();
        let mut frame = BlendFrame::default();
        props.blend_prop(id, &mut frame);
        // Nothing has moved, so nothing would be written.
        assert_eq!(props.blend_prop(id, &mut frame).first_bone, None);
        let once = everything_drawn(props, id);
        assert_eq!(everything_drawn(props, id), once);
    }

    #[test]
    fn a_fork_on_the_same_knobs_draws_the_same_bones_and_finds_its_poses_made() {
        let (mut resolver, id) = running("count = 3", 4.0);
        let mut side = resolver.living_props().fork();
        for _ in 0..240 {
            resolver.process_living(1.0 / 60.0);
            resolver.process_living_beside(&mut side, 1.0 / 60.0, None);
        }
        assert_eq!(
            side.prop(id).beats(),
            resolver.living_props().prop(id).beats()
        );
        let ours = everything_drawn(resolver.living_props_mut(), id);
        assert_eq!(everything_drawn(&mut side, id), ours);
        let stats = side.stats();
        assert_eq!(stats.poses_made, 0, "the scene made them first: {stats:?}");
        assert!(stats.poses_from_cache > 0, "{stats:?}");
        assert_eq!(stats.frames_starved, 0, "{stats:?}");
    }

    #[test]
    fn a_fork_keeps_the_species_values_and_turns_its_own() {
        let (resolver, id) = running("count = 2, spread = 0.2", 1.0);
        let mut props = resolver.living_props().fork();
        let species = props.species_of(id).to_string();
        let slow = SpeciesOverride {
            clock: ClockOverride {
                step: Some(1.5),
                ..ClockOverride::default()
            },
            ..SpeciesOverride::default()
        };
        assert!(props.set_species(&species, slow));
        let side = props.fork();
        assert_eq!(
            side.resolved(id, None),
            props.resolved(id, None),
            "carried over"
        );

        let mut side = side;
        let quick = SpeciesOverride {
            clock: ClockOverride {
                step: Some(0.25),
                ..ClockOverride::default()
            },
            ..SpeciesOverride::default()
        };
        side.set_species(&species, quick);
        assert_ne!(side.resolved(id, None), props.resolved(id, None));
        assert_eq!(
            props.resolved(id, None),
            props.fork().resolved(id, None),
            "left alone"
        );
    }

    #[test]
    fn a_fork_turns_its_own_knobs_and_settles_them_at_once() {
        let (mut resolver, id) = running("count = 2, spread = 0.2", 6.0);
        let before = everything_drawn(resolver.living_props_mut(), id);
        let mut side = resolver.living_props().fork();
        side.set_echoes(EchoConfig {
            count: 6,
            spread: 1.2,
            alpha: 0.3,
        });
        side.set_stagger(Stagger::PerBranch);
        side.set_variance(VarianceConfig {
            amount: 1.0,
            steady: 0.0,
            keep_size: false,
        });
        assert!(side.prop(id).is_waiting() && side.is_reholding());
        resolver.settle_living_now(Some(&mut side));
        let prop = side.prop(id);
        assert!(!prop.is_waiting() && !side.is_reholding());
        assert_eq!(prop.echo_count(), 6);
        assert_eq!(prop.ready(), prop.ring_len(), "full");

        // Settled is still: ticks that move no clock change nothing it draws.
        let settled = everything_drawn(&mut side, id);
        for _ in 0..30 {
            resolver.process_living_beside(&mut side, 0.0, None);
        }
        assert_eq!(everything_drawn(&mut side, id), settled);
        // And the scene's own look never heard of it.
        assert_eq!(everything_drawn(resolver.living_props_mut(), id), before);
        assert_eq!(resolver.living_props().prop(id).echo_count(), 2);
    }

    #[test]
    fn a_wider_spread_waits_until_the_ring_has_what_it_reads() {
        let (mut resolver, id) = running("count = 2, spread = 0.2", 8.0);
        let props = resolver.living_props_mut();
        let first_before = props.prop(id).first_step().unwrap();
        let in_use: Vec<EchoCopy> = (0..2).map(|i| props.prop(id).echo(i)).collect();

        props.set_echoes(EchoConfig {
            count: 4,
            spread: WIDE,
            alpha: 0.2,
        });
        let prop = props.prop(id);
        assert!(prop.is_waiting());
        assert_eq!(prop.echo_config().count, 4, "a knob reads as turned");
        assert_eq!(prop.echo_count(), 2, "and the old echoes keep drawing");
        assert_eq!(prop.echo(0), in_use[0]);
        assert!(
            prop.first_step().unwrap() < first_before,
            "its window has reached back for them"
        );

        // A frame with no budget can't make them, so nothing has changed.
        let mut frame = BlendFrame::default();
        resolver.living_props_mut().stats = LivingStats::default();
        resolver.frame_budget_for_test(0.0);
        resolver.process_living(1.0 / 60.0);
        blend_all(&mut resolver, &mut frame, id);
        assert!(resolver.living_props().prop(id).is_waiting());
        assert_eq!(resolver.living_props().prop(id).echo_count(), 2);
        no_pose_missed(&resolver);

        // Given the room, the ring fills and they start, all at once.
        resolver.frame_budget_for_test(100.0);
        resolver.process_living(1.0 / 60.0);
        blend_all(&mut resolver, &mut frame, id);
        let prop = resolver.living_props().prop(id);
        assert!(!prop.is_waiting());
        assert_eq!(prop.echo_count(), 4);
        assert_eq!(prop.echo(0).offset, -WIDE);
        no_pose_missed(&resolver);
    }

    /// Wide enough to reach well behind, and short of where an echo fades out.
    const WIDE: f32 = 1.6;

    /// The whole range of both knobs, back and forth, at a budget tight enough to
    /// matter and with blending the whole way, as a hand on a slider would. The
    /// ring has to keep up, or hold the old values: no pose is ever missing.
    #[test]
    fn sweeping_count_and_spread_across_their_range_misses_no_pose() {
        let mut resolver = resolver_with(&sapling(
            "stagger = \"per_branch\"\nechoes = { count = 6, spread = 0.9 }",
        ));
        let mut ids = Vec::new();
        for n in 0..6u64 {
            let name = format!("oak_{n}");
            ids.push(resolver.add_living_prop(request(n + 1, &name)).unwrap().0);
        }
        resolver.make_living_now();
        // A pose a frame: well under what a real budget makes, so it has to wait.
        resolver.frame_budget_for_test(1e-6);
        let mut frame = BlendFrame::default();
        let dt = 1.0 / 60.0;
        let mut most_echoes = 0;
        let mut waits = 0;
        for f in 0..1800 {
            let t = f64::from(f) / 1800.0;
            // A triangle each, out of step, so every pairing of the two gets a turn.
            let tri = |x: f64| 1.0 - (2.0 * x.fract() - 1.0).abs();
            let echoes = EchoConfig {
                count: (tri(t * 3.0) * f64::from(crate::spec::MAX_ECHOES)).round() as u32,
                spread: (tri(t * 7.0 + 0.3) * f64::from(crate::spec::MAX_ECHO_SPREAD)) as f32,
                alpha: 0.2,
            };
            resolver.living_props_mut().set_echoes(echoes);
            resolver.process_living(dt);
            for id in &ids {
                blend_all(&mut resolver, &mut frame, *id);
                let prop = resolver.living_props().prop(*id);
                most_echoes = most_echoes.max(prop.echo_count());
                waits += usize::from(prop.is_waiting());
                assert!(prop.ring_len() >= 3);
            }
        }
        no_pose_missed(&resolver);
        assert!(most_echoes >= 14, "the sweep got there: {most_echoes}");
        assert!(waits > 0, "and some of it had to wait for poses");

        // At rest it settles at what it was last told, with a ring its own size.
        let echoes = EchoConfig {
            count: 0,
            spread: 0.0,
            alpha: 0.2,
        };
        resolver.living_props_mut().set_echoes(echoes);
        resolver.frame_budget_for_test(100.0);
        resolver.process_living(dt);
        for id in &ids {
            let prop = resolver.living_props().prop(*id);
            assert!(!prop.is_waiting());
            assert_eq!(prop.echo_count(), 0);
            assert_eq!(prop.ring_len(), 4, "back to pose_ring");
        }
    }

    #[test]
    fn a_change_of_alpha_keeps_the_echoes_where_they_are() {
        let (mut resolver, id) = running("count = 2, spread = 0.5, alpha = 0.2", 2.0);
        let mut frame = BlendFrame::default();
        blend_all(&mut resolver, &mut frame, id);
        let offset = resolver.living_props().prop(id).echo(0).offset;
        let opacity = resolver.living_props().prop(id).echo(0).opacity;
        let (first, len) = {
            let prop = resolver.living_props().prop(id);
            (prop.first_step(), prop.ring_len())
        };

        resolver.living_props_mut().set_echoes(EchoConfig {
            count: 2,
            spread: 0.5,
            alpha: 0.4,
        });
        resolver.process_living(0.0);
        let prop = resolver.living_props().prop(id);
        assert!(!prop.is_waiting(), "it needs no new pose");
        assert_eq!(prop.echo(0).offset, offset);
        assert!((prop.echo(0).opacity - opacity * 2.0).abs() < 1e-5);
        assert_eq!((prop.first_step(), prop.ring_len()), (first, len));

        // A clock that's stopped still shows it: an opacity reaches the renderer
        // with a write, so both echoes have one to send.
        assert_eq!(blend_all(&mut resolver, &mut frame, id), 2);
    }

    #[test]
    fn dropping_echoes_says_how_many_are_left_once() {
        let (mut resolver, id) = running("count = 4", 1.0);
        let mut frame = BlendFrame::default();
        resolver.living_props_mut().blend_prop(id, &mut frame);
        resolver.living_props_mut().set_echoes(EchoConfig {
            count: 1,
            ..EchoConfig::default()
        });
        resolver.process_living(0.0);
        let blend = resolver.living_props_mut().blend_prop(id, &mut frame);
        assert_eq!(blend.echo_count, Some(1));
        let blend = resolver.living_props_mut().blend_prop(id, &mut frame);
        assert_eq!(blend.echo_count, None);
    }

    #[test]
    fn a_new_stagger_grows_the_ring_first_and_starts_in_one_piece() {
        let mut resolver = resolver_with(&sapling("stagger = \"none\""));
        let mut small = request(1, "oak");
        small.pose_ring = Some(2);
        let id = resolver.add_living_prop(small).unwrap().0;
        resolver.make_living_now();
        run(&mut resolver, 4.0, 1.0 / 60.0);
        assert_eq!(resolver.living_props().prop(id).ring_len(), 2);

        resolver.living_props_mut().set_stagger(Stagger::PerBranch);
        let prop = resolver.living_props().prop(id);
        assert_eq!(prop.stagger(), Stagger::PerBranch);
        assert_eq!(prop.clock().stagger, Stagger::None, "not until it's ready");

        let mut frame = BlendFrame::default();
        resolver.process_living(1.0 / 60.0);
        blend_all(&mut resolver, &mut frame, id);
        let prop = resolver.living_props().prop(id);
        assert_eq!(prop.clock().stagger, Stagger::PerBranch);
        assert_eq!(prop.ring_len(), 3, "per-branch reaches further");
        no_pose_missed(&resolver);
    }

    fn held_is_what_variance_says(
        resolver: &ProcGenResolver,
        id: LivingPropId,
        want: VarianceConfig,
    ) {
        let props = resolver.living_props();
        let prop = props.prop(id);
        let (skeleton, _) = props.rig_of(id).unwrap();
        let first = prop.first_step().unwrap();
        for index in first..first + prop.ring_len() as u64 {
            let full = prop.full_pose(index).unwrap();
            let held = prop.pose(index).unwrap();
            assert_eq!(
                **held,
                skeleton.with_variance(full, &want).unwrap(),
                "{index}"
            );
        }
    }

    /// A pose is as its seed grew it, in the ring and the cache alike, so a new
    /// variance reaches all of them and nothing has to grow again.
    #[test]
    fn a_new_variance_reaches_every_pose_in_the_ring_and_regrows_none() {
        let (mut resolver, id) = running("count = 2", 3.0);
        let start = resolver.living_props().stats();
        let want = VarianceConfig {
            amount: 0.9,
            steady: 0.1,
            keep_size: true,
        };
        resolver.living_props_mut().set_variance(want);
        assert!(resolver.living_props().is_reholding());
        resolver.process_living(0.0);
        assert!(!resolver.living_props().is_reholding());
        held_is_what_variance_says(&resolver, id, want);
        let stats = resolver.living_props().stats();
        assert_eq!(stats.poses_made, start.poses_made, "no regrowing");
        assert!(stats.poses_reheld >= 5, "{stats:?}");

        // What's made from here on starts at it, whichever way it comes: from the
        // generator, and from the cache after it was left behind.
        run(&mut resolver, 2.0, 1.0 / 60.0);
        held_is_what_variance_says(&resolver, id, want);
        let back = resolver.living_props().prop(id).first_step().unwrap();
        resolver.living_props_mut().set_echoes(EchoConfig {
            count: 2,
            spread: 1.8,
            alpha: 0.2,
        });
        resolver.process_living(0.0);
        let stats = resolver.living_props().stats();
        assert!(
            resolver.living_props().prop(id).first_step().unwrap() < back,
            "it reached back into the cache"
        );
        assert!(stats.poses_from_cache > 0, "{stats:?}");
        held_is_what_variance_says(&resolver, id, want);
    }

    #[test]
    fn unheld_variance_is_each_pose_exactly_as_it_grew() {
        let (mut resolver, id) = running("count = 0", 2.0);
        resolver
            .living_props_mut()
            .set_variance(VarianceConfig::FULL);
        resolver.process_living(0.0);
        let props = resolver.living_props();
        let prop = props.prop(id);
        let (skeleton, _) = props.rig_of(id).unwrap();
        let spec = ProcGenSpec::parse(&sapling("")).unwrap();
        let tree = crate::generators::tree::TreeGenerator;
        let first = prop.first_step().unwrap();
        for index in first..first + prop.ring_len() as u64 {
            let grew = LivingGenerator::full_pose(&tree, &spec, skeleton, prop.shape_seed(index));
            assert_eq!(**prop.pose(index).unwrap(), grew.unwrap(), "{index}");
        }
    }

    /// Re-holding comes out of the frame's budget, a share at a time, and a prop
    /// holding still draws what it gets, without its clock having moved.
    #[test]
    fn a_re_hold_spreads_over_frames_and_redraws_a_stopped_prop() {
        let mut resolver = resolver_with(&sapling("echoes = { count = 2 }"));
        let mut ids = Vec::new();
        for n in 0..8u64 {
            let name = format!("oak_{n}");
            ids.push(resolver.add_living_prop(request(n + 1, &name)).unwrap().0);
        }
        resolver.make_living_now();
        resolver.process_living(0.0);
        let mut frame = BlendFrame::default();
        for id in &ids {
            blend_all(&mut resolver, &mut frame, *id);
            assert_eq!(
                blend_all(&mut resolver, &mut frame, *id),
                0,
                "holding still is free"
            );
        }

        resolver.living_props_mut().set_variance(VarianceConfig {
            amount: 0.9,
            steady: 0.0,
            keep_size: true,
        });
        // No room at all: it waits its turn and can't lose anything.
        resolver.frame_budget_for_test(0.0);
        resolver.process_living(0.0);
        assert!(resolver.living_props().is_reholding());
        assert_eq!(resolver.living_props().stats().poses_reheld, 0);

        // A frame's worth of room for one is the least it gets: it still finishes.
        resolver.frame_budget_for_test(1e-6);
        let mut frames = 0;
        while resolver.living_props().is_reholding() {
            resolver.process_living(0.0);
            frames += 1;
            assert!(frames < 1000);
        }
        assert!(frames > 1, "took {frames} frames");
        for id in &ids {
            assert!(
                blend_all(&mut resolver, &mut frame, *id) > 0,
                "the new poses go up though no clock has moved"
            );
        }
        no_pose_missed(&resolver);
    }

    /// A re-hold costs what a pose does, so a whole grove's rings take seconds.
    /// What the props are drawing has to go first, on every prop, before any ring
    /// gets finished.
    #[test]
    fn a_re_hold_reaches_what_every_prop_is_drawing_first() {
        let mut resolver = resolver_with(&sapling("echoes = { count = 0 }"));
        let mut ids = Vec::new();
        for n in 0..6u64 {
            let name = format!("oak_{n}");
            ids.push(resolver.add_living_prop(request(n + 1, &name)).unwrap().0);
        }
        resolver.make_living_now();
        run(&mut resolver, 1.0, 1.0 / 60.0);

        let want = VarianceConfig {
            amount: 0.9,
            steady: 0.1,
            keep_size: true,
        };
        resolver.living_props_mut().set_variance(want);
        let is_current = |resolver: &ProcGenResolver, id: LivingPropId, index: u64| {
            let props = resolver.living_props();
            let prop = props.prop(id);
            let (skeleton, _) = props.rig_of(id).unwrap();
            let full = prop.full_pose(index).unwrap();
            **prop.pose(index).unwrap() == skeleton.with_variance(full, &want).unwrap()
        };
        // A pose a frame, and enough frames for what's in use and little more.
        resolver.frame_budget_for_test(1e-6);
        let in_use = |resolver: &ProcGenResolver, id: LivingPropId| {
            let prop = resolver.living_props().prop(id);
            let first = prop.beats().floor() as u64;
            first..first + prop.clock().reach(2) + 1
        };
        let mut frames = 0;
        while !ids
            .iter()
            .all(|id| in_use(&resolver, *id).all(|index| is_current(&resolver, *id, index)))
        {
            resolver.process_living(0.0);
            frames += 1;
            assert!(frames < 200);
        }
        assert!(
            resolver.living_props().is_reholding(),
            "it got here with some of the rings still to do"
        );
        let last = *ids.last().unwrap();
        let prop = resolver.living_props().prop(last);
        let ring_end = prop.first_step().unwrap() + prop.ring_len() as u64;
        assert!(
            !is_current(&resolver, last, ring_end - 1),
            "and hadn't gone on to what nothing is reading yet"
        );
    }

    /// A ring about to run out is more urgent than any re-hold.
    #[test]
    fn a_ring_about_to_run_out_comes_before_a_re_hold() {
        let (mut resolver, id) = running("count = 0", 1.0);
        let props = resolver.living_props_mut();
        props.props[id.0].ring.as_mut().unwrap().slide_to(2);
        let first = props.prop(id).beats().floor() as u64;
        let ring = props.props[id.0].ring.as_mut().unwrap();
        let (slot, was) = (ring.slot(first + 1), first + 1);
        if ring.slots[slot].take().is_some() {
            ring.ready -= 1;
        }
        assert_eq!(
            props.prop(id).next_needed(false).map(|(step, _)| step),
            Some(was)
        );
        props.set_variance(VarianceConfig {
            amount: 0.9,
            ..VarianceConfig::default()
        });
        resolver.frame_budget_for_test(1e-6);
        resolver.process_living(0.0);
        let prop = resolver.living_props().prop(id);
        assert!(prop.pose(was).is_some(), "the pose came first");
        assert_eq!(resolver.living_props().stats().pose_failures, 0);
    }

    /// All of it at once with a pose a frame to do it in: six props at 0.12 s a step
    /// want 50 poses a second out of 60, under a stagger that reaches far, a new
    /// variance every frame, and echoes that want poses behind the props. There's
    /// only room if what's being read wins every time.
    /// A stall on a busy machine leaves the estimate over the budget, and it comes
    /// down a little with each pose until it's just under. Exactly at the budget,
    /// no frame can ever fit a pose, since finding that out takes some of the frame.
    /// The rings ran dry like this at 0.25 ms of a 4 ms budget, for as long as the
    /// game went on, at a steady 60 fps (#219). It takes a grove's worth of props:
    /// with a few, stepping their clocks fits inside the timer's resolution, the
    /// frame reads as having used nothing, and an estimate at the budget still fits.
    #[test]
    fn an_estimate_just_under_the_budget_does_not_stop_the_poses() {
        let mut resolver = resolver_with(&sapling("step = 0.1"));
        for n in 0..256u64 {
            let name = format!("oak_{n}");
            resolver.add_living_prop(request(n + 1, &name)).unwrap();
        }
        resolver.make_living_now();
        resolver.frame_budget_for_test(4.0);
        let made_before = resolver.living_props().stats().poses_made;
        resolver.living_props_mut().pose_estimate_ms = 4.0;

        run(&mut resolver, 4.0, 1.0 / 60.0);

        let props = resolver.living_props();
        assert!(
            props.stats().poses_made > made_before,
            "no poses in 4 s: {:?}",
            props.stats()
        );
        for prop in props.props() {
            assert_eq!(prop.ready(), prop.ring_len(), "{:?}", props.stats());
        }
    }

    #[test]
    fn a_pose_being_read_beats_every_other_use_of_a_tight_budget() {
        let mut resolver = resolver_with(&sapling(
            "step = 0.12\nstagger = \"per_branch\"\nechoes = { count = 6, spread = 0.9 }",
        ));
        let mut ids = Vec::new();
        for n in 0..6u64 {
            let name = format!("oak_{n}");
            ids.push(resolver.add_living_prop(request(n + 1, &name)).unwrap().0);
        }
        resolver.make_living_now();
        resolver.frame_budget_for_test(1e-6);
        let mut frame = BlendFrame::default();
        let tri = |x: f64| 1.0 - (2.0 * x.fract() - 1.0).abs();
        for f in 0..1800 {
            let t = f64::from(f) / 1800.0;
            let props = resolver.living_props_mut();
            props.set_echoes(EchoConfig {
                count: (tri(t * 5.0) * 16.0).round() as u32,
                spread: (tri(t * 9.0 + 0.3) * 1.8) as f32,
                alpha: 0.2,
            });
            props.set_variance(VarianceConfig {
                amount: tri(t * 7.0) as f32,
                steady: tri(t * 3.0) as f32,
                keep_size: f % 200 < 100,
            });
            resolver.process_living(1.0 / 60.0);
            for id in &ids {
                blend_all(&mut resolver, &mut frame, *id);
            }
        }
        no_pose_missed(&resolver);
    }

    #[test]
    fn advancing_by_beats_moves_every_clock_one_whole_step() {
        let (mut resolver, id) = running("count = 2", 1.0);
        let before = resolver.living_props().prop(id).beats();
        let time = resolver.living_props().time();
        resolver.living_props_mut().advance_beats(1.0, 0.5);
        assert_eq!(resolver.living_props().prop(id).beats() - before, 1.0);
        assert_eq!(resolver.living_props().time() - time, 0.5);
        resolver.process_living(0.0);
        let prop = resolver.living_props().prop(id);
        assert!(prop.pose(prop.first_step().unwrap()).is_some(), "ready");
        no_pose_missed(&resolver);
    }

    #[test]
    fn the_scene_can_say_stagger_and_variance_over_the_spec() {
        let mut resolver = resolver_with(&sapling("stagger = \"none\""));
        resolver.set_living_defaults(LivingDefaults {
            stagger: Some(Stagger::PerBranch),
            variance: VarianceOverride {
                amount: Some(1.0),
                steady: None,
                keep_size: Some(false),
            },
            ..Default::default()
        });
        let mut own = request(1, "oak");
        own.stagger = Some(Stagger::None);
        let id = resolver.add_living_prop(own).unwrap().0;
        resolver.make_living_now();
        let props = resolver.living_props();
        assert_eq!(props.prop(id).clock().stagger, Stagger::PerBranch);
        let variance = props.variance().unwrap();
        assert_eq!(variance.amount, 1.0);
        assert_eq!(variance.steady, 0.5, "from the spec");
        assert!(!variance.keep_size);
        held_is_what_variance_says(&resolver, id, variance);
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

    #[test]
    fn a_redraw_sends_every_leaf_tint_even_one_on_step_zero() {
        let spec = std::fs::read_to_string(OAK).unwrap();
        let mut resolver = resolver_with(&spec);
        let mut req = request(1, "oak");
        req.spec_name = "oak_living".into();
        let id = resolver.add_living_prop(req).unwrap().0;
        resolver.make_living_now();
        // With no jitter every part sits on step 0, the value a cleared memory of
        // what was sent would read as.
        let still = WoozeField::new(WoozeSample {
            jitter: 0.0,
            ..WoozeSample::NEUTRAL
        });
        let mut frame = BlendFrame::default();
        resolver.process_living_in(1.0 / 60.0, Some((&still, 0.0)));
        let sent = |resolver: &mut ProcGenResolver, frame: &mut BlendFrame| {
            resolver.living_props_mut().blend_prop(id, frame);
            frame.tints.len()
        };
        let leaves = resolver
            .living_props()
            .rig_of(id)
            .unwrap()
            .1
            .leaf_parts
            .len();
        assert!(leaves > 0);
        sent(&mut resolver, &mut frame);
        assert_eq!(sent(&mut resolver, &mut frame), 0, "nothing new to send");
        resolver.living_props_mut().redraw_all();
        assert_eq!(sent(&mut resolver, &mut frame), leaves, "all of them again");
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
                // A first blend sends every part's tint, the identity included.
                for (_, tint) in frame.tints.iter().filter(|(_, tint)| *tint != [1.0; 3]) {
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

    // ---- Species (#246) ----

    /// The sapling under another name, and nothing else changed.
    fn renamed(spec: &str, name: &str) -> String {
        spec.replace("name = \"sapling\"", &format!("name = \"{name}\""))
    }

    /// Two saplings and two thorns, with the scene's species values, built.
    fn grove(defaults: LivingDefaults) -> ProcGenResolver {
        let spec = sapling("echoes = { count = 1, spread = 0.5 }");
        let mut resolver = resolver_with(&spec);
        resolver.index_spec(ProcGenSpec::parse(&renamed(&spec, "thorn")).unwrap());
        resolver.set_living_defaults(defaults);
        for (i, species) in ["sapling", "sapling", "thorn", "thorn"].iter().enumerate() {
            let mut asked = request(i as u64 + 1, &format!("{species}_{i}"));
            asked.spec_name = species.to_string();
            asked.position = [i as f32 * 10.0, 0.0, 0.0];
            resolver.add_living_prop(asked).unwrap();
        }
        resolver.make_living_now();
        resolver
    }

    fn thorny() -> LivingDefaults {
        let mut defaults = LivingDefaults::default();
        defaults.species.insert(
            "thorn".to_string(),
            SpeciesOverride {
                clock: ClockOverride {
                    step: Some(0.25),
                    ..Default::default()
                },
                stagger: Some(Stagger::None),
                echoes: EchoOverride {
                    count: Some(3),
                    ..Default::default()
                },
                variance: VarianceOverride {
                    amount: Some(0.9),
                    ..Default::default()
                },
            },
        );
        defaults
    }

    #[test]
    fn a_species_value_beats_the_field_and_its_zones_and_no_other_species_sees_it() {
        let mut resolver = grove(thorny());
        // A zone over all four that slows the step, and says glide too.
        let field = field(
            "glide = 0.5",
            &[r#"name = "slow"
                shape = "sphere"
                center = [15, 0, 0]
                radius = 40
                step = 1.0"#],
        );
        tick_with(&mut resolver, &field, 0.0, 1.0 / 60.0);
        let props = resolver.living_props();
        let at = |i: usize| props.resolved(LivingPropId(i), Some((&field, 0.0)));

        let sapling = at(0);
        assert_eq!(sapling.species, "sapling");
        assert_eq!(sapling.step.value, 1.0);
        assert_eq!(sapling.step.layer, LivingLayer::Zone("slow".into()));
        assert_eq!(sapling.glide.layer, LivingLayer::Global);
        assert_eq!(sapling.echo_count.value, 1);
        assert_eq!(sapling.echo_count.layer, LivingLayer::Spec);
        assert_eq!(sapling.stagger.value, Stagger::PerLevel);
        assert_eq!(sapling.variance_amount.value, 0.5);

        let thorn = at(2);
        assert_eq!(thorn.step.value, 0.25, "the species beats the zone");
        assert_eq!(thorn.step.layer, LivingLayer::Species);
        assert_eq!(thorn.glide.value, 0.5, "and leaves the rest to the field");
        assert_eq!(props.prop(LivingPropId(2)).clock().step, 0.25);
        assert_eq!((thorn.echo_count.value, thorn.echo_spread.value), (3, 0.5));
        assert_eq!(thorn.echo_count.layer, LivingLayer::Species);
        assert_eq!(thorn.echo_spread.layer, LivingLayer::Spec);
        assert_eq!(thorn.stagger.value, Stagger::None);
        assert_eq!(thorn.variance_amount.value, 0.9);
        assert_eq!(thorn.variance_steady.layer, LivingLayer::Spec);
        assert_eq!(props.prop(LivingPropId(3)).echo_count(), 3);
        assert_eq!(
            props.variances(),
            vec![
                VarianceConfig {
                    amount: 0.5,
                    steady: 0.5,
                    keep_size: false
                },
                VarianceConfig {
                    amount: 0.9,
                    steady: 0.5,
                    keep_size: false
                },
            ]
        );
    }

    #[test]
    fn a_global_knob_moved_live_leaves_explicit_species_values_standing() {
        let mut resolver = grove(thorny());
        let mut field = field("", &[]);
        tick_with(&mut resolver, &field, 0.0, 1.0 / 60.0);

        // Every global knob a person can move, moved.
        field
            .set_global(&"step = 2.0\nglide = 0.9".parse().unwrap(), 0.0, 0.0)
            .unwrap();
        let props = resolver.living_props_mut();
        props.set_echoes(EchoConfig {
            count: 6,
            spread: 1.0,
            alpha: 0.4,
        });
        props.set_stagger(Stagger::PerBranch);
        props.set_variance(VarianceConfig {
            amount: 0.2,
            steady: 0.1,
            keep_size: true,
        });
        tick_with(&mut resolver, &field, 0.0, 1.0 / 60.0);

        let props = resolver.living_props();
        let sapling = props.resolved(LivingPropId(0), Some((&field, 0.0)));
        assert_eq!((sapling.step.value, sapling.glide.value), (2.0, 0.9));
        assert_eq!(sapling.echo_count.value, 6);
        assert_eq!(sapling.echo_count.layer, LivingLayer::Global);
        assert_eq!(sapling.stagger.value, Stagger::PerBranch);
        assert_eq!(sapling.variance_amount.value, 0.2);

        let thorn = props.resolved(LivingPropId(2), Some((&field, 0.0)));
        assert_eq!(thorn.step.value, 0.25, "the species' step stood");
        assert_eq!(thorn.glide.value, 0.9, "what it doesn't set follows");
        assert_eq!(thorn.echo_count.value, 3);
        assert_eq!(thorn.echo_spread.value, 1.0);
        assert_eq!(thorn.echo_spread.layer, LivingLayer::Global);
        assert_eq!(thorn.stagger.value, Stagger::None);
        assert_eq!(thorn.variance_amount.value, 0.9);
        assert_eq!(thorn.variance_steady.value, 0.1);

        // Cleared, the species takes what the grove has.
        let props = resolver.living_props_mut();
        assert!(props.set_species("thorn", SpeciesOverride::default()));
        tick_with(&mut resolver, &field, 0.0, 1.0 / 60.0);
        let thorn = resolver
            .living_props()
            .resolved(LivingPropId(2), Some((&field, 0.0)));
        assert_eq!(thorn.step.value, 2.0);
        assert_eq!(thorn.echo_count.value, 6);
        assert_eq!(thorn.stagger.value, Stagger::PerBranch);
        assert_eq!(thorn.variance_amount.value, 0.2);
    }

    #[test]
    fn a_species_scope_reaches_its_own_props_and_lineages_only() {
        let mut resolver = grove(LivingDefaults::default());
        let props = resolver.living_props_mut();
        let echoes = EchoOverride {
            alpha: Some(0.05),
            ..Default::default()
        };
        assert!(props.tune_echoes(LivingScope::Species("thorn"), echoes));
        assert!(props.tune_stagger(LivingScope::Species("thorn"), Stagger::PerBranch));
        let variance = VarianceOverride {
            keep_size: Some(true),
            ..Default::default()
        };
        assert!(props.tune_variance(LivingScope::Species("thorn"), variance));
        assert!(props.tune_clock(
            "thorn",
            ClockOverride {
                glide: Some(0.0),
                ..Default::default()
            }
        ));
        let alphas: Vec<f32> = props
            .props()
            .iter()
            .map(|p| p.echo_config().alpha)
            .collect();
        let spec_alpha = EchoConfig::default().alpha;
        assert_eq!(alphas, [spec_alpha, spec_alpha, 0.05, 0.05]);
        let staggers: Vec<Stagger> = props.props().iter().map(|p| p.stagger()).collect();
        assert_eq!(staggers[..2], [Stagger::PerLevel; 2]);
        assert_eq!(staggers[2..], [Stagger::PerBranch; 2]);
        let keeps: Vec<bool> = props.variances().iter().map(|v| v.keep_size).collect();
        assert_eq!(keeps, [false, true]);
        // With no field, a species' clock goes over its spec's at once.
        let glides: Vec<f32> = props.props().iter().map(|p| p.clock().glide).collect();
        assert_eq!(glides, [0.3, 0.3, 0.0, 0.0]);

        // A species with no props keeps what it's given, for none of them.
        assert!(!props.tune_echoes(LivingScope::Species("birch"), echoes));
        assert_eq!(props.species_override("birch").unwrap().echoes, echoes);
        assert!(!props.has_species("birch"));
        assert!(props.has_species("thorn"));
    }

    #[test]
    fn with_no_species_values_every_layer_is_what_it_was() {
        let resolver = grove(LivingDefaults::default());
        let props = resolver.living_props();
        for i in 0..4 {
            let at = props.resolved(LivingPropId(i), None);
            for layer in [
                &at.amplitude.layer,
                &at.step.layer,
                &at.glide.layer,
                &at.jitter.layer,
                &at.stagger.layer,
                &at.echo_count.layer,
                &at.variance_amount.layer,
                &at.keep_size.layer,
            ] {
                assert_eq!(*layer, LivingLayer::Spec);
            }
            assert_eq!(
                props.prop(LivingPropId(i)).wooze(),
                props.prop(LivingPropId(i)).wooze_below()
            );
        }
    }

    #[test]
    fn an_entity_stagger_is_its_own_layer_under_the_global_one() {
        let spec = sapling("");
        let mut resolver = resolver_with(&spec);
        let mut asked = request(1, "own");
        asked.stagger = Some(Stagger::None);
        let id = resolver.add_living_prop(asked).unwrap().0;
        resolver.make_living_now();
        let props = resolver.living_props_mut();
        assert_eq!(props.resolved(id, None).stagger.layer, LivingLayer::Entity);
        props.set_stagger(Stagger::PerBranch);
        assert_eq!(props.resolved(id, None).stagger.layer, LivingLayer::Global);
        props.tune_stagger(LivingScope::Species("sapling"), Stagger::PerLevel);
        let stagger = props.resolved(id, None).stagger;
        assert_eq!(
            (stagger.value, stagger.layer),
            (Stagger::PerLevel, LivingLayer::Species)
        );
    }

    #[test]
    fn a_clock_given_back_live_keeps_the_species_step_over_it() {
        let mut resolver = grove(thorny());
        let props = resolver.living_props_mut();
        let id = LivingPropId(2);
        let back = WoozeSample {
            step: 0.7,
            ..WoozeSample::NEUTRAL
        };
        props.set_wooze_of(id, back);
        assert_eq!(props.prop(id).clock().step, 0.25);
        assert_eq!(props.prop(id).wooze_below(), back);
        assert_eq!(props.resolved(id, None).glide.layer, LivingLayer::Global);
        props.set_wooze_of(LivingPropId(0), back);
        assert_eq!(props.prop(LivingPropId(0)).clock().step, 0.7);
    }

    #[test]
    fn layers_put_back_are_the_grove_as_it_was() {
        let mut resolver = grove(thorny());
        let start = resolver.living_props().layers();
        let configs = |r: &ProcGenResolver| -> Vec<(EchoConfig, Stagger, f32)> {
            let props = r.living_props();
            props
                .props()
                .iter()
                .map(|p| (p.echo_config(), p.stagger(), p.clock().step))
                .collect()
        };
        let before = (configs(&resolver), resolver.living_props().variances());
        let props = resolver.living_props_mut();
        props.set_echoes(EchoConfig::default());
        props.set_stagger(Stagger::PerBranch);
        props.set_species("thorn", SpeciesOverride::default());
        props.tune_clock(
            "sapling",
            ClockOverride {
                step: Some(3.0),
                ..Default::default()
            },
        );
        assert_ne!(configs(&resolver), before.0);
        resolver.living_props_mut().restore_layers(&start);
        assert_eq!(
            (configs(&resolver), resolver.living_props().variances()),
            before
        );
    }

    /// A fair side-by-side pair is a copy of a spec under a new name with the same
    /// lineage seed: its own species, and the same tree. Its poses come from the
    /// prop's name and the step, so the same name grows the very same pose.
    #[test]
    fn a_copy_of_a_spec_under_a_new_name_grows_the_same_skeleton() {
        let spec = sapling("");
        let mut resolver = resolver_with(&spec);
        resolver.index_spec(ProcGenSpec::parse(&renamed(&spec, "sapling_twin")).unwrap());
        let mut twin = request(2, "same_name");
        twin.spec_name = "sapling_twin".to_string();
        let a = resolver.add_living_prop(request(1, "same_name")).unwrap();
        let b = resolver.add_living_prop(twin).unwrap();
        assert_eq!(a.1, "sapling@7");
        assert_eq!(b.1, "sapling_twin@7", "a mesh of its own");
        resolver.make_living_now();
        let props = resolver.living_props();
        assert_eq!(props.lineage_count(), 2);
        let (skeleton_a, rig_a) = props.rig_of(a.0).unwrap();
        let (skeleton_b, rig_b) = props.rig_of(b.0).unwrap();
        assert_eq!(**skeleton_a, **skeleton_b);
        assert_eq!(**rig_a, **rig_b);
        assert_eq!(props.rest_height(a.0), props.rest_height(b.0));
        let first = props.prop(a.0).first_step().unwrap();
        for step in first..first + props.prop(a.0).ring_len() as u64 {
            assert_eq!(props.prop(a.0).pose(step), props.prop(b.0).pose(step));
        }
        assert_eq!(props.species_of(b.0), "sapling_twin");
    }
}
