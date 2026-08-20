//! Tap-to-beat calibration: play the foundation bus solo, collect the
//! player's pulses, and write the median signed offset vs the
//! latency-compensated beat grid to `logs/latency/calibration-*.toml`.
//! Consumed by play-chart as part of the total judgment offset.
//! Undiegetic and ugly by design at this stage (production plan D6).

use anyhow::{bail, Context, Result};
use flint_music::clock_bridge::ClockBridge;
use flint_music::input_stream::InputEvent;
use flint_music::session::RealtimePlayer;
use flint_music::{validate_manifest, validate_manifest_assets, SuiteManifest};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use flint_music::chart_session::latest_latency_ms;

#[derive(clap::Args)]
pub struct CalibrateArgs {
    /// Path to the suite manifest (.suite.toml)
    pub manifest: String,

    /// Directory the manifest's file paths are relative to (default: cwd)
    #[arg(long)]
    pub base_dir: Option<String>,

    /// Number of taps to collect
    #[arg(long, default_value_t = 16)]
    pub taps: u32,
}

pub fn run(args: CalibrateArgs) -> Result<()> {
    let base_dir = args
        .base_dir
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));

    let manifest = SuiteManifest::load(Path::new(&args.manifest))
        .with_context(|| format!("loading {}", args.manifest))?;
    let mut issues = validate_manifest(&manifest);
    issues.extend(validate_manifest_assets(&manifest, &base_dir));
    if !issues.is_empty() {
        for i in &issues {
            eprintln!("{i}");
        }
        bail!("manifest failed validation ({} issue(s))", issues.len());
    }

    let latency = latest_latency_ms(&base_dir);
    let latency_ms = match &latency {
        Some((file, ms)) => {
            println!("compensating measured output latency: {ms:.1} ms (from {file})");
            Some(*ms)
        }
        None => {
            println!("WARNING: no output-latency measurement on record; calibrating without");
            None
        }
    };
    let offset_samples =
        (latency_ms.unwrap_or(0.0) / 1000.0 * manifest.sample_rate as f64).round() as i64;

    let mut player = RealtimePlayer::start(&manifest, &base_dir, latency_ms)
        .map_err(|e| anyhow::anyhow!("{e}"))?;

    // Foundation solo: the pulse-bearing stem is the only thing to tap to.
    let others: Vec<String> = player
        .session
        .mixer
        .states()
        .map(|(n, _)| n.to_string())
        .filter(|n| n != "foundation")
        .collect();
    for name in &others {
        if let Some(bus) = player.session.mixer.bus_mut(name) {
            bus.set_gain_now(-60.0);
        }
    }

    let bridge = ClockBridge::new(manifest.sample_rate);
    let (capture_handle, input_rx) = flint_input_capture::spawn(
        bridge.clone(),
        flint_input_capture::CaptureConfig {
            offset_samples,
            ..Default::default()
        },
    )
    .map_err(|e| anyhow::anyhow!("{e}"))?;

    println!(
        "tap South or the right trigger on the beat, {} times...",
        args.taps
    );

    let sample_rate = manifest.sample_rate as f64;
    let mut errors_ms: Vec<f64> = Vec::new();
    while errors_ms.len() < args.taps as usize {
        std::thread::sleep(Duration::from_millis(2));
        player.session.pump();
        bridge.observe(Instant::now(), player.session.clock_sample());

        while let Ok(ev) = input_rx.try_recv() {
            let InputEvent::Pulse(p) = ev else { continue };
            if p.sample < 0 {
                continue; // pre-roll
            }
            let conductor = &player.session.conductor;
            let beat = conductor.position_at_sample(p.sample).beat;
            let err_ms =
                (p.sample - conductor.sample_at_beat(beat.round())) as f64 / sample_rate * 1000.0;
            errors_ms.push(err_ms);
            println!("  tap {:>2}: {err_ms:+.1} ms", errors_ms.len());
            if errors_ms.len() >= args.taps as usize {
                break;
            }
        }

        if player.session.mixer.all_stopped() {
            bail!(
                "stems ended after {} tap(s); need {} — rerun",
                errors_ms.len(),
                args.taps
            );
        }
    }
    drop(capture_handle);

    let median_ms = median(&mut errors_ms.clone());
    let mad_ms = {
        let mut devs: Vec<f64> = errors_ms.iter().map(|e| (e - median_ms).abs()).collect();
        median(&mut devs)
    };
    println!(
        "calibration: median {median_ms:+.1} ms, MAD {mad_ms:.1} ms over {} taps",
        errors_ms.len()
    );

    let latency_file = latency.map(|(f, _)| f).unwrap_or_else(|| "none".into());
    let contents = format!(
        "# Tap-to-beat calibration (flint calibrate)\n[calibration]\nmedian_ms = {median_ms:.3}\nmad_ms = {mad_ms:.3}\ntaps = {}\nlatency_file_used = \"{latency_file}\"\n",
        errors_ms.len()
    );
    let path = super::write_latency_report(&base_dir, "calibration", &contents)?;
    println!("wrote {}", path.display());
    Ok(())
}

fn median(vals: &mut [f64]) -> f64 {
    vals.sort_by(|a, b| a.partial_cmp(b).unwrap());
    vals[vals.len() / 2]
}
