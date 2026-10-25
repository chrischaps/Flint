//! Frame pacing for the `--exit-after` report: how even the frames were,
//! not just how fast on average. A 60 fps mean can hide frames that come
//! 4 ms, 4 ms, 4 ms, then 50 ms, which is judder for anything moved by dt.
//!
//! Two series are kept after `PERF_SKIP_SECS`: the clock's dt (what the
//! game steps by) and the wall time from one `present()` returning to the
//! next (what the swapchain was handed).

use std::time::Instant;

#[derive(Debug, Clone, Default)]
pub(crate) struct Pacing {
    dts_ms: Vec<f32>,
    presents_ms: Vec<f32>,
    last_present: Option<Instant>,
}

impl Pacing {
    /// Once a frame, right after `present()` returns.
    pub(crate) fn record(&mut self, dt: f64, presented: Instant) {
        self.dts_ms.push((dt * 1000.0) as f32);
        if let Some(last) = self.last_present {
            self.presents_ms
                .push(presented.duration_since(last).as_secs_f32() * 1000.0);
        }
        self.last_present = Some(presented);
    }

    pub(crate) fn dt(&self) -> Option<Spread> {
        Spread::of(&self.dts_ms)
    }

    pub(crate) fn present(&self) -> Option<Spread> {
        Spread::of(&self.presents_ms)
    }
}

/// Percentiles of a frame series, in ms, and how many frames were lumps:
/// over one and a half times the median.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Spread {
    pub p50: f32,
    pub p90: f32,
    pub p99: f32,
    pub max: f32,
    pub lumps: usize,
    pub frames: usize,
}

impl Spread {
    pub(crate) fn of(ms: &[f32]) -> Option<Spread> {
        if ms.is_empty() {
            return None;
        }
        let mut sorted = ms.to_vec();
        sorted.sort_by(f32::total_cmp);
        let at = |q: f32| sorted[((sorted.len() - 1) as f32 * q).round() as usize];
        let p50 = at(0.5);
        Some(Spread {
            p50,
            p90: at(0.9),
            p99: at(0.99),
            max: sorted[sorted.len() - 1],
            lumps: ms.iter().filter(|&&m| m > p50 * 1.5).count(),
            frames: ms.len(),
        })
    }
}

impl std::fmt::Display for Spread {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "p50 {:.1} p90 {:.1} p99 {:.1} max {:.1} ms, {} of {} over 1.5x p50",
            self.p50, self.p90, self.p99, self.max, self.lumps, self.frames
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn even_frames_have_no_lumps() {
        let s = Spread::of(&[16.7; 60]).unwrap();
        assert_eq!(s.p50, 16.7);
        assert_eq!(s.p99, 16.7);
        assert_eq!(s.lumps, 0);
    }

    #[test]
    fn bursts_show_as_lumps_at_the_same_mean() {
        // Three 4 ms frames then a 50 ms one: a 60 fps mean, uneven.
        let ms: Vec<f32> = (0..60).map(|i| if i % 4 == 3 { 50.0 } else { 4.0 }).collect();
        let s = Spread::of(&ms).unwrap();
        assert_eq!(s.p50, 4.0);
        assert_eq!(s.max, 50.0);
        assert_eq!(s.lumps, 15);
    }

    #[test]
    fn presents_are_intervals_between_records() {
        let mut p = Pacing::default();
        let t0 = Instant::now();
        p.record(0.016, t0);
        p.record(0.016, t0 + Duration::from_millis(10));
        p.record(0.016, t0 + Duration::from_millis(30));
        assert_eq!(p.dt().unwrap().frames, 3);
        let present = p.present().unwrap();
        assert_eq!(present.frames, 2);
        assert!((present.max - 20.0).abs() < 0.01);
    }

    #[test]
    fn nothing_recorded_is_none() {
        assert!(Pacing::default().dt().is_none());
    }
}
