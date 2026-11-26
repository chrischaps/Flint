//! The debug HUD's seam (#1559): what the player asks of the debug-hud
//! package, which makes the engine's F3/F4 panels, lays them out, drives F3
//! and F4, paints the error cards and draws F2's frame-time graph.
//!
//! A launcher that lists the package registers one [`DebugHud`] maker
//! ([`Registry::debug_hud`](crate::Registry::debug_hud)). The player makes
//! it once a run and puts the run's [`DebugPanels`] in its resources at the
//! same time; with no maker there are neither, every panel a subsystem
//! would add has nowhere to go, and F3, F4 and the graph are not offered.
//!
//! | Moment | Hook | What the player lends |
//! |---|---|---|
//! | a scene's panels, after its renderer share | [`DebugHud::scene_panels`] | [`HudScene`]: the world, the grass the scene bound, its authored styles, looks, schemas, the packages' panels |
//! | a look changed (#356) | [`DebugHud::look_changed`] | the authored styles under it, its name |
//! | once a frame, before the renderer prepares | [`DebugHud::sync`] | [`PanelCtx`]: world, renderer, device, camera, particles, the panels' answers |
//! | the script's world lines are in | [`DebugHud::world_lines`] | the world and the camera, the frame's lines |
//! | F3, F4 | [`DebugHud::toggle`] | which |
//! | the HUD pass | [`DebugHud::ui`], [`DebugHud::cards`], [`DebugHud::graph`] | egui, the panels, a drawer for the player's own panels |
//!
//! What the hooks read of the player is named here, field by field: the
//! frame history and the hitch detector stay the player's, which hands the
//! graph its frames as [`GraphFrame`]s and its counters as
//! [`HitchCounters`], kernel types.

use crate::panels::{DebugPanel, DebugPanels, PanelCtx};
use flint_core::world_line::WorldLine;
use flint_ecs::FlintWorld;
use flint_render::stylize::{StyleRegistry, StyleSettings};

/// Makes the run's debug HUD: what the debug-hud package registers.
pub type MakeHud = fn() -> Box<dyn DebugHud>;

/// The debug HUD a package brings. Its state is its own (a widget's mesh,
/// what it remembers between frames); the panels are the run's
/// [`DebugPanels`], lent to each hook, since subsystems add theirs too.
pub trait DebugHud: Send {
    /// A scene came up: the last scene's panels go (those that don't
    /// [outlive a scene](DebugPanel::outlives_scene)), this one's are made.
    fn scene_panels(&mut self, panels: &mut DebugPanels, scene: HudScene);

    /// The look changed (#356): F4 shows it as the one in use, and the
    /// style editors' Authored button and markers follow the scene's keys
    /// under it.
    fn look_changed(
        &mut self,
        panels: &mut DebugPanels,
        authored: &StyleSettings,
        current: Option<&str>,
    );

    /// Once a frame, before the renderer prepares: every panel applies its
    /// edits and reads back what it shows (#744). What the panels tell the
    /// player comes back in `ctx.host`.
    fn sync(&mut self, panels: &mut DebugPanels, ctx: &mut PanelCtx);

    /// F3 or F4 was pressed: `Some(open)` for whether the panels it toggled
    /// are open now, `None` when it had none to toggle.
    fn toggle(&mut self, panels: &mut DebugPanels, which: Toggle) -> Option<bool>;

    /// Lines a panel draws in the world this frame (the ocean's parity
    /// probe, #749), added to `out`.
    fn world_lines(&mut self, panels: &mut DebugPanels, view: &WorldView, out: &mut Vec<WorldLine>);

    /// The HUD pass's panels: what each paints in the world, then the open
    /// ones in their columns and strips, above everything a game draws.
    /// Returns the width the columns take at the right edge.
    fn ui(&mut self, ui: HudUi) -> f32;

    /// The cards in the bottom corner: the scripts that fail (#693), the
    /// styles that don't compile or load (#275), what a style's cap left
    /// out (#1246).
    fn cards(&mut self, ctx: &egui::Context, cards: &Cards);

    /// F2's frame-time graph (#1078) and its counters.
    fn graph(&mut self, ui: &mut egui::Ui, graph: &GraphView);
}

/// The two panel keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Toggle {
    /// F3: the scene's panels (and the game's), not Rendering & Effects.
    Scene,
    /// F4: Rendering & Effects (ADR 0053).
    Render,
}

/// The grass the scene's renderer share bound, for its panel.
pub struct GrassPanel {
    pub config: flint_terrain::GrassConfig,
    /// The entity holding the `grass.*` fields.
    pub entity: String,
    /// Which component holds them: `terrain` or `grass_ground`.
    pub component: &'static str,
}

/// What [`DebugHud::scene_panels`] is lent: the scene as the player brought
/// it up.
pub struct HudScene<'a> {
    pub world: &'a FlintWorld,
    pub scene_path: &'a str,
    pub grass: Option<GrassPanel>,
    /// The style settings the scene authors (its `[post_process.styles]`),
    /// for the style editors' Authored button and markers (#268).
    pub authored: StyleSettings,
    /// Where the style editors find palettes and glyph sets (#322).
    pub assets: flint_core::project::AssetScope,
    /// The looks the scene's project offers, and the one in use.
    pub looks: Vec<flint_render::look::Look>,
    pub current_look: Option<&'a str>,
    /// The file Commit to scene writes (#364); `None` for a bundle's scene
    /// or the web's.
    pub scene_file: Option<std::path::PathBuf>,
    /// The schemas the scene was read with: a component whose schema puts
    /// fields on a panel gets one (#744).
    pub schemas: &'a flint_schema::SchemaRegistry,
    /// The packages' panels for this scene (`ScenePanels`, #1558), made by
    /// the player in registration order, for the HUD to place.
    pub packages: Vec<Box<dyn DebugPanel>>,
}

/// The camera and clock a panel's world lines are drawn from.
pub struct WorldView<'a> {
    pub world: &'a FlintWorld,
    /// The clock the renderer's waves run on.
    pub time: f64,
    pub eye: [f32; 3],
    pub forward: [f32; 3],
}

/// What [`DebugHud::ui`] is lent.
pub struct HudUi<'a> {
    pub ctx: &'a egui::Context,
    pub panels: &'a mut DebugPanels,
    pub view_proj: [[f32; 4]; 4],
    /// Draws one panel's contents: the player draws its own (a game's,
    /// through the extension that named it) and hands the rest to
    /// [`DebugPanel::ui`].
    pub draw: &'a mut dyn FnMut(&mut dyn DebugPanel, &mut egui::Ui),
}

/// What the corner cards show.
pub struct Cards<'a> {
    pub script_errors: &'a flint_script::ScriptErrors,
    /// The styles the scene draws with: their errors, and the registry the
    /// card's "doesn't load" names (#742).
    pub styles: &'a StyleRegistry,
    /// Points kept clear at the bottom (the web page's wordmark).
    pub inset: f32,
}

/// F2's graph this frame: its frames and counters, from the player's
/// history and hitch detector, and where it goes.
pub struct GraphView<'a> {
    pub frames: &'a [GraphFrame],
    pub counters: HitchCounters<'a>,
    pub layout: GraphLayout,
    /// The full overlay's form (the legend), or the compact one.
    pub full: bool,
}

/// The bands a bar is split into, bottom up. A host maps its own phases
/// onto these.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Band {
    /// Scripts' callbacks, and the script UI
    Scripts,
    /// The fixed-step physics
    Physics,
    /// Everything else of the host's own work: input, animation, effects
    Other,
    /// Syncing the renderer with the world, drawing and submitting
    Render,
    /// The HUD pass, and handing the frame to the swapchain (the wait for
    /// the display under vsync)
    HudPresent,
    /// The frame's time that no phase accounts for: the OS, the driver,
    /// the compositor
    Elsewhere,
}

/// How many bands a frame has.
pub const BANDS: usize = Band::ALL.len();

impl Band {
    pub const ALL: [Band; 6] = [
        Band::Scripts,
        Band::Physics,
        Band::Other,
        Band::Render,
        Band::HudPresent,
        Band::Elsewhere,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Band::Scripts => "scripts",
            Band::Physics => "physics",
            Band::Other => "other",
            Band::Render => "render",
            Band::HudPresent => "hud/present",
            Band::Elsewhere => "elsewhere",
        }
    }

    /// The band's colour, from Okabe and Ito's palette (2008), which stays
    /// apart under the common colour-vision deficiencies. Vermillion is
    /// kept for the hitch mark alone.
    pub fn color(self) -> egui::Color32 {
        use egui::Color32;
        match self {
            Band::Scripts => Color32::from_rgb(230, 159, 0), // orange
            Band::Physics => Color32::from_rgb(0, 158, 115), // bluish green
            Band::Other => Color32::from_rgb(240, 228, 66),  // yellow
            Band::Render => Color32::from_rgb(86, 180, 233), // sky blue
            Band::HudPresent => Color32::from_rgb(204, 121, 167), // reddish purple
            Band::Elsewhere => Color32::from_gray(150),
        }
    }
}

/// One frame, as the graph draws it. Times are ms unless named.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct GraphFrame {
    /// When it began, seconds (any epoch, the same for every frame).
    pub at: f64,
    /// Its wall time: how much of the graph's width it takes.
    pub wall_ms: f32,
    /// The bar's height: how long it stood on screen (under vsync pacing,
    /// whole refreshes; else the wall time).
    pub ms: f32,
    /// Its time in each band, indexed as [`Band::ALL`], stacked bottom
    /// up. What runs past `ms` is cut off at the top of the bar.
    pub bands: [f32; BANDS],
    /// The frame was a hitch: it gets a mark.
    pub hitch: bool,
}

impl GraphFrame {
    /// When it ended, seconds.
    pub fn end(&self) -> f64 {
        self.at + self.wall_ms as f64 / 1000.0
    }
}

/// How the graph is laid out.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GraphLayout {
    /// The graph's size, points.
    pub size: egui::Vec2,
    /// The seconds its width spans, up to the newest frame's end.
    pub window_secs: f32,
    /// The display's refresh period, ms: the budget line, and twice it.
    pub budget_ms: f32,
    /// Label the budget lines in ms and fps (the compact form leaves the
    /// fps off).
    pub fps_labels: bool,
}

// What the counters under a graph say. Times are ms unless named.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct HitchCounters<'a> {
    /// Hitches since the run began
    pub hitches: u64,
    /// Hitches a minute, over the run
    pub per_minute: f64,
    /// The worst frame judged
    pub worst_ms: f32,
    /// Seconds since the last hitch ended, if there was one
    pub since_last_secs: Option<f64>,
    /// The last hitch in one line: how long and whose time it was
    pub last: Option<&'a str>,
    /// Hitches of scene loads drawn behind a fade, counted apart from
    /// `hitches` (flint-player's `loading`, #1125); shown when there are any
    pub loading: u64,
}
