use std::path::PathBuf;
use flint_terrain::GrassConfig;
use crate::DebugPanel;

pub struct GrassDebugPanel {
    config: GrassConfig,
    original: GrassConfig,
    scene_path: PathBuf,
    terrain_entity_name: String,
    open: bool,
    dirty: bool,
    density_changed: bool,
}

impl GrassDebugPanel {
    pub fn new(config: GrassConfig, scene_path: PathBuf, terrain_entity_name: String) -> Self {
        Self {
            original: config.clone(),
            config,
            scene_path,
            terrain_entity_name,
            open: false,
            dirty: false,
            density_changed: false,
        }
    }

    /// Read-only access to the working config for the player to push to the renderer.
    pub fn config(&self) -> &GrassConfig {
        &self.config
    }

    /// Whether the density field specifically changed (requires buffer reallocation).
    pub fn density_changed(&self) -> bool {
        self.density_changed
    }

    /// Clear the density_changed flag after the player has handled reallocation.
    pub fn clear_density_changed(&mut self) {
        self.density_changed = false
    }
}

impl DebugPanel for GrassDebugPanel {
    fn name(&self) -> &str { "Grass Debug" }

    fn ui(&mut self, _ui: &mut egui::Ui) {
        // TODO: implement in Task 3
    }

    fn is_open(&self) -> bool { self.open }
    fn toggle(&mut self) { self.open = !self.open; }
    fn is_dirty(&self) -> bool { self.dirty }
    fn clear_dirty(&mut self) { self.dirty = false; }
    fn as_any(&self) -> &dyn std::any::Any { self }
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any { self }
}
