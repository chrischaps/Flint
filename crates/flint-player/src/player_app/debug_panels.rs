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
    /// Create the Particles panel when the scene has any `particle_emitter`
    /// or `particle_effect` entity (ADR 0068). Rows are refreshed each frame
    /// by the drain in `frame.rs`.
    #[cfg(feature = "debug-hud")]
    pub(super) fn create_particles_debug_panel(&mut self) {
        let has_any = !self
            .world
            .entities_with_component(comp::PARTICLE_EMITTER)
            .is_empty()
            || !self
                .world
                .entities_with_component(comp::PARTICLE_EFFECT)
                .is_empty();
        if !has_any {
            return;
        }
        self.debug_panels
            .push(Box::new(flint_debug_ui::ParticlesDebugPanel::new()));
    }

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

    /// Add the panels the extensions name (ADR 0107), after the engine's,
    /// replacing any added before. A panel that was open stays open.
    #[cfg(feature = "debug-hud")]
    pub(super) fn create_extension_debug_panels(&mut self) {
        let mut open = Vec::new();
        self.debug_panels.retain(|panel| {
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
                self.debug_panels.push(Box::new(ExtensionPanel {
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
#[cfg(feature = "debug-hud")]
pub(super) struct ExtensionPanel {
    /// Index into `PlayerApp::extensions`.
    extension: usize,
    name: String,
    open: bool,
}

#[cfg(feature = "debug-hud")]
impl flint_debug_ui::DebugPanel for ExtensionPanel {
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
#[cfg(feature = "debug-hud")]
pub(super) fn draw_panel(
    panel: &mut dyn flint_debug_ui::DebugPanel,
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

/// Side columns open this wide (the old `SidePanel` default). The grip on
/// a column's left edge resizes it, down to the minimum and up to half the
/// window.
#[cfg(feature = "debug-hud")]
const DEBUG_COLUMN_WIDTH: f32 = 300.0;
#[cfg(feature = "debug-hud")]
const DEBUG_COLUMN_MIN_WIDTH: f32 = 280.0;
#[cfg(feature = "debug-hud")]
const DEBUG_BOTTOM_HEIGHT: f32 = 112.0;

/// Draw the open debug panels: full-width strips along the bottom (e.g. the
/// timeline), then columns from the right edge above them, each panel's
/// contents by `draw` (see [`draw_panel`]). Returns the width the columns
/// take.
///
/// They float on egui's Foreground order rather than being egui
/// `SidePanel`s, which always paint on the background layer. That keeps
/// them above everything a game draws with egui (`PlayerExtension::ui`):
/// its panels are background and its windows Middle. Popups the panels
/// open (combo boxes, menus) are Foreground too and come up on top.
#[cfg(feature = "debug-hud")]
pub(super) fn show_debug_panels(
    ctx: &egui::Context,
    panels: &mut [Box<dyn flint_debug_ui::DebugPanel>],
    draw: &mut dyn FnMut(&mut dyn flint_debug_ui::DebugPanel, &mut egui::Ui),
) -> f32 {
    use flint_debug_ui::PanelLayout;

    // Logical points (screen_rect), never config.width (physical px).
    let screen = ctx.screen_rect();

    // Bottom strips draw their own compact labels (a heading would eat the
    // vertical budget).
    let mut bottom = screen.max.y;
    for panel in panels.iter_mut() {
        if panel.is_open() && matches!(panel.layout(), PanelLayout::Bottom) {
            let id = egui::Id::new(("debug_bottom", panel.name()));
            let size = egui::vec2(screen.width(), DEBUG_BOTTOM_HEIGHT);
            let top_right = egui::pos2(screen.max.x, bottom - DEBUG_BOTTOM_HEIGHT);
            debug_area(ctx, id, top_right, size, |ui| draw(panel.as_mut(), ui));
            bottom -= DEBUG_BOTTOM_HEIGHT;
        }
    }

    // Open side-panel indices, in creation order.
    let open: Vec<usize> = panels
        .iter()
        .enumerate()
        .filter(|(_, p)| p.is_open() && matches!(p.layout(), PanelLayout::SideRight))
        .map(|(i, _)| i)
        .collect();
    if open.is_empty() {
        return 0.0;
    }

    // Panels take at most ~half the window; fewer columns when narrow.
    let max_cols = (((screen.width() * 0.5) / DEBUG_COLUMN_WIDTH).floor() as usize).clamp(1, 3);
    let names: Vec<&str> = open.iter().map(|&i| panels[i].name()).collect();
    let columns = flint_debug_ui::assign_columns(&names, max_cols);
    let max_width = (screen.width() * 0.5).max(DEBUG_COLUMN_MIN_WIDTH);

    let mut right = screen.max.x;
    for (col_idx, col) in columns.iter().enumerate() {
        let id = egui::Id::new(("debug_col", col_idx));
        let width_id = id.with("width");
        let width = ctx
            .data(|d| d.get_temp::<f32>(width_id))
            .unwrap_or(DEBUG_COLUMN_WIDTH)
            .clamp(DEBUG_COLUMN_MIN_WIDTH, max_width);
        let size = egui::vec2(width, bottom - screen.min.y);
        let top_right = egui::pos2(right, screen.min.y);

        let rect = debug_area(ctx, id, top_right, size, |ui| {
            // The column's outer rect (last frame's size): where the grip
            // goes this frame.
            let area = ui.max_rect() + egui::Frame::side_top_panel(ui.style()).inner_margin;
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    for &slot in col {
                        let panel = &mut panels[open[slot]];
                        let name = panel.name().to_owned();
                        egui::CollapsingHeader::new(egui::RichText::new(name).heading())
                            .default_open(true)
                            .show(ui, |ui| draw(panel.as_mut(), ui));
                        ui.separator();
                    }
                });

            // Resize grip along the left edge, registered last so it wins
            // over the content under it.
            let grip = egui::Rect::from_x_y_ranges(area.min.x..=area.min.x + 4.0, area.y_range());
            let response = ui.interact(grip, id.with("grip"), egui::Sense::drag());
            let active = response.hovered() || response.dragged();
            if active {
                ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
            }
            // The width follows the pointer (as in a SidePanel), not summed
            // deltas, which lose movement to the one-frame lag of `area`.
            if let (true, Some(pointer)) = (response.dragged(), response.interact_pointer_pos()) {
                let new_width = (top_right.x - pointer.x).clamp(DEBUG_COLUMN_MIN_WIDTH, max_width);
                ui.ctx().data_mut(|d| d.insert_temp(width_id, new_width));
            }
            let visuals = ui.visuals();
            let stroke = if active {
                visuals.widgets.active.fg_stroke
            } else {
                visuals.widgets.noninteractive.bg_stroke
            };
            ui.painter().vline(area.min.x + 0.5, area.y_range(), stroke);
        });
        right = rect.min.x;
    }
    screen.max.x - right
}

/// One unmovable Foreground area of `size` with its top-right corner pinned
/// at `top_right`, framed and filled like an egui side panel. Contents
/// wider than the area are clipped, as in a `SidePanel` of the same width.
/// Returns the area's rect.
#[cfg(feature = "debug-hud")]
fn debug_area(
    ctx: &egui::Context,
    id: egui::Id,
    top_right: egui::Pos2,
    size: egui::Vec2,
    add_contents: impl FnOnce(&mut egui::Ui),
) -> egui::Rect {
    egui::Area::new(id)
        .order(egui::Order::Foreground)
        .pivot(egui::Align2::RIGHT_TOP)
        .fixed_pos(top_right)
        .constrain(false)
        .fade_in(false)
        .show(ctx, |ui| {
            let frame = egui::Frame::side_top_panel(ui.style());
            let inner = (size - frame.total_margin().sum()).max(egui::Vec2::ZERO);
            frame.show(ui, |ui| {
                // Exactly `size`: contents that fill the available width
                // (scroll areas, separators) would otherwise keep whatever
                // width the area had last frame, starting with the
                // screen-wide sizing pass.
                ui.set_min_size(inner);
                ui.set_max_size(inner);
                add_contents(ui);
            });
        })
        .response
        .rect
}

#[cfg(all(test, feature = "debug-hud"))]
mod tests {
    use super::*;

    struct Dummy;
    impl flint_debug_ui::DebugPanel for Dummy {
        fn name(&self) -> &str {
            "Dummy"
        }
        fn ui(&mut self, ui: &mut egui::Ui) {
            ui.label("rows");
        }
        fn is_open(&self) -> bool {
            true
        }
        fn toggle(&mut self) {}
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

    /// Run one frame at 1000x600 with `events`; returns the column width.
    fn frame(
        ctx: &egui::Context,
        panels: &mut [Box<dyn flint_debug_ui::DebugPanel>],
        events: Vec<egui::Event>,
    ) -> f32 {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1000.0, 600.0),
            )),
            events,
            ..Default::default()
        };
        let mut width = 0.0;
        let _ = ctx.run(input, |ctx| {
            width = show_debug_panels(ctx, panels, &mut |panel, ui| panel.ui(ui))
        });
        width
    }

    fn pointer(pos: egui::Pos2, pressed: Option<bool>) -> Vec<egui::Event> {
        let mut events = vec![egui::Event::PointerMoved(pos)];
        if let Some(pressed) = pressed {
            events.push(egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: Default::default(),
            });
        }
        events
    }

    #[test]
    fn column_keeps_its_width_and_the_grip_resizes_it() {
        let ctx = egui::Context::default();
        let mut panels: Vec<Box<dyn flint_debug_ui::DebugPanel>> = vec![Box::new(Dummy)];
        for _ in 0..3 {
            frame(&ctx, &mut panels, vec![]);
        }
        let width = frame(&ctx, &mut panels, vec![]);
        assert_eq!(
            width, DEBUG_COLUMN_WIDTH,
            "a column opens at its default width"
        );

        // Drag the left edge 80 points left.
        let edge = 1000.0 - DEBUG_COLUMN_WIDTH + 1.0;
        frame(&ctx, &mut panels, pointer(egui::pos2(edge, 300.0), None));
        frame(
            &ctx,
            &mut panels,
            pointer(egui::pos2(edge, 300.0), Some(true)),
        );
        // Big steps, one per frame: summing deltas against last frame's
        // rect would lose some of them.
        for step in 1..=4 {
            let x = edge - 20.0 * step as f32;
            frame(&ctx, &mut panels, pointer(egui::pos2(x, 300.0), None));
        }
        frame(
            &ctx,
            &mut panels,
            pointer(egui::pos2(edge - 80.0, 300.0), Some(false)),
        );
        let width = frame(&ctx, &mut panels, vec![]);
        assert!(
            (width - (DEBUG_COLUMN_WIDTH + 80.0)).abs() < 2.0,
            "dragging the grip widens the column: {width}"
        );
    }
}
