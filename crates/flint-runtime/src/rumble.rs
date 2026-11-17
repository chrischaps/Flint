//! A general rumble any game can ask for (#748): both motors of a pad at one
//! strength for a while, the script function `rumble(pad, strength, seconds)`.
//!
//! It goes through gilrs's force-feedback server, which ticks every 50 ms, so
//! a buzz is rounded up to whole ticks and lasts at least one. That is fine
//! for a hit or a pickup and too coarse for a beat: a music session's
//! rumble is flint-input-capture's engine, stamped on the audio clock and
//! written to the motors directly (ADR 0025). While a session runs the player
//! has handed its pads to that engine's thread, so a host has no [`Gilrs`]
//! to rumble through and [`PadRumble::rumble`] isn't called.
//!
//! An effect stops the moment its handle drops, so [`PadRumble`] keeps each
//! one until its time is up; the host calls [`PadRumble::update`] once a
//! frame to let the finished ones go. With no pad, no force-feedback pad, no
//! backend (a headless build, the browser, Android) it rumbles nothing and
//! says so with `Ok(0)`.

use crate::pad::Gilrs;
use web_time::{Duration, Instant};

/// The longest rumble one call asks for; a longer one is cut to this.
pub const MAX_RUMBLE_SECONDS: f64 = 10.0;

/// gilrs's force-feedback tick (`gilrs::ff`'s `TICK_DURATION`, not public).
const TICK_MS: u64 = 50;

/// Which pads a rumble is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PadTarget {
    /// Every connected pad that can rumble.
    Every,
    /// One pad, by the index an input config's `gamepad = n` names.
    Index(u32),
}

impl PadTarget {
    /// A script's `pad`: negative is every pad, else that pad's index.
    pub fn from_script(pad: i64) -> Self {
        if pad < 0 {
            PadTarget::Every
        } else {
            PadTarget::Index(pad.min(u32::MAX as i64) as u32)
        }
    }

    pub fn matches(self, index: u32) -> bool {
        match self {
            PadTarget::Every => true,
            PadTarget::Index(i) => i == index,
        }
    }
}

/// One rumble, as asked for and made safe: strength in 0..=1 as a motor
/// magnitude, the length cut to [`MAX_RUMBLE_SECONDS`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RumbleRequest {
    pub target: PadTarget,
    /// Both motors' magnitude, 0..=`u16::MAX`.
    pub magnitude: u16,
    pub duration: Duration,
}

impl RumbleRequest {
    /// A NaN strength or length reads as 0, which stops.
    pub fn new(target: PadTarget, strength: f64, seconds: f64) -> Self {
        let strength = if strength.is_nan() {
            0.0
        } else {
            strength.clamp(0.0, 1.0)
        };
        let seconds = if seconds.is_nan() {
            0.0
        } else {
            seconds.clamp(0.0, MAX_RUMBLE_SECONDS)
        };
        RumbleRequest {
            target,
            magnitude: (strength * u16::MAX as f64).round() as u16,
            duration: Duration::from_secs_f64(seconds),
        }
    }

    /// No strength or no length: stop what plays on the target's pads.
    pub fn is_stop(&self) -> bool {
        self.magnitude == 0 || self.duration.is_zero()
    }

    /// How long the motors run: the length rounded up to whole 50 ms ticks.
    pub fn ticks(&self) -> u32 {
        let ms = self.duration.as_millis() as u64;
        ms.div_ceil(TICK_MS).max(1) as u32
    }

    /// When a handle for this rumble may drop without cutting it short: the
    /// ticks it runs plus one, for the server's next tick to stop it.
    pub fn hold_until(&self, now: Instant) -> Instant {
        now + Duration::from_millis((self.ticks() as u64 + 1) * TICK_MS)
    }
}

/// Why a rumble didn't start on a pad that should have taken it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RumbleError(pub String);

impl std::fmt::Display for RumbleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for RumbleError {}

struct Playing {
    pad: u32,
    until: Instant,
    /// Dropping it stops the motors.
    #[cfg(feature = "gamepad")]
    _effect: gilrs::ff::Effect,
}

/// The rumbles playing now, one per pad: a new one on a pad replaces the
/// one it had.
#[derive(Default)]
pub struct PadRumble {
    playing: Vec<Playing>,
}

impl PadRumble {
    pub fn new() -> Self {
        Self::default()
    }

    /// Start `request` on its pads, replacing what they were playing. A stop
    /// request ([`RumbleRequest::is_stop`]) only stops. Returns how many pads
    /// it started on: 0 with no force-feedback pad that matches.
    pub fn rumble(
        &mut self,
        gilrs: &mut Gilrs,
        request: RumbleRequest,
        now: Instant,
    ) -> Result<usize, RumbleError> {
        self.playing.retain(|p| !request.target.matches(p.pad));
        if request.is_stop() {
            return Ok(0);
        }
        #[cfg(feature = "gamepad")]
        {
            use gilrs::ff::{BaseEffect, BaseEffectType, EffectBuilder, Repeat, Replay, Ticks};
            let ticks = Ticks::from_ms(request.ticks() * TICK_MS as u32);
            let scheduling = Replay {
                play_for: ticks,
                ..Default::default()
            };
            let motor = |kind| BaseEffect {
                kind,
                scheduling,
                envelope: Default::default(),
            };
            let magnitude = request.magnitude;
            let pads: Vec<_> = gilrs
                .gamepads()
                .filter(|(id, pad)| {
                    pad.is_ff_supported() && request.target.matches(usize::from(*id) as u32)
                })
                .map(|(id, _)| id)
                .collect();
            let mut started = 0;
            let mut failed = None;
            for id in pads {
                let effect = EffectBuilder::new()
                    .add_effect(motor(BaseEffectType::Strong { magnitude }))
                    .add_effect(motor(BaseEffectType::Weak { magnitude }))
                    .repeat(Repeat::For(ticks))
                    .gamepads(&[id])
                    .finish(gilrs)
                    .and_then(|effect| effect.play().map(|()| effect));
                match effect {
                    Ok(effect) => {
                        self.playing.push(Playing {
                            pad: usize::from(id) as u32,
                            until: request.hold_until(now),
                            _effect: effect,
                        });
                        started += 1;
                    }
                    Err(e) => failed = Some(format!("pad {}: {e}", usize::from(id))),
                }
            }
            match failed {
                Some(e) if started == 0 => Err(RumbleError(e)),
                _ => Ok(started),
            }
        }
        #[cfg(not(feature = "gamepad"))]
        {
            let _ = now;
            match *gilrs {}
        }
    }

    /// Let go of the rumbles whose time is up. Once a frame.
    pub fn update(&mut self, now: Instant) {
        self.playing.retain(|p| p.until > now);
    }

    /// Stop every pad now (the pads are going to another owner, or away).
    pub fn stop_all(&mut self) {
        self.playing.clear();
    }

    /// How many pads are rumbling.
    pub fn playing(&self) -> usize {
        self.playing.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_negative_pad_is_every_pad() {
        assert_eq!(PadTarget::from_script(-1), PadTarget::Every);
        assert_eq!(PadTarget::from_script(0), PadTarget::Index(0));
        assert_eq!(PadTarget::from_script(3), PadTarget::Index(3));
        assert!(PadTarget::Every.matches(7));
        assert!(PadTarget::Index(2).matches(2));
        assert!(!PadTarget::Index(2).matches(1));
    }

    #[test]
    fn strength_and_length_are_clamped() {
        let full = RumbleRequest::new(PadTarget::Every, 4.0, 60.0);
        assert_eq!(full.magnitude, u16::MAX);
        assert_eq!(full.duration, Duration::from_secs(10));
        let half = RumbleRequest::new(PadTarget::Every, 0.5, 0.25);
        assert_eq!(half.magnitude, 32768);
        assert_eq!(half.duration, Duration::from_millis(250));
        assert!(!half.is_stop());
    }

    #[test]
    fn nothing_negative_or_nan_rumbles() {
        for (strength, seconds) in [
            (0.0, 1.0),
            (1.0, 0.0),
            (-1.0, 1.0),
            (1.0, -2.0),
            (f64::NAN, 1.0),
            (1.0, f64::NAN),
        ] {
            let r = RumbleRequest::new(PadTarget::Every, strength, seconds);
            assert!(r.is_stop(), "{strength} for {seconds}s should stop");
        }
    }

    #[test]
    fn a_rumble_runs_whole_ticks_and_is_held_one_more() {
        let blip = RumbleRequest::new(PadTarget::Every, 1.0, 0.01);
        assert_eq!(blip.ticks(), 1, "a blip still runs one tick");
        let r = RumbleRequest::new(PadTarget::Every, 1.0, 0.12);
        assert_eq!(r.ticks(), 3, "120 ms rounds up to 150");
        let now = Instant::now();
        assert_eq!(r.hold_until(now) - now, Duration::from_millis(200));
    }

    #[test]
    fn nothing_plays_until_a_pad_takes_it() {
        let mut rumble = PadRumble::new();
        assert_eq!(rumble.playing(), 0);
        rumble.update(Instant::now());
        rumble.stop_all();
        assert_eq!(rumble.playing(), 0);
    }

    /// With a backend and no pad (CI, most machines here), a rumble starts
    /// nowhere and isn't an error; with no backend there's no `Gilrs` to
    /// call it with, which is the host's `Option`.
    #[cfg(feature = "gamepad")]
    #[test]
    fn a_rumble_with_no_pad_starts_nowhere() {
        let Ok(mut gilrs) = Gilrs::new() else {
            return;
        };
        if gilrs.gamepads().next().is_some() {
            return; // a real pad: the hardware test is `rumble_a_connected_pad`
        }
        let mut rumble = PadRumble::new();
        let request = RumbleRequest::new(PadTarget::Every, 0.8, 0.2);
        assert_eq!(rumble.rumble(&mut gilrs, request, Instant::now()), Ok(0));
        assert_eq!(rumble.playing(), 0);
    }

    /// On a machine with a pad: `cargo test -p flint-runtime --features
    /// gamepad -- --ignored rumble_a_connected_pad --nocapture`. You should
    /// feel half a second at 60%.
    #[cfg(feature = "gamepad")]
    #[test]
    #[ignore = "needs a force-feedback gamepad"]
    fn rumble_a_connected_pad() {
        let mut gilrs = Gilrs::new().expect("a gamepad backend");
        for _ in 0..20 {
            while gilrs.next_event().is_some() {}
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let mut rumble = PadRumble::new();
        let request = RumbleRequest::new(PadTarget::Every, 0.6, 0.5);
        let started = rumble
            .rumble(&mut gilrs, request, Instant::now())
            .expect("rumble");
        println!("rumbling on {started} pad(s)");
        assert!(started > 0, "no force-feedback pad connected");
        assert_eq!(rumble.playing(), started);
        std::thread::sleep(std::time::Duration::from_millis(700));
        rumble.update(Instant::now());
        assert_eq!(rumble.playing(), 0, "the finished rumble is let go");
    }
}
