//! The F3 Expression panel (#525): what scripts are hearing from the
//! expression feed (#518), beside the picture instead of in a terminal.
//!
//! Shown only when the player has a feed: the expression subsystem adds it
//! at its first frame (and again after a transition drops the scene's
//! panels) and hands it a [`FeedView`] each frame while it is open, right
//! after the frame's states were delivered, so a state shows the frame it
//! arrives. Top to bottom:
//!
//! - **Now**: the latest state as a key/value table (`state` first, numbers
//!   in 0..1 with a bar), with its source and how long ago it arrived, in
//!   game time.
//! - **Timeline**: a ruler of every event, the playhead and the next event's
//!   time. **Socket**: its address, connections open, and the lines and
//!   states it has dropped, by why.
//! - **Recent**: the last [`RECENT`](super::expression_feed::RECENT) states,
//!   newest first, each with what changed from the one before it, so a
//!   flurry reads as a sequence rather than a blur.

use super::expression_feed::{Arrival, FeedView, Source, TimelineView};
use egui::{Color32, RichText, Sense, Stroke, Vec2};
use serde_json::Value;

pub const EXPRESSION_PANEL: &str = "Expression";

/// How long a new state glows, in game seconds.
const FRESH_SECS: f64 = 1.2;
/// The most changes a recent row spells out before "+N".
const MAX_CHANGES: usize = 3;

/// The timeline's colour: warm, a score being played.
const TIMELINE: Color32 = Color32::from_rgb(232, 178, 104);
/// The socket's colour: cool, a voice from outside.
const SOCKET: Color32 = Color32::from_rgb(110, 196, 196);
/// A dropped count above zero.
const DROPPED: Color32 = Color32::from_rgb(230, 120, 100);

fn source_color(source: Source) -> Color32 {
    match source {
        Source::Timeline => TIMELINE,
        Source::Socket => SOCKET,
    }
}

pub struct ExpressionPanel {
    open: bool,
    /// Every timeline event's time, for the ruler; `None` with no timeline.
    times: Option<Vec<f64>>,
    view: Option<FeedView>,
}

impl ExpressionPanel {
    pub fn new(times: Option<Vec<f64>>) -> Self {
        Self {
            open: false,
            times,
            view: None,
        }
    }

    /// The feed as it stands this frame (only while open).
    pub fn set_view(&mut self, view: FeedView) {
        self.view = Some(view);
    }

    /// What it shows now, for tests.
    #[cfg(test)]
    pub fn view(&self) -> Option<&FeedView> {
        self.view.as_ref()
    }
}

impl flint_debug_ui::DebugPanel for ExpressionPanel {
    fn name(&self) -> &str {
        EXPRESSION_PANEL
    }

    fn ui(&mut self, ui: &mut egui::Ui) {
        let Some(view) = &self.view else {
            ui.label(RichText::new("waiting for the feed's first frame").weak());
            return;
        };
        now_section(ui, view);
        if let Some(timeline) = &view.timeline {
            ui.separator();
            timeline_section(ui, view.now, timeline, self.times.as_deref());
        }
        if let Some(socket) = &view.socket {
            ui.separator();
            socket_section(ui, socket);
        }
        ui.separator();
        recent_section(ui, view);
    }

    fn is_open(&self) -> bool {
        self.open
    }

    fn toggle(&mut self) {
        self.open = !self.open;
    }

    /// The table's rows, the sources' and the strip's.
    fn weight(&self) -> u32 {
        let Some(view) = &self.view else { return 6 };
        let fields = view.record.latest().map_or(1, |a| a.state.len());
        let sources = 2 * (view.timeline.is_some() as usize + view.socket.is_some() as usize);
        (3 + fields + sources + view.record.newest_first().count()) as u32
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

/// How fresh an arrival is at `now`: 1 the frame it came, 0 after
/// [`FRESH_SECS`].
fn freshness(arrival: &Arrival, now: f64) -> f32 {
    (1.0 - (now - arrival.at) / FRESH_SECS).clamp(0.0, 1.0) as f32
}

fn chip(ui: &mut egui::Ui, source: Source) {
    let color = source_color(source);
    egui::Frame::NONE
        .fill(color.gamma_multiply(0.22))
        .stroke(Stroke::new(1.0, color.gamma_multiply(0.7)))
        .corner_radius(3.0)
        .inner_margin(egui::Margin::symmetric(5, 1))
        .show(ui, |ui| {
            ui.label(RichText::new(source.label()).small().color(color));
        });
}

fn now_section(ui: &mut egui::Ui, view: &FeedView) {
    let Some(latest) = view.record.latest() else {
        ui.label(RichText::new("no state yet: scripts' expression() is #{}").weak());
        return;
    };
    let fresh = freshness(latest, view.now);
    ui.horizontal(|ui| {
        chip(ui, latest.source);
        let ago = (view.now - latest.at).max(0.0);
        let text = if latest.frame == view.frame {
            "arrived this frame".to_string()
        } else {
            format!("arrived {ago:.2} s ago")
        };
        ui.label(RichText::new(text).strong());
        ui.label(
            RichText::new(format!("at {:.2} s · frame {}", latest.at, latest.frame))
                .small()
                .weak(),
        );
    });
    let glow = source_color(latest.source).gamma_multiply(0.16 * fresh);
    egui::Frame::NONE
        .fill(glow)
        .corner_radius(3.0)
        .inner_margin(egui::Margin::symmetric(4, 3))
        .show(ui, |ui| {
            egui::Grid::new("expression now")
                .num_columns(2)
                .spacing([10.0, 3.0])
                .show(ui, |ui| {
                    for (key, value) in ordered(&latest.state) {
                        ui.label(RichText::new(key).monospace().weak());
                        value_cell(ui, key, value);
                        ui.end_row();
                    }
                });
        });
}

/// `state` first, then the rest as the map holds them.
fn ordered(state: &serde_json::Map<String, Value>) -> impl Iterator<Item = (&str, &Value)> {
    let first = state.get_key_value("state");
    first
        .into_iter()
        .chain(state.iter().filter(|(k, _)| k.as_str() != "state"))
        .map(|(k, v)| (k.as_str(), v))
}

fn value_cell(ui: &mut egui::Ui, key: &str, value: &Value) {
    match value {
        Value::Number(n) => {
            let x = n.as_f64().unwrap_or(0.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new(format!("{x:.2}")).monospace());
                // A level (0..1) gets a bar; a time doesn't
                if (0.0..=1.0).contains(&x) && key != "t" {
                    level_bar(ui, x as f32);
                }
            });
        }
        Value::String(s) if key == "state" => {
            ui.label(RichText::new(s).strong().size(15.0));
        }
        other => {
            ui.label(RichText::new(short(other, 40)).monospace());
        }
    }
}

fn level_bar(ui: &mut egui::Ui, x: f32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(72.0, 6.0), Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, 3.0, ui.visuals().extreme_bg_color);
    let mut filled = rect;
    filled.set_width(rect.width() * x.clamp(0.0, 1.0));
    painter.rect_filled(filled, 3.0, ui.visuals().text_color().gamma_multiply(0.55));
}

/// A value as one short line: strings bare, the rest as compact JSON.
pub(crate) fn short(value: &Value, max: usize) -> String {
    let text = match value {
        Value::String(s) => s.clone(),
        Value::Number(n) => match n.as_f64() {
            Some(x) if x.fract() == 0.0 && x.abs() < 1e9 => format!("{x:.0}"),
            Some(x) => format!("{x:.2}"),
            None => n.to_string(),
        },
        other => serde_json::to_string(other).unwrap_or_default(),
    };
    if text.chars().count() > max {
        let cut: String = text.chars().take(max.saturating_sub(1)).collect();
        format!("{cut}…")
    } else {
        text
    }
}

fn timeline_section(ui: &mut egui::Ui, now: f64, timeline: &TimelineView, times: Option<&[f64]>) {
    ui.horizontal(|ui| {
        chip(ui, Source::Timeline);
        ui.label(format!("playhead {now:.2} s"));
    });
    if let Some(times) = times {
        ruler(ui, now, times, timeline.played);
    }
    let left = match timeline.next {
        Some(next) => format!(
            "next {next:.2} s (in {:.2}) · {} of {} played",
            (next - now).max(0.0),
            timeline.played,
            timeline.len
        ),
        None => format!("all {} played", timeline.len),
    };
    ui.label(RichText::new(left).small().weak());
}

/// Every event as a tick on one line, played ones solid, and the playhead.
fn ruler(ui: &mut egui::Ui, now: f64, times: &[f64], played: usize) {
    let width = ui.available_width().clamp(120.0, 420.0);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, 16.0), Sense::hover());
    let painter = ui.painter();
    let end = times.last().copied().unwrap_or(0.0).max(now).max(1.0);
    let x_of = |t: f64| rect.left() + 4.0 + (rect.width() - 8.0) * (t / end) as f32;
    let y = rect.center().y;
    let faint = ui.visuals().weak_text_color();
    painter.line_segment(
        [
            egui::pos2(rect.left() + 4.0, y),
            egui::pos2(rect.right() - 4.0, y),
        ],
        Stroke::new(1.0, faint.gamma_multiply(0.6)),
    );
    for (i, &t) in times.iter().enumerate() {
        let at = egui::pos2(x_of(t), y);
        if i < played {
            painter.circle_filled(at, 2.5, TIMELINE);
        } else {
            painter.circle_stroke(at, 2.5, Stroke::new(1.0, faint));
        }
    }
    let x = x_of(now.min(end));
    painter.line_segment(
        [egui::pos2(x, rect.top()), egui::pos2(x, rect.bottom())],
        Stroke::new(1.5, ui.visuals().strong_text_color()),
    );
}

fn socket_section(ui: &mut egui::Ui, socket: &super::expression_feed::SocketCounts) {
    ui.horizontal(|ui| {
        chip(ui, Source::Socket);
        if !socket.open {
            ui.label(format!("{} · opens after the first frame", socket.addr));
            return;
        }
        let n = socket.connections;
        ui.label(format!(
            "{} · {n} connection{}",
            socket.addr,
            if n == 1 { "" } else { "s" }
        ));
    });
    ui.horizontal(|ui| {
        ui.label(RichText::new("dropped").small().weak());
        for (count, why) in [
            (socket.malformed, "malformed"),
            (socket.oversize, "oversize"),
            (socket.flooded, "flooded"),
        ] {
            let text = RichText::new(format!("{count} {why}")).small();
            ui.label(if count > 0 {
                text.color(DROPPED)
            } else {
                text.weak()
            });
        }
    });
}

fn recent_section(ui: &mut egui::Ui, view: &FeedView) {
    let total = view.record.delivered();
    ui.label(
        RichText::new(format!("recent · {total} delivered"))
            .small()
            .weak(),
    );
    let rows: Vec<&Arrival> = view.record.newest_first().collect();
    for (i, arrival) in rows.iter().enumerate() {
        let before = rows.get(i + 1).copied();
        let fresh = freshness(arrival, view.now);
        let color = source_color(arrival.source);
        egui::Frame::NONE
            .fill(color.gamma_multiply(0.14 * fresh))
            .corner_radius(2.0)
            .inner_margin(egui::Margin::symmetric(3, 1))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let (dot, _) = ui.allocate_exact_size(Vec2::splat(8.0), Sense::hover());
                    ui.painter().circle_filled(dot.center(), 3.0, color);
                    ui.label(
                        RichText::new(format!("{:7.2}", arrival.at))
                            .monospace()
                            .small()
                            .weak(),
                    );
                    if let Some(state) = arrival.state.get("state") {
                        ui.label(RichText::new(short(state, 18)).strong());
                    }
                    ui.label(RichText::new(changes(arrival, before)).small());
                });
            });
    }
}

/// What changed from the state before (`t` and `state` aside, which the row
/// shows already), as "certainty 0.50 to 0.20, tone neutral to grave"; the first
/// row lists its fields.
pub(crate) fn changes(arrival: &Arrival, before: Option<&Arrival>) -> String {
    let fields = arrival
        .state
        .iter()
        .filter(|(k, _)| k.as_str() != "t" && k.as_str() != "state");
    let mut parts: Vec<String> = match before {
        None => fields
            .map(|(k, v)| format!("{k} {}", short(v, 12)))
            .collect(),
        Some(before) => {
            let mut parts: Vec<String> = fields
                .filter_map(|(k, v)| match before.state.get(k) {
                    Some(old) if old == v => None,
                    Some(old) => Some(format!("{k} {} to {}", short(old, 12), short(v, 12))),
                    None => Some(format!("+{k} {}", short(v, 12))),
                })
                .collect();
            parts.extend(
                before
                    .state
                    .keys()
                    .filter(|k| !arrival.state.contains_key(*k) && *k != "t" && *k != "state")
                    .map(|k| format!("no {k}")),
            );
            if parts.is_empty() {
                parts.push("same".into());
            }
            parts
        }
    };
    if parts.len() > MAX_CHANGES {
        let more = parts.len() - MAX_CHANGES;
        parts.truncate(MAX_CHANGES);
        parts.push(format!("+{more}"));
    }
    parts.join(", ")
}

#[cfg(test)]
mod tests {
    use super::super::expression_feed::{FeedRecord, SocketCounts};
    use super::*;
    use flint_debug_ui::DebugPanel;

    fn arrival(at: f64, json: &str) -> Arrival {
        Arrival {
            at,
            frame: (at * 60.0) as u64,
            source: Source::Timeline,
            state: serde_json::from_str(json).unwrap(),
        }
    }

    #[test]
    fn a_row_says_what_changed_from_the_one_before() {
        let a = arrival(
            15.5,
            r#"{"state": "thinking", "certainty": 0.5, "tone": "neutral", "t": 15.5}"#,
        );
        let b = arrival(
            18.0,
            r#"{"state": "thinking", "certainty": 0.2, "tone": "grave", "t": 18.0}"#,
        );
        assert_eq!(
            changes(&b, Some(&a)),
            "certainty 0.50 to 0.20, tone neutral to grave"
        );
        assert_eq!(changes(&a, None), "certainty 0.50, tone neutral");
        assert_eq!(changes(&a, Some(&a)), "same");
        let c = arrival(19.0, r#"{"state": "x", "energy": 1}"#);
        assert_eq!(changes(&c, Some(&b)), "+energy 1, no certainty, no tone");
        let wide = arrival(20.0, r#"{"a": 1, "b": 2, "c": 3, "d": 4, "e": 5}"#);
        assert_eq!(changes(&wide, None), "a 1, b 2, c 3, +2");
    }

    #[test]
    fn values_are_one_short_line() {
        assert_eq!(short(&serde_json::json!(0.25), 10), "0.25");
        assert_eq!(short(&serde_json::json!(3), 10), "3");
        assert_eq!(short(&serde_json::json!("grave"), 10), "grave");
        assert_eq!(short(&serde_json::json!([1, 2]), 10), "[1,2]");
        assert_eq!(short(&serde_json::json!("abcdefghijkl"), 6), "abcde…");
    }

    #[test]
    fn it_draws_every_section_headless() {
        let mut record = FeedRecord::default();
        for (i, s) in ["arriving", "present", "listening"].iter().enumerate() {
            record.push(arrival(
                i as f64 * 2.0,
                &format!(r#"{{"state": "{s}", "attention": 0.{i}, "t": {}}}"#, i * 2),
            ));
        }
        let mut panel = ExpressionPanel::new(Some(vec![0.0, 2.0, 4.0, 8.0]));
        panel.toggle();
        panel.set_view(FeedView {
            now: 4.1,
            frame: 246,
            record,
            timeline: Some(TimelineView {
                played: 3,
                len: 4,
                next: Some(8.0),
            }),
            socket: Some(SocketCounts {
                open: true,
                addr: "127.0.0.1:7777".parse().unwrap(),
                connections: 1,
                malformed: 2,
                oversize: 0,
                flooded: 0,
            }),
        });
        assert!(panel.weight() > 6, "{}", panel.weight());
        let ctx = egui::Context::default();
        let output = flint_debug_ui::run_headless_frame(&ctx, egui::RawInput::default(), |ui| {
            egui::CentralPanel::default().show(ui, |ui| panel.ui(ui));
        });
        assert!(!output.shapes.is_empty(), "it drew");
    }
}
