mod grass_panel;
mod ocean_panel;

pub use grass_panel::GrassDebugPanel;
pub use ocean_panel::{OceanDebugPanel, OceanPanelConfig};

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
