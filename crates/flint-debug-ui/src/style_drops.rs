//! The on-screen card for what a style's cap left out (#1246). A cap that
//! drops something (ASCII's 8 dissolve bands, pencil's paper images, the
//! style mask's ids...) warns once in the log and names the first entity
//! past it (#740); this card says the same in the window, so whoever looks
//! at a prop drawn wrong sees why. The list is the process's
//! (`flint_render::stylize::variant_table::dropped`): a scene binding
//! clears it, so the card empties when a scene without drops loads. The
//! player and the scene viewer draw it above the style error card.

use egui::{Color32, FontId, RichText};
use flint_render::stylize::variant_table::Dropped;

/// The most drops the card lists; the log has every one.
const MAX_SHOWN: usize = 4;

/// Paint the card for `drops`. Nothing when the list is empty. Returns the
/// height it took (0 when nothing), so a card above it can stand clear;
/// `bottom_inset` lifts it above the cards already in the corner.
pub fn paint_style_drops(ctx: &egui::Context, drops: &[Dropped], bottom_inset: f32) -> f32 {
    if drops.is_empty() {
        return 0.0;
    }
    // The HUD blends in display space: translucent colours are built
    // premultiplied by hand, as the other cards' are. A cap is a limit
    // met, not a mistake: a cool tide, not the error cards' ember
    let ground = Color32::from_rgba_premultiplied(9, 13, 16, 228);
    let tide = Color32::from_rgb(120, 206, 214);
    let ink = Color32::from_rgb(226, 232, 230);
    let quiet = Color32::from_rgb(142, 156, 158);
    let response = egui::Area::new(egui::Id::new("flint_style_drops"))
        .order(egui::Order::Foreground)
        .anchor(egui::Align2::LEFT_BOTTOM, [12.0, -12.0 - bottom_inset])
        .interactable(false)
        .show(ctx, |ui| {
            egui::Frame::NONE
                .fill(ground)
                .stroke(egui::Stroke::new(1.0_f32, tide.gamma_multiply(0.55)))
                .corner_radius(4.0)
                .inner_margin(egui::Margin::symmetric(12, 9))
                .show(ui, |ui| {
                    ui.set_max_width(ctx.content_rect().width().min(760.0) - 24.0);
                    for (i, drop) in drops.iter().take(MAX_SHOWN).enumerate() {
                        if i > 0 {
                            ui.add_space(6.0);
                        }
                        // The entity in monospace: a name to search the scene for
                        let part = |font: FontId, color: Color32| egui::TextFormat {
                            font_id: font,
                            color,
                            ..Default::default()
                        };
                        let mut line = egui::text::LayoutJob::default();
                        line.append(
                            &format!("{} leaves ", who(drop)),
                            0.0,
                            part(FontId::proportional(14.0), tide),
                        );
                        line.append(&drop.entity, 0.0, part(FontId::monospace(13.5), ink));
                        line.append(" out", 0.0, part(FontId::proportional(14.0), tide));
                        ui.label(line);
                        ui.label(
                            RichText::new(detail(drop))
                                .color(ink)
                                .font(FontId::proportional(12.5)),
                        );
                    }
                    if drops.len() > MAX_SHOWN {
                        ui.add_space(5.0);
                        let more = drops.len() - MAX_SHOWN;
                        ui.label(
                            RichText::new(format!(
                                "\u{2026} and {more} more cap{} in the log",
                                if more == 1 { "" } else { "s" }
                            ))
                            .color(quiet)
                            .font(FontId::proportional(11.5)),
                        );
                    }
                    ui.add_space(4.0);
                    ui.label(
                        RichText::new(HINT)
                            .color(quiet)
                            .font(FontId::proportional(11.5)),
                    );
                });
        });
    response.response.rect.height() + 8.0
}

/// Who left the entity out, as the heading names it (`style ‘ascii’`
/// leaves `prop008` out). The style mask isn't a style, so it is named as
/// itself.
fn who(drop: &Dropped) -> String {
    if drop.style == "style mask" {
        "the style mask".to_string()
    } else {
        format!("style \u{2018}{}\u{2019}", drop.style)
    }
}

/// `past the 8 dissolve bands one frame holds, so it is dissolved in its
/// cells' unranked order`: the cap, its size, and how the entity draws.
fn detail(drop: &Dropped) -> String {
    format!(
        "past the {} {} one frame holds, so it is {}",
        drop.limit, drop.cap, drop.instead
    )
}

/// The card's last line: who else is left out, how long it says so, and
/// how to bring them back.
const HINT: &str = "The entity named is the first past the cap; those after it are left \
                    out too. Fewer on screen at once, or entities sharing their settings, \
                    bring them back. Said until another scene loads.";

#[cfg(test)]
mod tests {
    use super::*;

    fn bands(entity: &str) -> Dropped {
        Dropped {
            style: "ascii".into(),
            cap: "dissolve bands".into(),
            limit: 8,
            entity: entity.into(),
            instead: "dissolved in its cells' unranked order".into(),
        }
    }

    #[test]
    fn a_drop_names_the_style_the_cap_and_the_entity() {
        let drop = bands("prop008");
        assert_eq!(who(&drop), "style \u{2018}ascii\u{2019}");
        assert_eq!(
            detail(&drop),
            "past the 8 dissolve bands one frame holds, so it is dissolved in its \
             cells' unranked order"
        );
        let mask = Dropped {
            style: "style mask".into(),
            cap: "style ids".into(),
            limit: 65534,
            entity: "crate_9".into(),
            instead: "drawn unstyled".into(),
        };
        assert_eq!(who(&mask), "the style mask");
    }

    #[test]
    fn the_card_paints_and_says_how_tall_it_is() {
        let ctx = egui::Context::default();
        let drops = vec![bands("prop008")];
        let mut height = 0.0;
        let mut text = String::new();
        // Two frames: an Area's first is egui's sizing pass
        for _ in 0..2 {
            let output = crate::run_headless_frame(&ctx, egui::RawInput::default(), |ui| {
                height = paint_style_drops(ui.ctx(), &drops, 0.0);
            });
            text = output
                .shapes
                .iter()
                .filter_map(|clipped| match &clipped.shape {
                    egui::Shape::Text(t) => Some(t.galley.text().to_string()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n");
        }
        assert!(height > 40.0, "{height}");
        assert!(
            text.contains("style \u{2018}ascii\u{2019} leaves prop008 out"),
            "the heading names the style and the entity:\n{text}"
        );
        assert!(text.contains("past the 8 dissolve bands"), "{text}");
        let mut none = 1.0;
        crate::run_headless_frame(&ctx, egui::RawInput::default(), |ui| {
            none = paint_style_drops(ui.ctx(), &[], 0.0);
        });
        assert_eq!(none, 0.0, "no drops, no card");
    }
}
