//! Assets a package makes by name (ADR 0137 section 4, #1557): the kernel's
//! registration point for whatever turns a `model.asset` or a `texture`
//! name into something the renderer draws before the model loader looks
//! for a file. The procgen package is its one client: a name with a
//! `*.procgen.toml` spec is generated, and the rest stays the file
//! loader's (#32, catalog > procgen > file).
//!
//! The kernel calls an [`AssetMaker`] at five moments of a run, and lends
//! it the parts each needs. It never names what a maker makes, or how: a
//! player built without the package registers no maker, and every name is
//! a file.
//!
//! | Moment | Hook | What the kernel lends |
//! |---|---|---|
//! | the first scene's start, once a run | [`AssetMaker::run_starts`] | [`RunStart`]: resources, project, scene folder, whether a disk tier is wanted |
//! | a scene's load, before its models | [`AssetMaker::scene`] | [`MakeCtx`]: the world, the renderer, the colliders, the resources |
//! | entities spawned since (#1107) | [`AssetMaker::spawned`] | the same, for those ids |
//! | each frame, before `Stage::MadeAssets` | [`AssetMaker::frame`] | [`FrameMake`]: the camera, the renderer when there is one |
//! | a scene's teardown | [`AssetMaker::scene_gone`] | the resources |
//!
//! And [`AssetMaker::pending`], asked when physics wants a model's
//! triangles: a name still being made has none yet.

use crate::{Registry, Resources};
use flint_core::package::InUse;
use flint_core::project::Project;
use flint_core::EntityId;
use flint_ecs::FlintWorld;
use flint_render::model_loader::ModelLoadConfig;
use flint_render::SceneRenderer;
use std::path::Path;

/// When a scene's made assets are made (`LaunchOptions::procgen_mode`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MakeMode {
    /// During the load: the first frame shows the real assets.
    #[default]
    Immediate,
    /// Placeholders at once, the assets on a frame-budgeted queue.
    Deferred,
}

/// The renderer and its device, lent to a maker that uploads.
pub struct Gpu<'a> {
    pub renderer: &'a mut SceneRenderer,
    pub device: &'a wgpu::Device,
    pub queue: &'a wgpu::Queue,
}

/// Where a made mesh's triangles go, for the colliders that name it
/// (#1222): the kernel's physics reads them in place of a file's.
pub trait MadeMeshes {
    /// `name` is now this mesh.
    fn made(&mut self, name: &str, positions: Vec<[f32; 3]>, triangles: Vec<[u32; 3]>);
    /// `name` is no made mesh any more: a placeholder or a file stands there.
    fn forget(&mut self, name: &str);
}

/// What a maker is lent at a scene's load and for spawned entities.
pub struct MakeCtx<'a> {
    pub world: &'a mut FlintWorld,
    pub scene_path: &'a str,
    /// The scene file's top-level tables, as the host settled them: what a
    /// package's own tables say its made assets start from (living's
    /// `[living]`, the props' defaults, #1558).
    pub tables: &'a toml::Table,
    /// The model loader's search: its catalog, the project, the scene's
    /// folder. A name the catalog has is the catalog's.
    pub config: &'a ModelLoadConfig,
    pub gpu: Gpu<'a>,
    pub colliders: &'a mut dyn MadeMeshes,
    pub resources: &'a mut Resources,
    /// What the launcher registered: another package's registration points
    /// ([`Registry::point`]).
    pub registry: &'a Registry,
    /// The packages the project's `[packages]` lets in; `None`: every one.
    pub packages: Option<&'a InUse>,
    pub mode: MakeMode,
}

impl MakeCtx<'_> {
    /// The project's `[packages]` leaves `package` out.
    pub fn leaves_out(&self, package: &str) -> bool {
        self.packages.is_some_and(|in_use| !in_use.is_on(package))
    }
}

/// What a maker is lent each frame.
pub struct FrameMake<'a> {
    pub resources: &'a mut Resources,
    /// Where the camera stands: what is nearest is made first.
    pub camera: [f32; 3],
    pub project: &'a Project,
    pub scene_path: &'a str,
    /// `None` on a host with no renderer (a headless test): what the queue
    /// finished is made and dropped.
    pub gpu: Option<Gpu<'a>>,
    pub colliders: &'a mut dyn MadeMeshes,
}

/// What a maker is lent once, before the first scene's assets.
pub struct RunStart<'a> {
    pub resources: &'a mut Resources,
    pub project: &'a Project,
    pub scene_dir: &'a Path,
    /// The host keeps made assets on disk between runs
    /// (`LaunchOptions::procgen_disk_cache`).
    pub disk: bool,
}

/// A package that makes assets by name. Registered with
/// [`Registry::asset_maker`]; every hook but [`scene`](Self::scene)
/// defaults to nothing.
pub trait AssetMaker: Sync {
    /// Once a run, before the first scene's assets (the disk tier, #181).
    fn run_starts(&self, _ctx: &mut RunStart) {}

    /// A scene's load, before the model loader: make every name the
    /// scene's entities ask for that this maker can make.
    fn scene(&self, ctx: &mut MakeCtx);

    /// Entities spawned since the scene's load (`ids` alone, #1107).
    fn spawned(&self, _ctx: &mut MakeCtx, _ids: &[EntityId]) {}

    /// Each frame, before `Stage::MadeAssets`: what a queue finished goes up.
    fn frame(&self, _ctx: &mut FrameMake) {}

    /// Whether `name` is still being made (queued behind a placeholder).
    fn pending(&self, _resources: &Resources, _name: &str) -> bool {
        false
    }

    /// The scene is going: drop what was queued for it.
    fn scene_gone(&self, _resources: &mut Resources) {}
}
