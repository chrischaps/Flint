mod grass_panel;

pub use grass_panel::GrassDebugPanel;

/// Where the host should dock a panel when rendering it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PanelLayout {
    /// A right-hand side panel (the historical default).
    #[default]
    SideRight,
    /// A full-width bottom strip (timeline-shaped panels).
    Bottom,
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

    /// Returns true if the panel has unapplied changes.
    fn is_dirty(&self) -> bool;

    /// Clear the dirty flag after changes have been applied by the host.
    fn clear_dirty(&mut self);

    /// Downcast support for accessing concrete panel types.
    fn as_any(&self) -> &dyn std::any::Any;
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any;
}
