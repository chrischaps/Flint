//! What a debug key says when it has nothing to act on (#367): F3 in a scene
//! with no panels, F4 with no renderer, the music keys with no session. The
//! person who pressed the key is looking at the window, so the note shows
//! there for a moment, and the console gets the same line (`[debug] F3: ...`,
//! printed like the `[transition]` and `[script]` lines). It used to go to
//! `tracing::info!`, which the player's log filter never prints.
//!
//! One note at a time: a second key's note replaces the first. Its clock is
//! egui's, so under `--frame-step` it runs on game time and a capture shows
//! it on the same frames every run.

/// Seconds a note shows at full strength, then the seconds it fades over.
pub(super) const HOLD: f64 = 1.6;
pub(super) const FADE: f64 = 0.6;

#[derive(Default)]
pub(crate) struct KeyNote {
    shown: Option<Shown>,
}

struct Shown {
    key: &'static str,
    text: String,
    /// egui time it was first painted; `None` until then, so a note said
    /// between frames gets its whole time on screen.
    since: Option<f64>,
}

#[cfg_attr(not(feature = "debug-hud"), allow(dead_code))]
impl KeyNote {
    /// Show `text` for `key` and print it to the console.
    pub(crate) fn say(&mut self, key: &'static str, text: &str) {
        println!("[debug] {key}: {text}");
        self.shown = Some(Shown {
            key,
            text: text.to_string(),
            since: None,
        });
    }

    /// The note showing, as `(key, text)`.
    #[cfg(test)]
    pub(crate) fn showing(&self) -> Option<(&str, &str)> {
        self.shown.as_ref().map(|s| (s.key, s.text.as_str()))
    }

    /// The note's strength at egui time `now` (1 held, falling to 0 over the
    /// fade), starting its clock on the first call. Clears it once it's done.
    fn strength(&mut self, now: f64) -> Option<f32> {
        let shown = self.shown.as_mut()?;
        let age = now - *shown.since.get_or_insert(now);
        if age >= HOLD + FADE {
            self.shown = None;
            return None;
        }
        let fading = ((age - HOLD) / FADE).clamp(0.0, 1.0);
        // Ease out: the note lingers, then goes
        Some((1.0 - fading * fading) as f32)
    }

    /// Paint the note at the top of the window, over everything, taking no
    /// input. `top`: points from the top edge.
    pub(super) fn paint(&mut self, ctx: &egui::Context, top: f32) {
        let now = ctx.input(|i| i.time);
        let Some(a) = self.strength(now) else {
            return;
        };
        let Some(shown) = &self.shown else {
            return;
        };
        // The HUD blends in display space: build every translucent colour
        // premultiplied, scaled by the note's strength (crates/flint-player
        // CLAUDE.md, egui 0.30's premultiply gotcha).
        let fade = |r: u8, g: u8, b: u8, alpha: u8| {
            let k = a * alpha as f32 / 255.0;
            let p = |c: u8| (c as f32 * k).round() as u8;
            egui::Color32::from_rgba_premultiplied(p(r), p(g), p(b), p(255))
        };
        // It appears where it sits and only fades: nothing slides, so any
        // one capture frame reads cleanly.
        egui::Area::new(egui::Id::new("debug_key_note"))
            .anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, top))
            .order(egui::Order::Foreground)
            .interactable(false)
            .show(ctx, |ui| {
                egui::Frame::NONE
                    .fill(fade(14, 16, 22, 224))
                    .stroke(egui::Stroke::new(1.0_f32, fade(255, 255, 255, 40)))
                    .corner_radius(egui::CornerRadius::same(8))
                    .inner_margin(egui::Margin::symmetric(12, 8))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 10.0;
                            // The key, as a keycap
                            egui::Frame::NONE
                                .fill(fade(255, 196, 92, 36))
                                .stroke(egui::Stroke::new(1.0_f32, fade(255, 196, 92, 150)))
                                .corner_radius(egui::CornerRadius::same(4))
                                .inner_margin(egui::Margin::symmetric(6, 2))
                                .show(ui, |ui| {
                                    ui.label(
                                        egui::RichText::new(shown.key)
                                            .monospace()
                                            .size(12.0)
                                            .color(fade(255, 214, 140, 255)),
                                    );
                                });
                            ui.label(
                                egui::RichText::new(&shown.text)
                                    .size(14.0)
                                    .color(fade(226, 230, 238, 255)),
                            );
                        });
                    });
            });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One egui pass at time `t` with the note painted; whether it drew.
    fn frame_at(note: &mut KeyNote, ctx: &egui::Context, t: f64) -> bool {
        let input = egui::RawInput {
            time: Some(t),
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1280.0, 720.0),
            )),
            ..Default::default()
        };
        let out = ctx.run_ui(input, |ui| note.paint(&ui.ctx().clone(), 24.0));
        !out.shapes.is_empty()
    }

    #[test]
    fn a_note_shows_holds_fades_and_goes() {
        let ctx = egui::Context::default();
        let mut note = KeyNote::default();
        assert!(
            !frame_at(&mut note, &ctx, 0.0),
            "nothing said, nothing drawn"
        );

        // Said at t=10 (between frames): its clock starts at the next paint
        note.say("F3", "no debug panels in this scene");
        assert!(frame_at(&mut note, &ctx, 10.0));
        assert_eq!(note.strength(10.0 + HOLD - 0.01), Some(1.0), "held");
        let mid = note.strength(10.0 + HOLD + FADE / 2.0).unwrap();
        assert!(mid > 0.0 && mid < 1.0, "fading: {mid}");
        assert!(frame_at(&mut note, &ctx, 10.0 + HOLD + FADE / 2.0));
        assert!(
            !frame_at(&mut note, &ctx, 10.0 + HOLD + FADE + 0.01),
            "gone"
        );
        assert!(note.showing().is_none(), "and cleared");
    }

    #[test]
    fn a_second_note_replaces_the_first_and_starts_over() {
        let ctx = egui::Context::default();
        let mut note = KeyNote::default();
        note.say("F3", "no debug panels in this scene");
        frame_at(&mut note, &ctx, 0.0);
        note.say("F4", "no renderer active — nothing to tune");
        assert_eq!(
            note.showing(),
            Some(("F4", "no renderer active — nothing to tune"))
        );
        frame_at(&mut note, &ctx, HOLD + 0.1);
        assert_eq!(note.strength(2.0 * HOLD), Some(1.0), "its own hold");
    }
}
