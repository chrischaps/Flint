//! The "Music Guide" debug panel (Phase 4 F8 slice, ADR 0035): a summonable
//! egui overlay that shows what input the running chart expects — upcoming
//! pulse/press/flick windows with a countdown, and per-channel targets next
//! to the player's actual stick/trigger state. Toggled with Backquote while
//! a music session runs; compiled out entirely without the `debug-hud`
//! feature. This is judgment-shaped dev surface by design — it never ships
//! in the felt experience (the no-gamified-UI rule).

use flint_debug_ui::DebugPanel;
use flint_music::chart_session::{GuideFrame, GuideWindow};
use flint_music::VisualFrame;

pub const MUSIC_GUIDE_PANEL: &str = "Music Guide";

/// Beats of lookahead shown in the upcoming list.
pub const GUIDE_HORIZON_BEATS: f64 = 8.0;
/// Max upcoming rows drawn (windows can cluster in dense bars).
const MAX_ROWS: usize = 8;

pub struct MusicGuidePanel {
    open: bool,
    frame: VisualFrame,
    guide: GuideFrame,
}

impl MusicGuidePanel {
    pub fn new() -> Self {
        Self {
            open: false,
            frame: VisualFrame::default(),
            guide: GuideFrame::default(),
        }
    }

    /// Per-frame data feed from the running session (only while open).
    pub fn set_data(&mut self, frame: VisualFrame, guide: GuideFrame) {
        self.frame = frame;
        self.guide = guide;
    }
}

/// 8-way arrow for an authored direction (screen convention: +y up).
fn arrow_for(dir: [f64; 2]) -> &'static str {
    const ARROWS: [&str; 8] = ["→", "↗", "↑", "↖", "←", "↙", "↓", "↘"];
    let angle = dir[1].atan2(dir[0]); // -π..π
    let octant = ((angle / (std::f64::consts::PI / 4.0)).round() as i64).rem_euclid(8);
    ARROWS[octant as usize]
}

/// One window as a verb + how: "PULSE (A)", "PRESS trigger ≥ 0.6", "FLICK ↗".
fn describe(w: &GuideWindow) -> String {
    match w.kind.as_str() {
        "pulse" => "PULSE  (A / South)".to_string(),
        "press" => match w.strength {
            Some(s) => format!("PRESS  trigger ≥ {s:.2}"),
            None => "PRESS  trigger".to_string(),
        },
        "flick" => match w.direction {
            Some(d) => format!("FLICK  right stick {}", arrow_for(d)),
            None => "FLICK  right stick".to_string(),
        },
        other => other.to_uppercase(),
    }
}

fn kind_color(kind: &str) -> egui::Color32 {
    match kind {
        "pulse" => egui::Color32::from_rgb(255, 200, 120), // warm — the beat verb
        "press" => egui::Color32::from_rgb(150, 210, 255), // cool — trigger depth
        "flick" => egui::Color32::from_rgb(190, 255, 170), // green — gesture
        _ => egui::Color32::LIGHT_GRAY,
    }
}

/// A small 2D stick pad: square with crosshair, ring = authored target,
/// dot = player actual. Values in [-1,1]²; chart convention +y up.
fn stick_pad(ui: &mut egui::Ui, label: &str, actual: [f32; 2], target: Option<[f32; 2]>) {
    ui.vertical(|ui| {
        ui.label(egui::RichText::new(label).size(10.0).weak());
        let size = 72.0;
        let (resp, painter) = ui.allocate_painter(egui::vec2(size, size), egui::Sense::hover());
        let rect = resp.rect;
        let center = rect.center();
        let half = size / 2.0 - 2.0;
        let to_screen = |v: [f32; 2]| {
            egui::pos2(
                center.x + v[0].clamp(-1.0, 1.0) * half,
                center.y - v[1].clamp(-1.0, 1.0) * half,
            )
        };
        painter.rect_stroke(
            rect.shrink(1.0),
            2.0,
            egui::Stroke::new(1.0, egui::Color32::from_gray(90)),
        );
        let cross = egui::Stroke::new(0.5, egui::Color32::from_gray(60));
        painter.line_segment(
            [
                egui::pos2(rect.left(), center.y),
                egui::pos2(rect.right(), center.y),
            ],
            cross,
        );
        painter.line_segment(
            [
                egui::pos2(center.x, rect.top()),
                egui::pos2(center.x, rect.bottom()),
            ],
            cross,
        );
        if let Some(t) = target {
            painter.circle_stroke(
                to_screen(t),
                5.0,
                egui::Stroke::new(1.5, egui::Color32::from_rgb(255, 200, 120)),
            );
        }
        painter.circle_filled(
            to_screen(actual),
            3.5,
            egui::Color32::from_rgb(150, 210, 255),
        );
    });
}

/// A trigger depth bar with an optional target notch.
fn trigger_bar(ui: &mut egui::Ui, label: &str, actual: f32, target: Option<f64>) {
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(label).size(10.0).weak());
        let (resp, painter) = ui.allocate_painter(egui::vec2(120.0, 12.0), egui::Sense::hover());
        let rect = resp.rect;
        painter.rect_stroke(
            rect,
            2.0,
            egui::Stroke::new(1.0, egui::Color32::from_gray(90)),
        );
        let fill = egui::Rect::from_min_size(
            rect.min,
            egui::vec2(rect.width() * actual.clamp(0.0, 1.0), rect.height()),
        );
        painter.rect_filled(
            fill.shrink(1.0),
            1.0,
            egui::Color32::from_rgb(150, 210, 255),
        );
        if let Some(t) = target {
            let x = rect.left() + rect.width() * (t.clamp(0.0, 1.0) as f32);
            painter.line_segment(
                [egui::pos2(x, rect.top()), egui::pos2(x, rect.bottom())],
                egui::Stroke::new(2.0, egui::Color32::from_rgb(255, 200, 120)),
            );
        }
        ui.label(
            egui::RichText::new(match target {
                Some(t) => format!("{actual:.2} / {t:.2}"),
                None => format!("{actual:.2}"),
            })
            .size(10.0)
            .monospace(),
        );
    });
}

impl DebugPanel for MusicGuidePanel {
    fn name(&self) -> &str {
        MUSIC_GUIDE_PANEL
    }

    fn ui(&mut self, ui: &mut egui::Ui) {
        let f = &self.frame;
        // Position + coherence — the CLI status line's information, live.
        ui.label(
            egui::RichText::new(format!(
                "bar {}  beat {:.2}  {}  coherence {:.2}",
                f.bar,
                f.beat,
                if f.section.is_empty() {
                    "—"
                } else {
                    &f.section
                },
                f.coherence
            ))
            .monospace()
            .size(11.0),
        );
        if f.preroll {
            ui.label(egui::RichText::new("preroll…").italics());
        }
        if f.no_input {
            ui.colored_label(egui::Color32::RED, "NO INPUT SEEN — check the pad");
        }
        ui.separator();

        // NOW: any window currently open is the thing to do this instant.
        let open: Vec<&GuideWindow> = self.guide.windows.iter().filter(|w| w.open_now).collect();
        if open.is_empty() {
            ui.label(
                egui::RichText::new("now: follow the curves")
                    .weak()
                    .size(11.0),
            );
        } else {
            for w in open {
                ui.label(
                    egui::RichText::new(format!("NOW  {}", describe(w)))
                        .color(kind_color(&w.kind))
                        .strong()
                        .size(15.0),
                );
            }
        }
        ui.separator();

        // Upcoming windows within the horizon, soonest first.
        ui.label(egui::RichText::new("upcoming").weak().size(10.0));
        let mut rows = 0;
        for w in self
            .guide
            .windows
            .iter()
            .filter(|w| !w.open_now && w.beats_until > 0.0)
        {
            if rows >= MAX_ROWS {
                ui.label(egui::RichText::new("…").weak());
                break;
            }
            rows += 1;
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(format!("in {:>4.1} beats", w.beats_until))
                        .monospace()
                        .size(11.0),
                );
                ui.label(
                    egui::RichText::new(describe(w))
                        .color(kind_color(&w.kind))
                        .size(11.0),
                );
            });
        }
        if rows == 0 {
            ui.label(
                egui::RichText::new("(none in the next 8 beats)")
                    .weak()
                    .size(10.0),
            );
        }
        ui.separator();

        // Continuous channels: actual vs authored target.
        ui.horizontal(|ui| {
            stick_pad(ui, "lean (L stick)", f.lean, Some(f.target));
            stick_pad(
                ui,
                "sway (R stick)",
                f.sway,
                self.guide.sway_target.map(|t| [t[0] as f32, t[1] as f32]),
            );
        });
        trigger_bar(ui, "LT", f.pressure_l, self.guide.pressure_l_target);
        trigger_bar(ui, "RT", f.pressure_r, self.guide.pressure_r_target);
        ui.add_space(4.0);
        ui.label(
            egui::RichText::new("ring = chart target · dot = you · notch = required depth")
                .weak()
                .size(9.0),
        );
    }

    fn is_open(&self) -> bool {
        self.open
    }

    fn toggle(&mut self) {
        self.open = !self.open;
    }

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
