//! Placed sounds as plain panned voices (#138).
//!
//! A kira spatial track works out a sound's distance falloff and each ear's
//! share for every sample it mixes: on the Pixel 8 Pro's main thread that
//! was most of a 15 ms mix, and natively a spatial track costs about 14
//! times a plain voice (`examples/mix_bench.rs`). Here the same gains are
//! worked out once a frame, from the same formulas, and handed to an
//! ordinary sound as a volume and a constant-power pan, which kira applies
//! per sample for almost nothing. The listener and the sources move a few
//! centimetres a frame, and each change is tweened over the frame, so the
//! ear can't tell. A stereo file keeps its stereo instead of being folded
//! to mono, the one difference; Float's are all mono.

use glam::{Quat, Vec3};
use std::f32::consts::FRAC_PI_8;

/// Below this kira's falloff is silence, and `amplitude_to_db` stops here.
const SILENCE_DB: f32 = -60.0;

/// kira's half-distance between the ears, metres.
const EAR_DISTANCE: f32 = 0.1;

/// How much a spatial track's side matters, kira's default: each ear hears
/// at least a quarter of the sound, however it faces.
const STRENGTH: f32 = 0.75;

/// A placed sound's falloff: full volume within `min` metres of the
/// listener, silent past `max`, `-60·r^rolloff` dB between, with `r` the
/// listener's share of the way from `min` to `max` (see
/// [`DEFAULT_ROLLOFF`](crate::engine::DEFAULT_ROLLOFF)).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Falloff {
    pub min: f32,
    pub max: f32,
    pub rolloff: f32,
}

impl Falloff {
    /// The amplitude at `distance` metres, as kira's spatial track has it.
    pub fn amplitude(&self, distance: f32) -> f32 {
        let span = self.max - self.min;
        let r = if span > 0.0 {
            ((distance.clamp(self.min, self.max) - self.min) / span) as f64
        } else if distance > self.max {
            1.0
        } else {
            0.0
        };
        // kira: Easing::OutPowf(rolloff) of (1 - r), from -60 dB to 0 dB.
        let rolloff = self.rolloff.max(0.01) as f64;
        let db = SILENCE_DB * r.powf(rolloff) as f32;
        if db <= SILENCE_DB {
            0.0
        } else {
            10f32.powf(db / 20.0)
        }
    }
}

/// The listener's ears: where its head is and which way it faces.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ears {
    pub position: Vec3,
    pub orientation: Quat,
}

impl Ears {
    /// The left and right gains kira's spatial track gives a mono sound at
    /// `source`: the falloff, times each ear's `0.25 + 0.75·(cos + 1) / 2`
    /// towards it, each ear turned π/8 forward of straight out.
    pub fn gains(&self, source: Vec3, falloff: &Falloff) -> (f32, f32) {
        let amplitude = falloff.amplitude((self.position - source).length());
        if amplitude == 0.0 {
            return (0.0, 0.0);
        }
        let o = self.orientation;
        let left_ear = self.position + o * (Vec3::NEG_X * EAR_DISTANCE);
        let right_ear = self.position + o * (Vec3::X * EAR_DISTANCE);
        let left_dir = o * (Quat::from_rotation_y(-FRAC_PI_8) * Vec3::NEG_X);
        let right_dir = o * (Quat::from_rotation_y(FRAC_PI_8) * Vec3::X);
        let ear = |dir: Vec3, at: Vec3| {
            let facing = (dir.dot((source - at).normalize_or_zero()) + 1.0) / 2.0;
            1.0 - STRENGTH + STRENGTH * facing
        };
        (
            amplitude * ear(left_dir, left_ear),
            amplitude * ear(right_dir, right_ear),
        )
    }
}

/// A sound's amplitude and kira `Panning` (-1 left to 1 right) that give
/// these ear gains to a mono sound. kira pans at constant power, scaled so
/// the centre is unchanged: left `a·√2·√(1-m)`, right `a·√2·√m`, with
/// `m = (pan + 1) / 2`.
pub fn volume_and_pan((left, right): (f32, f32)) -> (f32, f32) {
    let power = left * left + right * right;
    if power <= 0.0 {
        return (0.0, 0.0);
    }
    ((power / 2.0).sqrt(), 2.0 * right * right / power - 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    const FALLOFF: Falloff = Falloff {
        min: 1.0,
        max: 12.0,
        rolloff: 2.0,
    };

    /// kira's own panning, as `Frame::panned` does it.
    fn kira_pan(amplitude: f32, pan: f32) -> (f32, f32) {
        let m = (pan.clamp(-1.0, 1.0) + 1.0) * 0.5;
        let s = std::f32::consts::SQRT_2;
        (amplitude * (1.0 - m).sqrt() * s, amplitude * m.sqrt() * s)
    }

    #[test]
    fn the_falloff_is_kiras() {
        assert_eq!(FALLOFF.amplitude(0.5), 1.0);
        assert_eq!(FALLOFF.amplitude(1.0), 1.0);
        assert_eq!(FALLOFF.amplitude(12.0), 0.0);
        assert_eq!(FALLOFF.amplitude(40.0), 0.0);
        // Halfway: -60 · 0.5² = -15 dB.
        let half = FALLOFF.amplitude(6.5);
        assert!((half - 10f32.powf(-15.0 / 20.0)).abs() < 1e-5, "{half}");
    }

    #[test]
    fn a_pan_gives_back_the_gains_it_was_made_from() {
        for gains in [(0.8, 0.2), (0.3, 0.3), (0.0, 0.5), (0.61, 0.0), (1.0, 1.0)] {
            let (a, p) = volume_and_pan(gains);
            let (l, r) = kira_pan(a, p);
            assert!(
                (l - gains.0).abs() < 1e-5 && (r - gains.1).abs() < 1e-5,
                "{gains:?} -> {l} {r}"
            );
        }
        assert_eq!(volume_and_pan((0.0, 0.0)), (0.0, 0.0));
    }

    /// Keeps what the main track mixed.
    struct Capture(std::sync::Arc<std::sync::Mutex<Vec<kira::Frame>>>);

    impl kira::effect::EffectBuilder for Capture {
        type Handle = ();
        fn build(self) -> (Box<dyn kira::effect::Effect>, ()) {
            (Box::new(self), ())
        }
    }

    impl kira::effect::Effect for Capture {
        fn process(&mut self, input: &mut [kira::Frame], _dt: f64, _info: &kira::info::Info) {
            self.0.lock().unwrap().extend_from_slice(input);
        }
    }

    /// The last block of a steady mono tone through kira's mixer, from a
    /// listener at `ears` hearing `source`: on a spatial track when `track`,
    /// else as a panned voice.
    fn render(ears: &Ears, source: Vec3, track: bool) -> Vec<kira::Frame> {
        use kira::backend::mock::{MockBackend, MockBackendSettings};
        use kira::sound::static_sound::StaticSoundData;
        let out = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let mut settings = kira::AudioManagerSettings::<MockBackend>::default();
        settings.backend_settings = MockBackendSettings { sample_rate: 1000 };
        settings.main_track_builder.add_effect(Capture(out.clone()));
        let mut manager = kira::AudioManager::<MockBackend>::new(settings).unwrap();
        let listener = manager
            .add_listener(ears.position, ears.orientation)
            .unwrap();
        let tone: std::sync::Arc<[kira::Frame]> = (0..1000)
            .map(|i| kira::Frame::from_mono(0.5 + 0.25 * (i as f32 * 0.37).sin()))
            .collect();
        let data = StaticSoundData {
            sample_rate: 1000,
            frames: tone,
            settings: Default::default(),
            slice: None,
        }
        .loop_region(..);
        let _keep = if track {
            let mut t = manager
                .add_spatial_sub_track(
                    listener.id(),
                    source,
                    kira::track::SpatialTrackBuilder::new()
                        .distances(kira::track::SpatialTrackDistances {
                            min_distance: FALLOFF.min,
                            max_distance: FALLOFF.max,
                        })
                        .attenuation_function(Some(kira::Easing::OutPowf(FALLOFF.rolloff as f64))),
                )
                .unwrap();
            t.play(data).unwrap();
            Some(t)
        } else {
            let (a, p) = volume_and_pan(ears.gains(source, &FALLOFF));
            manager
                .play(
                    data.volume(crate::engine::amplitude_to_db(a as f64))
                        .panning(kira::Panning(p)),
                )
                .unwrap();
            None
        };
        let backend = manager.backend_mut();
        for _ in 0..4 {
            backend.on_start_processing();
            backend.process();
        }
        let frames = out.lock().unwrap();
        frames[frames.len() - 64..].to_vec()
    }

    #[test]
    fn a_panned_voice_sounds_as_kiras_spatial_track_does() {
        let cases = [
            (
                Vec3::new(0.0, 1.6, 0.0),
                Quat::IDENTITY,
                Vec3::new(1.2, 0.0, 0.0),
            ),
            (
                Vec3::new(0.0, 1.6, 0.0),
                Quat::IDENTITY,
                Vec3::new(-0.4, 0.0, -1.15),
            ),
            (
                Vec3::new(2.0, 1.6, 1.0),
                Quat::from_rotation_y(0.9),
                Vec3::new(-3.0, 0.0, 4.0),
            ),
            (
                Vec3::new(0.0, 1.6, 0.0),
                Quat::from_euler(glam::EulerRot::YXZ, 2.4, -0.5, 0.0),
                Vec3::new(0.0, 0.0, 9.0),
            ),
            (
                Vec3::new(0.0, 1.6, 0.0),
                Quat::IDENTITY,
                Vec3::new(0.0, 1.6, 0.5),
            ),
        ];
        for (position, orientation, source) in cases {
            let ears = Ears {
                position,
                orientation,
            };
            let spatial = render(&ears, source, true);
            let panned = render(&ears, source, false);
            let peak = spatial
                .iter()
                .map(|f| f.left.abs().max(f.right.abs()))
                .fold(0.0, f32::max);
            assert!(peak > 0.0, "{source}: silent");
            for (s, p) in spatial.iter().zip(&panned) {
                assert!(
                    (s.left - p.left).abs() < 1e-4 && (s.right - p.right).abs() < 1e-4,
                    "{source}: track {s:?} panned {p:?}"
                );
            }
        }
    }

    #[test]
    fn a_sound_to_the_right_is_louder_in_the_right_ear() {
        let ears = Ears {
            position: Vec3::ZERO,
            orientation: Quat::IDENTITY,
        };
        let (l, r) = ears.gains(Vec3::new(3.0, 0.0, 0.0), &FALLOFF);
        // The far ear still hears a quarter or so (kira's 0.75 strength).
        assert!(r > 0.9 * FALLOFF.amplitude(3.0) && l < 0.4 * r, "{l} {r}");
        // Straight ahead (kira's listener faces -Z): both ears alike.
        let (l, r) = ears.gains(Vec3::new(0.0, 0.0, -3.0), &FALLOFF);
        assert!((l - r).abs() < 1e-5);
        // Turned to face it, the one on the right is ahead and centred.
        let turned = Ears {
            orientation: Quat::from_rotation_y(-std::f32::consts::FRAC_PI_2),
            ..ears
        };
        let (l, r) = turned.gains(Vec3::new(3.0, 0.0, 0.0), &FALLOFF);
        assert!((l - r).abs() < 1e-4, "{l} {r}");
        assert_eq!(ears.gains(Vec3::new(30.0, 0.0, 0.0), &FALLOFF), (0.0, 0.0));
    }
}
