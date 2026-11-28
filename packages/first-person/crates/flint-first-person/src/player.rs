//! The first-person package's player part (#1618, ADR 0138 decision 4):
//! what a launcher lists ([`PACKAGE`]). Its controller's state lives in its
//! subsystem, built for a scene that has a player, and it writes the
//! kernel's [`PlayerView`].
//!
//! The frame, as ADR 0138 decision 2 lays it out. Physics comes first in
//! the registry's order (requires first), so each stage meets physics'
//! share of it done:
//!
//! | when | what |
//! |---|---|
//! | the builder (the scene's entry, after physics made the bodies) | the player found and facing its `start_yaw`, its body driven by the controller, and the view: the player, mouse look, the six actions |
//! | `Stage::Scripts` | the look and the jump press, once a frame, while the state runs physics (after physics' `begin_frame`) |
//! | step `Control` | the walk (before physics' sync-in) |
//! | `Stage::View` | the player's eye |
//!
//! A capture (`flint render --scripts`) steps no physics: its builder sets
//! the view's player alone, for `on_interact` and
//! `find_nearest_interactable`, and makes no subsystem.

use crate::{CharacterController, PACKAGE_NAME};
use flint_core::components as comp;
use flint_package::{FixedCtx, Package, Registry, SceneCtx, Stage, StageCtx, StepStage, Subsystem};
use flint_runtime::PlayerView;

/// The `first-person` package (`packages/first-person/package.toml`).
pub static PACKAGE: FirstPersonPackage = FirstPersonPackage;

pub struct FirstPersonPackage;

impl Package for FirstPersonPackage {
    fn name(&self) -> &'static str {
        PACKAGE_NAME
    }

    /// Its builder and the line for a project that leaves it out. It has
    /// no policy of its own: the walk pauses with physics.
    fn register(&self, registry: &mut Registry) {
        registry.scene_subsystem(for_scene).when_left_out(left_out);
    }
}

/// The scene's walker, when it has a player.
pub struct FirstPerson {
    controller: CharacterController,
}

impl FirstPerson {
    /// The controller, to read (its yaw, pitch and footing).
    pub fn controller(&self) -> &CharacterController {
        &self.controller
    }
}

/// The subsystem for the scene being entered: one when the scene has a
/// player, built after physics' (which made the bodies), so the cursor and
/// the controls line at the end of the entry read the view this writes
/// before the first frame (#1614's risk).
fn for_scene(ctx: &mut SceneCtx) -> Option<Box<dyn Subsystem>> {
    if ctx.capture.is_some() {
        // Who the scene is played as, for `on_interact` and
        // `find_nearest_interactable`: the controller's rule alone, no
        // eye, no mouse look
        let player = crate::find_player(ctx.world);
        ctx.resources.get_or_insert_with(PlayerView::default).player = player;
        return None;
    }
    let mut controller = CharacterController::new();
    // Without physics in the run there is nothing to walk on; the package
    // requires it, so only a host that never registered physics gets here
    let physics = flint_physics::of_mut(ctx.resources)?;
    controller.enter(ctx.world, physics)?;
    controller.enter_view(ctx.resources.get_or_insert_with(PlayerView::default));
    Some(Box::new(FirstPerson { controller }))
}

impl Subsystem for FirstPerson {
    fn name(&self) -> &'static str {
        PACKAGE_NAME
    }

    fn update(&mut self, stage: Stage, ctx: &mut StageCtx) {
        match stage {
            // Look and the jump press once a frame, however many steps
            // run: none on most frames above 60 Hz, two at 30 (#728)
            Stage::Scripts => {
                let runs = ctx
                    .policy
                    .is_none_or(|p| p.runs(flint_runtime::state::PHYSICS));
                if runs {
                    self.controller.frame_input(ctx.input, ctx.world);
                }
            }
            // The eye, after the steps (ADR 0138's `View`)
            Stage::View => {
                if let Some(view) = ctx.resources.get_mut::<PlayerView>() {
                    self.controller.place_eye(ctx.world, view);
                }
            }
            _ => {}
        }
    }

    /// The walk, before physics' sync-in.
    fn step(&mut self, stage: StepStage, ctx: &mut FixedCtx) {
        if stage != StepStage::Control {
            return;
        }
        if let Some(physics) = flint_physics::of_mut(ctx.resources) {
            self.controller
                .update(ctx.input, ctx.world, physics, ctx.dt);
        }
    }
}

/// What a scene with a `character_controller` hears when the project's
/// `[packages]` leaves `first-person` out.
fn left_out(ctx: &SceneCtx) -> Option<String> {
    let in_use = ctx.packages?;
    let uses: Vec<&str> = [comp::CHARACTER_CONTROLLER]
        .into_iter()
        .filter(|c| !ctx.world.entities_with_component(c).is_empty())
        .collect();
    flint_package::left_out_line(
        in_use,
        PACKAGE_NAME,
        &uses,
        "stand where they are written: no one walks them or looks through their eyes",
    )
}
