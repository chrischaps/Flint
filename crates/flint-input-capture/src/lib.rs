//! High-rate gamepad capture for rhythm judgment.
//!
//! A dedicated thread owns `Gilrs` and polls it far above frame rate
//! (default 1 kHz), so pulse timing is resolved at millisecond granularity
//! instead of the player app's once-per-frame drain. Each event is stamped
//! via the shared [`ClockBridge`] with a **compensated suite sample**
//! (bridged clock sample minus the caller's total judgment offset — output
//! latency plus tap calibration), then sent over an `mpsc` channel as
//! flint-music [`InputEvent`]s. Downstream code never touches gilrs; the
//! replay path constructs the identical stream from a file.
//!
//! Verb mapping (prototype scope, physical → chart verb space):
//! left stick → `lean` (radial deadzone, rescaled), South button or the
//! right trigger (R2) → `pulse`. Charts never see buttons.
//!
//! On Windows gilrs sits on XInput, which is itself polled — event
//! timestamps are only as fine as the poll cadence, which is exactly why
//! this thread exists and why [`measure_granularity`] reports the real
//! numbers instead of trusting the driver.

use flint_music::clock_bridge::ClockBridge;
use flint_music::input_stream::{InputEvent, LeanSample, PulseEvent};
use gilrs::{Axis, Button, EventType, Gilrs};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime};

#[derive(Debug, Clone, Copy)]
pub struct CaptureConfig {
    /// Poll frequency of the capture thread.
    pub poll_hz: u32,
    /// Radial stick deadzone in [0, 1); input is rescaled so the deadzone
    /// edge maps to 0 and full deflection stays 1.
    pub deadzone: f64,
    /// Total judgment offset in samples (measured output latency plus tap
    /// calibration), subtracted from every bridged timestamp so events carry
    /// the musical moment the player was responding to.
    pub offset_samples: i64,
}

impl Default for CaptureConfig {
    fn default() -> Self {
        Self {
            poll_hz: 1000,
            deadzone: 0.12,
            offset_samples: 0,
        }
    }
}

/// Owns the capture thread; dropping (or calling [`CaptureHandle::stop`])
/// stops it. The paired receiver yields events in nondecreasing sample
/// order (one producer, monotonic stamping).
pub struct CaptureHandle {
    stop: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
}

impl CaptureHandle {
    pub fn stop(mut self) {
        self.stop_inner();
    }

    fn stop_inner(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

impl Drop for CaptureHandle {
    fn drop(&mut self) {
        self.stop_inner();
    }
}

/// Spawn the capture thread. Returns `Err` if no gamepad backend is
/// available (headless/CI — the caller decides whether that is fatal).
/// Events observed before the bridge warms up are dropped with a warning.
pub fn spawn(
    bridge: ClockBridge,
    cfg: CaptureConfig,
) -> flint_core::Result<(CaptureHandle, mpsc::Receiver<InputEvent>)> {
    // Probe for a backend on the caller's thread so failure is synchronous;
    // the thread creates its own instance (Gilrs is not Send on all
    // platforms and must live where it is polled).
    Gilrs::new().map_err(|e| {
        flint_core::FlintError::InputError(format!("gamepad backend unavailable: {e}"))
    })?;

    let stop = Arc::new(AtomicBool::new(false));
    let (tx, rx) = mpsc::channel();
    let thread_stop = stop.clone();
    let join = std::thread::Builder::new()
        .name("flint-input-capture".into())
        .spawn(move || capture_loop(bridge, cfg, thread_stop, tx))
        .map_err(|e| flint_core::FlintError::InputError(format!("spawn capture thread: {e}")))?;

    Ok((
        CaptureHandle {
            stop,
            join: Some(join),
        },
        rx,
    ))
}

fn capture_loop(
    bridge: ClockBridge,
    cfg: CaptureConfig,
    stop: Arc<AtomicBool>,
    tx: mpsc::Sender<InputEvent>,
) {
    let mut gilrs = match Gilrs::new() {
        Ok(g) => g,
        Err(e) => {
            tracing::error!("capture thread lost gamepad backend: {e}");
            return;
        }
    };
    let period = Duration::from_secs_f64(1.0 / cfg.poll_hz.max(1) as f64);
    let (mut raw_x, mut raw_y) = (0.0f64, 0.0f64);
    let mut warned_cold = false;
    let mut last_sample = i64::MIN;

    while !stop.load(Ordering::Relaxed) {
        let mut lean_dirty = false;
        let mut pulses = 0u32;
        while let Some(ev) = gilrs.next_event() {
            match ev.event {
                EventType::AxisChanged(Axis::LeftStickX, v, _) => {
                    raw_x = v as f64;
                    lean_dirty = true;
                }
                EventType::AxisChanged(Axis::LeftStickY, v, _) => {
                    raw_y = v as f64;
                    lean_dirty = true;
                }
                EventType::ButtonPressed(Button::South | Button::RightTrigger2, _) => {
                    pulses += 1;
                }
                _ => {}
            }
        }

        if lean_dirty || pulses > 0 {
            match bridge.sample_at(Instant::now()) {
                Some(clock_sample) => {
                    // Monotonic guard: a bridge refit can step the mapping
                    // backwards by a hair; judgment requires nondecreasing.
                    let sample = (clock_sample - cfg.offset_samples).max(last_sample);
                    last_sample = sample;
                    if lean_dirty {
                        let (x, y) = apply_deadzone(raw_x, raw_y, cfg.deadzone);
                        if tx.send(InputEvent::Lean(LeanSample { sample, x, y })).is_err() {
                            return; // receiver gone; session over
                        }
                    }
                    for _ in 0..pulses {
                        let pulse = PulseEvent {
                            sample,
                            kind: "pulse".into(),
                        };
                        if tx.send(InputEvent::Pulse(pulse)).is_err() {
                            return;
                        }
                    }
                }
                None => {
                    if !warned_cold {
                        warned_cold = true;
                        tracing::warn!(
                            "input before clock bridge warm-up dropped (pre-roll)"
                        );
                    }
                }
            }
        }

        std::thread::sleep(period);
    }
}

/// Radial deadzone with rescaling: below `dz` is neutral; above, magnitude
/// is remapped so the deadzone edge is 0 and full deflection stays 1.
pub fn apply_deadzone(x: f64, y: f64, dz: f64) -> (f64, f64) {
    let mag = (x * x + y * y).sqrt();
    if mag <= dz || dz >= 1.0 {
        return (0.0, 0.0);
    }
    let scale = ((mag - dz) / (1.0 - dz)).min(1.0) / mag;
    (x * scale, y * scale)
}

/// One bucket of an inter-event-interval histogram.
#[derive(Debug, Clone, Copy)]
pub struct DeltaBucket {
    pub upper_ms: f64,
    pub count: u64,
}

/// The D1 spike's answer: how finely this machine's input path actually
/// resolves time, measured two ways — deltas between our receipt instants
/// and deltas between gilrs's own event timestamps.
#[derive(Debug, Clone)]
pub struct GranularityReport {
    pub duration_s: f64,
    pub poll_hz: u32,
    pub events: u64,
    pub receipt_deltas: Vec<DeltaBucket>,
    pub driver_deltas: Vec<DeltaBucket>,
    pub receipt_median_ms: f64,
    pub driver_median_ms: f64,
}

const BUCKET_EDGES_MS: [f64; 8] = [0.5, 1.0, 2.0, 4.0, 8.0, 16.0, 33.0, f64::INFINITY];

/// Poll gilrs at `poll_hz` for `duration`, recording inter-event intervals
/// while the operator wiggles the stick. Blocking; run on a spare thread or
/// a dedicated CLI mode.
pub fn measure_granularity(duration: Duration, poll_hz: u32) -> flint_core::Result<GranularityReport> {
    let mut gilrs = Gilrs::new().map_err(|e| {
        flint_core::FlintError::InputError(format!("gamepad backend unavailable: {e}"))
    })?;
    let period = Duration::from_secs_f64(1.0 / poll_hz.max(1) as f64);
    let start = Instant::now();
    let mut receipt: Vec<f64> = Vec::new();
    let mut driver: Vec<f64> = Vec::new();
    let mut last_receipt: Option<Instant> = None;
    let mut last_driver: Option<SystemTime> = None;
    let mut events = 0u64;

    while start.elapsed() < duration {
        while let Some(ev) = gilrs.next_event() {
            if !matches!(ev.event, EventType::AxisChanged(..) | EventType::ButtonPressed(..)) {
                continue;
            }
            events += 1;
            let now = Instant::now();
            if let Some(prev) = last_receipt.replace(now) {
                receipt.push(now.duration_since(prev).as_secs_f64() * 1000.0);
            }
            if let Some(prev) = last_driver.replace(ev.time) {
                if let Ok(d) = ev.time.duration_since(prev) {
                    driver.push(d.as_secs_f64() * 1000.0);
                }
            }
        }
        std::thread::sleep(period);
    }

    Ok(GranularityReport {
        duration_s: duration.as_secs_f64(),
        poll_hz,
        events,
        receipt_median_ms: median(&mut receipt.clone()),
        driver_median_ms: median(&mut driver.clone()),
        receipt_deltas: bucketize(&receipt),
        driver_deltas: bucketize(&driver),
    })
}

impl GranularityReport {
    /// Serialize as the TOML committed to `logs/latency/`.
    pub fn to_toml(&self) -> String {
        let mut out = String::new();
        out.push_str("# Input-path granularity spike (flint-input-capture)\n");
        out.push_str("[run]\n");
        out.push_str(&format!("duration_s = {}\n", self.duration_s));
        out.push_str(&format!("poll_hz = {}\n", self.poll_hz));
        out.push_str(&format!("events = {}\n", self.events));
        out.push_str(&format!("receipt_median_ms = {}\n", fmt(self.receipt_median_ms)));
        out.push_str(&format!("driver_median_ms = {}\n", fmt(self.driver_median_ms)));
        for (name, buckets) in [
            ("receipt_deltas", &self.receipt_deltas),
            ("driver_deltas", &self.driver_deltas),
        ] {
            out.push_str(&format!("\n[{name}]\n"));
            for b in buckets.iter() {
                let label = if b.upper_ms.is_finite() {
                    format!("le_{}", b.upper_ms.to_string().replace('.', "_"))
                } else {
                    "over".into()
                };
                out.push_str(&format!("{label} = {}\n", b.count));
            }
        }
        out
    }
}

fn fmt(v: f64) -> String {
    if v.is_finite() {
        format!("{v:.3}")
    } else {
        "nan".into()
    }
}

fn median(vals: &mut [f64]) -> f64 {
    if vals.is_empty() {
        return f64::NAN;
    }
    vals.sort_by(|a, b| a.partial_cmp(b).unwrap());
    vals[vals.len() / 2]
}

fn bucketize(deltas: &[f64]) -> Vec<DeltaBucket> {
    BUCKET_EDGES_MS
        .iter()
        .map(|&upper_ms| DeltaBucket {
            upper_ms,
            count: deltas
                .iter()
                .filter(|&&d| {
                    let lower = BUCKET_EDGES_MS
                        .iter()
                        .take_while(|&&e| e < upper_ms)
                        .last()
                        .copied()
                        .unwrap_or(0.0);
                    d > lower && d <= upper_ms
                })
                .count() as u64,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deadzone_neutral_inside_and_rescaled_outside() {
        assert_eq!(apply_deadzone(0.05, -0.05, 0.12), (0.0, 0.0));
        // Just past the edge: tiny but nonzero, same direction.
        let (x, y) = apply_deadzone(0.2, 0.0, 0.12);
        assert!(x > 0.0 && x < 0.12 && y == 0.0);
        // Full deflection stays full.
        let (x, _) = apply_deadzone(1.0, 0.0, 0.12);
        assert!((x - 1.0).abs() < 1e-12);
        // Diagonal full deflection is clamped to unit magnitude.
        let (x, y) = apply_deadzone(1.0, 1.0, 0.12);
        assert!((x * x + y * y).sqrt() <= 1.0 + 1e-12);
    }

    #[test]
    fn bucketize_partitions_all_deltas() {
        let deltas = [0.3, 0.9, 1.5, 3.0, 7.0, 15.0, 30.0, 100.0];
        let buckets = bucketize(&deltas);
        let total: u64 = buckets.iter().map(|b| b.count).sum();
        assert_eq!(total, deltas.len() as u64);
    }
}
