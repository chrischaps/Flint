//! Input session recording, replay, and synthesis (ADR 0009).
//!
//! Format: one file, `*.session.jsonl` — line 1 is a JSON header (suite,
//! chart, sample rate, both timing offsets, config snapshot, capture
//! params), each further line one event:
//!
//! ```text
//! {"t":"header","schema":0,"suite":"prototype",...}
//! {"t":"lean","sample":12345,"x":0.31,"y":-0.05}
//! {"t":"pulse","sample":98765,"kind":"pulse"}
//! ```
//!
//! `sample` is the compensated suite sample (see `input_stream`), so a
//! replay feeds the identical judgment/coherence code with no clock, no
//! audio, and no input backend. The writer enforces nondecreasing samples
//! and change-compresses lean chatter; the reader re-checks monotonicity so
//! a hand-edited file cannot silently break determinism. Synthetic
//! profiles (perfect / late-biased / neglect) generate the same stream from
//! a chart — the milestone evidence and `--synthetic` both use them.

use crate::chart_eval::{ChannelValue, ChartEval};
use crate::conductor::Conductor;
use crate::input_stream::{InputEvent, LeanSample, PulseEvent};
use flint_core::{FlintError, Result};
use std::io::{BufRead, Write};
use std::path::Path;

/// Lean deltas below this within [`LEAN_MIN_INTERVAL_MS`] are not recorded.
const LEAN_EPSILON: f64 = 1e-3;
const LEAN_MIN_INTERVAL_MS: f64 = 4.0;

#[derive(Debug, Clone, PartialEq)]
pub struct SessionHeader {
    pub schema: i64,
    pub suite: String,
    pub chart: String,
    pub sample_rate: u32,
    pub latency_ms: f64,
    pub calibration_ms: f64,
    /// Opaque snapshots for reproducibility (coherence config, capture
    /// parameters); consumers that understand them may use them.
    pub coherence_config: Option<serde_json::Value>,
    pub capture: Option<serde_json::Value>,
}

impl SessionHeader {
    pub fn to_json(&self) -> serde_json::Value {
        let mut v = serde_json::json!({
            "t": "header", "schema": self.schema,
            "suite": self.suite, "chart": self.chart,
            "sample_rate": self.sample_rate,
            "latency_ms": self.latency_ms, "calibration_ms": self.calibration_ms,
        });
        if let Some(c) = &self.coherence_config {
            v["coherence_config"] = c.clone();
        }
        if let Some(c) = &self.capture {
            v["capture"] = c.clone();
        }
        v
    }

    fn from_json(v: &serde_json::Value) -> Result<Self> {
        let bad = |what: &str| {
            FlintError::ParseError(format!("session header: missing or malformed `{what}`"))
        };
        if v.get("t").and_then(|t| t.as_str()) != Some("header") {
            return Err(bad("t"));
        }
        Ok(Self {
            schema: v.get("schema").and_then(|x| x.as_i64()).ok_or_else(|| bad("schema"))?,
            suite: v
                .get("suite")
                .and_then(|x| x.as_str())
                .map(String::from)
                .ok_or_else(|| bad("suite"))?,
            chart: v
                .get("chart")
                .and_then(|x| x.as_str())
                .map(String::from)
                .unwrap_or_default(),
            sample_rate: v
                .get("sample_rate")
                .and_then(|x| x.as_u64())
                .ok_or_else(|| bad("sample_rate"))? as u32,
            latency_ms: v.get("latency_ms").and_then(|x| x.as_f64()).unwrap_or(0.0),
            calibration_ms: v
                .get("calibration_ms")
                .and_then(|x| x.as_f64())
                .unwrap_or(0.0),
            coherence_config: v.get("coherence_config").cloned(),
            capture: v.get("capture").cloned(),
        })
    }
}

pub struct SessionWriter {
    out: std::io::BufWriter<std::fs::File>,
    last_sample: i64,
    last_lean: Option<LeanSample>,
    written: u64,
}

impl SessionWriter {
    pub fn create(path: &Path, header: &SessionHeader) -> Result<Self> {
        if header.schema != crate::SCHEMA_VERSION {
            return Err(FlintError::ValidationError(format!(
                "session schema {} not writable by this build",
                header.schema
            )));
        }
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let mut out = std::io::BufWriter::new(std::fs::File::create(path)?);
        writeln!(out, "{}", header.to_json())?;
        Ok(Self {
            out,
            last_sample: i64::MIN,
            last_lean: None,
            written: 0,
        })
    }

    /// Record one event. Errors on out-of-order samples; silently skips
    /// lean chatter (tiny delta within the minimum interval).
    pub fn write(&mut self, ev: &InputEvent, sample_rate: u32) -> Result<()> {
        let s = ev.sample();
        if s < self.last_sample {
            return Err(FlintError::ValidationError(format!(
                "session events out of order: {s} after {}",
                self.last_sample
            )));
        }
        let line = match ev {
            InputEvent::Lean(l) => {
                if let Some(prev) = &self.last_lean {
                    let small = (l.x - prev.x).abs() < LEAN_EPSILON
                        && (l.y - prev.y).abs() < LEAN_EPSILON;
                    let soon = ((l.sample - prev.sample) as f64)
                        < LEAN_MIN_INTERVAL_MS / 1000.0 * sample_rate as f64;
                    if small && soon {
                        return Ok(());
                    }
                }
                self.last_lean = Some(*l);
                serde_json::json!({"t": "lean", "sample": l.sample, "x": l.x, "y": l.y})
            }
            InputEvent::Pulse(p) => {
                serde_json::json!({"t": "pulse", "sample": p.sample, "kind": p.kind})
            }
        };
        self.last_sample = s;
        writeln!(self.out, "{line}")?;
        self.written += 1;
        Ok(())
    }

    /// Record a reintegration timeline jump (ADR 0009 amendment, ADR 0014).
    /// Events in a session file are stamped in **raw clock samples** (which
    /// never rewind, keeping the file monotonic); replay reconstructs suite
    /// time as `suite = raw - offset` using the most recent jump record.
    pub fn write_timeline_jump(&mut self, raw_sample: i64, offset: i64) -> Result<()> {
        if raw_sample < self.last_sample {
            return Err(FlintError::ValidationError(format!(
                "timeline jump out of order: {raw_sample} after {}",
                self.last_sample
            )));
        }
        self.last_sample = raw_sample;
        writeln!(
            self.out,
            "{}",
            serde_json::json!({"t": "timeline_jump", "sample": raw_sample, "offset": offset})
        )?;
        self.written += 1;
        Ok(())
    }

    pub fn written(&self) -> u64 {
        self.written
    }

    pub fn flush(&mut self) -> Result<()> {
        self.out.flush()?;
        Ok(())
    }
}

/// Load a session file: header plus events, monotonicity re-checked.
pub fn read_session(path: &Path) -> Result<(SessionHeader, Vec<InputEvent>)> {
    let file = std::fs::File::open(path)?;
    let mut lines = std::io::BufReader::new(file).lines();
    let header_line = lines
        .next()
        .ok_or_else(|| FlintError::ParseError("session file is empty".into()))??;
    let header_json: serde_json::Value = serde_json::from_str(&header_line)
        .map_err(|e| FlintError::ParseError(format!("session header: {e}")))?;
    let header = SessionHeader::from_json(&header_json)?;
    if header.schema != crate::SCHEMA_VERSION {
        return Err(FlintError::ValidationError(format!(
            "session schema {} unknown to this build",
            header.schema
        )));
    }

    let mut events = Vec::new();
    let mut last = i64::MIN;
    for (i, line) in lines.enumerate() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let v: serde_json::Value = serde_json::from_str(&line)
            .map_err(|e| FlintError::ParseError(format!("session line {}: {e}", i + 2)))?;
        let bad =
            |what: &str| FlintError::ParseError(format!("session line {}: `{what}`", i + 2));
        let sample = v.get("sample").and_then(|x| x.as_i64()).ok_or_else(|| bad("sample"))?;
        if sample < last {
            return Err(FlintError::ValidationError(format!(
                "session line {}: sample {sample} after {last}",
                i + 2
            )));
        }
        last = sample;
        let ev = match v.get("t").and_then(|t| t.as_str()) {
            // Reintegration jump records ride along for the reactive replay
            // path (E6); the plain event reader skips them.
            Some("timeline_jump") => continue,
            Some("lean") => InputEvent::Lean(LeanSample {
                sample,
                x: v.get("x").and_then(|x| x.as_f64()).ok_or_else(|| bad("x"))?,
                y: v.get("y").and_then(|x| x.as_f64()).ok_or_else(|| bad("y"))?,
            }),
            Some("pulse") => InputEvent::Pulse(PulseEvent {
                sample,
                kind: v
                    .get("kind")
                    .and_then(|x| x.as_str())
                    .unwrap_or("pulse")
                    .to_string(),
            }),
            other => return Err(FlintError::ParseError(format!(
                "session line {}: unknown event type {other:?}",
                i + 2
            ))),
        };
        events.push(ev);
    }
    Ok((header, events))
}

/// Synthetic play styles for headless evidence and `--synthetic`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SyntheticProfile {
    /// Lean tracks the chart exactly; every window hit dead center.
    Perfect,
    /// Perfect lean; every pulse this many ms late (may exceed windows).
    LateMs(f64),
    /// Neutral stick, no pulses at all.
    Neglect,
}

impl SyntheticProfile {
    /// `perfect` | `late:<ms>` | `neglect`.
    pub fn parse(text: &str) -> Result<Self> {
        match text {
            "perfect" => Ok(Self::Perfect),
            "neglect" => Ok(Self::Neglect),
            other => match other.strip_prefix("late:") {
                Some(ms) => ms
                    .parse::<f64>()
                    .map(Self::LateMs)
                    .map_err(|_| bad_profile(other)),
                None => Err(bad_profile(other)),
            },
        }
    }
}

fn bad_profile(text: &str) -> FlintError {
    FlintError::ParseError(format!(
        "unknown synthetic profile `{text}` (perfect | late:<ms> | neglect)"
    ))
}

/// Lean sampling cadence of synthesized sessions.
const SYNTH_LEAN_HZ: f64 = 200.0;

/// Deterministic event stream for a profile against a chart. Pulses take
/// each window's kind, so press/flick windows are exercised too.
pub fn synthesize(
    eval: &ChartEval,
    conductor: &Conductor,
    profile: SyntheticProfile,
) -> Vec<InputEvent> {
    let sample_rate = conductor.tempo().sample_rate() as f64;
    let end_sample = eval
        .pulse_windows()
        .iter()
        .map(|w| w.close())
        .chain(
            eval.last_key_beat(crate::judgment::TRACKED_CHANNEL)
                .map(|b| conductor.sample_at_beat(b)),
        )
        .max()
        .unwrap_or(0);

    let mut events = Vec::new();
    let step = (sample_rate / SYNTH_LEAN_HZ).round() as i64;
    let mut s = 0;
    while s <= end_sample {
        let (x, y) = match profile {
            SyntheticProfile::Neglect => (0.0, 0.0),
            _ => {
                let beat = conductor.position_at_sample(s).beat;
                match eval.sample_channel(crate::judgment::TRACKED_CHANNEL, beat) {
                    Some(ChannelValue::Vec2([x, y])) => (x, y),
                    _ => (0.0, 0.0),
                }
            }
        };
        events.push(InputEvent::Lean(LeanSample { sample: s, x, y }));
        s += step;
    }

    if !matches!(profile, SyntheticProfile::Neglect) {
        let offset = match profile {
            SyntheticProfile::LateMs(ms) => (ms / 1000.0 * sample_rate).round() as i64,
            _ => 0,
        };
        for w in eval.pulse_windows() {
            events.push(InputEvent::Pulse(PulseEvent {
                sample: w.center_sample + offset,
                kind: w.kind.clone(),
            }));
        }
    }

    events.sort_by_key(|e| e.sample());
    events
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chart::Chart;
    use crate::manifest::SuiteManifest;

    fn setup() -> (ChartEval, Conductor) {
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
        let conductor = Conductor::new(&manifest, None);
        let chart = Chart::parse(
            "schema_version = 0\nsuite = \"t\"\n[[curves]]\nchannel = \"lean\"\nbeat = 0.0\nvalue = [0.2, -0.4]\ninterp = \"linear\"\n[[curves]]\nchannel = \"lean\"\nbeat = 8.0\nvalue = [-0.6, 0.3]\ninterp = \"smooth\"\n[[pulses]]\nbeat = 2.0\n[[pulses]]\nbeat = 4.0\n",
        )
        .unwrap();
        (ChartEval::new(&chart, &conductor).unwrap(), conductor)
    }

    fn header() -> SessionHeader {
        SessionHeader {
            schema: 0,
            suite: "t".into(),
            chart: "test.chart.toml".into(),
            sample_rate: 48_000,
            latency_ms: 120.0,
            calibration_ms: -5.0,
            coherence_config: None,
            capture: None,
        }
    }

    #[test]
    fn writer_reader_round_trip() {
        let (eval, conductor) = setup();
        let events = synthesize(&eval, &conductor, SyntheticProfile::Perfect);
        let path = std::env::temp_dir().join("flint-replay-roundtrip.session.jsonl");
        let mut w = SessionWriter::create(&path, &header()).unwrap();
        for ev in &events {
            w.write(ev, 48_000).unwrap();
        }
        w.flush().unwrap();

        let (h, read) = read_session(&path).unwrap();
        assert_eq!(h, header());
        // Change-compression may drop chatter, but pulses all survive and
        // order is preserved.
        let pulses = |evs: &[InputEvent]| {
            evs.iter()
                .filter(|e| matches!(e, InputEvent::Pulse(_)))
                .count()
        };
        assert_eq!(pulses(&read), pulses(&events));
        assert!(read.windows(2).all(|w| w[0].sample() <= w[1].sample()));
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn writer_rejects_out_of_order() {
        let path = std::env::temp_dir().join("flint-replay-order.session.jsonl");
        let mut w = SessionWriter::create(&path, &header()).unwrap();
        w.write(
            &InputEvent::Pulse(PulseEvent {
                sample: 100,
                kind: "pulse".into(),
            }),
            48_000,
        )
        .unwrap();
        let err = w.write(
            &InputEvent::Pulse(PulseEvent {
                sample: 50,
                kind: "pulse".into(),
            }),
            48_000,
        );
        assert!(err.is_err());
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn reader_rejects_out_of_order() {
        let path = std::env::temp_dir().join("flint-replay-badfile.session.jsonl");
        std::fs::write(
            &path,
            "{\"t\":\"header\",\"schema\":0,\"suite\":\"t\",\"sample_rate\":48000}\n{\"t\":\"pulse\",\"sample\":100}\n{\"t\":\"pulse\",\"sample\":50}\n",
        )
        .unwrap();
        assert!(read_session(&path).is_err());
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn synthesis_is_deterministic_and_profile_shaped() {
        let (eval, conductor) = setup();
        let a = synthesize(&eval, &conductor, SyntheticProfile::Perfect);
        let b = synthesize(&eval, &conductor, SyntheticProfile::Perfect);
        assert_eq!(a, b);

        // Perfect pulses sit exactly on centers.
        let centers: Vec<i64> = eval.pulse_windows().iter().map(|w| w.center_sample).collect();
        let pulse_samples: Vec<i64> = a
            .iter()
            .filter_map(|e| match e {
                InputEvent::Pulse(p) => Some(p.sample),
                _ => None,
            })
            .collect();
        assert_eq!(pulse_samples, centers);

        // Late profile shifts them all by +25 ms = 1_200 samples.
        let late = synthesize(&eval, &conductor, SyntheticProfile::LateMs(25.0));
        let late_samples: Vec<i64> = late
            .iter()
            .filter_map(|e| match e {
                InputEvent::Pulse(p) => Some(p.sample),
                _ => None,
            })
            .collect();
        assert_eq!(
            late_samples,
            centers.iter().map(|c| c + 1_200).collect::<Vec<_>>()
        );

        // Neglect: no pulses, all-neutral lean.
        let neglect = synthesize(&eval, &conductor, SyntheticProfile::Neglect);
        assert!(neglect.iter().all(|e| match e {
            InputEvent::Pulse(_) => false,
            InputEvent::Lean(l) => l.x == 0.0 && l.y == 0.0,
        }));
    }

    #[test]
    fn profile_parsing() {
        assert_eq!(
            SyntheticProfile::parse("perfect").unwrap(),
            SyntheticProfile::Perfect
        );
        assert_eq!(
            SyntheticProfile::parse("late:20").unwrap(),
            SyntheticProfile::LateMs(20.0)
        );
        assert_eq!(
            SyntheticProfile::parse("neglect").unwrap(),
            SyntheticProfile::Neglect
        );
        assert!(SyntheticProfile::parse("flawless").is_err());
    }
}
