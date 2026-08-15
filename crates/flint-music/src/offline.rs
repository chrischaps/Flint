//! Deterministic offline rendering: the same `SuiteSession` that plays live,
//! driven through a custom kira backend into a caller-visible buffer, faster
//! than realtime.
//!
//! Determinism: single thread, no audio device, no wall clock. All time flows
//! from `Renderer::process` calls in fixed-size chunks; scheduler commands are
//! issued between chunks and consumed at the next `on_start_processing`, so
//! command-to-audio ordering is fixed and two renders of the same inputs are
//! bit-identical. kira advances clocks once per internal chunk, so scheduled
//! events land with chunk granularity — render with a small `chunk_frames`
//! (the default 128, or less) for tight alignment asserts.

use crate::conductor::MusicalPosition;
use crate::event_script::EventScript;
use crate::manifest::SuiteManifest;
use crate::mixer::BusMixer;
use crate::session::{StemResolver, SuiteSession};
use flint_core::{FlintError, Result};
use kira::backend::{Backend, Renderer};
use kira::{AudioManager, AudioManagerSettings};
use std::path::Path;

/// A kira backend that hands the [`Renderer`] to the driving loop instead of
/// an audio device (kira's own `MockBackend` keeps its frames private).
/// Reach it through `AudioManager::backend_mut`.
pub struct OfflineBackend {
    renderer: Option<Renderer>,
}

/// Settings: just the sample rate the renderer should run at (the manifest's).
#[derive(Debug, Clone, Copy)]
pub struct OfflineBackendSettings {
    pub sample_rate: u32,
}

impl OfflineBackend {
    /// Advance the renderer by one chunk into `out` (interleaved stereo).
    fn process_chunk(&mut self, out: &mut [f32]) {
        let renderer = self.renderer.as_mut().expect("backend not started");
        renderer.on_start_processing();
        renderer.process(out, 2);
    }
}

impl Backend for OfflineBackend {
    type Settings = OfflineBackendSettings;
    type Error = std::convert::Infallible;

    fn setup(
        settings: Self::Settings,
        _internal_buffer_size: usize,
    ) -> std::result::Result<(Self, u32), Self::Error> {
        Ok((Self { renderer: None }, settings.sample_rate))
    }

    fn start(&mut self, renderer: Renderer) -> std::result::Result<(), Self::Error> {
        self.renderer = Some(renderer);
        Ok(())
    }
}

pub struct OfflineRenderConfig {
    /// Suite samples to render (from sample 0; pre-roll is rendered and
    /// discarded automatically, so output index = suite sample).
    pub duration_samples: i64,
    /// Frames per processing chunk = kira's internal buffer size. Scheduled
    /// events land with this granularity; smaller = tighter, slower.
    pub chunk_frames: usize,
}

impl Default for OfflineRenderConfig {
    fn default() -> Self {
        Self {
            duration_samples: 0,
            chunk_frames: 128,
        }
    }
}

/// Render a scripted session to interleaved stereo f32, deterministic and
/// faster than realtime. The observer runs once per chunk (after the
/// scheduler pump, before the audio for that chunk) — use it for status
/// output or position logging; return values are collected by the caller.
pub fn render_offline(
    manifest: &SuiteManifest,
    stems: &dyn StemResolver,
    script: &EventScript,
    cfg: &OfflineRenderConfig,
    mut observer: impl FnMut(&MusicalPosition, &BusMixer),
) -> Result<Vec<f32>> {
    if cfg.duration_samples <= 0 {
        return Err(FlintError::AudioError(
            "offline render: duration_samples must be positive".into(),
        ));
    }
    let settings = AudioManagerSettings::<OfflineBackend> {
        capacities: Default::default(),
        main_track_builder: Default::default(),
        internal_buffer_size: cfg.chunk_frames,
        backend_settings: OfflineBackendSettings {
            sample_rate: manifest.sample_rate,
        },
    };
    let mut manager = AudioManager::<OfflineBackend>::new(settings)
        .map_err(|e| FlintError::AudioError(format!("offline backend: {e}")))?;

    let mut session = SuiteSession::start(manifest, &mut manager, stems, None)?;
    for ev in &script.events {
        session.scheduler.push(ev.clone(), &session.conductor);
    }

    let preroll = session.preroll_samples() as i64;
    let total_frames = (preroll + cfg.duration_samples) as usize;
    let mut out = Vec::with_capacity(total_frames * 2);
    let mut chunk = vec![0.0f32; cfg.chunk_frames * 2];

    let mut rendered = 0usize;
    while rendered < total_frames {
        session.pump();
        let pos = session
            .conductor
            .position_at_sample(rendered as i64 - preroll);
        observer(&pos, &session.mixer);

        let frames = cfg.chunk_frames.min(total_frames - rendered);
        let slice = &mut chunk[..frames * 2];
        manager.backend_mut().process_chunk(slice);
        out.extend_from_slice(slice);
        rendered += frames;
    }

    // Drop the pre-roll so output index 0 = suite sample 0.
    out.drain(..preroll as usize * 2);
    Ok(out)
}

/// Write interleaved stereo f32 to a 32-bit float WAV.
pub fn write_wav(path: &Path, interleaved: &[f32], sample_rate: u32) -> Result<()> {
    let spec = hound::WavSpec {
        channels: 2,
        sample_rate,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut writer = hound::WavWriter::create(path, spec)
        .map_err(|e| FlintError::AudioError(format!("wav create {}: {e}", path.display())))?;
    for &s in interleaved {
        writer
            .write_sample(s)
            .map_err(|e| FlintError::AudioError(format!("wav write: {e}")))?;
    }
    writer
        .finalize()
        .map_err(|e| FlintError::AudioError(format!("wav finalize: {e}")))?;
    Ok(())
}
