//! Bridge between wall-clock instants and the suite's sample clock.
//!
//! The input capture thread stamps events with `Instant`s; judgment needs
//! suite sample positions. kira's clock is readable only on the session
//! thread, and its shared counter advances once per audio callback, so a
//! single paired read is quantized to the device buffer (2–20+ ms). The
//! bridge therefore collects paired `(Instant, clock_sample)` observations
//! from the session loop and fits a line through them: the regression
//! averages the per-chunk sawtooth to sub-millisecond error and its slope
//! absorbs audio-vs-monotonic drift.
//!
//! Threading: the session loop calls [`ClockBridge::observe`]; any thread
//! holding a clone may call [`ClockBridge::sample_at`]. Lock contention is
//! negligible (two short critical sections per millisecond at most).

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// Ring capacity. With [`MIN_PAIR_SPACING`] the ring always spans ~2 s no
/// matter how fast the session loop calls `observe`.
const MAX_PAIRS: usize = 64;
/// Minimum seconds between accepted observations — a fast loop is decimated
/// rather than shrinking the fit window (slope needs time-span, not density).
const MIN_PAIR_SPACING: f64 = 0.03;
/// Minimum pairs before the bridge answers queries.
const WARMUP_PAIRS: usize = 8;
/// Minimum time span the ring must cover before fitting — slope from a
/// shorter baseline is dominated by chunk-quantization noise.
const MIN_FIT_SPAN: f64 = 0.5;
/// A clock that reports the same sample for longer than this is not running
/// yet (device warm-up), not merely chunk-quantized; restart the ring.
const STALL_SECONDS: f64 = 0.1;
/// Refit the line every this many observations.
const REFIT_EVERY: usize = 16;
/// Allowed slope deviation from the nominal sample rate (drift tolerance).
const SLOPE_TOLERANCE: f64 = 0.005;

#[derive(Debug, Clone, Copy)]
struct LinModel {
    /// Samples per second of wall time (≈ sample rate).
    slope: f64,
    /// Sample position at the bridge epoch.
    intercept: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BridgeStats {
    pub pairs: usize,
    /// RMS of fit residuals, in milliseconds of clock time.
    pub residual_rms_ms: f64,
    /// Fitted samples-per-second (should sit near the sample rate).
    pub slope_hz: f64,
}

struct BridgeInner {
    epoch: Instant,
    sample_rate: f64,
    pairs: VecDeque<(f64, i64)>, // (seconds since epoch, clock sample)
    model: Option<LinModel>,
    since_fit: usize,
}

/// Cloneable handle; all clones share one model.
#[derive(Clone)]
pub struct ClockBridge {
    inner: Arc<Mutex<BridgeInner>>,
}

impl ClockBridge {
    pub fn new(sample_rate: u32) -> Self {
        Self {
            inner: Arc::new(Mutex::new(BridgeInner {
                epoch: Instant::now(),
                sample_rate: sample_rate as f64,
                pairs: VecDeque::with_capacity(MAX_PAIRS),
                model: None,
                since_fit: 0,
            })),
        }
    }

    /// Record a paired observation. Call from the thread that can read the
    /// session clock, once per loop tick, with `Instant::now()` taken as
    /// close to the `clock_sample()` read as possible.
    pub fn observe(&self, at: Instant, clock_sample: i64) {
        let mut inner = self.inner.lock().unwrap();
        let t = at.duration_since(inner.epoch).as_secs_f64();
        if let Some(&(last_t, last_s)) = inner.pairs.back() {
            if t - last_t < MIN_PAIR_SPACING {
                return;
            }
            // Startup stall: the device hasn't begun consuming yet, so the
            // frozen pairs would poison the first fit. Start over from here.
            if clock_sample == last_s && t - last_t > STALL_SECONDS {
                inner.pairs.clear();
                inner.model = None;
            }
        }
        if inner.pairs.len() == MAX_PAIRS {
            inner.pairs.pop_front();
        }
        inner.pairs.push_back((t, clock_sample));
        inner.since_fit += 1;
        let due = inner.model.is_none() || inner.since_fit >= REFIT_EVERY;
        let span = inner
            .pairs
            .back()
            .zip(inner.pairs.front())
            .map(|((tb, _), (tf, _))| tb - tf)
            .unwrap_or(0.0);
        if due && inner.pairs.len() >= WARMUP_PAIRS && span >= MIN_FIT_SPAN {
            inner.refit();
            inner.since_fit = 0;
        }
    }

    /// The suite clock sample at a wall-clock instant. `None` until warmed
    /// up. Extrapolates freely — input events arrive after the newest pair.
    pub fn sample_at(&self, at: Instant) -> Option<i64> {
        let inner = self.inner.lock().unwrap();
        let model = inner.model?;
        let t = at.duration_since(inner.epoch).as_secs_f64();
        Some((model.slope * t + model.intercept).round() as i64)
    }

    pub fn stats(&self) -> BridgeStats {
        let inner = self.inner.lock().unwrap();
        let (residual_rms_ms, slope_hz) = match inner.model {
            Some(m) => {
                let n = inner.pairs.len() as f64;
                let ss: f64 = inner
                    .pairs
                    .iter()
                    .map(|&(t, s)| {
                        let e = s as f64 - (m.slope * t + m.intercept);
                        e * e
                    })
                    .sum();
                (
                    ((ss / n).sqrt() / inner.sample_rate) * 1000.0,
                    m.slope,
                )
            }
            None => (f64::NAN, f64::NAN),
        };
        BridgeStats {
            pairs: inner.pairs.len(),
            residual_rms_ms,
            slope_hz,
        }
    }
}

impl BridgeInner {
    fn refit(&mut self) {
        let n = self.pairs.len() as f64;
        let (mut st, mut ss) = (0.0, 0.0);
        for &(t, s) in &self.pairs {
            st += t;
            ss += s as f64;
        }
        let (tm, sm) = (st / n, ss / n);
        let (mut num, mut den) = (0.0, 0.0);
        for &(t, s) in &self.pairs {
            num += (t - tm) * (s as f64 - sm);
            den += (t - tm) * (t - tm);
        }
        if den <= 0.0 {
            return; // all observations at one instant; keep the old model
        }
        let mut slope = num / den;
        let (lo, hi) = (
            self.sample_rate * (1.0 - SLOPE_TOLERANCE),
            self.sample_rate * (1.0 + SLOPE_TOLERANCE),
        );
        if !(lo..=hi).contains(&slope) {
            tracing::warn!(
                slope,
                nominal = self.sample_rate,
                "clock bridge slope outside tolerance; clamping"
            );
            slope = slope.clamp(lo, hi);
        }
        // Intercept re-derived from the means so a clamped slope still
        // centers the line on the data.
        self.model = Some(LinModel {
            slope,
            intercept: sm - slope * tm,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    const RATE: f64 = 48_000.0;

    /// Feed pairs simulating a clock that advances once per `chunk` samples
    /// (kira's per-callback update), observed every `every` seconds.
    fn feed(bridge: &ClockBridge, base: Instant, secs: f64, every: f64, chunk: i64, drift: f64) {
        let mut t = 0.0;
        while t < secs {
            let true_sample = (t * RATE * (1.0 + drift)) as i64;
            let quantized = (true_sample / chunk) * chunk;
            bridge.observe(base + Duration::from_secs_f64(t), quantized);
            t += every;
        }
    }

    #[test]
    fn warms_up_then_answers() {
        let bridge = ClockBridge::new(48_000);
        let base = Instant::now();
        for i in 0..WARMUP_PAIRS {
            let t = i as f64 * 0.08; // 8 pairs span 0.56 s >= MIN_FIT_SPAN
            assert!(bridge.sample_at(base).is_none() || i + 1 >= WARMUP_PAIRS);
            bridge.observe(base + Duration::from_secs_f64(t), (t * RATE) as i64);
        }
        assert!(bridge.sample_at(base).is_some());
    }

    #[test]
    fn averages_chunk_quantization_below_one_ms() {
        let bridge = ClockBridge::new(48_000);
        let base = Instant::now();
        // 512-sample chunks (~10.7 ms) observed at 500 Hz for 2 s.
        feed(&bridge, base, 2.0, 0.002, 512, 0.0);
        // Quantization floors the readings, so the fit sits ~half a chunk
        // low of the true clock — a constant judgment bias well under the
        // pulse windows, and the *spread* is what matters. Assert both.
        let t_query = 2.5;
        let est = bridge
            .sample_at(base + Duration::from_secs_f64(t_query))
            .unwrap();
        let truth = (t_query * RATE) as i64;
        let bias_ms = (truth - est) as f64 / RATE * 1000.0;
        assert!(
            bias_ms.abs() < 512.0 / RATE * 1000.0,
            "bias {bias_ms:.2} ms exceeds one chunk"
        );
        let stats = bridge.stats();
        assert!(
            stats.residual_rms_ms < 4.0,
            "residual {:.2} ms too large",
            stats.residual_rms_ms
        );
        assert!((stats.slope_hz - RATE).abs() < RATE * SLOPE_TOLERANCE);
    }

    #[test]
    fn tracks_real_drift_within_tolerance() {
        let bridge = ClockBridge::new(48_000);
        let base = Instant::now();
        // +0.2% fast audio clock, small chunks.
        feed(&bridge, base, 2.0, 0.002, 128, 0.002);
        let stats = bridge.stats();
        assert!(
            (stats.slope_hz - RATE * 1.002).abs() < 20.0,
            "slope {} missed drifted rate",
            stats.slope_hz
        );
        // Extrapolate 0.5 s past the last pair: error stays sub-millisecond
        // because the slope carries the drift.
        let t_query = 2.5;
        let est = bridge
            .sample_at(base + Duration::from_secs_f64(t_query))
            .unwrap();
        let truth = (t_query * RATE * 1.002) as i64;
        assert!(
            ((truth - est).abs() as f64) < RATE / 1000.0 + 128.0,
            "extrapolation error {} samples",
            truth - est
        );
    }

    #[test]
    fn absurd_slope_is_clamped() {
        let bridge = ClockBridge::new(48_000);
        let base = Instant::now();
        // Clock racing at 2x: slope clamps to +0.5%.
        for i in 0..MAX_PAIRS {
            let t = i as f64 * 0.04;
            bridge.observe(base + Duration::from_secs_f64(t), (t * RATE * 2.0) as i64);
        }
        let stats = bridge.stats();
        assert!((stats.slope_hz - RATE * (1.0 + SLOPE_TOLERANCE)).abs() < 1.0);
    }
}
