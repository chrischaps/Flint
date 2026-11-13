//! The on-screen card for user-authored styles that don't compile (#275,
//! ADR 0112) or don't load (#425). A broken `styles/<name>.wgsl` never takes
//! the frame down: the style draws its last good build, or its input
//! unchanged, and this card says why, in the corner, until the file is
//! fixed and saved. A `styles/<name>.shader.toml` that doesn't read
//! registers nothing, so the scene's table for it is ignored: the card
//! names the file and the mistake until it loads or is deleted. The player
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
                        let verb = if is_description_error(message) {
                            "doesn't load"
                        } else {
                            "doesn't compile"
                        };
                        ui.label(
                            RichText::new(format!("style \u{2018}{style}\u{2019} {verb}"))
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
                    ui.label(
                        RichText::new(hint(errors))
                            .color(quiet)
                            .font(FontId::proportional(11.5)),
                    );
                });
        });
}

/// Whether `message` is about a style's description, not its shader: its
/// first line starts with the `.shader.toml`'s path (`<path>.shader.toml:
/// ...`, as every error `flint_render::stylize::custom` reads one with
/// does). A shader's message that names a description later on doesn't
/// count: its path is followed by `: ` long before.
fn is_description_error(message: &str) -> bool {
    message
        .lines()
        .next()
        .and_then(|line| line.split_once(".shader.toml:"))
        .is_some_and(|(path, _)| !path.contains(": "))
}

/// The card's last line: what draws meanwhile, and how to try again. A
/// style that never loaded draws nothing (the scene's table for it waits);
/// one that did keeps its last good build.
fn hint(errors: &[(String, String)]) -> &'static str {
    // A bundle is fixed: nothing reloads on the web (#415)
    if cfg!(target_arch = "wasm32") {
        return "Its input draws unchanged. Fix the file and pack the bundle again.";
    }
    let registered = |(style, _): &(String, String)| flint_render::stylize::find(style).is_some();
    match (
        errors.iter().any(|e| !registered(e)),
        errors.iter().any(registered),
    ) {
        (true, false) => {
            "Nothing of it draws, and the scene's settings for it wait. \
             Save the file to try again."
        }
        (true, true) => {
            "A style that never loaded draws nothing; the others draw their last \
             good build (or their input, unchanged). Save the file to try again."
        }
        _ => {
            "Its last good build (or its input, unchanged) draws meanwhile. \
             Save the file to try again."
        }
    }
}

#[cfg(test)]
mod tests {
    use super::is_description_error;

    #[test]
    fn a_description_error_names_the_shader_toml_first() {
        assert!(is_description_error(
            "styles/x.shader.toml: stage = \"hrd\"; it is \"display\" or \"hdr\""
        ));
        assert!(is_description_error(
            "C:\\My Game\\styles\\x.shader.toml: TOML parse error at line 3\n  |\n3 | k ="
        ));
        assert!(!is_description_error(
            "styles/x.wgsl:9:9: unknown identifier"
        ));
        // A shader's message that mentions the description further on
        assert!(!is_description_error(
            "styles/x.wgsl:1:1: see the params in x.shader.toml: k"
        ));
    }
}
