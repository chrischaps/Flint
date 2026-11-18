//! The frame debugger's window, laid out: a header saying what was
//! captured, the passes as a tree on the left, the selected pass's image in
//! the middle, its draws on the right, and the frame's times along the
//! bottom. Everything links: a pass selects its image and its draws, a
//! draw selects its entity, and the entity is marked in every pass.

use super::super::display::{Channel, Mode};
use super::super::read::Capture;
use super::loader::{Loader, Slot};
use super::model::{Model, Sort, TableQuery};
use super::replay::{Replay, Status};
use super::timing::{band_of, Timing};
use super::viewer::{self, Side, ViewState, Wipe};
use crate::commands::render::replay_host::Controls;
use egui::{Align, Color32, FontId, Key, Layout, Pos2, Rect, RichText, Sense, Stroke, Vec2};
use flint_debug_ui::widgets::{Band, FrameGraph, GraphLayout};
use std::collections::HashMap;

/// The window's colours: a warm dark ground, one teal for what is
/// selected, a gold thread for the selected entity, and the F2 overlay's
/// Okabe-Ito bands for time (vermillion only for a hitch).
pub(super) mod ink {
    use egui::Color32;
    pub const GROUND: Color32 = Color32::from_rgb(20, 21, 24);
    pub const PANEL: Color32 = Color32::from_rgb(27, 28, 32);
    pub const RAISED: Color32 = Color32::from_rgb(36, 37, 43);
    pub const RULE: Color32 = Color32::from_rgb(48, 49, 56);
    pub const TEXT: Color32 = Color32::from_rgb(222, 218, 208);
    pub const DIM: Color32 = Color32::from_rgb(140, 137, 130);
    pub const FAINT: Color32 = Color32::from_rgb(96, 95, 92);
    pub const SELECT: Color32 = Color32::from_rgb(38, 92, 88);
    pub const SELECT_LINE: Color32 = Color32::from_rgb(86, 190, 176);
    pub const ENTITY: Color32 = Color32::from_rgb(232, 196, 110);
    pub const GPU: Color32 = Color32::from_rgb(86, 180, 233);
    pub const GPU_ALT: Color32 = Color32::from_rgb(58, 128, 170);
    pub const HITCH: Color32 = Color32::from_rgb(213, 94, 0);
}

/// What the window opens on.
#[derive(Debug, Clone, Default)]
pub struct Start {
    /// A pass by name or key.
    pub pass: Option<String>,
    /// An image of it by target id.
    pub target: Option<String>,
    /// An entity by name.
    pub entity: Option<String>,
    /// A wipe to start with.
    pub wipe: Option<Wipe>,
    /// Replay the frame at once, under these controls (#1089).
    pub replay: Option<Controls>,
    /// The scene, when it isn't where the capture says.
    pub scene: Option<std::path::PathBuf>,
    /// `--schemas`, for the replay's load.
    pub schemas: Vec<String>,
}

pub struct App {
    capture: Capture,
    shown: String,
    model: Model,
    timing: Timing,
    loader: Loader,
    selected: usize,
    /// Each pass's chosen image, among its own.
    choice: HashMap<usize, usize>,
    draw: Option<usize>,
    entity: Option<String>,
    query: TableQuery,
    view: ViewState,
    graph: FrameGraph,
    summary: String,
    exposure: f32,
    srgb_target: bool,
    /// The largest texture the GPU takes, texels a side.
    max_side: u32,
    /// A line said for a moment (Copied), and until when.
    toast: Option<(String, f64)>,
    /// The tree should bring the selected pass into view.
    scroll_tree: bool,
    styled: bool,
    /// Drawing the frame again (#1089).
    replay: Replay,
    /// The captured frame, while a replay is shown in its place.
    captured: Option<(Capture, String)>,
    /// Whether the capture holds replay inputs.
    replayable: bool,
    /// Added to each image's key in the viewer's GPU cache, a new one each
    /// time another capture (a replay) is shown.
    key_base: usize,
}

impl App {
    pub fn new(
        capture: Capture,
        shown: String,
        start: &Start,
        srgb_target: bool,
        max_side: u32,
    ) -> Self {
        let d = &capture.description;
        let model = Model::new(d);
        let timing = Timing::new(d, &model.passes);
        let summary = super::super::show::summary_of(d, &shown);
        let exposure = super::super::image_out::exposure(d);
        let replayable = d.replay.as_ref().is_some_and(|r| r.available);
        let replay = Replay::new(&capture.dir, start.scene.clone(), start.schemas.clone());
        let mut app = Self {
            replay,
            captured: None,
            replayable,
            key_base: 0,
            selected: model.default_pass(),
            model,
            timing,
            loader: Loader::new(2, 24),
            choice: HashMap::new(),
            draw: None,
            entity: None,
            query: TableQuery::default(),
            view: ViewState::default(),
            graph: FrameGraph::new(),
            summary,
            exposure,
            srgb_target,
            max_side,
            toast: None,
            scroll_tree: true,
            styled: false,
            shown,
            capture,
        };
        if let Some(name) = &start.pass {
            match app.model.find_pass(name) {
                Some(row) => app.selected = row,
                None => tracing::warn!("no pass named {name:?} in this capture"),
            }
        }
        if let Some(target) = &start.target {
            let row = &app.model.passes[app.selected.min(app.model.passes.len().saturating_sub(1))];
            if let Some(i) = row
                .images
                .iter()
                .position(|&im| app.model.images[im].image.target == *target)
            {
                app.choice.insert(app.selected, i);
            }
        }
        if let Some(entity) = &start.entity {
            app.select_entity(entity.clone());
        }
        if let Some(wipe) = start.wipe {
            app.view.wipe = wipe;
        }
        if let Some(controls) = &start.replay {
            app.replay.want = controls.clone();
            app.replay.start();
            // A step of a pass is that pass's image
            if controls.draws.is_some() && start.pass.is_none() {
                if let Some(row) = app.model.find_pass(controls.stepped_name()) {
                    app.selected = row;
                }
            }
        }
        app
    }

    /// Wait for the replay under way, and show it: for a screenshot.
    pub fn wait_for_replay(&mut self) {
        if let Some(drawn) = self.replay.wait() {
            self.show_replay(&drawn.dir);
        }
    }

    /// Show the capture at `dir` (a replay) in place of the captured frame,
    /// on the same pass and entity.
    fn show_replay(&mut self, dir: &std::path::Path) {
        match Capture::open(dir) {
            Ok(replayed) => {
                let shown = Capture::shown(dir);
                let was = self.show(replayed, shown);
                if self.captured.is_none() {
                    self.captured = Some(was);
                }
                // The CPU side stays the captured frame's: a replay runs no
                // game loop, and those phases are the frame it draws (#1208)
                if let Some((original, _)) = &self.captured {
                    self.timing = std::mem::take(&mut self.timing).replaying(&original.description);
                }
            }
            Err(e) => self.replay.status = Status::Failed(format!("{e:#}")),
        }
    }

    /// Put the captured frame back.
    fn show_captured(&mut self) {
        if let Some((capture, shown)) = self.captured.take() {
            self.show(capture, shown);
        }
    }

    /// Show `capture`, keeping the pass, its image and the entity selected;
    /// returns what was shown.
    fn show(&mut self, capture: Capture, shown: String) -> (Capture, String) {
        let key = self.model.passes.get(self.selected).map(|p| p.key.clone());
        let d = &capture.description;
        self.model = Model::new(d);
        self.timing = Timing::new(d, &self.model.passes);
        self.summary = super::super::show::summary_of(d, &shown);
        self.exposure = super::super::image_out::exposure(d);
        self.loader = Loader::new(2, 24);
        self.key_base += 1 << 20;
        self.selected = key
            .and_then(|k| self.model.find_pass(&k))
            .unwrap_or_else(|| self.model.default_pass());
        self.draw = None;
        let was = (
            std::mem::replace(&mut self.capture, capture),
            std::mem::replace(&mut self.shown, shown),
        );
        if let Some(entity) = self.entity.clone() {
            self.select_entity(entity);
        }
        was
    }

    /// Whether a replay is on screen in place of the captured frame.
    fn showing_replay(&self) -> bool {
        self.captured.is_some()
    }

    /// Ask for the frame under `controls`.
    fn replay_with(&mut self, controls: Controls) {
        self.replay.ask(controls);
    }

    /// The pass the draw slider and `[` `]` step (#1204): the selected pass
    /// when it can be stepped (a shadow layer, the style mask or albedo),
    /// else the scene's.
    fn stepped_pass(&self) -> flint_render::SteppedPass {
        self.model
            .passes
            .get(self.selected)
            .filter(|row| row.panel.is_none())
            .and_then(|row| flint_render::SteppedPass::from_name(&row.name))
            .unwrap_or(flint_render::SteppedPass::Scene)
    }

    /// The captured frame's draws of `pass`: the slider's end.
    fn pass_draws(&self, pass: flint_render::SteppedPass) -> usize {
        self.captured_description()
            .pass(pass.name())
            .map_or(0, |p| p.draws.len())
    }

    /// How many of `pass`'s draws the controls ask for: all of them unless
    /// it is the pass they step.
    fn asked_draws(&self, pass: flint_render::SteppedPass) -> usize {
        let want = &self.replay.want;
        match want.draws {
            Some(n) if want.stepped() == Ok(pass) => n,
            _ => self.pass_draws(pass),
        }
    }

    /// The controls asked for, with `pass` drawn to its `k`th draw (`None`:
    /// whole, which also lifts a limit on another pass).
    fn stepped_to(&self, pass: flint_render::SteppedPass, k: Option<usize>) -> Controls {
        let mut c = self.replay.want.clone();
        c.draws = k;
        c.pass = (k.is_some() && pass != flint_render::SteppedPass::Scene)
            .then(|| pass.name().to_string());
        c
    }

    /// Whether an image is still being read (a screenshot waits for it).
    pub fn loading(&self) -> bool {
        self.loader.pending()
    }

    /// Block until every image asked for is read.
    pub fn wait_for_images(&mut self) {
        self.loader.wait();
    }

    fn select_entity(&mut self, name: String) {
        // A part of a name finds the first entity that has it
        let name = if self
            .model
            .draws
            .iter()
            .any(|d| d.draw.entity.as_deref() == Some(name.as_str()))
        {
            name
        } else {
            let lower = name.to_lowercase();
            match self
                .model
                .draws
                .iter()
                .filter_map(|d| d.draw.entity.as_ref())
                .find(|e| e.to_lowercase().contains(&lower))
            {
                Some(found) => found.clone(),
                None => {
                    tracing::warn!("no draw in this capture is of an entity named {name:?}");
                    self.query.filter = name;
                    self.query.all_passes = true;
                    return;
                }
            }
        };
        self.query.filter = name.clone();
        self.query.all_passes = true;
        self.draw = self
            .model
            .draws
            .iter()
            .position(|d| {
                d.draw.entity.as_deref() == Some(name.as_str()) && d.pass == self.selected
            })
            .or_else(|| {
                self.model
                    .draws
                    .iter()
                    .position(|d| d.draw.entity.as_deref() == Some(name.as_str()))
            });
        self.entity = Some(name);
    }

    fn select_pass(&mut self, row: usize) {
        if row != self.selected {
            self.selected = row;
            self.query.layer = None;
            self.scroll_tree = true;
        }
    }

    /// The image the viewer shows and the one it compares with.
    fn images(&self) -> (Option<(usize, bool)>, Option<usize>) {
        let choice = self.choice.get(&self.selected).copied().unwrap_or(0);
        let shown = self.model.shown_image(self.selected, choice);
        let compare = shown.and_then(|(a, _)| match self.view.wipe {
            Wipe::Off => None,
            Wipe::Previous => self.model.previous_image(a),
            Wipe::Final => self.model.final_image().filter(|f| *f != a),
        });
        (shown, compare)
    }

    fn request(&mut self, image: usize) {
        let im = &self.model.images[image];
        let path = self.capture.file(&im.image);
        self.loader.request(image, path, &im.image, self.exposure);
    }

    fn style(&mut self, ctx: &egui::Context) {
        if self.styled {
            return;
        }
        self.styled = true;
        apply_style(ctx);
    }
}

/// The window's look, for both tabs.
pub(super) fn apply_style(ctx: &egui::Context) {
    {
        let mut v = egui::Visuals::dark();
        v.panel_fill = ink::PANEL;
        v.window_fill = ink::RAISED;
        v.extreme_bg_color = ink::GROUND;
        v.faint_bg_color = ink::RAISED;
        v.override_text_color = Some(ink::TEXT);
        v.selection.bg_fill = ink::SELECT;
        v.selection.stroke = Stroke::new(1.0, ink::SELECT_LINE);
        v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, ink::RULE);
        v.widgets.inactive.weak_bg_fill = ink::RAISED;
        v.widgets.inactive.bg_fill = ink::RAISED;
        v.widgets.hovered.weak_bg_fill = Color32::from_rgb(46, 48, 55);
        v.widgets.active.weak_bg_fill = Color32::from_rgb(54, 57, 64);
        ctx.set_visuals(v);
        ctx.global_style_mut(|s| {
            s.spacing.item_spacing = Vec2::new(8.0, 5.0);
            s.spacing.button_padding = Vec2::new(7.0, 3.0);
        });
    }
}

impl App {
    /// The frame: input, then every pane.
    pub fn ui(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        self.style(&ctx);
        self.loader.poll();
        if let Some(drawn) = self.replay.poll() {
            // Each replay asked for is shown as it lands
            self.show_replay(&drawn.dir);
        }
        if self.replay.busy() {
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
        }
        self.keys(&ctx);

        let (shown, compare) = self.images();
        if let Some((a, _)) = shown {
            self.request(a);
        }
        if let Some(b) = compare {
            self.request(b);
        }
        // The neighbours, so a key press finds them read
        for step in [1isize, -1] {
            let next = self.model.step(self.selected, step);
            if let Some((i, _)) = self.model.shown_image(next, 0) {
                self.request(i);
            }
        }

        egui::Panel::top("frame-header")
            .frame(
                egui::Frame::new()
                    .fill(ink::GROUND)
                    .inner_margin(egui::Margin::symmetric(14, 10)),
            )
            .show(ui, |ui| self.header(ui));
        let timing_h = if self.timing.history.is_empty() {
            112.0
        } else {
            236.0
        };
        egui::Panel::bottom("frame-timing")
            .resizable(true)
            .default_size(timing_h)
            .min_size(120.0)
            .frame(
                egui::Frame::new()
                    .fill(ink::GROUND)
                    .inner_margin(egui::Margin::symmetric(14, 10)),
            )
            .show(ui, |ui| self.timing(ui));
        egui::Panel::left("frame-passes")
            .resizable(true)
            .default_size(320.0)
            .min_size(220.0)
            .frame(
                egui::Frame::new()
                    .fill(ink::PANEL)
                    .inner_margin(egui::Margin::symmetric(0, 8)),
            )
            .show(ui, |ui| self.tree(ui));
        egui::Panel::right("frame-draws")
            .resizable(true)
            .default_size(480.0)
            .min_size(300.0)
            .frame(
                egui::Frame::new()
                    .fill(ink::PANEL)
                    .inner_margin(egui::Margin::symmetric(10, 8)),
            )
            .show(ui, |ui| self.draws(ui));
        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(ink::GROUND)
                    .inner_margin(egui::Margin::symmetric(12, 8)),
            )
            .show(ui, |ui| self.centre(ui, shown, compare));

        if self.loader.pending() {
            ctx.request_repaint_after(std::time::Duration::from_millis(30));
        }
        if let Some((_, until)) = &self.toast {
            let now = ctx.input(|i| i.time);
            if now > *until {
                self.toast = None;
            } else {
                ctx.request_repaint_after(std::time::Duration::from_millis(100));
            }
        }
    }

    fn keys(&mut self, ctx: &egui::Context) {
        if ctx.egui_wants_keyboard_input() {
            return;
        }
        let pressed = |k: Key| ctx.input(|i| i.key_pressed(k));
        let shift = ctx.input(|i| i.modifiers.shift);
        let mut to = None;
        if pressed(Key::ArrowDown) || pressed(Key::J) {
            to = Some(if shift {
                self.model.step_to_image(self.selected, true)
            } else {
                self.model.step(self.selected, 1)
            });
        }
        if pressed(Key::ArrowUp) || pressed(Key::K) {
            to = Some(if shift {
                self.model.step_to_image(self.selected, false)
            } else {
                self.model.step(self.selected, -1)
            });
        }
        if pressed(Key::PageDown) {
            to = Some(self.model.step_to_image(self.selected, true));
        }
        if pressed(Key::PageUp) {
            to = Some(self.model.step_to_image(self.selected, false));
        }
        if pressed(Key::Home) {
            to = Some(0);
        }
        if pressed(Key::End) {
            to = Some(self.model.passes.len().saturating_sub(1));
        }
        if let Some(row) = to {
            self.select_pass(row);
        }
        let own = self
            .model
            .passes
            .get(self.selected)
            .map_or(0, |p| p.images.len());
        if own > 1 {
            let c = self.choice.entry(self.selected).or_insert(0);
            if pressed(Key::ArrowRight) {
                *c = (*c + 1).min(own - 1);
            }
            if pressed(Key::ArrowLeft) {
                *c = c.saturating_sub(1);
            }
        }
        if pressed(Key::F) {
            self.view.fit();
        }
        if pressed(Key::Num1) {
            let ppp = ctx.pixels_per_point();
            self.view.zoom = Some(1.0 / ppp);
            self.view.pan = Vec2::ZERO;
        }
        if pressed(Key::W) {
            self.view.wipe = match self.view.wipe {
                Wipe::Off => Wipe::Previous,
                Wipe::Previous => Wipe::Final,
                Wipe::Final => Wipe::Off,
            };
        }
        if pressed(Key::C) {
            let i = Channel::ALL
                .iter()
                .position(|c| *c == self.view.channel)
                .unwrap_or(0);
            self.view.channel = Channel::ALL[(i + 1) % Channel::ALL.len()];
        }
        if pressed(Key::Escape) {
            self.entity = None;
            self.draw = None;
            self.query.filter.clear();
            self.query.all_passes = false;
        }
        // The replay (#1089): R draws the frame again, or switches between
        // it and the capture; [ and ] step the scene pass a draw; H hides
        // the selected entity
        if self.replayable {
            if pressed(Key::R) {
                match (&self.replay.status, self.showing_replay()) {
                    (Status::Off, _) => self.replay.start(),
                    (_, true) => self.show_captured(),
                    (_, false) => {
                        if let Some(dir) = self.replay.last.as_ref().map(|l| l.dir.clone()) {
                            self.show_replay(&dir);
                        }
                    }
                }
            }
            if self.replay.status == Status::Ready {
                let pass = self.stepped_pass();
                let total = self.pass_draws(pass);
                let now = self.asked_draws(pass);
                let step = |to: usize| (to < total).then_some(to);
                if pressed(Key::OpenBracket) {
                    let c = self.stepped_to(pass, step(now.saturating_sub(1)));
                    self.replay_with(c);
                }
                if pressed(Key::CloseBracket) {
                    let c = self.stepped_to(pass, step((now + 1).min(total)));
                    self.replay_with(c);
                }
                if pressed(Key::O) {
                    let mut c = self.replay.want.clone();
                    c.outline = match (&c.outline, &self.entity) {
                        (Some(_), _) => None,
                        (None, e) => e.clone(),
                    };
                    self.replay_with(c);
                }
                if pressed(Key::H) {
                    if let Some(e) = self.entity.clone() {
                        let mut c = self.replay.want.clone();
                        match c.hidden.iter().position(|h| *h == e) {
                            Some(i) => {
                                c.hidden.remove(i);
                            }
                            None => c.hidden.push(e),
                        }
                        self.replay_with(c);
                    }
                }
            }
        }
    }

    // ── The header ──

    fn header(&mut self, ui: &mut egui::Ui) {
        let d = &self.capture.description;
        let f = &d.frame;
        ui.horizontal(|ui| {
            let title = d
                .scene
                .as_ref()
                .map(|s| {
                    if s.name.is_empty() {
                        s.path.clone()
                    } else {
                        s.name.clone()
                    }
                })
                .unwrap_or_else(|| "A frame".to_string());
            ui.label(RichText::new(title).size(19.0).strong().color(ink::TEXT));
            ui.add_space(6.0);
            if let Some(s) = &d.scene {
                ui.label(RichText::new(&s.path).color(ink::DIM));
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui
                    .button("Copy summary")
                    .on_hover_text("The text `flint frame show` prints")
                    .clicked()
                {
                    ui.ctx().copy_text(self.summary.clone());
                    let now = ui.ctx().input(|i| i.time);
                    self.toast = Some(("Copied".into(), now + 1.5));
                }
                if let Some((t, _)) = &self.toast {
                    ui.label(RichText::new(t).color(ink::SELECT_LINE));
                }
                ui.label(RichText::new(&self.shown).small().color(ink::FAINT));
            });
        });
        let mut facts: Vec<String> = Vec::new();
        if let Some(i) = f.index {
            facts.push(format!("frame {i}"));
        }
        if let Some(t) = f.game_time {
            facts.push(format!("t {t:.3} s"));
        }
        if f.size != [0, 0] {
            facts.push(format!("{}×{}", f.size[0], f.size[1]));
        }
        if f.scene_size != f.size && f.scene_size != [0, 0] {
            facts.push(format!(
                "scene {}×{} at ×{:.2}",
                f.scene_size[0], f.scene_size[1], f.render_scale
            ));
        }
        if f.msaa > 1 {
            facts.push(format!("MSAA {}×", f.msaa));
        }
        if let Some(a) = &f.adapter {
            facts.push(a.clone());
        }
        if let Some(b) = &f.backend {
            facts.push(b.clone());
        }
        if let Some(p) = &f.present_mode {
            facts.push(p.clone());
        }
        if let Some(ms) = f.gpu_frame_ms {
            facts.push(format!("GPU {ms:.2} ms"));
        }
        if f.debug_mode != "Pbr" && !f.debug_mode.is_empty() {
            facts.push(format!("debug {}", f.debug_mode));
        }
        ui.label(RichText::new(facts.join("  ·  ")).color(ink::DIM));

        let mut origin: Vec<String> = Vec::new();
        if let Some(by) = &d.written_by {
            origin.push(format!("written by {by}"));
        }
        if let Some(at) = &d.captured_at {
            origin.push(at.clone());
        }
        if let Some(e) = &d.engine {
            let mut s = format!("engine {}", e.version);
            s.push_str(&super::super::commit_words(
                e.commit.as_deref(),
                e.subdir.as_deref(),
                e.dirty,
            ));
            origin.push(s);
        }
        // A game's own checkout, when it isn't the engine's (#1290)
        if let Some(c) = d.project.as_ref().and_then(|p| p.commit.as_deref()) {
            let engine = d.engine.as_ref().and_then(|e| e.commit.as_deref());
            if engine != Some(c) {
                let short: String = c.chars().take(10).collect();
                origin.push(format!("project at {short}"));
            }
        }
        if let Some(t) = &d.trigger {
            let how = match t.kind.as_str() {
                "at" => "at a time",
                "key" => "the debug key",
                "script" => "an input script",
                "hitch" => "a hitch",
                other => other,
            };
            if t.detail.is_empty() {
                origin.push(format!("captured on {how}"));
            } else {
                origin.push(format!("captured on {how}: {}", t.detail));
            }
        }
        if !origin.is_empty() {
            ui.label(
                RichText::new(origin.join("  ·  "))
                    .small()
                    .color(ink::FAINT),
            );
        }
        let styles: Vec<String> = f
            .styles
            .iter()
            .map(|s| format!("{} ({})", s.name, s.stage))
            .collect();
        // `bloom`, `no fxaa`, `msaa 1`: a switch by its name, a setting
        // with its value
        let quality: Vec<String> = f
            .quality
            .iter()
            .map(|(k, v)| match v.as_str() {
                "true" => k.clone(),
                "false" => format!("no {k}"),
                _ => format!("{k} {v}"),
            })
            .collect();
        let replay = d.replay.as_ref();
        if !styles.is_empty() || !quality.is_empty() || replay.is_some() || !d.notes.is_empty() {
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                let chip = |ui: &mut egui::Ui, label: &str, body: String| {
                    ui.label(RichText::new(label).small().color(ink::DIM));
                    ui.label(RichText::new(body).small().color(ink::TEXT));
                    ui.add_space(10.0);
                };
                if !styles.is_empty() {
                    chip(ui, "styles", styles.join(", "));
                }
                if !quality.is_empty() {
                    chip(ui, "quality", quality.join(" · "));
                }
                if let Some(r) = replay {
                    let words = if !r.available {
                        "not written".to_string()
                    } else if r.fresh_only.is_empty() {
                        "everything as captured".to_string()
                    } else {
                        format!("draws fresh: {}", r.fresh_only.join("; "))
                    };
                    chip(ui, "replay", words);
                }
                for note in &d.notes {
                    chip(ui, "note", note.clone());
                }
            });
        }
    }

    // ── The pass tree ──

    fn tree(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.add_space(12.0);
            let ran = self.model.passes.iter().filter(|p| p.ran).count();
            ui.label(RichText::new("Passes").strong());
            ui.label(
                RichText::new(format!("{ran} of {} ran", self.model.passes.len()))
                    .small()
                    .color(ink::DIM),
            );
        });
        ui.add_space(4.0);
        let marks: HashMap<usize, (usize, usize)> = self
            .entity
            .as_ref()
            .map(|e| {
                self.model
                    .entity_passes(e)
                    .into_iter()
                    .map(|(p, n, c)| (p, (n, c)))
                    .collect()
            })
            .unwrap_or_default();
        let mut clicked = None;
        let mut layer_click: Option<Option<String>> = None;
        let scroll = std::mem::take(&mut self.scroll_tree);
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let mut panel: Option<&str> = None;
                for (i, row) in self.model.passes.iter().enumerate() {
                    if row.panel.as_deref() != panel {
                        panel = row.panel.as_deref();
                        ui.add_space(4.0);
                        ui.horizontal(|ui| {
                            ui.add_space(12.0);
                            let name = panel.map_or("Page".to_string(), |p| format!("Panel {p}"));
                            ui.label(RichText::new(name).small().strong().color(ink::DIM));
                        });
                    }
                    let selected = i == self.selected;
                    let resp = pass_row(
                        ui,
                        row,
                        selected,
                        self.model.max_gpu_ms,
                        marks.get(&i).copied(),
                    );
                    if selected && scroll {
                        resp.scroll_to_me(Some(Align::Center));
                    }
                    if resp.clicked() {
                        clicked = Some(i);
                    }
                    if selected && row.layers.len() > 1 {
                        for (layer, n) in &row.layers {
                            let on = self.query.layer.as_deref() == Some(layer.as_str());
                            let r = layer_row(ui, row.depth + 1, layer, *n, on);
                            if r.clicked() {
                                layer_click = Some(if on { None } else { Some(layer.clone()) });
                            }
                        }
                    }
                }
                ui.add_space(8.0);
            });
        if let Some(i) = clicked {
            self.select_pass(i);
        }
        if let Some(layer) = layer_click {
            self.query.layer = layer;
            self.query.all_passes = false;
        }
    }

    // ── The image ──

    fn centre(&mut self, ui: &mut egui::Ui, shown: Option<(usize, bool)>, compare: Option<usize>) {
        // The panel's own bounds: a row that overflows widens what is
        // left of the ui, never the pane
        let bounds = ui.max_rect();
        let Some(row) = self.model.passes.get(self.selected).cloned() else {
            centred(ui, ui.max_rect(), "This capture has no passes.", ink::DIM);
            return;
        };
        // The pass, and what it touched
        ui.horizontal(|ui| {
            let mut about = format!(
                "pass {:02} (its image files are targets/{:02}-...)",
                row.index, row.index
            );
            if !row.render_passes.is_empty() {
                about.push_str(&format!(
                    "\nrender passes: {}",
                    row.render_passes.join(", ")
                ));
            }
            if !row.also.is_empty() {
                about.push_str(&format!("\nalso: {}", row.also.join("; ")));
            }
            ui.label(RichText::new(&row.key).size(16.0).strong())
                .on_hover_text(about);
            if let Some(ms) = row.gpu_ms {
                ui.label(RichText::new(format!("{ms:.3} ms")).color(ink::GPU));
            } else if row.ran && !row.timed {
                ui.label(RichText::new("untimed").color(ink::FAINT));
            }
            if !row.writes.is_empty() {
                ui.label(
                    RichText::new(format!("writes {}", row.writes.join(", ")))
                        .small()
                        .color(ink::DIM),
                );
            }
            if !row.reads.is_empty() {
                ui.label(
                    RichText::new(format!("reads {}", row.reads.join(", ")))
                        .small()
                        .color(ink::FAINT),
                );
            }
        });
        let a_loaded = shown.and_then(|(a, _)| self.loader.ready(a));
        // Its images
        if row.images.len() > 1 {
            ui.horizontal_wrapped(|ui| {
                let c = self.choice.get(&self.selected).copied().unwrap_or(0);
                for (n, &im) in row.images.iter().enumerate() {
                    let label = self.model.images[im].label();
                    if ui
                        .selectable_label(n == c, label)
                        .on_hover_text("Left and right step through them")
                        .clicked()
                    {
                        self.choice.insert(self.selected, n);
                    }
                }
            });
        }
        self.controls(ui, a_loaded.as_deref());

        let readout_h = 22.0;
        let top = ui.available_rect_before_wrap().top();
        let avail = Rect::from_min_max(Pos2::new(bounds.left(), top), bounds.max);
        let pane = Rect::from_min_max(
            avail.min,
            Pos2::new(avail.max.x, avail.max.y - readout_h - 4.0),
        );
        ui.painter().rect_filled(pane, 4.0, ink::GROUND);

        let Some((a, own)) = shown else {
            let why = if self.model.images.is_empty() {
                "This capture holds no images: it is a frame's description \
                 (`flint render --frame-dump`). `--frame-capture` writes each pass's image."
                    .to_string()
            } else {
                format!("Nothing has been drawn yet at {}.", row.key)
            };
            centred(ui, pane, &why, ink::DIM);
            ui.allocate_rect(avail, Sense::hover());
            return;
        };
        let size = self.model.images[a].image.size;
        if size[0] > self.max_side || size[1] > self.max_side {
            let words = format!(
                "This image is {}×{}, past the {} a side this GPU takes. \
                 `flint frame image` copies it out as a PNG.",
                size[0], size[1], self.max_side
            );
            centred(ui, pane, &words, ink::DIM);
            ui.allocate_rect(avail, Sense::hover());
            return;
        }
        let a_slot = self.loader.get(a).cloned();
        let hover = match a_slot {
            Some(Slot::Ready(la)) => {
                let a_side = Side {
                    key: self.key_base + a,
                    display: self.view.display(&la),
                    loaded: &la,
                };
                let lb = compare.and_then(|b| self.loader.ready(b));
                let b_side = match (&lb, compare) {
                    (Some(lb), Some(b)) => Some(Side {
                        key: self.key_base + b,
                        display: self.view.display(lb),
                        loaded: lb,
                    }),
                    _ => None,
                };
                let hover =
                    viewer::show(ui, pane, &mut self.view, a_side, b_side, self.srgb_target);
                // What isn't this pass's own, and the wipe's two names
                let painter = ui.painter_at(pane);
                if !own || row.skipped.is_some() {
                    let from = &self.model.passes[self.model.images[a].pass].key;
                    let words = match &row.skipped {
                        Some(why) => format!("skipped: {why} · showing {from}'s image"),
                        None => format!("wrote no image · showing {from}'s"),
                    };
                    tag(
                        &painter,
                        pane.left_top() + Vec2::new(10.0, 10.0),
                        &words,
                        ink::DIM,
                    );
                }
                if let (Some(b), Some(_)) = (compare, &lb) {
                    let split_x = pane.left() + self.view.split * pane.width();
                    let bname = format!(
                        "{} · {}",
                        self.model.passes[self.model.images[b].pass].key,
                        self.model.images[b].label()
                    );
                    let aname = format!("{} · {}", row.key, self.model.images[a].label());
                    tag_ending_at(
                        &painter,
                        Pos2::new(split_x - 12.0, pane.bottom() - 30.0),
                        &bname,
                        ink::TEXT,
                    );
                    tag(
                        &painter,
                        Pos2::new(split_x + 12.0, pane.bottom() - 30.0),
                        &aname,
                        ink::TEXT,
                    );
                } else if compare.is_some() {
                    tag(
                        &painter,
                        pane.left_bottom() + Vec2::new(10.0, -34.0),
                        "reading the other image…",
                        ink::DIM,
                    );
                }
                hover.map(|h| (h, la))
            }
            Some(Slot::Failed(why)) => {
                centred(
                    ui,
                    pane,
                    &format!("Couldn't read this image: {why}"),
                    ink::HITCH,
                );
                None
            }
            _ => {
                centred(ui, pane, "Reading…", ink::DIM);
                None
            }
        };
        // The readout
        let line = Rect::from_min_max(Pos2::new(avail.min.x, pane.max.y + 4.0), avail.max);
        ui.allocate_rect(avail, Sense::hover());
        let im = &self.model.images[a];
        let mut text = String::new();
        if let Some((h, la)) = &hover {
            let names = ["R", "G", "B", "A"];
            let p = &la.prepared;
            let vals: Vec<String> = h
                .values
                .iter()
                .enumerate()
                .map(|(k, v)| {
                    let name = if p.kind == super::super::read::Kind::Depth {
                        "Z"
                    } else {
                        names[k.min(3)]
                    };
                    if p.integer || p.scale > 1.0 {
                        format!("{name} {v:.0}")
                    } else {
                        format!("{name} {v:.4}")
                    }
                })
                .collect();
            text = format!(
                "{}{}, {}    {}",
                if h.other { "B  " } else { "" },
                h.texel.0,
                h.texel.1,
                vals.join("  ")
            );
            let swatch = Rect::from_min_size(
                Pos2::new(line.left(), line.center().y - 6.0),
                Vec2::splat(12.0),
            );
            let c = h.shown.map(|v| (v * 255.0 + 0.5) as u8);
            ui.painter()
                .rect_filled(swatch, 2.0, Color32::from_rgb(c[0], c[1], c[2]));
        }
        let info = {
            let i = &im.image;
            let mut s = format!("{}×{}  {}  {}", i.size[0], i.size[1], i.format, i.stored);
            if let Some(l) = self.loader.ready(a) {
                let d = self.view.display(&l);
                let untouched =
                    self.view.mode.is_none() && self.view.ev == 0.0 && self.view.depth.is_none();
                s.push_str(&format!("  ·  {}", describe(&d, &l.words, untouched)));
            }
            s
        };
        let painter = ui.painter();
        if text.is_empty() {
            text = "point at the image to read a texel; scroll zooms, drag pans".into();
            painter.text(
                Pos2::new(line.left(), line.center().y),
                egui::Align2::LEFT_CENTER,
                text,
                FontId::proportional(12.0),
                ink::FAINT,
            );
        } else {
            painter.text(
                Pos2::new(line.left() + 18.0, line.center().y),
                egui::Align2::LEFT_CENTER,
                text,
                FontId::monospace(12.5),
                ink::TEXT,
            );
        }
        let corner = ui.painter_at(pane);
        tag_ending_at(
            &corner,
            pane.right_top() + Vec2::new(-10.0, 10.0),
            &info,
            ink::DIM,
        );
    }

    fn controls(&mut self, ui: &mut egui::Ui, loaded: Option<&super::loader::Loaded>) {
        ui.horizontal_wrapped(|ui| {
            let auto = loaded.map(|l| l.auto.mode);
            let current = self.view.mode.or(auto).unwrap_or(Mode::Stored);
            let label = match self.view.mode {
                None => format!("Auto: {}", current.label()),
                Some(m) => m.label().to_string(),
            };
            egui::ComboBox::from_id_salt("frame-mode")
                .selected_text(label)
                .width(132.0)
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.view.mode, None, "Auto");
                    for m in Mode::ALL {
                        ui.selectable_value(&mut self.view.mode, Some(m), m.label());
                    }
                });
            ui.add_space(4.0);
            for c in Channel::ALL {
                if ui
                    .selectable_label(self.view.channel == c, c.label())
                    .on_hover_text("C cycles")
                    .clicked()
                {
                    self.view.channel = c;
                }
            }
            ui.add_space(8.0);
            if current.exposed() {
                ui.label(RichText::new(format!("EV {:+.2}", self.view.ev)).color(ink::DIM))
                    .on_hover_text("Stops over the frame's exposure");
                ui.spacing_mut().slider_width = 90.0;
                ui.add(
                    egui::Slider::new(&mut self.view.ev, -8.0..=8.0)
                        .step_by(0.25)
                        .show_value(false),
                );
                if self.view.ev != 0.0
                    && ui
                        .small_button("0")
                        .on_hover_text("The frame's exposure")
                        .clicked()
                {
                    self.view.ev = 0.0;
                }
            }
            if current == Mode::Depth {
                if let Some(l) = loaded {
                    let (mut lo, mut hi) = self.view.depth.unwrap_or((l.auto.lo, l.auto.hi));
                    ui.label(RichText::new("depth").color(ink::DIM));
                    let a = ui.add(
                        egui::DragValue::new(&mut lo)
                            .speed(0.0005)
                            .range(0.0..=1.0)
                            .max_decimals(5),
                    );
                    let b = ui.add(
                        egui::DragValue::new(&mut hi)
                            .speed(0.0005)
                            .range(0.0..=1.0)
                            .max_decimals(5),
                    );
                    if a.changed() || b.changed() {
                        self.view.depth = Some((lo, hi));
                    }
                    if self.view.depth.is_some()
                        && ui
                            .small_button("drawn")
                            .on_hover_text("The depths the frame drew")
                            .clicked()
                    {
                        self.view.depth = None;
                    }
                }
            }
            ui.add_space(14.0);
            self.view_buttons(ui, loaded);
        });
    }

    /// The wipe and the zoom.
    fn view_buttons(&mut self, ui: &mut egui::Ui, loaded: Option<&super::loader::Loaded>) {
        let wipe = match self.view.wipe {
            Wipe::Off => "A/B off",
            Wipe::Previous => "A/B: previous pass",
            Wipe::Final => "A/B: finished frame",
        };
        egui::ComboBox::from_id_salt("frame-wipe")
            .selected_text(wipe)
            .width(138.0)
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut self.view.wipe, Wipe::Off, "Off");
                ui.selectable_value(
                    &mut self.view.wipe,
                    Wipe::Previous,
                    "The previous pass's image",
                );
                ui.selectable_value(&mut self.view.wipe, Wipe::Final, "The finished frame");
            })
            .response
            .on_hover_text("W cycles; drag the line to wipe");
        ui.add_space(6.0);
        if ui
            .button("Fit")
            .on_hover_text("F, or double-click")
            .clicked()
        {
            self.view.fit();
        }
        let ppp = ui.ctx().pixels_per_point();
        if ui.button("1:1").on_hover_text("1").clicked() {
            self.view.zoom = Some(1.0 / ppp);
            self.view.pan = Vec2::ZERO;
        }
        if let (Some(z), Some(_)) = (self.view.zoom, loaded) {
            ui.label(RichText::new(format!("{:.0}%", z * ppp * 100.0)).color(ink::DIM));
        }
    }

    // ── The draws ──

    /// The captured frame's description, whichever is on screen.
    fn captured_description(&self) -> &flint_render::frame_capture::FrameDescription {
        match &self.captured {
            Some((c, _)) => &c.description,
            None => &self.capture.description,
        }
    }

    /// The replay's strip, above the draws (#1089): what it found, how close
    /// it came, and its controls.
    fn replay_ui(&mut self, ui: &mut egui::Ui) {
        let status = self.replay.status.clone();
        ui.horizontal(|ui| {
            ui.label(RichText::new("Replay").strong());
            match &status {
                Status::Off => {
                    if ui
                        .button("Draw this frame again")
                        .on_hover_text(
                            "Load the scene and draw the frame from its replay inputs (R),                              then step the scene pass, hide entities, switch passes off",
                        )
                        .clicked()
                    {
                        self.replay.start();
                    }
                }
                Status::Loading => {
                    ui.spinner();
                    ui.label(RichText::new("loading the scene").color(ink::DIM));
                }
                Status::Failed(_) => {
                    ui.label(RichText::new("couldn't replay").color(ink::HITCH));
                }
                Status::Ready => {
                    let showing = self.showing_replay();
                    if ui
                        .selectable_label(showing, "replayed")
                        .on_hover_text("R switches")
                        .clicked()
                        && !showing
                    {
                        if let Some(dir) = self.replay.last.as_ref().map(|l| l.dir.clone()) {
                            self.show_replay(&dir);
                        }
                    }
                    if ui
                        .selectable_label(!showing, "captured")
                        .on_hover_text("R switches")
                        .clicked()
                        && showing
                    {
                        self.show_captured();
                    }
                    if self.replay.busy() {
                        ui.spinner();
                    }
                }
            }
        });
        if let Status::Failed(e) = &status {
            ui.label(RichText::new(e).small().color(ink::HITCH));
            return;
        }
        if status != Status::Ready {
            return;
        }

        // How close it came
        let fresh = self
            .replay
            .loaded
            .as_ref()
            .map(|l| l.fresh_only.clone())
            .unwrap_or_default();
        if let Some(last) = &self.replay.last {
            let asked = last.controls != Controls::default();
            let mut all = true;
            for m in &last.matches {
                all &= m.identical();
                let colour = if m.identical() {
                    ink::SELECT_LINE
                } else if asked {
                    ink::DIM
                } else {
                    ink::ENTITY
                };
                let what = match m.image {
                    "final" => "final image".to_string(),
                    "scene" => "scene pass ".to_string(),
                    _ => last.controls.stepped_name().to_lowercase(),
                };
                ui.label(
                    RichText::new(format!("{what}  {}", m.words()))
                        .small()
                        .color(colour),
                );
            }
            if !all && !asked && !fresh.is_empty() {
                ui.label(
                    RichText::new(format!("drawn fresh: {}", fresh.join("; ")))
                        .small()
                        .color(ink::DIM),
                );
            }
            for r in &last.refused {
                ui.label(RichText::new(r).small().color(ink::HITCH));
            }
        }

        // The selected pass (the scene's unless another that can be is
        // selected), a draw at a time (#1204)
        let pass = self.stepped_pass();
        let name = pass.name();
        let total = self.pass_draws(pass);
        if total > 0 {
            let mut k = self.asked_draws(pass);
            let mut moved = false;
            let label = match pass {
                flint_render::SteppedPass::Scene => "scene draws".to_string(),
                flint_render::SteppedPass::Shadow(_)
                | flint_render::SteppedPass::StyleMask
                | flint_render::SteppedPass::StyleAlbedo => name.to_lowercase(),
            };
            ui.horizontal(|ui| {
                ui.label(RichText::new(label).small().color(ink::DIM));
                let w = (ui.available_width() - 70.0).max(60.0);
                ui.spacing_mut().slider_width = w;
                moved = ui
                    .add(egui::Slider::new(&mut k, 0..=total).show_value(false))
                    .on_hover_text(
                        "[ and ] step a draw; select a shadow layer or a style pass to step it",
                    )
                    .changed();
                ui.label(RichText::new(format!("{k} / {total}")).monospace());
            });
            if moved {
                let c = self.stepped_to(pass, (k < total).then_some(k));
                self.replay_with(c);
                // What the slider builds is the stepped pass's image
                if let Some(row) = self.model.find_pass(name) {
                    self.select_pass(row);
                }
            }
            let at = self.asked_draws(pass);
            // The replay's own last draw (hiding moves the count), else the
            // captured frame's at that place
            let drawn_here = self
                .showing_replay()
                .then(|| self.capture.description.pass(name))
                .flatten()
                .filter(|p| p.draws.len() == at)
                .and_then(|p| p.draws.last());
            let last_drawn = (at > 0)
                .then(|| {
                    drawn_here.or_else(|| {
                        self.captured_description()
                            .pass(name)
                            .and_then(|p| p.draws.get(at - 1))
                    })
                })
                .flatten()
                .map(|d| {
                    let who = d.entity.clone().unwrap_or_else(|| d.kind.clone());
                    format!("draw {at}: {who} · {} · {}", d.kind, d.layer)
                });
            let nothing = match pass {
                flint_render::SteppedPass::Scene => {
                    "nothing drawn yet: the sky and what isn't a draw"
                }
                flint_render::SteppedPass::Shadow(_) => "no caster drawn yet: the layer cleared",
                flint_render::SteppedPass::StyleMask => "no styled draw yet: the mask cleared",
                flint_render::SteppedPass::StyleAlbedo => "no mesh drawn yet: the albedo cleared",
            };
            let words = match last_drawn {
                Some(w) if at < total => w,
                _ if at == 0 => nothing.to_string(),
                _ => "every draw".to_string(),
            };
            ui.label(RichText::new(words).small().color(ink::TEXT));
        }

        // Passes off
        let skippable = self
            .replay
            .loaded
            .as_ref()
            .map(|l| l.skippable.clone())
            .unwrap_or_default();
        if !skippable.is_empty() {
            let mut change = None;
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new("passes").small().color(ink::DIM));
                for s in &skippable {
                    let mut on = !self.replay.want.skip.contains(&s.pass);
                    if ui
                        .checkbox(&mut on, RichText::new(&s.pass).small())
                        .on_hover_text(format!("Off: {}", s.does))
                        .changed()
                    {
                        change = Some((s.pass.clone(), on));
                    }
                }
            });
            if let Some((pass, on)) = change {
                let mut c = self.replay.want.clone();
                c.skip.retain(|p| *p != pass);
                if !on {
                    c.skip.push(pass);
                }
                self.replay_with(c);
            }
        }

        // Entities hidden
        let mut change = None;
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new("hidden").small().color(ink::DIM));
            if self.replay.want.hidden.is_empty() {
                ui.label(RichText::new("none").small().color(ink::FAINT));
            }
            for h in &self.replay.want.hidden {
                if ui
                    .small_button(RichText::new(format!("{h}  x")).color(ink::ENTITY))
                    .on_hover_text("Draw it again")
                    .clicked()
                {
                    change = Some((h.clone(), false));
                }
            }
            if let Some(e) = &self.entity {
                if !self.replay.want.hidden.contains(e)
                    && ui
                        .small_button(format!("hide {e}"))
                        .on_hover_text("Draw the frame without it (H)")
                        .clicked()
                {
                    change = Some((e.clone(), true));
                }
            }
        });
        // Its pixels marked
        let mut outline = None;
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new("outlined").small().color(ink::DIM));
            match (&self.replay.want.outline, &self.entity) {
                (Some(o), _) => {
                    if ui
                        .small_button(RichText::new(format!("{o}  x")).color(ink::ENTITY))
                        .on_hover_text("Take the outline off (O)")
                        .clicked()
                    {
                        outline = Some(None);
                    }
                }
                (None, Some(e)) => {
                    if ui
                        .small_button(format!("outline {e}"))
                        .on_hover_text("Mark its pixels with the selection outline (O)")
                        .clicked()
                    {
                        outline = Some(Some(e.clone()));
                    }
                }
                (None, None) => {
                    ui.label(
                        RichText::new("select a draw's entity")
                            .small()
                            .color(ink::FAINT),
                    );
                }
            }
        });
        if let Some(o) = outline {
            let mut c = self.replay.want.clone();
            c.outline = o;
            self.replay_with(c);
        }
        if let Some((name, hide)) = change {
            let mut c = self.replay.want.clone();
            c.hidden.retain(|h| *h != name);
            if hide {
                c.hidden.push(name);
            }
            self.replay_with(c);
        }

        // What the load found
        if let Some(loaded) = &self.replay.loaded {
            for a in loaded.assets.iter().filter(|a| !a.same_bytes()) {
                let words = match a.state {
                    "missing" => format!("{} {} is missing (was {})", a.kind, a.name, a.path),
                    _ => format!("{} {} has changed since the capture", a.kind, a.name),
                };
                ui.label(RichText::new(words).small().color(ink::HITCH));
            }
            if !loaded.notes.is_empty() {
                egui::CollapsingHeader::new(
                    RichText::new(format!("notes ({})", loaded.notes.len()))
                        .small()
                        .color(ink::DIM),
                )
                .id_salt("replay-notes")
                .show(ui, |ui| {
                    for n in &loaded.notes {
                        ui.label(RichText::new(n).small().color(ink::DIM));
                    }
                });
            }
        }
    }

    fn draws(&mut self, ui: &mut egui::Ui) {
        if self.replayable {
            self.replay_ui(ui);
            ui.separator();
        }
        self.query.pass = Some(self.selected);
        ui.horizontal(|ui| {
            ui.label(RichText::new("Draws").strong());
            let filter = ui.add(
                egui::TextEdit::singleline(&mut self.query.filter)
                    .hint_text("entity, kind, pipeline, mesh  (tree_*)")
                    .desired_width(ui.available_width() - 4.0),
            );
            if filter.changed() && !self.query.filter.is_empty() {
                self.query.all_passes = true;
            }
        });
        ui.horizontal(|ui| {
            let this = !self.query.all_passes;
            let pass = self
                .model
                .passes
                .get(self.selected)
                .map_or("", |p| p.key.as_str());
            if ui
                .selectable_label(this, format!("this pass ({pass})"))
                .clicked()
            {
                self.query.all_passes = false;
            }
            if ui.selectable_label(!this, "every pass").clicked() {
                self.query.all_passes = true;
            }
            ui.checkbox(&mut self.query.show_culled, "culled");
            if let Some(layer) = self.query.layer.clone() {
                if ui
                    .small_button(format!("{layer}  x"))
                    .on_hover_text("Every layer's draws")
                    .clicked()
                {
                    self.query.layer = None;
                }
            }
        });
        let rows = self.model.table(&self.query);
        let all = self.query.all_passes;
        let details_h = if self.draw.is_some() { 270.0 } else { 0.0 };
        let table_h = (ui.available_height() - details_h).max(80.0);
        let cols = columns(ui.available_width(), all, self.query.show_culled);

        // The header, clickable to sort
        let (head, _) =
            ui.allocate_exact_size(Vec2::new(ui.available_width(), 20.0), Sense::hover());
        let painter = ui.painter_at(head);
        painter.line_segment(
            [head.left_bottom(), head.right_bottom()],
            Stroke::new(1.0, ink::RULE),
        );
        for col in &cols {
            let r = Rect::from_min_size(
                Pos2::new(head.left() + col.x, head.top()),
                Vec2::new(col.w, head.height()),
            );
            let id = ui.id().with(("sort", col.name));
            let resp = ui.interact(r, id, Sense::click());
            let on = self.query.sort == col.sort;
            let colour = if on || resp.hovered() {
                ink::TEXT
            } else {
                ink::DIM
            };
            let g =
                painter.layout_no_wrap(col.name.to_string(), FontId::proportional(12.0), colour);
            let gx = if col.right {
                r.right() - 8.0 - g.size().x
            } else {
                r.left() + 2.0
            };
            let gw = g.size().x;
            painter.galley(Pos2::new(gx, r.center().y - g.size().y / 2.0), g, colour);
            if on {
                // The sort's direction, drawn: the font has no arrows
                let c = Pos2::new(gx + gw + 7.0, r.center().y);
                let (tip, base) = if self.query.descending {
                    (-3.0, 2.0)
                } else {
                    (3.0, -2.0)
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
                if on {
                    self.query.descending = !self.query.descending;
                } else {
                    self.query.sort = col.sort;
                    self.query.descending = false;
                }
            }
        }

        let row_h = 20.0;
        let mut clicked = None;
        let entity = self.entity.clone();
        egui::ScrollArea::vertical()
            .id_salt("draw-table")
            .max_height(table_h)
            .auto_shrink([false, false])
            .show_rows(ui, row_h, rows.len().max(1), |ui, range| {
                if rows.is_empty() {
                    let msg = if self.query.filter.is_empty() {
                        "No draws in this pass."
                    } else {
                        "No draw matches."
                    };
                    ui.label(RichText::new(msg).color(ink::DIM));
                    return;
                }
                for &i in &rows[range] {
                    let d = &self.model.draws[i];
                    let (r, resp) = ui.allocate_exact_size(
                        Vec2::new(ui.available_width(), row_h),
                        Sense::click(),
                    );
                    let painter = ui.painter_at(r);
                    let same = entity.is_some() && d.draw.entity == entity;
                    if self.draw == Some(i) {
                        painter.rect_filled(r, 2.0, ink::SELECT);
                    } else if same {
                        painter.rect_filled(
                            r,
                            2.0,
                            Color32::from_rgba_unmultiplied(232, 196, 110, 24),
                        );
                    } else if resp.hovered() {
                        painter.rect_filled(r, 2.0, ink::RAISED);
                    }
                    if same {
                        painter.rect_filled(
                            Rect::from_min_size(r.left_top(), Vec2::new(2.0, r.height())),
                            0.0,
                            ink::ENTITY,
                        );
                    }
                    let colour = if d.culled() { ink::FAINT } else { ink::TEXT };
                    let tris = d.draw.triangles as u64 * d.draw.instances.max(1) as u64;
                    let pass = &self.model.passes[d.pass].key;
                    for col in &cols {
                        let text = match col.sort {
                            Sort::Order => pass.clone(),
                            Sort::Entity => d.entity_label(),
                            Sort::Kind => d.draw.kind.clone(),
                            Sort::Pipeline => d.draw.pipeline.clone(),
                            Sort::Triangles => {
                                if d.draw.instances > 1 {
                                    format!("{} ×{}", d.draw.triangles, d.draw.instances)
                                } else {
                                    tris.to_string()
                                }
                            }
                            Sort::Blend => d.draw.blend.clone(),
                            Sort::Culled => d.draw.culled.clone().unwrap_or_default(),
                        };
                        let pad = if col.right { 8.0 } else { 6.0 };
                        let cell = Rect::from_min_size(
                            Pos2::new(r.left() + col.x, r.top()),
                            Vec2::new(col.w - pad, r.height()),
                        );
                        let font = if col.right {
                            FontId::monospace(11.5)
                        } else {
                            FontId::proportional(12.5)
                        };
                        let galley = elided(&painter, text, font, colour, cell.width() - 2.0);
                        let pos = if col.right {
                            Pos2::new(
                                cell.right() - galley.size().x,
                                cell.center().y - galley.size().y / 2.0,
                            )
                        } else {
                            Pos2::new(cell.left() + 2.0, cell.center().y - galley.size().y / 2.0)
                        };
                        painter.galley(pos, galley, colour);
                    }
                    if resp.clicked() {
                        clicked = Some(i);
                    }
                }
            });
        if let Some(i) = clicked {
            self.draw = Some(i);
            self.entity = self.model.draws[i].draw.entity.clone();
        }
        if let Some(i) = self.draw {
            ui.add_space(6.0);
            ui.separator();
            self.details(ui, i);
        }
    }

    fn details(&mut self, ui: &mut egui::Ui, i: usize) {
        let d = self.model.draws[i].clone();
        let pass = self.model.passes[d.pass].key.clone();
        ui.horizontal(|ui| {
            ui.label(RichText::new(d.entity_label()).strong().color(ink::ENTITY));
            if let Some(id) = d.draw.entity_id {
                ui.label(RichText::new(format!("id {id}")).small().color(ink::FAINT));
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.small_button("x").on_hover_text("Esc").clicked() {
                    self.draw = None;
                    self.entity = None;
                }
            });
        });
        let mut jump = None;
        egui::ScrollArea::vertical()
            .id_salt("draw-details")
            .auto_shrink([false, true])
            .show(ui, |ui| {
                ui.columns(2, |cols| {
                    let w = &d.draw;
                    let mut facts: Vec<(&str, String)> = vec![
                        ("pass", pass.clone()),
                        ("layer", w.layer.clone()),
                        ("kind", w.kind.clone()),
                        ("pipeline", w.pipeline.clone()),
                        ("blend", w.blend.clone()),
                        ("vertex", w.vertex.clone()),
                        (
                            "triangles",
                            if w.instances > 1 {
                                format!("{} × {} instances", w.triangles, w.instances)
                            } else {
                                w.triangles.to_string()
                            },
                        ),
                    ];
                    if let Some(m) = &w.mesh {
                        facts.push(("mesh", m.clone()));
                    }
                    if let Some(m) = &w.material {
                        let id = w.material_id.map_or(String::new(), |n| format!(" (#{n})"));
                        facts.push(("material", format!("{m}{id}")));
                    }
                    let mut flags = Vec::new();
                    if w.casts_shadow {
                        flags.push("casts shadow");
                    }
                    if w.double_sided {
                        flags.push("double-sided");
                    }
                    if !flags.is_empty() {
                        facts.push(("", flags.join(", ")));
                    }
                    if let Some(c) = &w.culled {
                        facts.push(("culled", c.clone()));
                    }
                    egui::Grid::new("draw-facts")
                        .num_columns(2)
                        .spacing([10.0, 3.0])
                        .show(&mut cols[0], |ui| {
                            for (k, v) in facts {
                                ui.label(RichText::new(k).size(12.0).color(ink::DIM));
                                ui.label(RichText::new(v).size(12.0));
                                ui.end_row();
                            }
                        });
                    let ui = &mut cols[1];
                    ui.label(RichText::new("in every pass").size(12.0).color(ink::DIM));
                    if let Some(e) = &w.entity {
                        egui::ScrollArea::vertical()
                            .id_salt("entity-passes")
                            .max_height(190.0)
                            .show(ui, |ui| {
                                for (p, n, c) in self.model.entity_passes(e) {
                                    let key = &self.model.passes[p].key;
                                    let mut s = format!("{key}  {n}");
                                    if c > 0 {
                                        s.push_str(&format!("  ({c} culled)"));
                                    }
                                    if ui
                                        .selectable_label(
                                            p == self.selected,
                                            RichText::new(s).size(12.0),
                                        )
                                        .clicked()
                                    {
                                        jump = Some(p);
                                    }
                                }
                            });
                    } else {
                        ui.label(
                            RichText::new("a draw of no entity")
                                .small()
                                .color(ink::FAINT),
                        );
                    }
                })
            });
        if let Some(p) = jump {
            self.select_pass(p);
        }
    }

    // ── Time ──

    fn timing(&mut self, ui: &mut egui::Ui) {
        let label_w = 92.0;
        // A copy: a click on a GPU segment selects its pass
        let timing = self.timing.clone();
        let t = &timing;
        // CPU
        ui.horizontal(|ui| {
            ui.allocate_ui(Vec2::new(label_w, 18.0), |ui| {
                ui.label(RichText::new("CPU").strong());
                if let Some(ms) = t.cpu_ms {
                    ui.label(RichText::new(format!("{ms:.2} ms")).small().color(ink::DIM));
                }
                if t.cpu_of_captured {
                    ui.label(RichText::new("as captured").small().color(ink::FAINT));
                }
            });
            if t.cpu.is_empty() {
                ui.label(
                    RichText::new(t.cpu_missing.clone().unwrap_or_default()).color(ink::FAINT),
                );
            } else {
                let total = t
                    .cpu_ms
                    .unwrap_or(0.0)
                    .max(t.cpu.iter().map(|s| s.ms).sum());
                let segs: Vec<(String, f32, Color32, Option<usize>)> = t
                    .cpu
                    .iter()
                    .map(|s| (s.label.clone(), s.ms, band_of(&s.label).color(), None))
                    .collect();
                bar(ui, &segs, total, None);
            }
        });
        if !t.scripts.is_empty() {
            ui.horizontal(|ui| {
                ui.add_space(label_w + 8.0);
                let words: Vec<String> = t
                    .scripts
                    .iter()
                    .take(6)
                    .map(|s| format!("{} {:.2} ms", s.label, s.ms))
                    .collect();
                ui.label(
                    RichText::new("scripts")
                        .small()
                        .color(Band::Scripts.color()),
                );
                ui.label(RichText::new(words.join("  ·  ")).small().color(ink::DIM));
            });
        }
        // GPU
        let mut clicked = None;
        ui.horizontal(|ui| {
            ui.allocate_ui(Vec2::new(label_w, 18.0), |ui| {
                ui.label(RichText::new("GPU").strong());
                if let Some(ms) = t.gpu_ms {
                    ui.label(RichText::new(format!("{ms:.3} ms")).small().color(ink::DIM));
                }
            });
            if t.gpu.is_empty() {
                ui.label(
                    RichText::new(
                        "no GPU times: the adapter had no timestamp queries, or the frame wasn't timed",
                    )
                    .color(ink::FAINT),
                );
            } else {
                let total = t.gpu_ms.unwrap_or(0.0).max(t.gpu.iter().map(|s| s.ms).sum());
                let segs: Vec<(String, f32, Color32, Option<usize>)> = t
                    .gpu
                    .iter()
                    .enumerate()
                    .map(|(n, s)| (s.label.clone(), s.ms, if n % 2 == 0 { ink::GPU } else { ink::GPU_ALT }, s.pass))
                    .collect();
                clicked = bar(ui, &segs, total, Some(self.selected));
            }
        });
        if let Some(p) = clicked {
            self.select_pass(p);
        }
        // The history
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.allocate_ui(Vec2::new(label_w, 18.0), |ui| {
                ui.label(RichText::new("History").strong());
                if !t.history.is_empty() {
                    ui.label(
                        RichText::new(format!("{} frames", t.history.len()))
                            .small()
                            .color(ink::DIM),
                    );
                }
                if t.cpu_of_captured && !t.history.is_empty() {
                    ui.label(RichText::new("as captured").small().color(ink::FAINT));
                }
            });
            if t.history.is_empty() {
                ui.label(RichText::new(t.history_missing.clone()).color(ink::FAINT));
                return;
            }
            let h = (ui.available_height() - 22.0).clamp(48.0, 140.0);
            let secs = t.history_secs().max(0.001);
            let layout = GraphLayout {
                size: Vec2::new(ui.available_width(), h),
                window_secs: secs,
                budget_ms: 1000.0 / 60.0,
                fps_labels: true,
            };
            let resp = self.graph.show(ui, &t.history, layout);
            if let Some(c) = t.captured.and_then(|c| t.history.get(c)) {
                let r = resp.rect;
                let x0 = r.left() + (c.at as f32 / secs) * r.width();
                let x1 = r.left() + (c.end() as f32 / secs) * r.width();
                let mark = Rect::from_min_max(
                    Pos2::new(x0 - 1.5, r.top()),
                    Pos2::new(x1.max(x0 + 2.0) + 1.5, r.bottom()),
                );
                ui.painter().rect_stroke(
                    mark,
                    1.0,
                    Stroke::new(1.5, ink::SELECT_LINE),
                    egui::StrokeKind::Outside,
                );
                tag_ending_at(
                    ui.painter(),
                    Pos2::new(mark.left() - 4.0, r.top() + 2.0),
                    "captured",
                    ink::SELECT_LINE,
                );
            }
        });
        if let Some(h) = &t.hitch {
            ui.horizontal(|ui| {
                ui.add_space(label_w + 8.0);
                ui.label(RichText::new(h).small().color(ink::HITCH));
            });
        }
    }
}

/// A pass's row of the tree.
fn pass_row(
    ui: &mut egui::Ui,
    row: &super::model::PassRow,
    selected: bool,
    max_ms: f32,
    mark: Option<(usize, usize)>,
) -> egui::Response {
    let h = 24.0;
    let (r, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), h), Sense::click());
    let painter = ui.painter_at(r);
    if selected {
        painter.rect_filled(r, 0.0, ink::SELECT);
        painter.rect_filled(
            Rect::from_min_size(r.left_top(), Vec2::new(3.0, h)),
            0.0,
            ink::SELECT_LINE,
        );
    } else if resp.hovered() {
        painter.rect_filled(r, 0.0, ink::RAISED);
    }
    let x = r.left() + 14.0 + row.depth as f32 * 14.0;
    let y = r.center().y;
    // The selected entity's thread: gold where it drew, grey where only
    // culled
    if let Some((n, _)) = mark {
        painter.circle_filled(
            Pos2::new(r.left() + 8.0, y),
            3.0,
            if n > 0 { ink::ENTITY } else { ink::FAINT },
        );
    }
    let (name_colour, italic) = if row.ran {
        (ink::TEXT, false)
    } else {
        (ink::FAINT, true)
    };
    let mut name = RichText::new(&row.key).size(13.0).color(name_colour);
    if italic {
        name = name.italics();
    }
    let galley = egui::WidgetText::from(name).into_galley(
        ui,
        Some(egui::TextWrapMode::Truncate),
        r.width() * 0.55,
        egui::TextStyle::Body,
    );
    let name_w = galley.size().x;
    painter.galley(Pos2::new(x, y - galley.size().y / 2.0), galley, name_colour);

    // Right side: draws, then ms over a thin bar
    let right = r.right() - 10.0;
    if let Some(why) = &row.skipped {
        let room = (right - (x + name_w + 12.0)).max(0.0);
        let g = elided(
            &painter,
            why.clone(),
            FontId::proportional(11.0),
            ink::FAINT,
            room,
        );
        let at = Pos2::new(right - g.size().x, y - g.size().y / 2.0);
        painter.galley(at, g, ink::FAINT);
        return resp.on_hover_text(format!("skipped: {why}"));
    }
    let ms_w = 52.0;
    if let Some(ms) = row.gpu_ms {
        painter.text(
            Pos2::new(right, y - 1.0),
            egui::Align2::RIGHT_CENTER,
            format!("{ms:.3}"),
            FontId::monospace(11.5),
            ink::GPU,
        );
        let frac = if max_ms > 0.0 {
            (ms / max_ms).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let bar = Rect::from_min_size(
            Pos2::new(right - ms_w, r.bottom() - 4.0),
            Vec2::new(ms_w * frac.max(0.03), 2.0),
        );
        painter.rect_filled(
            Rect::from_min_size(
                Pos2::new(right - ms_w, r.bottom() - 4.0),
                Vec2::new(ms_w, 2.0),
            ),
            1.0,
            ink::RULE,
        );
        painter.rect_filled(bar, 1.0, ink::GPU);
    } else if row.ran && !row.timed {
        painter.text(
            Pos2::new(right, y),
            egui::Align2::RIGHT_CENTER,
            "·",
            FontId::monospace(11.5),
            ink::FAINT,
        );
    }
    let n = row.draw_count();
    let mut counts_left = right - ms_w - 10.0;
    if n > 0 {
        let s = if row.culled.is_empty() {
            format!("{n}")
        } else {
            format!("{n} −{}", row.culled.len())
        };
        let r = painter.text(
            Pos2::new(right - ms_w - 10.0, y),
            egui::Align2::RIGHT_CENTER,
            s,
            FontId::monospace(11.5),
            ink::DIM,
        );
        counts_left = r.left();
    }
    if !row.images.is_empty() {
        // A small picture frame: this pass left an image
        let at = Rect::from_center_size(
            Pos2::new(counts_left.min(right - ms_w - 34.0) - 14.0, y),
            Vec2::new(10.0, 8.0),
        );
        painter.rect_stroke(
            at,
            1.0,
            Stroke::new(1.0, ink::DIM),
            egui::StrokeKind::Inside,
        );
        if row.images.len() > 1 {
            painter.rect_stroke(
                at.translate(Vec2::new(2.0, -2.0)),
                1.0,
                Stroke::new(1.0, ink::FAINT),
                egui::StrokeKind::Inside,
            );
        }
    }
    let mut tip = String::new();
    if n > 0 {
        tip.push_str(&format!("{n} draws, {} triangles", row.triangles));
        if !row.culled.is_empty() {
            tip.push_str(&format!(", {} culled", row.culled.len()));
        }
    }
    if !row.writes.is_empty() {
        if !tip.is_empty() {
            tip.push('\n');
        }
        tip.push_str(&format!("writes {}", row.writes.join(", ")));
    }
    if !row.images.is_empty() {
        tip.push_str(&format!("\n{} image(s) captured", row.images.len()));
    }
    if tip.is_empty() {
        resp
    } else {
        resp.on_hover_text(tip)
    }
}

/// A draw-list layer under the selected pass.
fn layer_row(ui: &mut egui::Ui, depth: usize, layer: &str, n: usize, on: bool) -> egui::Response {
    let (r, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 18.0), Sense::click());
    let painter = ui.painter_at(r);
    if on {
        painter.rect_filled(r, 0.0, Color32::from_rgba_unmultiplied(86, 190, 176, 30));
    } else if resp.hovered() {
        painter.rect_filled(r, 0.0, ink::RAISED);
    }
    let x = r.left() + 14.0 + depth as f32 * 14.0;
    painter.text(
        Pos2::new(x, r.center().y),
        egui::Align2::LEFT_CENTER,
        layer,
        FontId::proportional(11.5),
        if on { ink::TEXT } else { ink::DIM },
    );
    painter.text(
        Pos2::new(r.right() - 72.0, r.center().y),
        egui::Align2::RIGHT_CENTER,
        n.to_string(),
        FontId::monospace(11.0),
        ink::DIM,
    );
    resp.on_hover_text("Show only this layer's draws")
}

/// A horizontal bar of segments; returns the pass of a segment clicked.
fn bar(
    ui: &mut egui::Ui,
    segs: &[(String, f32, Color32, Option<usize>)],
    total: f32,
    selected: Option<usize>,
) -> Option<usize> {
    let (r, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 18.0), Sense::hover());
    let painter = ui.painter_at(r);
    painter.rect_filled(r, 2.0, ink::RAISED);
    let total = total.max(1e-6);
    let mut x = r.left();
    let mut clicked = None;
    for (n, (label, ms, colour, pass)) in segs.iter().enumerate() {
        let w = ms / total * r.width();
        let seg = Rect::from_min_size(Pos2::new(x, r.top()), Vec2::new(w.max(1.0), r.height()));
        let id = ui.id().with(("seg", n, label.as_str()));
        let resp = ui.interact(seg, id, Sense::click());
        let fill = if resp.hovered() {
            colour.gamma_multiply(1.2)
        } else {
            *colour
        };
        painter.rect_filled(seg.shrink2(Vec2::new(0.5, 0.0)), 1.0, fill);
        if pass.is_some() && *pass == selected {
            painter.rect_stroke(
                seg,
                1.0,
                Stroke::new(1.5, Color32::WHITE),
                egui::StrokeKind::Inside,
            );
        }
        let g = painter.layout_no_wrap(
            label.clone(),
            FontId::proportional(11.0),
            Color32::from_rgb(16, 18, 22),
        );
        if g.size().x + 8.0 < w {
            painter.galley(
                Pos2::new(seg.left() + 4.0, seg.center().y - g.size().y / 2.0),
                g,
                Color32::BLACK,
            );
        }
        let resp = resp.on_hover_text(format!("{label}  {ms:.3} ms  ({:.0}%)", ms / total * 100.0));
        if resp.clicked() {
            clicked = *pass;
        }
        x += w;
    }
    clicked
}

/// Words starting at `pos`, on a dark tag.
fn tag(painter: &egui::Painter, pos: Pos2, words: &str, colour: Color32) {
    let g = painter.layout_no_wrap(words.to_string(), FontId::proportional(12.0), colour);
    let r = Rect::from_min_size(pos, g.size() + Vec2::new(14.0, 8.0));
    painter.rect_filled(r, 3.0, Color32::from_rgba_unmultiplied(12, 13, 15, 210));
    painter.galley(r.min + Vec2::new(7.0, 4.0), g, colour);
}

/// Words ending at `pos`, on a dark tag: the wipe's left side.
fn tag_ending_at(painter: &egui::Painter, pos: Pos2, words: &str, colour: Color32) {
    let g = painter.layout_no_wrap(words.to_string(), FontId::proportional(12.0), colour);
    let size = g.size() + Vec2::new(14.0, 8.0);
    let r = Rect::from_min_size(Pos2::new(pos.x - size.x, pos.y), size);
    painter.rect_filled(r, 3.0, Color32::from_rgba_unmultiplied(12, 13, 15, 210));
    painter.galley(r.min + Vec2::new(7.0, 4.0), g, colour);
}

/// `text` laid out on one line, cut with an ellipsis past `max_w`.
pub(super) fn elided(
    painter: &egui::Painter,
    text: String,
    font: FontId,
    colour: Color32,
    max_w: f32,
) -> std::sync::Arc<egui::Galley> {
    let mut job =
        egui::text::LayoutJob::single_section(text, egui::TextFormat::simple(font, colour));
    job.wrap = egui::text::TextWrapping::truncate_at_width(max_w.max(8.0));
    painter.layout_job(job)
}

fn centred(ui: &egui::Ui, r: Rect, words: &str, colour: Color32) {
    let g = ui.painter().layout(
        words.to_string(),
        FontId::proportional(14.0),
        colour,
        (r.width() - 60.0).max(100.0),
    );
    let pos = r.center() - g.size() / 2.0;
    ui.painter().galley(pos, g, colour);
}

/// How the image is being shown, in words.
fn describe(d: &super::super::display::Display, auto_words: &str, untouched: bool) -> String {
    if untouched {
        return auto_words.to_string();
    }
    match d.mode {
        Mode::Tonemap => format!("tonemapped at exposure {:.3}", d.exposure),
        Mode::Linear => format!("linear at exposure {:.3}", d.exposure),
        Mode::Depth => format!("depth {:.5}..{:.5}", d.lo, d.hi),
        Mode::Ids => "ids coloured".into(),
        Mode::Log => format!("log over 0..{:.3}", d.max),
        Mode::Signed => format!("signed, ±{:.3}", d.max.max(1.0)),
        Mode::Stored => "as stored".into(),
    }
}

/// A column of the draw table.
struct Col {
    name: &'static str,
    sort: Sort,
    x: f32,
    w: f32,
    right: bool,
}

fn columns(width: f32, with_pass: bool, with_culled: bool) -> Vec<Col> {
    // Fixed columns, the entity and pipeline sharing what is left
    let mut fixed: Vec<(&'static str, Sort, f32, bool)> = vec![
        ("Kind", Sort::Kind, 58.0, false),
        ("Tris", Sort::Triangles, 60.0, true),
        ("Blend", Sort::Blend, 64.0, false),
    ];
    if with_culled {
        fixed.push(("Culled", Sort::Culled, 78.0, false));
    }
    let pass_w = if with_pass { 104.0 } else { 0.0 };
    let fixed_w: f32 = fixed.iter().map(|c| c.2).sum::<f32>() + pass_w;
    let flex = (width - fixed_w).max(120.0);
    let mut order: Vec<(&'static str, Sort, f32, bool)> = Vec::new();
    if with_pass {
        order.push(("Pass", Sort::Order, pass_w, false));
    }
    order.push(("Entity", Sort::Entity, flex * 0.55, false));
    order.push(fixed[0]);
    order.push(("Pipeline", Sort::Pipeline, flex * 0.45, false));
    order.extend_from_slice(&fixed[1..]);
    let mut x = 0.0;
    order
        .into_iter()
        .map(|(name, sort, w, right)| {
            let c = Col {
                name,
                sort,
                x,
                w,
                right,
            };
            x += w;
            c
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_columns_fill_the_width_and_put_the_pass_first_when_asked() {
        let cols = columns(460.0, false, true);
        let end = cols.last().map(|c| c.x + c.w).unwrap();
        assert!((end - 460.0).abs() < 0.5, "{end}");
        assert_eq!(cols[0].name, "Entity");
        assert_eq!(columns(560.0, true, false)[0].name, "Pass");
        assert!(columns(560.0, true, false)
            .iter()
            .all(|c| c.name != "Culled"));
    }
}

#[cfg(test)]
mod app_tests {
    //! The window's own logic, a frame at a time with no GPU: the keys a
    //! person presses and what the panes then show.
    use super::*;
    use std::path::Path;

    fn app(start: Start) -> App {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/frames/two_crates.flintframe");
        let capture = Capture::open(&path).expect("the frozen fixture reads");
        App::new(capture, "two_crates".into(), &start, true, 8192)
    }

    fn frame(ctx: &egui::Context, app: &mut App, keys: &[Key]) {
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1600.0, 1000.0))),
            events: keys
                .iter()
                .map(|&key| egui::Event::Key {
                    key,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: Default::default(),
                })
                .collect(),
            ..Default::default()
        };
        flint_debug_ui::run_headless_frame(ctx, input, |ui| app.ui(ui));
    }

    fn shown_target(app: &App) -> Option<String> {
        let (shown, _) = app.images();
        shown.map(|(i, _)| app.model.images[i].image.target.clone())
    }

    #[test]
    fn a_replay_of_a_player_capture_keeps_the_captured_frames_cpu_side() {
        use flint_render::frame_capture::{CpuDescription, CpuFrame};
        // The frozen fixture, as the player would have written it: one
        // frame of history, the captured one
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/frames/two_crates.flintframe");
        let mut capture = Capture::open(&path).unwrap();
        let captured = CpuFrame {
            index: 7,
            ms: 16.5,
            phases_ms: [("scripts".to_string(), 2.0), ("render".to_string(), 4.0)]
                .into_iter()
                .collect(),
            ..Default::default()
        };
        capture.description.written_by = Some("flint-player".into());
        capture.description.cpu = Some(CpuDescription {
            frame: Some(captured.clone()),
            window: vec![captured],
            ..Default::default()
        });
        let mut app = App::new(capture, "two_crates".into(), &Start::default(), true, 8192);
        assert!(!app.timing.cpu_of_captured);
        // What `flint frame replay` writes: no CPU side of its own
        let dir = std::env::temp_dir().join(format!("flint-1208-replay-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut manifest: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(path.join("manifest.json")).unwrap())
                .unwrap();
        manifest["written_by"] = "flint frame replay".into();
        manifest["cpu"] = serde_json::Value::Null;
        std::fs::write(dir.join("manifest.json"), manifest.to_string()).unwrap();

        app.show_replay(&dir);
        assert!(app.showing_replay());
        let t = &app.timing;
        assert!(t.cpu_of_captured);
        assert_eq!(t.cpu_ms, Some(16.5), "the captured frame's phases");
        assert_eq!(t.cpu.len(), 2);
        assert_eq!(t.history.len(), 1);
        assert_eq!(t.cpu_missing, None, "nothing blames `flint render`");
        // The strip draws with it
        let ctx = egui::Context::default();
        frame(&ctx, &mut app, &[]);
        // and the captured frame put back is the capture's own again
        app.show_captured();
        assert!(!app.timing.cpu_of_captured);
        assert_eq!(app.timing.cpu_ms, Some(16.5));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_down_key_steps_the_frame_from_its_shadows_to_its_finished_image() {
        let ctx = egui::Context::default();
        let mut app = app(Start {
            pass: Some("Grass".into()),
            ..Default::default()
        });
        frame(&ctx, &mut app, &[]);
        assert_eq!(app.selected, 0);
        let mut seen: Vec<String> = Vec::new();
        loop {
            if let Some(t) = shown_target(&app) {
                if seen.last() != Some(&t) {
                    seen.push(t);
                }
            }
            let before = app.selected;
            frame(&ctx, &mut app, &[Key::ArrowDown]);
            if app.selected == before {
                break;
            }
            assert_eq!(app.selected, before + 1, "one pass a press");
        }
        assert_eq!(
            app.selected,
            app.model.passes.len() - 1,
            "it reached the last pass"
        );
        assert_eq!(seen.first().map(String::as_str), Some("shadow_map"));
        assert_eq!(seen.last().map(String::as_str), Some("surface"));
        let at = |t: &str| {
            seen.iter()
                .position(|s| s == t)
                .unwrap_or_else(|| panic!("{t} in {seen:?}"))
        };
        assert!(at("shadow_map") < at("hdr_scene") && at("hdr_scene") < at("ssao"));
        assert!(at("ssao") < at("bloom_mip_0") && at("bloom_mip_0") < at("surface"));

        // Home, End, and Shift+Down to the next pass with an image of its own
        frame(&ctx, &mut app, &[Key::Home]);
        assert_eq!(app.selected, 0);
        frame(&ctx, &mut app, &[Key::PageDown]);
        assert_eq!(app.model.passes[app.selected].key, "Shadow cascade 0");
        frame(&ctx, &mut app, &[Key::End]);
        assert_eq!(app.selected, app.model.passes.len() - 1);
    }

    #[test]
    fn left_and_right_pick_among_a_passes_images() {
        let ctx = egui::Context::default();
        let mut app = app(Start {
            pass: Some("Scene".into()),
            ..Default::default()
        });
        frame(&ctx, &mut app, &[]);
        assert_eq!(shown_target(&app).as_deref(), Some("hdr_scene"));
        frame(&ctx, &mut app, &[Key::ArrowRight]);
        assert_eq!(shown_target(&app).as_deref(), Some("depth"));
        frame(&ctx, &mut app, &[Key::ArrowRight]);
        assert_eq!(
            shown_target(&app).as_deref(),
            Some("depth"),
            "it stops at the last"
        );
        frame(&ctx, &mut app, &[Key::ArrowLeft]);
        assert_eq!(shown_target(&app).as_deref(), Some("hdr_scene"));
        // W turns the wipe on against the pass before
        frame(&ctx, &mut app, &[Key::W]);
        let (_, compare) = app.images();
        let b = compare.expect("a wipe");
        assert_eq!(
            app.model.passes[app.model.images[b].pass].key,
            "Shadow cascade 2"
        );
    }

    #[test]
    fn an_entity_is_found_by_part_of_its_name_and_marked_in_every_pass() {
        let ctx = egui::Context::default();
        let mut app = app(Start {
            entity: Some("lant".into()),
            ..Default::default()
        });
        frame(&ctx, &mut app, &[]);
        let name = app.entity.clone().expect("an entity selected");
        assert!(name.contains("lant"), "{name}");
        let passes = app.model.entity_passes(&name);
        assert!(
            passes.len() >= 2,
            "drawn in the shadows and the scene: {passes:?}"
        );
        // The table lists its draws from every pass, and nothing else
        let rows = app.model.table(&app.query);
        assert_eq!(rows.len(), passes.iter().map(|p| p.1).sum::<usize>());
        assert!(rows
            .iter()
            .all(|&i| app.model.draws[i].draw.entity.as_deref() == Some(name.as_str())));
        // Escape lets it go
        frame(&ctx, &mut app, &[Key::Escape]);
        assert!(app.entity.is_none() && app.query.filter.is_empty());
    }

    #[test]
    fn copy_summary_holds_what_frame_show_prints() {
        let app = app(Start::default());
        assert!(app.summary.starts_with("Capture"), "{}", app.summary);
        assert!(app.summary.contains("two_crates"));
    }
}
