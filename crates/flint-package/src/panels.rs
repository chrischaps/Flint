//! The F3/F4 panels' vocabulary (#1559): what a panel is, where the run
//! keeps them, and what one is lent each frame. It is the kernel's, so a
//! package (rhythm's guide, living's props) and the kernel itself (a
//! game's panel, the expression feed's) can each add a panel without
//! naming the debug-hud package. The debug-hud package makes the engine's
//! panels, lays them out and drives F3 and F4 (`crate::hud::DebugHud`).
//!
//! The panels are the run's [`DebugPanels`] resource, which the player puts
//! in only when its launcher lists a debug HUD: `None` in a build without
//! the debug-hud package, and in a capture.

/// Where the host should dock a panel when rendering it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PanelLayout {
    /// A right-hand side panel (the historical default).
    #[default]
    SideRight,
    /// A full-width bottom strip (timeline-shaped panels).
    Bottom,
}

/// What a panel's [`sync`](DebugPanel::sync) reaches each frame: the world,
/// the renderer and its device, the camera and the particles (#744). The
/// player builds one a frame and hands it to every panel in one loop, so it
/// never asks a panel its name to know what to do with it.
pub struct PanelCtx<'a> {
    pub world: &'a mut flint_ecs::FlintWorld,
    pub renderer: &'a mut flint_render::SceneRenderer,
    pub device: &'a wgpu::Device,
    pub camera: &'a mut flint_render::Camera,
    pub particles: &'a mut flint_particles::ParticleSync,
    /// What the panels tell the player, and the player's own switches.
    pub host: &'a mut PanelHost,
}

/// The player's state that panels read and set: the Rendering & Effects
/// panel's freeze switch and look, a style a panel committed to the scene,
/// whether particles draw. The player reads it back after the loop.
#[derive(Debug, Default)]
pub struct PanelHost {
    /// F4's "Freeze scripts" this frame (`None`: no panel says).
    pub freeze_scripts: Option<bool>,
    /// A look to switch to (`Some(None)`: the scene's own).
    pub look: Option<Option<String>>,
    /// Seconds to fade to `look` over (#496); 0 cuts.
    pub look_seconds: f64,
    /// The scene's authored styles after a panel wrote one into the file.
    pub committed_styles: Option<flint_render::stylize::StyleSettings>,
    /// Whether particles draw (the Particles panel's switch).
    pub particles_render: bool,
}

/// The player's F3/F4 panels, in its run resources (ADR 0137 decision 2,
/// #1550): the registration point a subsystem asks for with
/// `resources.get_mut::<DebugPanels>()` to add its own panel. `None` in a
/// build without the debug HUD and in a capture.
#[derive(Default)]
pub struct DebugPanels(pub Vec<Box<dyn DebugPanel>>);

impl std::ops::Deref for DebugPanels {
    type Target = Vec<Box<dyn DebugPanel>>;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl std::ops::DerefMut for DebugPanels {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

/// Common interface for debug overlay panels.
/// The player app holds them in a [`DebugPanels`] and renders them
/// generically.
pub trait DebugPanel {
    /// Panel identifier used as egui ID and display title.
    fn name(&self) -> &str;

    /// Render the panel contents into the provided egui Ui.
    fn ui(&mut self, ui: &mut egui::Ui);

    /// Whether the panel is currently visible.
    fn is_open(&self) -> bool;

    /// Toggle visibility.
    fn toggle(&mut self);

    /// How the host should dock this panel (defaulted so existing panels
    /// keep the side-panel layout untouched).
    fn layout(&self) -> PanelLayout {
        PanelLayout::default()
    }

    /// Whether the panel is the player's, not the scene's: it outlives a
    /// scene transition, open or closed as it was (#268). Rendering &
    /// Effects (F4) and a game's panels (ADR 0107) are; the default is a
    /// scene's panel, which the next scene's panels replace.
    fn outlives_scene(&self) -> bool {
        false
    }

    /// Once a frame, before the panels draw: apply what the panel's widgets
    /// changed and read back what it shows. The default does nothing (a
    /// panel the host fills itself, a game's).
    fn sync(&mut self, _ctx: &mut PanelCtx) {}

    /// Paint what the panel shows in the world (the Living Props pose
    /// overlay and a picked tree's marks, #218, #248), in the HUD pass under
    /// every panel, with the frame's view-projection. The default paints
    /// nothing.
    fn paint_world(&mut self, _ctx: &egui::Context, _view_proj: &[[f32; 4]; 4]) {}

    /// About how many widget rows tall the panel is, for balancing the F3
    /// columns.
    fn weight(&self) -> u32 {
        6
    }

    /// Returns true if the panel has unapplied changes.
    fn is_dirty(&self) -> bool;

    /// Clear the dirty flag after changes have been applied by the host.
    fn clear_dirty(&mut self);

    /// Downcast support for accessing concrete panel types.
    fn as_any(&self) -> &dyn std::any::Any;
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any;
}

/// A package's F3 panel for a scene that uses it (#1558, ADR 0137 section
/// 4's "debug-hud's registration point"): `None` when the scene doesn't.
pub type ScenePanel = fn(&flint_ecs::FlintWorld) -> Option<Box<dyn DebugPanel>>;

/// The registration point packages add their scene panels to: the kernel
/// inserts it, and the player makes each at a scene's panels, in the order
/// added, with the package it belongs to (a project's `[packages]` that
/// leaves the package out gets none). Living's Living Props panel is one.
#[derive(Default)]
pub struct ScenePanels(pub Vec<(&'static str, ScenePanel)>);
