//! The animation package's player part: what a launcher lists
//! ([`PACKAGE`]). Its subsystem plays every scene's clips, and its script
//! modules and the clip-kind point are registered with it.
//!
//! The frame (ADR 0139, #1620, #1621), each where the player ran it before
//! the package was one:
//!
//! | when | what |
//! |---|---|
//! | the scene's build | `.anim.toml`, the registered kinds' clips (the rig's from the scene's models, the flipbook's `.sprite.toml`), `.sequence.toml`, then the first sync |
//! | `Stage::Tweens` | `tween`'s field tweens, while the game state runs scripts |
//! | `Stage::Animate` | property clips, sequences, then the registered kinds by slot (skeletal, node, flipbook); one-shot ends and sequence cues to the scripts |
//! | `Stage::Pose` | the kinds' skins to the GPU, then each kind's pose (the rig's bone probes and two-bone IK) |
//! | `late_models` | a spawned model, to each kind that takes one (the rig's skeleton and clips) |
//!
//! A capture (`flint render --anim-time`) runs no `Animate` or `Pose`: its
//! animation is a [`HeadlessAnimation`], which the `CapturePoses` point
//! this package fills makes.

mod capture;
pub mod loading;
mod module;
mod subsystem;
mod tween_module;

use crate::kind::ClipKind;
use crate::AnimationSystem;
use flint_core::components as comp;
use flint_core::project::Project;
use flint_package::{CapturePose, CapturePoses, Package, Registry, SceneCtx};
use flint_render::model_loader::ModelLoadResult;

pub use capture::HeadlessAnimation;
pub use module::{AnimationModule, ANIMATION_MODULE};
pub use subsystem::Animation;
pub use tween_module::{TweenField, TweenModule, TWEEN_MODULE};

/// The package's name, as its manifest gives it.
pub const PACKAGE_NAME: &str = "animation";

/// What a kind's maker is handed: the scene it makes clips for.
pub struct KindCtx<'a> {
    pub project: &'a Project,
    pub scene_path: &'a str,
    /// The scene's models, as the model loader made them (the rig's
    /// skeletons and clips are theirs); none for a host that loads no model.
    pub models: Option<&'a ModelLoadResult>,
}

/// What a kind's maker says, a line each, with what the line is about
/// (`skeletal_clip`, `sprite_clip`...): the player's load log, `flint
/// render`'s report.
pub type Say<'a> = dyn FnMut(&'static str, String) + 'a;

/// Makes a scene's kind of clip, its clips loaded from what the scene can
/// see, each registered told to `say`.
pub type MakeKind = fn(&KindCtx, &mut Say) -> Box<dyn ClipKind>;

/// Hands a model spawned after the scene's build (#1107) to the kinds that
/// play models' clips, found in the scene's animation by type
/// ([`AnimationSystem::kind_of_mut`]).
pub type LateModels = fn(&mut AnimationSystem, &ModelLoadResult, &mut Say);

/// A kind a package registered on [`ClipKinds`].
#[derive(Clone, Copy)]
pub struct KindEntry {
    /// The package that registered it: a scene of a project that leaves
    /// the package out makes none of its kinds.
    pub package: &'static str,
    /// The kind's name, as [`ClipKind::name`] says it.
    pub name: &'static str,
    pub make: MakeKind,
    /// What it does with a model spawned later, if it plays models' clips.
    pub late: Option<LateModels>,
}

/// The point a package that plays its own kind of clip fills (ADR 0139
/// section 5, `Registry::insert_point`): the rig package's skeletal and node
/// clips, the flipbook package's sprite-sheet clips. Each scene's animation,
/// and each capture's, makes every kind here, in the order the packages
/// registered (requires first); each then runs at its slot.
#[derive(Default)]
pub struct ClipKinds(pub Vec<KindEntry>);

impl ClipKinds {
    /// The kinds of the packages `keep` says yes to.
    pub fn of(&self, keep: impl Fn(&str) -> bool) -> ClipKinds {
        ClipKinds(self.0.iter().filter(|k| keep(k.package)).copied().collect())
    }

    /// Every kind, made for the scene `ctx` names.
    pub fn make(&self, ctx: &KindCtx, say: &mut Say) -> Vec<Box<dyn ClipKind>> {
        self.0.iter().map(|k| (k.make)(ctx, say)).collect()
    }

    /// A late model, to every kind that takes one.
    pub fn late(&self, system: &mut AnimationSystem, models: &ModelLoadResult, say: &mut Say) {
        for late in self.0.iter().filter_map(|k| k.late) {
            late(system, models, say);
        }
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
    /// that leaves it out, its script modules, the point kinds fill and the
    /// capture's animation (`flint render --anim-time`).
    fn register(&self, registry: &mut Registry) {
        registry
            .scene_subsystem(subsystem::for_scene)
            .policy(crate::POLICY)
            .when_left_out(left_out)
            .script_modules(&[&ANIMATION_MODULE, &TWEEN_MODULE])
            .insert_point(ClipKinds::default())
            .insert_point(CapturePoses(capture_pose));
    }
}

/// A capture's animation, with the kinds the run's packages registered.
fn capture_pose(
    models: &ModelLoadResult,
    project: &Project,
    scene_path: &str,
    registry: &Registry,
) -> Box<dyn CapturePose> {
    let kinds = registry.point::<ClipKinds>();
    Box::new(HeadlessAnimation::load(models, project, scene_path, kinds))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn no_kind(_: &KindCtx, _: &mut Say) -> Box<dyn ClipKind> {
        unreachable!("never made here")
    }

    fn entry(package: &'static str, name: &'static str) -> KindEntry {
        KindEntry {
            package,
            name,
            make: no_kind,
            late: None,
        }
    }

    /// A project that leaves a package out gets none of its kinds (#1621):
    /// `rig = false` plays no skeleton in a build that has the rig.
    #[test]
    fn a_left_out_packages_kinds_are_not_made() {
        let all = ClipKinds(vec![
            entry("rig", "skeletal"),
            entry("rig", "node"),
            entry("flipbook", "flipbook"),
        ]);
        let kept = all.of(|package| package != "rig");
        let names: Vec<&str> = kept.0.iter().map(|k| k.name).collect();
        assert_eq!(names, ["flipbook"]);
        assert_eq!(all.of(|_| true).0.len(), 3);
    }
}

/// What a scene with animators hears when the project's `[packages]`
/// leaves `animation` out (the rig's components are the rig's to name).
fn left_out(ctx: &SceneCtx) -> Option<String> {
    let in_use = ctx.packages?;
    let uses: Vec<&str> = [comp::ANIMATOR]
        .into_iter()
        .filter(|c| !ctx.world.entities_with_component(c).is_empty())
        .collect();
    flint_package::left_out_line(in_use, PACKAGE_NAME, &uses, "hold still")
}
