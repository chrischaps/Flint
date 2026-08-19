//! The "Manifest Map" debug panel: a full-width bottom strip mapping the
//! whole suite — sections, bar ruler, tempo/meter changes, re-entry points —
//! with a playhead riding the current sample and this-run history (judged
//! pulses, full-fail seams). Toggled with Backslash while a music session
//! runs; compiled out entirely without the `debug-hud` feature. Judgment-
//! shaped dev surface by design — it never ships in the felt experience
//! (the no-gamified-UI rule).

use flint_debug_ui::{DebugPanel, PanelLayout};
use flint_music::chart_session::{
    PulseKind, SeamMarkKind, TimelineFrame, TimelineMap,
};

pub const MANIFEST_MAP_PANEL: &str = "Manifest Map";

/// Strip heights (logical points).
const SECTION_H: f32 = 18.0;
const RULER_H: f32 = 14.0;
const HISTORY_H: f32 = 40.0;

pub struct ManifestMapPanel {
    open: bool,
    map: TimelineMap,
    frame: TimelineFrame,
}

impl ManifestMapPanel {
    pub fn new() -> Self {
        Self {
            open: false,
            map: TimelineMap::default(),
            frame: TimelineFrame::default(),
        }
    }

    /// The static suite structure — set once at panel registration.
    pub fn set_map(&mut self, map: TimelineMap) {
        self.map = map;
    }

    /// Per-frame playhead + history feed (only while open).
    pub fn set_frame(&mut self, frame: TimelineFrame) {
        self.frame = frame;
    }
}

fn seam_color(kind: SeamMarkKind) -> egui::Color32 {
    match kind {
        SeamMarkKind::FullFail => egui::Color32::from_rgb(235, 90, 90),
        SeamMarkKind::Seam => egui::Color32::from_rgb(255, 165, 70),
        SeamMarkKind::ReassemblyComplete => egui::Color32::from_rgb(120, 180, 255),
    }
}

impl DebugPanel for ManifestMapPanel {
    fn name(&self) -> &str {
        MANIFEST_MAP_PANEL
    }

    fn layout(&self) -> PanelLayout {
        PanelLayout::Bottom
    }

    fn ui(&mut self, ui: &mut egui::Ui) {
        let map = &self.map;
        let f = &self.frame;
        if map.end_sample <= 0 {
            ui.label(egui::RichText::new("no suite map").weak());
            return;
        }

        // Status line: the raw/suite clock relationship the strip visualizes.
        ui.label(
            egui::RichText::new(format!(
                "raw {}  offset {}  suite {}{}",
                f.raw_clock_sample,
                f.timeline_offset,
                f.playhead_sample,
                if f.preroll { "  (preroll)" } else { "" }
            ))
            .monospace()
            .size(10.0)
            .weak(),
        );

        let height = SECTION_H + RULER_H + HISTORY_H;
        let (resp, painter) =
            ui.allocate_painter(egui::vec2(ui.available_width(), height), egui::Sense::hover());
        let rect = resp.rect;

        // Suite-sample → x. Domain starts at min(playhead, 0) so the preroll
        // playhead is visible approaching zero.
        let min_s = f.playhead_sample.min(0);
        let span = (map.end_sample - min_s).max(1) as f32;
        let to_x = |sample: i64| rect.left() + (sample - min_s) as f32 / span * rect.width();

        let sec_top = rect.top();
        let ruler_top = sec_top + SECTION_H;
        let hist_top = ruler_top + RULER_H;
        let hist_mid = hist_top + HISTORY_H * 0.5;

        // Preroll region: everything left of sample 0, shaded dark.
        if min_s < 0 {
            let zero_x = to_x(0);
            painter.rect_filled(
                egui::Rect::from_min_max(rect.min, egui::pos2(zero_x, rect.bottom())),
                0.0,
                egui::Color32::from_gray(18),
            );
        }

        // Lane 1 — sections band: alternating fills, names when they fit,
        // re-entry sections outlined with a diamond at their start.
        for (i, s) in map.sections.iter().enumerate() {
            let x0 = to_x(s.start_sample);
            let x1 = to_x(s.end_sample);
            let band = egui::Rect::from_min_max(
                egui::pos2(x0, sec_top),
                egui::pos2(x1, sec_top + SECTION_H),
            );
            let fill = if i % 2 == 0 {
                egui::Color32::from_gray(48)
            } else {
                egui::Color32::from_gray(38)
            };
            painter.rect_filled(band, 0.0, fill);
            if s.re_entry {
                painter.rect_stroke(
                    band.shrink(0.5),
                    0.0,
                    egui::Stroke::new(1.0, egui::Color32::from_rgb(255, 200, 120)),
                );
                let c = egui::pos2(x0 + 5.0, sec_top + SECTION_H * 0.5);
                let r = 3.5;
                painter.add(egui::Shape::convex_polygon(
                    vec![
                        egui::pos2(c.x, c.y - r),
                        egui::pos2(c.x + r, c.y),
                        egui::pos2(c.x, c.y + r),
                        egui::pos2(c.x - r, c.y),
                    ],
                    egui::Color32::from_rgb(255, 200, 120),
                    egui::Stroke::NONE,
                ));
            }
            let galley = painter.layout_no_wrap(
                s.name.clone(),
                egui::FontId::monospace(9.0),
                egui::Color32::from_gray(200),
            );
            let text_x = x0 + if s.re_entry { 11.0 } else { 3.0 };
            if text_x + galley.size().x <= x1 - 2.0 {
                painter.galley(
                    egui::pos2(text_x, sec_top + (SECTION_H - galley.size().y) * 0.5),
                    galley,
                    egui::Color32::from_gray(200),
                );
            }
        }

        // Lane 2 — bar ruler, density-thinned so ticks never smear.
        if map.bars.len() > 1 {
            let px_per_bar = rect.width() / map.bars.len() as f32;
            let mut stride = 1usize;
            while px_per_bar * (stride as f32) < 6.0 {
                stride *= 2;
            }
            let label_ok = px_per_bar * stride as f32 >= 30.0;
            for b in map.bars.iter().filter(|b| b.bar % stride as i64 == 0) {
                let x = to_x(b.sample);
                painter.line_segment(
                    [egui::pos2(x, ruler_top), egui::pos2(x, ruler_top + 5.0)],
                    egui::Stroke::new(1.0, egui::Color32::from_gray(110)),
                );
                if label_ok {
                    painter.text(
                        egui::pos2(x + 2.0, ruler_top + RULER_H * 0.5),
                        egui::Align2::LEFT_CENTER,
                        b.bar.to_string(),
                        egui::FontId::monospace(8.0),
                        egui::Color32::from_gray(140),
                    );
                }
            }
        }

        // Tempo/meter change flags (the sample-0 anchor is the opening
        // tempo, not a change — flag it only when later anchors exist).
        let show_first = map.tempo_marks.len() > 1;
        for (i, t) in map.tempo_marks.iter().enumerate() {
            if i == 0 && !show_first {
                continue;
            }
            let x = to_x(t.sample);
            let color = egui::Color32::from_rgb(150, 210, 255);
            painter.line_segment(
                [egui::pos2(x, ruler_top), egui::pos2(x, hist_top + HISTORY_H)],
                egui::Stroke::new(1.0, color),
            );
            painter.text(
                egui::pos2(x + 3.0, ruler_top + RULER_H - 2.0),
                egui::Align2::LEFT_BOTTOM,
                format!("{}/{} {:.0}", t.beats_per_bar, t.beat_unit, t.bpm),
                egui::FontId::monospace(8.0),
                color,
            );
        }

        // Lane 3 — this-run history. Pulse ticks: hit up-green, miss
        // down-red, spurious mid-yellow (the guide panel's color language).
        painter.line_segment(
            [egui::pos2(rect.left(), hist_mid), egui::pos2(rect.right(), hist_mid)],
            egui::Stroke::new(0.5, egui::Color32::from_gray(55)),
        );
        for p in &f.pulses {
            let x = to_x(p.sample);
            let (y0, y1, color) = match p.kind {
                PulseKind::Hit => (
                    hist_mid - 12.0,
                    hist_mid,
                    egui::Color32::from_rgb(140, 220, 130),
                ),
                PulseKind::Miss => (
                    hist_mid,
                    hist_mid + 12.0,
                    egui::Color32::from_rgb(235, 90, 90),
                ),
                PulseKind::Spurious => (
                    hist_mid - 5.0,
                    hist_mid + 5.0,
                    egui::Color32::from_rgb(230, 210, 110),
                ),
            };
            painter.line_segment(
                [egui::pos2(x, y0), egui::pos2(x, y1)],
                egui::Stroke::new(1.0, color),
            );
        }

        // Seams: full-fail line with a connector back to its re-entry point,
        // an orange chevron at each committed re-entry, a blue reassembly tick.
        for s in &f.seams {
            let x = to_x(s.suite_sample);
            let color = seam_color(s.kind);
            match s.kind {
                SeamMarkKind::FullFail => {
                    painter.line_segment(
                        [egui::pos2(x, hist_top), egui::pos2(x, hist_top + HISTORY_H)],
                        egui::Stroke::new(1.5, color),
                    );
                    let rx = to_x(s.re_entry_sample);
                    let y = hist_top + 3.0;
                    painter.line_segment(
                        [egui::pos2(x, y), egui::pos2(rx, y)],
                        egui::Stroke::new(1.0, color.gamma_multiply(0.6)),
                    );
                    // Arrowhead at the re-entry end: the rewind's direction.
                    painter.line_segment(
                        [egui::pos2(rx, y), egui::pos2(rx + 4.0, y - 3.0)],
                        egui::Stroke::new(1.0, color.gamma_multiply(0.6)),
                    );
                    painter.line_segment(
                        [egui::pos2(rx, y), egui::pos2(rx + 4.0, y + 3.0)],
                        egui::Stroke::new(1.0, color.gamma_multiply(0.6)),
                    );
                }
                SeamMarkKind::Seam => {
                    let y = hist_top + HISTORY_H - 5.0;
                    let stroke = egui::Stroke::new(1.5, color);
                    painter.line_segment(
                        [egui::pos2(x - 4.0, y - 4.0), egui::pos2(x, y)],
                        stroke,
                    );
                    painter.line_segment(
                        [egui::pos2(x, y), egui::pos2(x - 4.0, y + 4.0)],
                        stroke,
                    );
                }
                SeamMarkKind::ReassemblyComplete => {
                    painter.line_segment(
                        [
                            egui::pos2(x, hist_top + HISTORY_H - 8.0),
                            egui::pos2(x, hist_top + HISTORY_H),
                        ],
                        egui::Stroke::new(1.5, color),
                    );
                }
            }
        }

        // Playhead: full height, weak while preroll.
        let px = to_x(f.playhead_sample);
        let (ph_color, ph_w) = if f.preroll {
            (egui::Color32::from_gray(120), 1.0)
        } else {
            (egui::Color32::WHITE, 2.0)
        };
        painter.line_segment(
            [egui::pos2(px, rect.top()), egui::pos2(px, rect.bottom())],
            egui::Stroke::new(ph_w, ph_color),
        );
        painter.add(egui::Shape::convex_polygon(
            vec![
                egui::pos2(px - 4.0, rect.top()),
                egui::pos2(px + 4.0, rect.top()),
                egui::pos2(px, rect.top() + 5.0),
            ],
            ph_color,
            egui::Stroke::NONE,
        ));

        // Hover: cursor x → sample/bar readout.
        if let Some(pos) = resp.hover_pos() {
            let sample = min_s
                + ((pos.x - rect.left()) / rect.width().max(1.0) * span) as i64;
            let bar = match map.bars.binary_search_by_key(&sample, |b| b.sample) {
                Ok(i) => map.bars[i].bar,
                Err(0) => -1,
                Err(i) => map.bars[i - 1].bar,
            };
            resp.on_hover_text(
                egui::RichText::new(format!("sample {sample}  bar {bar}"))
                    .monospace()
                    .size(10.0),
            );
        }
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
