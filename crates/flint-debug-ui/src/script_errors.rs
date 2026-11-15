//! The on-screen card for scripts that fail (#693): a typo'd function in
//! `on_update` used to give sixty identical warnings a second on stderr and a
//! game that just looked inert. The player now counts each error once
//! (`flint_script::ScriptErrors`) and this card says which script, on which
//! entity, in which callback, at which line, and how often, in the bottom
//! left corner above the style card, until the file is saved again or the
//! scene changes.
//!
//! It takes plain rows, not `flint_script` types, so this crate stays free
//! of the scripting engine: the player turns each error into a
//! [`ScriptErrorRow`].

use egui::{Color32, FontId, RichText};

/// The most errors the card lists; the rest are counted on its last line.
pub const MAX_SHOWN: usize = 4;

/// The most lines of one message the card shows; the console has the rest.
const MAX_LINES: usize = 4;

/// One error, as the card shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptErrorRow {
    /// `script error · hud_controller · on_update`
    pub heading: String,
    /// `scripts/hud.rhai:42:5`
    pub location: String,
    /// What went wrong; its first lines are shown.
    pub message: String,
    /// `1,240 times since frame 3`
    pub count: String,
}

/// Paint the card for `rows` (the most recent first), `more` errors past
/// them unshown. Nothing when `rows` is empty. Returns the height it took,
/// with a gap, so a card painted after it (the style card) sits above it.
pub fn paint_script_errors(
    ctx: &egui::Context,
    rows: &[ScriptErrorRow],
    more: usize,
    bottom_inset: f32,
) -> f32 {
    if rows.is_empty() {
        return 0.0;
    }
    // The HUD blends in display space: translucent colours are built
    // premultiplied by hand, as the style card's are
    let ground = Color32::from_rgba_premultiplied(12, 10, 16, 232);
    let flare = Color32::from_rgb(255, 176, 72);
    let ink = Color32::from_rgb(236, 228, 214);
    let place = Color32::from_rgb(150, 196, 232);
    let quiet = Color32::from_rgb(160, 150, 142);
    let response = egui::Area::new(egui::Id::new("flint_script_errors"))
        .order(egui::Order::Foreground)
        .anchor(egui::Align2::LEFT_BOTTOM, [12.0, -12.0 - bottom_inset])
        .interactable(false)
        .show(ctx, |ui| {
            egui::Frame::NONE
                .fill(ground)
                .stroke(egui::Stroke::new(1.0_f32, flare.gamma_multiply(0.6)))
                .corner_radius(4.0)
                .inner_margin(egui::Margin::symmetric(12, 9))
                .show(ui, |ui| {
                    ui.set_max_width(ctx.content_rect().width().min(760.0) - 24.0);
                    for (i, row) in rows.iter().take(MAX_SHOWN).enumerate() {
                        if i > 0 {
                            ui.add_space(7.0);
                        }
                        ui.label(
                            RichText::new(&row.heading)
                                .color(flare)
                                .font(FontId::proportional(14.0))
                                .strong(),
                        );
                        ui.label(
                            RichText::new(&row.location)
                                .color(place)
                                .font(FontId::monospace(12.0)),
                        );
                        let lines: Vec<&str> = row.message.lines().collect();
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
                        ui.label(
                            RichText::new(&row.count)
                                .color(quiet)
                                .font(FontId::proportional(11.5)),
                        );
                    }
                    let unshown = more + rows.len().saturating_sub(MAX_SHOWN);
                    if unshown > 0 {
                        ui.add_space(5.0);
                        ui.label(
                            RichText::new(format!(
                                "\u{2026} and {unshown} more script error{} in the log",
                                if unshown == 1 { "" } else { "s" }
                            ))
                            .color(quiet)
                            .font(FontId::proportional(11.5)),
                        );
                    }
                    ui.add_space(4.0);
                    ui.label(
                        RichText::new(hint())
                            .color(quiet)
                            .font(FontId::proportional(11.5)),
                    );
                });
        });
    response.response.rect.height() + 8.0
}

/// The card's last line: where the rest is, and how to try again.
fn hint() -> &'static str {
    // A bundle is fixed: nothing reloads on the web (#415)
    if cfg!(target_arch = "wasm32") {
        "The console has each error once, then a count. Fix the script and pack the bundle again."
    } else {
        "The console has each error once, then its count at 10, 100, 1000. \
         Saving the file reloads it."
    }
}

/// `1,240`
pub fn grouped(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_are_grouped_in_threes() {
        assert_eq!(grouped(7), "7");
        assert_eq!(grouped(1240), "1,240");
        assert_eq!(grouped(1_000_000), "1,000,000");
    }

    #[test]
    fn the_card_paints_and_says_how_tall_it_is() {
        let ctx = egui::Context::default();
        let rows = vec![ScriptErrorRow {
            heading: "script error \u{b7} hud \u{b7} on_update".into(),
            location: "scripts/hud.rhai:3:5".into(),
            message: "Function not found: drw_text (i64)".into(),
            count: "60 times since frame 1".into(),
        }];
        let mut height = 0.0;
        // Two frames: an Area's first is egui's sizing pass
        for _ in 0..2 {
            crate::run_headless_frame(&ctx, egui::RawInput::default(), |ui| {
                height = paint_script_errors(ui.ctx(), &rows, 0, 0.0);
            });
        }
        assert!(height > 40.0, "{height}");
        let mut none = 1.0;
        crate::run_headless_frame(&ctx, egui::RawInput::default(), |ui| {
            none = paint_script_errors(ui.ctx(), &[], 0, 0.0);
        });
        assert_eq!(none, 0.0);
    }
}
