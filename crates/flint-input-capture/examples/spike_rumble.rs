//! H1 rumble spike (ADR 0025): prove force feedback fires on this stack,
//! let the operator feel the three weight-axis prototypes (tick / thump /
//! grind), time both command paths, and write the report to
//! `logs/latency/` beside the audio-latency and input-granularity spikes.
//!
//! ```text
//! cargo run -p flint-input-capture --example spike_rumble -- [--base-dir DIR] [--no-feel]
//! ```
//!
//! An experiment, not a tool a game needs, so it lives here rather than as
//! a `flint` command (#709; it was `flint spike-rumble`, a hidden alias
//! for one release until #1141). The report is `rumble-<host>-<epoch>.toml`, the
//! naming every report in `logs/latency/` has (the name sorts newest last).
#![allow(clippy::disallowed_methods)] // an example: writes its report to the disk, not a runtime read

#[cfg(not(target_arch = "wasm32"))]
fn main() -> Result<(), String> {
    let mut base_dir = std::path::PathBuf::from(".");
    let mut feel = true;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--base-dir" => base_dir = args.next().ok_or("--base-dir needs a directory")?.into(),
            "--no-feel" => feel = false,
            "-h" | "--help" => {
                println!(
                    "spike_rumble [--base-dir DIR] [--no-feel]\n  \
                     --base-dir  directory whose logs/latency/ receives the report (default: .)\n  \
                     --no-feel   skip the operator-felt tick/thump/grind demo (timing only)"
                );
                return Ok(());
            }
            other => return Err(format!("unknown argument `{other}` (try --help)")),
        }
    }

    println!("rumble command-path spike (hold the controller)");
    let report = flint_input_capture::rumble::spike_rumble(feel).map_err(|e| e.to_string())?;
    println!("{}", report.summary());

    let dir = base_dir.join("logs/latency");
    std::fs::create_dir_all(&dir).map_err(|e| format!("creating {}: {e}", dir.display()))?;
    let host = std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_else(|_| "host".into());
    let epoch = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let path = dir.join(format!("rumble-{host}-{epoch}.toml"));
    std::fs::write(&path, report.to_toml())
        .map_err(|e| format!("writing {}: {e}", path.display()))?;
    println!("wrote {}", path.display());
    Ok(())
}

/// A browser has no motors to write to (ADR 0025).
#[cfg(target_arch = "wasm32")]
fn main() {}
