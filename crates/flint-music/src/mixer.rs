//! The stem mixer: runtime gain, low-pass, and detune per bus.
//!
//! Every bus the manifest declares gets a stem bus (a track with a low-pass,
//! `flint_audio::clocked`), in the order it declares them, including silent
//! ones, which simply have no sound playing. Silent buses stay addressable
//! so a layer over this one (buses entering after a timeline jump, degraded
//! alternates) can bring material in without rebuilding the mixer.
//!
//! The buses are made by a [`ClockedMixer`] (#1216): kira here, or the
//! browser's worklet, whose handles are commands. Handles are write-only,
//! so each bus keeps shadow state of the last *commanded* values for
//! introspection. During a ramp the audible value lags the shadow value by
//! design; the readout reports intent, not the DSP state.

use flint_audio::clocked::{
    ClockedMixer, MixClock, Ramp, Start, Stem, StemBus as Track, StemPlay, StemVoice,
};
use flint_core::{FlintError, Result};

/// An error from the mixer, said of what it was making.
fn named(what: String, e: FlintError) -> FlintError {
    FlintError::AudioError(format!("{what}: {e}"))
}

/// Cutoff meaning "filter open" — matches the spike and Milestone 0.
pub const LPF_OPEN_HZ: f64 = flint_audio::clocked::CUTOFF_OPEN_HZ;

/// Last commanded values for one bus (shadow state; see module docs).
#[derive(Debug, Clone, Copy)]
pub struct BusState {
    pub gain_db: f32,
    pub lpf_hz: f64,
    pub detune_semitones: f64,
    /// Whether this bus has a stem loaded (silent buses: false).
    pub playing: bool,
    /// Last commanded bus↔alternate crossfade weight (0 = bus, 1 =
    /// alternate; ADR 0032). Always 0 on buses without an alternate.
    pub alternate_mix: f64,
}

impl Default for BusState {
    fn default() -> Self {
        Self {
            gain_db: 0.0,
            lpf_hz: LPF_OPEN_HZ,
            detune_semitones: 0.0,
            playing: false,
            alternate_mix: 0.0,
        }
    }
}

/// Volume floor for the muted half of a bus↔alternate crossfade — matches
/// the rhythm layer's ladder silence convention (`flint_rhythm::ladder::DROP_DB`).
const ALT_MUTED_DB: f32 = -80.0;

/// Linear crossfade weight → dB (floor at [`ALT_MUTED_DB`]).
fn mix_db(weight: f64) -> f32 {
    if weight <= 1e-4 {
        ALT_MUTED_DB
    } else {
        (20.0 * weight.log10()).max(ALT_MUTED_DB as f64) as f32
    }
}

/// A composed degraded alternate riding its bus's track (ADR 0032): a
/// full-length stem, authored silent outside its manifest span, started at
/// suite start muted — sample-locked by construction (the shared-tick
/// start), crossfaded in per-sound volume when a ladder rung engages.
struct AlternateVoice {
    sound: StemVoice,
    data: Stem,
    from_sample: i64,
    to_sample: i64,
}

pub struct StemBus {
    pub name: String,
    // Declared before the stems: a bus goes before the sounds it plays.
    track: Track,
    sound: Option<StemVoice>,
    /// The stem, retained so a timeline jump can re-play it at a new
    /// position (kira's data is Arc-backed; the worklet's is sent once).
    /// `None` for silent buses.
    data: Option<Stem>,
    alternate: Option<AlternateVoice>,
    pub state: BusState,
}

impl StemBus {
    /// Set bus gain in dB. `ramp.start` may be a future clock tick — that
    /// is the scheduling hook every grid-aligned event uses.
    pub fn set_gain(&mut self, db: f32, ramp: Ramp) {
        self.track.set_volume(db, ramp);
        self.state.gain_db = db;
    }

    /// Immediate gain set with the default ramp (10 ms), for callers (CLI
    /// harnesses) with a plain fade.
    pub fn set_gain_now(&mut self, db: f32) {
        self.set_gain(db, Ramp::default());
    }

    /// Set the low-pass cutoff in Hz ([`LPF_OPEN_HZ`] = effectively open).
    pub fn set_lpf(&mut self, hz: f64, ramp: Ramp) {
        self.track.set_cutoff(hz, ramp);
        self.state.lpf_hz = hz;
    }

    /// Detune in semitones via playback rate (per-sound — kira has no
    /// per-track pitch; see ADR 0001). Warble only: any nonzero detune breaks
    /// sample-lock against the other stems while active, so offline alignment
    /// asserts only hold at detune 0. No-op with a warning on silent buses.
    /// An alternate voice warbles identically — whichever half of the
    /// crossfade is audible, the rung's wobble is on it.
    pub fn set_detune(&mut self, semitones: f64, ramp: Ramp) {
        match &mut self.sound {
            Some(sound) => {
                let rate = 2f64.powf(semitones / 12.0);
                sound.set_rate(rate, ramp);
                if let Some(alt) = &mut self.alternate {
                    alt.sound.set_rate(rate, ramp);
                }
                self.state.detune_semitones = semitones;
            }
            None => tracing::warn!("set_detune on silent bus '{}' ignored", self.name),
        }
    }

    /// Crossfade between this bus's stem and its composed alternate
    /// (0 = bus, 1 = alternate; ADR 0032), via per-sound volumes so the
    /// track's ladder trim and LPF apply to both halves equally. Span-gated:
    /// outside the alternate's authored span the mix is forced to 0 (the
    /// alternate is silent there by authoring, and the bus must not duck
    /// against nothing). No-op when the bus has no alternate.
    ///
    /// Per-sound volume is also the entry-envelope channel after a timeline
    /// jump: a caller ramping the buses back in owns it until the ramp ends,
    /// and nothing else may write it meanwhile.
    pub fn set_alternate_mix(&mut self, mix: f64, now_sample: i64, ramp: Ramp) {
        let Some(alt) = &mut self.alternate else {
            return;
        };
        let in_span = (alt.from_sample..alt.to_sample).contains(&now_sample);
        let m = if in_span { mix.clamp(0.0, 1.0) } else { 0.0 };
        if let Some(sound) = &mut self.sound {
            sound.set_volume(mix_db(1.0 - m), ramp);
        }
        alt.sound.set_volume(mix_db(m), ramp);
        self.state.alternate_mix = m;
    }

    /// Whether this bus carries a composed alternate.
    pub fn has_alternate(&self) -> bool {
        self.alternate.is_some()
    }

    /// The alternate's authored span `[from, to)`, if one is attached.
    pub fn alternate_span(&self) -> Option<(i64, i64)> {
        self.alternate
            .as_ref()
            .map(|a| (a.from_sample, a.to_sample))
    }

    pub fn sound(&self) -> Option<&StemVoice> {
        self.sound.as_ref()
    }

    /// Fade this bus's playing sound to silence and stop it. The ramp's
    /// start and length are the jump's authored fade envelope: this is how
    /// a timeline jump leaves the old timeline without a hard cut.
    pub fn stop(&mut self, fade: Ramp) {
        if let Some(sound) = &mut self.sound {
            sound.stop(fade);
            self.state.playing = false;
        }
        if let Some(alt) = &mut self.alternate {
            alt.sound.stop(fade);
        }
    }

    /// Play this bus's retained stem again from `start_position_samples`
    /// (a position within the stem = a suite sample, since stems start at
    /// suite sample 0), scheduled at `start_at`, with an initial per-sound
    /// volume of `entry_db` (the bus's entry level after a jump: 0 for a
    /// bus that leads back in, deep negative for one that ramps back in). Replaces the old
    /// handle; detune resets to 0, which re-establishes sample-lock.
    /// Track-level gain and LPF carry over untouched. No-op on silent buses.
    pub fn replay_at(
        &mut self,
        start_position_samples: u64,
        start_at: Start,
        entry_db: f32,
    ) -> Result<()> {
        let Some(data) = &self.data else {
            return Ok(());
        };
        let handle = self
            .track
            .play(
                data,
                StemPlay {
                    from_frame: start_position_samples,
                    start: start_at,
                    db: entry_db,
                    ..Default::default()
                },
            )
            .map_err(|e| named(format!("bus '{}' replay", self.name), e))?;
        self.sound = Some(handle);
        self.state.playing = true;
        self.state.detune_semitones = 0.0;
        // The alternate re-plays from the same tick, muted — the crossfade
        // state resets to bus-active; the world re-enters clean (ADR 0032).
        if let Some(alt) = &mut self.alternate {
            let handle = self
                .track
                .play(
                    &alt.data,
                    StemPlay {
                        from_frame: start_position_samples,
                        start: start_at,
                        db: ALT_MUTED_DB,
                        ..Default::default()
                    },
                )
                .map_err(|e| named(format!("bus '{}' alternate replay", self.name), e))?;
            alt.sound = handle;
            self.state.alternate_mix = 0.0;
        }
        Ok(())
    }

    /// Set the *per-sound* volume (independent of the track gain the ladder
    /// drives). A ramp back in after a timeline jump lives here: one
    /// clock-timed tween per bus from its entry level to full, so entry
    /// envelopes never fight the track's trims. No-op on silent buses.
    pub fn set_sound_volume(&mut self, db: f32, ramp: Ramp) {
        if let Some(sound) = &mut self.sound {
            sound.set_volume(db, ramp);
        }
    }

    /// Play an extra one-shot of this bus's retained stem as a *cue* — a
    /// gesture outside the sample-locked timeline (a pickup that winds up
    /// into a jump's re-entry). The caller owns the returned handle and commands its
    /// tweens/stop; the bus's own `sound` slot is untouched. `Ok(None)` on
    /// silent buses.
    pub fn play_cue(
        &mut self,
        start_position_samples: u64,
        start_at: Start,
        volume_db: f32,
        rate_semitones: f64,
    ) -> Result<Option<StemVoice>> {
        let Some(data) = &self.data else {
            return Ok(None);
        };
        let handle = self
            .track
            .play(
                data,
                StemPlay {
                    from_frame: start_position_samples,
                    start: start_at,
                    db: volume_db,
                    rate: 2f64.powf(rate_semitones / 12.0),
                },
            )
            .map_err(|e| named(format!("bus '{}' cue", self.name), e))?;
        Ok(Some(handle))
    }
}

/// Every bus of the suite, in the manifest's declared order.
pub struct BusMixer {
    buses: Vec<StemBus>,
}

impl BusMixer {
    /// Build one stem bus per bus on `clock` and schedule each provided
    /// stem to start at `start_at` (one shared clock tick = sample-locked
    /// start). `stems` pairs each bus name with its stem (made by the same
    /// `mixer`'s [`ClockedMixer::add_stem`]), `None` for silent.
    pub fn build(
        mixer: &mut dyn ClockedMixer,
        clock: &MixClock,
        stems: Vec<(String, Option<Stem>)>,
        start_at: Start,
    ) -> Result<Self> {
        let mut buses = Vec::new();
        for (name, data) in stems {
            let mut track = mixer
                .add_bus(clock)
                .map_err(|e| named(format!("bus '{name}'"), e))?;
            let sound = match &data {
                Some(data) => Some(
                    track
                        .play(
                            data,
                            StemPlay {
                                start: start_at,
                                ..Default::default()
                            },
                        )
                        .map_err(|e| named(format!("bus '{name}'"), e))?,
                ),
                None => None,
            };
            let state = BusState {
                playing: sound.is_some(),
                ..Default::default()
            };
            buses.push(StemBus {
                name,
                track,
                sound,
                data,
                alternate: None,
                state,
            });
        }
        Ok(Self { buses })
    }

    /// Attach a composed degraded alternate to a bus (ADR 0032): the
    /// full-length stem starts muted at the same shared clock time as
    /// everything else (sample-locked by construction), on the bus's own
    /// track so ladder LPF/trim hit both crossfade halves. Call between
    /// [`BusMixer::build`] and the clock start.
    pub fn attach_alternate(
        &mut self,
        bus_name: &str,
        data: Stem,
        from_sample: i64,
        to_sample: i64,
        start_at: Start,
    ) -> Result<()> {
        let bus = self.bus_mut(bus_name).ok_or_else(|| {
            FlintError::AudioError(format!("alternate for unknown bus '{bus_name}'"))
        })?;
        if bus.alternate.is_some() {
            return Err(FlintError::AudioError(format!(
                "bus '{bus_name}' already has an alternate (budget is one per bus)"
            )));
        }
        let sound = bus
            .track
            .play(
                &data,
                StemPlay {
                    start: start_at,
                    db: ALT_MUTED_DB,
                    ..Default::default()
                },
            )
            .map_err(|e| named(format!("bus '{bus_name}' alternate"), e))?;
        bus.alternate = Some(AlternateVoice {
            sound,
            data,
            from_sample,
            to_sample,
        });
        Ok(())
    }

    pub fn bus_mut(&mut self, name: &str) -> Option<&mut StemBus> {
        self.buses.iter_mut().find(|b| b.name == name)
    }

    pub fn buses(&self) -> impl Iterator<Item = &StemBus> {
        self.buses.iter()
    }

    pub fn states(&self) -> impl Iterator<Item = (&str, &BusState)> {
        self.buses.iter().map(|b| (b.name.as_str(), &b.state))
    }

    /// Whether every loaded stem has finished playing.
    pub fn all_stopped(&self) -> bool {
        self.buses
            .iter()
            .filter_map(|b| b.sound.as_ref())
            .all(StemVoice::is_stopped)
    }

    /// Fade every playing stem to silence and stop it (see [`StemBus::stop`]).
    pub fn stop_all(&mut self, fade: Ramp) {
        for bus in &mut self.buses {
            bus.stop(fade);
        }
    }

    /// Re-play every non-silent stem from `start_position_samples`, all
    /// scheduled at the same `start_at` — one shared clock time is what makes
    /// the re-entry sample-locked, exactly like the initial start.
    /// `entry_db` gives each bus its entry level by name.
    pub fn replay_all_at(
        &mut self,
        start_position_samples: u64,
        start_at: Start,
        entry_db: &dyn Fn(&str) -> f32,
    ) -> Result<()> {
        for bus in &mut self.buses {
            let db = entry_db(&bus.name);
            bus.replay_at(start_position_samples, start_at, db)?;
        }
        Ok(())
    }

    /// Max pairwise stem position disagreement, in seconds. Positions are
    /// polled back-to-back; expect ~0 while all stems are playing.
    pub fn max_stem_skew(&self) -> f64 {
        let positions: Vec<f64> = self
            .buses
            .iter()
            .flat_map(|b| b.sound.iter().chain(b.alternate.as_ref().map(|a| &a.sound)))
            .map(|s| s.position())
            .collect();
        let mut max = 0.0f64;
        for i in 0..positions.len() {
            for j in (i + 1)..positions.len() {
                max = max.max((positions[i] - positions[j]).abs());
            }
        }
        max
    }
}
