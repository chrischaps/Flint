//! The terrain package's player part (#1623, ADR 0138 decision 5): what a
//! launcher lists ([`PACKAGE`]).
//!
//! | when | what |
//! |---|---|
//! | the scene's bind (the kernel's scene binder, every host) | [`scene::bind_terrain`](crate::scene::bind_terrain): the first `terrain`'s chunks, material and grass built into the renderer, and its [`SceneGround`] handed back |
//! | the player, after the bind | physics' static heightfield over the ground's grid (the kernel's, until physics is a package, #1617) |
//! | the scene's build | the [`TerrainSubsystem`], which hands `terrain_height` the ground the host kept in its resources |
//!
//! Every scene builds the subsystem, so `terrain_height` answers 0.0 in a
//! scene without a terrain, as it always did, and a capture (which binds no
//! ground into its game's resources) reads 0.0 too.

mod live;
mod module;
mod watch;

use crate::scene::bind_terrain;
use flint_core::components as comp;
use flint_package::bind::{BindCtx, SceneGround};
use flint_package::{Package, Registry, SceneCtx, Stage, StageCtx, Subsystem};
use std::path::{Path, PathBuf};

pub use live::LIVE_FIELDS;
pub use module::{TerrainHeight, TerrainModule, TERRAIN_MODULE};

pub use crate::PACKAGE_NAME;

/// The `terrain` package (`packages/terrain/package.toml`).
pub static PACKAGE: TerrainPackage = TerrainPackage;

pub struct TerrainPackage;

impl Package for TerrainPackage {
    fn name(&self) -> &'static str {
        PACKAGE_NAME
    }

    /// Its scene binder, its subsystem (every scene has one), its line for
    /// a project that leaves it out and its script module.
    fn register(&self, registry: &mut Registry) {
        registry
            .scene_binder(bind)
            .scene_subsystem(for_scene)
            .when_left_out(left_out)
            .script_modules(&[&TERRAIN_MODULE]);
    }
}

/// The scene binder: the scene's first terrain built into the renderer the
/// scene was just bound into, and its ground for the host.
pub fn bind(ctx: &mut BindCtx) -> Option<SceneGround> {
    let loaded = bind_terrain(
        ctx.gpu.renderer,
        ctx.gpu.device,
        ctx.gpu.queue,
        ctx.world,
        ctx.bound,
    )?;
    Some(loaded.ground())
}

/// What a scene with a terrain hears when the project's `[packages]` leaves
/// `terrain` out.
fn left_out(ctx: &SceneCtx) -> Option<String> {
    let in_use = ctx.packages?;
    let uses: Vec<&str> = [comp::TERRAIN]
        .into_iter()
        .filter(|c| !ctx.world.entities_with_component(c).is_empty())
        .collect();
    flint_package::left_out_line(
        in_use,
        PACKAGE_NAME,
        &uses,
        "draw nothing and hold nothing up",
    )
}

/// Every scene has one: `terrain_height` is any scene's. In the player,
/// one whose terrain is made from its recipe watches it (#1697).
pub fn for_scene(ctx: &mut SceneCtx) -> Option<Box<dyn Subsystem>> {
    let height = TerrainHeight::of(ctx.resources.get::<SceneGround>());
    ctx.script.set_module_state(height);
    let scene_dir = Path::new(ctx.scene_path).parent().unwrap_or(Path::new(""));
    let live = live::LiveTerrain::of(ctx.world, ctx.project, scene_dir);
    let watch = if ctx.capture.is_none() {
        watch::SpecWatch::of(ctx)
    } else {
        None
    };
    Some(Box::new(TerrainSubsystem {
        live,
        watch,
        scene_dir: scene_dir.to_path_buf(),
    }))
}

/// The scene's terrain, as its scripts read it (the `terrain` module) and
/// change it: what they set on its material and grass reaches the renderer
/// at the frame's sync (#1698, [`live`]); and, in the player, the watch on
/// its recipe, which rebakes it when the recipe is saved (#1697, [`watch`]).
pub struct TerrainSubsystem {
    live: Option<live::LiveTerrain>,
    watch: Option<watch::SpecWatch>,
    scene_dir: PathBuf,
}

impl Subsystem for TerrainSubsystem {
    fn name(&self) -> &'static str {
        PACKAGE_NAME
    }

    fn script_modules(&self) -> &[&'static str] {
        &[module::MODULE]
    }

    /// Before the scripts, the recipe (#1697): a rebake lands here, before
    /// physics' frame, which takes the new ground the same frame, and the
    /// live reread takes the new recipe as its base. After every script of
    /// the frame, before it is drawn, what they changed on the terrain goes
    /// to the renderer (#1698).
    fn update(&mut self, stage: Stage, ctx: &mut StageCtx) {
        if stage == Stage::PreScripts {
            let rebuilt = self.watch.as_mut().and_then(|watch| watch.poll(ctx));
            if let (Some(recipe), Some(live)) = (rebuilt, &mut self.live) {
                live.rebase(ctx.world, Some(&recipe), &self.scene_dir);
            }
            return;
        }
        if stage != Stage::Sync {
            return;
        }
        if let Some(live) = &mut self.live {
            let ground = ctx.resources.get::<SceneGround>();
            live.sync(ctx.world, ctx.renderer.as_deref_mut(), ctx.gpu, ground);
        }
    }

    /// The scene's heights go with it.
    fn scene_unloading(&mut self, ctx: &mut SceneCtx) {
        ctx.script.set_module_state(TerrainHeight::default());
        self.watch = None;
    }
}
