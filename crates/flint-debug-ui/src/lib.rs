mod camera_panel;
mod dead_calm_panel;
mod grass_panel;
mod ocean_panel;
mod reality_panel;
mod tod_panel;
mod visitor_panel;
mod weather_panel;

pub use camera_panel::{CameraDebugPanel, CameraPanelConfig};
pub use dead_calm_panel::DeadCalmDebugPanel;
pub use grass_panel::GrassDebugPanel;
pub use ocean_panel::{OceanDebugPanel, OceanPanelConfig};
pub use reality_panel::{RealityDebugPanel, RealityPanelConfig};
pub use tod_panel::{TimeOfDayDebugPanel, TimeOfDayPanelConfig};
pub use visitor_panel::VisitorDebugPanel;
pub use weather_panel::{WeatherDebugPanel, WeatherPanelConfig};

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

    /// Returns true if the panel has unapplied changes.
    fn is_dirty(&self) -> bool;

    /// Clear the dirty flag after changes have been applied by the host.
    fn clear_dirty(&mut self);

    /// Downcast support for accessing concrete panel types.
    fn as_any(&self) -> &dyn std::any::Any;
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any;
}

/// Estimated content height (in widget rows) per panel name; used for column balancing.
fn panel_weight(name: &str) -> u32 {
    match name {
        "Ocean Debug" => 46,
        "Grass Debug" => 22,
        "Reality" => 20,
        "Weather" => 15,
        "Day / Time" => 10,
        _ => 6, // Camera, Dead Calm, Visitor, future panels
    }
}

/// Distribute panels (by name, in stable creation order) across up to `max_cols`
/// columns via greedy lightest-bin assignment. Returns per-column index lists
/// into the input slice. Deterministic; no empty columns for non-empty input.
pub fn assign_columns(names: &[&str], max_cols: usize) -> Vec<Vec<usize>> {
    let n_cols = max_cols.max(1).min(names.len().max(1));
    let mut cols: Vec<Vec<usize>> = vec![Vec::new(); n_cols];
    let mut loads = vec![0u32; n_cols];
    for (i, name) in names.iter().enumerate() {
        let c = (0..n_cols).min_by_key(|&c| (loads[c], c)).unwrap();
        cols[c].push(i);
        loads[c] += panel_weight(name);
    }
    cols
}

#[cfg(test)]
mod column_tests {
    use super::assign_columns;

    #[test]
    fn single_panel_single_column() {
        let cols = assign_columns(&["Ocean Debug"], 3);
        assert_eq!(cols, vec![vec![0]]);
    }

    #[test]
    fn two_panels_two_columns() {
        let cols = assign_columns(&["Ocean Debug", "Camera"], 3);
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
        let cols = assign_columns(&["Ocean Debug", "Weather", "Camera"], 1);
        assert_eq!(cols, vec![vec![0, 1, 2]]);
    }

    #[test]
    fn full_roster_balances_three_columns() {
        // Float's creation order (no grass): mod.rs create_* sequence.
        let names = [
            "Ocean Debug",
            "Day / Time",
            "Weather",
            "Reality",
            "Visitor",
            "Dead Calm",
            "Camera",
        ];
        let cols = assign_columns(&names, 3);
        assert_eq!(cols.len(), 3);
        assert!(cols.iter().all(|c| !c.is_empty()));
        // Ocean (46 rows) is heavy enough to hold a column alone.
        assert_eq!(cols[0], vec![0]);
        // Every panel lands in exactly one column.
        let mut all: Vec<usize> = cols.iter().flatten().copied().collect();
        all.sort_unstable();
        assert_eq!(all, (0..names.len()).collect::<Vec<_>>());
    }
}
