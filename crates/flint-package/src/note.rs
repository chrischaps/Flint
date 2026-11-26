//! What a debug key says when it has nothing to act on (#367): F3 in a scene
//! with no panels, F4 with no renderer, the music keys with no session. The
//! person who pressed the key is looking at the window, so the note shows
//! there for a moment, and the log gets the same line (`[debug] F3: ...`,
//! at info under target `player`, which the player's filter shows, #716).
//!
//! One note at a time: a second key's note replaces the first. Its clock is
//! egui's, so under `--frame-step` it runs on game time and a capture shows
//! it on the same frames every run.
//!
//! A kernel type (ADR 0137 section 3): every subsystem's [`StageCtx`](crate::StageCtx)
//! lends the host's one note, and the host paints it.

/// Seconds a note shows at full strength, then the seconds it fades over.
pub const HOLD: f64 = 1.6;
pub const FADE: f64 = 0.6;

/// The one note a host shows for a debug key (#367).
#[derive(Default)]
pub struct Note {
    shown: Option<Shown>,
}

struct Shown {
    key: String,
    text: String,
    /// egui time it was first painted; `None` until then, so a note said
    /// between frames gets its whole time on screen.
    since: Option<f64>,
}

impl Note {
    /// Show `text` for `key` and print it to the console.
    pub fn say(&mut self, key: &str, text: &str) {
        tracing::info!(target: "player", "[debug] {key}: {text}");
        self.shown = Some(Shown {
            key: key.to_string(),
            text: text.to_string(),
            since: None,
        });
    }

    /// The note showing, as `(key, text)`.
    pub fn showing(&self) -> Option<(&str, &str)> {
        self.shown
            .as_ref()
            .map(|s| (s.key.as_str(), s.text.as_str()))
    }

    /// The note's strength at egui time `now` (1 held, falling to 0 over the
    /// fade), starting its clock on the first call. Clears it once it's done.
    pub fn strength(&mut self, now: f64) -> Option<f32> {
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
    pub fn paint(&mut self, ctx: &egui::Context, top: f32) {
        let now = ctx.input(|i| i.time);
        let Some(a) = self.strength(now) else {
            return;
        };
        let Some(shown) = &self.shown else {
            return;
        };
        // The HUD blends in display space: build every translucent colour
        // premultiplied, scaled by the note's strength (crates/flint-player
        // CLAUDE.md, the HUD blend).
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
                                        egui::RichText::new(&shown.key)
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
