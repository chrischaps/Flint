//! Render statistics panel — FPS counter and frame timing

use std::collections::VecDeque;
use std::time::Instant;

/// Tracks rendering performance metrics
pub struct RenderStats {
    frame_times: VecDeque<Instant>,
    fps: f32,
    last_update: Instant,
}

impl Default for RenderStats {
    fn default() -> Self {
        Self {
            frame_times: VecDeque::new(),
            fps: 0.0,
            last_update: Instant::now(),
        }
    }
}

impl RenderStats {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record that a frame was rendered
    pub fn record_frame(&mut self) {
        let now = Instant::now();
        self.frame_times.push_back(now);

        // Keep only frames from the last second
        let cutoff = now - std::time::Duration::from_secs(1);
        while self.frame_times.front().is_some_and(|&t| t < cutoff) {
            self.frame_times.pop_front();
        }

        // Update FPS every 250ms
        if now.duration_since(self.last_update).as_millis() > 250 {
            self.fps = self.frame_times.len() as f32;
            self.last_update = now;
        }
    }

    /// Draw the stats UI
    pub fn ui(&self, ui: &mut egui::Ui) {
        ui.monospace(format!("FPS: {:.0}", self.fps));
        if self.fps > 0.0 {
            ui.monospace(format!("Frame: {:.1}ms", 1000.0 / self.fps));
        }
    }
}
