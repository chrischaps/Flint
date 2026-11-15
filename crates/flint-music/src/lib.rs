//! Music for Flint games: suite manifests, tempo maps, the conductor, stems
//! on buses, scheduled bus events, realtime and offline playback, and
//! latency calibration.
//!
//! A suite is a per-suite TOML manifest: a tempo map, sections on bar lines,
//! and stem buses, as many as the suite has, named by the manifest and mixed
//! in the order it declares them. An event script schedules bus changes
//! (gain, low-pass, detune) on the musical grid. [`SuiteSession`] plays a
//! suite on a kira manager with every stem sample-locked to one clock tick;
//! [`render_offline`] renders the same session to samples, deterministically.
//!
//! This is the general layer. A rhythm game's contract (the fixed six-bus
//! layout, beatmap charts, judgment, coherence, the degradation ladder,
//! reintegration seams, replay and haptics) is `flint-rhythm`, which is
//! built on this crate and adds its own rules to a manifest's validation.

pub mod analysis;
pub mod clock_bridge;
pub mod conductor;
/// TOML plumbing shared with the rhythm layer's formats. Not a stable API.
#[doc(hidden)]
pub mod config_toml;
pub mod event_script;
pub mod latency;
pub mod manifest;
pub mod mixer;
pub mod offline;
pub mod probe;
pub mod scheduler;
pub mod session;
pub mod status;
pub mod tempo;
pub mod validate;

pub use clock_bridge::{BridgeStats, ClockBridge};
pub use conductor::{Conductor, Grid, MusicalPosition};
pub use event_script::EventScript;
pub use latency::{
    latest_calibration_ms, latest_latency_ms, resolve_timing_offsets, TimingOffsets,
};
pub use manifest::SuiteManifest;
pub use mixer::{BusMixer, BusState, StemBus};
pub use offline::{render_offline, render_offline_with, write_wav, OfflineRenderConfig};
pub use scheduler::{BusAction, EventTime, FiredEvent, ScheduledEvent, Scheduler};
pub use session::{RealtimePlayer, StemResolver, SuiteSession};
pub use tempo::TempoMap;
pub use validate::{validate_manifest, validate_manifest_assets, Issue};

/// The one schema version this build understands.
pub const SCHEMA_VERSION: i64 = 0;
