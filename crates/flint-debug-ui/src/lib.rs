mod camera_panel;
mod component_panel;
mod grass_panel;
mod headless_frame;
#[cfg(feature = "living")]
mod living_props_panel;
mod ocean_panel;
mod particles_panel;
mod render_panel;
mod script_errors;
mod style_errors;
mod styles_panel;
mod styles_ui;
pub mod widgets;

pub use camera_panel::{CameraDebugPanel, CameraPanelConfig};
pub use component_panel::ComponentPanel;
pub use grass_panel::GrassDebugPanel;
pub use headless_frame::run_headless_frame;
#[cfg(feature = "living")]
pub use living_props_panel::{
    pick_prop, Knob, KnobControl, KnobDesc, KnobFlags, KnobPanel, KnobSection, KnobSources,
    KnobValue, LivingAbState, LivingHealth, LivingKnobs, LivingPanelAction, LivingPanelState,
    LivingPickedState, LivingPropRow, LivingPropsDebugPanel, LivingSpeciesState, PanelScope,
    KNOB_TABLE, LIVING_PROPS_DEBUG_PANEL,
};
pub use ocean_panel::{OceanDebugPanel, OceanPanelConfig};
pub use particles_panel::{
    EmitterRow, ParticlePanelAction, ParticlesDebugPanel, PARTICLES_DEBUG_PANEL,
};
pub use render_panel::{RenderDebugPanel, RenderPanelFlags, RENDER_DEBUG_PANEL};
pub use script_errors::{grouped, paint_script_errors, ScriptErrorRow};
pub use style_errors::paint_style_errors;
pub use styles_panel::{styled_entity_rows, StyledEntityRow, StylesDebugPanel, STYLES_DEBUG_PANEL};
pub use styles_ui::{StylesEdit, StylesEditor};

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

/// Common interface for debug overlay panels.
/// The player app holds `Vec<Box<dyn DebugPanel>>` and renders them generically.
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

    /// Once a frame, before the panels draw: apply what the panel's widgets
    /// changed and read back what it shows. The default does nothing (a
    /// panel the host fills itself, a game's).
    fn sync(&mut self, _ctx: &mut PanelCtx) {}

    /// About how many widget rows tall the panel is, for balancing the F3
    /// columns ([`assign_columns`]).
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

/// Distribute panels (by [`DebugPanel::weight`], in stable creation order)
/// across up to `max_cols` columns via greedy lightest-bin assignment.
/// Returns per-column index lists into the input slice. Deterministic; no
/// empty columns for non-empty input.
pub fn assign_columns(weights: &[u32], max_cols: usize) -> Vec<Vec<usize>> {
    let n_cols = max_cols.max(1).min(weights.len().max(1));
    let mut cols: Vec<Vec<usize>> = vec![Vec::new(); n_cols];
    let mut loads = vec![0u32; n_cols];
    for (i, weight) in weights.iter().enumerate() {
        let c = (0..n_cols).min_by_key(|&c| (loads[c], c)).unwrap();
        cols[c].push(i);
        loads[c] += weight;
    }
    cols
}

#[cfg(test)]
mod column_tests {
    use super::assign_columns;

    #[test]
    fn single_panel_single_column() {
        let cols = assign_columns(&[46], 3);
        assert_eq!(cols, vec![vec![0]]);
    }

    #[test]
    fn two_panels_two_columns() {
        let cols = assign_columns(&[46, 6], 3);
        assert_eq!(cols, vec![vec![0], vec![1]]);
    }

    #[test]
    fn empty_input_no_empty_columns() {
        let cols = assign_columns(&[], 3);
        assert_eq!(cols.iter().filter(|c| !c.is_empty()).count(), 0);
        assert!(cols.len() <= 1);
    }

    #[test]
    fn max_cols_one_stacks_everything() {
        let cols = assign_columns(&[46, 15, 6], 1);
        assert_eq!(cols, vec![vec![0, 1, 2]]);
    }

    #[test]
    fn full_roster_balances_three_columns() {
        // A representative full roster in creation order, mixing engine
        // panels with game-supplied ones (which take the default weight).
        // Rendering & Effects, Ocean, then a game's component panels
        // (Day / Time, Weather, Lantern, Fog Bank, Tide) and Camera.
        let weights = [60, 46, 10, 15, 20, 5, 6, 6];
        let cols = assign_columns(&weights, 3);
        assert_eq!(cols.len(), 3);
        assert!(cols.iter().all(|c| !c.is_empty()));
        // Rendering & Effects (60 rows) is heavy enough to hold a column
        // alone; Ocean (46 rows) anchors the second column.
        assert_eq!(cols[0], vec![0]);
        assert_eq!(cols[1].first(), Some(&1));
        // Every panel lands in exactly one column.
        let mut all: Vec<usize> = cols.iter().flatten().copied().collect();
        all.sort_unstable();
        assert_eq!(all, (0..weights.len()).collect::<Vec<_>>());
    }
}
