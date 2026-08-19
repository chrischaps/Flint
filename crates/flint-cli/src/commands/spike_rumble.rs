//! H1 rumble spike (ADR 0025): prove force feedback fires on this stack,
//! let the operator feel the three weight-axis prototypes (tick / thump /
//! grind), time both command paths, and commit the report to
//! `logs/latency/` beside the audio-latency and input-granularity spikes.

use anyhow::{Context, Result};
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

pub fn run(args: SpikeRumbleArgs) -> Result<()> {
    let base_dir = args
        .base_dir
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));

    println!("rumble command-path spike (hold the controller)");
    let report =
        flint_input_capture::rumble::spike_rumble(!args.no_feel).map_err(|e| anyhow::anyhow!("{e}"))?;
    println!(
        "direct set_ff_state: median {:.1} us, p99 {:.1} us, max {:.1} us (n={})",
        report.direct.median_us, report.direct.p99_us, report.direct.max_us, report.direct.n
    );
    if let Some(ep) = &report.effect_play {
        println!(
            "Effect::play call:   median {:.1} us (n={}) — plus the ff server's structural \
             0-50 ms tick quantization on top",
            ep.median_us, ep.n
        );
    }

    let dir = base_dir.join("logs/latency");
    std::fs::create_dir_all(&dir).context("creating logs/latency")?;
    let host = std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_else(|_| "host".into());
    let epoch = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let path = dir.join(format!("rumble-{host}-{epoch}.toml"));
    std::fs::write(&path, report.to_toml()).with_context(|| format!("writing {path:?}"))?;
    println!("wrote {}", path.display());
    Ok(())
}
