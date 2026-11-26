mod camera_panel;
mod component_panel;
mod grass_panel;
mod headless_frame;
mod ocean_panel;
mod particles_panel;
#[cfg(feature = "player")]
pub mod player;
mod render_panel;
mod script_errors;
mod style_drops;
mod style_errors;
mod styles_panel;
mod styles_ui;
pub mod widgets;

pub use camera_panel::{CameraDebugPanel, CameraPanelConfig};
pub use component_panel::ComponentPanel;
pub use grass_panel::GrassDebugPanel;
pub use headless_frame::run_headless_frame;
pub use ocean_panel::{OceanDebugPanel, OceanPanelConfig};
pub use particles_panel::{
    EmitterRow, ParticlePanelAction, ParticlesDebugPanel, PARTICLES_DEBUG_PANEL,
};
pub use render_panel::{RenderDebugPanel, RenderPanelFlags, RENDER_DEBUG_PANEL};
pub use script_errors::{grouped, paint_script_errors, ScriptErrorRow};
pub use style_drops::paint_style_drops;
pub use style_errors::paint_style_errors;
pub use styles_panel::{styled_entity_rows, StyledEntityRow, StylesDebugPanel, STYLES_DEBUG_PANEL};
pub use styles_ui::{StylesEdit, StylesEditor};

// The panel vocabulary is the kernel's (#1559), so a package or the player
// adds a panel without naming this crate; it is here at its old paths.
pub use flint_package::panels::{
    DebugPanel, DebugPanels, PanelCtx, PanelHost, PanelLayout, ScenePanel, ScenePanels,
};

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
