//! Painting the on-screen touch controls (#133), and keeping their fingers
//! from egui.
//!
//! `flint_runtime::TouchControls` decides what is where and which finger
//! holds what; this draws its [`TouchWidget`]s in their own egui layer on
//! the Foreground order, over script UI and the game's egui, in logical
//! points. The look is quiet glass: a dark disc under a thin light ring,
//! so the controls read on bright and dark scenes alike without hiding
//! either. A stick's ring lights up on the side it's pushed toward.

use super::PlayerApp;
use egui::{Color32, Pos2, Stroke};
use flint_runtime::TouchWidget;
use winit::event::{Touch, TouchPhase};

/// The layer the controls draw on.
pub(super) fn layer() -> egui::LayerId {
    egui::LayerId::new(
        egui::Order::Foreground,
        egui::Id::new("flint_touch_controls"),
    )
}

/// Alpha `a` (0-1) of the controls' opacity, as a byte.
fn alpha(a: f32, opacity: f32) -> u8 {
    (a * opacity * 255.0).round().clamp(0.0, 255.0) as u8
}

/// White at that alpha, premultiplied here. The HUD blends in display
/// space, but egui's own constructors (`from_white_alpha`,
/// `from_rgba_unmultiplied`) premultiply in linear light, so a 13% white
/// read as nearly 70% over the scene.
fn light(a: f32, opacity: f32) -> Color32 {
    let a = alpha(a, opacity);
    Color32::from_rgba_premultiplied(a, a, a, a)
}

fn shade(a: f32, opacity: f32) -> Color32 {
    Color32::from_rgba_premultiplied(0, 0, 0, alpha(a, opacity))
}

fn pos(p: [f32; 2]) -> Pos2 {
    Pos2::new(p[0], p[1])
}

/// Draw every widget.
pub(super) fn paint(ctx: &egui::Context, widgets: &[TouchWidget], opacity: f32) {
    if widgets.is_empty() {
        return;
    }
    let painter = ctx.layer_painter(layer());
    for widget in widgets {
        match widget {
            TouchWidget::Stick {
                base,
                knob,
                radius,
                value,
                held,
            } => {
                // At rest a floating stick is a hint of where to put a thumb.
                let o = if *held { opacity } else { opacity * 0.6 };
                let c = pos(*base);
                painter.circle(
                    c,
                    *radius,
                    shade(0.22, o),
                    Stroke::new(1.5_f32, light(0.55, o)),
                );
                // The ring lights toward the push, brighter the further.
                let mag = (value[0] * value[0] + value[1] * value[1]).sqrt();
                if mag > 0.01 {
                    let angle = (-value[1]).atan2(value[0]);
                    let half = 0.55 + 0.25 * mag;
                    let steps = 24;
                    let arc: Vec<Pos2> = (0..=steps)
                        .map(|i| {
                            let a = angle - half + 2.0 * half * i as f32 / steps as f32;
                            c + egui::vec2(a.cos(), a.sin()) * *radius
                        })
                        .collect();
                    painter.add(egui::Shape::line(
                        arc,
                        Stroke::new(3.0_f32, light(0.5 + 0.5 * mag, o)),
                    ));
                }
                // A faint inner circle marks the travel's centre.
                painter.circle_stroke(c, radius * 0.18, Stroke::new(1.0_f32, light(0.22, o)));
                let k = pos(*knob);
                let knob_r = radius * 0.42;
                painter.circle(
                    k,
                    knob_r,
                    light(if *held { 0.6 } else { 0.32 }, o),
                    Stroke::new(1.5_f32, light(0.8, o)),
                );
            }
            TouchWidget::Button {
                center,
                radius,
                label,
                held,
            } => {
                let c = pos(*center);
                let (fill, ring) = if *held {
                    (light(0.6, opacity), light(0.9, opacity))
                } else {
                    (shade(0.24, opacity), light(0.62, opacity))
                };
                painter.circle(c, *radius, fill, Stroke::new(1.5_f32, ring));
                // A word fits a small face; a lone glyph (an arrow) reads
                // as an icon, so it gets a larger one.
                let size = if label.chars().count() == 1 {
                    (radius * 0.62).clamp(14.0, 30.0)
                } else {
                    (radius * 0.34).clamp(10.0, 16.0)
                };
                let text = if *held {
                    shade(0.85, opacity)
                } else {
                    light(0.9, opacity)
                };
                painter.text(
                    c,
                    egui::Align2::CENTER_CENTER,
                    label.to_uppercase(),
                    egui::FontId::proportional(size),
                    text,
                );
            }
            TouchWidget::Menu {
                center,
                radius,
                held,
            } => {
                let c = pos(*center);
                let fill = if *held {
                    light(0.6, opacity)
                } else {
                    shade(0.28, opacity)
                };
                painter.circle(c, *radius, fill, Stroke::new(1.5_f32, light(0.62, opacity)));
                // Three bars: the menu everyone knows.
                let w = radius * 0.46;
                let bar = Stroke::new(2.0_f32, light(0.9, opacity));
                for dy in [-0.32, 0.0, 0.32] {
                    let y = c.y + radius * dy;
                    painter.line_segment([Pos2::new(c.x - w, y), Pos2::new(c.x + w, y)], bar);
                }
            }
        }
    }
}

impl PlayerApp {
    /// Hand the controls this frame's layout metrics: the surface size, the
    /// window's points-to-pixels scale, and the page's safe-area insets.
    pub(super) fn sync_touch_controls(&mut self) {
        let screen = self.input.screen_size();
        let ppp = self.egui_ctx.pixels_per_point() as f64;
        #[cfg(target_arch = "wasm32")]
        let insets = super::web_page::safe_area_insets();
        #[cfg(not(target_arch = "wasm32"))]
        let insets = [0.0; 4];
        self.input
            .touch_controls_mut()
            .set_metrics(screen, ppp, insets);
        // A finger can't look through a captured cursor: let it go when the
        // controls come up (a touchscreen laptop, or `--touch-controls
        // always`). Android's capture is only a flag.
        #[cfg(not(target_os = "android"))]
        if self.cursor_captured && self.input.touch_controls_visible() {
            self.release_cursor();
        }
    }

    /// Whether a finger belongs to a stick, button or the menu: it then
    /// skips egui and extensions, as the controls sit above both.
    pub(super) fn touch_controls_take(&self, touch: &Touch) -> bool {
        let controls = self.input.touch_controls();
        match touch.phase {
            TouchPhase::Started => {
                let (x, y) = (touch.location.x, touch.location.y);
                #[cfg(target_arch = "wasm32")]
                let (x, y) = self.touch_to_surface(x, y);
                controls.would_grab((x, y))
            }
            _ => controls.holds(touch.id),
        }
    }
}
