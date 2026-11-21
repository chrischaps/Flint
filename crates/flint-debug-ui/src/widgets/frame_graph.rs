//! A frame-time graph (#1078): one bar a frame for the last few seconds,
//! each bar its frame's time split into a few coloured bands, with the
//! display's refresh period and twice it drawn across, and a mark over
//! every frame that hitched.
//!
//! It is meant to be left on while playing, so it is quiet when nothing is
//! wrong: a bar within a refresh is drawn at a low alpha, and only a bar
//! that runs past the budget takes on its full colour. A hitch is the one
//! loud thing: full colour, a vermillion mark above it, and a faint column
//! behind it, so a spike shows what it was made of and stands out from the
//! frames around it.
//!
//! The widget knows nothing of the player. A host hands it a slice of
//! [`GraphFrame`]s (its own frame records, each turned into a height, the
//! bands and a hitch flag) and keeps the [`FrameGraph`] from frame to frame:
//! it holds the ceiling the bars are scaled to, which eases to fit what is
//! shown, and the one mesh every bar goes into, reused while egui gives it
//! back (it does, once a frame's shapes are tessellated).

use egui::emath::GuiRounding as _;
use egui::{Color32, Mesh, Pos2, Rect, Shape, Stroke, Vec2};
use std::sync::Arc;

/// The bands a bar is split into, bottom up. A host maps its own phases
/// onto these.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Band {
    /// Scripts' callbacks, and the script UI
    Scripts,
    /// The fixed-step physics
    Physics,
    /// Everything else of the host's own work: input, animation, effects
    Other,
    /// Syncing the renderer with the world, drawing and submitting
    Render,
    /// The HUD pass, and handing the frame to the swapchain (the wait for
    /// the display under vsync)
    HudPresent,
    /// The frame's time that no phase accounts for: the OS, the driver,
    /// the compositor
    Elsewhere,
}

/// How many bands a frame has.
pub const BANDS: usize = Band::ALL.len();

impl Band {
    pub const ALL: [Band; 6] = [
        Band::Scripts,
        Band::Physics,
        Band::Other,
        Band::Render,
        Band::HudPresent,
        Band::Elsewhere,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Band::Scripts => "scripts",
            Band::Physics => "physics",
            Band::Other => "other",
            Band::Render => "render",
            Band::HudPresent => "hud/present",
            Band::Elsewhere => "elsewhere",
        }
    }

    /// The band's colour, from Okabe and Ito's palette (2008), which stays
    /// apart under the common colour-vision deficiencies. Vermillion is
    /// kept for the hitch mark alone.
    pub fn color(self) -> Color32 {
        match self {
            Band::Scripts => Color32::from_rgb(230, 159, 0), // orange
            Band::Physics => Color32::from_rgb(0, 158, 115), // bluish green
            Band::Other => Color32::from_rgb(240, 228, 66),  // yellow
            Band::Render => Color32::from_rgb(86, 180, 233), // sky blue
            Band::HudPresent => Color32::from_rgb(204, 121, 167), // reddish purple
            Band::Elsewhere => Color32::from_gray(150),
        }
    }
}

/// The hitch mark's colour: Okabe and Ito's vermillion.
pub const HITCH_COLOR: Color32 = Color32::from_rgb(213, 94, 0);

/// One frame, as the graph draws it. Times are ms unless named.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct GraphFrame {
    /// When it began, seconds (any epoch, the same for every frame).
    pub at: f64,
    /// Its wall time: how much of the graph's width it takes.
    pub wall_ms: f32,
    /// The bar's height: how long it stood on screen (under vsync pacing,
    /// whole refreshes; else the wall time).
    pub ms: f32,
    /// Its time in each band, indexed as [`Band::ALL`], stacked bottom
    /// up. What runs past `ms` is cut off at the top of the bar.
    pub bands: [f32; BANDS],
    /// The frame was a hitch: it gets a mark.
    pub hitch: bool,
}

impl GraphFrame {
    /// When it ended, seconds.
    pub fn end(&self) -> f64 {
        self.at + self.wall_ms as f64 / 1000.0
    }
}

/// How the graph is laid out.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GraphLayout {
    /// The graph's size, points.
    pub size: Vec2,
    /// The seconds its width spans, up to the newest frame's end.
    pub window_secs: f32,
    /// The display's refresh period, ms: the budget line, and twice it.
    pub budget_ms: f32,
    /// Label the budget lines in ms and fps (the compact form leaves the
    /// fps off).
    pub fps_labels: bool,
}

/// A column's width, points. Each column is a slice of time, and shows
/// the worst frame that falls in it: one frame a column at 60 Hz in a
/// four-second graph, the worst of four at 240 Hz, and a 100 ms stall six
/// columns wide, as long as it took.
pub const COLUMN_POINTS: f32 = 1.0;
/// The tallest the ceiling goes, ms. A frame past it (a scene load) is
/// cut off at the top rather than flattening every other bar for seconds.
pub const MAX_CEILING_MS: f32 = 250.0;
/// How fast the ceiling rises to a spike and falls back, per second.
const RISE: f32 = 14.0;
const FALL: f32 = 1.2;
/// No frame in a column.
const EMPTY: u32 = u32::MAX;

/// The graph's state from frame to frame; see the module docs.
pub struct FrameGraph {
    ceiling_ms: Option<f32>,
    mesh: Arc<Mesh>,
    /// Each column's frame (an index into the frames shown), and each
    /// hitch's span of columns: room kept from frame to frame.
    columns: Vec<u32>,
    marks: Vec<(usize, usize, u32)>,
    /// Where the last `show` put its columns.
    placed: Option<Placed>,
}

/// Where a graph's columns were drawn, for a host that maps a pointer to a
/// frame or a time to a place on it (the profiler's strip, #1099).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Placed {
    /// The graph's rectangle.
    pub rect: Rect,
    first: i64,
    slice: f64,
    col_w: f32,
    cols: usize,
}

impl Placed {
    /// The x of time `t`, seconds on the frames' epoch.
    pub fn x_of(&self, t: f64) -> f32 {
        let col = t / self.slice - self.first as f64;
        self.rect.right() - (self.cols as f64 - col) as f32 * self.col_w
    }

    /// The time at `x`, seconds on the frames' epoch.
    pub fn time_at(&self, x: f32) -> f64 {
        let col = self.cols as f64 - ((self.rect.right() - x) / self.col_w) as f64;
        (col + self.first as f64) * self.slice
    }

    /// The column `x` falls in, if any.
    fn column_at(&self, x: f32) -> Option<usize> {
        let left = self.rect.right() - self.cols as f32 * self.col_w;
        let c = ((x - left) / self.col_w).floor();
        (c >= 0.0 && (c as usize) < self.cols).then_some(c as usize)
    }
}

impl Default for FrameGraph {
    fn default() -> Self {
        Self::new()
    }
}

impl FrameGraph {
    pub fn new() -> Self {
        Self {
            ceiling_ms: None,
            mesh: Arc::new(Mesh::default()),
            columns: Vec::new(),
            marks: Vec::new(),
            placed: None,
        }
    }

    /// Where the last [`show`](Self::show) drew, if it drew.
    pub fn placed(&self) -> Option<Placed> {
        self.placed
    }

    /// The frame (an index into the frames last shown) the column under
    /// `x` shows: the tallest that touches it.
    pub fn frame_at(&self, x: f32) -> Option<usize> {
        let c = self.placed?.column_at(x)?;
        let i = *self.columns.get(c)?;
        (i != EMPTY).then_some(i as usize)
    }

    /// The ms the top of the graph stands for now.
    pub fn ceiling_ms(&self) -> Option<f32> {
        self.ceiling_ms
    }

    /// How many columns a graph `width` points wide has at `ppp` pixels
    /// per point.
    pub fn columns(width: f32, ppp: f32) -> usize {
        (width / column_width(ppp)).floor().max(1.0) as usize
    }

    /// The mesh's vertex buffer, for a test that it is reused.
    #[doc(hidden)]
    pub fn mesh_buffer(&self) -> (*const egui::epaint::Vertex, usize) {
        (self.mesh.vertices.as_ptr(), self.mesh.vertices.capacity())
    }

    /// Draw `frames` (oldest first, the newest ending at the right edge;
    /// those older than the window are passed over) into the next
    /// `layout.size` of `ui`. Passive: it senses hover only.
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        frames: &[GraphFrame],
        layout: GraphLayout,
    ) -> egui::Response {
        let (rect, response) = ui.allocate_exact_size(layout.size, egui::Sense::hover());
        if !ui.is_rect_visible(rect) {
            return response;
        }
        let ppp = ui.ctx().pixels_per_point();
        let col_w = column_width(ppp);
        let cols = Self::columns(rect.width(), ppp);
        let budget = layout.budget_ms.max(1.0);
        self.place(frames, cols, layout.window_secs as f64);
        let slice = (layout.window_secs as f64).max(1e-3) / cols as f64;
        let last = frames
            .last()
            .map_or(0, |f| (f.end() / slice).floor() as i64);
        self.placed = Some(Placed {
            rect,
            first: last - cols as i64 + 1,
            slice,
            col_w,
            cols,
        });

        // The ceiling eases toward the tallest bar shown, never under a
        // little over twice the budget, so both lines are always in view.
        let tallest = self
            .columns
            .iter()
            .filter(|&&i| i != EMPTY)
            .map(|&i| frames[i as usize].ms)
            .fold(0.0, f32::max);
        let target = (tallest * 1.1).max(budget * 2.4).min(MAX_CEILING_MS);
        let dt = ui.input(|i| i.stable_dt).clamp(0.0, 0.25);
        let ceiling = match self.ceiling_ms {
            None => target,
            Some(c) => {
                let rate = if target > c { RISE } else { FALL };
                c + (target - c) * (1.0 - (-rate * dt).exp())
            }
        };
        self.ceiling_ms = Some(ceiling);
        let y = |ms: f32| rect.bottom() - (ms / ceiling).min(1.0) * rect.height();
        let x = |col: usize| rect.right() - (cols - col) as f32 * col_w;

        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 2.0, Color32::from_rgba_unmultiplied(255, 255, 255, 6));

        // Every bar, mark and column into the one mesh, kept from the last
        // frame when egui has let go of it.
        let mesh = Arc::make_mut(&mut self.mesh);
        mesh.vertices.clear();
        mesh.indices.clear();
        for &(c0, c1, _) in &self.marks {
            // A faint band behind a hitch, over the graph's height
            let band = Rect::from_x_y_ranges(x(c0)..=x(c1) + col_w, rect.y_range());
            mesh.add_colored_rect(band, HITCH_COLOR.gamma_multiply(0.16));
        }
        for (col, &i) in self.columns.iter().enumerate() {
            if i == EMPTY {
                continue;
            }
            let frame = &frames[i as usize];
            let (x0, x1) = (x(col), x(col) + col_w);
            let loudness = loudness(frame, budget);
            let mut stacked = 0.0;
            for (band, &ms) in Band::ALL.iter().zip(&frame.bands) {
                let h = ms.min(frame.ms - stacked);
                if h <= 0.0 {
                    continue;
                }
                let r = Rect::from_x_y_ranges(x0..=x1, y(stacked + h)..=y(stacked));
                mesh.add_colored_rect(r, band.color().gamma_multiply(loudness));
                stacked += h;
            }
        }
        for &(c0, c1, i) in &self.marks {
            // A small downward triangle over the middle of the hitch
            let size = 3.0;
            let mid = (x(c0) + x(c1) + col_w) * 0.5;
            let tip = (y(frames[i as usize].ms) - 1.5).max(rect.top() + size * 1.4 + 1.0);
            let v = mesh.vertices.len() as u32;
            mesh.colored_vertex(Pos2::new(mid, tip), HITCH_COLOR);
            mesh.colored_vertex(Pos2::new(mid - size, tip - size * 1.4), HITCH_COLOR);
            mesh.colored_vertex(Pos2::new(mid + size, tip - size * 1.4), HITCH_COLOR);
            mesh.add_triangle(v, v + 1, v + 2);
        }
        painter.add(Shape::Mesh(self.mesh.clone()));

        // The budget lines, over the bars, labelled at the left; twice the
        // budget's label only where there is room for it.
        let font = egui::FontId::monospace(8.5);
        let mut last_label = f32::INFINITY;
        for (n, alpha) in [(1.0, 70), (2.0, 40)] {
            let ms = budget * n;
            if ms >= ceiling {
                continue;
            }
            let line_y = y(ms).round_to_pixel_center(ppp);
            painter.hline(
                rect.x_range(),
                line_y,
                Stroke::new(1.0 / ppp, Color32::from_white_alpha(alpha)),
            );
            if last_label - line_y < 10.0 {
                continue;
            }
            last_label = line_y;
            let label = if layout.fps_labels {
                format!("{ms:.1} ms  {:.0} fps", 1000.0 / ms)
            } else {
                format!("{ms:.1}")
            };
            // A soft shadow under it, so it reads over the bars
            let at = Pos2::new(rect.left() + 3.0, line_y - 1.0);
            let shadow = Color32::from_black_alpha(200);
            let galley = painter.layout_no_wrap(label, font.clone(), Color32::WHITE);
            let at = egui::Align2::LEFT_BOTTOM.anchor_size(at, galley.size()).min;
            painter.galley_with_override_text_color(
                at + Vec2::splat(1.0 / ppp),
                galley.clone(),
                shadow,
            );
            painter.galley_with_override_text_color(
                at,
                galley,
                Color32::from_white_alpha(alpha + 50),
            );
        }
        response
    }

    /// Which frame each of `cols` columns shows, the last `window` seconds
    /// up to the newest frame's end: the tallest that touches it. Columns
    /// are slices of time counted from the epoch, not from now, so as time
    /// passes the bars move a whole column at a time and never shimmer.
    fn place(&mut self, frames: &[GraphFrame], cols: usize, window: f64) {
        self.columns.clear();
        self.columns.resize(cols, EMPTY);
        self.marks.clear();
        let Some(newest) = frames.last() else {
            return;
        };
        let slice = window.max(1e-3) / cols as f64;
        let col_of = |t: f64| (t / slice).floor() as i64;
        let last = col_of(newest.end());
        let first = last - cols as i64 + 1;
        for (i, f) in frames.iter().enumerate().rev() {
            // The columns it touches: from its start to just before its end
            let c1 = col_of(f.end() - slice * 1e-6).max(col_of(f.at));
            if c1 < first {
                break;
            }
            let c0 = col_of(f.at).max(first);
            let c1 = c1.min(last);
            for c in c0..=c1 {
                let slot = &mut self.columns[(c - first) as usize];
                if *slot == EMPTY || f.ms > frames[*slot as usize].ms {
                    *slot = i as u32;
                }
            }
            if f.hitch {
                self.marks
                    .push(((c0 - first) as usize, (c1 - first) as usize, i as u32));
            }
        }
    }
}

/// A column's width at `ppp` pixels per point: [`COLUMN_POINTS`], in
/// whole pixels, so the bars stay on the pixel grid.
fn column_width(ppp: f32) -> f32 {
    (COLUMN_POINTS * ppp).round().max(1.0) / ppp
}

/// How much of its colour a bar shows: little within the budget, more as
/// it runs past it, all of it at twice the budget or for a hitch.
fn loudness(frame: &GraphFrame, budget: f32) -> f32 {
    const QUIET: f32 = 0.42;
    if frame.hitch {
        return 1.0;
    }
    let over = ((frame.ms / budget - 1.15) / 0.85).clamp(0.0, 1.0);
    QUIET + (1.0 - QUIET) * over
}

// What the counters under a graph say. Times are ms unless named.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct HitchCounters<'a> {
    /// Hitches since the run began
    pub hitches: u64,
    /// Hitches a minute, over the run
    pub per_minute: f64,
    /// The worst frame judged
    pub worst_ms: f32,
    /// Seconds since the last hitch ended, if there was one
    pub since_last_secs: Option<f64>,
    /// The last hitch in one line: how long and whose time it was
    pub last: Option<&'a str>,
    /// Hitches of scene loads drawn behind a fade, counted apart from
    /// `hitches` (flint-player's `loading`, #1125); shown when there are any
    pub loading: u64,
}

/// How long the hitch count stays loud after a hitch, seconds, and how
/// long it then takes to fade back to quiet.
const LOUD_SECS: f64 = 2.0;
const FADE_SECS: f64 = 1.5;

/// The counters under a graph: the count (vermillion for a moment after a
/// hitch, then grey), a minute's rate, the worst frame, how long since the
/// last, and the last hitch's cause on a line of its own. `compact` leaves
/// the rate out, for a graph left on in a corner.
pub fn hitch_counters(ui: &mut egui::Ui, c: &HitchCounters, compact: bool) {
    let quiet = Color32::from_gray(130);
    let dim = Color32::from_gray(95);
    let heat = c.since_last_secs.map_or(0.0, |s| {
        (1.0 - ((s - LOUD_SECS) / FADE_SECS).clamp(0.0, 1.0)) as f32
    });
    let count_color = lerp_color(Color32::from_gray(200), HITCH_COLOR, heat);
    ui.spacing_mut().item_spacing.x = 0.0;
    ui.horizontal(|ui| {
        ui.colored_label(
            count_color,
            egui::RichText::new(format!("{}", c.hitches)).strong(),
        );
        ui.colored_label(quiet, if c.hitches == 1 { " hitch" } else { " hitches" });
        if !compact {
            ui.colored_label(dim, format!("  {:.1}/min", c.per_minute));
        }
        if c.loading > 0 {
            ui.colored_label(dim, format!("  +{} loading", c.loading));
        }
        ui.colored_label(dim, format!("  worst {:.0} ms", c.worst_ms));
        if let Some(s) = c.since_last_secs {
            ui.colored_label(dim, format!("  {} ago", seconds(s)));
        }
    });
    if let Some(last) = c.last {
        ui.add(
            egui::Label::new(egui::RichText::new(last).color(lerp_color(dim, quiet, heat)))
                .truncate(),
        );
    }
}

/// The bands' colours with their names, in one wrapping row.
pub fn band_legend(ui: &mut egui::Ui) {
    band_legend_of(ui, Band::ALL.map(|band| (band, band.label())));
}

/// The legend for a host that fills only some bands, each named in its own
/// words (the scene viewer's `update` and `egui`, #1110), in the order given.
pub fn band_legend_of<'a>(ui: &mut egui::Ui, bands: impl IntoIterator<Item = (Band, &'a str)>) {
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(6.0, 1.0);
        for (band, label) in bands {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 3.0;
                let (r, _) = ui.allocate_exact_size(egui::vec2(6.0, 6.0), egui::Sense::hover());
                ui.painter().rect_filled(r, 1.0, band.color());
                ui.colored_label(Color32::from_gray(120), label);
            });
        }
    });
}

fn seconds(s: f64) -> String {
    if s < 60.0 {
        format!("{s:.1} s")
    } else {
        format!("{}m{:02.0}s", (s / 60.0) as u64, s % 60.0)
    }
}

fn lerp_color(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let mix = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgba_premultiplied(
        mix(a.r(), b.r()),
        mix(a.g(), b.g()),
        mix(a.b(), b.b()),
        mix(a.a(), b.a()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const BANDS_16: [f32; BANDS] = [2.0, 1.0, 0.5, 3.0, 9.0, 1.2];

    /// `n` frames of `ms` each, back to back from t = 0.
    fn steady(n: usize, ms: f32) -> Vec<GraphFrame> {
        (0..n)
            .map(|i| GraphFrame {
                at: i as f64 * ms as f64 / 1000.0,
                wall_ms: ms,
                ms,
                bands: BANDS_16,
                hitch: false,
            })
            .collect()
    }

    /// One more frame after the last, `ms` long.
    fn push(frames: &mut Vec<GraphFrame>, ms: f32, hitch: bool) {
        let at = frames.last().map_or(0.0, GraphFrame::end);
        frames.push(GraphFrame {
            at,
            wall_ms: ms,
            ms,
            bands: BANDS_16,
            hitch,
        });
    }

    fn layout(window_secs: f32) -> GraphLayout {
        GraphLayout {
            size: egui::vec2(200.0, 60.0),
            window_secs,
            budget_ms: 16.7,
            fps_labels: true,
        }
    }

    fn frame(
        ctx: &egui::Context,
        graph: &mut FrameGraph,
        frames: &[GraphFrame],
        window_secs: f32,
    ) -> egui::FullOutput {
        crate::run_headless_frame(ctx, Default::default(), |ui| {
            graph.show(ui, frames, layout(window_secs));
        })
    }

    #[test]
    fn the_mesh_buffer_is_reused_once_egui_lets_go_of_it() {
        let ctx = egui::Context::default();
        let mut graph = FrameGraph::new();
        let frames = steady(400, 16.7);
        let out = frame(&ctx, &mut graph, &frames, 4.0);
        // The host tessellates the frame's shapes, then drops them.
        drop(ctx.tessellate(out.shapes, out.pixels_per_point));
        let first = graph.mesh_buffer();
        assert!(first.1 > 0);
        for _ in 0..5 {
            let out = frame(&ctx, &mut graph, &frames, 4.0);
            drop(ctx.tessellate(out.shapes, out.pixels_per_point));
            assert_eq!(graph.mesh_buffer(), first, "the same buffer, the same room");
        }
    }

    #[test]
    fn columns_are_whole_pixels() {
        assert_eq!(FrameGraph::columns(200.0, 1.0), 200);
        // At 1.5 pixels a point a column is 2 pixels: 150 of them in 300 px.
        assert_eq!(FrameGraph::columns(200.0, 1.5), 150);
    }

    #[test]
    fn at_240_hz_a_column_shows_the_worst_of_its_frames() {
        // Four seconds over 200 columns: 20 ms a column, about five frames.
        let mut frames = steady(1200, 1000.0 / 240.0);
        push(&mut frames, 9.0, false);
        for _ in 0..100 {
            push(&mut frames, 1000.0 / 240.0, false);
        }
        let mut graph = FrameGraph::new();
        graph.place(&frames, 200, 4.0);
        assert!(
            graph.columns.iter().all(|&i| i != EMPTY),
            "every column has a frame"
        );
        let shown: Vec<f32> = graph
            .columns
            .iter()
            .map(|&i| frames[i as usize].ms)
            .collect();
        // In the one or two columns it touches, over the quick frames there
        let worst = shown.iter().filter(|&&ms| ms == 9.0).count();
        assert!((1..=2).contains(&worst), "{worst}");
        assert!(graph.marks.is_empty());
    }

    #[test]
    fn a_stall_is_as_wide_as_the_time_it_took_and_marked_once() {
        let mut frames = steady(600, 1000.0 / 240.0);
        push(&mut frames, 100.0, true);
        for _ in 0..240 {
            push(&mut frames, 1000.0 / 240.0, false);
        }
        let mut graph = FrameGraph::new();
        graph.place(&frames, 200, 4.0);
        assert_eq!(graph.marks.len(), 1);
        let (c0, c1, i) = graph.marks[0];
        assert!(frames[i as usize].hitch);
        // 100 ms over 20 ms columns: five, or six where it straddles one
        let wide = c1 - c0 + 1;
        assert!((5..=6).contains(&wide), "{wide}");
        // A second later, it is a second's worth of columns from the right
        assert!((145..=155).contains(&c1), "{c1}");
    }

    #[test]
    fn frames_older_than_the_window_are_passed_over() {
        let mut frames = steady(60, 16.7);
        push(&mut frames, 100.0, true);
        // Five seconds more: the stall is out of a four-second window
        for _ in 0..300 {
            push(&mut frames, 16.7, false);
        }
        let mut graph = FrameGraph::new();
        graph.place(&frames, 200, 4.0);
        assert!(graph.marks.is_empty());
        let newest = graph.columns.iter().rev().find(|&&i| i != EMPTY).unwrap();
        assert_eq!(*newest as usize, frames.len() - 1);
    }

    #[test]
    fn the_ceiling_jumps_toward_a_spike_and_drifts_back() {
        let ctx = egui::Context::default();
        let mut graph = FrameGraph::new();
        let mut frames = steady(200, 16.7);
        frame(&ctx, &mut graph, &frames, 4.0);
        let calm = graph.ceiling_ms().unwrap();
        assert!((calm - 16.7 * 2.4).abs() < 0.1, "{calm}");
        push(&mut frames, 110.0, true);
        for _ in 0..30 {
            frame(&ctx, &mut graph, &frames, 4.0);
        }
        let spiked = graph.ceiling_ms().unwrap();
        assert!(spiked > 110.0, "{spiked}");
        assert!(spiked <= 121.0 + 0.1, "{spiked}");
        // A frame past the cap doesn't flatten the graph.
        push(&mut frames, 2000.0, false);
        for _ in 0..200 {
            frame(&ctx, &mut graph, &frames, 4.0);
        }
        assert!(graph.ceiling_ms().unwrap() <= MAX_CEILING_MS);
    }

    #[test]
    fn a_bar_within_budget_is_quiet_and_a_hitch_is_loud() {
        let mut f = steady(1, 16.7)[0];
        let quiet = loudness(&f, 16.7);
        f.ms = 40.0;
        let late = loudness(&f, 16.7);
        f.hitch = true;
        assert!(quiet < 0.5);
        assert!(late > 0.99);
        assert_eq!(loudness(&f, 16.7), 1.0);
    }

    #[test]
    fn the_bands_have_six_distinct_colours_apart_from_the_mark() {
        let mut colours: Vec<Color32> = Band::ALL.iter().map(|b| b.color()).collect();
        colours.push(HITCH_COLOR);
        for (i, a) in colours.iter().enumerate() {
            for b in &colours[i + 1..] {
                assert_ne!(a, b);
            }
        }
    }
}
