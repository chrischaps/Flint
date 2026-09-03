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
use crate::gradient::GradientOffsets;
use crate::ladder::{Ladder, LadderDriver, LadderParams};
use crate::manifest::Reintegration;
use crate::session::SuiteSession;
use crate::tempo::ms_to_samples;
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
        /// Raw sample where the audible rewind gesture begins (== seam_raw
        /// when there is no gesture).
        gesture_start_raw: i64,
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
    /// Rewind gesture length in beats (at the tempo where the fail happens):
    /// every stem spins down like a record played backwards for exactly this
    /// long, ending at the seam. 0 = no spin-down.
    pub rewind_beats: f64,
    /// Playback-rate landing point of the spin-down, in semitones (< 0).
    pub rewind_drop_st: f64,
    /// Pickup spin-up: beats (at the re-entry tempo) of lead-bus cue winding
    /// up from `rewind_drop_st` to full speed, arriving on the re-entry
    /// downbeat. 0 disables.
    pub pickup_beats: f64,
    /// Lead-in: shift the re-entry point this many beats BEFORE the
    /// checkpoint downbeat, so the ensemble re-enters on the pickup into the
    /// phrase — "3, 4, go" (P5, Chris). Beats at the re-entry tempo; clamped
    /// at suite sample 0. 0 = enter on the downbeat (pre-lead-in behavior,
    /// arithmetic untouched).
    pub lead_in_beats: f64,
    /// Debug: force the next Playing tick to trigger reintegration,
    /// bypassing arming and the hold (set via [`Self::debug_force_fail`];
    /// dev surfaces only — nothing on a deterministic path sets it).
    force_fail: bool,
    phase: Phase,
    /// Raw sample when coherence first went below the full-fail threshold.
    below_since: Option<i64>,
}

impl Reintegrator {
    pub fn new(decl: Reintegration) -> Self {
        Self {
            decl,
            fade_ms: DEFAULT_SEAM_FADE_MS,
            rewind_beats: 2.0,
            rewind_drop_st: -30.0,
            pickup_beats: 2.0,
            lead_in_beats: 0.0,
            force_fail: false,
            phase: Phase::Playing,
            below_since: None,
        }
    }

    /// Debug: trigger the full reintegration (rewind → seam → reassembly) on
    /// the next Playing tick, regardless of arming or the coherence hold.
    /// A no-op while a seam is already in flight. Dev surface for iterating
    /// on seam recovery without playing to a fail; the seam machinery runs
    /// exactly as in a real fail.
    pub fn debug_force_fail(&mut self) {
        if matches!(self.phase, Phase::Playing) {
            self.force_fail = true;
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
    /// after `session.pump()`. `offsets` is the error-gradient's additive
    /// contribution (ADR 0024) — honored only while `Playing`: `Failing`
    /// commands nothing (the rewind's rate ramp is already queued and must
    /// not be fought) and `Reassembling` re-gathers clean, the gradient
    /// easing back in via its own attack once play resumes.
    pub fn tick(
        &mut self,
        coherence: f64,
        offsets: &GradientOffsets,
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
                let xfade_ms = ladder.config().alternate_xfade_ms;
                driver.apply(
                    &params,
                    offsets,
                    now_seconds,
                    params.ramp_ms,
                    Some((now_suite, xfade_ms)),
                    &mut session.mixer,
                );

                // The hold starts below `enter_below` and cancels only above
                // `exit_above`: under neglect the value saw-tooths (miss
                // impulses down, tracking integrator back up), and a peak
                // poking over the enter threshold must not save the world.
                if self.force_fail {
                    self.force_fail = false;
                    self.trigger(session, driver, events)?;
                    self.below_since = None;
                    return Ok(SeqTick {
                        params: ladder.full_fail_params(),
                        reassembly: 1.0,
                        rewind: 0.0,
                    });
                }
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
                gesture_start_raw,
                seam_raw,
                re_entry,
                span,
            } => {
                // The record spins backwards toward the seam (the rate ramp
                // is already queued in kira); the world holds at its most
                // disintegrated. Nothing new is commanded here. Visual
                // rewind progress tracks the audible gesture window only.
                let rewind = if seam_raw > gesture_start_raw {
                    ((raw - gesture_start_raw) as f64 / (seam_raw - gesture_start_raw) as f64)
                        .clamp(0.0, 1.0)
                } else {
                    0.0
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
                // `None`: per-sound volume belongs to the entry envelopes
                // during reassembly (ADR 0032); the seam re-play already
                // reset every crossfade to bus-active.
                driver.apply(
                    &params,
                    &GradientOffsets::default(),
                    now_seconds,
                    REASSEMBLY_RAMP_MS,
                    None,
                    &mut session.mixer,
                );
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

        let checkpoint = session
            .conductor
            .previous_re_entry(now_suite, &self.decl)
            .map(|s| s.start_sample)
            .unwrap_or(0);
        // Lead-in ("3, 4, go"): everything re-enters `lead_in_beats` before
        // the checkpoint downbeat — full ensemble, sample-locked, judgment
        // windows in the lead-in re-opened — giving the player prep time
        // into the phrase. The reassembly span below stays anchored to the
        // checkpoint bar, so the ramp's musical length is unchanged.
        let re_entry = if self.lead_in_beats > 0.0 && checkpoint > 0 {
            let entry_beat = session.conductor.position_at_sample(checkpoint).beat;
            session
                .conductor
                .sample_at_beat((entry_beat - self.lead_in_beats).max(0.0))
                .max(0)
        } else {
            checkpoint
        };

        // Seam on a bar line with enough lead for fade + command delivery
        // plus the rewind gesture: grid continuity comes free — section
        // starts sit on bar lines, so beat phase is continuous across the
        // jump. The gesture is measured in beats at the CURRENT tempo and
        // ends at the seam, so (for whole-beat lengths) it starts on a beat.
        let lead_samples = ms_to_samples(self.fade_ms + MIN_LEAD_MS, sample_rate);
        let now_beat = session.conductor.position_at_sample(now_suite).beat;
        let beat_samples = (session.conductor.sample_at_beat(now_beat + 1.0)
            - session.conductor.sample_at_beat(now_beat))
        .max(1);
        let rewind_span = (self.rewind_beats.max(0.0) * beat_samples as f64).round() as i64;
        let seam_suite = session
            .conductor
            .next_grid_sample(now_suite + lead_samples + rewind_span, Grid::Bar);

        // The rewind: every playing stem's rate ramps down together — a
        // record spun backwards, not an effect on one instrument — for
        // exactly `rewind_beats`, ending at the seam (clock-timed start;
        // the music simply continues until the gesture begins). Commanded on
        // the OLD sounds, before `schedule_seam` swaps the handles;
        // sample-lock breaks during the ramp, which is fine — everything
        // re-plays locked at the seam.
        let gesture_start_suite = seam_suite - rewind_span;
        if rewind_span > 0 {
            let start_time = kira::StartTime::ClockTime(
                session.clock_time_at_raw(gesture_start_suite + session.timeline_offset()),
            );
            let dur = Duration::from_secs_f64(rewind_span as f64 / sample_rate);
            for name in crate::BUSES {
                if let Some(bus) = session.mixer.bus_mut(name) {
                    if bus.state.playing {
                        bus.set_detune(
                            self.rewind_drop_st,
                            Tween {
                                start_time,
                                duration: dur,
                                ..Default::default()
                            },
                        );
                    }
                }
            }
        }

        // Reassembly span, measured on the post-seam timeline (anchored to
        // the checkpoint bar — the phrase start — not the lead-in point).
        let entry_bar = session.conductor.position_at_sample(checkpoint).bar;
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

        // The pickup: in the last beats before the seam, the lead bus plays
        // the re-entry material as a cue spinning up from the rewind's
        // landing rate to full speed, arriving on the downbeat — "…and-a-
        // ONE". A gesture outside the locked timeline: whatever position
        // error the rate ramp accrues, the real ensemble still enters
        // sample-pure at the seam while the cue stops under the seam fade.
        if self.pickup_beats > 0.0 {
            // One beat's samples at the re-entry tempo (robust at suite
            // sample 0, where there are no beats before the entry).
            let entry_beat = session.conductor.position_at_sample(re_entry).beat;
            let beat_samples =
                (session.conductor.sample_at_beat(entry_beat + 1.0) - re_entry).max(1);
            let mut span = (self.pickup_beats * beat_samples as f64).round() as i64;
            // The cue must start inside the interlude, after commands land.
            let available = seam_suite - now_suite - lead_samples;
            span = span.min(available);
            if span > beat_samples / 4 {
                let seam_raw_pre = seam_suite + session.timeline_offset();
                let cue_start =
                    kira::StartTime::ClockTime(session.clock_time_at_raw(seam_raw_pre - span));
                let ramp = Tween {
                    start_time: cue_start,
                    duration: Duration::from_secs_f64(span as f64 / sample_rate),
                    ..Default::default()
                };
                // Fade completes AT the seam so the cue never doubles the
                // ensemble's sample-pure downbeat entry.
                let fade_samples = ms_to_samples(self.fade_ms, sample_rate);
                let stop_at_seam = Tween {
                    start_time: kira::StartTime::ClockTime(
                        session.clock_time_at_raw(seam_raw_pre - fade_samples),
                    ),
                    duration: Duration::from_secs_f64(self.fade_ms / 1000.0),
                    ..Default::default()
                };
                if let Some(bus) = session.mixer.bus_mut(&self.decl.lead_bus) {
                    match bus.play_cue(re_entry as u64, cue_start, -18.0, self.rewind_drop_st) {
                        Ok(Some(mut cue)) => {
                            cue.set_playback_rate(kira::PlaybackRate(1.0), ramp);
                            cue.set_volume(kira::Decibels(0.0), ramp);
                            cue.stop(stop_at_seam);
                        }
                        Ok(None) => {}
                        Err(e) => tracing::warn!("pickup cue failed: {e}"),
                    }
                }
            }
        }

        driver.reset();
        events.push(ReintegrationEvent::FullFail {
            at_suite_sample: now_suite,
            re_entry_sample: re_entry,
            seam_suite_sample: seam_suite,
        });
        self.phase = Phase::Failing {
            gesture_start_raw: seam_raw - rewind_span,
            seam_raw,
            re_entry,
            span,
        };
        Ok(())
    }
}
