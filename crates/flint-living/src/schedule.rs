//! Filling the rings within the frame budget: which pose comes next, re-holds
//! at a new variance, and building lineages. The budget only decides which
//! frame a pose is ready on, never which pose it is.

use std::sync::Arc;

use flint_wooze::WoozeField;
use web_time::Instant;

use super::layers::LivingShadows;
use super::prop::LivingProp;
use super::ring::{PoseRing, RingPose};
use super::{BuiltLineage, Lineage, LineageDone, LivingProps, LivingStats};
use flint_procgen::living::{LivingPose, SegmentPose};
use flint_procgen::GeneratorRegistry;
use flint_procgen::LivingGenerator;
use flint_procgen::ProcGenSpec;

/// The re-hold pass that reaches poses nothing is reading yet, which take turns
/// with making new ones. See [`LivingProp::rehold_span`].
pub(super) const REHOLD_PASS_REST: u8 = 2;

/// How fast the pose estimate forgets its slowest pose, each pose. Poses vary
/// by a factor of two, so an estimate that forgets within a frame or two lets a
/// slow one overrun the budget. At this rate it takes about a second of a grove's
/// poses to halve.
pub(super) const POSE_ESTIMATE_DECAY: f64 = 0.99;

/// The budget arithmetic of [`LivingProps::fill_within`], apart from the clock
/// and the generator, so a test can give it a pose cost of its own (#597).
/// `used_ms` is the frame's time so far; `fill` makes the next pose and says
/// whether that took generating and how many ms it took, or `None` if nothing
/// is missing. Returns whether it generated anything.
pub(super) fn fill_while_it_fits(
    estimate_ms: &mut f64,
    budget_ms: f64,
    mut used_ms: impl FnMut() -> f64,
    mut fill: impl FnMut() -> Option<(bool, f64)>,
) -> bool {
    let mut made = false;
    loop {
        let used = used_ms();
        let estimate = *estimate_ms;
        let a_later_frame_fits_it = estimate > 0.0 && estimate <= budget_ms;
        if used + estimate > budget_ms && (made || budget_ms <= 0.0 || a_later_frame_fits_it) {
            // Only making a pose teaches the estimate anything, so one that came
            // down to just under the budget would be turned away by every frame
            // after it, and the rings would run dry with the budget unspent.
            if !made {
                *estimate_ms *= POSE_ESTIMATE_DECAY;
            }
            return made;
        }
        let Some((generated, took)) = fill() else {
            return made;
        };
        if generated {
            made = true;
            *estimate_ms = (*estimate_ms * POSE_ESTIMATE_DECAY).max(took);
        }
    }
}

impl LivingProps {
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
        flint_core::scope!("living::tick", self.props.len());
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
            // Times 1.0 is exact, so an untuned boil reads the props' clock (#529).
            prop.jitter_time += dt * prop.jitter_rate_in(&species[prop.species]);
            prop.row_time += dt * prop.row_rate_in(&species[prop.species]);
            let first = prop.window_start();
            if let Some(ring) = &mut prop.ring {
                ring.slide_to(first);
            }
        }

        let done = if budget_ms > 0.0 {
            flint_core::scope!("living::build_lineage");
            self.build_next_lineage(registry)
        } else {
            None
        };
        let worked = done.is_some() || {
            flint_core::scope!("living::fill_rings");
            self.fill_within(budget_ms, start, registry)
        };
        self.start_what_is_ready();

        let spent = start.elapsed().as_secs_f64() * 1000.0;
        self.record_frame(spent, budget_ms, worked);
        done.into_iter().collect()
    }

    /// Generate poses, emptiest ring first, until the next would pass the
    /// budget. A budget too small for even one pose (a debug build) still gets
    /// one a frame, or the rings would never fill at all. Returns whether it
    /// generated anything.
    pub(super) fn fill_within(
        &mut self,
        budget_ms: f64,
        start: Instant,
        registry: &GeneratorRegistry,
    ) -> bool {
        let mut estimate = self.pose_estimate_ms;
        let made = fill_while_it_fits(
            &mut estimate,
            budget_ms,
            || start.elapsed().as_secs_f64() * 1000.0,
            || {
                let began = Instant::now();
                let generated = self.fill_one(registry)?;
                Some((generated, began.elapsed().as_secs_f64() * 1000.0))
            },
        );
        self.pose_estimate_ms = estimate;
        made
    }

    /// Give the emptiest ring its next pose. Returns whether that took
    /// generating (false for a cache hit), or `None` if nothing is missing.
    pub(super) fn fill_one(&mut self, registry: &GeneratorRegistry) -> Option<bool> {
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
    pub(super) fn pose_needed_soonest(&self) -> Option<(usize, u64)> {
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
    pub(super) fn rehold_one(&mut self) -> bool {
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
    pub(super) fn start_what_is_ready(&mut self) {
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
    pub(super) fn build_next_lineage(
        &mut self,
        registry: &GeneratorRegistry,
    ) -> Option<LineageDone> {
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
                built.rig.still_bones_into(&mut rest_palette);
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

    pub(super) fn record_frame(&mut self, spent_ms: f64, budget_ms: f64, worked: bool) {
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

/// `full` held to its lineage's variance. A pose of its skeleton can always be
/// held; one that somehow couldn't keeps its full form, rather than a hole.
pub(super) fn hold(lineage: &Lineage, full: &Arc<LivingPose>) -> Arc<LivingPose> {
    lineage
        .built
        .as_ref()
        .and_then(|built| built.skeleton.with_variance(full, &lineage.variance).ok())
        .map_or_else(|| full.clone(), Arc::new)
}

pub(super) fn living_side<'a>(
    registry: &'a GeneratorRegistry,
    spec: &ProcGenSpec,
) -> Option<&'a dyn LivingGenerator> {
    registry.get(&spec.generator)?.as_living()
}

pub(super) fn build_lineage(
    registry: &GeneratorRegistry,
    spec: &ProcGenSpec,
    seed: u64,
) -> std::result::Result<BuiltLineage, String> {
    let generator = living_side(registry, spec)
        .ok_or_else(|| format!("its generator '{}' can't live", spec.generator))?;
    let skeleton = generator.structure(spec, seed).map_err(|e| e.to_string())?;
    let rig = generator.rig(spec, seed).map_err(|e| e.to_string())?;
    let rest = skeleton.rest_bounds();
    let rest_height = rest.max.y;
    let rest_reach = [rest.min.x, rest.max.x, rest.min.z, rest.max.z]
        .into_iter()
        .fold(0.0_f32, |reach, side| reach.max(side.abs()));
    Ok(BuiltLineage {
        skeleton: Arc::new(skeleton),
        rig: Arc::new(rig),
        rest_height,
        rest_reach,
    })
}
