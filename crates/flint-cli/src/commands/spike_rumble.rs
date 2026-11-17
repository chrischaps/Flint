//! `flint spike-rumble`, hidden since #709: the H1 rumble spike (ADR 0025)
//! is `cargo run -p flint-input-capture --example spike_rumble` now, and
//! this alias goes in a later release. It runs the same spike.

use anyhow::Result;
use std::path::PathBuf;

#[derive(clap::Args)]
pub struct SpikeRumbleArgs {
    /// Directory whose logs/latency/ receives the report (default: cwd)
    #[arg(long)]
    pub base_dir: Option<String>,

    /// Skip the operator-felt tick/thump/grind demo (timing only)
    #[arg(long)]
    pub no_feel: bool,
}

const MOVED: &str = "note: `flint spike-rumble` is \
    `cargo run -p flint-input-capture --example spike_rumble` now; \
    the old name goes in a later release";

pub fn run(args: SpikeRumbleArgs) -> Result<()> {
    eprintln!("{MOVED}");
    let base_dir = args
        .base_dir
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));

    println!("rumble command-path spike (hold the controller)");
    let report = flint_input_capture::rumble::spike_rumble(!args.no_feel)
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    println!("{}", report.summary());

    let path = super::write_latency_report(&base_dir, "rumble", &report.to_toml())?;
    println!("wrote {}", path.display());
    Ok(())
}
