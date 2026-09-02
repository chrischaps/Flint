//! Programmatic ears: pure analysis functions over rendered audio buffers.
//!
//! These are the assertions the offline render harness makes available —
//! per-tone envelopes (Goertzel), level-transition location, and click
//! detection — so seam and scheduling checks are automated tests instead of
//! listening tasks. All functions take interleaved stereo f32 as produced by
//! [`crate::offline::render_offline`], where index 0 = suite sample 0.

/// Power of a single frequency over `[from, to)` (suite samples), via the
/// Goertzel algorithm on the mono sum. Cheap: one pass, no FFT.
pub fn goertzel_power(
    interleaved: &[f32],
    sample_rate: u32,
    freq: f64,
    from: usize,
    to: usize,
) -> f64 {
    let frames = interleaved.len() / 2;
    let (from, to) = (from.min(frames), to.min(frames));
    if to <= from {
        return 0.0;
    }
    let n = to - from;
    let w = std::f64::consts::TAU * freq / sample_rate as f64;
    let coeff = 2.0 * w.cos();
    let (mut s1, mut s2) = (0.0f64, 0.0);
    for i in from..to {
        let x = (interleaved[i * 2] + interleaved[i * 2 + 1]) as f64 * 0.5;
        let s0 = x + coeff * s1 - s2;
        s2 = s1;
        s1 = s0;
    }
    let power = s1 * s1 + s2 * s2 - coeff * s1 * s2;
    power / (n as f64 * n as f64 / 4.0) // normalize ~ amplitude^2
}

/// Per-window power of one frequency along the whole buffer: element `i`
/// covers suite samples `[i*window, (i+1)*window)`.
pub fn tone_envelope(interleaved: &[f32], sample_rate: u32, freq: f64, window: usize) -> Vec<f64> {
    let frames = interleaved.len() / 2;
    (0..frames / window)
        .map(|i| goertzel_power(interleaved, sample_rate, freq, i * window, (i + 1) * window))
        .collect()
}

/// Locate level transitions of one tone: suite samples where its windowed
/// power first crosses below (drop) or above (rise) the midpoint between the
/// local levels on either side. Resolution is one window.
pub fn find_tone_transitions(envelope: &[f64], window: usize) -> Vec<(i64, TransitionKind)> {
    // Reference level: median-ish via max of the envelope.
    let peak = envelope.iter().cloned().fold(0.0f64, f64::max);
    if peak <= 0.0 {
        return vec![];
    }
    let hi = peak * 0.5; // above = "on"
    let lo = peak * 0.01; // below = "off" (-20 dB)
    let mut out = Vec::new();
    let mut state_on = envelope.first().map(|&e| e > hi).unwrap_or(false);
    for (i, &e) in envelope.iter().enumerate() {
        if state_on && e < lo {
            state_on = false;
            out.push(((i * window) as i64, TransitionKind::Drop));
        } else if !state_on && e > hi {
            state_on = true;
            out.push(((i * window) as i64, TransitionKind::Rise));
        }
    }
    out
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransitionKind {
    Drop,
    Rise,
}

/// Max per-channel sample-to-sample step — the cheap click/discontinuity
/// detector. Scripted changes always ramp (>= a few ms), so any step above
/// the content's natural slew is a genuine discontinuity.
pub fn max_step(interleaved: &[f32]) -> f32 {
    let mut max = 0.0f32;
    for ch in 0..2 {
        let mut prev: Option<f32> = None;
        for frame in interleaved.chunks_exact(2) {
            let x = frame[ch];
            if let Some(p) = prev {
                max = max.max((x - p).abs());
            }
            prev = Some(x);
        }
    }
    max
}

/// Peak absolute sample value.
pub fn peak(interleaved: &[f32]) -> f32 {
    interleaved.iter().fold(0.0f32, |m, &x| m.max(x.abs()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn goertzel_finds_its_tone() {
        let sr = 48_000u32;
        let mut buf = Vec::new();
        for i in 0..sr as usize {
            let t = i as f64 / sr as f64;
            let v = (std::f64::consts::TAU * 1000.0 * t).sin() as f32 * 0.5;
            buf.push(v);
            buf.push(v);
        }
        let on = goertzel_power(&buf, sr, 1000.0, 0, 48_000);
        let off = goertzel_power(&buf, sr, 3000.0, 0, 48_000);
        assert!(on > 0.2, "tone power {on}");
        assert!(off < on / 1000.0, "off-tone power {off} vs {on}");
    }

    #[test]
    fn transitions_locate_a_gated_tone() {
        let sr = 48_000u32;
        let window = 64usize;
        // 1 kHz tone, silent in the middle third [16000, 32000).
        let mut buf = Vec::new();
        for i in 0..48_000usize {
            let t = i as f64 / sr as f64;
            let amp = if (16_000..32_000).contains(&i) {
                0.0
            } else {
                0.5
            };
            let v = (std::f64::consts::TAU * 1000.0 * t).sin() as f32 * amp as f32;
            buf.push(v);
            buf.push(v);
        }
        let env = tone_envelope(&buf, sr, 1000.0, window);
        let tr = find_tone_transitions(&env, window);
        assert_eq!(tr.len(), 2, "{tr:?}");
        assert_eq!(tr[0].1, TransitionKind::Drop);
        assert!(
            (tr[0].0 - 16_000).unsigned_abs() as usize <= 2 * window,
            "{tr:?}"
        );
        assert_eq!(tr[1].1, TransitionKind::Rise);
        assert!(
            (tr[1].0 - 32_000).unsigned_abs() as usize <= 2 * window,
            "{tr:?}"
        );
    }

    #[test]
    fn max_step_catches_a_click() {
        let mut buf = vec![0.0f32; 2000];
        assert!(max_step(&buf) == 0.0);
        buf[1000] = 0.9; // hard discontinuity on the left channel
        assert!(max_step(&buf) >= 0.9);
    }
}
