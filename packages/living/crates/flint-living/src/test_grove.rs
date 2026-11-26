//! The tests' host: a resolver and the props beside it, as a game keeps them,
//! with the entry points the tests used when the resolver held the props
//! (#1024). Each calls the public seam, so the tests drive what a host does.

use flint_procgen::{CacheStats, ProcGenCacheConfig, ProcGenResolver, ProcGenSpec};
use flint_wooze::WoozeField;

use crate::{LineageDone, LivingDefaults, LivingPropId, LivingPropRequest, LivingProps};

pub(crate) struct Grove {
    resolver: ProcGenResolver,
    living: LivingProps,
}

impl Grove {
    pub(crate) fn new() -> Self {
        Self::with_config(ProcGenCacheConfig::default())
    }

    pub(crate) fn with_config(config: ProcGenCacheConfig) -> Self {
        Self {
            resolver: ProcGenResolver::with_config(config),
            living: LivingProps::default(),
        }
    }

    pub(crate) fn index_spec(&mut self, spec: ProcGenSpec) {
        self.resolver.index_spec(spec);
    }

    pub(crate) fn cache_stats(&self) -> CacheStats {
        self.resolver.cache_stats()
    }

    pub(crate) fn frame_budget_for_test(&mut self, budget_ms: f64) {
        self.resolver.set_frame_budget_ms(budget_ms);
    }

    pub(crate) fn add_living_prop(
        &mut self,
        request: LivingPropRequest,
    ) -> Result<(LivingPropId, String), String> {
        self.living.add_prop(&mut self.resolver, request)
    }

    pub(crate) fn make_living_now(&mut self) -> Vec<LineageDone> {
        self.living.make_now(&self.resolver)
    }

    pub(crate) fn process_living(&mut self, dt: f64) -> Vec<LineageDone> {
        self.living.process(&self.resolver, dt, None)
    }

    pub(crate) fn process_living_in(
        &mut self,
        dt: f64,
        wooze: Option<(&WoozeField, f64)>,
    ) -> Vec<LineageDone> {
        self.living.process(&self.resolver, dt, wooze)
    }

    pub(crate) fn step_living_now(
        &mut self,
        dt: f64,
        wooze: Option<(&WoozeField, f64)>,
    ) -> Vec<LineageDone> {
        self.living.step_now(&self.resolver, dt, wooze)
    }

    pub(crate) fn process_living_beside(
        &mut self,
        side: &mut LivingProps,
        dt: f64,
        wooze: Option<(&WoozeField, f64)>,
    ) {
        self.living.process_beside(side, &self.resolver, dt, wooze);
    }

    pub(crate) fn settle_living_now(&mut self, side: Option<&mut LivingProps>) {
        self.living.settle(&self.resolver, side);
    }

    pub(crate) fn living_props(&self) -> &LivingProps {
        &self.living
    }

    pub(crate) fn living_props_mut(&mut self) -> &mut LivingProps {
        &mut self.living
    }

    pub(crate) fn set_living_defaults(&mut self, defaults: LivingDefaults) {
        self.living.set_defaults(defaults);
    }

    pub(crate) fn clear_living_props(&mut self) {
        self.living.clear_props();
    }
}
