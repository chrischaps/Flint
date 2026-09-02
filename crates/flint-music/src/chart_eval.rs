//! Chart evaluation: the query side of the beatmap.
//!
//! `Chart` (chart.rs) is the parsed file; `ChartEval` answers the questions
//! judgment asks of it — "what should the lean be at this beat?" and "which
//! pulse windows exist, in samples?". It is immutable and shareable; all
//! per-run mutable state (window consumption, grid cursors) lives in the
//! judgment layer.
//!
//! Semantics pinned here (and by the golden-fixture tests):
//! - A segment between keys `k0` and `k1` is interpolated with **`k0`'s**
//!   interp — the mode a key declares governs the curve *leaving* it.
//! - Before the first key and after the last, the value clamps to that key.
//! - A channel with no keys evaluates to `None`; it contributes no error.
//! - `pulse_window_ms` is a **half-width** (schema doc): the window spans
//!   `center ± half_width`. A per-pulse `window_ms` override wins over the
//!   enclosing section's value.

use crate::chart::Chart;
use crate::conductor::Conductor;
use crate::tempo::ms_to_samples;
use flint_core::{FlintError, Result};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ChannelValue {
    Scalar(f64),
    Vec2([f64; 2]),
}

impl ChannelValue {
    pub fn as_vec2(self) -> Option<[f64; 2]> {
        match self {
            ChannelValue::Vec2(v) => Some(v),
            ChannelValue::Scalar(_) => None,
        }
    }
}

/// A pulse's judgment window, resolved to samples.
#[derive(Debug, Clone, PartialEq)]
pub struct PulseWindow {
    /// Index into the chart's pulse list (stable identity for logs).
    pub index: usize,
    pub kind: String,
    pub beat: f64,
    pub center_sample: i64,
    pub half_width_samples: i64,
    pub strength: Option<f64>,
    pub direction: Option<[f64; 2]>,
}

impl PulseWindow {
    pub fn open(&self) -> i64 {
        self.center_sample - self.half_width_samples
    }
    pub fn close(&self) -> i64 {
        self.center_sample + self.half_width_samples
    }
    pub fn contains(&self, sample: i64) -> bool {
        (self.open()..=self.close()).contains(&sample)
    }
}

pub struct ChartEval {
    /// Per-channel keys sorted by beat. Key: channel name.
    channels: BTreeMap<String, Vec<(f64, Vec<f64>, Interp)>>,
    /// Windows sorted by center sample.
    windows: Vec<PulseWindow>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Interp {
    Linear,
    Hold,
    Smooth,
}

impl ChartEval {
    /// Build from a validated chart. Errors on anything the arithmetic
    /// cannot tolerate (unknown channel/interp, arity mismatch, unordered
    /// keys) — the validator should have caught all of them earlier.
    pub fn new(chart: &Chart, conductor: &Conductor) -> Result<Self> {
        let sample_rate = conductor.tempo().sample_rate() as f64;

        let mut channels: BTreeMap<String, Vec<(f64, Vec<f64>, Interp)>> = BTreeMap::new();
        for key in &chart.curves {
            let arity = crate::CHANNELS
                .iter()
                .find(|(name, _)| *name == key.channel)
                .map(|(_, a)| *a)
                .ok_or_else(|| bad(format!("unknown channel `{}`", key.channel)))?;
            if key.value.len() != arity {
                return Err(bad(format!(
                    "channel `{}` expects {} value(s), got {}",
                    key.channel,
                    arity,
                    key.value.len()
                )));
            }
            let interp = match key.interp.as_str() {
                "linear" => Interp::Linear,
                "hold" => Interp::Hold,
                "smooth" => Interp::Smooth,
                other => return Err(bad(format!("unknown interp `{other}`"))),
            };
            channels.entry(key.channel.clone()).or_default().push((
                key.beat,
                key.value.clone(),
                interp,
            ));
        }
        for (name, keys) in &channels {
            if keys.windows(2).any(|w| w[1].0 <= w[0].0) {
                return Err(bad(format!(
                    "channel `{name}` keys not strictly increasing in beat"
                )));
            }
        }

        let mut windows = Vec::with_capacity(chart.pulses.len());
        for (index, pulse) in chart.pulses.iter().enumerate() {
            let center_sample = conductor.sample_at_beat(pulse.beat);
            let half_width_ms = match pulse.window_ms {
                Some(ms) => ms,
                None => {
                    conductor
                        .section_at_sample(center_sample)
                        .ok_or_else(|| {
                            bad(format!(
                                "pulse at beat {} is outside all sections",
                                pulse.beat
                            ))
                        })?
                        .pulse_window_ms
                }
            };
            let direction = match &pulse.direction {
                Some(d) if d.len() == 2 => Some([d[0], d[1]]),
                Some(d) => {
                    return Err(bad(format!(
                        "pulse at beat {} direction has arity {}",
                        pulse.beat,
                        d.len()
                    )))
                }
                None => None,
            };
            windows.push(PulseWindow {
                index,
                kind: pulse.kind.clone(),
                beat: pulse.beat,
                center_sample,
                half_width_samples: ms_to_samples(half_width_ms, sample_rate),
                strength: pulse.strength,
                direction,
            });
        }
        windows.sort_by_key(|w| w.center_sample);

        Ok(Self { channels, windows })
    }

    /// The channel's target value at a beat position, or `None` for a
    /// channel with no keys in this chart.
    pub fn sample_channel(&self, channel: &str, beat: f64) -> Option<ChannelValue> {
        let keys = self.channels.get(channel)?;
        let (first, last) = (keys.first()?, keys.last()?);
        let values = if beat <= first.0 {
            first.1.clone()
        } else if beat >= last.0 {
            last.1.clone()
        } else {
            // Partition point: index of the first key strictly after `beat`;
            // the governing segment is [idx-1, idx].
            let idx = keys.partition_point(|(b, _, _)| *b <= beat);
            let (b0, v0, interp) = &keys[idx - 1];
            let (b1, v1, _) = &keys[idx];
            let t = (beat - b0) / (b1 - b0);
            let f = match interp {
                Interp::Hold => 0.0,
                Interp::Linear => t,
                Interp::Smooth => (1.0 - (std::f64::consts::PI * t).cos()) / 2.0,
            };
            v0.iter().zip(v1).map(|(a, b)| a + (b - a) * f).collect()
        };
        Some(match values.len() {
            1 => ChannelValue::Scalar(values[0]),
            _ => ChannelValue::Vec2([values[0], values[1]]),
        })
    }

    /// All pulse windows, sorted by center sample.
    pub fn pulse_windows(&self) -> &[PulseWindow] {
        &self.windows
    }

    /// The channel's last key beat, if it has any keys — the end of its
    /// authored content (values clamp beyond it).
    pub fn last_key_beat(&self, channel: &str) -> Option<f64> {
        self.channels.get(channel)?.last().map(|(b, _, _)| *b)
    }

    /// The first authored key strictly after `beat`, or `None` when the
    /// channel has no keys past that point (or doesn't exist). A query
    /// exactly on a key beat returns the *following* key.
    pub fn next_key(&self, channel: &str, beat: f64) -> Option<(f64, ChannelValue)> {
        let keys = self.channels.get(channel)?;
        let idx = keys.partition_point(|(b, _, _)| *b <= beat);
        keys.get(idx).map(|(b, v, _)| {
            let value = match v.len() {
                1 => ChannelValue::Scalar(v[0]),
                _ => ChannelValue::Vec2([v[0], v[1]]),
            };
            (*b, value)
        })
    }

    /// The channel's authored keys as `(beat, value)` pairs, sorted by beat.
    /// Arrival-style judgment treats these as its targets; the interpolation
    /// between them stays a visual/curve concern.
    pub fn channel_keys(&self, channel: &str) -> Vec<(f64, ChannelValue)> {
        self.channels
            .get(channel)
            .map(|keys| {
                keys.iter()
                    .map(|(b, v, _)| {
                        let value = match v.len() {
                            1 => ChannelValue::Scalar(v[0]),
                            _ => ChannelValue::Vec2([v[0], v[1]]),
                        };
                        (*b, value)
                    })
                    .collect()
            })
            .unwrap_or_default()
    }
}

fn bad(msg: String) -> FlintError {
    FlintError::ValidationError(format!("chart_eval: {msg}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::SuiteManifest;

    // 120 BPM 4/4 at 48 kHz: one beat = 24_000 samples.
    fn conductor() -> Conductor {
        let manifest: SuiteManifest = SuiteManifest::parse(
            r#"
schema_version = 0
[suite]
id = "t"
title = "T"
[audio]
sample_rate = 48000
[[tempo]]
sample = 0
bpm = 120.0
time_signature = [4, 4]
[[sections]]
name = "a"
start_sample = 0
pulse_window_ms = 100
[reintegration]
re_entry_sections = ["a"]
lead_bus = "home_theme"
reassembly_bars = 1
[buses.foundation]
silent = true
[buses.harmony]
silent = true
[buses.world_voice]
silent = true
[buses.home_theme]
silent = true
[buses.child_motif]
silent = true
[buses.texture]
silent = true
"#,
        )
        .unwrap();
        Conductor::new(&manifest, None)
    }

    fn eval(chart_toml: &str) -> ChartEval {
        let chart = Chart::parse(chart_toml).unwrap();
        ChartEval::new(&chart, &conductor()).unwrap()
    }

    const HEADER: &str = "schema_version = 0\nsuite = \"t\"\n";

    #[test]
    fn hold_and_linear_and_clamp() {
        let e = eval(&format!(
            r#"{HEADER}
[[curves]]
channel = "lean"
beat = 4.0
value = [1.0, 0.0]
interp = "hold"
[[curves]]
channel = "lean"
beat = 8.0
value = [0.0, 1.0]
interp = "linear"
[[curves]]
channel = "lean"
beat = 12.0
value = [1.0, 1.0]
interp = "linear"
"#
        ));
        // Clamp before first key.
        assert_eq!(
            e.sample_channel("lean", 0.0),
            Some(ChannelValue::Vec2([1.0, 0.0]))
        );
        // Hold spans its whole segment.
        assert_eq!(
            e.sample_channel("lean", 7.999),
            Some(ChannelValue::Vec2([1.0, 0.0]))
        );
        // At the key itself, the key's value.
        assert_eq!(
            e.sample_channel("lean", 8.0),
            Some(ChannelValue::Vec2([0.0, 1.0]))
        );
        // Linear midpoint out of key 8.
        assert_eq!(
            e.sample_channel("lean", 10.0),
            Some(ChannelValue::Vec2([0.5, 1.0]))
        );
        // Clamp after last key.
        assert_eq!(
            e.sample_channel("lean", 99.0),
            Some(ChannelValue::Vec2([1.0, 1.0]))
        );
    }

    #[test]
    fn smooth_is_cosine() {
        let e = eval(&format!(
            r#"{HEADER}
[[curves]]
channel = "pressure_l"
beat = 0.0
value = [0.0]
interp = "smooth"
[[curves]]
channel = "pressure_l"
beat = 8.0
value = [1.0]
interp = "linear"
"#
        ));
        let quarter = (1.0 - (std::f64::consts::PI * 0.25).cos()) / 2.0;
        match e.sample_channel("pressure_l", 2.0) {
            Some(ChannelValue::Scalar(v)) => assert!((v - quarter).abs() < 1e-12),
            other => panic!("unexpected {other:?}"),
        }
        // Midpoint of cosine equals the linear midpoint.
        match e.sample_channel("pressure_l", 4.0) {
            Some(ChannelValue::Scalar(v)) => assert!((v - 0.5).abs() < 1e-12),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn empty_channel_is_none() {
        let e = eval(HEADER);
        assert_eq!(e.sample_channel("lean", 0.0), None);
        assert_eq!(e.sample_channel("nonsense", 0.0), None);
    }

    #[test]
    fn windows_resolve_section_and_override() {
        let e = eval(&format!(
            r#"{HEADER}
[[pulses]]
beat = 2.0
[[pulses]]
beat = 4.0
window_ms = 50
"#
        ));
        let w = e.pulse_windows();
        assert_eq!(w.len(), 2);
        // Section half-width 100 ms = 4_800 samples at 48 kHz.
        assert_eq!(w[0].center_sample, 48_000);
        assert_eq!(w[0].half_width_samples, 4_800);
        assert!(w[0].contains(48_000 - 4_800) && w[0].contains(48_000 + 4_800));
        assert!(!w[0].contains(48_000 + 4_801));
        // Override half-width 50 ms = 2_400 samples.
        assert_eq!(w[1].center_sample, 96_000);
        assert_eq!(w[1].half_width_samples, 2_400);
    }

    #[test]
    fn bad_charts_error() {
        let chart = Chart::parse(&format!(
            "{HEADER}[[curves]]\nchannel = \"bogus\"\nbeat = 0.0\nvalue = [0.0]\ninterp = \"linear\"\n"
        ))
        .unwrap();
        assert!(ChartEval::new(&chart, &conductor()).is_err());

        let chart = Chart::parse(&format!(
            "{HEADER}[[curves]]\nchannel = \"lean\"\nbeat = 0.0\nvalue = [0.0]\ninterp = \"linear\"\n"
        ))
        .unwrap();
        assert!(
            ChartEval::new(&chart, &conductor()).is_err(),
            "arity mismatch"
        );
    }
}
