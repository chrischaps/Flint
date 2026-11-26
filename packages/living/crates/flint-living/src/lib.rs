//! Living props at run time (#211): each prop's window of poses, kept full a
//! few steps ahead of its clock within the procgen frame budget.
//!
//! This is the scheduler a game links only when it has living props (#1024):
//! `flint-procgen` keeps the generators' side ([`LivingGenerator`], the rig,
//! the skeleton and its poses, the `[living]` spec and scene tables), and a
//! host keeps a [`LivingProps`] beside its [`ProcGenResolver`], which lends it
//! specs, generators and the frame budget its queue left (`seam.rs`).
//!
//! [`LivingGenerator`]: flint_procgen::LivingGenerator
//! [`ProcGenResolver`]: flint_procgen::ProcGenResolver
//!
//! A lineage is one spec and lineage seed: a [`LivingSkeleton`] and a
//! [`LivingRig`], shared by every prop that grows from it. A prop is one entity
//! on a lineage, with its own stream of shape seeds and its own [`PoseRing`].
//!
//! Poses don't go through [`ProcGenQueue`](flint_procgen::ProcGenQueue). That queue
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

// Unit tests read specs off the disk; the lib build still holds the rules (#761).
#![cfg_attr(test, allow(clippy::disallowed_methods))]

mod ab;
mod blend;
pub mod components;
mod defs;
mod layers;
mod prop;
mod ring;
mod schedule;
mod seam;
#[cfg(test)]
mod test_grove;

use std::sync::Arc;

use flint_core::{ContentHash, EntityId};
use flint_wooze::{Cycle, WoozeSample};

use flint_procgen::living::{mix64, LivingClock, LivingJitter, LivingRig, LivingSkeleton};
use flint_procgen::{ProcGenSpec, Stagger, VarianceConfig};

pub use blend::{BlendFrame, PropBlend};
pub use defs::{
    apply_preset, defaults_of, echoes_of, jitter_rate_of, row_rate_of, softness_of, species_of,
    species_problems, variance_of,
};
pub use flint_procgen::{KnobValue, LivingKnob};
pub use layers::{
    jitter_rate_clamped, row_rate_clamped, ClockOverride, EchoOverride, Layer, Layered,
    LivingDefaults, LivingLayer, LivingLayers, LivingScope, LivingShadows, Resolved, ResolvedProp,
    SpeciesOverride, VarianceOverride, JITTER_RATE_MAX, ROW_RATE_MAX,
};
pub use prop::LivingProp;

use blend::{DrawnBones, DrawnState, PropEcho};
use ring::PoseCache;

/// How many bytes of poses the cache keeps once no ring holds them. A pose of
/// the oak is under 6 KB, so this is about 1,400 of them: a grove's rings four
/// times over.
pub const POSE_CACHE_BYTES: usize = 8 * 1024 * 1024;

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

/// What a lineage is once it's built.
#[derive(Debug, Clone)]
struct BuiltLineage {
    skeleton: Arc<LivingSkeleton>,
    rig: Arc<LivingRig>,
    /// How tall it stands at rest. Kept, since working it out poses the whole tree.
    rest_height: f32,
    /// How far its rest pose reaches from the trunk, sideways (#248).
    rest_reach: f32,
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
    /// Its variance's layers below its species: the spec's, and the global
    /// layer's word over it.
    variance_layers: Layered<VarianceOverride>,
}

/// A species and its own values (#246).
#[derive(Debug, Clone)]
struct Species {
    name: String,
    over: SpeciesOverride,
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

    /// How far a prop reaches sideways from its trunk at rest, once its lineage
    /// is built: half its crown's width, for picking it on screen (#248).
    pub fn rest_reach(&self, id: LivingPropId) -> Option<f32> {
        let built = self.lineages[self.props[id.0].lineage].built.as_ref()?;
        Some(built.rest_reach)
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

    /// Move every prop's clock on by exactly `beats` of its own steps, however
    /// long those are, and the seconds its jitter reads by `seconds` at its
    /// jitter rate. For
    /// single-stepping: with `dt` at 0 it's the only thing moving the clock.
    pub fn advance_beats(&mut self, beats: f64, seconds: f64) {
        self.time += seconds;
        let species = &self.species;
        for prop in &mut self.props {
            prop.beats += beats;
            prop.jitter_time += seconds * prop.jitter_rate_in(&species[prop.species]);
            prop.row_time += seconds * prop.row_rate_in(&species[prop.species]);
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
            let variance_layers = Layered::new(living.variance, self.defaults.variance);
            self.lineages.push(Lineage {
                spec: Arc::new(spec.clone()),
                seed,
                asset_name: asset_name.clone(),
                identity: ContentHash::from_str(&format!("{identity}\n{seed}")),
                built: None,
                tried: false,
                variance: variance_layers.value(self.species[species].over.variance),
                generation: 0,
                species,
                variance_layers,
            });
            self.lineages.len() - 1
        });

        let over = self.species[species].over;
        let stream = name_hash(&request.entity_name);
        let phase_offset = request
            .phase_offset
            .map_or_else(|| name_phase(stream), |phase| phase.rem_euclid(1.0));
        let mut clock = LivingClock::from(living);
        let stagger_layers = Layered::new(
            request.stagger.unwrap_or(clock.stagger),
            self.defaults.stagger,
        );
        clock.stagger = stagger_layers.value(over.stagger);
        let echo_layers = Layered::new(living.echoes, self.defaults.echoes);
        let echo_config = echo_layers.value(over.echoes);
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
            // Where the props' clock is, so that at a rate of 1 the two agree.
            jitter_time: self.time,
            // Its words start where they were written (#576).
            row_time: 0.0,
            beats: f64::from(phase_offset),
            position: request.position,
            wooze,
            shadow_cycle: Cycle::default(),
            drawn: DrawnState::default(),
            echoes: echo_config
                .copies_placed(&living.echo_places)
                .into_iter()
                .map(|copy| PropEcho {
                    copy,
                    drawn: DrawnBones::default(),
                })
                .collect(),
            echo_config,
            echo_blend: living.echo_blend,
            echo_slide: living.echo_slide,
            echo_places: living.echo_places.clone(),
            max_depth: 0,
            pending: None,
            echoes_changed: false,
            species,
            wooze_below,
            clock_live: false,
            echo_layers,
            stagger_layers,
            stagger_entity: request.stagger.is_some(),
            softness_layers: Layered::new(living.softness, self.defaults.softness),
            jitter_rate_layers: Layered::new(1.0, self.defaults.jitter_rate),
            row_rate_layers: Layered::new(1.0, self.defaults.row_rate),
        });
        (LivingPropId(self.props.len() - 1), asset_name)
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
mod tests;
