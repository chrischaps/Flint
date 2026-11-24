//! The arithmetic over frames both readers' views share: quantiles where a
//! frame a scope didn't run in counts as 0, and the "normal" frames a slow
//! one is held against, which `flint profile explain` and the profiler
//! tab's Compare both take their medians over (#1287).

use super::Trace;

/// The `q` quantile (0..1, linear between ranks) of `n` values of which
/// `present` are the non-zero ones (any order) and the rest are 0.
pub fn quantile(present: &[f64], n: usize, q: f64) -> f64 {
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
pub fn mean_sd(present: &[f64], n: usize) -> (f64, f64) {
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
#[derive(Clone, Debug, Default)]
pub struct Normal {
    /// Positions in `trace.frames`, by start.
    pub positions: Vec<usize>,
    /// Their median frame time.
    pub median_ms: f64,
    /// What was left out, in words.
    pub rule: String,
}

/// Frames over this many times the median frame aren't normal.
pub const NORMAL_FACTOR: f64 = 2.0;

/// The trace's normal frames, from those its scopes cover: not the first
/// or last (the look-back's edges, whose scopes may be cut), not a hitch
/// the detector found, not over [`NORMAL_FACTOR`] times the median frame,
/// and not `except` (the frame asked about, so a normal frame isn't held
/// against itself).
pub fn normal_frames(trace: &Trace, except: Option<usize>) -> Normal {
    let covered = trace.covered();
    let n = covered.len();
    let hitch: Vec<usize> = trace
        .hitch_frames()
        .into_iter()
        .flatten()
        .filter(|f| covered.contains(f))
        .collect();
    let edge = |i: usize| n > 2 && (i == covered.start || i + 1 == covered.end);
    let candidates: Vec<usize> = covered
        .clone()
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
        covered.filter(|&i| Some(i) != except).collect()
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
}
