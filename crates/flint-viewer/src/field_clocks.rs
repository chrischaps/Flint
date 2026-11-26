//! The viewer's share of a scene's wooze field (#214, #215, #252): the field
//! and the clocks the viewer keeps on it, the shading boil's, shallow
//! wooze's and woozy grass's, as the player's living subsystem keeps them.
//!
//! All of it is the living package's (#1558): its CLI extension registers
//! [`SceneFields`] on the command line (`flint_package::cli::Points`), the
//! settle of a scene's `[living]` / `[wooze]`, its field and its clocks.
//! Without it the field is never read, every clock says nothing, and a
//! scene with a `[wooze]` table is told so once; the renderer takes the
//! clocks' answers as `flint_core::field_frame` data either way, so
//! `app.rs` reads one API in both builds.

use flint_core::field_frame::{BoilFrame, ShallowWoozeDraw};
use flint_core::EntityId;
use flint_ecs::FlintWorld;
use std::path::Path;

/// A scene's field, as the package that reads it made it.
pub type SceneField = Box<dyn std::any::Any + Send>;

/// The clocks the viewer keeps on a scene's field.
pub trait Clocks {
    /// The scene's entities are new (a reload): their clocks start over.
    fn restart_meshes(&mut self);

    /// Move every clock on by `dt`, with the viewer at `eye`. `grass_comb`
    /// is the grass's `GrassConfig::wooze_comb`, `boil_on` the drawn
    /// config's. With no field the boil is off and nothing else moves.
    fn tick(
        &mut self,
        field: Option<&SceneField>,
        world: &FlintWorld,
        eye: [f32; 3],
        grass_comb: Option<(f32, f32)>,
        boil_on: u32,
        dt: f64,
    );

    /// This frame's boil drawing.
    fn boil(&self) -> BoilFrame;

    /// Woozy grass's sway clock, or `None` for the frame's.
    fn grass_time(&self) -> Option<f32>;

    /// Woozy grass's comb.
    fn grass_comb(&self) -> Option<ShallowWoozeDraw>;

    /// What each shallow-wooze entity draws with, as of the last tick.
    fn shallow_draws(&self) -> &[(EntityId, ShallowWoozeDraw)];
}

/// The viewer's registration point for the package a scene's `[wooze]` is
/// (living's, #1558).
pub struct SceneFields {
    /// Check the package's tables and fill in what they name (the preset
    /// `[wooze]` names), as the player does at load.
    pub settle: fn(&mut flint_scene::SceneFile, &Path) -> flint_core::Result<()>,
    /// The scene's field, naming what's wrong with it and going on without
    /// that part, as the player does; `None` with no `[wooze]`.
    pub field_of: fn(&flint_scene::SceneFile) -> Option<SceneField>,
    /// A fresh set of clocks.
    pub clocks: fn() -> Box<dyn Clocks + Send>,
}

fn point() -> Option<&'static SceneFields> {
    flint_package::cli::points().get::<SceneFields>()
}

/// The package's tables in `scene_file` settled; with no package, as
/// written (#1543: the viewer reads neither table then).
pub(crate) fn settle(
    scene_file: &mut flint_scene::SceneFile,
    path: &Path,
) -> flint_core::Result<()> {
    match point() {
        Some(fields) => (fields.settle)(scene_file, path),
        None => Ok(()),
    }
}

/// The wooze field a scene describes. A viewer without the living package
/// says once that a scene's `[wooze]` stands still.
pub(crate) fn field_of(scene: &flint_scene::SceneFile) -> Option<SceneField> {
    match point() {
        Some(fields) => (fields.field_of)(scene),
        None => {
            if scene.tables.contains_key("wooze") {
                tracing::warn!(
                    target: "viewer",
                    "this viewer is built without the `living` package: the scene's [wooze] \
                     field doesn't boil its lines or sway its meshes and grass"
                );
            }
            None
        }
    }
}

/// The clocks the viewer keeps: the package's, or still ones without it.
pub(crate) struct FieldClocks(Option<Box<dyn Clocks + Send>>);

impl Default for FieldClocks {
    fn default() -> Self {
        Self(point().map(|fields| (fields.clocks)()))
    }
}

impl FieldClocks {
    pub(crate) fn restart_meshes(&mut self) {
        if let Some(clocks) = &mut self.0 {
            clocks.restart_meshes();
        }
    }

    pub(crate) fn tick(
        &mut self,
        field: Option<&SceneField>,
        world: &FlintWorld,
        eye: [f32; 3],
        grass_comb: Option<(f32, f32)>,
        boil_on: u32,
        dt: f64,
    ) {
        if let Some(clocks) = &mut self.0 {
            clocks.tick(field, world, eye, grass_comb, boil_on, dt);
        }
    }

    pub(crate) fn boil(&self) -> BoilFrame {
        self.0.as_ref().map(|c| c.boil()).unwrap_or_default()
    }

    pub(crate) fn grass_time(&self) -> Option<f32> {
        self.0.as_ref().and_then(|c| c.grass_time())
    }

    pub(crate) fn grass_comb(&self) -> Option<ShallowWoozeDraw> {
        self.0.as_ref().and_then(|c| c.grass_comb())
    }

    pub(crate) fn shallow_draws(&self) -> &[(EntityId, ShallowWoozeDraw)] {
        self.0.as_ref().map_or(&[], |c| c.shallow_draws())
    }
}
