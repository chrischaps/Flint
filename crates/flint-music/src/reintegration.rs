//! Full-fail detection and the reintegration sequencer (ADR 0014/0015).
//!
//! Below the ladder's full-fail threshold (held for its debounce), the
//! sequencer takes over: it flushes the scheduler, schedules a seam on the
//! next reachable bar line — the old timeline fades out over the authored
//! seam envelope, every stem re-plays sample-locked from the previous
//! re-entry section, the lead motif bus enters at full level while the rest
//! enter deep and ramp in over the manifest's `reassembly_bars` — and runs
//! the ladder in reverse: applied params lerp from the deepest rung back to
//! clean over the same span, so the world re-gathers as one thing. No hard
//! audio cut, no input interruption; the caller keeps judging throughout
//! (rewinding its judge at the seam via [`ReintegrationEvent::Seam`]).
//!
//! State machine: `Playing → Failing (seam scheduled, world holds at the
//! deepest rung) → Reassembling (progress 0→1) → Playing`. Coherence is not
//! reset — a player still absent after reassembly will fail again, which is
//! the designed loop; only the debounce restarts.

use crate::conductor::Grid;
use crate::ladder::{Ladder, LadderDriver, LadderParams};
use crate::manifest::Reintegration;
use crate::session::SuiteSession;
use flint_core::Result;
use kira::Tween;
use std::time::Duration;

/// Authored seam fade (ms) — the envelope that carries the world out of the
/// old timeline. An envelope, not a cut; tuned in E7, adjudicated by the
/// Milestone 3 listen.
pub const DEFAULT_SEAM_FADE_MS: f64 = 30.0;
/// Entry level (dB) for non-lead buses at the seam.
pub const ENTRY_DB: f32 = -60.0;
/// Scheduling headroom: the seam must sit at least this far past "now" so
/// fade and re-play commands reach the audio thread in time (realtime tick
/// cadence is 2 ms; this is generous).
const MIN_LEAD_MS: f64 = 100.0;
/// Ramp for per-tick param moves while reassembly lerps the ladder back.
const REASSEMBLY_RAMP_MS: f64 = 30.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SeqPhase {
    Playing,
    Failing,
    Reassembling,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ReintegrationEvent {
    /// The detector tripped and the seam is scheduled.
    FullFail {
        at_suite_sample: i64,
        re_entry_sample: i64,
        seam_suite_sample: i64,
    },
    /// The clock reached the seam; the suite timeline jumped. The caller
    /// rewinds its judge to `re_entry_sample` on this event.
    Seam {
        raw_sample: i64,
        timeline_offset: i64,
        re_entry_sample: i64,
    },
    /// The reassembly span completed; normal play resumes.
    ReassemblyComplete { at_suite_sample: i64 },
}

/// What one sequencer tick resolved: the params the world should show this
/// tick (already applied to the mixer), the reassembly progress for visuals
/// (1.0 in normal play, 0→1 while re-gathering), and the rewind progress
/// (0 = not rewinding, else 0→1 through the pre-seam spin-down).
pub struct SeqTick {
    pub params: LadderParams,
    pub reassembly: f64,
    pub rewind: f64,
}

enum Phase {
    Playing,
    Failing {
        fail_raw: i64,
        seam_raw: i64,
        re_entry: i64,
        span: i64,
    },
    Reassembling {
        seam_raw: i64,
        span: i64,
    },
}

pub struct Reintegrator {
    decl: Reintegration,
    pub fade_ms: f64,
    /// Rewind interlude: bars between fail and seam, during which every stem
    /// spins down like a record played backwards. 0 = immediate seam.
    pub rewind_bars: i64,
    /// Playback-rate landing point of the spin-down, in semitones (< 0).
    pub rewind_drop_st: f64,
    phase: Phase,
    /// Raw sample when coherence first went below the full-fail threshold.
    below_since: Option<i64>,
}

impl Reintegrator {
    pub fn new(decl: Reintegration) -> Self {
        Self {
            decl,
            fade_ms: DEFAULT_SEAM_FADE_MS,
            rewind_bars: 1,
            rewind_drop_st: -30.0,
            phase: Phase::Playing,
            below_since: None,
        }
    }

    pub fn phase(&self) -> SeqPhase {
        match self.phase {
            Phase::Playing => SeqPhase::Playing,
            Phase::Failing { .. } => SeqPhase::Failing,
            Phase::Reassembling { .. } => SeqPhase::Reassembling,
        }
    }

    /// One tick of the coherence-consumer side: observe the ladder, drive
    /// the mixer (directly or through the reassembly lerp), detect full-fail,
    /// and step the seam state machine. Call every tick once past pre-roll,
    /// after `session.pump()`.
    pub fn tick(
        &mut self,
        coherence: f64,
        ladder: &mut Ladder,
        driver: &mut LadderDriver,
        session: &mut SuiteSession,
        events: &mut Vec<ReintegrationEvent>,
    ) -> Result<SeqTick> {
        let raw = session.raw_clock_sample();
        let sample_rate = session.conductor.tempo().sample_rate() as f64;
        let now_suite = session.clock_sample();
        let now_seconds = now_suite as f64 / sample_rate;

        match self.phase {
            Phase::Playing => {
                ladder.observe(coherence);
                let params = ladder.params();
                driver.apply(&params, now_seconds, params.ramp_ms, &mut session.mixer);

                // The hold starts below `enter_below` and cancels only above
                // `exit_above`: under neglect the value saw-tooths (miss
                // impulses down, tracking integrator back up), and a peak
                // poking over the enter threshold must not save the world.
                let ff = ladder.config().full_fail;
                if ladder.armed() && coherence < ff.enter_below {
                    let since = *self.below_since.get_or_insert(raw);
                    let held = (raw - since) as f64 / sample_rate * 1000.0;
                    if held >= ff.hold_ms {
                        self.trigger(session, driver, events)?;
                        self.below_since = None;
                        return Ok(SeqTick {
                            params: ladder.full_fail_params(),
                            reassembly: 1.0,
                            rewind: 0.0,
                        });
                    }
                } else if coherence > ff.exit_above {
                    self.below_since = None;
                }
                Ok(SeqTick {
                    params,
                    reassembly: 1.0,
                    rewind: 0.0,
                })
            }

            Phase::Failing {
                fail_raw,
                seam_raw,
                re_entry,
                span,
            } => {
                // The record spins backwards toward the seam (the rate ramp
                // is already queued in kira); the world holds at its most
                // disintegrated. Nothing new is commanded here.
                let rewind = if seam_raw > fail_raw {
                    ((raw - fail_raw) as f64 / (seam_raw - fail_raw) as f64).clamp(0.0, 1.0)
                } else {
                    1.0
                };
                if raw >= seam_raw {
                    events.push(ReintegrationEvent::Seam {
                        raw_sample: raw,
                        timeline_offset: session.timeline_offset(),
                        re_entry_sample: re_entry,
                    });
                    // Fresh sounds, fresh shadow state: restate everything.
                    driver.reset();
                    self.phase = Phase::Reassembling { seam_raw, span };
                }
                Ok(SeqTick {
                    params: ladder.full_fail_params(),
                    reassembly: 1.0,
                    rewind,
                })
            }

            Phase::Reassembling { seam_raw, span } => {
                let t = ((raw - seam_raw) as f64 / span.max(1) as f64).clamp(0.0, 1.0);
                let params = ladder.full_fail_params().lerp(&LadderParams::clean(), t);
                driver.apply(&params, now_seconds, REASSEMBLY_RAMP_MS, &mut session.mixer);
                if raw >= seam_raw + span {
                    events.push(ReintegrationEvent::ReassemblyComplete {
                        at_suite_sample: now_suite,
                    });
                    // The ladder resumes from live coherence next tick; the
                    // debounce restarts so a still-absent player gets the
                    // full hold before the next fall.
                    self.below_since = None;
                    self.phase = Phase::Playing;
                }
                Ok(SeqTick {
                    params,
                    reassembly: t,
                    rewind: 0.0,
                })
            }
        }
    }

    /// Full-fail fired: pick the checkpoint, schedule the seam and the
    /// reassembly ramps, flush stale scheduler events.
    fn trigger(
        &mut self,
        session: &mut SuiteSession,
        driver: &mut LadderDriver,
        events: &mut Vec<ReintegrationEvent>,
    ) -> Result<()> {
        let sample_rate = session.conductor.tempo().sample_rate() as f64;
        let now_suite = session.clock_sample();

        let re_entry = session
            .conductor
            .previous_re_entry(now_suite, &self.decl)
            .map(|s| s.start_sample)
            .unwrap_or(0);

        // Seam on a bar line with enough lead for fade + command delivery,
        // pushed out by the rewind interlude: grid continuity comes free —
        // section starts sit on bar lines, so beat phase is continuous
        // across the jump.
        let lead_samples =
            ((self.fade_ms + MIN_LEAD_MS) / 1000.0 * sample_rate).round() as i64;
        let seam0 = session
            .conductor
            .next_grid_sample(now_suite + lead_samples, Grid::Bar);
        let seam_suite = if self.rewind_bars > 0 {
            let bar = session.conductor.position_at_sample(seam0).bar;
            session
                .conductor
                .sample_at_bar(bar + self.rewind_bars, 0.0)
        } else {
            seam0
        };

        // The rewind: every playing stem's rate ramps down together over the
        // whole interlude — a record spun backwards, not an effect on one
        // instrument. Commanded on the OLD sounds, before `schedule_seam`
        // swaps the handles; sample-lock breaks during the ramp, which is
        // fine — everything re-plays locked at the seam.
        if self.rewind_bars > 0 {
            let dur = Duration::from_secs_f64((seam_suite - now_suite) as f64 / sample_rate);
            for name in crate::BUSES {
                if let Some(bus) = session.mixer.bus_mut(name) {
                    if bus.state.playing {
                        bus.set_detune(
                            self.rewind_drop_st,
                            Tween {
                                duration: dur,
                                ..Default::default()
                            },
                        );
                    }
                }
            }
        }

        // Reassembly span, measured on the post-seam timeline.
        let entry_bar = session.conductor.position_at_sample(re_entry).bar;
        let end = session
            .conductor
            .sample_at_bar(entry_bar + self.decl.reassembly_bars, 0.0);
        let span = (end - re_entry).max(1);

        let flushed = session.scheduler.flush();
        if flushed > 0 {
            tracing::info!("reintegration flushed {flushed} pending scheduler event(s)");
        }

        let lead_bus = self.decl.lead_bus.clone();
        session.schedule_seam(re_entry, seam_suite, self.fade_ms, &|bus| {
            if bus == lead_bus {
                0.0
            } else {
                ENTRY_DB
            }
        })?;

        // One clock-timed volume ramp per non-lead bus: entry level → full
        // over the span, starting exactly at the seam. Per-sound volume, so
        // it never fights the ladder's track-gain trims.
        let seam_raw = seam_suite + session.timeline_offset();
        let ramp = Tween {
            start_time: kira::StartTime::ClockTime(session.clock_time_at_raw(seam_raw)),
            duration: Duration::from_secs_f64(span as f64 / sample_rate),
            ..Default::default()
        };
        for name in crate::BUSES {
            if name == self.decl.lead_bus {
                continue;
            }
            if let Some(bus) = session.mixer.bus_mut(name) {
                bus.set_sound_volume(0.0, ramp);
            }
        }

        driver.reset();
        events.push(ReintegrationEvent::FullFail {
            at_suite_sample: now_suite,
            re_entry_sample: re_entry,
            seam_suite_sample: seam_suite,
        });
        self.phase = Phase::Failing {
            fail_raw: seam_raw - (seam_suite - now_suite),
            seam_raw,
            re_entry,
            span,
        };
        Ok(())
    }
}
