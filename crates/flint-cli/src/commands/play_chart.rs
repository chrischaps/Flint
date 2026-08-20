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
    judgment_offset_samples, parse_lean_mode, resolve_timing_offsets, ChartSession,
    ChartSessionConfig, Tick,
};
use std::path::{Path, PathBuf};
use std::time::Duration;

#[derive(clap::Args)]
pub struct PlayChartArgs {
    #[command(flatten)]
    pub common: super::common_args::ChartCommonArgs,

    /// Stop after this many bars (default: play to the end)
    #[arg(long)]
    pub bars: Option<u64>,

    /// Coherence config TOML (default: config/coherence.toml if present)
    #[arg(long)]
    pub config: Option<String>,

    /// Run the input-granularity spike for N seconds and exit (no audio)
    #[arg(long)]
    pub spike_input_secs: Option<u64>,

    /// Record the input session to logs/sessions/<NAME>.session.jsonl
    #[arg(long)]
    pub record: Option<String>,

    /// Open a bare visual window (absorbs mapper keystrokes; shows
    /// wordless cues). Console output continues underneath.
    #[arg(long)]
    pub window: bool,

    /// Lean judgment: "arrival" (be at each target on its beat, roll
    /// freely between) or "track" (follow the curve continuously)
    #[arg(long, default_value = "arrival")]
    pub lean_mode: String,

    /// Disintegration ladder TOML (default: config/ladder.toml if present)
    #[arg(long)]
    pub ladder: Option<String>,

    /// Error-gradient TOML (default: config/gradient.toml if present,
    /// else inert)
    #[arg(long)]
    pub gradient: Option<String>,

    /// Haptics TOML (default: config/haptics.toml if present, else
    /// inert — no rumble)
    #[arg(long)]
    pub haptics: Option<String>,

    /// Physical→verb mapping: "prototype" (lean + pulse) or "full"
    /// (adds sway, pressure, press, flick — ADR 0030)
    #[arg(long, default_value = "prototype")]
    pub input_map: String,
}

pub fn run(args: PlayChartArgs) -> Result<()> {
    let base_dir = args
        .common
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
/// printing every startup line exactly where it always printed. The shared
/// bring-up lives in `flint_music::chart_session::resolve_timing_offsets` +
/// `flint_input_capture::attach_session_input` (one wording for the CLI and
/// the player); this front end just prints the lines.
fn open_session(args: &PlayChartArgs, base_dir: &Path) -> Result<ChartSession> {
    let offsets = resolve_timing_offsets(base_dir);
    for n in &offsets.notices {
        println!("{n}");
    }
    let (latency_ms, calibration_ms) = (offsets.latency_ms, offsets.calibration_ms);

    let cfg = ChartSessionConfig {
        manifest: PathBuf::from(&args.common.manifest),
        chart: PathBuf::from(&args.common.chart),
        base_dir: base_dir.to_path_buf(),
        coherence_config: args.config.clone().map(PathBuf::from),
        ladder_config: args.ladder.clone().map(PathBuf::from),
        gradient_config: args.gradient.clone().map(PathBuf::from),
        haptics_config: args.haptics.clone().map(PathBuf::from),
        record: args.record.clone(),
        bars: args.bars,
        lean_mode: parse_lean_mode(&args.lean_mode)
            .with_context(|| format!("parsing lean mode '{}'", args.lean_mode))?,
        latency_ms,
        calibration_ms,
    };
    let (mut session, notices) = ChartSession::open(&cfg)
        .with_context(|| format!("opening chart session ({})", args.common.manifest))?;
    for n in notices {
        println!("{n}");
    }

    // Total judgment offset: what the ear hears, adjusted by the player's
    // own measured tap tendency. The capture thread applies it at the stamp.
    let offset_samples = judgment_offset_samples(latency_ms, calibration_ms, session.sample_rate());
    let verb_map = flint_input_capture::VerbMap::parse(&args.input_map)
        .with_context(|| format!("parsing input map '{}'", args.input_map))?;
    for n in flint_input_capture::attach_session_input(&mut session, verb_map, offset_samples) {
        println!("{n}");
    }
    Ok(session)
}

/// Console front end: 2 ms cadence, line-buffered stdin for control.
fn run_cli(mut session: ChartSession) -> Result<()> {
    let stdin_rx = spawn_stdin_reader();
    loop {
        std::thread::sleep(Duration::from_millis(2));
        let out = session.tick().context("ticking chart session")?;
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
                for n in session
                    .reload_config()
                    .context("reloading session configs")?
                {
                    println!("{n}");
                }
            }
            Err(_) => {}
        }
    }
    for n in session.finish().context("finishing chart session")? {
        println!("{n}");
    }
    Ok(())
}

fn run_spike(base_dir: &Path, secs: u64) -> Result<()> {
    let pads = flint_input_capture::connected_gamepads().context("listing connected gamepads")?;
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
    let report = flint_input_capture::measure_granularity(Duration::from_secs(secs), 1000)
        .context("measuring input granularity")?;
    println!(
        "{} events; receipt median {:.3} ms, driver median {:.3} ms",
        report.events, report.receipt_median_ms, report.driver_median_ms
    );
    let path = super::write_latency_report(base_dir, "input-granularity", &report.to_toml())?;
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
