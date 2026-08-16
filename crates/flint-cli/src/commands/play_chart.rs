//! Play a suite against its beatmap chart with live input capture: the
//! Phase 2 dev harness. Thin front end over [`flint_music::ChartSession`]
//! (the reactive loop itself lives in the library — ADR 0016): resolves the
//! timing offsets, opens the session, spawns the 1 kHz capture thread, and
//! prints the session's notice lines.
//!
//! Two front ends share one `ChartSession`: the console loop below, and
//! the `--window` mode in `play_chart_window` (a focused window keeps
//! gamepad-to-keyboard mappers from typing into the terminal, and carries
//! the wordless visual cues).

use anyhow::{Context, Result};
use flint_music::chart_session::{
    judgment_offset_samples, latest_calibration_ms, latest_latency_ms, parse_lean_mode,
    ChartSession, ChartSessionConfig, Tick,
};
use std::path::{Path, PathBuf};
use std::time::Duration;

pub struct PlayChartArgs {
    pub manifest: String,
    pub chart: String,
    pub base_dir: Option<String>,
    pub bars: Option<u64>,
    /// Coherence config path (default: `<base_dir>/config/coherence.toml`
    /// when present, else built-in defaults).
    pub config: Option<String>,
    /// Record the input session to `logs/sessions/<name>.session.jsonl`.
    pub record: Option<String>,
    /// Run the input-granularity spike for this many seconds and exit
    /// (wiggle the stick; no audio involved).
    pub spike_input_secs: Option<u64>,
    /// Open a bare graphical window instead of running console-only.
    pub window: bool,
    /// Lean judgment mode: "arrival" (beat-anchored targets, the current
    /// feel direction) or "track" (continuous curve-following).
    pub lean_mode: String,
    /// Disintegration ladder config path (default: `<base_dir>/config/
    /// ladder.toml` when present, else built-in defaults).
    pub ladder: Option<String>,
}

pub fn run(args: PlayChartArgs) -> Result<()> {
    let base_dir = args
        .base_dir
        .clone()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));

    if let Some(secs) = args.spike_input_secs {
        return run_spike(&base_dir, secs);
    }

    let session = open_session(&args, &base_dir)?;
    if args.window {
        super::play_chart_window::run_windowed(session)
    } else {
        run_cli(session)
    }
}

/// Resolve offsets, open the library session, and attach live capture —
/// printing every startup line exactly where it always printed.
fn open_session(args: &PlayChartArgs, base_dir: &Path) -> Result<ChartSession> {
    // --- offsets ---------------------------------------------------------
    let latency_ms = match latest_latency_ms(base_dir) {
        Some((file, ms)) => {
            println!("measured output latency: {ms:.1} ms (compensating; from {file})");
            Some(ms)
        }
        None => {
            println!(
                "measured output latency: NONE ON RECORD — run spikes/latency_harness \
                 and commit its log to logs/latency/"
            );
            None
        }
    };
    let calibration_ms = match latest_calibration_ms(base_dir) {
        Some((file, ms)) => {
            println!("tap calibration: {ms:+.1} ms (from {file})");
            ms
        }
        None => {
            println!("tap calibration: none on record (run `flint calibrate`)");
            0.0
        }
    };

    let cfg = ChartSessionConfig {
        manifest: PathBuf::from(&args.manifest),
        chart: PathBuf::from(&args.chart),
        base_dir: base_dir.to_path_buf(),
        coherence_config: args.config.clone().map(PathBuf::from),
        ladder_config: args.ladder.clone().map(PathBuf::from),
        record: args.record.clone(),
        bars: args.bars,
        lean_mode: parse_lean_mode(&args.lean_mode).map_err(|e| anyhow::anyhow!("{e}"))?,
        latency_ms,
        calibration_ms,
    };
    let (mut session, notices) = ChartSession::open(&cfg).map_err(|e| anyhow::anyhow!("{e}"))?;
    for n in notices {
        println!("{n}");
    }

    // Total judgment offset: what the ear hears, adjusted by the player's
    // own measured tap tendency. The capture thread applies it at the stamp.
    let offset_samples =
        judgment_offset_samples(latency_ms, calibration_ms, session.sample_rate());
    let capture = flint_input_capture::spawn(
        session.bridge(),
        flint_input_capture::CaptureConfig {
            offset_samples,
            ..Default::default()
        },
    );
    match capture {
        Ok((handle, rx)) => {
            println!("gamepad capture running (left stick = lean, South/RT = pulse)");
            // The backend can be alive with zero devices (ADR 0011's
            // silent failure — e.g. a mapper presenting the pad as
            // keyboard/mouse). Make that loud BEFORE the session.
            match flint_input_capture::connected_gamepads() {
                Ok(pads) if pads.is_empty() => {
                    println!(
                        "  *** WARNING: NO GAMEPADS VISIBLE — the session will receive no \
                         input.\n  *** On Windows only XInput-class devices are seen; check \
                         the controller/mapper mode (e.g. Legion Space)."
                    );
                }
                Ok(pads) => println!("  gamepad(s): {}", pads.join(", ")),
                Err(_) => {}
            }
            session.set_input(rx, Box::new(handle));
        }
        Err(e) => {
            println!("gamepad capture unavailable ({e}); playing without input");
        }
    }
    Ok(session)
}

/// Console front end: 2 ms cadence, line-buffered stdin for control.
fn run_cli(mut session: ChartSession) -> Result<()> {
    let stdin_rx = spawn_stdin_reader();
    loop {
        std::thread::sleep(Duration::from_millis(2));
        let out = session.tick().map_err(|e| anyhow::anyhow!("{e}"))?;
        for n in &out.notices {
            println!("{n}");
        }
        if out.state == Tick::Finished {
            break;
        }
        match stdin_rx.try_recv() {
            Ok(StdinCommand::Quit) => {
                println!("quit requested");
                break;
            }
            Ok(StdinCommand::Reload) => {
                for n in session.reload_config().map_err(|e| anyhow::anyhow!("{e}"))? {
                    println!("{n}");
                }
            }
            Err(_) => {}
        }
    }
    for n in session.finish().map_err(|e| anyhow::anyhow!("{e}"))? {
        println!("{n}");
    }
    Ok(())
}

fn run_spike(base_dir: &Path, secs: u64) -> Result<()> {
    let pads = flint_input_capture::connected_gamepads().map_err(|e| anyhow::anyhow!("{e}"))?;
    if pads.is_empty() {
        eprintln!(
            "WARNING: the input backend sees NO gamepads — the spike will record zero \
             events.\nOn Windows this build uses the XInput backend; DirectInput-only \
             pads (e.g. DualShock/DualSense without a mapper) are invisible to it."
        );
    } else {
        println!("gamepad(s) visible: {}", pads.join(", "));
    }
    println!("input-granularity spike: wiggle the stick for {secs} s...");
    let report =
        flint_input_capture::measure_granularity(Duration::from_secs(secs), 1000)
            .map_err(|e| anyhow::anyhow!("{e}"))?;
    println!(
        "{} events; receipt median {:.3} ms, driver median {:.3} ms",
        report.events, report.receipt_median_ms, report.driver_median_ms
    );
    let dir = base_dir.join("logs/latency");
    std::fs::create_dir_all(&dir).context("creating logs/latency")?;
    let host = std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_else(|_| "host".into());
    let epoch = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let path = dir.join(format!("input-granularity-{host}-{epoch}.toml"));
    std::fs::write(&path, report.to_toml()).with_context(|| format!("writing {path:?}"))?;
    println!("wrote {}", path.display());
    Ok(())
}

enum StdinCommand {
    Reload,
    Quit,
}

/// Line-buffered stdin on its own thread: `r` + Enter reloads the coherence
/// config, `q` + Enter quits, anything else (including bare Enter) is ignored
/// — gamepad-to-keyboard mappers (Steam's desktop layout maps A → Enter) spam
/// stray keystrokes into the console during play, so every command needs a
/// deliberate letter. Line-buffered beats raw mode here — no extra deps, no
/// terminal-state cleanup, and a dev harness doesn't need better.
fn spawn_stdin_reader() -> std::sync::mpsc::Receiver<StdinCommand> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::Builder::new()
        .name("play-chart-stdin".into())
        .spawn(move || {
            let stdin = std::io::stdin();
            let mut line = String::new();
            loop {
                line.clear();
                use std::io::BufRead;
                if stdin.lock().read_line(&mut line).unwrap_or(0) == 0 {
                    return; // EOF (piped/headless): no interactive control
                }
                let cmd = match line.trim() {
                    "q" | "quit" => Some(StdinCommand::Quit),
                    "r" | "reload" => Some(StdinCommand::Reload),
                    _ => None, // stray input (gamepad mappers, accidental Enter)
                };
                if let Some(cmd) = cmd {
                    if tx.send(cmd).is_err() {
                        return;
                    }
                }
            }
        })
        .ok();
    rx
}
