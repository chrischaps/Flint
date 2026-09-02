//! Visitor panel (F3 family).
//!
//! Controls a game-side `raft_visitor` component (convention: a visitor
//! script schedules a rare second-raft drift-by, publishes `phase` 0-3 and
//! `day`, and honors a one-shot `trigger_visit` flag). The panel is created
//! only when the component exists, so games without a visitor never see it.
//!
//! The script owns the published fields; the panel shows them read-only via
//! [`VisitorDebugPanel::sync_status`] and never writes them. The trigger
//! button queues a one-shot push (see `take_one_shots`) that the script
//! consumes and resets after acting on it. All tuning lives in the scene
//! TOML — this panel is deliberately just a status line and a doorbell.

use crate::DebugPanel;

pub const VISITOR_PHASE_NAMES: [&str; 4] = ["Away", "Approaching", "Holding", "Departing"];

pub struct VisitorDebugPanel {
    visitor_entity_name: String,
    open: bool,
    // Read-only status published by the visitor script.
    phase: f32,
    day: f32,
    // One-shot flag queued by the button, drained by the host each frame.
    pending_trigger: bool,
}

impl VisitorDebugPanel {
    pub fn new(visitor_entity_name: String) -> Self {
        Self {
            visitor_entity_name,
            open: false,
            phase: 0.0,
            day: 1.0,
            pending_trigger: false,
        }
    }

    pub fn entity_name(&self) -> &str {
        &self.visitor_entity_name
    }

    /// Update the read-only displays from the script-published fields.
    pub fn sync_status(&mut self, phase: f32, day: f32) {
        self.phase = phase;
        self.day = day;
    }

    /// Drain the queued one-shot flag as component field writes.
    pub fn take_one_shots(&mut self) -> Vec<(&'static str, toml::Value)> {
        let mut fields = Vec::new();
        if self.pending_trigger {
            self.pending_trigger = false;
            fields.push(("trigger_visit", toml::Value::Float(1.0)));
        }
        fields
    }
}

impl DebugPanel for VisitorDebugPanel {
    fn name(&self) -> &str {
        "Visitor"
    }

    fn ui(&mut self, ui: &mut egui::Ui) {
        let phase_i = (self.phase.round() as usize).min(3);
        ui.label(format!(
            "{}  ·  day {:.0}",
            VISITOR_PHASE_NAMES[phase_i], self.day
        ));
        ui.separator();
        // The script fires only from Away; while a visit is running the
        // press is consumed and ignored, so no need to disable the button.
        if ui.button("Visit now").clicked() {
            self.pending_trigger = true;
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
