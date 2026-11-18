//! The window's two tabs (#1099): the frame debugger ([`App`], a capture)
//! and the profiler ([`Profiler`], a scope trace). A capture that holds a
//! trace (`--capture-on-hitch` while recording) opens with both, a capture
//! alone with the frame debugger, a trace alone with the profiler. P
//! switches between them, as does the tab bar.

use super::profile::{ProfileStart, Profiler};
use super::ui::{apply_style, ink, App, Start};
use crate::commands::frame::read::Capture;
use crate::commands::frame::trace::Trace;
use egui::{Key, RichText};

/// What the window was opened on.
pub struct Opened {
    pub capture: Option<(Capture, String)>,
    pub trace: Option<(Trace, String)>,
    /// Why a capture's trace couldn't be read, said in the tab bar.
    pub trace_problem: Option<String>,
}

impl Opened {
    /// The window's title: what it shows.
    pub fn title(&self) -> String {
        match (&self.capture, &self.trace) {
            (Some((_, shown)), _) => format!("Flint frame — {shown}"),
            (None, Some((_, shown))) => format!("Flint profile — {shown}"),
            (None, None) => "Flint frame".into(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tab {
    Frame,
    Profile,
}

pub struct Tabs {
    frame: Option<App>,
    profile: Option<Profiler>,
    tab: Tab,
    styled: bool,
    trace_problem: Option<String>,
}

impl Tabs {
    pub fn new(
        opened: Opened,
        start: &Start,
        profile_start: &ProfileStart,
        open_on_profile: bool,
        srgb_target: bool,
        max_side: u32,
    ) -> Self {
        let mut profile_start = profile_start.clone();
        if let Some((c, _)) = &opened.capture {
            if profile_start.capture_hitch.is_none() {
                profile_start.capture_hitch = c
                    .description
                    .cpu
                    .as_ref()
                    .and_then(|cpu| cpu.hitch.as_ref())
                    .map(|h| h.frame as i64);
            }
            if profile_start.capture_frame.is_none() {
                profile_start.capture_frame = c
                    .description
                    .frame
                    .index
                    .and_then(|i| u32::try_from(i).ok());
            }
        }
        let frame = opened
            .capture
            .map(|(c, shown)| App::new(c, shown, start, srgb_target, max_side));
        let profile = opened
            .trace
            .map(|(t, shown)| Profiler::new(t, shown, &profile_start));
        let tab = if profile.is_some() && (open_on_profile || frame.is_none()) {
            Tab::Profile
        } else {
            Tab::Frame
        };
        Self {
            frame,
            profile,
            tab,
            styled: false,
            trace_problem: opened.trace_problem,
        }
    }

    #[cfg(test)]
    pub fn tab(&self) -> Tab {
        self.tab
    }

    /// For a screenshot: the frame debugger's replay and images, waited on.
    pub fn wait_for_replay(&mut self) {
        if let Some(f) = &mut self.frame {
            f.wait_for_replay();
        }
    }

    pub fn wait_for_images(&mut self) {
        if let Some(f) = &mut self.frame {
            f.wait_for_images();
        }
    }

    pub fn loading(&self) -> bool {
        self.frame.as_ref().is_some_and(App::loading)
    }

    pub fn ui(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        if !self.styled {
            apply_style(&ctx);
            self.styled = true;
        }
        let both = self.frame.is_some() && self.profile.is_some();
        if both && !ctx.egui_wants_keyboard_input() && ctx.input(|i| i.key_pressed(Key::P)) {
            self.tab = match self.tab {
                Tab::Frame => Tab::Profile,
                Tab::Profile => Tab::Frame,
            };
        }
        if both || self.trace_problem.is_some() {
            egui::Panel::top("window-tabs")
                .frame(
                    egui::Frame::new()
                        .fill(ink::GROUND)
                        .inner_margin(egui::Margin {
                            left: 10,
                            right: 14,
                            top: 6,
                            bottom: 0,
                        }),
                )
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        if self.frame.is_some()
                            && ui
                                .selectable_label(self.tab == Tab::Frame, "  Frame  ")
                                .on_hover_text("The passes, images and draws (P)")
                                .clicked()
                        {
                            self.tab = Tab::Frame;
                        }
                        if self.profile.is_some()
                            && ui
                                .selectable_label(self.tab == Tab::Profile, "  Profiler  ")
                                .on_hover_text("The scope trace around the frame (P)")
                                .clicked()
                        {
                            self.tab = Tab::Profile;
                        }
                        if let Some(why) = &self.trace_problem {
                            ui.label(RichText::new(why).small().color(ink::HITCH));
                        } else {
                            ui.label(RichText::new("P switches").small().color(ink::FAINT));
                        }
                    });
                });
        }
        match (self.tab, &mut self.frame, &mut self.profile) {
            (Tab::Profile, _, Some(p)) => p.ui(ui),
            (_, Some(f), _) => f.ui(ui),
            (_, None, Some(p)) => p.ui(ui),
            (_, None, None) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::frame::trace::tests::sample;
    use egui::{Pos2, Rect, Vec2};
    use std::path::Path;

    fn opened(capture: bool, trace: bool) -> Opened {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/frames/two_crates.flintframe");
        Opened {
            capture: capture.then(|| (Capture::open(&path).unwrap(), "two_crates".into())),
            trace: trace.then(|| (Trace::read(&sample()).unwrap(), "t.json".into())),
            trace_problem: None,
        }
    }

    fn frame(ctx: &egui::Context, tabs: &mut Tabs, keys: &[Key]) {
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
        flint_debug_ui::run_headless_frame(ctx, input, |ui| tabs.ui(ui));
    }

    fn tabs(capture: bool, trace: bool, on_profile: bool) -> Tabs {
        Tabs::new(
            opened(capture, trace),
            &Start::default(),
            &ProfileStart::default(),
            on_profile,
            true,
            8192,
        )
    }

    #[test]
    fn a_capture_with_a_trace_opens_on_the_frame_and_p_switches_tabs() {
        let ctx = egui::Context::default();
        let mut t = tabs(true, true, false);
        assert_eq!(t.tab(), Tab::Frame);
        frame(&ctx, &mut t, &[]);
        frame(&ctx, &mut t, &[Key::P]);
        assert_eq!(t.tab(), Tab::Profile);
        frame(&ctx, &mut t, &[]);
        frame(&ctx, &mut t, &[Key::P]);
        assert_eq!(t.tab(), Tab::Frame);
        assert_eq!(tabs(true, true, true).tab(), Tab::Profile, "--profile");
    }

    #[test]
    fn a_trace_alone_is_the_profiler_and_a_capture_alone_the_frame() {
        let ctx = egui::Context::default();
        let mut t = tabs(false, true, false);
        assert_eq!(t.tab(), Tab::Profile);
        frame(&ctx, &mut t, &[Key::P]);
        assert_eq!(t.tab(), Tab::Profile, "P has nowhere to go");
        let mut t = tabs(true, false, true);
        assert_eq!(t.tab(), Tab::Frame, "no trace, no profiler");
        frame(&ctx, &mut t, &[]);
    }
}
