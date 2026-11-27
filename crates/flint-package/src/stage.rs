//! The stages of a frame (ADR 0117 decision 1), public since ADR 0137.

/// The stages of a frame, in the order the player's `tick` runs them (ADR
/// 0117 decision 1). A subsystem's `update` is called at each, after the
/// player's own work of the stage before it and before its own work of this
/// one, and then each extension's `PlayerExtension::update` (decision 5,
/// #754). `flint_player::Stage` is this type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Stage {
    /// The frame's very start, before the clock ticks, in place of the
    /// player's pad polling while a subsystem has the pads
    /// ([`Pads`](crate::Pads)): where the rhythm session drains its capture
    /// into input (ADR 0018). Run by the host, which owns the clock, not by
    /// the core's step; a host with no pads (a capture) skips it.
    Clock,
    /// The clock has ticked and scripted input is in; before the transition
    /// advances and before extensions' `pre_update`. Where outside input for
    /// scripts is queued (the expression feed).
    Input,
    /// After extensions' `pre_update`, before scripts' `on_update`.
    PreScripts,
    /// After `on_update`, before the fixed physics steps.
    Scripts,
    /// Inside each fixed physics step, before Rapier's step. Delivered
    /// through [`Subsystem::fixed_update`](crate::Subsystem::fixed_update)
    /// with a [`FixedCtx`](crate::FixedCtx), once a step, not through
    /// `update`; named here so the schedule is whole in one place.
    FixedStep,
    /// After the physics steps and the first-person camera.
    Physics,
    /// After script commands and camera overrides: the camera is final for
    /// the frame.
    PostPhysics,
    /// After `on_draw_ui` and the draw commands.
    ScriptUi,
    /// After audio triggers, animation and particles.
    Animation,
    /// After the asset makers' frame (`flint_package::assets`: the procgen
    /// queue) and what moves on with it (living props). Named for what the
    /// kernel does there, not for a package (#1632).
    MadeAssets,
    /// After the renderer's sync, the frame's post overrides and a
    /// script's quality settings; before the frame's input is cleared.
    Sync,
    /// Before the frame is encoded. Delivered through
    /// [`Subsystem::frame_inputs`](crate::Subsystem::frame_inputs) and
    /// [`post_encode`](crate::Subsystem::post_encode), with the renderer,
    /// not through `update`; named here so the schedule is whole in one
    /// place.
    PreRender,
}

impl Stage {
    /// The stage's name, for a profiler scope's detail (#1096).
    pub fn label(self) -> &'static str {
        match self {
            Stage::Clock => "clock",
            Stage::Input => "input",
            Stage::PreScripts => "pre_scripts",
            Stage::Scripts => "scripts",
            Stage::FixedStep => "fixed_step",
            Stage::Physics => "physics",
            Stage::PostPhysics => "post_physics",
            Stage::ScriptUi => "script_ui",
            Stage::Animation => "animation",
            Stage::MadeAssets => "made_assets",
            Stage::Sync => "sync",
            Stage::PreRender => "pre_render",
        }
    }

    /// Every stage `update` hears in the player's frame, in order: the
    /// host's [`Stage::Clock`], then the core's.
    pub const UPDATES: [Stage; 10] = [
        Stage::Clock,
        Stage::Input,
        Stage::PreScripts,
        Stage::Scripts,
        Stage::Physics,
        Stage::PostPhysics,
        Stage::ScriptUi,
        Stage::Animation,
        Stage::MadeAssets,
        Stage::Sync,
    ];
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_stage_update_hears_has_its_own_label() {
        let mut labels: Vec<_> = Stage::UPDATES.iter().map(|s| s.label()).collect();
        labels.sort_unstable();
        labels.dedup();
        assert_eq!(labels.len(), Stage::UPDATES.len());
        assert!(!Stage::UPDATES.contains(&Stage::PreRender));
        assert!(!Stage::UPDATES.contains(&Stage::FixedStep));
        assert_ne!(Stage::FixedStep.label(), Stage::PreRender.label());
    }
}
