//! Rumble (force feedback) — the H1 command-path spike.
//!
//! Two candidate paths from code to the motors on this stack:
//!
//! - **gilrs high-level `ff::Effect`**: `play()` sends a message to gilrs's
//!   ff server thread, which processes its queue on a fixed 50 ms sleep loop
//!   (`gilrs::ff::TICK_DURATION`, `ff/server.rs`) — so an effect starts on
//!   the *next* server tick: uniform 0–50 ms added latency. Structural, not
//!   measurable from the call site (motor state is write-only), so it is
//!   cited here from source rather than timed.
//! - **`gilrs_core::FfDevice::set_ff_state`**: a direct synchronous
//!   `XInputSetState` call (gilrs-core `windows_xinput/ff.rs`). No queue,
//!   no server tick.
//!
//! A beat-entrainment tick cannot afford a uniform 0–50 ms jitter (the whole
//! point is tightening a ~36 ms pulse sd), which is why the spike times both
//! call sites: if the direct call is cheap enough to sit inside the 1 kHz
//! capture loop, the rumble service (H2) synthesizes its own envelopes there
//! and never touches the ff server.
//!
//! Actuator spin-up (~20–50 ms for ERM motors) is invisible to software on
//! every path; it is absorbed by the feel-tuned `lead_ms` config knob
//! (ADR 0025).

use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::time::{Duration, Instant};

// ---------------------------------------------------------------------------
// H2: the capture-thread rumble service (ADR 0026)
// ---------------------------------------------------------------------------

/// One motor burst: magnitudes in [0, 1] (weak = high-frequency motor, the
/// light "tick" weight; strong = low-frequency motor, the heavy "thump" /
/// "grind" weight), rectangular envelope — ERM motors bring their own
/// attack/decay, and rectangles keep the engine deterministic.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Burst {
    pub strong: f64,
    pub weak: f64,
    pub duration_samples: i64,
}

/// Commands into the capture thread's rumble engine. All times are **raw
/// clock samples** (the [`ClockBridge`]'s native domain — the sender adds
/// the suite→raw `timeline_offset`), all durations samples: the capture
/// crate never learns the sample rate or the seam bookkeeping; `Flush`
/// covers timeline jumps.
#[derive(Debug, Clone, Copy)]
pub enum RumbleCommand {
    /// Fire `burst` when the raw clock reaches `at_clock_sample - lead`.
    Schedule { at_clock_sample: i64, burst: Burst },
    /// Play `burst` on the next engine tick: no lead, no late-drop. For
    /// felt confirmations of things that already happened (the pulse-hit
    /// thump) — as fast as the motors can, never dropped as "late".
    Fire { burst: Burst },
    /// Sustained motor state (rewind grind / pickup swell); overwrites the
    /// previous continuous state, `(0, 0)` stops. Applied immediately.
    SetContinuous { strong: f64, weak: f64 },
    /// Drop every scheduled burst, end active ones, stop the continuous
    /// state. Sent at a seam (scheduled ticks resolved against the old
    /// timeline are wrong after the jump) and at session teardown.
    Flush,
    /// Fire early by this many samples (the feel-tuned `lead_ms` knob,
    /// converted by the sender; hot-reload re-sends it).
    SetLead { samples: i64 },
    /// Drop a burst arriving later than this past its fire point instead of
    /// playing it — a late tick fights the ear and is worse than none.
    SetLateDrop { samples: i64 },
    /// Master gain in [0, 1]; 0 silences everything.
    SetGain(f64),
}

#[derive(Debug, Clone, Copy)]
struct Scheduled {
    fire_at: i64, // raw target sample; the lead is applied at pop time
    seq: u64,
    burst: Burst,
}

impl PartialEq for Scheduled {
    fn eq(&self, other: &Self) -> bool {
        (self.fire_at, self.seq) == (other.fire_at, other.seq)
    }
}
impl Eq for Scheduled {}
impl PartialOrd for Scheduled {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for Scheduled {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        (self.fire_at, self.seq).cmp(&(other.fire_at, other.seq))
    }
}

#[derive(Debug, Clone, Copy)]
struct Active {
    ends_at: i64,
    strong: f64,
    weak: f64,
}

/// Desired motor state as full-scale u16 magnitudes (XInput's unit).
pub type MotorState = (u16, u16);

/// Pure rumble state machine: commands in, `set_ff_state` writes out.
/// Owns no hardware — the capture loop feeds it the bridged clock sample
/// each 1 kHz tick and performs the returned write; tests drive it with
/// plain integers (the `scheduler.rs` resolved-once pattern, motor-shaped).
#[derive(Debug)]
pub struct RumbleEngine {
    queue: BinaryHeap<Reverse<Scheduled>>,
    /// `Fire` bursts awaiting the next tick (which knows `now`).
    immediate: Vec<Burst>,
    active: Vec<Active>,
    continuous: (f64, f64),
    gain: f64,
    lead: i64,
    late_drop: i64,
    seq: u64,
    last: MotorState,
    /// Lifetime evidence counters (ADR 0011's lesson: a silent haptics path
    /// must be diagnosable): commands in, bursts fired/dropped, writes out.
    stats: RumbleStats,
}

/// Evidence counters, logged by the capture loop at exit.
#[derive(Debug, Clone, Copy, Default)]
pub struct RumbleStats {
    pub commands: u64,
    pub fired: u64,
    pub dropped_late: u64,
    pub writes: u64,
}

impl Default for RumbleEngine {
    fn default() -> Self {
        Self {
            queue: BinaryHeap::new(),
            immediate: Vec::new(),
            active: Vec::new(),
            continuous: (0.0, 0.0),
            gain: 1.0,
            lead: 0,
            // ~50 ms at 48 kHz — harmless before the sender's own value
            // arrives (it converts from config and the real sample rate).
            late_drop: 2400,
            seq: 0,
            last: (0, 0),
            stats: RumbleStats::default(),
        }
    }
}

impl RumbleEngine {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn stats(&self) -> RumbleStats {
        self.stats
    }

    pub fn push(&mut self, cmd: RumbleCommand) {
        self.stats.commands += 1;
        match cmd {
            RumbleCommand::Schedule {
                at_clock_sample,
                burst,
            } => {
                self.seq += 1;
                self.queue.push(Reverse(Scheduled {
                    fire_at: at_clock_sample,
                    seq: self.seq,
                    burst,
                }));
            }
            RumbleCommand::Fire { burst } => self.immediate.push(burst),
            RumbleCommand::SetContinuous { strong, weak } => {
                self.continuous = (strong.clamp(0.0, 1.0), weak.clamp(0.0, 1.0));
            }
            RumbleCommand::Flush => {
                self.queue.clear();
                self.immediate.clear();
                self.active.clear();
                self.continuous = (0.0, 0.0);
            }
            RumbleCommand::SetLead { samples } => self.lead = samples.max(0),
            RumbleCommand::SetLateDrop { samples } => self.late_drop = samples.max(0),
            RumbleCommand::SetGain(g) => self.gain = g.clamp(0.0, 1.0),
        }
    }

    /// Advance to the bridged clock sample `now`; returns the motor state to
    /// write when it changed since the last write, else `None`.
    pub fn tick(&mut self, now: i64) -> Option<MotorState> {
        for burst in self.immediate.drain(..) {
            self.stats.fired += 1;
            self.active.push(Active {
                ends_at: now + burst.duration_samples.max(0),
                strong: burst.strong.clamp(0.0, 1.0),
                weak: burst.weak.clamp(0.0, 1.0),
            });
        }
        while let Some(Reverse(head)) = self.queue.peek().copied() {
            let due = head.fire_at - self.lead;
            if due > now {
                break;
            }
            self.queue.pop();
            if now - due > self.late_drop {
                self.stats.dropped_late += 1;
                tracing::warn!(
                    "rumble burst {} samples late (> {}); dropped — a late tick is worse than none",
                    now - due,
                    self.late_drop
                );
                continue;
            }
            self.stats.fired += 1;
            self.active.push(Active {
                ends_at: now + head.burst.duration_samples.max(0),
                strong: head.burst.strong.clamp(0.0, 1.0),
                weak: head.burst.weak.clamp(0.0, 1.0),
            });
        }
        self.active.retain(|a| a.ends_at > now);

        let (mut strong, mut weak) = self.continuous;
        for a in &self.active {
            strong += a.strong;
            weak += a.weak;
        }
        let state = (
            (strong.min(1.0) * self.gain * f64::from(u16::MAX)) as u16,
            (weak.min(1.0) * self.gain * f64::from(u16::MAX)) as u16,
        );
        if state != self.last {
            self.last = state;
            self.stats.writes += 1;
            Some(state)
        } else {
            None
        }
    }

    /// The unconditional all-off write for teardown paths.
    pub fn silence(&mut self) -> MotorState {
        self.queue.clear();
        self.immediate.clear();
        self.active.clear();
        self.continuous = (0.0, 0.0);
        self.last = (0, 0);
        (0, 0)
    }
}

/// The standard sink adapter for [`crate::spawn_with_rumble`] callers: maps
/// flint-music's [`flint_music::HapticEvent`]s (already raw-clock addressed
/// by `ChartSession`) onto the rumble channel. Send failures are ignored —
/// the capture thread being gone means the session is tearing down, and its
/// exit path has already silenced the motors.
pub fn haptic_sink(
    tx: std::sync::mpsc::Sender<RumbleCommand>,
) -> Box<dyn FnMut(flint_music::HapticEvent) + Send> {
    use flint_music::HapticEvent;
    Box::new(move |ev| {
        let _ = match ev {
            HapticEvent::Burst {
                at_suite_sample,
                strong,
                weak,
                duration_samples,
            } => tx.send(RumbleCommand::Schedule {
                at_clock_sample: at_suite_sample,
                burst: Burst {
                    strong,
                    weak,
                    duration_samples,
                },
            }),
            HapticEvent::Immediate {
                strong,
                weak,
                duration_samples,
            } => tx.send(RumbleCommand::Fire {
                burst: Burst {
                    strong,
                    weak,
                    duration_samples,
                },
            }),
            HapticEvent::Continuous { strong, weak } => {
                tx.send(RumbleCommand::SetContinuous { strong, weak })
            }
            HapticEvent::Flush => tx.send(RumbleCommand::Flush),
            HapticEvent::Config {
                lead_samples,
                late_drop_samples,
                gain,
            } => {
                let _ = tx.send(RumbleCommand::SetLead {
                    samples: lead_samples,
                });
                let _ = tx.send(RumbleCommand::SetLateDrop {
                    samples: late_drop_samples,
                });
                tx.send(RumbleCommand::SetGain(gain))
            }
        };
    })
}

/// Acquire an ff device for the capture loop: first connected ff-capable
/// pad, or a loud warning and `None` (commands become no-ops — rumble must
/// never take the input path down with it).
pub(crate) fn ff_device_or_warn() -> Option<gilrs_core::FfDevice> {
    match open_ff_device() {
        Ok((pad, dev)) => {
            tracing::info!("rumble motors on: {pad}");
            Some(dev)
        }
        Err(e) => {
            tracing::warn!("rumble unavailable ({e}); haptic commands will be ignored");
            None
        }
    }
}

// ---------------------------------------------------------------------------
// H1: the command-path spike (ADR 0025)
// ---------------------------------------------------------------------------

/// Call-duration statistics in microseconds.
#[derive(Debug, Clone)]
pub struct CallStats {
    pub n: u64,
    pub mean_us: f64,
    pub median_us: f64,
    pub p99_us: f64,
    pub max_us: f64,
}

fn call_stats(samples: &[f64]) -> CallStats {
    let mut v: Vec<f64> = samples.to_vec();
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let n = v.len();
    let pick = |q: f64| -> f64 {
        if n == 0 {
            f64::NAN
        } else {
            v[((n as f64 - 1.0) * q).round() as usize]
        }
    };
    CallStats {
        n: n as u64,
        mean_us: if n == 0 {
            f64::NAN
        } else {
            v.iter().sum::<f64>() / n as f64
        },
        median_us: pick(0.5),
        p99_us: pick(0.99),
        max_us: v.last().copied().unwrap_or(f64::NAN),
    }
}

/// The H1 spike's answer: whether ff works on this backend at all, what the
/// three weight-axis prototypes feel like, and what each command path costs
/// at the call site.
#[derive(Debug, Clone)]
pub struct RumbleSpikeReport {
    pub pad: String,
    /// Direct `gilrs_core::FfDevice::set_ff_state` call durations.
    pub direct: CallStats,
    /// High-level `gilrs::ff::Effect::play()` call durations (channel send
    /// only — the 0–50 ms server-tick latency is on top of this, unmeasured
    /// by construction).
    pub effect_play: Option<CallStats>,
    /// Whether the operator-felt demo patterns ran.
    pub feel_patterns: bool,
}

impl RumbleSpikeReport {
    /// Serialize as the TOML committed to `logs/latency/`.
    pub fn to_toml(&self) -> String {
        let mut out = String::new();
        out.push_str("# Rumble command-path spike (flint-input-capture, ADR 0025)\n");
        out.push_str("# Motor onset is write-only hardware state: both measurements below are\n");
        out.push_str("# call-site costs. The gilrs ff-server path adds a structural 0-50 ms\n");
        out.push_str("# quantization on top (server ticks every 50 ms; gilrs src/ff/server.rs);\n");
        out.push_str(
            "# ERM actuator spin-up (~20-50 ms) is on top of everything and is absorbed\n",
        );
        out.push_str("# by the feel-tuned lead_ms knob in config/haptics.toml.\n");
        out.push_str("[run]\n");
        out.push_str(&format!("pad = {:?}\n", self.pad));
        out.push_str(&format!("feel_patterns = {}\n", self.feel_patterns));
        out.push_str("gilrs_ff_server_tick_ms = 50 # cited from gilrs 0.11 source, not measured\n");
        for (name, stats) in [
            ("direct_set_ff_state_call_us", Some(&self.direct)),
            ("effect_play_call_us", self.effect_play.as_ref()),
        ] {
            let Some(s) = stats else { continue };
            out.push_str(&format!("\n[{name}]\n"));
            out.push_str(&format!("n = {}\n", s.n));
            out.push_str(&format!("mean = {}\n", fmt(s.mean_us)));
            out.push_str(&format!("median = {}\n", fmt(s.median_us)));
            out.push_str(&format!("p99 = {}\n", fmt(s.p99_us)));
            out.push_str(&format!("max = {}\n", fmt(s.max_us)));
        }
        out
    }
}

fn fmt(v: f64) -> String {
    if v.is_finite() {
        format!("{v:.1}")
    } else {
        "nan".into()
    }
}

/// Acquire the first connected ff-capable pad's device via gilrs-core.
fn open_ff_device() -> flint_core::Result<(String, gilrs_core::FfDevice)> {
    let gilrs = gilrs_core::Gilrs::new().map_err(|e| {
        flint_core::FlintError::InputError(format!("gamepad backend unavailable: {e}"))
    })?;
    for id in 0..gilrs.last_gamepad_hint() {
        let Some(pad) = gilrs.gamepad(id) else {
            continue;
        };
        if !pad.is_connected() {
            continue;
        }
        if !pad.is_ff_supported() {
            tracing::warn!("{} does not support force feedback", pad.name());
            continue;
        }
        match pad.ff_device() {
            Some(dev) => return Ok((pad.name().to_string(), dev)),
            None => tracing::warn!("{}: ff supported but no ff device handle", pad.name()),
        }
    }
    Err(flint_core::FlintError::InputError(
        "no connected force-feedback-capable gamepad (on Windows only XInput-class \
         devices are seen; check the controller/mapper mode)"
            .into(),
    ))
}

/// Motor magnitudes for the three weight-axis prototypes (u16 full scale).
const TICK_WEAK: u16 = 22_000;
const THUMP_STRONG: u16 = 45_000;
const THUMP_WEAK: u16 = 25_000;
const GRIND_STRONG: u16 = 14_000;

/// Run the H1 spike: prove ff fires, let the operator feel the three
/// weight-axis prototypes, and time both command paths. Blocking; prints
/// progress. `feel = false` skips the operator-felt patterns (timing only).
pub fn spike_rumble(feel: bool) -> flint_core::Result<RumbleSpikeReport> {
    let (pad, mut dev) = open_ff_device()?;
    println!("rumble spike on: {pad}");
    // `min_duration` is ignored by the XInput backend; motors hold their
    // state until the next call, so every burst below is explicitly ended
    // with a (0, 0) write.
    let hold = Duration::from_millis(0);

    if feel {
        println!("  tick x8 (light, weak motor — the subdivision weight)...");
        for _ in 0..8 {
            dev.set_ff_state(0, TICK_WEAK, hold);
            std::thread::sleep(Duration::from_millis(30));
            dev.set_ff_state(0, 0, hold);
            std::thread::sleep(Duration::from_millis(370));
        }
        println!("  thump x4 (heavy, both motors — the downbeat weight)...");
        for _ in 0..4 {
            dev.set_ff_state(THUMP_STRONG, THUMP_WEAK, hold);
            std::thread::sleep(Duration::from_millis(70));
            dev.set_ff_state(0, 0, hold);
            std::thread::sleep(Duration::from_millis(730));
        }
        println!("  grind 2 s (low, strong motor sustained — the rewind weight)...");
        dev.set_ff_state(GRIND_STRONG, 0, hold);
        std::thread::sleep(Duration::from_millis(2000));
        dev.set_ff_state(0, 0, hold);
        std::thread::sleep(Duration::from_millis(300));
    }

    // Direct-path call cost: alternate a faint pulse on/off at the capture
    // loop's own cadence, timing each XInputSetState round trip.
    println!("  timing direct set_ff_state x400...");
    let mut direct = Vec::with_capacity(400);
    for i in 0..400u32 {
        let (strong, weak) = if i % 2 == 0 { (0, 6_000) } else { (0, 0) };
        let t = Instant::now();
        dev.set_ff_state(strong, weak, hold);
        direct.push(t.elapsed().as_secs_f64() * 1e6);
        std::thread::sleep(Duration::from_millis(1));
    }
    dev.set_ff_state(0, 0, hold);
    drop(dev);

    // High-level path call cost, for the record (the interesting number —
    // the 0–50 ms server tick — is structural and cited, not timed).
    println!("  timing high-level Effect::play x100...");
    let effect_play = time_effect_play().map_or_else(
        |e| {
            println!("  (high-level path unavailable: {e})");
            None
        },
        Some,
    );

    Ok(RumbleSpikeReport {
        pad,
        direct: call_stats(&direct),
        effect_play,
        feel_patterns: feel,
    })
}

fn time_effect_play() -> flint_core::Result<CallStats> {
    use gilrs::ff::{BaseEffect, BaseEffectType, EffectBuilder, Replay, Ticks};
    let mut gilrs = gilrs::Gilrs::new().map_err(|e| {
        flint_core::FlintError::InputError(format!("gamepad backend unavailable: {e}"))
    })?;
    let ids: Vec<_> = gilrs
        .gamepads()
        .filter_map(|(id, gp)| gp.is_ff_supported().then_some(id))
        .collect();
    if ids.is_empty() {
        return Err(flint_core::FlintError::InputError(
            "no ff-capable pad via high-level gilrs".into(),
        ));
    }
    let effect = EffectBuilder::new()
        .add_effect(BaseEffect {
            kind: BaseEffectType::Weak { magnitude: 6_000 },
            scheduling: Replay {
                play_for: Ticks::from_ms(50),
                ..Default::default()
            },
            envelope: Default::default(),
        })
        .gamepads(&ids)
        .finish(&mut gilrs)
        .map_err(|e| flint_core::FlintError::InputError(format!("building ff effect: {e}")))?;
    let mut samples = Vec::with_capacity(100);
    for _ in 0..100 {
        let t = Instant::now();
        effect
            .play()
            .map_err(|e| flint_core::FlintError::InputError(format!("Effect::play: {e}")))?;
        samples.push(t.elapsed().as_secs_f64() * 1e6);
        std::thread::sleep(Duration::from_millis(2));
        let _ = effect.stop();
        std::thread::sleep(Duration::from_millis(2));
    }
    let _ = effect.stop();
    // Give the 50 ms server tick a chance to apply the stop before the
    // instance (and its server thread channel) goes away.
    std::thread::sleep(Duration::from_millis(120));
    Ok(call_stats(&samples))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn call_stats_orders_and_picks() {
        let s = call_stats(&[3.0, 1.0, 2.0, 4.0, 100.0]);
        assert_eq!(s.n, 5);
        assert_eq!(s.median_us, 3.0);
        assert_eq!(s.max_us, 100.0);
        assert!(s.p99_us >= s.median_us);
    }

    fn burst(strong: f64, weak: f64, dur: i64) -> Burst {
        Burst {
            strong,
            weak,
            duration_samples: dur,
        }
    }

    #[test]
    fn burst_fires_at_lead_adjusted_sample_and_expires() {
        let mut e = RumbleEngine::new();
        e.push(RumbleCommand::SetLead { samples: 100 });
        e.push(RumbleCommand::Schedule {
            at_clock_sample: 1000,
            burst: burst(0.0, 0.5, 200),
        });
        assert_eq!(e.tick(899), None); // before the lead-adjusted point
        let (s, w) = e.tick(900).expect("fires at target - lead");
        assert_eq!(s, 0);
        assert!(w > 0);
        assert_eq!(e.tick(950), None); // held, no re-write
        assert_eq!(e.tick(1100), Some((0, 0))); // expired at fire + duration
    }

    #[test]
    fn fire_plays_immediately_without_lead_or_late_drop() {
        let mut e = RumbleEngine::new();
        e.push(RumbleCommand::SetLead { samples: 5000 });
        e.push(RumbleCommand::SetLateDrop { samples: 10 });
        // A Fire burst at any clock time plays on the very next tick — the
        // thump path: felt confirmation of a press already in the past.
        e.push(RumbleCommand::Fire {
            burst: burst(0.6, 0.2, 100),
        });
        let (s, w) = e.tick(1_000_000).expect("fires now");
        assert!(s > 0 && w > 0);
        assert_eq!(e.stats().fired, 1);
        assert_eq!(e.stats().dropped_late, 0);
        assert_eq!(e.tick(1_000_100), Some((0, 0))); // expires normally
    }

    #[test]
    fn late_burst_is_dropped_not_played() {
        let mut e = RumbleEngine::new();
        e.push(RumbleCommand::SetLateDrop { samples: 50 });
        e.push(RumbleCommand::Schedule {
            at_clock_sample: 1000,
            burst: burst(0.0, 1.0, 200),
        });
        assert_eq!(e.tick(1051), None); // 51 past due: dropped
        assert_eq!(e.tick(1052), None); // and gone, not deferred
    }

    #[test]
    fn overlapping_bursts_sum_and_clamp_and_gain_scales() {
        let mut e = RumbleEngine::new();
        e.push(RumbleCommand::Schedule {
            at_clock_sample: 0,
            burst: burst(0.8, 0.0, 100),
        });
        e.push(RumbleCommand::Schedule {
            at_clock_sample: 0,
            burst: burst(0.8, 0.0, 100),
        });
        let (s, _) = e.tick(0).unwrap();
        assert_eq!(s, u16::MAX); // 1.6 clamps to full scale
        let mut e = RumbleEngine::new();
        e.push(RumbleCommand::SetGain(0.5));
        e.push(RumbleCommand::SetContinuous {
            strong: 1.0,
            weak: 0.0,
        });
        let (s, _) = e.tick(0).unwrap();
        assert_eq!(s, (0.5 * f64::from(u16::MAX)) as u16);
    }

    #[test]
    fn flush_drops_scheduled_ends_active_and_stops_continuous() {
        let mut e = RumbleEngine::new();
        e.push(RumbleCommand::SetContinuous {
            strong: 0.3,
            weak: 0.0,
        });
        e.push(RumbleCommand::Schedule {
            at_clock_sample: 0,
            burst: burst(0.0, 0.4, 1000),
        });
        e.push(RumbleCommand::Schedule {
            at_clock_sample: 5000,
            burst: burst(0.0, 0.4, 1000),
        });
        assert!(e.tick(0).is_some()); // continuous + first burst live
        e.push(RumbleCommand::Flush);
        assert_eq!(e.tick(1), Some((0, 0))); // everything off, one write
        assert_eq!(e.tick(5000), None); // the future burst is gone
    }

    #[test]
    fn same_sample_bursts_fire_in_push_order_deterministically() {
        let mut e = RumbleEngine::new();
        for _ in 0..3 {
            e.push(RumbleCommand::Schedule {
                at_clock_sample: 10,
                burst: burst(0.0, 0.2, 50),
            });
        }
        let (_, w) = e.tick(10).unwrap();
        assert_eq!(w, (0.6f64.min(1.0) * f64::from(u16::MAX)) as u16);
    }

    #[test]
    fn silence_is_unconditional_and_resets_shadow() {
        let mut e = RumbleEngine::new();
        e.push(RumbleCommand::SetContinuous {
            strong: 1.0,
            weak: 1.0,
        });
        assert!(e.tick(0).is_some());
        assert_eq!(e.silence(), (0, 0));
        // Shadow reset: a fresh nonzero state must re-write.
        e.push(RumbleCommand::SetContinuous {
            strong: 0.2,
            weak: 0.0,
        });
        assert!(e.tick(1).is_some());
    }

    #[test]
    fn empty_stats_are_nan_not_panic() {
        let s = call_stats(&[]);
        assert_eq!(s.n, 0);
        assert!(s.mean_us.is_nan() && s.median_us.is_nan());
    }
}
