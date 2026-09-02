//! Lookahead scheduler: events queued in musical time, landing on the grid.
//!
//! kira's clock-timed `Tween`s do the sample-accurate landing; the scheduler's
//! job is to hold events beyond a lookahead horizon, resolve musical time to
//! a sample position **once, at push** (via the Conductor — a resolved event
//! never re-interprets time), and issue the kira commands as events enter the
//! horizon. Section transitions and reintegration entries in later phases are
//! consumers of this same queue.

use crate::conductor::Conductor;
use crate::mixer::BusMixer;
use kira::clock::{ClockHandle, ClockTime};
use kira::{StartTime, Tween};
use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::time::Duration;

/// When an event should land, in any of the Conductor's time vocabularies.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum EventTime {
    Sample(i64),
    Beat(f64),
    BarBeat { bar: i64, beat: f64 },
}

/// What lands.
#[derive(Debug, Clone, PartialEq)]
pub enum BusAction {
    SetGain {
        db: f32,
        ramp_ms: f64,
    },
    SetLpf {
        hz: f64,
        ramp_ms: f64,
    },
    SetDetune {
        semitones: f64,
        ramp_ms: f64,
    },
    /// No audio effect; returned from `pump` for logging / section hooks.
    Marker(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct ScheduledEvent {
    pub at: EventTime,
    /// Target bus; `None` is only meaningful for `Marker`.
    pub bus: Option<String>,
    pub action: BusAction,
}

/// An event the scheduler has issued (its tween is queued in kira, or it was
/// a marker), reported back from `pump`.
#[derive(Debug, Clone, PartialEq)]
pub struct FiredEvent {
    pub sample: i64,
    pub bus: Option<String>,
    pub action: BusAction,
    /// True if the event's sample was already behind the play head when
    /// issued; it fired immediately instead of landing on the grid.
    pub late: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct QueueKey {
    sample: i64,
    /// Push order breaks ties deterministically.
    seq: u64,
}

pub struct Scheduler {
    horizon_samples: i64,
    next_seq: u64,
    pending: BinaryHeap<Reverse<(QueueKey, EventKeyed)>>,
}

/// BinaryHeap needs Ord; events themselves aren't Ord, so they ride along
/// behind the key and are never compared (QueueKey is unique via seq).
#[derive(Debug, Clone)]
struct EventKeyed(ScheduledEvent);

impl PartialEq for EventKeyed {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}
impl Eq for EventKeyed {}
impl PartialOrd for EventKeyed {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for EventKeyed {
    fn cmp(&self, _: &Self) -> std::cmp::Ordering {
        std::cmp::Ordering::Equal
    }
}

impl Scheduler {
    /// `horizon_samples`: how far ahead of the play head events are handed to
    /// kira. Must comfortably exceed the caller's pump interval plus output
    /// latency; the realtime default is 500 ms of samples.
    pub fn new(horizon_samples: i64) -> Self {
        Self {
            horizon_samples,
            next_seq: 0,
            pending: BinaryHeap::new(),
        }
    }

    pub fn default_horizon(sample_rate: u32) -> i64 {
        sample_rate as i64 / 2
    }

    /// Queue an event, resolving its musical time to a sample now.
    pub fn push(&mut self, ev: ScheduledEvent, conductor: &Conductor) {
        let sample = match ev.at {
            EventTime::Sample(s) => s,
            EventTime::Beat(b) => conductor.sample_at_beat(b),
            EventTime::BarBeat { bar, beat } => conductor.sample_at_bar(bar, beat),
        };
        let key = QueueKey {
            sample,
            seq: self.next_seq,
        };
        self.next_seq += 1;
        self.pending.push(Reverse((key, EventKeyed(ev))));
    }

    pub fn pending_len(&self) -> usize {
        self.pending.len()
    }

    /// Drop every event not yet handed to kira. Reintegration calls this at
    /// full-fail: pending events were resolved against the old timeline and
    /// would land at wrong musical positions after the seam. Returns how many
    /// were discarded. Events already inside the horizon are kira's — they
    /// cannot be withdrawn, which is one reason ladder moves don't use the
    /// scheduler.
    pub fn flush(&mut self) -> usize {
        let n = self.pending.len();
        self.pending.clear();
        n
    }

    /// Issue every pending event within `now_sample + horizon` as a
    /// clock-timed kira command (or immediately, with a warning, if its time
    /// has already passed). Returns what was issued, in order.
    /// `tick_base` maps suite samples to clock ticks (`tick = sample +
    /// tick_base`): the pre-roll length, plus the session's timeline offset
    /// once reintegration seams exist.
    pub fn pump(
        &mut self,
        now_sample: i64,
        clock: &ClockHandle,
        tick_base: i64,
        mixer: &mut BusMixer,
    ) -> Vec<FiredEvent> {
        let mut fired = Vec::new();
        while let Some(Reverse((key, _))) = self.pending.peek() {
            if key.sample > now_sample + self.horizon_samples {
                break;
            }
            let Reverse((key, EventKeyed(ev))) = self.pending.pop().expect("peeked");
            // Also immediate if the target tick would precede clock start
            // (an event scheduled before the pre-roll window can reach).
            let late = key.sample < now_sample || key.sample + tick_base < 0;
            let start_time = if late {
                tracing::warn!(
                    "event at sample {} issued late (play head at {}); firing immediately",
                    key.sample,
                    now_sample
                );
                StartTime::Immediate
            } else {
                // Suite sample -> clock tick via the caller's base (pre-roll
                // plus any reintegration timeline offset).
                StartTime::ClockTime(ClockTime::from_ticks_u64(
                    clock,
                    (key.sample + tick_base) as u64,
                ))
            };
            let tween = |ramp_ms: f64| Tween {
                start_time,
                duration: Duration::from_secs_f64(ramp_ms.max(0.0) / 1000.0),
                ..Default::default()
            };
            match (&ev.action, ev.bus.as_deref()) {
                (BusAction::Marker(_), _) => {}
                (action, Some(bus_name)) => match mixer.bus_mut(bus_name) {
                    Some(bus) => match action {
                        BusAction::SetGain { db, ramp_ms } => bus.set_gain(*db, tween(*ramp_ms)),
                        BusAction::SetLpf { hz, ramp_ms } => bus.set_lpf(*hz, tween(*ramp_ms)),
                        BusAction::SetDetune { semitones, ramp_ms } => {
                            bus.set_detune(*semitones, tween(*ramp_ms))
                        }
                        BusAction::Marker(_) => unreachable!(),
                    },
                    None => {
                        tracing::warn!("scheduled event targets unknown bus '{bus_name}'; dropped")
                    }
                },
                (_, None) => {
                    tracing::warn!("scheduled bus action has no bus; dropped");
                }
            }
            fired.push(FiredEvent {
                sample: key.sample,
                bus: ev.bus,
                action: ev.action,
                late,
            });
        }
        fired
    }
}
