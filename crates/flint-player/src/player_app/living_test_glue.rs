//! The living tests' way in (#724 step 3c): the methods they call on
//! `PlayerApp`, each handing the living subsystem its context, and the
//! subsystem itself (`living()`), made bare if the test entered no scene.

#![allow(unused_imports)]
use super::*;
use crate::player_app::living_ab::AbState;
use crate::player_app::living_subsystem::LivingSubsystem;
use crate::player_app::subsystem::stage_ctx;
use crate::player_app::PlayerApp;
#[cfg(feature = "debug-hud")]
use flint_debug_ui::{KnobFlags, LivingPanelAction, LivingPickedState, LivingSpeciesState};

impl PlayerApp {
    /// The living subsystem, a bare one if the scene made none.
    pub(crate) fn living(&mut self) -> &mut LivingSubsystem {
        if self.core.subsystems.find::<LivingSubsystem>().is_none() {
            // As a scene's builder makes it: the comparison --compare asked for
            let mut living = LivingSubsystem::default();
            living.tune.ab_picks = self.launch.compare.clone();
            living.tune.compare_at_start = self.launch.compare_at_start.take();
            self.core.subsystems.push(Box::new(living));
        }
        self.core.subsystems.find_mut::<LivingSubsystem>().unwrap()
    }

    /// Run `f` on the living subsystem with its context.
    pub(crate) fn with_living<R>(
        &mut self,
        f: impl FnOnce(&mut LivingSubsystem, &mut crate::player_app::subsystem::StageCtx) -> R,
    ) -> R {
        self.living();
        let living = self.core.subsystems.find_mut::<LivingSubsystem>().unwrap();
        let mut ctx = stage_ctx!(self, None);
        f(living, &mut ctx)
    }

    /// The procgen queue, then `dt` of living props, as one frame's
    /// `tick_procgen(dt)` was before the living subsystem.
    pub(crate) fn tick_procgen_for_test(&mut self, dt: f64) {
        self.tick_procgen();
        self.with_living(|living, ctx| living.tick(ctx, dt));
    }

    #[cfg(feature = "debug-hud")]
    pub(crate) fn apply_living_action(&mut self, action: LivingPanelAction) {
        self.with_living(|living, ctx| {
            let _ = &ctx;
            living.apply_living_action(ctx, action)
        })
    }

    pub(crate) fn before_living_tick(&mut self) -> f64 {
        self.with_living(|living, ctx| {
            let _ = &ctx;
            living.before_living_tick(ctx)
        })
    }

    pub(crate) fn clear_living_species(
        &mut self,
        name: &str,
        keys: &[String],
    ) -> Result<(), String> {
        self.with_living(|living, ctx| {
            let _ = &ctx;
            living.clear_living_species(ctx, name, keys)
        })
    }

    pub(crate) fn commit_living(&mut self) -> Result<String, String> {
        self.with_living(|living, ctx| {
            let _ = &ctx;
            living.commit_living(ctx)
        })
    }

    pub(crate) fn compare_living(&mut self, a: &str, b: &str) -> Result<(), String> {
        self.with_living(|living, ctx| {
            let _ = &ctx;
            living.compare_living(ctx, a, b)
        })
    }

    #[cfg(feature = "debug-hud")]
    pub(crate) fn drain_living_panel(&mut self) {
        self.with_living(|living, ctx| {
            let _ = &ctx;
            living.drain_living_panel(ctx)
        })
    }

    pub(crate) fn flip_living(&mut self) -> bool {
        self.with_living(|living, ctx| {
            let _ = &ctx;
            living.flip_living(ctx)
        })
    }

    pub(crate) fn hidden_side_stats(&mut self) -> Option<flint_procgen::LivingStats> {
        self.with_living(|living, ctx| {
            let _ = &ctx;
            living.hidden_side_stats()
        })
    }

    pub(crate) fn living_ab_key(&mut self, split: bool) {
        self.with_living(|living, ctx| {
            let _ = &ctx;
            living.living_ab_key(ctx, split)
        })
    }

    pub(crate) fn living_ab_state(&mut self) -> Option<AbState> {
        self.with_living(|living, ctx| {
            let _ = &ctx;
            living.living_ab_state()
        })
    }

    pub(crate) fn living_knobs(&mut self) -> WoozePreset {
        self.with_living(|living, ctx| {
            let _ = &ctx;
            living.living_knobs(ctx)
        })
    }

    pub(crate) fn living_let_go(&mut self) -> bool {
        self.with_living(|living, ctx| {
            let _ = &ctx;
            living.living_let_go(ctx)
        })
    }

    pub(crate) fn living_look(&mut self) -> WoozePreset {
        self.with_living(|living, ctx| {
            let _ = &ctx;
            living.living_look(ctx)
        })
    }

    #[cfg(feature = "debug-hud")]
    pub(crate) fn living_mixed(&mut self) -> KnobFlags {
        self.with_living(|living, ctx| {
            let _ = &ctx;
            living.living_mixed(ctx)
        })
    }

    #[cfg(feature = "debug-hud")]
    pub(crate) fn living_picked_state(&mut self, name: &str) -> Option<LivingPickedState> {
        self.with_living(|living, ctx| {
            let _ = &ctx;
            living.living_picked_state(ctx, name)
        })
    }

    pub(crate) fn living_preset_names(&mut self) -> Vec<String> {
        self.with_living(|living, ctx| {
            let _ = &ctx;
            living.living_preset_names(ctx)
        })
    }

    pub(crate) fn living_resolved(&mut self) -> Vec<ResolvedProp> {
        self.with_living(|living, ctx| {
            let _ = &ctx;
            living.living_resolved(ctx)
        })
    }

    pub(crate) fn living_species(&mut self) -> Vec<String> {
        self.with_living(|living, ctx| {
            let _ = &ctx;
            living.living_species(ctx)
        })
    }

    #[cfg(feature = "debug-hud")]
    pub(crate) fn living_species_states(
        &mut self,
        unsaved: &super::Unsaved,
    ) -> Vec<LivingSpeciesState> {
        self.with_living(|living, ctx| {
            let _ = &ctx;
            living.living_species_states(ctx, unsaved)
        })
    }

    pub(crate) fn living_split(&mut self) -> bool {
        self.with_living(|living, ctx| {
            let _ = &ctx;
            living.living_split()
        })
    }

    pub(crate) fn living_takes_click(
        &mut self,
        state: winit::event::ElementState,
        button: winit::event::MouseButton,
    ) -> bool {
        self.with_living(|living, ctx| {
            let _ = &ctx;
            living.living_takes_click(ctx, state, button)
        })
    }

    pub(crate) fn living_unsaved(&mut self) -> Unsaved {
        self.with_living(|living, ctx| {
            let _ = &ctx;
            living.living_unsaved(ctx)
        })
    }

    pub(crate) fn living_view(&mut self, species: Option<&str>) -> LivingView {
        self.with_living(|living, ctx| {
            let _ = &ctx;
            living.living_view(ctx, species)
        })
    }

    pub(crate) fn load_living_preset(&mut self, name: &str) -> Result<(), String> {
        self.with_living(|living, ctx| {
            let _ = &ctx;
            living.load_living_preset(ctx, name)
        })
    }

    #[cfg(feature = "debug-hud")]
    pub(crate) fn pick_living(&mut self, name: Option<&str>) -> bool {
        self.with_living(|living, ctx| {
            let _ = &ctx;
            living.pick_living(ctx, name)
        })
    }

    #[cfg(feature = "debug-hud")]
    pub(crate) fn pick_living_at_centre(&mut self) -> bool {
        self.with_living(|living, ctx| {
            let _ = &ctx;
            living.pick_living_at_centre(ctx)
        })
    }

    #[cfg(feature = "debug-hud")]
    pub(crate) fn pick_next_living(&mut self, forward: bool) -> bool {
        self.with_living(|living, ctx| {
            let _ = &ctx;
            living.pick_next_living(ctx, forward)
        })
    }

    #[cfg(feature = "debug-hud")]
    pub(crate) fn picked_living(&mut self) -> Option<String> {
        self.with_living(|living, ctx| {
            let _ = &ctx;
            living.picked_living(ctx)
        })
    }

    pub(crate) fn reset_living_knobs(&mut self) {
        self.with_living(|living, ctx| {
            let _ = &ctx;
            living.reset_living_knobs(ctx)
        })
    }

    pub(crate) fn save_living_preset(&mut self, name: &str) -> Result<PathBuf, String> {
        self.with_living(|living, ctx| {
            let _ = &ctx;
            living.save_living_preset(ctx, name)
        })
    }

    pub(crate) fn step_living(&mut self) {
        self.with_living(|living, ctx| {
            let _ = &ctx;
            living.step_living()
        })
    }

    pub(crate) fn stop_comparing_living(&mut self) {
        self.with_living(|living, ctx| {
            let _ = &ctx;
            living.stop_comparing_living(ctx)
        })
    }

    pub(crate) fn tune_living(&mut self, tune: &WoozePreset) -> Result<(), String> {
        self.with_living(|living, ctx| {
            let _ = &ctx;
            living.tune_living(ctx, tune)
        })
    }

    pub(crate) fn tune_living_from_script(&mut self, knobs: toml::Value) -> Result<(), String> {
        self.with_living(|living, ctx| {
            let _ = &ctx;
            living.tune_living_from_script(ctx, knobs)
        })
    }

    pub(crate) fn tune_species(&mut self, name: &str, def: &LivingSpeciesDef) -> Vec<String> {
        self.with_living(|living, ctx| {
            let _ = &ctx;
            living.tune_species(ctx, name, def)
        })
    }
}
