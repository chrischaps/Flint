//! Dead Calm panel (F3 family).
//!
//! Controls a game-side `dead_calm` component (convention: a calm script
//! schedules a rare ocean-stillness event, publishes `phase` 0-3, `calm`
//! 0..1 and `next_in_s`, and honors one-shot `trigger_calm` / `end_now`
//! flags). The panel is created only when the component exists, so games
//! without a dead calm never see it.
//!
//! The script owns the published fields; the panel shows them read-only via
//! [`DeadCalmDebugPanel::sync_status`] and never writes them. The buttons
//! queue one-shot pushes (see `take_one_shots`) that the script consumes
//! and resets after acting on them. All tuning lives in the scene TOML —
//! this panel is deliberately just a status line and two doorbells.

use crate::DebugPanel;

pub const DEAD_CALM_PHASE_NAMES: [&str; 4] =
    ["Idle", "Falling calm", "Dead calm", "Life returning"];

pub struct DeadCalmDebugPanel {
    calm_entity_name: String,
    open: bool,
    // Read-only status published by the calm script.
    phase: f32,
    calm: f32,
    next_in_s: f32,
    // One-shot flags queued by the buttons, drained by the host each frame.
    pending_trigger: bool,
    pending_end: bool,
}

impl DeadCalmDebugPanel {
    pub fn new(calm_entity_name: String) -> Self {
        Self {
            calm_entity_name,
            open: false,
            phase: 0.0,
            calm: 0.0,
            next_in_s: -1.0,
            pending_trigger: false,
            pending_end: false,
        }
    }

    pub fn entity_name(&self) -> &str {
        &self.calm_entity_name
    }

    /// Update the read-only displays from the script-published fields.
    pub fn sync_status(&mut self, phase: f32, calm: f32, next_in_s: f32) {
        self.phase = phase;
        self.calm = calm;
        self.next_in_s = next_in_s;
    }

    /// Drain the queued one-shot flags as component field writes.
    pub fn take_one_shots(&mut self) -> Vec<(&'static str, toml::Value)> {
        let mut fields = Vec::new();
        if self.pending_trigger {
            self.pending_trigger = false;
            fields.push(("trigger_calm", toml::Value::Float(1.0)));
        }
        if self.pending_end {
            self.pending_end = false;
            fields.push(("end_now", toml::Value::Boolean(true)));
        }
        fields
    }
}

impl DebugPanel for DeadCalmDebugPanel {
    fn name(&self) -> &str {
        "Dead Calm"
    }

    fn ui(&mut self, ui: &mut egui::Ui) {
        let phase_i = (self.phase.round() as usize).min(3);
        let next = if self.next_in_s >= 0.0 {
            format!("next in {:.0}s", self.next_in_s)
        } else {
            "no attempt scheduled".to_string()
        };
        ui.label(format!(
            "{}  ·  calm {:.2}  ·  {}",
            DEAD_CALM_PHASE_NAMES[phase_i], self.calm, next
        ));
        ui.separator();
        // The script starts only from Idle and ends only from an active
        // calm; stray presses are consumed and ignored, so neither button
        // needs disabling.
        ui.horizontal(|ui| {
            if ui.button("Calm now").clicked() {
                self.pending_trigger = true;
            }
            if ui.button("End now").clicked() {
                self.pending_end = true;
            }
        });
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
