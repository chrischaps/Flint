//! The player's share of load reports (#1092): when a load begins and
//! ends, and what it says. What is timed inside one is
//! `flint_core::load_report`, fed by every timed frame note and by the
//! stages the bring-up names (`scene_entry.rs`, `init.rs`).
//!
//! A start-up is begun by whoever builds the app (`player_args::start`,
//! the web page under `?perf`), before the scene is read. A transition
//! begins in `execute_scene_transition` and a chunk in `load_chunk`, when
//! `launch.load_report` is on. Each ends at the next present after its
//! bring-up (`frame_presented`), so its last row is the first frame;
//! one still open at exit ends there.

use super::PlayerApp;
use flint_core::load_report::{self, LoadKind, LoadReport};

impl PlayerApp {
    /// Begin recording a `kind` load of `name`, when the player reports
    /// loads. A load still open (a transition asked for before the
    /// start-up's first frame) is finished and said first.
    pub(super) fn begin_load_report(&mut self, kind: LoadKind, name: &str) {
        if !self.launch.load_report {
            return;
        }
        if let Some(last) = load_report::begin(kind, name) {
            self.say_load(last);
        }
    }

    /// The event loop is about to start: until the first `resumed`, the
    /// start-up waits on the window system (#1092).
    pub(crate) fn awaiting_window(&mut self) {
        self.stats.awaiting_window =
            Some(load_report::span(load_report::Stage::Window, String::new));
    }

    /// The first `resumed` came: the wait for it ends.
    pub(super) fn window_resumed(&mut self) {
        self.stats.awaiting_window = None;
    }

    /// The bring-up is done: what runs until the next present is the
    /// load's first frame.
    pub(super) fn load_brought_up(&mut self) {
        load_report::set_name(flint_core::project::shown(std::path::Path::new(
            &self.core.scene_path,
        )));
        load_report::bring_up_done();
    }

    /// A frame was presented: a load waiting for its first frame ends.
    pub(super) fn frame_presented_for_loads(&mut self) {
        if load_report::waiting_for_first_frame() {
            if let Some(report) = load_report::finish() {
                self.say_load(report);
            }
        }
    }

    /// The run ends: a load that never reached a frame (a headless run, a
    /// window closed during the load) is said as far as it got.
    pub(super) fn finish_load_reports(&mut self) {
        if let Some(report) = load_report::finish() {
            self.say_load(report);
        }
    }

    /// Say a load's table (target `report`, which `--quiet` keeps) and
    /// keep it.
    fn say_load(&mut self, report: LoadReport) {
        // The table alone: fields would trail its last row
        tracing::info!(target: "report", "{}", report.table());
        self.stats.loads.push(report);
    }

    /// Every load's report so far, the start-up first (#1092).
    pub fn load_reports(&self) -> &[LoadReport] {
        &self.stats.loads
    }
}
