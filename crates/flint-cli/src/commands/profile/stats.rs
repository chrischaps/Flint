//! What every view counts: each scope key's time in each frame, and how a
//! view writes a number. The quantiles and the "normal" frames a slow one
//! is held against are the trace reader's (`commands::trace::stats`), the
//! profiler tab's too.

use crate::commands::trace::{Thread, Trace};
use std::collections::HashMap;

pub(crate) use crate::commands::trace::stats::{mean_sd, normal_frames, quantile};

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
            let k: &mut KeyFrame = out[p].entry(trace.key(s, split)).or_default();
            if !trace.inside_itself(t, i, split) {
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
    fn deltas_read_with_their_sign() {
        assert_eq!(signed(1.25), "+1.25");
        assert_eq!(signed(-0.031), "-0.031");
        assert_eq!(ms(0.0), "0.00");
    }
}
