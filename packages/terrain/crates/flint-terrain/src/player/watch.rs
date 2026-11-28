//! A terrain made from its recipe, rebaked when the recipe is saved (#1697).
//!
//! The subsystem keeps one [`SpecWatch`] for a scene whose terrain names its
//! `spec`. At `Stage::PreScripts` it asks `flint_core::file_watch` about the
//! spec every [`POLL_FRAMES`] frames (the player's watcher thread answers,
//! so a look never waits on the disk, #633), and on a change reads and bakes
//! it on a thread of its own: the cache answers at once when nothing the
//! bake reads changed (a texture, the grass), and a real bake doesn't stall
//! the frames. When the thread is done the terrain is built again from the
//! component over the new recipe ([`made_from`], [`build_terrain`]): chunks,
//! material, grass, the scene's ground in the run's resources, the
//! `terrain` module's heights, and the kernel's `Surfaces` put in again,
//! which the physics package takes at `Stage::Scripts` the same frame and
//! makes the heightfield of. A spec that no longer reads keeps the terrain
//! as it was, with a warning.

use crate::player::TerrainHeight;
use crate::scene::build_terrain;
use crate::source::{self, made_from, Recipe};
use flint_core::components as comp;
use flint_core::file_watch::{self, Stamp};
use flint_core::EntityId;
use flint_package::bind::SceneGround;
use flint_package::surfaces::{SurfaceGrid, Surfaces};
use flint_package::{SceneCtx, StageCtx};
use std::path::{Path, PathBuf};
use std::thread::JoinHandle;

/// Frames between two looks at the spec: a quarter second at 60 Hz.
pub const POLL_FRAMES: u32 = 15;

/// The recipe of the scene's terrain, and a bake under way.
pub struct SpecWatch {
    entity: EntityId,
    name: String,
    path: PathBuf,
    stamp: Option<Stamp>,
    /// No `grass_ground` holds the scene's grass, so the terrain's is its own.
    grass_free: bool,
    frames: u32,
    job: Option<JoinHandle<Result<Recipe, String>>>,
}

impl SpecWatch {
    /// The watch for the scene's terrain, when the binder made one from a
    /// recipe and the player watches files (not under `--no-hot-reload`).
    pub fn of(ctx: &SceneCtx) -> Option<Self> {
        if file_watch::mode() == file_watch::Mode::Off {
            return None;
        }
        let ground = ctx.resources.get::<SceneGround>()?;
        let view = ctx.world.view(ground.entity, comp::TERRAIN);
        let scene_dir = Path::new(ctx.scene_path).parent().unwrap_or(Path::new(""));
        let path = source::spec_path(&view, ctx.project, scene_dir)?.ok()?;
        Some(Self {
            entity: ground.entity,
            name: ground.name.clone(),
            stamp: file_watch::stamp(&path),
            path,
            grass_free: ctx
                .world
                .entities_with_component(comp::GRASS_GROUND)
                .is_empty(),
            frames: 0,
            job: None,
        })
    }

    /// One frame's look: a finished bake built in, else (every
    /// [`POLL_FRAMES`]) a changed spec sent to be baked.
    pub fn poll(&mut self, ctx: &mut StageCtx) {
        if self.job.as_ref().is_some_and(|job| job.is_finished()) {
            let job = self.job.take().expect("checked above");
            let result = job
                .join()
                .unwrap_or_else(|_| Err("the bake panicked".to_string()));
            self.build(result, ctx);
            return;
        }
        self.frames = self.frames.wrapping_add(1);
        if self.job.is_some() || self.frames % POLL_FRAMES != 0 {
            return;
        }
        let now = file_watch::stamp(&self.path);
        if now == self.stamp || now.is_none() {
            return;
        }
        self.stamp = now;
        let seed = ctx
            .world
            .view(self.entity, comp::TERRAIN)
            .opt_i64("seed")
            .map(|s| s as u64);
        let disk = source::cache_dir(ctx.project, scene_dir(ctx.scene_path));
        let path = self.path.clone();
        tracing::info!(
            target: "player",
            "[terrain] {} saved: baking it again",
            flint_core::project::shown(&path)
        );
        self.job = Some(std::thread::spawn(move || {
            Recipe::bake(&path, seed, disk.as_deref())
        }));
    }

    /// The terrain built again from the new recipe, or left as it was.
    fn build(&self, result: Result<Recipe, String>, ctx: &mut StageCtx) {
        let shown = flint_core::project::shown(&self.path);
        let recipe = match result {
            Ok(recipe) => recipe,
            Err(e) => {
                tracing::warn!("[terrain] {shown}: {e}; the terrain stays as it was");
                return;
            }
        };
        for issue in &recipe.source.issues {
            tracing::warn!("[terrain] {shown}: {issue}");
        }
        let words = recipe.words();
        let view = ctx.world.view(self.entity, comp::TERRAIN);
        let made = match made_from(&view, Some(recipe), ctx.project, scene_dir(ctx.scene_path)) {
            Ok(made) => made,
            Err(e) => {
                tracing::warn!("[terrain] {shown}: {e}; the terrain stays as it was");
                return;
            }
        };
        let (Some(renderer), Some((device, queue))) = (ctx.renderer.as_deref_mut(), ctx.gpu) else {
            return;
        };
        let (loaded, _grass) = build_terrain(
            renderer,
            device,
            queue,
            ctx.world,
            self.entity,
            &self.name,
            made,
            self.grass_free,
        );
        let ground = loaded.ground();
        ctx.script
            .set_module_state(TerrainHeight::of(Some(&ground)));
        ctx.resources.insert(Surfaces {
            meshes: Vec::new(),
            grids: vec![SurfaceGrid::of_ground(&ground)],
        });
        ctx.resources.insert(ground);
        tracing::info!(target: "player", "[terrain] '{}' rebuilt from {words}", self.name);
    }
}

fn scene_dir(scene_path: &str) -> &Path {
    Path::new(scene_path).parent().unwrap_or(Path::new(""))
}
