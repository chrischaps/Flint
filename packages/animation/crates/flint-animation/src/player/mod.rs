//! The animation package's player part: what a launcher lists
//! ([`PACKAGE`]). Its subsystem plays every scene's clips, and its script
//! modules and the clip-kind point are registered with it.
//!
//! The frame (ADR 0139, #1620), each where the player ran it before the
//! package was one:
//!
//! | when | what |
//! |---|---|
//! | the scene's build | skeletons and clips from the scene's models, `.anim.toml`, the registered kinds' clips (`.sprite.toml`), `.sequence.toml`, then the first sync |
//! | `Stage::Tweens` | `tween`'s field tweens, while the game state runs scripts |
//! | `Stage::Animate` | property clips, sequences, skeletal and node clips, then the registered kinds; one-shot ends and sequence cues to the scripts |
//! | `Stage::Pose` | the bones to the GPU, bone probes, then two-bone IK |
//! | `late_models` | a spawned model's skeleton and clips |
//!
//! A capture (`flint render --anim-time`) runs no `Animate` or `Pose`: its
//! animation is a [`HeadlessAnimation`] the command makes.

mod capture;
pub mod loading;
mod module;
mod subsystem;
mod tween_module;

use crate::kind::ClipKind;
use flint_core::components as comp;
use flint_core::project::Project;
use flint_package::{Package, Registry, SceneCtx};

pub use capture::{publish_bone_probes, HeadlessAnimation};
pub use module::{AnimationModule, ANIMATION_MODULE};
pub use subsystem::Animation;
pub use tween_module::{TweenField, TweenModule, TWEEN_MODULE};

/// The package's name, as its manifest gives it.
pub const PACKAGE_NAME: &str = "animation";

/// Makes a scene's kind of clip: its clips loaded from the files the scene
/// can see (`project`, `scene_path`), each registered told to `say` (the
/// player's load log, `flint render`'s report).
pub type MakeKind = fn(&Project, &str, &mut dyn FnMut(String)) -> Box<dyn ClipKind>;

/// The point a package that plays its own kind of clip fills (ADR 0139
/// section 5, `Registry::insert_point`): the flipbook package's
/// sprite-sheet clips. Each scene's animation, and each capture's, makes
/// every kind here, in the order the packages registered (requires first).
#[derive(Default)]
pub struct ClipKinds(pub Vec<(&'static str, MakeKind)>);

impl ClipKinds {
    /// Every kind, made for the scene at `scene_path`.
    pub fn make(
        &self,
        project: &Project,
        scene_path: &str,
        say: &mut dyn FnMut(String),
    ) -> Vec<Box<dyn ClipKind>> {
        self.0
            .iter()
            .map(|(_, make)| make(project, scene_path, say))
            .collect()
    }
}

/// The `animation` package (`packages/animation/package.toml`).
pub static PACKAGE: AnimationPackage = AnimationPackage;

pub struct AnimationPackage;

impl Package for AnimationPackage {
    fn name(&self) -> &'static str {
        PACKAGE_NAME
    }

    /// Its builder (every scene has one: `tween` and the `animation`
    /// module are any scene's), its policy name, its line for a project
    /// that leaves it out, its script modules and the point kinds fill.
    fn register(&self, registry: &mut Registry) {
        registry
            .scene_subsystem(subsystem::for_scene)
            .policy(crate::POLICY)
            .when_left_out(left_out)
            .script_modules(&[&ANIMATION_MODULE, &TWEEN_MODULE])
            .insert_point(ClipKinds::default());
    }
}

/// What a scene with animators hears when the project's `[packages]`
/// leaves `animation` out.
fn left_out(ctx: &SceneCtx) -> Option<String> {
    let in_use = ctx.packages?;
    let uses: Vec<&str> = [
        comp::ANIMATOR,
        comp::SKELETON,
        comp::IK_TWO_BONE,
        comp::BONE_PROBE,
    ]
    .into_iter()
    .filter(|c| !ctx.world.entities_with_component(c).is_empty())
    .collect();
    flint_package::left_out_line(in_use, PACKAGE_NAME, &uses, "hold still")
}
