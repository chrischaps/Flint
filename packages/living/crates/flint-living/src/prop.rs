//! One prop: its clocks, its echoes, and what they ask of its ring.

use std::sync::Arc;

use flint_core::EntityId;
use flint_wooze::{Cycle, WoozeSample};

use super::blend::{DrawnBones, DrawnState, PropEcho};
use super::layers::{EchoOverride, Layered};
use super::ring::PoseRing;
use flint_procgen::living::{LivingClock, LivingJitter, LivingPose, WarpedTime};
use flint_procgen::{EchoBlend, EchoConfig, EchoCopy, EchoPlaces, Stagger};

/// One prop's runtime state, the other half of its `living_prop` component.
#[derive(Debug, Clone)]
pub struct LivingProp {
    pub(super) entity: EntityId,
    pub(super) entity_name: String,
    pub(super) asset_name: String,
    pub(super) lineage: usize,
    /// Added to a step index to give that step's shape seed.
    pub(super) stream: u64,
    pub(super) phase_offset: f32,
    pub(super) clock: LivingClock,
    pub(super) pose_ring: u32,
    /// `None` until its lineage is built, since its size depends on the depth.
    pub(super) ring: Option<PoseRing>,
    pub(super) jitter: LivingJitter,
    /// Seconds its jitter runs ahead of the props' clock, fixed when it's added:
    /// each value already has its own phase, and has no need to bend with the
    /// step. Only the slow layer does.
    pub(super) jitter_ahead: f64,
    /// The step when it was added, which puts its echoes' jitter in seconds.
    pub(super) jitter_step: f64,
    /// The seconds its jitter has run through (#529): the props' clock, but moved
    /// on by `dt` times its jitter rate, so a new rate changes how fast the boil
    /// ticks from here and never where it is. At a rate of 1 it's the props'
    /// clock to the bit.
    pub(super) jitter_time: f64,
    /// The seconds its words have drifted along their lines through (#576): 0
    /// when it's added, moved on by `dt` times its row rate, so a new rate
    /// changes how fast they go from here and never where they are.
    pub(super) row_time: f64,
    /// See [`Self::beats`].
    pub(super) beats: f64,
    pub(super) position: [f32; 3],
    pub(super) wooze: WoozeSample,
    /// The same as [`DrawnBones::parts`] for the drift-only shadow, whose cycle is
    /// two steps long.
    pub(super) shadow_cycle: Cycle,
    pub(super) drawn: DrawnState,
    /// Farthest from now first, which is the order they draw in.
    pub(super) echoes: Vec<PropEcho>,
    /// What its echoes are set to, or are about to be, in the units a person
    /// turns: [`Self::echoes`] is what those work out to.
    pub(super) echo_config: EchoConfig,
    /// How its echoes meet what's behind them: its spec's.
    pub(super) echo_blend: EchoBlend,
    /// Its spec's `echo_slide`: 0, or the steps at which an echo's slide is whole.
    pub(super) echo_slide: f32,
    /// Its spec's `echo_places`: where its echoes stand at a wide spread.
    pub(super) echo_places: EchoPlaces,
    /// How deep its lineage's tree is, once built. A ring's size depends on it.
    pub(super) max_depth: u32,
    /// Clocks it's been asked for and is waiting on poses to start.
    pub(super) pending: Option<PendingClocks>,
    /// Whether its echoes have changed since it last blended.
    pub(super) echoes_changed: bool,
    /// Its species, as an index into [`LivingProps::species`].
    pub(super) species: usize,
    /// What the layers under its species say of its clock: the field where it
    /// stands, or in a scene with none its spec's, or what was set live.
    pub(super) wooze_below: WoozeSample,
    /// Whether `wooze_below` was set live in a scene with no field, which makes
    /// it the global layer's and not the spec's.
    pub(super) clock_live: bool,
    /// Its echoes' layers below its species: its spec's, and the global
    /// layer's word, the scene's `[living] echoes` and whatever was moved live
    /// for every prop.
    pub(super) echo_layers: Layered<EchoOverride>,
    /// Its stagger's: its own, the entity's or else the spec's, and the global
    /// layer's word.
    pub(super) stagger_layers: Layered<Option<Stagger>>,
    /// Whether its own stagger is the entity's.
    pub(super) stagger_entity: bool,
    /// Its softness' (#582): its spec's, and the global layer's word.
    pub(super) softness_layers: Layered<Option<f32>>,
    /// Its jitter rate's (#529): 1, and the global layer's word.
    pub(super) jitter_rate_layers: Layered<Option<f32>>,
    /// Its row rate's (#576): 1, and the global layer's word.
    pub(super) row_rate_layers: Layered<Option<f32>>,
}

/// New clocks for a prop, held back until its ring has what they read.
#[derive(Debug, Clone)]
pub(super) struct PendingClocks {
    pub(super) stagger: Stagger,
    pub(super) copies: Vec<EchoCopy>,
}

/// What a set of clocks asks of a prop's stream of poses, at this moment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct RingNeeds {
    /// The earliest step any part of it can be reading.
    pub(super) first: u64,
    /// How many steps from there it can read: without these it would starve.
    pub(super) reads: u64,
    /// How many poses a ring for it keeps. At least `reads`, and the prop's
    /// `pose_ring`.
    pub(super) len: u64,
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

    /// Where its jitter's clock stands, in seconds: [`Self::jitter_time`] plus the
    /// phase it was given. What its leaf cards and shades read now.
    pub fn jitter_now(&self) -> f64 {
        self.jitter_time + self.jitter_ahead
    }

    /// The seconds its jitter has run through: the props' clock, at its jitter
    /// rate (#529).
    pub fn jitter_time(&self) -> f64 {
        self.jitter_time
    }

    /// The seconds its words have drifted along their lines through (#576):
    /// what its drift bones read now.
    pub fn row_time(&self) -> f64 {
        self.row_time
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
    /// pose step it was added on, since jitter doesn't bend with that step; `t`
    /// is the jitter's own clock, so an echo keeps the same many boil ticks
    /// behind at any jitter rate (#529).
    pub(super) fn echo_jitter_time(&self, index: usize, t: f64) -> f64 {
        t + f64::from(self.echoes[index].copy.offset) * self.jitter_step
    }

    /// The seconds echo `index`'s words read when this prop's read `t`: the
    /// same many pose steps from now as its poses, by the step it was added on,
    /// on the row clock, so an echo shows the lines a little before or after.
    pub(super) fn echo_row_time(&self, index: usize, t: f64) -> f64 {
        t + f64::from(self.echoes[index].copy.offset) * self.jitter_step
    }

    /// How fast its words drift against its spec's pace (#576), from its
    /// layers: its species' own, then the global layer's, then 1.
    pub(super) fn row_rate_in(&self, species: &super::Species) -> f64 {
        f64::from((self.row_rate_layers).value(species.over.row_rate))
    }

    /// How fast its jitter's clock runs against the props' (#529), from its
    /// layers: its species' own, then the global layer's, then 1.
    pub(super) fn jitter_rate_in(&self, species: &super::Species) -> f64 {
        f64::from((self.jitter_rate_layers).value(species.over.jitter_rate))
    }

    /// The first step its current clocks can be reading: the farthest echo's.
    /// A pose stays until the last echo has finished with it.
    pub(super) fn in_use_start(&self) -> u64 {
        let behind = self
            .echoes
            .iter()
            .map(|e| -e.copy.offset)
            .fold(0.0, f32::max);
        (self.beats - f64::from(behind)).max(0.0).floor() as u64
    }

    /// The first step its ring's window needs now: what's in use, and what's
    /// waiting to be.
    pub(super) fn window_start(&self) -> u64 {
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
    pub(super) fn needs_of(
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
    pub(super) fn rehold_span(&self, pass: u8, drift: bool) -> (u64, u64) {
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
    pub(super) fn needs_now(&self, drift: bool) -> RingNeeds {
        self.needs_of(
            self.clock.stagger,
            self.echoes.iter().map(|e| e.copy),
            drift,
        )
    }

    /// The first pose its current clocks read that isn't there, or will be read
    /// next, and how many steps into what they read it is: the fewer, the sooner.
    /// Only within the ring's window: a step past it has nowhere to go yet.
    pub(super) fn next_needed(&self, drift: bool) -> Option<(u64, u64)> {
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
    pub(super) fn window(&self, drift: bool) -> (u64, usize) {
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
    pub(super) fn fit_ring(&mut self, drift: bool) {
        let (first, len) = self.window(drift);
        if let Some(ring) = &mut self.ring {
            ring.reshape(first, len);
        }
    }

    /// Start the waiting clocks. An echo that stays where it was keeps its
    /// cycles, so a change of opacity alone moves nothing.
    pub(super) fn start_pending(&mut self) {
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
                    .filter(|was| was.copy.offset == copy.offset && was.copy.place == copy.place)
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
    pub(super) fn redraw(&mut self) {
        self.drawn.bones.times.clear();
        self.drawn.shadow = None;
        for echo in &mut self.echoes {
            echo.drawn.times.clear();
        }
    }

    /// How its echoes meet what's behind them, as its spec says.
    pub fn echo_blend(&self) -> EchoBlend {
        self.echo_blend
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
