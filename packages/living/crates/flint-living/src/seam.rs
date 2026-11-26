//! The seam with the procgen resolver (#1024): the props are the host's, kept
//! beside its [`ProcGenResolver`], which lends them three things and holds
//! nothing of theirs: its specs (to check one can live, and what a pose of it
//! depends on), its generators, and what its asset queue left of the frame
//! budget ([`ProcGenResolver::budget_left_ms`]).
//!
//! Every entry point that used to be a resolver method is one here, taking the
//! resolver: `resolver.add_living_prop(r)` is `props.add_prop(&mut resolver, r)`,
//! `resolver.process_living_in(dt, w)` is `props.process(&resolver, dt, w)`.

use flint_procgen::ProcGenResolver;
use flint_wooze::WoozeField;

use super::{LineageDone, LivingPropId, LivingPropRequest, LivingProps, POSE_CACHE_BYTES};

impl Default for LivingProps {
    /// No props, and a pose cache of [`POSE_CACHE_BYTES`]: a host's.
    fn default() -> Self {
        Self::new(POSE_CACHE_BYTES)
    }
}

impl LivingProps {
    /// Add a living prop whose spec `resolver` holds. Returns its id and the
    /// name of the mesh it draws, one per spec and lineage; `Err` says why its
    /// spec can't live, in which case nothing was added. Nothing is generated yet.
    pub fn add_prop(
        &mut self,
        resolver: &mut ProcGenResolver,
        request: LivingPropRequest,
    ) -> Result<(LivingPropId, String), String> {
        let (spec, identity) = resolver.living_spec(&request.spec_name)?;
        Ok(self.add(request, spec, &identity))
    }

    /// Build every lineage and fill every ring now, unbudgeted, for a scene load
    /// whose first frame should find them full.
    pub fn make_now(&mut self, resolver: &ProcGenResolver) -> Vec<LineageDone> {
        self.make_all_now(resolver.registry())
    }

    /// One frame: move the props' clock on by `dt` (0 while paused), then
    /// generate what the rings are missing in whatever the resolver's asset
    /// queue left of the frame budget; so call it after
    /// [`ProcGenResolver::process_frame`]. `wooze` is the scene's field with the
    /// moment on its own clock to sample it at; `None` for a scene with none, in
    /// which props keep to their specs and nothing is sampled. Lineages that
    /// finished come back to be uploaded. Does nothing at all with no props.
    pub fn process(
        &mut self,
        resolver: &ProcGenResolver,
        dt: f64,
        wooze: Option<(&WoozeField, f64)>,
    ) -> Vec<LineageDone> {
        if self.is_empty() {
            return Vec::new();
        }
        self.tick(dt, resolver.budget_left_ms(), resolver.registry(), wooze)
    }

    /// One step with no frame budget: every pose the rings want is there when
    /// this returns. For a capture, where nobody is waiting on the frame and a
    /// pose arriving late would change the picture (#216).
    pub fn step_now(
        &mut self,
        resolver: &ProcGenResolver,
        dt: f64,
        wooze: Option<(&WoozeField, f64)>,
    ) -> Vec<LineageDone> {
        if self.is_empty() {
            return Vec::new();
        }
        self.tick(dt, f64::INFINITY, resolver.registry(), wooze)
    }

    /// [`process`](Self::process) for a second look at these props (#238), made
    /// with [`fork`](Self::fork): its own knobs, the same clock and frame budget,
    /// and these props' pose cache, so a pose either side has made the other
    /// finds. Its lineages are built, so nothing comes back.
    pub fn process_beside(
        &mut self,
        side: &mut LivingProps,
        resolver: &ProcGenResolver,
        dt: f64,
        wooze: Option<(&WoozeField, f64)>,
    ) {
        if side.is_empty() {
            return;
        }
        let left = resolver.budget_left_ms();
        self.swap_cache(side);
        side.tick(dt, left, resolver.registry(), wooze);
        self.swap_cache(side);
    }

    /// Finish what the knobs have started, now and with no budget: see
    /// `settle_now`. With `side`, that side's instead, on these props' pose cache.
    pub fn settle(&mut self, resolver: &ProcGenResolver, side: Option<&mut LivingProps>) {
        match side {
            Some(side) => {
                self.swap_cache(side);
                side.settle_now(resolver.registry());
                self.swap_cache(side);
            }
            None => self.settle_now(resolver.registry()),
        }
    }
}
