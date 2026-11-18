//! What every view counts: each scope key's time in each frame, quantiles
//! over frames where a frame a scope didn't run in counts as 0, and the
//! "normal" frames a slow one is held against.

use super::read::{Thread, Trace};
use std::collections::HashMap;

/// One key's share of one frame.
#[derive(Clone, Copy, Default, Debug)]
pub(crate) struct KeyFrame {
    /// Inclusive time, a scope inside one of its own key not counted again.
    pub incl_ns: u64,
    pub self_ns: u64,
    pub calls: u32,
    pub worst_ns: u64,
}

impl KeyFrame {
    pub fn incl_ms(&self) -> f64 {
        self.incl_ns as f64 / 1e6
    }
    pub fn self_ms(&self) -> f64 {
        self.self_ns as f64 / 1e6
    }
}

/// Each frame's keys, by position in `trace.frames`. A scope whose frame
/// isn't one the trace spans (a worker's, closing outside the window) is
/// left out.
pub(crate) fn per_frame(
    trace: &Trace,
    threads: &[&Thread],
    split: bool,
) -> Vec<HashMap<String, KeyFrame>> {
    let pos: HashMap<u32, usize> = trace
        .frames
        .iter()
        .enumerate()
        .map(|(i, f)| (f.frame, i))
        .collect();
    let mut out = vec![HashMap::new(); trace.frames.len()];
    for t in threads {
        for (i, s) in t.scopes.iter().enumerate() {
            let Some(&p) = pos.get(&s.frame) else {
                continue;
            };
            let k: &mut KeyFrame = out[p].entry(s.key(split)).or_default();
            if !t.inside_itself(i, split) {
                k.incl_ns += s.dur_ns;
            }
            k.self_ns += t.self_ns[i];
            // A summed record is its calls, and its longest (#1288)
            k.calls += s.calls();
            k.worst_ns = k.worst_ns.max(s.worst_ns());
        }
    }
    out
}

/// The `q` quantile (0..1, linear between ranks) of `n` values of which
/// `present` are the non-zero ones (any order) and the rest are 0.
pub(crate) fn quantile(present: &[f64], n: usize, q: f64) -> f64 {
    let n = n.max(present.len());
    if n == 0 {
        return 0.0;
    }
    let mut sorted = present.to_vec();
    sorted.sort_by(f64::total_cmp);
    let zeros = n - sorted.len();
    let at = |i: usize| {
        if i < zeros {
            0.0
        } else {
            sorted[i - zeros]
        }
    };
    let p = q.clamp(0.0, 1.0) * (n - 1) as f64;
    let (lo, hi) = (p.floor() as usize, p.ceil() as usize);
    at(lo) + (at(hi) - at(lo)) * (p - lo as f64)
}

/// Mean and standard deviation of `n` values of which `present` are given
/// and the rest are 0.
pub(crate) fn mean_sd(present: &[f64], n: usize) -> (f64, f64) {
    let n = n.max(present.len());
    if n == 0 {
        return (0.0, 0.0);
    }
    let mean = present.iter().sum::<f64>() / n as f64;
    let zeros = (n - present.len()) as f64;
    let ss = present.iter().map(|v| (v - mean).powi(2)).sum::<f64>() + zeros * mean * mean;
    let sd = if n > 1 {
        (ss / (n - 1) as f64).sqrt()
    } else {
        0.0
    };
    (mean, sd)
}

/// The frames a slow frame is compared with, and how they were chosen.
pub(crate) struct Normal {
    /// Positions in `trace.frames`.
    pub positions: Vec<usize>,
    /// Their median frame time.
    pub median_ms: f64,
    /// What was left out, in words.
    pub rule: String,
}

/// Frames over this many times the median frame aren't normal.
pub(crate) const NORMAL_FACTOR: f64 = 2.0;

/// The trace's normal frames: not the first or last (the look-back's
/// edges, whose scopes may be cut), not a hitch the detector found, not
/// over [`NORMAL_FACTOR`] times the median frame, and not `except`.
pub(crate) fn normal_frames(trace: &Trace, except: Option<usize>) -> Normal {
    let n = trace.frames.len();
    let hitch: Vec<usize> = trace.hitch_frames().into_iter().flatten().collect();
    let edge = |i: usize| n > 2 && (i == 0 || i + 1 == n);
    let candidates: Vec<usize> = (0..n)
        .filter(|&i| !edge(i) && !hitch.contains(&i) && Some(i) != except)
        .collect();
    let ms: Vec<f64> = candidates.iter().map(|&i| trace.frames[i].ms()).collect();
    let median = quantile(&ms, ms.len(), 0.5);
    let candidates_n = candidates.len();
    let positions: Vec<usize> = candidates
        .into_iter()
        .filter(|&i| trace.frames[i].ms() <= median * NORMAL_FACTOR)
        .collect();
    let left_slow = candidates_n - positions.len();
    let positions = if positions.is_empty() {
        (0..n).filter(|&i| Some(i) != except).collect()
    } else {
        positions
    };
    let ms: Vec<f64> = positions.iter().map(|&i| trace.frames[i].ms()).collect();
    Normal {
        median_ms: quantile(&ms, ms.len(), 0.5),
        rule: format!(
            "{} of {n} frames: not the first or last, not one of the {} hitch{}, \
             not {left_slow} over {NORMAL_FACTOR}x the median",
            positions.len(),
            hitch.len(),
            if hitch.len() == 1 { "" } else { "es" },
        ),
        positions,
    }
}

/// Each key's value over `positions`, the frames it ran in only.
pub(crate) fn values<'a>(
    frames: &'a [HashMap<String, KeyFrame>],
    positions: &'a [usize],
    pick: impl Fn(&KeyFrame) -> f64 + 'a,
) -> HashMap<&'a str, Vec<f64>> {
    let mut out: HashMap<&str, Vec<f64>> = HashMap::new();
    for &p in positions {
        for (k, v) in &frames[p] {
            out.entry(k.as_str()).or_default().push(pick(v));
        }
    }
    out
}

/// `ms` as a view writes it: two decimals, three under a tenth.
pub(crate) fn ms(v: f64) -> String {
    if v != 0.0 && v.abs() < 0.1 {
        format!("{v:.3}")
    } else {
        format!("{v:.2}")
    }
}

/// A signed delta: `+1.25`, `-0.031`.
pub(crate) fn signed(v: f64) -> String {
    let s = ms(v.abs());
    if v < 0.0 {
        format!("-{s}")
    } else {
        format!("+{s}")
    }
}

/// A number for JSON: three decimals, as the trace writes them.
pub(crate) fn round(v: f64) -> f64 {
    (v * 1000.0).round() / 1000.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_frame_a_scope_missed_counts_as_zero() {
        // 3 of 5 frames ran it: 0, 0, 1, 2, 3
        assert_eq!(quantile(&[3.0, 1.0, 2.0], 5, 0.5), 1.0);
        assert_eq!(quantile(&[3.0, 1.0, 2.0], 5, 0.0), 0.0);
        assert_eq!(quantile(&[3.0, 1.0, 2.0], 5, 1.0), 3.0);
        // halfway between ranks
        assert_eq!(quantile(&[1.0, 2.0], 2, 0.5), 1.5);
        assert_eq!(quantile(&[], 4, 0.9), 0.0);
        let (mean, sd) = mean_sd(&[2.0, 2.0], 4);
        assert_eq!(mean, 1.0);
        assert!((sd - (4.0f64 / 3.0).sqrt()).abs() < 1e-12);
    }

    #[test]
    fn deltas_read_with_their_sign() {
        assert_eq!(signed(1.25), "+1.25");
        assert_eq!(signed(-0.031), "-0.031");
        assert_eq!(ms(0.0), "0.00");
    }
}
