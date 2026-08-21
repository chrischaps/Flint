//! Debug-panel creation and toggling for `PlayerApp` — code-motion sibling
//! of `mod.rs` (player_app decomposition; see the decomposition ADR).
//! Panel *drain* stays with the frame loop; this file owns construction and
//! the named-toggle plumbing, plus the ungated `camera_tuning` applier that
//! shares the `CAMERA_TUNING_COMPONENT` convention with its panel.

use super::{PlayerApp, CAMERA_TUNING_COMPONENT};
#[cfg(feature = "debug-hud")]
use super::{
    DEAD_CALM_COMPONENT, RAFT_VISITOR_COMPONENT, REALITY_COMPONENT, TIME_OF_DAY_COMPONENT,
    WEATHER_COMPONENT,
};
#[cfg(feature = "debug-hud")]
use flint_core::components as comp;

impl PlayerApp {
    /// Create the ocean tuning panel if the scene has an `ocean` component.
    #[cfg(feature = "debug-hud")]
    pub(super) fn create_ocean_debug_panel(&mut self) {
        let Some(&entity_id) = self
            .world
            .entities_with_component(comp::OCEAN)
            .iter()
            .next()
        else {
            return;
        };
        let Some(name) = self.world.get_name(entity_id).map(str::to_string) else {
            return;
        };
        let Some(ocean_comp) = self
            .world
            .get_components(entity_id)
            .and_then(|comps| comps.get(comp::OCEAN).cloned())
        else {
            return;
        };
        let config = flint_debug_ui::OceanPanelConfig::from_component(&ocean_comp);
        let panel = flint_debug_ui::OceanDebugPanel::new(
            config,
            std::path::PathBuf::from(&self.scene_path),
            name,
        );
        self.debug_panels.push(Box::new(panel));
    }

    /// Create the time-of-day scrubber if the scene has a `time_of_day`
    /// component (a game-side convention — see flint-debug-ui tod_panel).
    #[cfg(feature = "debug-hud")]
    pub(super) fn create_tod_debug_panel(&mut self) {
        let Some(&entity_id) = self
            .world
            .entities_with_component(TIME_OF_DAY_COMPONENT)
            .iter()
            .next()
        else {
            return;
        };
        let Some(name) = self.world.get_name(entity_id).map(str::to_string) else {
            return;
        };
        let Some(tod_comp) = self
            .world
            .get_components(entity_id)
            .and_then(|comps| comps.get(TIME_OF_DAY_COMPONENT).cloned())
        else {
            return;
        };
        let config = flint_debug_ui::TimeOfDayPanelConfig::from_component(&tod_comp);
        let panel = flint_debug_ui::TimeOfDayDebugPanel::new(
            config,
            std::path::PathBuf::from(&self.scene_path),
            name,
        );
        self.debug_panels.push(Box::new(panel));
    }

    /// Create the weather panel if the scene has a `weather` component
    /// (a game-side convention — see flint-debug-ui weather_panel).
    #[cfg(feature = "debug-hud")]
    pub(super) fn create_weather_debug_panel(&mut self) {
        let Some(&entity_id) = self
            .world
            .entities_with_component(WEATHER_COMPONENT)
            .iter()
            .next()
        else {
            return;
        };
        let Some(name) = self.world.get_name(entity_id).map(str::to_string) else {
            return;
        };
        let Some(weather_comp) = self
            .world
            .get_components(entity_id)
            .and_then(|comps| comps.get(WEATHER_COMPONENT).cloned())
        else {
            return;
        };
        let config = flint_debug_ui::WeatherPanelConfig::from_component(&weather_comp);
        let panel = flint_debug_ui::WeatherDebugPanel::new(
            config,
            std::path::PathBuf::from(&self.scene_path),
            name,
        );
        self.debug_panels.push(Box::new(panel));
    }

    /// Create the reality-tear panel if the scene has a `reality` component
    /// (a game-side convention — see flint-debug-ui reality_panel).
    #[cfg(feature = "debug-hud")]
    pub(super) fn create_reality_debug_panel(&mut self) {
        let Some(&entity_id) = self
            .world
            .entities_with_component(REALITY_COMPONENT)
            .iter()
            .next()
        else {
            return;
        };
        let Some(name) = self.world.get_name(entity_id).map(str::to_string) else {
            return;
        };
        let Some(reality_comp) = self
            .world
            .get_components(entity_id)
            .and_then(|comps| comps.get(REALITY_COMPONENT).cloned())
        else {
            return;
        };
        let config = flint_debug_ui::RealityPanelConfig::from_component(&reality_comp);
        let panel = flint_debug_ui::RealityDebugPanel::new(
            config,
            std::path::PathBuf::from(&self.scene_path),
            name,
        );
        self.debug_panels.push(Box::new(panel));
    }

    /// Create the visitor panel if the scene has a `raft_visitor` component
    /// (a game-side convention — see flint-debug-ui visitor_panel).
    #[cfg(feature = "debug-hud")]
    pub(super) fn create_visitor_debug_panel(&mut self) {
        let Some(&entity_id) = self
            .world
            .entities_with_component(RAFT_VISITOR_COMPONENT)
            .iter()
            .next()
        else {
            return;
        };
        let Some(name) = self.world.get_name(entity_id).map(str::to_string) else {
            return;
        };
        let panel = flint_debug_ui::VisitorDebugPanel::new(name);
        self.debug_panels.push(Box::new(panel));
    }

    /// Create the dead-calm panel if the scene has a `dead_calm` component
    /// (a game-side convention — see flint-debug-ui dead_calm_panel).
    #[cfg(feature = "debug-hud")]
    pub(super) fn create_dead_calm_debug_panel(&mut self) {
        let Some(&entity_id) = self
            .world
            .entities_with_component(DEAD_CALM_COMPONENT)
            .iter()
            .next()
        else {
            return;
        };
        let Some(name) = self.world.get_name(entity_id).map(str::to_string) else {
            return;
        };
        let panel = flint_debug_ui::DeadCalmDebugPanel::new(name);
        self.debug_panels.push(Box::new(panel));
    }

    /// Create the camera tuning panel if the scene has a `camera_tuning`
    /// component.
    #[cfg(feature = "debug-hud")]
    pub(super) fn create_camera_debug_panel(&mut self) {
        let Some(&entity_id) = self
            .world
            .entities_with_component(CAMERA_TUNING_COMPONENT)
            .iter()
            .next()
        else {
            return;
        };
        let Some(name) = self.world.get_name(entity_id).map(str::to_string) else {
            return;
        };
        let Some(cam_comp) = self
            .world
            .get_components(entity_id)
            .and_then(|comps| comps.get(CAMERA_TUNING_COMPONENT).cloned())
        else {
            return;
        };
        let config = flint_debug_ui::CameraPanelConfig::from_component(&cam_comp);
        let panel = flint_debug_ui::CameraDebugPanel::new(
            config,
            std::path::PathBuf::from(&self.scene_path),
            name,
        );
        self.debug_panels.push(Box::new(panel));
    }

    /// Apply the scene's `camera_tuning` component to the render camera.
    /// Called after `apply_camera_def()` so the tuning value wins when a
    /// scene declares both.
    pub(super) fn apply_camera_tuning(&mut self) {
        let Some(&entity_id) = self
            .world
            .entities_with_component(CAMERA_TUNING_COMPONENT)
            .iter()
            .next()
        else {
            return;
        };
        if let Some(fov) = self
            .world
            .get_components(entity_id)
            .and_then(|comps| comps.get(CAMERA_TUNING_COMPONENT))
            .and_then(|c| c.get("fov_deg"))
            .and_then(flint_core::toml_util::toml_f32)
        {
            self.camera.fov = fov;
        }
    }

    /// Toggle a named debug panel, adjusting cursor capture to match its new
    /// state; logs `absent_msg` when no such panel is registered (no music
    /// session running). One body for the Backquote/Backslash key handlers.
    #[cfg(feature = "debug-hud")]
    pub(super) fn toggle_named_panel(&mut self, name: &str, absent_msg: &str) {
        let mut opened = false;
        let mut exists = false;
        for panel in &mut self.debug_panels {
            if panel.name() == name {
                exists = true;
                panel.toggle();
                opened = panel.is_open();
            }
        }
        if exists {
            if opened {
                self.release_cursor();
            } else if self.physics.has_player_entity() {
                self.capture_cursor();
            }
        } else {
            tracing::info!("{absent_msg}");
        }
    }
}
