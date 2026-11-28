//! What Float's mix costs, old and lean (#138).
//!
//!     cargo run --release -p flint-audio --example mix_bench
//!
//! Builds Float's mixer the way `AudioEngine` does (a low-passed main
//! track, music and sfx buses) on kira's mock backend, plays Float's
//! looping `audio_source`s on it and times the mix of one 2048-frame
//! WebAudio buffer, as the browser asks for one.
//!
//! - **old**: as before #138. Each placed loop on its own spatial track,
//!   and the weather beds and four songs mixed at volume 0 (-60 dB).
//! - **lean**: placed loops as panned voices on the bus (`crate::pan`), and
//!   loops at volume 0 paused (`AudioSync::cull_inaudible`).
//! - **lean, calm sea**: the wash loops at 0 too, as when the raft is still.
//!
//! Native timings are not the phone's, but the share saved carries over.

use flint_audio::engine::amplitude_to_db;
use flint_audio::pan::{volume_and_pan, Ears, Falloff};
use kira::backend::mock::{MockBackend, MockBackendSettings};
use kira::effect::filter::{FilterBuilder, FilterMode};
use kira::listener::ListenerHandle;
use kira::sound::static_sound::{StaticSoundData, StaticSoundHandle};
use kira::track::{SpatialTrackBuilder, SpatialTrackDistances, SpatialTrackHandle, TrackBuilder};
use kira::{AudioManager, AudioManagerSettings, Easing, Frame, Panning, Tween};
use std::sync::Arc;
use std::time::Instant;

const RATE: u32 = 44_100;
/// kira renders in blocks of this many frames (its default).
const BLOCK: usize = 128;
/// One WebAudio buffer (`WebAudioSettings::buffer_frames`).
const BUFFER: usize = 2048;

/// Float's looping sources (scenes/float.scene.toml): name, volume, where
/// (placed ones ride the raft's edges), music bus. Volumes are a calm day
/// between episodes: the wash loops follow the water, the rest are faded.
const LOOPS: &[(&str, f64, Option<[f32; 3]>, bool)] = &[
    ("ocean_ambience", 0.55, None, false),
    ("wind_ambience", 0.3, None, false),
    ("rain_ambience", 0.0, None, false),
    ("storm_howl", 0.0, None, false),
    ("music_day", 0.0, None, true),
    ("music_dusk", 0.0, None, true),
    ("music_night", 0.0, None, true),
    ("music_deep", 0.0, None, true),
    ("wash_front", 0.2, Some([0.0, 0.0, 1.15]), false),
    ("wash_back", 0.2, Some([0.0, 0.0, -1.15]), false),
    ("wash_right", 0.2, Some([1.2, 0.0, 0.0]), false),
    ("wash_left", 0.2, Some([-1.2, 0.0, 0.0]), false),
    ("foam_fizz", 0.1, Some([0.0, 0.0, 0.0]), false),
];

const FALLOFF: Falloff = Falloff {
    min: 1.0,
    max: 12.0,
    rolloff: flint_audio::engine::DEFAULT_ROLLOFF,
};

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Old,
    Lean,
    LeanCalm,
}

fn loop_data(seconds: f32, seed: u32) -> StaticSoundData {
    let mut x = seed.wrapping_mul(2_654_435_761) | 1;
    let frames: Arc<[Frame]> = (0..(seconds * RATE as f32) as usize)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            Frame::from_mono((x as f32 / u32::MAX as f32 - 0.5) * 0.2)
        })
        .collect();
    StaticSoundData {
        sample_rate: RATE,
        frames,
        settings: Default::default(),
        slice: None,
    }
}

struct Mix {
    manager: AudioManager<MockBackend>,
    _listener: ListenerHandle,
    _buses: Vec<kira::track::TrackHandle>,
    _tracks: Vec<SpatialTrackHandle>,
    _sounds: Vec<StaticSoundHandle>,
    live: usize,
}

fn build(mode: Mode) -> Mix {
    let mut settings = AudioManagerSettings::<MockBackend> {
        backend_settings: MockBackendSettings { sample_rate: RATE },
        ..Default::default()
    };
    settings.main_track_builder.add_effect(
        FilterBuilder::new()
            .mode(FilterMode::LowPass)
            .cutoff(20_000.0),
    );
    let mut manager = AudioManager::<MockBackend>::new(settings).unwrap();
    let ears = Ears {
        position: glam::Vec3::new(0.3, 1.1, 0.2),
        orientation: glam::Quat::from_rotation_y(0.7),
    };
    let listener = manager
        .add_listener(ears.position, ears.orientation)
        .unwrap();
    let mut music = manager.add_sub_track(TrackBuilder::new()).unwrap();
    let mut sfx = manager.add_sub_track(TrackBuilder::new()).unwrap();
    let (mut tracks, mut sounds, mut live) = (Vec::new(), Vec::new(), 0);
    let pause = Tween {
        duration: std::time::Duration::ZERO,
        ..Default::default()
    };
    for (i, &(_, volume, at, is_music)) in LOOPS.iter().enumerate() {
        let volume = if mode == Mode::LeanCalm && at.is_some() {
            0.0
        } else {
            volume
        };
        // Songs are minutes long; the rest are short loops.
        let data = loop_data(if is_music { 60.0 } else { 8.0 }, i as u32 + 1).loop_region(..);
        let bus = if is_music { &mut music } else { &mut sfx };
        let silent = volume <= flint_audio::sync::SILENT_VOLUME;
        let mut handle = match (at, mode) {
            (Some(at), Mode::Old) => {
                let mut track = bus
                    .add_spatial_sub_track(
                        listener.id(),
                        glam::Vec3::from(at),
                        SpatialTrackBuilder::new()
                            .distances(SpatialTrackDistances {
                                min_distance: FALLOFF.min,
                                max_distance: FALLOFF.max,
                            })
                            .attenuation_function(Some(Easing::OutPowf(FALLOFF.rolloff as f64))),
                    )
                    .unwrap();
                let h = track.play(data.volume(amplitude_to_db(volume))).unwrap();
                tracks.push(track);
                h
            }
            (Some(at), _) => {
                let (gain, pan) = volume_and_pan(ears.gains(glam::Vec3::from(at), &FALLOFF));
                bus.play(
                    data.volume(amplitude_to_db(volume * gain as f64))
                        .panning(Panning(pan)),
                )
                .unwrap()
            }
            (None, _) => bus.play(data.volume(amplitude_to_db(volume))).unwrap(),
        };
        if mode != Mode::Old && silent {
            handle.pause(pause);
        } else {
            live += 1;
        }
        sounds.push(handle);
    }
    Mix {
        manager,
        _listener: listener,
        _buses: vec![music, sfx],
        _tracks: tracks,
        _sounds: sounds,
        live,
    }
}

/// Mean ms to mix one WebAudio buffer, over `buffers` of them.
fn time(mix: &mut Mix, buffers: usize) -> f64 {
    let backend = mix.manager.backend_mut();
    let start = Instant::now();
    for _ in 0..buffers {
        for _ in 0..BUFFER / BLOCK {
            backend.on_start_processing();
            backend.process();
        }
    }
    start.elapsed().as_secs_f64() * 1000.0 / buffers as f64
}

fn main() {
    let modes = [
        (Mode::Old, "old"),
        (Mode::Lean, "lean"),
        (Mode::LeanCalm, "lean, calm sea"),
    ];
    // Interleaved, so the machine's moods fall on all three alike.
    let mut times = vec![Vec::new(); modes.len()];
    let mut live = vec![0; modes.len()];
    for _ in 0..9 {
        for (i, &(mode, _)) in modes.iter().enumerate() {
            let mut mix = build(mode);
            time(&mut mix, 20); // let kira pick up the plays and pauses
            times[i].push(time(&mut mix, 300));
            live[i] = mix.live;
        }
    }
    let old = median(&mut times[0]);
    println!("ms to mix one {BUFFER}-frame buffer (median of 9 runs of 300):");
    for (i, &(_, name)) in modes.iter().enumerate() {
        let t = median(&mut times[i]);
        println!(
            "  {name:<15} {t:6.3} ms  {:>3.0}% less  {} of {} loops mixed",
            (1.0 - t / old) * 100.0,
            live[i],
            LOOPS.len()
        );
    }
}

fn median(v: &mut [f64]) -> f64 {
    v.sort_by(f64::total_cmp);
    v[v.len() / 2]
}
