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

use flint_core::{ContentHash, EntityId};
use web_time::Instant;

use crate::generator::LivingGenerator;
use crate::living::{LivingClock, LivingPose, LivingRig, LivingSkeleton, SegmentPose};
use crate::registry::GeneratorRegistry;
use crate::spec::{ProcGenSpec, Stagger};

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

    /// The clock its parts drift on, read at [`Self::local_time`].
    pub fn clock(&self) -> LivingClock {
        self.clock
    }

    /// How far into its step cycle it runs, as a fraction of a step.
    pub fn phase_offset(&self) -> f32 {
        self.phase_offset
    }

    /// Where this prop's clock stands when the props' clock reads `time`.
    pub fn local_time(&self, time: f64) -> f64 {
        time + f64::from(self.phase_offset) * f64::from(self.clock.step)
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
        }
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
        });
        (LivingPropId(self.props.len() - 1), asset_name)
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

    /// One frame: move the clock on by `dt`, slide the rings to it, and
    /// generate what's missing within `budget_ms`. Lineages come first, since
    /// nothing can pose before them; one can't be cut short, so a frame builds
    /// at most one and then stops.
    pub(crate) fn tick(
        &mut self,
        dt: f64,
        budget_ms: f64,
        registry: &GeneratorRegistry,
    ) -> Vec<LineageDone> {
        self.time += dt;
        let start = Instant::now();
        for prop in &mut self.props {
            let now = prop.clock.at(prop.local_time(self.time)).index;
            if let Some(ring) = &mut prop.ring {
                ring.slide_to(now);
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
                let rig = built.rig.clone();
                let max_depth = built.skeleton.max_depth();
                lineage.built = Some(built);
                for prop in self.props.iter_mut().filter(|p| p.lineage == index) {
                    let first = prop.clock.at(prop.local_time(self.time)).index;
                    // Every pose a staggered part can be using has to fit.
                    let len = u64::from(prop.pose_ring).max(prop.clock.reach(max_depth) + 1);
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
    let fnv = name.bytes().fold(0xcbf2_9ce4_8422_2325u64, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3)
    });
    let mut h = fnv ^ (fnv >> 33);
    h = h.wrapping_mul(0xff51_afd7_ed55_8ccd);
    h ^= h >> 33;
    h = h.wrapping_mul(0xc4ce_b9fe_1a85_ec53);
    h ^ (h >> 33)
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
"#
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
        assert_eq!(
            palette.len(),
            done[0].rig.as_ref().unwrap().bone_segments.len()
        );
        for bone in palette {
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
            let t = prop.local_time(f64::from(i) * 0.013);
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
            let t = prop.local_time(props.time());
            assert_eq!(prop.first_step(), Some(prop.clock().at(t).index));
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
}
