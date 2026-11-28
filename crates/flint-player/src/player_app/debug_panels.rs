//! The F3/F4 panels, the player's side (#1559). The debug HUD the launcher
//! registered (`flint_package::hud::DebugHud`, the debug-hud package's)
//! makes the engine's panels, syncs them, lays them out and toggles them;
//! this file hands it what a scene and a key lend, keeps the game's own
//! panels (ADR 0107) and the packages' scene panels, and moves the cursor.
//! Also here: the `camera_tuning` applier, which every build runs.
//!
//! The panels are the run's `DebugPanels` resource, there only when the
//! run has a debug HUD: a build without the package has none, and every
//! helper here is then a no-op.

use super::{PlayerApp, CAMERA_TUNING_COMPONENT};
use flint_package::hud::{GrassPanel, HudScene, Toggle};
use flint_package::panels::{DebugPanel, DebugPanels};

/// The F3/F4 panels in `resources`, to read; none without a debug HUD.
pub(super) fn panels_of(resources: &flint_package::Resources) -> &[Box<dyn DebugPanel>] {
    resources
        .get::<DebugPanels>()
        .map_or(&[], |panels| panels.as_slice())
}

/// The F3/F4 panels in `resources`, to change; `None` without a debug HUD.
pub(super) fn panels_mut_of(resources: &mut flint_package::Resources) -> Option<&mut DebugPanels> {
    resources.get_mut::<DebugPanels>()
}

impl PlayerApp {
    /// The F3/F4 panels, to read: none without a debug HUD.
    pub(super) fn debug_panels(&self) -> &[Box<dyn DebugPanel>] {
        panels_of(&self.core.resources)
    }

    /// The F3/F4 panels, made if the run has none: a test's.
    #[cfg(test)]
    pub(super) fn debug_panels_mut(&mut self) -> &mut DebugPanels {
        self.core.resources.get_or_insert_with(DebugPanels::default)
    }

    /// How many panels are open.
    pub(super) fn panels_open(&self) -> usize {
        self.debug_panels().iter().filter(|p| p.is_open()).count()
    }

    /// The style settings the scene authors (its `[post_process.styles]`;
    /// none when it has no `[post_process]` block), for the style editor's
    /// Authored button and markers (#268).
    pub(super) fn authored_styles(&self) -> flint_render::stylize::StyleSettings {
        self.scene
            .post_process
            .as_ref()
            .map(|d| {
                super::scene_loading::post_process_config_from_def(d, &self.core.styles).styles
            })
            .unwrap_or_default()
    }

    /// The scene file the style editors' Commit to scene writes (#364):
    /// the scene's path, unless it came from a bundle (nothing on the disk
    /// to write) or the web.
    fn styles_scene_file(&self) -> Option<std::path::PathBuf> {
        self.launch
            .scene_file_writable
            .then(|| std::path::PathBuf::from(&self.core.scene_path))
    }

    /// The scene's debug panels, by the run's debug HUD: the last scene's
    /// go (grass, ocean, particles, styles...); Rendering & Effects (F4)
    /// and the game's own belong to the player and are kept, open or closed
    /// as they were. Before #268 a transition dropped them and never made
    /// them again. Then the game's panels (ADR 0107), after the engine's.
    pub(super) fn create_scene_debug_panels(
        &mut self,
        grass: Option<super::scene_loading::LoadedGrassInfo>,
    ) {
        if self.hud.debug.is_none() || panels_mut_of(&mut self.core.resources).is_none() {
            return;
        }
        let authored = self.authored_styles();
        let assets = self.kept_asset_scope();
        let looks = flint_render::look::list_in(&assets);
        let current = self.current_look().map(str::to_string);
        let scene_file = self.styles_scene_file();
        let schemas = self
            .schemas
            .held_or_read(&self.schema_dirs.dirs, self.schema_dirs.packages.as_ref());
        let packages = self.package_debug_panels();
        let scene = HudScene {
            world: &self.core.world,
            scene_path: &self.core.scene_path,
            grass: grass.map(|info| GrassPanel {
                config: info.config,
                entity: info.terrain_entity_name,
                component: info.component,
            }),
            authored,
            assets,
            looks,
            current_look: current.as_deref(),
            scene_file,
            schemas: &schemas,
            packages,
        };
        if let (Some(hud), Some(panels)) = (
            self.hud.debug.as_mut(),
            self.core.resources.get_mut::<DebugPanels>(),
        ) {
            hud.scene_panels(panels, scene);
        }
        self.create_extension_debug_panels();
    }

    /// Each package's panel for this scene (#1558), in the order the
    /// packages registered them (`flint_package::panels::ScenePanels`): the
    /// Living Props panel in a scene with a `living_prop` (#218), whose
    /// subsystem mirrors the grove into it. None of a package the project's
    /// `[packages]` leaves out (#1528).
    pub(super) fn package_debug_panels(&self) -> Vec<Box<dyn DebugPanel>> {
        let Some(makers) =
            crate::packages::registry().point::<flint_package::panels::ScenePanels>()
        else {
            return Vec::new();
        };
        makers
            .0
            .iter()
            .filter(|(package, _)| !self.core.leaves_out(package))
            .filter_map(|(_, make)| make(&self.core.world))
            .collect()
    }

    /// The packages' panels for this scene, added to the run's (a test's
    /// way to make them alone).
    #[cfg(test)]
    pub(super) fn create_package_debug_panels(&mut self) {
        let made = self.package_debug_panels();
        self.debug_panels_mut().extend(made);
    }

    /// After a look changes (#356): F4 shows it as the one in use, and the
    /// style editors' Authored button and markers follow the scene's keys
    /// under it, which is what the scene draws with now.
    pub(super) fn refresh_style_panels_after_look(&mut self) {
        if self.hud.debug.is_none() {
            return;
        }
        let authored = self.authored_styles();
        let current = self.current_look().map(str::to_string);
        if let (Some(hud), Some(panels)) = (
            self.hud.debug.as_mut(),
            self.core.resources.get_mut::<DebugPanels>(),
        ) {
            hud.look_changed(panels, &authored, current.as_deref());
        }
    }

    /// Apply the scene's `camera_tuning` component to the render camera.
    /// Called after `apply_camera_def()` so the tuning value wins when a
    /// scene declares both.
    pub(super) fn apply_camera_tuning(&mut self) {
        let Some(&entity_id) = self
            .core
            .world
            .entities_with_component(CAMERA_TUNING_COMPONENT)
            .iter()
            .next()
        else {
            return;
        };
        if let Some(fov) = self
            .core
            .world
            .view(entity_id, CAMERA_TUNING_COMPONENT)
            .opt_f32("fov_deg")
        {
            self.camera.fov = fov;
        }
    }

    /// F3 or F4 through the debug HUD, the cursor following: let go while
    /// a panel is open, caught again once none is, in a first-person scene.
    /// With nothing to toggle, `key` says `absent` on screen and in the
    /// console (#367).
    pub(super) fn toggle_panels(&mut self, which: Toggle, key: &str, absent: &str) {
        let toggled = match (
            self.hud.debug.as_mut(),
            self.core.resources.get_mut::<DebugPanels>(),
        ) {
            (Some(hud), Some(panels)) => hud.toggle(panels, which),
            _ => None,
        };
        match toggled {
            Some(_) => self.cursor_follows_panels(),
            None => self.hud.key_note.say(key, absent),
        }
    }

    /// The cursor follows the panels: let go while any is open, captured
    /// again once none is, where the player looks with the mouse.
    pub(super) fn cursor_follows_panels(&mut self) {
        if self.panels_open() > 0 {
            self.release_cursor();
        } else if self.core.player_view().mouse_look {
            self.capture_cursor();
        }
    }

    /// Add the panels the extensions name (ADR 0107), after the engine's,
    /// replacing any added before. A panel that was open stays open.
    pub(super) fn create_extension_debug_panels(&mut self) {
        let Some(panels) = panels_mut_of(&mut self.core.resources) else {
            return;
        };
        let mut open = Vec::new();
        panels.retain(|panel| {
            let Some(ext) = panel.as_any().downcast_ref::<ExtensionPanel>() else {
                return true;
            };
            if ext.open {
                open.push((ext.extension, ext.name.clone()));
            }
            false
        });
        for (extension, ext) in self.extensions.iter().enumerate() {
            for name in ext.debug_panels() {
                let was_open = open.iter().any(|(e, n)| *e == extension && *n == name);
                panels.push(Box::new(ExtensionPanel {
                    extension,
                    name,
                    open: was_open,
                }));
            }
        }
    }
}

/// A debug panel a game names through `PlayerExtension::debug_panels`. It
/// holds only the panel's place and whether it's open: the frame draws its
/// contents with the extension's `debug_panel_ui` (see [`draw_panel`]).
pub(super) struct ExtensionPanel {
    /// Index into `PlayerApp::extensions`.
    pub(super) extension: usize,
    pub(super) name: String,
    pub(super) open: bool,
}

impl DebugPanel for ExtensionPanel {
    fn name(&self) -> &str {
        &self.name
    }
    /// Drawn by [`draw_panel`], which can reach the extension.
    fn ui(&mut self, _ui: &mut egui::Ui) {}
    fn is_open(&self) -> bool {
        self.open
    }
    fn toggle(&mut self) {
        self.open = !self.open;
    }
    /// The game's, not the scene's (ADR 0107): kept across a transition.
    fn outlives_scene(&self) -> bool {
        true
    }
    fn is_dirty(&self) -> bool {
        false
    }
    fn clear_dirty(&mut self) {}
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }
}

/// Draw one panel's contents: an engine panel draws itself, and a game's
/// is drawn by the extension that named it.
pub(super) fn draw_panel(
    panel: &mut dyn DebugPanel,
    extensions: &mut [Box<dyn crate::extension::PlayerExtension>],
    ui: &mut egui::Ui,
) {
    if let Some(ext) = panel.as_any().downcast_ref::<ExtensionPanel>() {
        if let Some(extension) = extensions.get_mut(ext.extension) {
            extension.debug_panel_ui(&ext.name, ui);
        }
    } else {
        panel.ui(ui);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app() -> PlayerApp {
        PlayerApp::new(
            flint_ecs::FlintWorld::new(),
            "panels_test.scene.toml".into(),
            false,
            None,
            None,
        )
    }

    fn names(app: &PlayerApp) -> Vec<&str> {
        app.debug_panels().iter().map(|p| p.name()).collect()
    }

    /// The player hands the HUD its scene: a styled one gets F3's Styles
    /// panel beside F4, through the package (#1559). A launcher without
    /// the debug HUD makes no panels at all, and so none for a style.
    #[test]
    fn a_styled_scene_gets_the_styles_panel_from_the_debug_hud() {
        let mut styled = app();
        let doc: toml::Table = "[post_process.styles.ascii]\ncell = 12\n".parse().unwrap();
        styled.scene.post_process = Some(doc["post_process"].clone().try_into().unwrap());
        assert_eq!(styled.authored_styles().param("ascii", "cell"), Some(12.0));
        styled.create_scene_debug_panels(None);
        if crate::packages::lacks("debug-hud") {
            assert!(names(&styled).is_empty(), "no HUD, no panels");
            return;
        }
        assert_eq!(
            names(&styled),
            [
                flint_debug_ui::STYLES_DEBUG_PANEL,
                flint_debug_ui::RENDER_DEBUG_PANEL
            ]
        );
    }
}
