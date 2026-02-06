//! HUD overlay — crosshair and interaction prompts
//!
//! Renders a minimal egui overlay on top of the 3D scene:
//! - Center crosshair dot
//! - Interaction prompt when near an interactable entity (fade in/out)

use flint_core::EntityId;
use flint_ecs::FlintWorld;
use flint_script::engine::find_nearest_interactable;

/// Current HUD state tracked across frames
pub struct HudState {
    /// Currently targeted interactable
    pub target_entity: Option<EntityId>,
    pub prompt_text: String,
    pub interaction_type: String,
    /// Fade alpha for the interaction prompt (0.0..1.0)
    pub prompt_alpha: f32,
}

impl HudState {
    pub fn new() -> Self {
        Self {
            target_entity: None,
            prompt_text: String::new(),
            interaction_type: String::new(),
            prompt_alpha: 0.0,
        }
    }

    /// Scan the world for the nearest interactable and update fade state
    pub fn update(&mut self, world: &FlintWorld, dt: f64) {
        let fade_speed = 5.0_f32; // ~0.2s fade

        match find_nearest_interactable(world) {
            Some(nearest) => {
                self.target_entity = Some(nearest.entity_id);
                self.prompt_text = nearest.prompt_text;
                self.interaction_type = nearest.interaction_type;
                self.prompt_alpha = (self.prompt_alpha + fade_speed * dt as f32).min(1.0);
            }
            None => {
                self.prompt_alpha = (self.prompt_alpha - fade_speed * dt as f32).max(0.0);
                if self.prompt_alpha <= 0.0 {
                    self.target_entity = None;
                }
            }
        }
    }

    /// Render the HUD via egui
    pub fn render(&self, ctx: &egui::Context) {
        // Draw crosshair and prompt as an overlay area (no panel, no background)
        egui::Area::new(egui::Id::new("hud_overlay"))
            .order(egui::Order::Foreground)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .interactable(false)
            .show(ctx, |ui| {
                // Crosshair dot
                let painter = ui.painter();
                let screen_center = ctx.screen_rect().center();
                painter.circle_filled(
                    screen_center,
                    2.0,
                    egui::Color32::from_rgba_unmultiplied(255, 255, 255, 140),
                );
                // Thin ring around dot for visibility on bright backgrounds
                painter.circle_stroke(
                    screen_center,
                    3.5,
                    egui::Stroke::new(0.8, egui::Color32::from_rgba_unmultiplied(0, 0, 0, 80)),
                );
            });

        // Interaction prompt (bottom-center, fades in/out)
        if self.prompt_alpha > 0.01 {
            let alpha = (self.prompt_alpha * 255.0) as u8;

            // Build the prompt string based on interaction type
            let verb = match self.interaction_type.as_str() {
                "talk" => "Talk to",
                "examine" => "Examine",
                _ => "Use",
            };
            let prompt = format!("[E] {} {}", verb, self.prompt_text);

            egui::Area::new(egui::Id::new("hud_prompt"))
                .order(egui::Order::Foreground)
                .anchor(egui::Align2::CENTER_BOTTOM, egui::Vec2::new(0.0, -60.0))
                .interactable(false)
                .show(ctx, |ui| {
                    let bg = egui::Color32::from_rgba_unmultiplied(10, 10, 10, (180.0 * self.prompt_alpha) as u8);
                    let text_color = egui::Color32::from_rgba_unmultiplied(240, 235, 220, alpha);

                    egui::Frame::none()
                        .fill(bg)
                        .rounding(12.0)
                        .inner_margin(egui::Margin::symmetric(20.0, 10.0))
                        .show(ui, |ui: &mut egui::Ui| {
                            ui.label(
                                egui::RichText::new(&prompt)
                                    .color(text_color)
                                    .size(16.0),
                            );
                        });
                });
        }
    }
}

impl Default for HudState {
    fn default() -> Self {
        Self::new()
    }
}
