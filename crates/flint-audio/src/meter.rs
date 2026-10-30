//! How steadily a main-thread audio stream is fed (#101).
//!
//! The browser's stream asks for each buffer from the main thread and
//! schedules it to start where the one before it ends. A buffer's *slack* is
//! how far ahead of the audio clock it was scheduled; negative slack means
//! its start had already passed, which leaves a gap you hear as a click.
//! [`BufferMeter`] keeps that per buffer and turns it into console lines. It
//! has no platform code, so its arithmetic is tested on the desktop.

/// Seconds between steadiness summaries.
pub const REPORT_SECONDS: f64 = 10.0;

/// At most this many late buffers get a line of their own in each report
/// window; the summary still counts them all.
pub const LATE_LINES_PER_WINDOW: u64 = 5;

/// How steadily the stream has been fed, since it started.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct BufferStats {
    /// Length of one buffer, ms.
    pub buffer_ms: f64,
    pub buffers: u64,
    /// Buffers scheduled after their start time had already passed.
    pub late: u64,
    /// The worst slack seen, ms; negative means late by that much.
    pub min_slack_ms: f64,
    /// Mean slack over every buffer, ms.
    pub mean_slack_ms: f64,
    /// Mean and worst time spent mixing one buffer, ms.
    pub mix_mean_ms: f64,
    pub mix_max_ms: f64,
    /// Longest wall time between two buffer requests, ms.
    pub wait_max_ms: f64,
}

/// What one buffer has to say. `warn` lines are about lost sound.
#[derive(Debug, Default, PartialEq)]
pub struct MeterLines {
    pub late: Option<String>,
    pub summary: Option<String>,
    /// The summary's window had late buffers.
    pub summary_warns: bool,
}

#[derive(Debug, Default, Clone, Copy)]
struct Sums {
    buffers: u64,
    late: u64,
    slack_sum: f64,
    slack_min: f64,
    mix_sum: f64,
    mix_max: f64,
    wait_max: f64,
}

impl Sums {
    fn add(&mut self, slack: f64, mix: f64, wait: f64) {
        self.slack_min = if self.buffers == 0 {
            slack
        } else {
            self.slack_min.min(slack)
        };
        self.buffers += 1;
        self.late += u64::from(slack < 0.0);
        self.slack_sum += slack;
        self.mix_sum += mix;
        self.mix_max = self.mix_max.max(mix);
        self.wait_max = self.wait_max.max(wait);
    }

    fn mean(sum: f64, n: u64) -> f64 {
        if n == 0 {
            0.0
        } else {
            sum / n as f64
        }
    }
}

/// Per-buffer slack and mix time, summed over the whole run and over each
/// report window. Means are plain means over the window's buffers, not a
/// running average, so one bad stretch is not smoothed away.
#[derive(Debug, Default)]
pub struct BufferMeter {
    buffer_ms: f64,
    window_start: Option<f64>,
    window: Sums,
    total: Sums,
}

impl BufferMeter {
    pub fn new(buffer_ms: f64) -> Self {
        Self {
            buffer_ms,
            ..Default::default()
        }
    }

    pub fn buffer_ms(&self) -> f64 {
        self.buffer_ms
    }

    pub fn stats(&self) -> BufferStats {
        let t = &self.total;
        BufferStats {
            buffer_ms: self.buffer_ms,
            buffers: t.buffers,
            late: t.late,
            min_slack_ms: t.slack_min,
            mean_slack_ms: Sums::mean(t.slack_sum, t.buffers),
            mix_mean_ms: Sums::mean(t.mix_sum, t.buffers),
            mix_max_ms: t.mix_max,
            wait_max_ms: t.wait_max,
        }
    }

    /// Record one buffer, all in ms: its `slack`, the time spent mixing it,
    /// the wall clock `now` when it was asked for, and the `wait` since the
    /// request before it (0 for the first).
    pub fn record(&mut self, slack: f64, mix: f64, now: f64, wait: f64) -> MeterLines {
        let start = *self.window_start.get_or_insert(now);
        self.window.add(slack, mix, wait);
        self.total.add(slack, mix, wait);
        let mut lines = MeterLines::default();
        if slack < 0.0 && self.window.late <= LATE_LINES_PER_WINDOW {
            lines.late = Some(format!(
                "[flint-audio] late buffer #{}: started {:.1} ms after its slot \
                 (the main thread went {:.0} ms between buffers; one buffer is {:.0} ms)",
                self.total.buffers, -slack, wait, self.buffer_ms
            ));
        }
        let elapsed = now - start;
        if elapsed >= REPORT_SECONDS * 1000.0 {
            let w = self.window;
            lines.summary = Some(format!(
                "[flint-audio] {:.0} s: {} buffers, {} late · slack min {:.1} ms, mean {:.1} ms · \
                 mix mean {:.2} ms, max {:.2} ms · longest wait {:.0} ms · run: {} late of {}",
                elapsed / 1000.0,
                w.buffers,
                w.late,
                w.slack_min,
                Sums::mean(w.slack_sum, w.buffers),
                Sums::mean(w.mix_sum, w.buffers),
                w.mix_max,
                w.wait_max,
                self.total.late,
                self.total.buffers,
            ));
            lines.summary_warns = w.late > 0;
            self.window = Sums::default();
            self.window_start = Some(now);
        }
        lines
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steady_buffers_report_once_a_window_and_never_warn() {
        let mut m = BufferMeter::new(46.4);
        let mut summaries = 0;
        for i in 0..1000 {
            let now = i as f64 * 46.4;
            let lines = m.record(40.0, 0.5, now, if i == 0 { 0.0 } else { 46.4 });
            assert!(lines.late.is_none());
            if let Some(s) = lines.summary {
                summaries += 1;
                assert!(!lines.summary_warns);
                assert!(s.contains(" 0 late"), "{s}");
            }
        }
        // 1000 buffers of 46.4 ms is 46.4 s: four whole windows.
        assert_eq!(summaries, 4);
        let s = m.stats();
        assert_eq!((s.buffers, s.late), (1000, 0));
        assert_eq!(s.min_slack_ms, 40.0);
        assert!((s.mean_slack_ms - 40.0).abs() < 1e-9);
        assert_eq!(s.wait_max_ms, 46.4);
    }

    #[test]
    fn a_late_buffer_is_counted_named_and_warns_its_window() {
        let mut m = BufferMeter::new(46.4);
        m.record(40.0, 0.4, 0.0, 0.0);
        let lines = m.record(-12.5, 0.4, 105.0, 105.0);
        let late = lines.late.expect("a line for the late buffer");
        assert!(
            late.contains("#2") && late.contains("12.5 ms after"),
            "{late}"
        );
        let lines = m.record(30.0, 0.4, 10_500.0, 46.4);
        assert!(lines.summary_warns);
        assert!(lines.summary.unwrap().contains(" 1 late"));
        let s = m.stats();
        assert_eq!((s.buffers, s.late, s.min_slack_ms), (3, 1, -12.5));
        // The next window starts clean.
        let lines = m.record(40.0, 0.4, 21_000.0, 46.4);
        assert!(!lines.summary_warns && lines.summary.unwrap().contains(" 0 late"));
    }

    #[test]
    fn a_burst_of_late_buffers_is_capped_per_window() {
        let mut m = BufferMeter::new(10.0);
        let named = (0..20)
            .filter(|&i| m.record(-1.0, 0.1, i as f64, 1.0).late.is_some())
            .count();
        assert_eq!(named as u64, LATE_LINES_PER_WINDOW);
        assert_eq!(m.stats().late, 20);
    }
}
