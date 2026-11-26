//! A prop's window of poses, and the cache of poses no window holds.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use flint_core::ContentHash;

use flint_procgen::living::{LivingPose, SegmentPose};

/// A window onto a prop's endless stream of poses: `first` and the steps after
/// it, in fixed slots, so sliding it along allocates nothing.
///
/// There's one slot more than the ring is asked to hold: a spare on deck, made
/// whenever the frame has room. So a beat, which takes the oldest pose away,
/// leaves a full ring and not one that is short until the queue catches up, and
/// poses get made across the quiet frames between beats.
#[derive(Debug, Clone)]
pub(super) struct PoseRing {
    pub(super) first: u64,
    pub(super) slots: Vec<Option<RingPose>>,
    pub(super) ready: usize,
}

/// One pose in a ring, in both of its forms.
#[derive(Debug, Clone)]
pub(super) struct RingPose {
    /// As its shape seed grew it. It never changes, so a new variance starts here.
    pub(super) full: Arc<LivingPose>,
    /// Held to the lineage's variance: what a prop draws.
    pub(super) held: Arc<LivingPose>,
    /// Which of the lineage's variances `held` is at.
    pub(super) generation: u64,
}

impl PoseRing {
    /// A ring that holds `len` poses, with its spare.
    pub(super) fn new(first: u64, len: usize) -> Self {
        Self {
            first,
            slots: vec![None; len + 1],
            ready: 0,
        }
    }

    /// How many poses it holds when full, not counting the spare.
    pub(super) fn len(&self) -> usize {
        self.slots.len() - 1
    }

    pub(super) fn slot(&self, index: u64) -> usize {
        (index % self.slots.len() as u64) as usize
    }

    pub(super) fn contains(&self, index: u64) -> bool {
        index >= self.first && index - self.first < self.slots.len() as u64
    }

    pub(super) fn get(&self, index: u64) -> Option<&Arc<LivingPose>> {
        if !self.contains(index) {
            return None;
        }
        self.slots[self.slot(index)].as_ref().map(|pose| &pose.held)
    }

    /// Whether it holds every pose from `first` for `count` steps.
    pub(super) fn holds(&self, first: u64, count: u64) -> bool {
        (first..first + count).all(|index| self.get(index).is_some())
    }

    /// A window at `first` of `len` poses and the spare, keeping whatever both
    /// windows have. Unlike [`slide_to`](Self::slide_to) this can move back, and
    /// what's now missing behind is what gets made next.
    pub(super) fn reshape(&mut self, first: u64, len: usize) {
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
    pub(super) fn next_stale(&self, first: u64, count: u64, generation: u64) -> Option<u64> {
        (first..first + count).find(|&index| {
            self.contains(index)
                && self.slots[self.slot(index)]
                    .as_ref()
                    .is_some_and(|pose| pose.generation != generation)
        })
    }

    /// Move the window forward, dropping the poses it leaves behind.
    pub(super) fn slide_to(&mut self, first: u64) {
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
    pub(super) fn next_missing(&self) -> Option<u64> {
        if self.ready == self.slots.len() {
            return None;
        }
        (self.first..self.first + self.slots.len() as u64).find(|&i| self.get(i).is_none())
    }

    pub(super) fn put(&mut self, index: u64, pose: RingPose) {
        let slot = self.slot(index);
        if self.slots[slot].replace(pose).is_none() {
            self.ready += 1;
        }
    }

    /// How many poses it's missing of what it should hold. The spare doesn't
    /// count: it's what stops this going above zero.
    pub(super) fn short(&self) -> usize {
        self.len().saturating_sub(self.ready)
    }
}

/// Poses that have been generated, apart from the meshes in
/// [`ProcGenCache`](flint_procgen::ProcGenCache): they're tiny, and a grove makes a
/// hundred a second, so they'd push the meshes out. First in, first out, since
/// a prop only moves forward through its stream, and a pose in a ring stays
/// alive whatever happens to it here.
#[derive(Debug)]
pub(super) struct PoseCache {
    pub(super) poses: HashMap<(ContentHash, u64), Arc<LivingPose>>,
    pub(super) order: VecDeque<(ContentHash, u64)>,
    pub(super) bytes: usize,
    pub(super) max_bytes: usize,
}

impl PoseCache {
    pub(super) fn new(max_bytes: usize) -> Self {
        Self {
            poses: HashMap::new(),
            order: VecDeque::new(),
            bytes: 0,
            max_bytes,
        }
    }

    pub(super) fn size_of(pose: &LivingPose) -> usize {
        pose.segments.len() * std::mem::size_of::<SegmentPose>()
    }

    pub(super) fn get(&self, identity: ContentHash, shape_seed: u64) -> Option<Arc<LivingPose>> {
        self.poses.get(&(identity, shape_seed)).cloned()
    }

    pub(super) fn insert(&mut self, identity: ContentHash, shape_seed: u64, pose: Arc<LivingPose>) {
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
