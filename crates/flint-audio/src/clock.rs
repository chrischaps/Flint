//! Where a playing sound is, between the moments the mixer says (#658).
//!
//! A game that moves its picture with its music needs the music's own
//! clock: the sum of `delta_time()` drifts from the device, a hitch
//! separates them, and on the web a sound in the late bundle starts at a
//! moment no script can see. The mixer knows where each sound is, but only
//! now and then: kira's handle moves once per device callback, and the
//! browser's worklet reports a few dozen times a second. [`PlayClock`] runs
//! on between those moments at the sound's rate, so a position read once a
//! frame moves smoothly, and follows each new sample of the mixer's
//! position, snapping to it after a stall, a pause or a seek.
//!
//! Time here is any steady clock in seconds. On the desktop it is the
//! engine's wall clock, and an observation is stamped when it is first
//! seen; in the browser it is the `AudioContext`'s own `currentTime`, and a
//! report is stamped with the audio time it describes, so nothing between
//! the audio thread and the frame adds jitter.

/// How long the clock runs on past its last fresh sample. Past this the
/// mixer has stalled (or the page sleeps), and the clock waits for it
/// rather than guessing. Long enough to carry a report that a long frame
/// held up on its way to the page.
pub const MAX_RUN_ON: f64 = 0.5;

/// A sample this far from where the clock thought the sound was is taken
/// as it is: a stall, a seek, a loop's wrap or a resume.
pub const SNAP: f64 = 0.05;

/// A step back smaller than this is held rather than shown, so the
/// position a script reads never runs backwards by a jitter.
pub const HOLD_BACK: f64 = 0.05;

/// One sample of where a sound is.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Observation {
    /// Seconds into the sound, wrapped for a loop.
    pub position: f64,
    /// The mixer is moving it (playing, or fading in or out).
    pub moving: bool,
    /// When it was there, on the clock's time base.
    pub at: f64,
}

/// A playing sound's position, smoothed between the mixer's samples.
#[derive(Debug, Clone)]
pub struct PlayClock {
    /// The loop's length in seconds; 0 when it doesn't loop.
    length: f64,
    /// How far a fresh sample pulls the clock toward it, 0..=1. A sample
    /// stamped when the frame first saw it is late by up to a frame, so
    /// the desktop follows it gently; the worklet's reports are stamped
    /// exactly, and are taken whole.
    gain: f64,
    /// (seconds into the sound, unwrapped; at)
    anchor: Option<(f64, f64)>,
    /// The last raw position seen, and how many loop lengths it has wrapped.
    last_raw: f64,
    laps: f64,
    moving: bool,
    rate: f64,
    /// The last position given out, unwrapped.
    shown: Option<f64>,
}

impl PlayClock {
    /// A clock for a sound `length` seconds long (0 when it doesn't loop),
    /// following samples with `gain` (see [`PlayClock::gain`]).
    pub fn new(length: f64, gain: f64) -> Self {
        Self {
            length: length.max(0.0),
            gain: gain.clamp(0.0, 1.0),
            anchor: None,
            last_raw: 0.0,
            laps: 0.0,
            moving: false,
            rate: 1.0,
            shown: None,
        }
    }

    /// Whether any sample has come in: until then the sound hasn't started.
    pub fn started(&self) -> bool {
        self.anchor.is_some()
    }

    /// Where the clock, unwrapped, has the sound at `at`.
    fn extrapolate(&self, at: f64) -> Option<f64> {
        let (pos, from) = self.anchor?;
        if !self.moving {
            return Some(pos);
        }
        Some(pos + (at - from).clamp(0.0, MAX_RUN_ON) * self.rate)
    }

    /// Take a sample of the mixer's position. `rate` is the sound's
    /// playback rate now (its pitch). A sample equal to the last one says
    /// nothing new and is ignored, unless the sound stopped or started
    /// moving: the desktop sees the same kira position on every frame
    /// between two device callbacks.
    pub fn observe(&mut self, obs: Observation, rate: f64) {
        self.rate = rate.abs();
        let first = self.anchor.is_none();
        let moving_changed = obs.moving != self.moving;
        if !first && !moving_changed && obs.position == self.last_raw {
            return;
        }
        // A position well behind the last one is the loop wrapping round.
        if !first && self.length > 0.0 && obs.position < self.last_raw - self.length * 0.5 {
            self.laps += self.length;
        }
        self.last_raw = obs.position;
        let fresh = obs.position + self.laps;
        let predicted = self.extrapolate(obs.at);
        self.anchor = Some(match predicted {
            Some(p) if !moving_changed && (fresh - p).abs() <= SNAP => {
                (p + (fresh - p) * self.gain, obs.at)
            }
            _ => (fresh, obs.at),
        });
        self.moving = obs.moving;
    }

    /// Seconds into the sound at `now`, wrapped for a loop; `None` before
    /// the first sample.
    pub fn position(&mut self, now: f64) -> Option<f64> {
        let mut at = self.extrapolate(now)?;
        if let Some(shown) = self.shown {
            if at < shown && shown - at < HOLD_BACK {
                at = shown;
            }
        }
        self.shown = Some(at);
        Some(if self.length > 0.0 {
            at.rem_euclid(self.length)
        } else {
            at
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tiny deterministic generator, for jitter.
    struct Lcg(u64);
    impl Lcg {
        fn next(&mut self) -> f64 {
            self.0 = self
                .0
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (self.0 >> 11) as f64 / (1u64 << 53) as f64
        }
    }

    /// Where a sound that started at 0 really is at `t`, unwrapped.
    fn truth(t: f64) -> f64 {
        t
    }

    /// The desktop's view: kira moves the position once per `callback`
    /// seconds (to where the callback ends: it renders ahead), and the frame
    /// polls at a jittery ~60 Hz, stamping what it sees when it sees it.
    fn run_desktop(
        clock: &mut PlayClock,
        length: f64,
        until: f64,
        callback: f64,
        mut stall: impl FnMut(f64) -> f64,
    ) -> (f64, f64) {
        let mut rng = Lcg(7);
        let mut t = 0.0;
        let mut worst: f64 = 0.0;
        let mut back: f64 = 0.0;
        let mut last: Option<f64> = None;
        while t < until {
            t += 1.0 / 60.0 + (rng.next() - 0.5) * 0.004 + stall(t);
            let rendered = ((t / callback).floor() + 1.0) * callback;
            let raw = rendered.rem_euclid(length);
            clock.observe(
                Observation {
                    position: raw,
                    moving: true,
                    at: t,
                },
                1.0,
            );
            let p = clock.position(t).unwrap();
            let err = wrapped_diff(p, truth(t), length);
            if t > 1.0 {
                worst = worst.max(err.abs());
            }
            if let Some(l) = last {
                let step = wrapped_diff(p, l, length);
                back = back.min(step);
            }
            last = Some(p);
        }
        (worst, back)
    }

    fn wrapped_diff(a: f64, b: f64, length: f64) -> f64 {
        let d = (a - b).rem_euclid(length);
        if d > length / 2.0 {
            d - length
        } else {
            d
        }
    }

    #[test]
    fn nothing_before_the_first_sample() {
        let mut c = PlayClock::new(2.0, 1.0);
        assert!(!c.started());
        assert_eq!(c.position(5.0), None);
    }

    #[test]
    fn the_desktop_clock_follows_kira_within_a_callback_and_never_steps_back() {
        for callback in [0.003, 0.010, 0.021, 0.042] {
            let mut c = PlayClock::new(2.0, 0.15);
            let (worst, back) = run_desktop(&mut c, 2.0, 600.0, callback, |_| 0.0);
            assert!(
                worst < callback + 0.02,
                "{callback}: {worst} from the sound over ten minutes"
            );
            assert!(back >= 0.0, "{callback}: ran back {back}");
        }
    }

    #[test]
    fn a_half_second_stall_snaps_back_to_the_sound() {
        let mut c = PlayClock::new(2.0, 0.15);
        let mut stalled = false;
        let (worst, _) = run_desktop(&mut c, 2.0, 30.0, 0.010, |t| {
            if t > 10.0 && !stalled {
                stalled = true;
                0.5
            } else {
                0.0
            }
        });
        assert!(worst < 0.03, "{worst}");
    }

    #[test]
    fn exact_reports_are_taken_whole_and_run_on_between() {
        // The worklet: a report every 1024 frames at 48 kHz, stamped with
        // the audio time it describes.
        let mut c = PlayClock::new(1.5, 1.0);
        let every = 1024.0 / 48_000.0;
        let mut at = 0.0;
        while at < 20.0 {
            c.observe(
                Observation {
                    position: at % 1.5,
                    moving: true,
                    at,
                },
                1.0,
            );
            for k in 0..3 {
                let now = at + every * k as f64 / 3.0;
                let p = c.position(now).unwrap();
                assert!(wrapped_diff(p, now, 1.5).abs() < 1e-9, "{now}: {p}");
            }
            at += every;
        }
    }

    #[test]
    fn a_pause_holds_and_a_resume_picks_up_where_it_stopped() {
        let mut c = PlayClock::new(4.0, 1.0);
        let obs = |position, moving, at| Observation {
            position,
            moving,
            at,
        };
        c.observe(obs(1.0, true, 10.0), 1.0);
        assert!((c.position(10.1).unwrap() - 1.1).abs() < 1e-9);
        c.observe(obs(1.2, false, 10.2), 1.0);
        assert_eq!(c.position(13.0), Some(1.2), "paused: held");
        c.observe(obs(1.2, true, 20.0), 1.0);
        assert!((c.position(20.1).unwrap() - 1.3).abs() < 1e-9);
    }

    #[test]
    fn a_stalled_mixer_stops_the_clock_after_its_run_on() {
        let mut c = PlayClock::new(0.0, 1.0);
        c.observe(
            Observation {
                position: 3.0,
                moving: true,
                at: 0.0,
            },
            2.0,
        );
        assert!((c.position(0.1).unwrap() - 3.2).abs() < 1e-9, "at its rate");
        assert!((c.position(5.0).unwrap() - (3.0 + 2.0 * MAX_RUN_ON)).abs() < 1e-9);
    }
}
