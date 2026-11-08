//! The on-screen card for user-authored styles that don't compile (#275,
//! ADR 0112). A broken `styles/<name>.wgsl` never takes the frame down: the
//! style draws its last good build, or its input unchanged, and this card
//! says why, in the corner, until the file is fixed and saved. The player
//! and the scene viewer both draw it, over everything else.

use egui::{Color32, FontId, RichText};

/// The most lines of one error the card shows; the console has the rest.
const MAX_LINES: usize = 8;

/// Paint the card for `errors` (`(style, message)`, as
/// `flint_render::stylize::custom::errors` gives them). Nothing when the
/// list is empty. `bottom_inset` lifts it above anything the host keeps at
/// the bottom edge.
pub fn paint_style_errors(ctx: &egui::Context, errors: &[(String, String)], bottom_inset: f32) {
    if errors.is_empty() {
        return;
    }
    // The HUD blends in display space: translucent colours are built
    // premultiplied by hand (see CLAUDE.md on egui 0.30's helpers)
    let ground = Color32::from_rgba_premultiplied(14, 10, 12, 230);
    let ember = Color32::from_rgb(255, 122, 89);
    let ink = Color32::from_rgb(236, 226, 214);
    let quiet = Color32::from_rgb(160, 150, 142);
    egui::Area::new(egui::Id::new("flint_style_errors"))
        .order(egui::Order::Foreground)
        .anchor(egui::Align2::LEFT_BOTTOM, [12.0, -12.0 - bottom_inset])
        .interactable(false)
        .show(ctx, |ui| {
            egui::Frame::none()
                .fill(ground)
                .stroke(egui::Stroke::new(1.0_f32, ember.gamma_multiply(0.6)))
                .rounding(4.0)
                .inner_margin(egui::Margin::symmetric(12.0, 9.0))
                .show(ui, |ui| {
                    ui.set_max_width(ctx.screen_rect().width().min(760.0) - 24.0);
                    for (i, (style, message)) in errors.iter().enumerate() {
                        if i > 0 {
                            ui.add_space(6.0);
                        }
                        ui.label(
                            RichText::new(format!("style \u{2018}{style}\u{2019} doesn't compile"))
                                .color(ember)
                                .font(FontId::proportional(14.0))
                                .strong(),
                        );
                        let lines: Vec<&str> = message.lines().collect();
                        for line in lines.iter().take(MAX_LINES) {
                            ui.label(
                                RichText::new(*line)
                                    .color(ink)
                                    .font(FontId::monospace(12.0)),
                            );
                        }
                        if lines.len() > MAX_LINES {
                            ui.label(
                                RichText::new(format!(
                                    "\u{2026} {} more lines in the log",
                                    lines.len() - MAX_LINES
                                ))
                                .color(quiet)
                                .font(FontId::monospace(11.0)),
                            );
                        }
                    }
                    ui.add_space(4.0);
                    // A bundle is fixed: nothing reloads on the web (#415)
                    #[cfg(not(target_arch = "wasm32"))]
                    let hint = "Its last good build (or its input, unchanged) draws meanwhile. \
                                Save the file to try again.";
                    #[cfg(target_arch = "wasm32")]
                    let hint = "Its input draws unchanged. Fix the file and pack the bundle again.";
                    ui.label(
                        RichText::new(hint)
                        .color(quiet)
                        .font(FontId::proportional(11.5)),
                    );
                });
        });
}
