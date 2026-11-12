//! Latency calibration on record: the newest output-latency measurement and
//! tap-to-beat calibration in `<base_dir>/logs/latency/`, which a realtime
//! session compensates its musical clock by (the conductor's `latency_ms`)
//! and a rhythm layer adds to its judgment offset.
//!
//! Reading the committed logs in `<base_dir>/logs/latency/`: output-latency
//! measurements (`latency-*.toml`, from spikes/latency_harness) and tap-to-beat
//! calibrations (`calibration-*.toml`, from `flint calibrate`). File names
//! embed date/epoch so lexicographic max = newest. Both offsets are echoed
//! into every judgment/session header so runs are reproducible.

use flint_core::vfs;
use std::path::Path;

/// Newest output-latency measurement: `(file_name, ms)`. Prefers the
/// loopback mean, falling back to the driver-reported mean.
pub fn latest_latency_ms(base_dir: &Path) -> Option<(String, f64)> {
    let (name, value) = newest_toml(base_dir, "latency-")?;
    let mean = |table: &str| {
        value
            .get(table)
            .and_then(|t| t.get("mean"))
            .and_then(flint_core::toml_util::toml_f64)
            .filter(|m| m.is_finite())
    };
    let ms = mean("loopback_ms").or_else(|| mean("driver_reported_ms"))?;
    Some((name, ms))
}

/// Newest tap-to-beat calibration: `(file_name, median_ms)`. Positive =
/// the player's taps land late relative to the latency-compensated grid.
pub fn latest_calibration_ms(base_dir: &Path) -> Option<(String, f64)> {
    let (name, value) = newest_toml(base_dir, "calibration-")?;
    let ms = value
        .get("calibration")
        .and_then(|t| t.get("median_ms"))
        .and_then(flint_core::toml_util::toml_f64)
        .filter(|m| m.is_finite())?;
    Some((name, ms))
}

/// Resolved judgment-timing offsets plus the notice lines describing where
/// they came from. One resolver for every live front end (CLI harness and
/// player) so their startup diagnostics can never drift apart again; each
/// front end prints the lines through its own sink/prefix.
pub struct TimingOffsets {
    pub latency_ms: Option<f64>,
    pub calibration_ms: f64,
    pub notices: Vec<String>,
}

/// Read the newest committed latency measurement and tap calibration from
/// `<base_dir>/logs/latency/` (see [`latest_latency_ms`] /
/// [`latest_calibration_ms`]) and describe the outcome.
pub fn resolve_timing_offsets(base_dir: &Path) -> TimingOffsets {
    let mut notices = Vec::new();
    let latency_ms = match latest_latency_ms(base_dir) {
        Some((file, ms)) => {
            notices.push(format!(
                "measured output latency: {ms:.1} ms (compensating; from {file})"
            ));
            Some(ms)
        }
        None => {
            notices.push(
                "measured output latency: NONE ON RECORD — run spikes/latency_harness \
                 and commit its log to logs/latency/"
                    .to_string(),
            );
            None
        }
    };
    let calibration_ms = match latest_calibration_ms(base_dir) {
        Some((file, ms)) => {
            notices.push(format!("tap calibration: {ms:+.1} ms (from {file})"));
            ms
        }
        None => {
            notices.push("tap calibration: none on record (run `flint calibrate`)".to_string());
            0.0
        }
    };
    TimingOffsets {
        latency_ms,
        calibration_ms,
        notices,
    }
}

fn newest_toml(base_dir: &Path, prefix: &str) -> Option<(String, toml::Value)> {
    let dir = base_dir.join("logs/latency");
    // A missing directory is the legitimate "nothing on record" case; any
    // other failure must be LOUD — a corrupt calibration file silently
    // reading as "no calibration" would run the session with a wrong 0 ms
    // offset, corrupting judgment (and any [H] verdict on top of it).
    let entries = match vfs::read_dir(&dir) {
        Ok(e) => e,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return None,
        Err(err) => {
            tracing::warn!(
                "cannot read {}: {err} — treating as no record",
                dir.display()
            );
            return None;
        }
    };
    let mut names: Vec<String> = entries
        .iter()
        .filter_map(|p| p.file_name())
        .map(|n| n.to_string_lossy().into_owned())
        .filter(|n| n.starts_with(prefix) && n.ends_with(".toml"))
        .collect();
    names.sort();
    // Newest first; an unreadable/unparseable file is warned about by name
    // and skipped, falling back to the next-newest record rather than
    // silently pretending nothing exists.
    while let Some(name) = names.pop() {
        let path = dir.join(&name);
        let text = match vfs::read_to_string(&path) {
            Ok(t) => t,
            Err(err) => {
                tracing::warn!(
                    "cannot read {}: {err} — skipping, trying next-newest",
                    path.display()
                );
                continue;
            }
        };
        match text.parse::<toml::Value>() {
            Ok(value) => return Some((name, value)),
            Err(err) => {
                tracing::warn!(
                    "cannot parse {}: {err} — skipping, trying next-newest",
                    path.display()
                );
            }
        }
    }
    None
}
