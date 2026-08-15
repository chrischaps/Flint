//! Reading the committed timing-offset logs in `<base_dir>/logs/latency/`:
//! output-latency measurements (`latency-*.toml`, from spikes/latency_harness)
//! and tap-to-beat calibrations (`calibration-*.toml`, from `flint calibrate`).
//! File names embed date/epoch so lexicographic max = newest. Both offsets
//! are echoed into every judgment/session header so runs are reproducible.

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

fn newest_toml(base_dir: &Path, prefix: &str) -> Option<(String, toml::Value)> {
    let dir = base_dir.join("logs/latency");
    let mut names: Vec<String> = std::fs::read_dir(&dir)
        .ok()?
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with(prefix) && n.ends_with(".toml"))
        .collect();
    names.sort();
    let name = names.pop()?;
    let value: toml::Value = std::fs::read_to_string(dir.join(&name)).ok()?.parse().ok()?;
    Some((name, value))
}
