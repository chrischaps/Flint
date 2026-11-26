//! A suite in the browser's worklet lands its beat where kira here does
//! (#1216).
//!
//! The same suite (a click on every beat, a gain change on a bar line and
//! a low-pass sweep on the next) is rendered twice: offline, on kira here
//! (`render_offline`, which is what the desktop's session runs on), and on
//! the worklet's own `Mixer`, driven natively through a `Link` exactly as
//! the page drives it: the session's commands and stems applied before
//! each 128-frame render, the mixer's reports fed back so the session's
//! clock moves. Every click must start on the same sample in both, and the
//! bar-line changes must land on the same samples.

use flint_audio::clocked::RemoteMixer;
use flint_audio::remote::Link;
use flint_audio_worklet::Mixer;
use flint_music::manifest::BusDecl;
use flint_music::session::StemResolver;
use flint_music::{render_offline, EventScript, OfflineRenderConfig, SuiteManifest, SuiteSession};
use kira::sound::static_sound::StaticSoundData;
use kira::Frame;
use std::sync::Arc;

const SR: u32 = 48_000;
const QUANTUM: usize = 128;
/// 120 BPM: a beat is 24 000 samples, a bar 96 000.
const BEAT: usize = 24_000;
const BARS: usize = 4;

const SUITE: &str = r#"
schema_version = 0
[suite]
id = "parity"
title = "Parity"
[audio]
sample_rate = 48000
[[tempo]]
sample = 0
bpm = 120.0
time_signature = [4, 4]
[[sections]]
name = "a"
start_sample = 0
[buses.click]
file = "click.wav"
[buses.hum]
file = "hum.wav"
"#;

/// The second bar (beat 4, sample 96 000: a script counts bars from 1): the
/// click drops 12 dB at once. The third (beat 8): the hum's low-pass shuts.
const EVENTS: &str = r#"
schema_version = 0
[[events]]
at = "bar:2"
bus = "click"
action = "set_gain"
db = -12.0
ramp_ms = 0.0
[[events]]
at = "bar:3"
bus = "hum"
action = "set_lpf"
hz = 200.0
ramp_ms = 0.0
"#;

/// The click: 48 frames of 0.5 on every beat. The hum: a quiet 6 kHz tone
/// that the low-pass at bar 2 takes away.
struct Stems;

impl StemResolver for Stems {
    fn load(&self, bus: &str, _: &BusDecl) -> flint_core::Result<Option<StaticSoundData>> {
        let len = (BARS + 1) * 4 * BEAT;
        let frames: Arc<[Frame]> = (0..len)
            .map(|i| match bus {
                "click" => Frame::from_mono(if i % BEAT < 48 { 0.5 } else { 0.0 }),
                _ => Frame::from_mono(
                    (std::f64::consts::TAU * 6_000.0 * i as f64 / SR as f64).sin() as f32 * 0.05,
                ),
            })
            .collect();
        Ok(Some(StaticSoundData {
            sample_rate: SR,
            frames,
            settings: Default::default(),
            slice: None,
        }))
    }
}

fn suite() -> (SuiteManifest, EventScript) {
    (
        SuiteManifest::parse(SUITE).unwrap(),
        EventScript::parse(EVENTS).unwrap(),
    )
}

/// kira here, offline: the left channel from suite sample 0.
fn kira_here() -> Vec<f32> {
    let (manifest, events) = suite();
    let cfg = OfflineRenderConfig {
        duration_samples: (BARS * 4 * BEAT) as i64,
        chunk_frames: QUANTUM,
    };
    let render = render_offline(&manifest, &Stems, &events, &cfg, |_, _| {}).unwrap();
    render.samples.iter().step_by(2).copied().collect()
}

/// The worklet's mixer, through a link: the left channel from suite
/// sample 0, and the clock the session read back at the end.
fn in_the_worklet() -> (Vec<f32>, i64) {
    let (manifest, events) = suite();
    let link = Link::default();
    let mut mixer = RemoteMixer::new(link.clone());
    let mut session = SuiteSession::start(&manifest, &mut mixer, &Stems, None).unwrap();
    for ev in &events.events {
        session.scheduler.push(ev.clone(), &session.conductor);
    }
    let mut worklet = Mixer::new(SR).unwrap();
    let preroll = session.preroll_samples() as usize;
    let total = preroll + BARS * 4 * BEAT;
    let mut left = Vec::with_capacity(total);
    while left.len() < total {
        session.pump();
        // What the page posts: the sounds first, then the batch
        let out = link.take();
        for load in &out.loads {
            worklet.load_sound(load.id, load.sample_rate, load.channels, &load.samples);
        }
        worklet.apply(&out.batch);
        left.extend_from_slice(&worklet.render(QUANTUM)[..QUANTUM]);
        // ...and what it posts back, stamped with the audio time
        let at = left.len() as f64 / SR as f64;
        link.receive_report(at, worklet.report());
    }
    assert_eq!(worklet.errors, 0, "every command applied");
    let clock = session.raw_clock_sample();
    left.drain(..preroll);
    left.truncate(BARS * 4 * BEAT);
    (left, clock)
}

/// Where each click starts: the first sample over half its level after a
/// quiet stretch.
fn onsets(left: &[f32], level: f32) -> Vec<usize> {
    let mut found = Vec::new();
    let mut quiet = usize::MAX;
    for (i, s) in left.iter().enumerate() {
        if s.abs() > level {
            if quiet > 1000 {
                found.push(i);
            }
            quiet = 0;
        } else {
            quiet = quiet.saturating_add(1);
        }
    }
    found
}

fn rms(s: &[f32]) -> f32 {
    (s.iter().map(|x| x * x).sum::<f32>() / s.len() as f32).sqrt()
}

#[test]
fn the_worklets_beat_lands_on_the_samples_kira_here_lands_it_on() {
    let here = kira_here();
    let (there, clock) = in_the_worklet();
    assert_eq!(here.len(), there.len());

    // Every beat of the first bar at full level, the rest 12 dB down (0.125
    // peak over a hum of 0.05): count over 0.1
    let a = onsets(&here, 0.1);
    let b = onsets(&there, 0.1);
    assert_eq!(a, b, "the same samples, beat for beat");
    // kira starts a clock-timed sound at the start of the chunk its tick
    // falls in, so everything lands a quantum early, here and there alike:
    // the first click starts in the count-in's last chunk (cut off with
    // it), and each later one 128 samples before its beat, up to the one
    // after the render's end (a sample later where the click is quieter
    // than the threshold's first crossing)
    assert_eq!(a.len(), BARS * 4, "a click on every later beat: {a:?}");
    for (n, at) in a.iter().enumerate() {
        let beat = (n + 1) * BEAT - QUANTUM;
        assert!(*at == beat || *at == beat + 1, "beat {}: {at}", n + 1);
    }

    // The gain change at the second bar (beat 4) lands alike in both
    let peak = |s: &[f32], beat: usize| {
        s[beat * BEAT - QUANTUM..beat * BEAT - QUANTUM + 48]
            .iter()
            .fold(0f32, |m, x| m.max(x.abs()))
    };
    // (the worklet's main track has the engine's master low-pass, open at
    // 20 kHz, which the offline render doesn't: it takes a few percent off
    // a click's edge, so compare each render's step, not its level)
    // (and beat 4's click, started a quantum early, plays out before the
    // change: a change lands on its tick, gliding across that chunk)
    let step = |s: &[f32], beat: usize| peak(s, beat) / peak(s, beat - 1);
    for beat in [4, 5, 6] {
        let (h, t) = (step(&here, beat), step(&there, beat));
        assert!(
            (h - t).abs() < 0.02,
            "the step at beat {beat}: {h} here, {t} there"
        );
    }
    assert!(step(&there, 5) < 0.5, "12 dB down: {}", step(&there, 5));
    assert!((step(&there, 6) - 1.0).abs() < 0.02, "and held");

    // The hum's low-pass at beat 8 (sample 192 000): loud before, gone
    // after, in both, between the clicks
    let gap = |s: &[f32], beat: usize| rms(&s[beat * BEAT + 2_000..beat * BEAT + 20_000]);
    for s in [&here, &there] {
        assert!(gap(s, 7) > 0.03, "the hum before beat 8: {}", gap(s, 7));
        assert!(gap(s, 8) < 0.003, "gone after: {}", gap(s, 8));
    }

    // The session read the worklet's clock: the whole render, less the
    // pre-roll, within the quantum reports lag by
    let rendered = (BARS * 4 * BEAT) as i64;
    assert!(
        (rendered - clock).abs() <= QUANTUM as i64,
        "clock {clock} vs {rendered}"
    );
}
