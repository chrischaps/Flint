//! The F1 card (#717, #1170): a [`DebugKeyTable`] painted over the frame,
//! the same card in the player and in every editor `flint edit` opens, so
//! F1 means one thing in every tool. Behind the `egui` feature: the table
//! itself needs no UI.

use crate::debug_keys::{DebugKeyTable, Standing};

/// A key's cap as the card shows it: the punctuation keys as their glyph.
pub fn cap(label: &str) -> String {
    let glyph = |name: &str| match name {
        "Backquote" => "`".to_string(),
        "Backslash" => "\\".to_string(),
        "BracketLeft" => "[".to_string(),
        "BracketRight" => "]".to_string(),
        "Semicolon" => ";".to_string(),
        "Quote" => "'".to_string(),
        "Comma" => ",".to_string(),
        "Period" => ".".to_string(),
        "Slash" => "/".to_string(),
        "Minus" => "-".to_string(),
        "Equal" => "=".to_string(),
        other => other
            .strip_prefix("Key")
            .or_else(|| other.strip_prefix("Digit"))
            .unwrap_or(other)
            .to_string(),
    };
    match label.strip_prefix("Shift+") {
        Some(rest) => format!("⇧ {}", glyph(rest)),
        None => glyph(label),
    }
}

/// The F1 card: every debug key of the run (the player's or an editor's), by owner, the live ones bright,
/// the rest dim with the scene that would bring them, a key the game took
/// struck through, and `footer` under them (where to rebind, how to
/// list). Centred, over everything, taking no input.
pub fn paint(ctx: &egui::Context, table: &DebugKeyTable, footer: &str) {
    use egui::{Color32, CornerRadius, Frame, Margin, RichText, Stroke};
    // The key note's palette (key_note.rs): slate card, amber keycaps
    let ink = Color32::from_rgb(226, 230, 238);
    let dim = Color32::from_rgb(132, 138, 152);
    let amber = Color32::from_rgb(255, 214, 140);
    let keycap = |ui: &mut egui::Ui, label: &str, live: bool, struck: bool| {
        let (fill, edge, text) = if live && !struck {
            (
                Color32::from_rgba_premultiplied(36, 28, 13, 36),
                Color32::from_rgba_premultiplied(150, 115, 54, 150),
                amber,
            )
        } else {
            (
                Color32::from_rgba_premultiplied(10, 10, 12, 24),
                Color32::from_rgba_premultiplied(48, 50, 56, 60),
                dim,
            )
        };
        Frame::NONE
            .fill(fill)
            .stroke(Stroke::new(1.0, edge))
            .corner_radius(CornerRadius::same(4))
            .inner_margin(Margin::symmetric(6, 1))
            .show(ui, |ui| {
                let mut t = RichText::new(cap(label)).monospace().size(12.0).color(text);
                if struck {
                    t = t.strikethrough();
                }
                ui.label(t);
            });
    };
    egui::Area::new(egui::Id::new("debug_key_table"))
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
        .order(egui::Order::Foreground)
        .interactable(false)
        .show(ctx, |ui| {
            Frame::NONE
                .fill(Color32::from_rgba_premultiplied(12, 14, 19, 235))
                .stroke(Stroke::new(
                    1.0,
                    Color32::from_rgba_premultiplied(40, 40, 40, 40),
                ))
                .corner_radius(CornerRadius::same(10))
                .inner_margin(Margin::symmetric(18, 14))
                .show(ui, |ui| {
                    ui.set_max_width(520.0);
                    ui.label(RichText::new("Debug keys").size(17.0).color(ink).strong());
                    ui.add_space(6.0);
                    let mut owner = "";
                    egui::Grid::new("debug_key_rows")
                        .num_columns(2)
                        .spacing([12.0, 5.0])
                        .show(ui, |ui| {
                            for e in &table.entries {
                                if e.owner != owner {
                                    owner = e.owner;
                                    let live_group =
                                        table.entries.iter().any(|o| o.owner == owner && o.live);
                                    ui.label("");
                                    let head = if live_group {
                                        RichText::new(owner).size(11.0).color(dim)
                                    } else {
                                        RichText::new(format!("{owner} · in {}", e.when))
                                            .size(11.0)
                                            .color(dim)
                                            .italics()
                                    };
                                    ui.label(head);
                                    ui.end_row();
                                }
                                let struck = matches!(e.standing, Standing::TakenByGame(_));
                                let bright = e.live && e.standing == Standing::Active;
                                keycap(ui, &e.key_label(), bright, struck);
                                let mut line = e.does.to_string();
                                if e.rebound() && e.key.is_some() {
                                    line.push_str(&format!(" — moved from {}", cap(e.default_key)));
                                }
                                match &e.standing {
                                    Standing::TakenByGame(a) => {
                                        line.push_str(&format!(" — the game's `{a}` has the key"))
                                    }
                                    Standing::Clash(a) => {
                                        line.push_str(&format!(" — `{a}` has the key first"))
                                    }
                                    Standing::Unbound => line.push_str(" — unbound"),
                                    Standing::Active => {}
                                }
                                ui.label(RichText::new(line).size(13.0).color(if bright {
                                    ink
                                } else {
                                    dim
                                }));
                                ui.end_row();
                            }
                        });
                    ui.add_space(8.0);
                    ui.label(RichText::new(footer).size(11.0).color(dim));
                });
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caps_show_glyphs() {
        assert_eq!(cap("Backquote"), "`");
        assert_eq!(cap("Shift+F8"), "⇧ F8");
        assert_eq!(cap("KeyK"), "K");
        assert_eq!(cap("none"), "none");
    }
}
