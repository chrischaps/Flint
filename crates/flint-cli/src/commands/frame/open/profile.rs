//! The profiler tab (#1099): a scope trace (#1097) as a picture.
//!
//! Along the top, the frame strip: every frame of the trace as one bar, its
//! phases in the F2 overlay's bands, hitches marked; click a frame, drag a
//! range, or step with the arrow keys. In the middle, the chosen frames as
//! a flame (icicle) chart: the phases, each thread's scopes nested under
//! their parents, the script callbacks and the GPU passes, each bar
//! coloured by the system it ran in, in the overlay's colours, so the two
//! tools read as one. On the right, every scope's statistics over the
//! selection; choosing one marks each of its calls and plots its time
//! across the whole trace. **Compare** tints each bar by how much longer
//! its path took than in the median frame, so a hitch's culprit is the one
//! red line down the chart.
//!
//! All the arithmetic is [`Profile`]'s, testable without egui; this is the
//! layout, the painting and the keys.

use super::ui::{elided, ink};
use crate::commands::frame::open::timing::band_of;
use crate::commands::trace::profile::{is_script, Profile, ScopeStat, StatSort};
use crate::commands::trace::Trace;
use egui::{Align, Color32, FontId, Key, Layout, Pos2, Rect, RichText, Sense, Stroke, Vec2};
use flint_debug_ui::widgets::{
    band_legend, Band, FrameGraph, GraphFrame, GraphLayout, HITCH_COLOR,
};
use std::ops::Range;

/// Which frames the tab opens on.
#[derive(Clone, Debug, PartialEq)]
pub enum FrameChoice {
    /// A recorder frame index, as the trace numbers it.
    Index(u32),
    /// Frames `a` to `b`, both included.
    Range(u32, u32),
    /// The hitch the file means: the capture's, else the one a trace was
    /// saved for, else the worst (`trace::meant_hitch`, the rule `flint
    /// profile`'s `hitch` follows too), else the longest frame.
    Hitch,
    /// The longest frame.
    Worst,
}

impl FrameChoice {
    pub fn parse(s: &str) -> Result<Self, String> {
        let s = s.trim();
        match s {
            "hitch" => return Ok(Self::Hitch),
            "worst" => return Ok(Self::Worst),
            _ => {}
        }
        let num = |v: &str| {
            v.trim()
                .parse::<u32>()
                .map_err(|_| format!("{s:?} is not a frame: a number, A..B, `hitch` or `worst`"))
        };
        match s.split_once("..") {
            Some((a, b)) => {
                let (a, b) = (num(a)?, num(b.trim_start_matches('='))?);
                Ok(Self::Range(a.min(b), a.max(b)))
            }
            None => Ok(Self::Index(num(s)?)),
        }
    }
}

/// How the tab opens.
#[derive(Clone, Debug, Default)]
pub struct ProfileStart {
    pub frame: Option<FrameChoice>,
    /// A scope chosen by name: its calls marked, its time plotted.
    pub scope: Option<String>,
    pub compare: bool,
    /// The capture's hitch (its `cpu.hitch.frame`, the player's history
    /// frame), which a trace's hitch names too.
    pub capture_hitch: Option<i64>,
    /// The frame a capture holds (its manifest's `frame.index`), which the
    /// trace numbers the same: the tab opens on it when the capture wasn't
    /// taken on a hitch, so P switches between two views of one frame.
    pub capture_frame: Option<u32>,
}

/// A row of the chart, points.
const ROW_H: f32 = 18.0;
/// A lane's title row.
const LANE_HEAD: f32 = 17.0;
/// The time ruler.
const RULER_H: f32 = 32.0;
/// The strip's height.
const STRIP_H: f32 = 86.0;
/// The series plot's height.
const PLOT_H: f32 = 104.0;
/// A bar's grey when it ran across systems (`tick`).
const NEUTRAL: Color32 = Color32::from_rgb(112, 114, 122);
/// Text on a bar.
const ON_BAR: Color32 = Color32::from_rgb(16, 17, 20);
/// The budget line the strip draws: a 60 Hz frame.
const BUDGET_MS: f32 = 1000.0 / 60.0;
/// The ring round the bar that carries a frame's excess under Compare.
const CARRIER_RING: Color32 = Color32::from_rgb(255, 214, 120);

/// What the pointer is over in the chart.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Hover {
    Scope(usize, usize),
    Phase(usize),
    Gpu(usize),
    Note(usize),
    Hitch(usize),
}

pub struct Profiler {
    p: Profile,
    shown: String,
    graph: FrameGraph,
    graph_frames: Vec<GraphFrame>,
    /// Whether each name is a script callback.
    script_name: Vec<bool>,
    /// The frames chosen, rows.
    sel: Range<usize>,
    anchor: usize,
    /// The chart's window, ns.
    view: (f64, f64),
    scroll_y: f32,
    stats: Vec<ScopeStat>,
    stats_for: Option<Range<usize>>,
    sort: StatSort,
    descending: bool,
    filter: String,
    chosen: Option<u32>,
    series: Vec<f32>,
    compare: bool,
    /// The tab's own time to lay out last frame, ms.
    drawn_ms: f32,
    notice: Option<String>,
    /// A range is being dragged out: the statistics wait for its end.
    dragging: bool,
}

fn lerp(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let m = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgb(m(a.r(), b.r()), m(a.g(), b.g()), m(a.b(), b.b()))
}

/// A duration, ms, with as many decimals as it needs.
pub fn ms_text(ns: u64) -> String {
    let ms = ns as f64 / 1e6;
    if ms >= 100.0 {
        format!("{ms:.0}")
    } else if ms >= 10.0 {
        format!("{ms:.1}")
    } else if ms >= 1.0 {
        format!("{ms:.2}")
    } else {
        format!("{ms:.3}")
    }
}

impl Profiler {
    pub fn new(trace: Trace, shown: String, start: &ProfileStart) -> Self {
        let p = Profile::new(trace);
        let t0 = p.rows.first().map_or(0, |r| r.start_ns);
        let graph_frames = p
            .rows
            .iter()
            .map(|r| GraphFrame {
                at: (r.start_ns - t0) as f64 / 1e9,
                wall_ms: r.ms(),
                ms: r.ms(),
                bands: r.bands,
                hitch: r.hitch.is_some(),
            })
            .collect();
        let script_name = p.trace.names.iter().map(|n| is_script(n)).collect();
        let mut tab = Self {
            graph: FrameGraph::new(),
            graph_frames,
            script_name,
            sel: 0..1,
            anchor: 0,
            view: (0.0, 1.0),
            scroll_y: 0.0,
            stats: Vec::new(),
            stats_for: None,
            sort: StatSort::Total,
            descending: true,
            filter: String::new(),
            chosen: None,
            series: Vec::new(),
            compare: false,
            drawn_ms: 0.0,
            notice: None,
            dragging: false,
            shown,
            p,
        };
        let rows = tab.start_rows(start);
        tab.select(rows);
        if let Some(name) = &start.scope {
            match tab.find_scope(name) {
                Some(id) => tab.choose(Some(id)),
                None => {
                    tracing::warn!("no scope named {name:?} in this trace");
                    tab.notice = Some(format!("no scope named {name:?}"));
                }
            }
        }
        if start.compare {
            tab.set_compare(true);
        }
        tab
    }

    /// The rows `start` asks for.
    fn start_rows(&mut self, start: &ProfileStart) -> Range<usize> {
        let n = self.p.rows.len();
        if n == 0 {
            return 0..0;
        }
        let one = |r: usize| r..r + 1;
        // Only a row with its scopes is chosen for the user (#1288): each
        // choice below holds only when its row is covered, and a hitch
        // before the scopes gives way to the worst hitch after them
        let covered = self.p.first_covered;
        let kept = |r: &usize| *r >= covered;
        let capture_hitch = || {
            start
                .capture_hitch
                .and_then(|f| {
                    self.p
                        .trace
                        .hitches
                        .iter()
                        .position(|h| h.history_frame == Some(f))
                })
                .and_then(|h| self.p.hitch_row(h))
                .filter(kept)
        };
        // The one a trace saved for a hitch was saved for, else the worst:
        // `flint profile`'s `hitch`, from the one reader (rows are its frames)
        let meant_hitch = || self.p.trace.meant_hitch_frame();
        let capture_frame = || {
            start
                .capture_frame
                .and_then(|i| self.p.row_of(i))
                .filter(kept)
        };
        let worst = self.p.worst_row().unwrap_or(n - 1);
        match &start.frame {
            Some(FrameChoice::Index(i)) => match self.p.row_of(*i) {
                Some(r) if r < covered => {
                    let first = self.p.rows[covered].index;
                    self.notice = Some(format!(
                        "frame {i} is before the scopes begin (frame {first}): shown as recorded"
                    ));
                    one(r)
                }
                Some(r) => one(r),
                None => {
                    let (a, b) = (self.p.rows[0].index, self.p.rows[n - 1].index);
                    self.notice = Some(format!("no frame {i}: this trace holds {a} to {b}"));
                    tracing::warn!("no frame {i} in this trace (it holds {a} to {b})");
                    one(worst)
                }
            },
            Some(FrameChoice::Range(a, b)) => {
                let lo = self.p.rows.partition_point(|r| r.index < *a);
                let hi = self.p.rows.partition_point(|r| r.index <= *b);
                if lo < hi {
                    lo..hi
                } else {
                    self.notice = Some(format!("no frames {a}..{b} in this trace"));
                    one(worst)
                }
            }
            Some(FrameChoice::Worst) => one(worst),
            Some(FrameChoice::Hitch) => one(capture_hitch().or_else(meant_hitch).unwrap_or(worst)),
            // A capture opens on its own frame (its hitch's, when it was
            // taken on one); a trace alone on the hitch it means
            None => one(capture_hitch()
                .or_else(capture_frame)
                .or_else(meant_hitch)
                .unwrap_or(worst)),
        }
    }

    /// A scope by name: exactly, else the busiest containing it.
    fn find_scope(&self, name: &str) -> Option<u32> {
        if let Some(i) = self.p.trace.names.iter().position(|n| n == name) {
            return Some(i as u32);
        }
        let lower = name.to_lowercase();
        let all = self.p.stats(0..self.p.rows.len());
        all.iter()
            .find(|s| self.p.name(s.name).to_lowercase().contains(&lower))
            .map(|s| s.name)
    }

    /// The rows chosen.
    #[cfg(test)]
    pub fn selection(&self) -> Range<usize> {
        self.sel.clone()
    }

    #[cfg(test)]
    pub fn chosen(&self) -> Option<&str> {
        self.chosen.map(|c| self.p.name(c))
    }

    #[cfg(test)]
    pub fn comparing(&self) -> bool {
        self.compare
    }

    #[cfg(test)]
    pub fn profile(&self) -> &Profile {
        &self.p
    }

    fn select(&mut self, rows: Range<usize>) {
        let n = self.p.rows.len();
        if n == 0 {
            return;
        }
        let a = rows.start.min(n - 1);
        let b = rows.end.clamp(a + 1, n);
        self.sel = a..b;
        self.anchor = a;
        self.fit();
    }

    /// The chart's window to the selection, with a little room each side.
    fn fit(&mut self) {
        if let Some((a, b)) = self.p.span_of(self.sel.clone()) {
            let pad = (b - a) as f64 * 0.015;
            self.view = (a as f64 - pad, b as f64 + pad);
        }
    }

    fn choose(&mut self, name: Option<u32>) {
        self.chosen = name;
        self.series = name.map(|n| self.p.series(n)).unwrap_or_default();
    }

    fn set_compare(&mut self, on: bool) {
        self.compare = on;
        if on {
            self.p.baseline();
        }
    }

    fn stats_now(&mut self) {
        if !self.dragging && self.stats_for.as_ref() != Some(&self.sel) {
            self.stats = self.p.stats(self.sel.clone());
            self.stats_for = Some(self.sel.clone());
            let (sort, desc) = (self.sort, self.descending);
            self.p.sort_stats(&mut self.stats, sort, desc);
        }
    }

    // ── Keys ──

    pub fn keys(&mut self, ctx: &egui::Context) {
        if ctx.egui_wants_keyboard_input() || self.p.rows.is_empty() {
            return;
        }
        let pressed = |k: Key| ctx.input(|i| i.key_pressed(k));
        let shift = ctx.input(|i| i.modifiers.shift);
        let n = self.p.rows.len();
        let mut to: Option<Range<usize>> = None;
        if pressed(Key::ArrowRight) || pressed(Key::L) {
            to = Some(if shift {
                self.sel.start..(self.sel.end + 1).min(n)
            } else {
                let r = (self.sel.end).min(n - 1);
                r..r + 1
            });
        }
        if pressed(Key::ArrowLeft) || pressed(Key::J) {
            to = Some(if shift {
                self.sel.start..(self.sel.end - 1).max(self.sel.start + 1)
            } else {
                let r = self.sel.start.saturating_sub(1);
                r..r + 1
            });
        }
        if pressed(Key::Home) {
            to = Some(0..1);
        }
        if pressed(Key::End) {
            to = Some(n - 1..n);
        }
        if pressed(Key::H) {
            let hitches = self.p.hitch_rows();
            let next = if shift {
                hitches.iter().rev().find(|&&r| r < self.sel.start).copied()
            } else {
                hitches.iter().find(|&&r| r >= self.sel.end).copied()
            }
            .or_else(|| {
                if shift {
                    hitches.last().copied()
                } else {
                    hitches.first().copied()
                }
            });
            if let Some(r) = next {
                to = Some(r..r + 1);
            }
        }
        if let Some(r) = to {
            let keep = self.sel.start;
            self.select(r);
            if shift && !pressed(Key::H) {
                self.anchor = keep;
            }
        }
        let (v0, v1) = self.view;
        let w = v1 - v0;
        if pressed(Key::W) {
            let c = (v0 + v1) / 2.0;
            self.zoom_about(c, 0.7);
        }
        if pressed(Key::S) {
            let c = (v0 + v1) / 2.0;
            self.zoom_about(c, 1.0 / 0.7);
        }
        if pressed(Key::A) {
            self.view = (v0 - w * 0.2, v1 - w * 0.2);
        }
        if pressed(Key::D) {
            self.view = (v0 + w * 0.2, v1 + w * 0.2);
        }
        if pressed(Key::F) {
            self.fit();
        }
        if pressed(Key::C) {
            self.set_compare(!self.compare);
        }
        if pressed(Key::Escape) {
            self.choose(None);
        }
    }

    fn zoom_about(&mut self, at: f64, factor: f64) {
        let (v0, v1) = self.view;
        let whole = self
            .p
            .whole()
            .map_or(1e9, |(a, b)| (b - a) as f64 * 1.2)
            .max(1e4);
        let w = ((v1 - v0) * factor).clamp(2_000.0, whole);
        let k = w / (v1 - v0);
        self.view = (at - (at - v0) * k, at + (v1 - at) * k);
    }

    // ── The tab ──

    pub fn ui(&mut self, ui: &mut egui::Ui) {
        let began = std::time::Instant::now();
        let ctx = ui.ctx().clone();
        self.keys(&ctx);
        self.stats_now();

        egui::Panel::top("profile-header")
            .frame(
                egui::Frame::new()
                    .fill(ink::GROUND)
                    .inner_margin(egui::Margin::symmetric(14, 10)),
            )
            .show(ui, |ui| self.header(ui));
        egui::Panel::top("profile-strip")
            .frame(
                egui::Frame::new()
                    .fill(ink::PANEL)
                    .inner_margin(egui::Margin::symmetric(14, 8)),
            )
            .show(ui, |ui| self.strip(ui));
        egui::Panel::right("profile-stats")
            .resizable(true)
            .default_size(470.0)
            .min_size(320.0)
            .frame(
                egui::Frame::new()
                    .fill(ink::PANEL)
                    .inner_margin(egui::Margin::symmetric(10, 8)),
            )
            .show(ui, |ui| self.side(ui));
        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(ink::GROUND)
                    .inner_margin(egui::Margin::symmetric(0, 0)),
            )
            .show(ui, |ui| self.chart(ui));
        self.drawn_ms = began.elapsed().as_secs_f32() * 1000.0;
    }

    fn header(&mut self, ui: &mut egui::Ui) {
        let t = &self.p.trace;
        let scene = t.header_str("scene").map(str::to_string);
        let title = scene
            .as_deref()
            .map(|s| {
                let stem = s.rsplit(['/', '\\']).next().unwrap_or(s);
                stem.trim_end_matches(".toml")
                    .trim_end_matches(".scene")
                    .to_string()
            })
            .unwrap_or_else(|| "A trace".to_string());
        let compare = self.compare;
        let mut toggle = false;
        ui.horizontal(|ui| {
            ui.label(RichText::new(title).size(19.0).strong().color(ink::TEXT));
            ui.add_space(6.0);
            if let Some(s) = &scene {
                ui.label(RichText::new(s).color(ink::DIM));
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let words = if compare {
                    "Comparing with the median frame"
                } else {
                    "Compare with the median frame"
                };
                if compare {
                    let (r, _) = ui.allocate_exact_size(Vec2::new(64.0, 10.0), Sense::hover());
                    let mut mesh = egui::Mesh::default();
                    let quiet = lerp(Band::Render.color(), Color32::from_gray(70), 0.62);
                    let n = 16;
                    for k in 0..n {
                        let t = k as f32 / (n - 1) as f32;
                        let x0 = r.left() + r.width() * k as f32 / n as f32;
                        let x1 = r.left() + r.width() * (k + 1) as f32 / n as f32;
                        mesh.add_colored_rect(
                            Rect::from_x_y_ranges(x0..=x1, r.y_range()),
                            lerp(quiet, HITCH_COLOR, t.powf(0.7)),
                        );
                    }
                    ui.painter().add(egui::Shape::mesh(mesh));
                    ui.label(
                        RichText::new("as usual to longer than the median frame")
                            .small()
                            .color(ink::DIM),
                    );
                }
                if ui
                    .selectable_label(compare, words)
                    .on_hover_text(
                        "C: tint each bar by how much longer its path took than in the \
                         median frame of the trace",
                    )
                    .clicked()
                {
                    toggle = true;
                }
                ui.label(
                    RichText::new(format!("drawn in {:.1} ms", self.drawn_ms))
                        .small()
                        .color(ink::FAINT),
                )
                .on_hover_text("How long this tab took to lay out its last frame");
                ui.label(RichText::new(&self.shown).small().color(ink::FAINT));
            });
        });
        if toggle {
            self.set_compare(!compare);
        }
        let t = &self.p.trace;
        let rows = &self.p.rows;
        let mut facts: Vec<String> = Vec::new();
        if let Some((a, b)) = self.p.whole() {
            facts.push(format!(
                "{} frames over {:.2} s",
                rows.len(),
                (b - a) as f64 / 1e9
            ));
        }
        let threads = t.threads.len();
        facts.push(format!(
            "{} scopes on {threads} thread{}",
            group_digits(t.scope_count()),
            if threads == 1 { "" } else { "s" }
        ));
        let hitches = rows.iter().filter(|r| r.hitch.is_some()).count();
        facts.push(match hitches {
            0 => "no hitches".to_string(),
            1 => "1 hitch".to_string(),
            n => format!("{n} hitches"),
        });
        let mut lengths: Vec<u64> = rows.iter().map(|r| r.end_ns - r.start_ns).collect();
        lengths.sort_unstable();
        if let Some(&m) = lengths.get(lengths.len() / 2) {
            facts.push(format!("median frame {} ms", ms_text(m)));
        }
        for key in ["build", "adapter"] {
            if let Some(v) = t.header_str(key) {
                facts.push(v.to_string());
            }
        }
        ui.label(RichText::new(facts.join("  ·  ")).color(ink::DIM));
        let mut origin: Vec<String> = Vec::new();
        if let Some(by) = t.header_str("written_by") {
            origin.push(format!("written by {by}"));
        }
        if let Some(at) = t.header_str("written_at") {
            origin.push(at.to_string());
        }
        if let Some(r) = t.header_str("reason") {
            origin.push(format!("saved on {r}"));
        }
        // The commit the binary was built from (#1290); a trace without one
        // says it doesn't know, and a game's own checkout follows when it
        // isn't the engine's
        let engine = t.header_str("engine_commit");
        let dirty = t.header.get("engine_dirty").and_then(|v| v.as_bool());
        origin.push(format!(
            "engine{}",
            super::super::commit_words(engine, t.header_str("engine_subdir"), dirty)
        ));
        if let Some(c) = t
            .header_str("project_commit")
            .filter(|c| Some(*c) != engine)
        {
            let short: String = c.chars().take(10).collect();
            origin.push(format!("project at {short}"));
        }
        if let Some(n) = &self.notice {
            origin.push(n.clone());
        }
        if !origin.is_empty() {
            ui.label(
                RichText::new(origin.join("  ·  "))
                    .small()
                    .color(ink::FAINT),
            );
        }
        if let Some(c) = &self.p.coverage {
            ui.label(RichText::new(c).small().color(ink::TEXT));
        }
    }

    // ── The frame strip ──

    fn strip(&mut self, ui: &mut egui::Ui) {
        if self.p.rows.is_empty() {
            ui.label(RichText::new("This trace holds no frames.").color(ink::DIM));
            return;
        }
        ui.horizontal(|ui| {
            ui.label(RichText::new("Frames").strong());
            ui.label(RichText::new(self.selection_words()).color(ink::TEXT));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.scope(|ui| {
                    ui.style_mut().override_text_style = Some(egui::TextStyle::Small);
                    band_legend(ui);
                });
            });
        });
        let t0 = self.p.rows[0].start_ns;
        let total = self
            .p
            .whole()
            .map_or(1.0, |(a, b)| (b - a) as f64 / 1e9)
            .max(1e-3);
        let width = ui.available_width();
        self.graph.show(
            ui,
            &self.graph_frames,
            GraphLayout {
                size: Vec2::new(width, STRIP_H),
                window_secs: total as f32,
                budget_ms: BUDGET_MS,
                fps_labels: true,
            },
        );
        let Some(placed) = self.graph.placed() else {
            return;
        };
        let rect = placed.rect;
        let x_of = |ns: u64| {
            placed
                .x_of((ns - t0) as f64 / 1e9)
                .clamp(rect.left(), rect.right())
        };
        let painter = ui.painter_at(rect.expand(2.0));
        // The frames before the scopes begin (#1288): veiled, the edge
        // marked and named, so an empty chart there reads as no record,
        // not as a frame where nothing ran
        if self.p.first_covered > 0 {
            let edge = x_of(self.p.rows[self.p.first_covered].start_ns);
            let veil = Rect::from_x_y_ranges(rect.left()..=edge, rect.y_range());
            painter.rect_filled(veil, 0.0, Color32::from_black_alpha(150));
            painter.line_segment(
                [Pos2::new(edge, rect.top()), Pos2::new(edge, rect.bottom())],
                Stroke::new(1.0, ink::DIM),
            );
            let label = painter.layout_no_wrap(
                "no scopes: the ring wrapped".into(),
                FontId::proportional(10.0),
                ink::DIM,
            );
            if label.size().x + 8.0 < edge - rect.left() {
                painter.galley(
                    Pos2::new(edge - label.size().x - 4.0, rect.top() + 2.0),
                    label,
                    ink::DIM,
                );
            }
        }
        // The chosen frames, and the chart's window when it is narrower
        if let Some((a, b)) = self.p.span_of(self.sel.clone()) {
            let (x0, x1) = (x_of(a), x_of(b));
            let r = Rect::from_x_y_ranges(x0..=(x1).max(x0 + 2.0), rect.y_range());
            painter.rect_filled(r, 1.0, Color32::from_rgba_unmultiplied(86, 190, 176, 34));
            painter.rect_stroke(
                r,
                1.0,
                Stroke::new(1.0, ink::SELECT_LINE),
                egui::StrokeKind::Inside,
            );
        }
        // The chosen scope's time, as a line over the bars
        if self.chosen.is_some() && !self.series.is_empty() {
            let ceiling = self.graph.ceiling_ms().unwrap_or(50.0).max(1.0);
            let pts: Vec<Pos2> = self
                .p
                .rows
                .iter()
                .zip(&self.series)
                .map(|(r, &ms)| {
                    let x = x_of(r.start_ns + (r.end_ns - r.start_ns) / 2);
                    let y = rect.bottom() - (ms / ceiling).min(1.0) * rect.height();
                    Pos2::new(x, y)
                })
                .collect();
            painter.add(egui::Shape::line(pts, Stroke::new(1.25, Color32::WHITE)));
        }
        let id = ui.id().with("profile-strip-sense");
        let resp = ui.interact(rect, id, Sense::click_and_drag());
        self.dragging = resp.dragged();
        let row_at = |tab: &Self, x: f32| -> usize {
            tab.graph.frame_at(x).unwrap_or_else(|| {
                let ns = t0 as f64 + placed.time_at(x) * 1e9;
                tab.p.row_at(ns.max(0.0) as u64).unwrap_or(0)
            })
        };
        if let Some(pos) = resp.interact_pointer_pos() {
            let r = row_at(self, pos.x);
            if resp.drag_started() || resp.clicked() {
                self.select(r..r + 1);
            } else if resp.dragged() {
                let a = self.anchor;
                let (lo, hi) = (a.min(r), a.max(r));
                self.sel = lo..hi + 1;
                self.fit();
            }
        }
        if let Some(pos) = resp.hover_pos() {
            if !resp.dragged() {
                let r = row_at(self, pos.x);
                let row = &self.p.rows[r];
                let mut words = format!(
                    "frame {}  {} ms",
                    row.index,
                    ms_text(row.end_ns - row.start_ns)
                );
                if let Some(h) = row.hitch {
                    words.push_str(&format!("  hitch: {}", self.p.trace.hitches[h].cause));
                }
                card(
                    ui.ctx(),
                    pos,
                    &[(words, ink::TEXT, FontId::proportional(12.0))],
                );
            }
        }
        ui.add_space(2.0);
        ui.label(
            RichText::new(
                "click a frame, drag a range  ·  Left / Right step (Shift widens)  ·  H next hitch  \
                 ·  W / S zoom, A / D pan, F fit, wheel zooms, drag pans  ·  C compare",
            )
            .small()
            .color(ink::FAINT),
        );
    }

    fn selection_words(&self) -> String {
        let rows = &self.p.rows;
        let Some(a) = rows.get(self.sel.start) else {
            return String::new();
        };
        if self.sel.len() == 1 {
            let mut s = format!(
                "frame {}  ·  {} ms",
                a.index,
                ms_text(a.end_ns - a.start_ns)
            );
            if let Some(h) = a.hitch {
                let h = &self.p.trace.hitches[h];
                if let Some(t) = h.game_time {
                    s.push_str(&format!("  ·  t {t:.2} s"));
                }
                s.push_str(&format!(
                    "  ·  hitch over a {:.1} ms median: {}",
                    h.median_ms.unwrap_or(0.0),
                    h.cause
                ));
            }
            if let Some((ti, i)) = self
                .compare
                .then(|| self.p.hottest(self.sel.start))
                .flatten()
            {
                let path = self.p.facts[ti].path[i];
                s.push_str(&format!("  ·  carried by {}", self.p.key(path)));
            }
            s
        } else {
            let b = &rows[self.sel.end - 1];
            let total: u64 = rows[self.sel.clone()]
                .iter()
                .map(|r| r.end_ns - r.start_ns)
                .sum();
            let hitches = rows[self.sel.clone()]
                .iter()
                .filter(|r| r.hitch.is_some())
                .count();
            format!(
                "frames {} to {}  ·  {} frames  ·  {} ms, {} ms a frame{}",
                a.index,
                b.index,
                self.sel.len(),
                ms_text(total),
                ms_text(total / self.sel.len() as u64),
                match hitches {
                    0 => String::new(),
                    1 => "  ·  1 hitch".into(),
                    n => format!("  ·  {n} hitches"),
                }
            )
        }
    }

    // ── The statistics ──

    fn side(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(RichText::new("Scopes").strong());
            let over = if self.sel.len() == 1 {
                format!(
                    "in frame {}",
                    self.p.rows.get(self.sel.start).map_or(0, |r| r.index)
                )
            } else {
                format!("over {} frames", self.sel.len())
            };
            ui.label(RichText::new(over).color(ink::DIM));
            ui.add(
                egui::TextEdit::singleline(&mut self.filter)
                    .hint_text("filter")
                    .desired_width(ui.available_width() - 4.0),
            );
        });
        let plot_h = if self.chosen.is_some() {
            PLOT_H + 84.0
        } else {
            0.0
        };
        let table_h = (ui.available_height() - plot_h - 50.0).max(80.0);
        let w = ui.available_width();
        // Columns: name, then the numbers right-aligned
        let nums: [(&str, StatSort, f32); 5] = [
            ("count", StatSort::Count, 50.0),
            ("total", StatSort::Total, 60.0),
            ("mean", StatSort::Mean, 58.0),
            ("p90", StatSort::P90, 58.0),
            ("worst", StatSort::Worst, 60.0),
        ];
        let name_w = (w - nums.iter().map(|n| n.2).sum::<f32>()).max(90.0);
        let mut xs = vec![(name_w, "scope", StatSort::Name, 0.0f32)];
        let mut x = name_w;
        for (label, sort, cw) in nums {
            xs.push((cw, label, sort, x));
            x += cw;
        }
        let (head, _) = ui.allocate_exact_size(Vec2::new(w, 20.0), Sense::hover());
        let painter = ui.painter_at(head);
        painter.line_segment(
            [head.left_bottom(), head.right_bottom()],
            Stroke::new(1.0, ink::RULE),
        );
        let mut resort = None;
        for &(cw, label, sort, x) in &xs {
            let r =
                Rect::from_min_size(Pos2::new(head.left() + x, head.top()), Vec2::new(cw, 20.0));
            let resp = ui.interact(r, ui.id().with(("stat-sort", label)), Sense::click());
            let on = self.sort == sort;
            let colour = if on || resp.hovered() {
                ink::TEXT
            } else {
                ink::DIM
            };
            let g = painter.layout_no_wrap(label.to_string(), FontId::proportional(12.0), colour);
            let right = sort != StatSort::Name;
            let gx = if right {
                r.right() - 8.0 - g.size().x
            } else {
                r.left() + 6.0
            };
            let gw = g.size().x;
            painter.galley(Pos2::new(gx, r.center().y - g.size().y / 2.0), g, colour);
            if on {
                // The font has no arrows: a triangle
                let c = Pos2::new(if right { gx - 7.0 } else { gx + gw + 7.0 }, r.center().y);
                let (tip, base) = if self.descending {
                    (3.0, -2.0)
                } else {
                    (-3.0, 2.0)
                };
                painter.add(egui::Shape::convex_polygon(
                    vec![
                        Pos2::new(c.x - 3.5, c.y + base),
                        Pos2::new(c.x + 3.5, c.y + base),
                        Pos2::new(c.x, c.y + tip),
                    ],
                    ink::SELECT_LINE,
                    Stroke::NONE,
                ));
            }
            if resp.clicked() {
                resort = Some(sort);
            }
        }
        if let Some(sort) = resort {
            if self.sort == sort {
                self.descending = !self.descending;
            } else {
                self.sort = sort;
                self.descending = sort != StatSort::Name;
            }
            let (s, d) = (self.sort, self.descending);
            self.p.sort_stats(&mut self.stats, s, d);
        }
        let filter = self.filter.to_lowercase();
        let shown: Vec<usize> = (0..self.stats.len())
            .filter(|&i| {
                filter.is_empty()
                    || self
                        .p
                        .name(self.stats[i].name)
                        .to_lowercase()
                        .contains(&filter)
            })
            .collect();
        let row_h = 20.0;
        let mut clicked = None;
        egui::ScrollArea::vertical()
            .id_salt("profile-stats-table")
            .max_height(table_h)
            .auto_shrink([false, false])
            .show_rows(ui, row_h, shown.len().max(1), |ui, range| {
                if shown.is_empty() {
                    ui.label(RichText::new("No scope matches.").color(ink::DIM));
                    return;
                }
                for &i in &shown[range] {
                    let s = &self.stats[i];
                    let (r, resp) = ui.allocate_exact_size(
                        Vec2::new(ui.available_width(), row_h),
                        Sense::click(),
                    );
                    let painter = ui.painter_at(r);
                    let on = self.chosen == Some(s.name);
                    if on {
                        painter.rect_filled(r, 2.0, ink::SELECT);
                    } else if resp.hovered() {
                        painter.rect_filled(r, 2.0, ink::RAISED);
                    }
                    let name = self.p.name(s.name);
                    let band = self.p.facts[s.worst_at.0].band[s.worst_at.1];
                    let swatch = band.map_or(NEUTRAL, Band::color);
                    painter.rect_filled(
                        Rect::from_min_size(
                            Pos2::new(r.left() + 1.0, r.center().y - 4.0),
                            Vec2::splat(8.0),
                        ),
                        2.0,
                        swatch,
                    );
                    let g = elided(
                        &painter,
                        name.to_string(),
                        FontId::proportional(12.5),
                        ink::TEXT,
                        name_w - 18.0,
                    );
                    painter.galley(
                        Pos2::new(r.left() + 14.0, r.center().y - g.size().y / 2.0),
                        g,
                        ink::TEXT,
                    );
                    let values = [
                        s.count.to_string(),
                        ms_text(s.total_ns),
                        ms_text(s.mean_ns),
                        ms_text(s.p90_ns),
                        ms_text(s.worst_ns),
                    ];
                    for (v, &(cw, _, _, x)) in values.iter().zip(&xs[1..]) {
                        let g =
                            painter.layout_no_wrap(v.clone(), FontId::monospace(11.5), ink::TEXT);
                        painter.galley(
                            Pos2::new(
                                r.left() + x + cw - 8.0 - g.size().x,
                                r.center().y - g.size().y / 2.0,
                            ),
                            g,
                            ink::TEXT,
                        );
                    }
                    if resp.clicked() {
                        clicked = Some(s.name);
                    }
                    resp.on_hover_text(name);
                }
            });
        ui.label(
            RichText::new(
                "times in ms  ·  every call, nested calls included  ·  p90 by nearest rank",
            )
            .small()
            .color(ink::FAINT),
        );
        if let Some(name) = clicked {
            self.choose(if self.chosen == Some(name) {
                None
            } else {
                Some(name)
            });
        }
        if self.chosen.is_some() {
            ui.add_space(4.0);
            ui.separator();
            self.plot(ui);
        }
    }

    /// The chosen scope's time in every frame of the trace.
    fn plot(&mut self, ui: &mut egui::Ui) {
        let Some(name) = self.chosen else {
            return;
        };
        let series = &self.series;
        let n = series.len();
        if n == 0 {
            return;
        }
        let mut sorted = series.clone();
        sorted.sort_by(f32::total_cmp);
        let median = sorted[n / 2];
        let max = sorted[n - 1].max(0.001);
        let chosen_total: f32 = series[self.sel.clone()].iter().sum();
        ui.horizontal(|ui| {
            ui.label(RichText::new(self.p.name(name)).strong().color(ink::ENTITY));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.small_button("x").on_hover_text("Esc").clicked() {
                    self.chosen = None;
                }
            });
        });
        let series = &self.series;
        ui.label(
            RichText::new(format!(
                "per frame across the trace: median {median:.3} ms, worst {max:.3} ms; \
                 {chosen_total:.3} ms in the frames chosen"
            ))
            .small()
            .color(ink::DIM),
        );
        let w = ui.available_width();
        let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, PLOT_H), Sense::click_and_drag());
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 2.0, Color32::from_rgba_unmultiplied(255, 255, 255, 6));
        let col = rect.width() / n as f32;
        let x_of = |i: usize| rect.left() + i as f32 * col;
        let y_of = |ms: f32| rect.bottom() - (ms / (max * 1.08)).min(1.0) * (rect.height() - 4.0);
        let sel = Rect::from_x_y_ranges(
            x_of(self.sel.start)..=x_of(self.sel.end).max(x_of(self.sel.start) + 2.0),
            rect.y_range(),
        );
        painter.rect_filled(sel, 0.0, Color32::from_rgba_unmultiplied(86, 190, 176, 40));
        let band = self
            .stats
            .iter()
            .find(|s| s.name == name)
            .and_then(|s| self.p.facts[s.worst_at.0].band[s.worst_at.1]);
        let colour = band.map_or(NEUTRAL, Band::color);
        // One bar a frame, or the tallest of the frames a pixel holds
        let ppp = ui.ctx().pixels_per_point();
        let px = (rect.width() * ppp).max(1.0) as usize;
        let mut mesh = egui::Mesh::default();
        if n <= px {
            for (i, &ms) in series.iter().enumerate() {
                let r = Rect::from_x_y_ranges(
                    x_of(i)..=x_of(i) + col.max(1.0 / ppp),
                    y_of(ms)..=rect.bottom(),
                );
                mesh.add_colored_rect(r, colour);
            }
        } else {
            for p in 0..px {
                let a = p * n / px;
                let b = ((p + 1) * n / px).max(a + 1).min(n);
                let ms = series[a..b].iter().copied().fold(0.0, f32::max);
                let x = rect.left() + p as f32 / ppp;
                mesh.add_colored_rect(
                    Rect::from_x_y_ranges(x..=x + 1.0 / ppp, y_of(ms)..=rect.bottom()),
                    colour,
                );
            }
        }
        painter.add(egui::Shape::mesh(mesh));
        for r in self.p.hitch_rows() {
            let x = x_of(r) + col / 2.0;
            painter.line_segment(
                [
                    Pos2::new(x, rect.bottom() - 4.0),
                    Pos2::new(x, rect.bottom()),
                ],
                Stroke::new(2.0, HITCH_COLOR),
            );
        }
        let my = y_of(median);
        painter.hline(
            rect.x_range(),
            my,
            Stroke::new(1.0, Color32::from_white_alpha(90)),
        );
        painter.text(
            Pos2::new(rect.left() + 4.0, my - 2.0),
            egui::Align2::LEFT_BOTTOM,
            format!("median {median:.3}"),
            FontId::monospace(9.0),
            Color32::from_white_alpha(150),
        );
        if let Some(pos) = resp.interact_pointer_pos() {
            let i = (((pos.x - rect.left()) / col).floor().max(0.0) as usize).min(n - 1);
            if resp.clicked() || resp.drag_started() {
                self.select(i..i + 1);
            } else if resp.dragged() {
                let a = self.anchor;
                self.sel = a.min(i)..a.max(i) + 1;
                self.fit();
            }
        }
        if let Some(pos) = resp.hover_pos() {
            let i = (((pos.x - rect.left()) / col).floor().max(0.0) as usize).min(n - 1);
            let words = format!("frame {}  {:.3} ms", self.p.rows[i].index, self.series[i]);
            card(
                ui.ctx(),
                pos,
                &[(words, ink::TEXT, FontId::proportional(12.0))],
            );
        }
    }

    // ── The flame chart ──

    fn chart(&mut self, ui: &mut egui::Ui) {
        let rect = ui.max_rect();
        let resp = ui.interact(rect, ui.id().with("profile-chart"), Sense::click_and_drag());
        let painter = ui.painter_at(rect);
        if self.p.rows.is_empty() {
            return;
        }
        // Wheel zooms about the pointer; drag pans both ways
        if let Some(pos) = resp.hover_pos() {
            let scroll = ui.input(|i| i.smooth_scroll_delta);
            let (v0, v1) = self.view;
            let at = v0 + (pos.x - rect.left()) as f64 / rect.width() as f64 * (v1 - v0);
            if scroll.y != 0.0 {
                self.zoom_about(at, (-scroll.y as f64 * 0.0025).exp());
            }
            if scroll.x != 0.0 {
                let w = v1 - v0;
                let dt = -scroll.x as f64 / rect.width() as f64 * w;
                self.view = (self.view.0 + dt, self.view.1 + dt);
            }
        }
        if resp.dragged() {
            let d = resp.drag_delta();
            let (v0, v1) = self.view;
            let dt = -d.x as f64 / rect.width() as f64 * (v1 - v0);
            self.view = (v0 + dt, v1 + dt);
            self.scroll_y = (self.scroll_y - d.y).max(0.0);
        }
        let (v0, v1) = self.view;
        let width_ns = (v1 - v0).max(1.0);
        let px_per_ns = rect.width() as f64 / width_ns;
        let x_of = |ns: u64| rect.left() + ((ns as f64 - v0) * px_per_ns) as f32;
        let rows = self.p.rows_touching(v0.max(0.0) as u64, v1.max(0.0) as u64);
        let ptr = resp.hover_pos().filter(|_| !resp.dragged());
        let mut hover: Option<Hover> = None;
        let baseline = if self.compare {
            self.p.baseline_if_made()
        } else {
            None
        };
        let hot: Vec<(usize, usize)> = match baseline {
            Some(_) => rows.clone().filter_map(|r| self.p.hottest(r)).collect(),
            None => Vec::new(),
        };

        // The frames under the chart: the chosen ones lit, a line where each begins
        for r in rows.clone() {
            let row = &self.p.rows[r];
            let (x0, x1) = (x_of(row.start_ns), x_of(row.end_ns));
            if self.sel.contains(&r) {
                painter.rect_filled(
                    Rect::from_x_y_ranges(x0..=x1, rect.top() + RULER_H..=rect.bottom()),
                    0.0,
                    Color32::from_rgba_unmultiplied(86, 190, 176, 10),
                );
            }
            if x1 - x0 > 6.0 {
                painter.vline(
                    x0,
                    rect.top()..=rect.bottom(),
                    Stroke::new(1.0, Color32::from_white_alpha(14)),
                );
            }
        }

        let top = rect.top() + RULER_H + 4.0 - self.scroll_y;
        let mut y = top;
        let font = FontId::proportional(11.5);
        let small = FontId::proportional(10.5);
        let chosen = self.chosen;

        // A bar: filled, a hairline between neighbours, its name if it fits
        let bar = |painter: &egui::Painter,
                   x0: f32,
                   x1: f32,
                   y: f32,
                   fill: Color32,
                   label: &str,
                   ms: Option<u64>,
                   text: Color32| {
            let r = Rect::from_min_max(
                Pos2::new(x0, y + 1.0),
                Pos2::new(x1.max(x0 + 1.0), y + ROW_H - 1.0),
            );
            painter.rect_filled(r, 2.0, fill);
            let room = r.width() - 8.0;
            if room > 18.0 {
                let words = match ms {
                    Some(ns) => format!("{label}  {}", ms_text(ns)),
                    None => label.to_string(),
                };
                let clipped = painter.with_clip_rect(r.intersect(painter.clip_rect()));
                let g = elided(&clipped, words, font.clone(), text, room);
                let tx = r.left().max(rect.left()) + 4.0;
                clipped.galley(Pos2::new(tx, r.center().y - g.size().y / 2.0), g, text);
            }
            r
        };
        let lane_head = |painter: &egui::Painter, y: f32, words: &str| {
            let r = Rect::from_min_max(
                Pos2::new(rect.left(), y),
                Pos2::new(rect.right(), y + LANE_HEAD),
            );
            painter.rect_filled(r, 0.0, Color32::from_rgba_unmultiplied(255, 255, 255, 5));
            painter.text(
                Pos2::new(rect.left() + 8.0, r.center().y),
                egui::Align2::LEFT_CENTER,
                words,
                small.clone(),
                ink::DIM,
            );
        };
        let visible = |a: f32, b: f32| b >= rect.left() && a <= rect.right();

        // The phases
        lane_head(&painter, y, "phases");
        y += LANE_HEAD;
        for r in rows.clone() {
            let range = self.p.rows[r].phases.clone();
            for (k, ph) in self.p.trace.phases[range.clone()].iter().enumerate() {
                let (x0, x1) = (x_of(ph.start_ns), x_of(ph.end_ns()));
                if !visible(x0, x1) {
                    continue;
                }
                let label = self.p.name(ph.name);
                let c = band_of(label).color();
                let fill = if self.compare {
                    lerp(c, ink::GROUND, 0.45)
                } else {
                    c
                };
                let br = bar(&painter, x0, x1, y, fill, label, None, ON_BAR);
                if ptr.is_some_and(|p| br.contains(p)) {
                    hover = Some(Hover::Phase(range.start + k));
                }
            }
        }
        y += ROW_H + 6.0;

        // Each thread's scopes, the frame thread first
        let mut order: Vec<usize> = (0..self.p.trace.threads.len()).collect();
        if let Some(f) = self.p.frame_thread {
            order.retain(|&t| t != f);
            order.insert(0, f);
        }
        let mut script_bars: Vec<(usize, usize)> = Vec::new();
        for &ti in &order {
            let thread = &self.p.trace.threads[ti];
            let facts = &self.p.facts[ti];
            let Some(first) = facts.rows.get(rows.start) else {
                continue;
            };
            let last = &facts.rows[rows.end.saturating_sub(1).min(facts.rows.len() - 1)];
            let span = first.start..last.end.max(first.start);
            if span.is_empty() {
                continue;
            }
            let depth = thread.scopes[span.clone()]
                .iter()
                .filter(|s| visible(x_of(s.start_ns), x_of(s.end_ns())))
                .map(|s| s.depth)
                .max();
            let Some(depth) = depth else {
                continue;
            };
            lane_head(
                &painter,
                y,
                &format!("{}  ·  {} scopes", thread.name, span.len()),
            );
            y += LANE_HEAD;
            // Bars under a pixel are dust: one pixel a depth for all of them
            let mut dust: Vec<f32> = vec![f32::NEG_INFINITY; depth as usize + 1];
            let frame_thread = Some(ti) == self.p.frame_thread;
            for i in span {
                let s = &thread.scopes[i];
                let (x0, x1) = (x_of(s.start_ns), x_of(s.end_ns()));
                if !visible(x0, x1) {
                    continue;
                }
                if frame_thread && self.script_name[s.name as usize] {
                    script_bars.push((ti, i));
                }
                let by = y + s.depth as f32 * ROW_H;
                if by > rect.bottom() || by + ROW_H < rect.top() + RULER_H {
                    continue;
                }
                let base = facts.band[i].map_or(NEUTRAL, Band::color);
                let mut fill = base;
                let mut text = if facts.band[i].is_some() {
                    ON_BAR
                } else {
                    ink::TEXT
                };
                if baseline.is_some() {
                    (fill, text) = self.tinted(ti, i, base);
                }
                if let Some(c) = chosen {
                    if c != s.name {
                        let dim = if baseline.is_some() { 0.45 } else { 0.7 };
                        fill = lerp(fill, ink::GROUND, dim);
                        text = ink::FAINT;
                    }
                }
                // Under Compare, the bar that carries its frame's excess:
                // `flint profile explain`'s answer, ringed
                let carries = baseline.is_some() && hot.contains(&(ti, i));
                if x1 - x0 < 1.0 && !carries {
                    let d = s.depth as usize;
                    if x0.floor() > dust[d] {
                        dust[d] = x0.floor();
                        painter.rect_filled(
                            Rect::from_min_size(
                                Pos2::new(x0.floor(), by + 1.0),
                                Vec2::new(1.0, ROW_H - 2.0),
                            ),
                            0.0,
                            fill.gamma_multiply(0.75),
                        );
                    }
                    continue;
                }
                // A summed record (#1288) is lighter and counts its calls:
                // it is laid from the first call for their total, so it
                // may overlap a sibling
                let summed_label;
                let label = match s.summed {
                    Some(sum) => {
                        fill = fill.gamma_multiply(0.6);
                        summed_label = format!("{} x{}", self.p.name(s.name), sum.calls);
                        summed_label.as_str()
                    }
                    None => self.p.name(s.name),
                };
                let br = bar(&painter, x0, x1, by, fill, label, Some(s.dur_ns), text);
                if chosen == Some(s.name) {
                    painter.rect_stroke(
                        br,
                        2.0,
                        Stroke::new(1.5, Color32::WHITE),
                        egui::StrokeKind::Inside,
                    );
                }
                if carries {
                    painter.rect_stroke(
                        br.expand(1.5),
                        3.0,
                        Stroke::new(2.0, CARRIER_RING),
                        egui::StrokeKind::Outside,
                    );
                }
                if ptr.is_some_and(|p| br.contains(p)) {
                    hover = Some(Hover::Scope(ti, i));
                }
            }
            y += (depth as f32 + 1.0) * ROW_H + 6.0;
        }

        // The script callbacks, on one row of their own
        if !script_bars.is_empty() {
            lane_head(&painter, y, "scripts");
            y += LANE_HEAD;
            for &(ti, i) in &script_bars {
                let s = self.p.scope(ti, i);
                let (x0, x1) = (x_of(s.start_ns), x_of(s.end_ns()));
                let label = self.p.name(s.name);
                let (mut fill, mut text) = if baseline.is_some() {
                    self.tinted(ti, i, Band::Scripts.color())
                } else {
                    (Band::Scripts.color(), ON_BAR)
                };
                if chosen.is_some_and(|c| c != s.name) {
                    fill = lerp(
                        fill,
                        ink::GROUND,
                        if baseline.is_some() { 0.45 } else { 0.7 },
                    );
                    text = ink::FAINT;
                }
                let br = bar(&painter, x0, x1, y, fill, label, Some(s.dur_ns), text);
                if ptr.is_some_and(|p| br.contains(p)) {
                    hover = Some(Hover::Scope(ti, i));
                }
            }
            y += ROW_H + 6.0;
        }

        // The GPU's passes, laid from each frame's render phase
        let gpu_any = rows.clone().any(|r| !self.p.rows[r].gpu.is_empty());
        if gpu_any {
            lane_head(&painter, y, "gpu  ·  laid from the frame's render phase");
            y += LANE_HEAD;
            for r in rows.clone() {
                let range = self.p.rows[r].gpu.clone();
                for (k, g) in self.p.trace.gpu[range.clone()].iter().enumerate() {
                    let (x0, x1) = (x_of(g.start_ns), x_of(g.end_ns()));
                    if !visible(x0, x1) {
                        continue;
                    }
                    let fill = if k % 2 == 0 { ink::GPU } else { ink::GPU_ALT };
                    let br = bar(
                        &painter,
                        x0,
                        x1,
                        y,
                        fill,
                        self.p.name(g.name),
                        Some(g.dur_ns),
                        ON_BAR,
                    );
                    if ptr.is_some_and(|p| br.contains(p)) {
                        hover = Some(Hover::Gpu(range.start + k));
                    }
                }
            }
            y += ROW_H + 6.0;
        }
        // Don't scroll past the last lane
        let content = y + self.scroll_y - top;
        let room = rect.height() - RULER_H - 8.0;
        self.scroll_y = self.scroll_y.min((content - room).max(0.0));

        // The ruler, over everything that scrolls
        let ruler = Rect::from_min_max(
            rect.left_top(),
            Pos2::new(rect.right(), rect.top() + RULER_H),
        );
        painter.rect_filled(ruler, 0.0, ink::PANEL);
        painter.line_segment(
            [ruler.left_bottom(), ruler.right_bottom()],
            Stroke::new(1.0, ink::RULE),
        );
        let origin = self.p.rows[self.sel.start.min(self.p.rows.len() - 1)].start_ns as f64;
        let step = tick_step(width_ns, rect.width());
        let first = ((v0 - origin) / step).ceil() as i64;
        let last = ((v1 - origin) / step).floor() as i64;
        for k in first..=last {
            let t = origin + k as f64 * step;
            let x = rect.left() + ((t - v0) * px_per_ns) as f32;
            painter.vline(
                x,
                ruler.bottom() - 5.0..=ruler.bottom(),
                Stroke::new(1.0, ink::DIM),
            );
            painter.text(
                Pos2::new(x + 3.0, ruler.bottom() - 4.0),
                egui::Align2::LEFT_BOTTOM,
                tick_words(k as f64 * step),
                FontId::monospace(9.5),
                ink::DIM,
            );
        }
        // Each frame's number where it begins, when there is room
        for r in rows.clone() {
            let row = &self.p.rows[r];
            let (x0, x1) = (x_of(row.start_ns), x_of(row.end_ns));
            if x1 - x0 > 70.0 && x0 >= rect.left() - 1.0 {
                let c = if self.sel.contains(&r) {
                    ink::SELECT_LINE
                } else {
                    ink::FAINT
                };
                painter.text(
                    Pos2::new(x0 + 4.0, ruler.top() + 2.0),
                    egui::Align2::LEFT_TOP,
                    format!("frame {}", row.index),
                    FontId::proportional(10.5),
                    c,
                );
            }
        }
        // Hitches along the ruler's foot, notes as diamonds on it
        for (h, hitch) in self.p.trace.hitches.iter().enumerate() {
            let (x0, x1) = (x_of(hitch.start_ns), x_of(hitch.start_ns + hitch.dur_ns));
            if !visible(x0, x1) {
                continue;
            }
            let r = Rect::from_x_y_ranges(
                x0.max(rect.left())..=x1.min(rect.right()),
                ruler.bottom() - 3.0..=ruler.bottom(),
            );
            painter.rect_filled(r, 0.0, HITCH_COLOR);
            if ptr.is_some_and(|p| r.expand2(Vec2::new(0.0, 4.0)).contains(p)) {
                hover = Some(Hover::Hitch(h));
            }
        }
        for (k, n) in self.p.trace.notes.iter().enumerate() {
            let x = x_of(n.at_ns);
            if x < rect.left() || x > rect.right() {
                continue;
            }
            let c = Pos2::new(x, ruler.top() + 6.0 + n.depth as f32 * 2.0);
            painter.add(egui::Shape::convex_polygon(
                vec![
                    Pos2::new(c.x, c.y - 4.0),
                    Pos2::new(c.x + 4.0, c.y),
                    Pos2::new(c.x, c.y + 4.0),
                    Pos2::new(c.x - 4.0, c.y),
                ],
                ink::ENTITY,
                Stroke::NONE,
            ));
            if ptr.is_some_and(|p| p.distance(c) < 6.0) {
                hover = Some(Hover::Note(k));
            }
        }

        // The pointer: a card; a click chooses, a double-click zooms
        if let (Some(h), Some(pos)) = (hover, ptr) {
            let lines = self.hover_lines(h);
            card(ui.ctx(), pos, &lines);
        }
        if let Some(Hover::Scope(ti, i)) = hover {
            let s = self.p.scope(ti, i).clone();
            if resp.double_clicked() {
                let pad = s.dur_ns as f64 * 0.05;
                self.view = (s.start_ns as f64 - pad, s.end_ns() as f64 + pad);
            } else if resp.clicked() {
                self.choose(if self.chosen == Some(s.name) {
                    None
                } else {
                    Some(s.name)
                });
            }
        }
    }

    /// A bar's colour under Compare: its system's, greyed when its path
    /// took what it takes in the median frame, toward vermillion as it took
    /// longer, scaled to how far over the median the whole frame ran.
    fn tinted(&self, ti: usize, i: usize, base: Color32) -> (Color32, Color32) {
        let Some(b) = self.p.baseline_if_made() else {
            return (base, ON_BAR);
        };
        let heat = b.heat[ti][i];
        let quiet = lerp(base, Color32::from_gray(70), 0.62);
        let text = if heat > 0.35 { ON_BAR } else { ink::TEXT };
        (lerp(quiet, HITCH_COLOR, heat), text)
    }

    /// What the card under the pointer says.
    fn hover_lines(&self, h: Hover) -> Vec<(String, Color32, FontId)> {
        let strong = FontId::proportional(13.0);
        let body = FontId::proportional(12.0);
        let mono = FontId::monospace(11.0);
        let mut out = Vec::new();
        let share = |ns: u64, frame: Option<u32>| -> String {
            frame
                .and_then(|f| self.p.row_of(f))
                .map(|r| {
                    let row = &self.p.rows[r];
                    let f = (row.end_ns - row.start_ns).max(1);
                    format!(
                        "{} ms  ·  {:.1}% of frame {}",
                        ms_text(ns),
                        ns as f64 / f as f64 * 100.0,
                        row.index
                    )
                })
                .unwrap_or_else(|| format!("{} ms", ms_text(ns)))
        };
        match h {
            Hover::Scope(ti, i) => {
                let s = self.p.scope(ti, i);
                let facts = &self.p.facts[ti];
                out.push((self.p.name(s.name).to_string(), ink::TEXT, strong));
                if let Some(d) = &s.detail {
                    out.push((d.to_string(), ink::ENTITY, body.clone()));
                }
                if let Some(sum) = s.summed {
                    out.push((
                        format!(
                            "{} calls summed, laid from the first  ·  longest {} ms",
                            sum.calls,
                            ms_text(sum.worst_ns)
                        ),
                        ink::ENTITY,
                        body.clone(),
                    ));
                }
                out.push((share(s.dur_ns, Some(s.frame)), ink::TEXT, body.clone()));
                out.push((
                    format!(
                        "self {} ms  ·  {} thread",
                        ms_text(self.p.trace.threads[ti].self_ns[i]),
                        self.p.trace.threads[ti].name
                    ),
                    ink::DIM,
                    body.clone(),
                ));
                let system = facts.band[i].map_or("across systems", Band::label);
                out.push((format!("system: {system}"), ink::DIM, body.clone()));
                if let Some(b) = self.p.baseline_if_made().filter(|_| self.compare) {
                    let path = facts.path[i];
                    let med = b.median_ns[path as usize];
                    let excess = self.p.excess_ns(ti, i).unwrap_or(0);
                    let words = if excess > 0 {
                        format!(
                            "median frame {} ms: +{} ms over it",
                            ms_text(med),
                            ms_text(excess as u64)
                        )
                    } else {
                        format!("median frame {} ms: as usual", ms_text(med))
                    };
                    if let Some(r) = self.p.row_of(s.frame) {
                        let over = self.p.rows[r].ms() - b.median_frame_ms;
                        if over > 0.5 {
                            out.push((
                                format!(
                                    "its frame ran {over:.1} ms over the median frame's {:.2} ms",
                                    b.median_frame_ms
                                ),
                                ink::DIM,
                                body.clone(),
                            ));
                        }
                    }
                    let c = if excess as f64 > 0.5e6 {
                        HITCH_COLOR
                    } else {
                        ink::DIM
                    };
                    out.push((words, c, body.clone()));
                    if self.p.row_of(s.frame).and_then(|r| self.p.hottest(r)) == Some((ti, i)) {
                        out.push((
                            "carries the frame's excess: what `flint profile explain` answers"
                                .into(),
                            CARRIER_RING,
                            body.clone(),
                        ));
                    }
                }
                out.push((self.p.path_text(facts.path[i]), ink::FAINT, mono));
            }
            Hover::Phase(k) => {
                let ph = &self.p.trace.phases[k];
                out.push((
                    format!("phase: {}", self.p.name(ph.name)),
                    ink::TEXT,
                    strong,
                ));
                out.push((share(ph.dur_ns, ph.frame), ink::TEXT, body));
            }
            Hover::Gpu(k) => {
                let g = &self.p.trace.gpu[k];
                out.push((format!("GPU: {}", self.p.name(g.name)), ink::GPU, strong));
                out.push((share(g.dur_ns, g.frame), ink::TEXT, body.clone()));
                out.push((
                    "placed from the frame's render phase; the GPU's own clock".into(),
                    ink::DIM,
                    body,
                ));
            }
            Hover::Note(k) => {
                let n = &self.p.trace.notes[k];
                out.push((n.kind.clone(), ink::ENTITY, strong));
                if !n.detail.is_empty() {
                    out.push((n.detail.clone(), ink::TEXT, body.clone()));
                }
                if let Some(ms) = n.ms {
                    out.push((format!("{ms:.2} ms"), ink::DIM, body));
                }
            }
            Hover::Hitch(k) => {
                let hi = &self.p.trace.hitches[k];
                out.push((
                    format!(
                        "hitch: {:.1} ms over a {:.1} ms median",
                        hi.judged(),
                        hi.median_ms.unwrap_or(0.0)
                    ),
                    HITCH_COLOR,
                    strong,
                ));
                if !hi.cause.is_empty() {
                    out.push((hi.cause.clone(), ink::TEXT, body));
                }
            }
        }
        out
    }
}

/// A card of lines by the pointer, on the tooltip layer, kept on screen.
fn card(ctx: &egui::Context, at: Pos2, lines: &[(String, Color32, FontId)]) {
    if lines.is_empty() {
        return;
    }
    let painter = ctx.layer_painter(egui::LayerId::new(
        egui::Order::Tooltip,
        egui::Id::new("profile-card"),
    ));
    let galleys: Vec<_> = lines
        .iter()
        .map(|(t, c, f)| painter.layout(t.clone(), f.clone(), *c, 460.0))
        .collect();
    let w = galleys.iter().map(|g| g.size().x).fold(0.0, f32::max) + 20.0;
    let h = galleys.iter().map(|g| g.size().y + 2.0).sum::<f32>() + 14.0;
    let screen = ctx.content_rect();
    let mut pos = at + Vec2::new(16.0, 18.0);
    if pos.x + w > screen.right() - 4.0 {
        pos.x = at.x - w - 12.0;
    }
    if pos.y + h > screen.bottom() - 4.0 {
        pos.y = at.y - h - 12.0;
    }
    let r = Rect::from_min_size(pos, Vec2::new(w, h));
    painter.rect_filled(
        r.translate(Vec2::new(0.0, 2.0)),
        5.0,
        Color32::from_black_alpha(90),
    );
    painter.rect_filled(r, 5.0, Color32::from_rgb(30, 31, 36));
    painter.rect_stroke(
        r,
        5.0,
        Stroke::new(1.0, ink::RULE),
        egui::StrokeKind::Inside,
    );
    let mut y = r.top() + 7.0;
    for (g, (_, c, _)) in galleys.into_iter().zip(lines) {
        let gh = g.size().y;
        painter.galley(Pos2::new(r.left() + 10.0, y), g, *c);
        y += gh + 2.0;
    }
}

/// A ruler's step, ns: 1, 2 or 5 of a power of ten, about 90 points apart.
fn tick_step(width_ns: f64, width_pt: f32) -> f64 {
    let want = width_ns / (width_pt as f64 / 90.0).max(1.0);
    let mut step = 1.0;
    while step * 10.0 <= want {
        step *= 10.0;
    }
    for m in [1.0, 2.0, 5.0, 10.0] {
        if step * m >= want {
            return step * m;
        }
    }
    step * 10.0
}

/// A ruler's label: time from the chosen frame's start.
fn tick_words(ns: f64) -> String {
    let ms = ns / 1e6;
    let sign = if ms > 0.0 { "+" } else { "" };
    let a = ms.abs();
    if a == 0.0 {
        "0".into()
    } else if a >= 10.0 {
        format!("{sign}{ms:.0} ms")
    } else if a >= 1.0 {
        format!("{sign}{ms:.1} ms")
    } else if a >= 0.01 {
        format!("{sign}{:.0} µs", ms * 1000.0)
    } else {
        format!("{sign}{:.1} µs", ms * 1000.0)
    }
}

fn group_digits(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(' ');
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::trace::tests::sample;

    fn tab(start: ProfileStart) -> Profiler {
        Profiler::new(Trace::read(&sample()).unwrap(), "t.json".into(), &start)
    }

    fn frame(ctx: &egui::Context, tab: &mut Profiler, events: Vec<egui::Event>) {
        // egui 0.36 reads Shift from a ModifiersChanged event, not the key's
        let shift = events
            .iter()
            .any(|e| matches!(e, egui::Event::Key { modifiers, .. } if modifiers.shift));
        let mut all = vec![egui::Event::ModifiersChanged(egui::Modifiers {
            shift,
            ..Default::default()
        })];
        all.extend(events);
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1600.0, 1000.0))),
            events: all,
            ..Default::default()
        };
        flint_debug_ui::run_headless_frame(ctx, input, |ui| tab.ui(ui));
    }

    fn key(k: Key, shift: bool) -> egui::Event {
        egui::Event::Key {
            key: k,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers {
                shift,
                ..Default::default()
            },
        }
    }

    #[test]
    fn frames_are_chosen_by_number_range_hitch_and_worst() {
        assert_eq!(FrameChoice::parse("12"), Ok(FrameChoice::Index(12)));
        assert_eq!(FrameChoice::parse("9..4"), Ok(FrameChoice::Range(4, 9)));
        assert_eq!(FrameChoice::parse("hitch"), Ok(FrameChoice::Hitch));
        assert!(FrameChoice::parse("soon").is_err());
        // With nothing asked, the hitch
        assert_eq!(tab(ProfileStart::default()).selection(), 2..3);
        let t = tab(ProfileStart {
            frame: Some(FrameChoice::Range(0, 1)),
            ..Default::default()
        });
        assert_eq!(t.selection(), 0..2);
        let t = tab(ProfileStart {
            frame: Some(FrameChoice::Index(0)),
            ..Default::default()
        });
        assert_eq!(t.selection(), 0..1);
        // A frame the trace doesn't hold says so and opens on the worst
        let t = tab(ProfileStart {
            frame: Some(FrameChoice::Index(99)),
            ..Default::default()
        });
        assert_eq!(t.selection(), 2..3);
        assert!(t.notice.as_deref().unwrap().contains("0 to 2"));
    }

    /// The sample with its main ring cut at frame 1's start (#1288): frame
    /// 0 has scopes in the file, but the header says they may be partial.
    fn wrapped_tab(start: ProfileStart) -> Profiler {
        let text = sample().replace(
            "\"reason\":\"exit\"",
            "\"reason\":\"exit\",\"seconds\":5,\"wrapped\":\"main\",\
             \"covered_from_ms\":11.0,\"cut_threads\":\"main\"",
        );
        Profiler::new(Trace::read(&text).unwrap(), "t.json".into(), &start)
    }

    #[test]
    fn a_wrapped_trace_opens_on_a_covered_frame_and_veils_the_rest() {
        let t = wrapped_tab(ProfileStart::default());
        assert_eq!(t.profile().first_covered, 1);
        assert_eq!(t.selection(), 2..3, "the hitch, which has its scopes");
        let line = t.profile().coverage.clone().unwrap();
        assert!(
            line.starts_with("scopes cover frames 1..2")
                && line.contains("the main ring wrapped, so the 1 frame before them has no scopes"),
            "{line}"
        );
        // Asked for a frame before them: shown, and said
        let t = wrapped_tab(ProfileStart {
            frame: Some(FrameChoice::Index(0)),
            ..Default::default()
        });
        assert_eq!(t.selection(), 0..1);
        assert!(
            t.notice
                .as_deref()
                .unwrap()
                .contains("frame 0 is before the scopes begin (frame 1)"),
            "{:?}",
            t.notice
        );
        // And it draws, veil and all
        let ctx = egui::Context::default();
        let mut t = wrapped_tab(ProfileStart::default());
        frame(&ctx, &mut t, vec![]);
        frame(&ctx, &mut t, vec![]);
    }

    /// The frame the tab opens on, as the trace numbers it.
    fn opens_on(text: &str, start: ProfileStart) -> u32 {
        let t = Profiler::new(Trace::read(text).unwrap(), "t.json".into(), &start);
        let sel = t.selection();
        assert_eq!(sel.len(), 1);
        t.p.rows[sel.start].index
    }

    /// #1287's done-when: the fixture's three staged causes, each frame
    /// opened in the tab under Compare and explained by `flint profile
    /// explain`. The bar the tab rings, at full heat, is the scope explain
    /// names; where explain finds no scope (frame 105's wait in acquire),
    /// the tab rings nothing.
    #[test]
    fn explains_answer_is_the_bar_compare_rings_hottest() {
        use crate::commands::profile::explain::explain;
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/traces/stall.trace.json");
        let read = || Trace::open(&path).unwrap();
        let explained = Profile::new(read());
        for (frame, says) in [
            (105, None),
            (109, Some("import_model (models/whale.glb)")),
            (113, Some("stall.rhai:on_update")),
        ] {
            let (_, doc) = explain(&explained, &frame.to_string(), 12).unwrap();
            let answer = doc["culprit"]["scope"].as_str();
            assert_eq!(answer, says, "explain on frame {frame}: {}", doc["answer"]);

            let t = Profiler::new(
                read(),
                "stall.trace.json".into(),
                &ProfileStart {
                    frame: Some(FrameChoice::Index(frame)),
                    compare: true,
                    ..Default::default()
                },
            );
            let row = t.selection().start;
            assert_eq!(t.p.rows[row].index, frame);
            let ringed = t.p.hottest(row);
            let key = ringed.map(|(ti, i)| t.p.key(t.p.facts[ti].path[i]));
            assert_eq!(key, says, "the tab's ring on frame {frame}");
            if let Some((ti, i)) = ringed {
                let b = t.p.baseline_if_made().unwrap();
                let hottest = t.p.facts[ti].rows[row]
                    .clone()
                    .map(|j| b.heat[ti][j])
                    .fold(0.0f32, f32::max);
                assert_eq!(b.heat[ti][i], hottest, "tinted hottest of its frame");
                assert_eq!(t.tinted(ti, i, NEUTRAL).0, HITCH_COLOR);
                assert!(t
                    .selection_words()
                    .contains(&format!("carried by {}", says.unwrap())));
            }
        }
        // And `hitch` is one frame both ways (#1289), the one explained
        let (_, doc) = explain(&explained, "hitch", 12).unwrap();
        let t = Profiler::new(read(), "s".into(), &ProfileStart::default());
        assert_eq!(doc["frame"]["frame"], t.p.rows[t.selection().start].index);
    }

    #[test]
    fn the_tab_and_flint_profile_mean_one_hitch() {
        use crate::commands::profile::pick_frame;
        use crate::commands::trace::tests::{three_hitches, SAVED_ON_SECOND};
        // #1289: saved for its second hitch, the tab opened on the worst
        // (105) and `flint profile` on the last (117); both mean 114 now.
        // On exit both mean the worst.
        for (reason, meant) in [(SAVED_ON_SECOND, 114), ("exit", 105)] {
            let text = three_hitches(reason);
            let profile = Trace::read(&text).unwrap();
            let explained = profile.frames[pick_frame(&profile, "hitch").unwrap()].frame;
            for frame in [None, Some(FrameChoice::Hitch)] {
                let start = ProfileStart {
                    frame: frame.clone(),
                    ..Default::default()
                };
                assert_eq!(opens_on(&text, start), meant, "{reason} {frame:?}");
            }
            assert_eq!(explained, meant, "{reason}");
        }
    }

    #[test]
    fn a_capture_opens_on_its_own_frame_or_the_hitch_it_was_taken_on() {
        use crate::commands::trace::tests::three_hitches;
        let text = three_hitches("capture");
        // Not taken on a hitch: the frame it holds, not the worst
        let own = ProfileStart {
            capture_frame: Some(110),
            ..Default::default()
        };
        assert_eq!(opens_on(&text, own.clone()), 110);
        // `hitch` asked for is still the hitch
        let asked = ProfileStart {
            frame: Some(FrameChoice::Hitch),
            ..own.clone()
        };
        assert_eq!(opens_on(&text, asked), 105);
        // Taken on a hitch: that hitch, matched by its history frame
        let on_hitch = ProfileStart {
            capture_hitch: Some(117),
            capture_frame: Some(118),
            ..Default::default()
        };
        assert_eq!(opens_on(&text, on_hitch), 117);
        // A frame the trace's ring no longer holds: the hitch it means
        let gone = ProfileStart {
            capture_frame: Some(400),
            ..Default::default()
        };
        assert_eq!(opens_on(&text, gone), 105);
    }

    #[test]
    fn the_keys_step_frames_widen_find_hitches_and_compare() {
        let ctx = egui::Context::default();
        let mut t = tab(ProfileStart {
            frame: Some(FrameChoice::Index(0)),
            ..Default::default()
        });
        frame(&ctx, &mut t, vec![]);
        frame(&ctx, &mut t, vec![key(Key::ArrowRight, false)]);
        assert_eq!(t.selection(), 1..2);
        frame(&ctx, &mut t, vec![key(Key::ArrowRight, true)]);
        assert_eq!(t.selection(), 1..3);
        frame(&ctx, &mut t, vec![key(Key::Home, false)]);
        assert_eq!(t.selection(), 0..1);
        frame(&ctx, &mut t, vec![key(Key::H, false)]);
        assert_eq!(t.selection(), 2..3, "H finds the hitch");
        assert!(!t.comparing());
        frame(&ctx, &mut t, vec![key(Key::C, false)]);
        assert!(t.comparing());
        assert!(t.profile().baseline_if_made().is_some());
    }

    #[test]
    fn a_scope_is_chosen_by_name_and_its_time_plotted_across_frames() {
        let ctx = egui::Context::default();
        let mut t = tab(ProfileStart {
            scope: Some("on_update".into()),
            compare: true,
            ..Default::default()
        });
        assert_eq!(t.chosen(), Some("stall.rhai:on_update"));
        assert_eq!(t.series, vec![1.0, 1.0, 31.0]);
        // Every pane draws, the chosen scope's plot included
        frame(&ctx, &mut t, vec![]);
        frame(&ctx, &mut t, vec![key(Key::Escape, false)]);
        assert_eq!(t.chosen(), None);
    }

    #[test]
    fn under_compare_a_stalls_path_is_vermillion_and_the_rest_quiet() {
        use crate::commands::trace::tests::synthetic;
        let trace = Trace::read(&synthetic(300, 14, 150)).unwrap();
        let t = Profiler::new(
            trace,
            "s.json".into(),
            &ProfileStart {
                compare: true,
                ..Default::default()
            },
        );
        assert_eq!(t.selection(), 150..151, "opens on the hitch");
        let p = t.profile();
        let main = &p.trace.threads[0];
        let find = |name: &str| {
            main.scopes
                .iter()
                .position(|s| s.frame == 150 && p.name(s.name) == name)
                .unwrap()
        };
        let (stall, _) = t.tinted(0, find("stall.rhai:on_update"), Band::Scripts.color());
        let (update, _) = t.tinted(0, find("script::update"), Band::Scripts.color());
        let (physics, _) = t.tinted(0, find("physics::step"), Band::Physics.color());
        assert_eq!(stall, HITCH_COLOR);
        assert_eq!(update, HITCH_COLOR, "its parent took the same time longer");
        assert_eq!(
            physics,
            lerp(Band::Physics.color(), Color32::from_gray(70), 0.62),
            "physics took what it always takes"
        );
    }

    /// The tab's own frame time on a trace of 5 s at 60 fps (#1099's
    /// done-when): `cargo test -p flint-cli --bin flint profiler_cost --
    /// --ignored --nocapture`, and with `--release` for a shipped build.
    #[test]
    #[ignore]
    fn profiler_cost() {
        use crate::commands::trace::tests::synthetic;
        let text = synthetic(300, 14, 150);
        let began = std::time::Instant::now();
        let trace = Trace::read(&text).unwrap();
        let read_ms = began.elapsed().as_secs_f64() * 1000.0;
        let scopes = trace.scope_count();
        let began = std::time::Instant::now();
        let mut t = Profiler::new(trace, "s.json".into(), &ProfileStart::default());
        let made_ms = began.elapsed().as_secs_f64() * 1000.0;
        let ctx = egui::Context::default();
        let time = |t: &mut Profiler, events: Vec<egui::Event>| {
            frame(&ctx, t, events);
            let n = 20;
            let began = std::time::Instant::now();
            for _ in 0..n {
                frame(&ctx, t, vec![]);
            }
            began.elapsed().as_secs_f64() * 1000.0 / n as f64
        };
        let one = time(&mut t, vec![]);
        t.select(0..300);
        let all = time(&mut t, vec![]);
        t.set_compare(true);
        let all_compare = time(&mut t, vec![]);
        t.select(150..151);
        t.choose(t.find_scope("stall"));
        let chosen = time(&mut t, vec![]);
        println!(
            "profiler, {scopes} scopes over 300 frames ({} KB): read {read_ms:.1} ms, \
             worked out {made_ms:.1} ms; a frame of the tab, egui included: \
             one frame chosen {one:.2} ms, all 300 {all:.2} ms, all 300 compared \
             {all_compare:.2} ms, a scope chosen {chosen:.2} ms",
            text.len() / 1024
        );
    }

    #[test]
    fn a_ruler_steps_in_ones_twos_and_fives() {
        assert_eq!(tick_step(10e6, 900.0), 1e6);
        assert_eq!(tick_step(17e6, 900.0), 2e6);
        assert_eq!(tick_step(40e6, 900.0), 5e6);
        assert_eq!(tick_words(2e6), "+2.0 ms");
        assert_eq!(tick_words(250e3), "+250 µs");
        assert_eq!(group_digits(140743), "140 743");
        assert_eq!(ms_text(31_000_000), "31.0");
        assert_eq!(ms_text(450_000), "0.450");
    }
}
