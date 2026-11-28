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
    /// After the fixed steps, before the camera follows the player: where
    /// a controller places the eye in the kernel's `PlayerView` (ADR 0138
    /// decision 2; physics' character controller until the first-person
    /// package takes it, #1618, and the vehicle package's chase camera,
    /// #1631). The kernel clears the view's last eye just before, and
    /// moves the camera and the listener to the eye straight after.
    View,
    /// After the camera followed the frame's eye.
    Physics,
    /// After the style tweens move on with game time, before the frame's
    /// script commands: where the animation package moves `tween`'s field
    /// tweens (#734, #1620), while the game state lets scripts run.
    Tweens,
    /// After script commands and camera overrides: the camera is final for
    /// the frame.
    PostPhysics,
    /// After `on_draw_ui` and the draw commands.
    ScriptUi,
    /// After the frame's sound, before the bones go to the GPU: the clock
    /// clips run on (the animation package's clips, layers, sequences and
    /// the kinds other packages register, flipbooks among them), and one-shot
    /// ends and sequence cues to the scripts (ADR 0139, #1620). A capture
    /// keeps its own animation and doesn't run it.
    Animate,
    /// After the bones go to the GPU, before the particles: bone probes,
    /// then two-bone IK, the last writers of transforms (ADR 0138's `Pose`).
    /// A capture doesn't run it.
    Pose,
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
            Stage::View => "view",
            Stage::Physics => "physics",
            Stage::Tweens => "tweens",
            Stage::PostPhysics => "post_physics",
            Stage::ScriptUi => "script_ui",
            Stage::Animate => "animate",
            Stage::Pose => "pose",
            Stage::Animation => "animation",
            Stage::MadeAssets => "made_assets",
            Stage::Sync => "sync",
            Stage::PreRender => "pre_render",
        }
    }

    /// Every stage `update` hears in the player's frame, in order: the
    /// host's [`Stage::Clock`], then the core's.
    pub const UPDATES: [Stage; 14] = [
        Stage::Clock,
        Stage::Input,
        Stage::PreScripts,
        Stage::Scripts,
        Stage::View,
        Stage::Physics,
        Stage::Tweens,
        Stage::PostPhysics,
        Stage::ScriptUi,
        Stage::Animate,
        Stage::Pose,
        Stage::Animation,
        Stage::MadeAssets,
        Stage::Sync,
    ];
}

/// Where in each fixed step a subsystem's work goes (ADR 0138 decision 2,
/// #1617), heard through [`Subsystem::step`](crate::Subsystem::step) in
/// this order, each stage over every subsystem before the next:
///
/// | Step stage | Who |
/// |---|---|
/// | `Control` | a controller's walk (physics' character controller, until the first-person package, #1618) |
/// | `SyncIn` | physics: new bodies and joints, kinematic poses, colliders, joint targets |
/// | `Forces` | every `fixed_update` (vehicle, a game's own) |
/// | `Integrate` | physics: Rapier's step, then the step's contacts onto the bus |
///
/// A step runs only while the game state runs physics, and a capture runs
/// none.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum StepStage {
    Control,
    SyncIn,
    Forces,
    Integrate,
}

impl StepStage {
    /// Every step stage, in the order a step runs them.
    pub const ALL: [StepStage; 4] = [
        StepStage::Control,
        StepStage::SyncIn,
        StepStage::Forces,
        StepStage::Integrate,
    ];

    /// The stage's name, for a profiler scope's detail.
    pub fn label(self) -> &'static str {
        match self {
            StepStage::Control => "control",
            StepStage::SyncIn => "sync_in",
            StepStage::Forces => "forces",
            StepStage::Integrate => "integrate",
        }
    }
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
