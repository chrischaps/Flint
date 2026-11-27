//! Where the open panels go (ADR 0035, ADR 0037): full-width strips along
//! the bottom, then columns from the right edge above them, balanced by
//! each panel's weight, on egui's Foreground order above everything a game
//! draws. Moved from flint-player's `debug_panels.rs` (#1559).

use crate::{assign_columns, DebugPanel, PanelLayout};

/// Side columns open this wide (the old `SidePanel` default). The grip on
/// a column's left edge resizes it, down to the minimum and up to half the
/// window.
const DEBUG_COLUMN_WIDTH: f32 = 300.0;
const DEBUG_COLUMN_MIN_WIDTH: f32 = 280.0;
const DEBUG_BOTTOM_HEIGHT: f32 = 112.0;

/// Draw the open debug panels: full-width strips along the bottom (e.g. the
/// timeline), then columns from the right edge above them, each panel's
/// contents by `draw` (the host's drawer: a game's panel is drawn by the
/// extension that named it). Returns the width the columns take.
///
/// They float on egui's Foreground order rather than being egui
/// `SidePanel`s, which always paint on the background layer. That keeps
/// them above everything a game draws with egui (`PlayerExtension::ui`):
/// its panels are background and its windows Middle. Popups the panels
/// open (combo boxes, menus) are Foreground too and come up on top.
pub fn show_debug_panels(
    ctx: &egui::Context,
    panels: &mut [Box<dyn DebugPanel>],
    draw: &mut dyn FnMut(&mut dyn DebugPanel, &mut egui::Ui),
) -> f32 {
    // Logical points (screen_rect), never config.width (physical px).
    let screen = ctx.content_rect();

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
    let weights: Vec<u32> = open.iter().map(|&i| panels[i].weight()).collect();
    let columns = assign_columns(&weights, max_cols);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::run_headless_frame;

    struct Dummy;
    impl DebugPanel for Dummy {
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
        panels: &mut [Box<dyn DebugPanel>],
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
        let _ = run_headless_frame(ctx, input, |ui| {
            let ctx = &ui.ctx().clone();
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
        let mut panels: Vec<Box<dyn DebugPanel>> = vec![Box::new(Dummy)];
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
